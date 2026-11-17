//! Payload 冷热分层存储。
//!
//! NodeId 目录常驻内存；已发布 raw JSON 可引用只读 mmap，新增和更新保存在堆增量层。
//! 解析结果进入有硬字节上限的 LRU cache，并通过 `Arc<Value>` 安全跨越淘汰边界。

use crate::error::{Result, TriviumError};
use crate::node::NodeId;
use crate::observability::PayloadMemoryStats;
use crate::storage::fs::robust_rename_and_sync;
use memmap2::Mmap;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const PAYLOAD_MAGIC: &[u8; 4] = b"TPLD";
const PAYLOAD_VERSION: u16 = 1;
const PAYLOAD_HEADER_SIZE: usize = 40;
const PAYLOAD_DIRECTORY_ENTRY_SIZE: usize = 32;

#[derive(Clone)]
enum RawPayload {
    Heap(Box<[u8]>),
    Mapped {
        mmap: Arc<Mmap>,
        range: Range<usize>,
    },
}

impl RawPayload {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Heap(bytes) => bytes,
            Self::Mapped { mmap, range } => &mmap[range.clone()],
        }
    }

    fn heap_bytes(&self) -> usize {
        match self {
            Self::Heap(bytes) => bytes.len(),
            Self::Mapped { .. } => 0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct PayloadEntry {
    raw: RawPayload,
}

struct CachedValue {
    value: Arc<serde_json::Value>,
    bytes: usize,
    /// 最近一次访问的序号，同时是该条目在 `ParsedCache::order` 里的键。
    seq: u64,
}

/// 解析结果 LRU。
///
/// 访问顺序放在 `order`（序号 → NodeId）而不是 `VecDeque`。命中、插入、失效都要把
/// 一个 id 移到最近端；用 `VecDeque::retain` 定位它是 O(缓存条数)，这让每次
/// `get_payload` 和每次 payload 写入都随缓存驻留量线性变慢（默认 64 MiB 能装下几万
/// 条小 payload）。按序号查找是 O(log n)，淘汰仍从最小序号、即最久未用的一端取。
///
/// Recency order is a sequence-keyed `BTreeMap` instead of a `VecDeque`, so
/// touching one entry is O(log n) rather than a scan of every resident entry.
#[derive(Default)]
struct ParsedCache {
    values: HashMap<NodeId, CachedValue>,
    order: BTreeMap<u64, NodeId>,
    /// 单调递增。按每纳秒一次算 u64 也要五百多年才用完，不做回绕处理。
    next_seq: u64,
    bytes: usize,
    hits: u64,
    misses: u64,
    evictions: u64,
    lookups: u64,
    parsed_bytes: u64,
}

impl ParsedCache {
    fn next_sequence(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// 把已缓存的条目标记为最近使用；不在缓存里则什么都不做。
    fn touch(&mut self, id: NodeId) {
        let Some(previous) = self.values.get(&id).map(|entry| entry.seq) else {
            return;
        };
        let seq = self.next_sequence();
        if let Some(entry) = self.values.get_mut(&id) {
            entry.seq = seq;
        }
        self.order.remove(&previous);
        self.order.insert(seq, id);
    }

    fn insert(&mut self, id: NodeId, value: Arc<serde_json::Value>, bytes: usize) {
        let seq = self.next_sequence();
        if let Some(previous) = self.values.insert(id, CachedValue { value, bytes, seq }) {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
            self.order.remove(&previous.seq);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.order.insert(seq, id);
    }

    fn remove(&mut self, id: NodeId) {
        if let Some(previous) = self.values.remove(&id) {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
            self.order.remove(&previous.seq);
        }
    }

    fn evict_to_budget(&mut self, max_bytes: usize) {
        while self.bytes > max_bytes {
            let Some((_, id)) = self.order.pop_first() else {
                break;
            };
            if let Some(previous) = self.values.remove(&id) {
                self.bytes = self.bytes.saturating_sub(previous.bytes);
                self.evictions = self.evictions.saturating_add(1);
            }
        }
    }
}

type MappedPayloadRecords = Vec<(NodeId, Range<usize>)>;

pub struct PayloadStore {
    entries: HashMap<NodeId, PayloadEntry>,
    heap_raw_bytes: usize,
    mapped: Option<Arc<Mmap>>,
    cache: Mutex<ParsedCache>,
    cache_max_bytes: usize,
    cache_max_entry_bytes: usize,
}

impl Default for PayloadStore {
    fn default() -> Self {
        Self::new(64 * 1024 * 1024, 8 * 1024 * 1024)
    }
}

impl PayloadStore {
    pub(crate) fn save_sidecar(&self, path: &Path, generation: u64) -> Result<()> {
        let mut ids = self.ids().collect::<Vec<_>>();
        ids.sort_unstable();
        let directory_bytes = ids
            .len()
            .checked_mul(PAYLOAD_DIRECTORY_ENTRY_SIZE)
            .ok_or_else(|| TriviumError::InvalidInput("Payload 目录大小溢出".into()))?;
        let data_offset = PAYLOAD_HEADER_SIZE
            .checked_add(directory_bytes)
            .ok_or_else(|| TriviumError::InvalidInput("Payload 数据偏移溢出".into()))?;
        let mut directory = Vec::new();
        directory
            .try_reserve_exact(directory_bytes)
            .map_err(|error| TriviumError::CapacityAllocationFailed {
                reason: format!("Payload 目录分配失败: {error}"),
            })?;
        let mut absolute = data_offset;
        for id in &ids {
            let raw = self
                .raw(*id)
                .ok_or_else(|| TriviumError::CorruptedFile("Payload 目录与数据不一致".into()))?;
            let length = u32::try_from(raw.len())
                .map_err(|_| TriviumError::InvalidInput("单条 Payload 不能超过 4 GiB".into()))?;
            directory.extend_from_slice(&id.to_le_bytes());
            directory.extend_from_slice(&(absolute as u64).to_le_bytes());
            directory.extend_from_slice(&length.to_le_bytes());
            directory.extend_from_slice(&0u32.to_le_bytes());
            directory.extend_from_slice(&crc32fast::hash(raw).to_le_bytes());
            directory.extend_from_slice(&0u32.to_le_bytes());
            absolute = absolute
                .checked_add(raw.len())
                .ok_or_else(|| TriviumError::InvalidInput("Payload 记录偏移溢出".into()))?;
        }

        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        {
            let file = File::create(&tmp)?;
            let mut writer = BufWriter::new(file);
            let mut hasher = crc32fast::Hasher::new();
            write_hashed(&mut writer, &mut hasher, PAYLOAD_MAGIC)?;
            write_hashed(&mut writer, &mut hasher, &PAYLOAD_VERSION.to_le_bytes())?;
            write_hashed(&mut writer, &mut hasher, &0u16.to_le_bytes())?;
            write_hashed(&mut writer, &mut hasher, &generation.to_le_bytes())?;
            write_hashed(&mut writer, &mut hasher, &(self.len() as u64).to_le_bytes())?;
            write_hashed(
                &mut writer,
                &mut hasher,
                &(PAYLOAD_HEADER_SIZE as u64).to_le_bytes(),
            )?;
            write_hashed(
                &mut writer,
                &mut hasher,
                &(data_offset as u64).to_le_bytes(),
            )?;
            write_hashed(&mut writer, &mut hasher, &directory)?;
            for id in ids {
                let raw = self.raw(id).ok_or_else(|| {
                    TriviumError::CorruptedFile("Payload 目录与数据不一致".into())
                })?;
                write_hashed(&mut writer, &mut hasher, raw)?;
            }
            writer.write_all(&hasher.finalize().to_le_bytes())?;
            writer.flush()?;
            writer
                .into_inner()
                .map_err(|error| TriviumError::Io(error.into_error()))?
                .sync_all()?;
        }
        robust_rename_and_sync(&tmp, path)?;
        Ok(())
    }

    pub(crate) fn open_sidecar_limited(
        path: &Path,
        admit: impl Fn(usize) -> Result<()>,
    ) -> Result<(u64, Mmap, MappedPayloadRecords)> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file) }?;
        let bytes = &mmap[..];
        if bytes.len() < PAYLOAD_HEADER_SIZE + 4 || bytes.get(..4) != Some(PAYLOAD_MAGIC) {
            return Err(TriviumError::CorruptedFile("Payload sidecar 头无效".into()));
        }
        let version = read_u16(bytes, 4, "Payload 版本")?;
        if read_u16(bytes, 6, "Payload header reserved")? != 0 {
            return Err(TriviumError::CorruptedFile(
                "Payload header reserved 必须为 0".into(),
            ));
        }
        if version != PAYLOAD_VERSION {
            return Err(TriviumError::CorruptedFile(format!(
                "不支持的 Payload 版本: {version}"
            )));
        }
        let generation = read_u64(bytes, 8, "Payload generation")?;
        let count = usize::try_from(read_u64(bytes, 16, "Payload 记录数")?)
            .map_err(|_| TriviumError::CorruptedFile("Payload 记录数超出平台范围".into()))?;
        let directory_offset = usize::try_from(read_u64(bytes, 24, "Payload 目录偏移")?)
            .map_err(|_| TriviumError::CorruptedFile("Payload 目录偏移超出平台范围".into()))?;
        let data_offset = usize::try_from(read_u64(bytes, 32, "Payload 数据偏移")?)
            .map_err(|_| TriviumError::CorruptedFile("Payload 数据偏移超出平台范围".into()))?;
        let footer = bytes.len() - 4;
        let expected_crc = read_u32(bytes, footer, "Payload 文件 CRC")?;
        if crc32fast::hash(&bytes[..footer]) != expected_crc {
            return Err(TriviumError::CorruptedFile(
                "Payload 文件 CRC 不匹配".into(),
            ));
        }
        let directory_end = directory_offset
            .checked_add(
                count
                    .checked_mul(PAYLOAD_DIRECTORY_ENTRY_SIZE)
                    .ok_or_else(|| TriviumError::CorruptedFile("Payload 目录长度溢出".into()))?,
            )
            .ok_or_else(|| TriviumError::CorruptedFile("Payload 目录结束偏移溢出".into()))?;
        if directory_offset != PAYLOAD_HEADER_SIZE
            || directory_end != data_offset
            || data_offset > footer
        {
            return Err(TriviumError::CorruptedFile("Payload 文件布局无效".into()));
        }
        admit(count.saturating_mul(std::mem::size_of::<(NodeId, Range<usize>)>()))?;
        let mut records = Vec::new();
        records.try_reserve_exact(count).map_err(|error| {
            TriviumError::CapacityAllocationFailed {
                reason: format!("Payload 目录分配失败: {error}"),
            }
        })?;
        let mut previous_id = 0;
        let mut expected_offset = data_offset;
        for index in 0..count {
            let cursor = directory_offset + index * PAYLOAD_DIRECTORY_ENTRY_SIZE;
            let id = read_u64(bytes, cursor, "Payload NodeId")?;
            let offset = usize::try_from(read_u64(bytes, cursor + 8, "Payload 记录偏移")?)
                .map_err(|_| TriviumError::CorruptedFile("Payload 记录偏移超出平台范围".into()))?;
            let length = read_u32(bytes, cursor + 16, "Payload 记录长度")? as usize;
            let flags = read_u32(bytes, cursor + 20, "Payload flags")?;
            let reserved = read_u32(bytes, cursor + 28, "Payload directory reserved")?;
            if flags != 0 || reserved != 0 {
                return Err(TriviumError::CorruptedFile(
                    "Payload sidecar 包含非法 flags 或 reserved".into(),
                ));
            }
            if id == 0 || id <= previous_id || offset != expected_offset {
                return Err(TriviumError::CorruptedFile(
                    "Payload 目录必须按 NodeId 和连续数据偏移严格递增".into(),
                ));
            }
            let end = offset
                .checked_add(length)
                .ok_or_else(|| TriviumError::CorruptedFile("Payload 记录范围溢出".into()))?;
            let raw = bytes
                .get(offset..end)
                .filter(|_| offset >= data_offset && end <= footer)
                .ok_or_else(|| TriviumError::CorruptedFile("Payload 记录范围越界".into()))?;
            if crc32fast::hash(raw) != read_u32(bytes, cursor + 24, "Payload 记录 CRC")? {
                return Err(TriviumError::CorruptedFile(format!(
                    "Payload 记录 CRC 不匹配: node_id={id}"
                )));
            }
            admit(
                count
                    .saturating_mul(24)
                    .saturating_add(raw.len().saturating_mul(64)),
            )?;
            validate_json(raw)?;
            records.push((id, offset..end));
            previous_id = id;
            expected_offset = end;
        }
        if expected_offset != footer {
            return Err(TriviumError::CorruptedFile(
                "Payload 数据区包含空洞或未声明尾随字节".into(),
            ));
        }
        validate_unique_records(&records)?;
        Ok((generation, mmap, records))
    }

    pub fn new(cache_max_bytes: usize, cache_max_entry_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            heap_raw_bytes: 0,
            mapped: None,
            cache: Mutex::new(ParsedCache::default()),
            cache_max_bytes,
            cache_max_entry_bytes,
        }
    }

    pub fn configure_cache(&mut self, max_bytes: usize, max_entry_bytes: usize) {
        self.cache_max_bytes = max_bytes;
        self.cache_max_entry_bytes = max_entry_bytes;
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        cache.evict_to_budget(max_bytes);
    }

    pub fn reserve(&mut self, additional: usize) -> Result<()> {
        self.entries.try_reserve(additional).map_err(|error| {
            TriviumError::InvalidInput(format!(
                "Payload 目录预留失败 (Payload directory reservation failed): additional={additional}, error={error}"
            ))
        })
    }

    pub fn capacity(&self) -> usize {
        self.entries.capacity()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.entries.contains_key(&id)
    }

    pub fn keys(&self) -> impl Iterator<Item = &NodeId> {
        self.entries.keys()
    }

    pub fn contains_key(&self, id: &NodeId) -> bool {
        self.contains(*id)
    }

    pub fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.entries.keys().copied()
    }

    pub fn insert_value(&mut self, id: NodeId, value: serde_json::Value) -> Result<()> {
        let raw = serde_json::to_vec(&value)
            .map_err(|error| TriviumError::InvalidInput(format!("Payload 序列化失败: {error}")))?;
        self.insert_raw(id, &raw)?;
        self.cache_insert(id, Arc::new(value));
        Ok(())
    }

    pub fn insert_raw(&mut self, id: NodeId, raw: &[u8]) -> Result<()> {
        validate_json(raw)?;
        self.invalidate(id);
        self.heap_raw_bytes = self
            .heap_raw_bytes
            .saturating_sub(
                self.entries
                    .get(&id)
                    .map_or(0, |entry| entry.raw.heap_bytes()),
            )
            .saturating_add(raw.len());
        self.entries.insert(
            id,
            PayloadEntry {
                raw: RawPayload::Heap(raw.to_vec().into_boxed_slice()),
            },
        );
        Ok(())
    }

    pub fn remove(&mut self, id: NodeId) -> bool {
        self.invalidate(id);
        if let Some(entry) = self.entries.remove(&id) {
            self.heap_raw_bytes = self.heap_raw_bytes.saturating_sub(entry.raw.heap_bytes());
            true
        } else {
            false
        }
    }

    pub fn raw(&self, id: NodeId) -> Option<&[u8]> {
        self.entries.get(&id).map(|entry| entry.raw.bytes())
    }

    pub(crate) fn fingerprint(&self) -> (u64, u32) {
        let mut ids = self.ids().collect::<Vec<_>>();
        ids.sort_unstable();
        let mut bytes = 0u64;
        let mut hash = crc32fast::Hasher::new();
        for id in ids {
            let raw = self.entries[&id].raw.bytes();
            hash.update(&id.to_le_bytes());
            hash.update(&(raw.len() as u64).to_le_bytes());
            hash.update(raw);
            bytes = bytes.saturating_add(raw.len() as u64);
        }
        (bytes, hash.finalize())
    }

    pub fn get_value(&self, id: NodeId) -> Option<Arc<serde_json::Value>> {
        self.try_get_value(id).ok().flatten()
    }

    pub(crate) fn try_get_value(&self, id: NodeId) -> Result<Option<Arc<serde_json::Value>>> {
        crate::request::record_payload_lookup();
        {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            cache.lookups = cache.lookups.saturating_add(1);
            if let Some(value) = cache.values.get(&id).map(|entry| Arc::clone(&entry.value)) {
                cache.hits = cache.hits.saturating_add(1);
                cache.touch(id);
                return Ok(Some(value));
            }
            cache.misses = cache.misses.saturating_add(1);
        }
        let Some(raw) = self.raw(id) else {
            return Ok(None);
        };
        let value = Arc::new(serde_json::from_slice(raw).map_err(|error| {
            TriviumError::CorruptedFile(format!(
                "Payload JSON 解码失败 (Payload JSON decoding failed): id={id}, {error}"
            ))
        })?);
        crate::request::record_payload_parse(raw.len() as u64);
        {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            cache.parsed_bytes = cache.parsed_bytes.saturating_add(raw.len() as u64);
        }
        self.cache_insert(id, Arc::clone(&value));
        Ok(Some(value))
    }

    pub fn iter(&self) -> impl Iterator<Item = (NodeId, Arc<serde_json::Value>)> + '_ {
        self.ids()
            .filter_map(|id| self.get_value(id).map(|value| (id, value)))
    }

    pub fn values(&self) -> impl Iterator<Item = Arc<serde_json::Value>> + '_ {
        self.ids().filter_map(|id| self.get_value(id))
    }

    pub(crate) fn heap_memory_bytes(&self) -> usize {
        self.entries
            .capacity()
            .saturating_mul(std::mem::size_of::<(NodeId, PayloadEntry)>())
            .saturating_add(self.heap_raw_bytes)
            .saturating_add(self.cache.lock().unwrap_or_else(|p| p.into_inner()).bytes)
    }

    pub fn memory_stats(&self) -> PayloadMemoryStats {
        let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        PayloadMemoryStats {
            directory_bytes: self
                .entries
                .capacity()
                .saturating_mul(std::mem::size_of::<(NodeId, PayloadEntry)>()),
            delta_raw_bytes: self.heap_raw_bytes,
            parsed_cache_bytes: cache.bytes,
            parsed_cache_entries: cache.values.len(),
            pinned_cache_entries: cache
                .values
                .values()
                .filter(|entry| Arc::strong_count(&entry.value) > 1)
                .count(),
            mapped_file_bytes: self.mapped.as_ref().map_or(0, |mmap| mmap.len()),
            cache_hits: cache.hits,
            cache_misses: cache.misses,
            cache_evictions: cache.evictions,
            payload_lookups: cache.lookups,
            payload_parsed_bytes: cache.parsed_bytes,
        }
    }

    pub(crate) fn install_mapped(
        &mut self,
        mmap: Mmap,
        records: Vec<(NodeId, Range<usize>)>,
    ) -> Result<()> {
        validate_unique_records(&records)?;
        for (_, range) in &records {
            let raw = mmap
                .get(range.clone())
                .ok_or_else(|| TriviumError::CorruptedFile("Payload mmap 范围越界".into()))?;
            validate_json(raw)?;
        }
        let mmap = Arc::new(mmap);
        self.entries.clear();
        self.heap_raw_bytes = 0;
        self.entries.reserve(records.len());
        for (id, range) in records {
            self.entries.insert(
                id,
                PayloadEntry {
                    raw: RawPayload::Mapped {
                        mmap: Arc::clone(&mmap),
                        range,
                    },
                },
            );
        }
        self.mapped = Some(mmap);
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        *cache = ParsedCache::default();
        Ok(())
    }

    fn invalidate(&self, id: NodeId) {
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        cache.remove(id);
    }

    fn cache_insert(&self, id: NodeId, value: Arc<serde_json::Value>) {
        if self.cache_max_bytes == 0 {
            return;
        }
        let bytes = estimate_json_memory(&value);
        if bytes > self.cache_max_entry_bytes || bytes > self.cache_max_bytes {
            return;
        }
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        cache.insert(id, value, bytes);
        cache.evict_to_budget(self.cache_max_bytes);
    }
}

fn write_hashed(
    writer: &mut impl Write,
    hasher: &mut crc32fast::Hasher,
    bytes: &[u8],
) -> Result<()> {
    writer.write_all(bytes)?;
    hasher.update(bytes);
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize, field: &str) -> Result<u16> {
    bytes
        .get(offset..offset.saturating_add(2))
        .and_then(|value| value.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| TriviumError::CorruptedFile(format!("{field} 被截断")))
}

fn read_u32(bytes: &[u8], offset: usize, field: &str) -> Result<u32> {
    bytes
        .get(offset..offset.saturating_add(4))
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| TriviumError::CorruptedFile(format!("{field} 被截断")))
}

fn read_u64(bytes: &[u8], offset: usize, field: &str) -> Result<u64> {
    bytes
        .get(offset..offset.saturating_add(8))
        .and_then(|value| value.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| TriviumError::CorruptedFile(format!("{field} 被截断")))
}

fn validate_json(raw: &[u8]) -> Result<()> {
    serde_json::from_slice::<serde_json::Value>(raw)
        .map(|_| ())
        .map_err(|error| {
            TriviumError::CorruptedFile(format!(
                "Payload JSON 无效 (Invalid payload JSON): {error}"
            ))
        })
}

/// 与默认 JSON 解码器保持相同的容器深度边界，计数过程不分配编码缓冲区。
pub(crate) fn json_encoded_len(value: &serde_json::Value) -> Result<usize> {
    fn validate_depth(value: &serde_json::Value, depth: usize) -> Result<()> {
        match value {
            serde_json::Value::Array(values) => {
                if depth >= 127 {
                    return Err(TriviumError::InvalidInput(
                        "JSON 容器嵌套超过 127 层 (JSON container nesting exceeds 127 levels)"
                            .into(),
                    ));
                }
                for value in values {
                    validate_depth(value, depth + 1)?;
                }
            }
            serde_json::Value::Object(values) => {
                if depth >= 127 {
                    return Err(TriviumError::InvalidInput(
                        "JSON 容器嵌套超过 127 层 (JSON container nesting exceeds 127 levels)"
                            .into(),
                    ));
                }
                for value in values.values() {
                    validate_depth(value, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    validate_depth(value, 0)?;
    serialized_json_len(value)
}

/// 对类型化SDK表示计数，在分配输出缓冲区前完成预算准入。
pub(crate) fn serialized_json_len(value: &impl serde::Serialize) -> Result<usize> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).map_err(|error| {
        TriviumError::InvalidInput(format!("JSON 编码失败 (JSON encoding failed): {error}"))
    })?;
    Ok(counter.0)
}

pub(crate) fn estimate_json_memory(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            std::mem::size_of::<serde_json::Value>()
        }
        serde_json::Value::String(text) => {
            std::mem::size_of::<serde_json::Value>() + text.capacity()
        }
        serde_json::Value::Array(values) => {
            std::mem::size_of::<serde_json::Value>()
                + values
                    .capacity()
                    .saturating_mul(std::mem::size_of::<serde_json::Value>())
                + values.iter().map(estimate_json_memory).sum::<usize>()
        }
        serde_json::Value::Object(values) => {
            std::mem::size_of::<serde_json::Value>()
                + values
                    .iter()
                    .map(|(key, value)| key.capacity() + estimate_json_memory(value))
                    .sum::<usize>()
        }
    }
}

/// 对已校验 JSON 扫描结构，估算解码缓冲、容器和返回值克隆的保守峰值，不解析 Payload。
pub(crate) fn query_decode_peak(raw: &[u8]) -> Result<usize> {
    let (mut quoted, mut escaped, mut atom) = (false, false, false);
    let (mut values, mut entries, mut objects) = (0usize, 0usize, 0usize);
    let mut containers = [0u8; 128];
    let mut depth = 0usize;
    for &byte in raw {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
            continue;
        }
        match byte {
            b'"' => {
                quoted = true;
                atom = false;
                values += 1;
            }
            b'[' | b'{' => {
                if depth == containers.len() {
                    return Err(TriviumError::CorruptedFile(
                        "Payload 容器深度超限 (Payload container depth exceeded)".into(),
                    ));
                }
                containers[depth] = if byte == b'{' { 0 } else { 2 };
                depth += 1;
                values += 1;
                atom = false;
            }
            b':' => {
                if depth == 0 {
                    return Err(TriviumError::CorruptedFile(
                        "Payload 对象结构无效 (Invalid Payload object structure)".into(),
                    ));
                }
                if containers[depth - 1] == 0 {
                    objects += 1;
                    containers[depth - 1] = 1;
                }
                entries += 1;
                atom = false;
            }
            b']' | b'}' => {
                depth = depth.saturating_sub(1);
                atom = false;
            }
            b',' | b' ' | b'\n' | b'\r' | b'\t' => atom = false,
            _ if !atom => {
                values += 1;
                atom = true;
            }
            _ => {}
        }
    }
    raw.len()
        .checked_mul(3)
        .and_then(|bytes| {
            values
                .checked_mul(std::mem::size_of::<serde_json::Value>() * 4)
                .and_then(|extra| bytes.checked_add(extra))
        })
        .and_then(|bytes| {
            entries
                .checked_mul(256)
                .and_then(|extra| bytes.checked_add(extra))
        })
        // 每个非空对象另计解码和返回副本的树根节点；空对象保持轻量。
        .and_then(|bytes| {
            objects
                .checked_mul(2048)
                .and_then(|extra| bytes.checked_add(extra))
        })
        .and_then(|bytes| bytes.checked_add(256))
        .ok_or_else(|| TriviumError::CapacityAllocationFailed {
            reason: "查询 Payload 峰值估算溢出 (Query Payload peak estimate overflowed)".into(),
        })
}

/// 查询复制已解析 JSON 时，连同对象树节点和容器余量一起预留。
pub(crate) fn query_clone_peak(value: &serde_json::Value) -> usize {
    let header = std::mem::size_of::<serde_json::Value>();
    match value {
        serde_json::Value::Object(values) if values.is_empty() => header,
        serde_json::Value::Object(values) => values.iter().fold(
            header
                .saturating_add(1024)
                .saturating_add(values.len().saturating_mul(128)),
            |sum, (key, value)| {
                sum.saturating_add(key.len())
                    .saturating_add(query_clone_peak(value))
            },
        ),
        serde_json::Value::Array(values) => values.iter().fold(
            header.saturating_add(values.capacity().saturating_mul(header)),
            |sum, value| sum.saturating_add(query_clone_peak(value)),
        ),
        serde_json::Value::String(value) => header.saturating_add(value.len()),
        _ => header,
    }
}

pub(crate) fn validate_unique_records(records: &[(NodeId, Range<usize>)]) -> Result<()> {
    let mut ids = BTreeSet::new();
    for (id, range) in records {
        if *id == 0 || !ids.insert(*id) || range.start > range.end {
            return Err(TriviumError::CorruptedFile(
                "Payload 目录包含非法或重复 NodeId/范围".into(),
            ));
        }
    }
    Ok(())
}
