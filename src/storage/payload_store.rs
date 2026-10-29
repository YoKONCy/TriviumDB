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

    #[cfg(test)]
    pub(crate) fn open_sidecar(path: &Path) -> Result<(u64, Mmap, MappedPayloadRecords)> {
        Self::open_sidecar_limited(path, |_| Ok(()))
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

    pub(crate) fn access_counters(&self) -> (u64, u64) {
        let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        (cache.lookups, cache.parsed_bytes)
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
    validate_depth(value, 0)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn 原始字节账本跨映射替换删除及无效写入保持准确() {
        let path = test_path("byte-ledger");
        let mut store = PayloadStore::new(0, 0);
        store.insert_raw(1, b"[1,2,3]").unwrap();
        store.insert_raw(2, b"true").unwrap();
        assert_eq!(store.memory_stats().delta_raw_bytes, 11);
        store.save_sidecar(&path, 7).unwrap();
        let (_, mmap, records) = PayloadStore::open_sidecar(&path).unwrap();
        store.install_mapped(mmap, records).unwrap();
        assert_eq!(store.memory_stats().delta_raw_bytes, 0);
        store.insert_raw(1, b"{\"v\":123}").unwrap();
        assert_eq!(store.memory_stats().delta_raw_bytes, 9);
        assert!(store.insert_raw(1, b"invalid").is_err());
        assert_eq!(store.raw(1), Some(b"{\"v\":123}".as_slice()));
        assert_eq!(store.memory_stats().delta_raw_bytes, 9);
        store.insert_raw(1, b"null").unwrap();
        assert_eq!(store.memory_stats().delta_raw_bytes, 4);
        assert!(store.remove(2));
        assert_eq!(store.memory_stats().delta_raw_bytes, 4);
        assert!(store.remove(1));
        assert!(!store.remove(1));
        assert_eq!(store.memory_stats().delta_raw_bytes, 0);
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    fn test_path(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!(
            "triviumdb-payload-{name}-{}-{nonce}.pld",
            std::process::id()
        ))
    }

    #[test]
    fn heap_crud_and_raw_round_trip() {
        let mut store = PayloadStore::new(0, 0);
        store
            .insert_raw(2, r#"{"name":"二","n":2}"#.as_bytes())
            .unwrap();
        store
            .insert_value(1, serde_json::json!({"name": "一", "n": 1}))
            .unwrap();
        assert_eq!(
            store.get_value(1).and_then(|value| value["n"].as_i64()),
            Some(1)
        );
        assert_eq!(
            store
                .get_value(2)
                .and_then(|value| value["name"].as_str().map(str::to_owned)),
            Some("二".to_owned())
        );
        assert!(store.insert_raw(3, b"{broken").is_err());
        assert!(!store.contains(3));
        assert!(store.remove(1));
        assert!(!store.remove(1));
    }

    #[test]
    fn sidecar_round_trip_uses_mapped_raw() {
        let path = test_path("round-trip");
        let mut source = PayloadStore::new(0, 0);
        source
            .insert_value(9, serde_json::json!({"文本": "冷数据"}))
            .unwrap();
        source
            .insert_value(3, serde_json::json!([1, 2, 3]))
            .unwrap();
        source.save_sidecar(&path, 17).unwrap();

        let (generation, mmap, records) = PayloadStore::open_sidecar(&path).unwrap();
        let mut mapped = PayloadStore::new(0, 0);
        mapped.install_mapped(mmap, records).unwrap();
        assert_eq!(generation, 17);
        assert_eq!(mapped.get_value(9), source.get_value(9));
        assert_eq!(mapped.get_value(3), source.get_value(3));
        let stats = mapped.memory_stats();
        assert_eq!(stats.delta_raw_bytes, 0);
        assert!(stats.mapped_file_bytes > 0);
        drop(mapped);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn sidecar_rejects_whole_file_and_record_corruption() {
        let path = test_path("corruption");
        let mut store = PayloadStore::new(0, 0);
        store
            .insert_value(1, serde_json::json!({"ok": true}))
            .unwrap();
        store.save_sidecar(&path, 1).unwrap();
        let original = std::fs::read(&path).unwrap();

        let mut mutations = vec![original[..original.len() - 1].to_vec(), original.clone()];
        mutations[1][0] ^= 0xff;
        let mut record_crc = original.clone();
        record_crc[40 + 24] ^= 1;
        let footer = record_crc.len() - 4;
        let crc = crc32fast::hash(&record_crc[..footer]);
        record_crc[footer..].copy_from_slice(&crc.to_le_bytes());
        mutations.push(record_crc);

        for mutation in mutations {
            std::fs::write(&path, mutation).unwrap();
            assert!(PayloadStore::open_sidecar(&path).is_err());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cache_zero_and_oversized_entries_bypass_storage() {
        let value = serde_json::json!({"text": "不会驻留"});
        let estimated = estimate_json_memory(&value);
        let mut disabled = PayloadStore::new(0, 0);
        disabled.insert_value(1, value.clone()).unwrap();
        assert_eq!(disabled.get_value(1).as_deref(), Some(&value));
        assert_eq!(disabled.get_value(1).as_deref(), Some(&value));
        let stats = disabled.memory_stats();
        assert_eq!(stats.parsed_cache_entries, 0);
        assert_eq!(stats.parsed_cache_bytes, 0);
        assert_eq!(stats.cache_misses, 2);

        let mut limited = PayloadStore::new(estimated.saturating_mul(2), estimated - 1);
        limited
            .insert_raw(1, r#"{"text":"不会驻留"}"#.as_bytes())
            .unwrap();
        assert!(limited.get_value(1).is_some());
        assert_eq!(limited.memory_stats().parsed_cache_entries, 0);
    }

    #[test]
    fn cache_lru_eviction_is_bounded_and_deterministic() {
        let values = [
            serde_json::json!({"id": 1}),
            serde_json::json!({"id": 2}),
            serde_json::json!({"id": 3}),
        ];
        let entry_bytes = values.iter().map(estimate_json_memory).max().unwrap();
        let budget = entry_bytes.saturating_mul(2);
        let mut store = PayloadStore::new(budget, entry_bytes);
        for (index, value) in values.iter().enumerate() {
            store
                .insert_raw((index + 1) as NodeId, &serde_json::to_vec(value).unwrap())
                .unwrap();
        }

        assert!(store.get_value(1).is_some());
        assert!(store.get_value(2).is_some());
        assert!(store.get_value(1).is_some());
        assert!(store.get_value(3).is_some());
        let before = store.memory_stats();
        assert!(before.parsed_cache_bytes <= budget);
        assert_eq!(before.parsed_cache_entries, 2);
        assert_eq!(before.cache_evictions, 1);

        assert!(store.get_value(2).is_some());
        let after = store.memory_stats();
        assert_eq!(after.cache_misses, 4);
        assert_eq!(after.cache_hits, 1);
        assert_eq!(after.cache_evictions, 2);
        assert!(after.parsed_cache_bytes <= budget);
    }

    /// 淘汰顺序必须按「最近访问」，不是按插入顺序。
    ///
    /// Recency, not insertion order, decides eviction. The sequence-keyed
    /// index has to reproduce what the deque did: reading an entry moves it
    /// out of the eviction path, and re-inserting an id must not leave its
    /// previous position behind.
    #[test]
    fn eviction_follows_access_recency_not_insertion_order() {
        let value = serde_json::json!({"text": "一二三四"});
        let entry_bytes = estimate_json_memory(&value);
        let mut store = PayloadStore::new(entry_bytes.saturating_mul(2), entry_bytes);
        for id in 1..=3 {
            store
                .insert_raw(id, &serde_json::to_vec(&value).unwrap())
                .unwrap();
        }

        // Cache 1 and 2, then read 1 again so 2 is the least recently used.
        assert!(store.get_value(1).is_some());
        assert!(store.get_value(2).is_some());
        assert!(store.get_value(1).is_some());
        // Caching 3 has to evict 2, and 1 has to survive.
        assert!(store.get_value(3).is_some());
        assert_eq!(store.memory_stats().parsed_cache_entries, 2);
        let hits_before = store.memory_stats().cache_hits;
        assert!(store.get_value(1).is_some());
        assert_eq!(store.memory_stats().cache_hits, hits_before + 1);

        // Re-inserting an already-cached id must replace its position rather
        // than leave a stale one that later evicts the wrong entry.
        store.insert_value(3, value.clone()).unwrap();
        store.insert_value(3, value.clone()).unwrap();
        let stats = store.memory_stats();
        assert!(stats.parsed_cache_bytes <= entry_bytes.saturating_mul(2));
        assert!(stats.parsed_cache_entries <= 2);
    }

    use proptest::prelude::*;
    use std::collections::VecDeque;

    /// 参照模型：原先的 `VecDeque` LRU。序号索引必须在任意操作序列下与它给出
    /// 完全相同的缓存内容、字节数和淘汰次数。
    #[derive(Default)]
    struct DequeLru {
        values: HashMap<NodeId, usize>,
        order: VecDeque<NodeId>,
        bytes: usize,
        evictions: u64,
    }

    impl DequeLru {
        fn touch(&mut self, id: NodeId) {
            self.order.retain(|candidate| *candidate != id);
            self.order.push_back(id);
        }

        fn hit(&mut self, id: NodeId) -> bool {
            if self.values.contains_key(&id) {
                self.touch(id);
                return true;
            }
            false
        }

        fn insert(&mut self, id: NodeId, bytes: usize, max_bytes: usize) {
            if let Some(previous) = self.values.remove(&id) {
                self.bytes -= previous;
            }
            self.touch(id);
            self.bytes += bytes;
            self.values.insert(id, bytes);
            while self.bytes > max_bytes {
                let Some(oldest) = self.order.pop_front() else {
                    break;
                };
                if let Some(previous) = self.values.remove(&oldest) {
                    self.bytes -= previous;
                    self.evictions += 1;
                }
            }
        }

        fn remove(&mut self, id: NodeId) {
            if let Some(previous) = self.values.remove(&id) {
                self.bytes -= previous;
            }
            self.order.retain(|candidate| *candidate != id);
        }
    }

    #[derive(Debug, Clone)]
    enum CacheOp {
        Get(NodeId),
        Insert(NodeId, usize),
        Remove(NodeId),
    }

    fn arb_cache_op() -> BoxedStrategy<CacheOp> {
        prop_oneof![
            (1..12u64).prop_map(CacheOp::Get),
            (1..12u64, 1..80usize).prop_map(|(id, bytes)| CacheOp::Insert(id, bytes)),
            (1..12u64).prop_map(CacheOp::Remove),
        ]
        .boxed()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]

        /// 不变量：序号索引与 `VecDeque` 参照在任意 get/insert/remove 序列后一致，
        /// 且 `order` 与 `values` 一一对应、字节数等于各条之和。
        #[test]
        fn parsed_cache_matches_deque_reference(
            max_bytes in prop_oneof![Just(0usize), 40..400usize, Just(5000usize)],
            ops in prop::collection::vec(arb_cache_op(), 1..200),
        ) {
            let mut cache = ParsedCache::default();
            let mut reference = DequeLru::default();
            let value = Arc::new(serde_json::Value::Null);

            for op in ops {
                match op {
                    CacheOp::Get(id) => {
                        let hit = cache.values.contains_key(&id);
                        if hit {
                            cache.touch(id);
                        }
                        prop_assert_eq!(hit, reference.hit(id));
                    }
                    CacheOp::Insert(id, bytes) => {
                        if max_bytes == 0 || bytes > max_bytes {
                            continue;
                        }
                        cache.insert(id, Arc::clone(&value), bytes);
                        cache.evict_to_budget(max_bytes);
                        reference.insert(id, bytes, max_bytes);
                    }
                    CacheOp::Remove(id) => {
                        cache.remove(id);
                        reference.remove(id);
                    }
                }

                let mut ours = cache.values.iter().map(|(id, entry)| (*id, entry.bytes)).collect::<Vec<_>>();
                let mut theirs = reference.values.iter().map(|(id, bytes)| (*id, *bytes)).collect::<Vec<_>>();
                ours.sort_unstable();
                theirs.sort_unstable();
                prop_assert_eq!(&ours, &theirs);
                prop_assert_eq!(cache.bytes, reference.bytes);
                prop_assert_eq!(cache.evictions, reference.evictions);

                prop_assert_eq!(cache.order.len(), cache.values.len());
                for (seq, id) in &cache.order {
                    let entry = cache.values.get(id);
                    prop_assert!(entry.is_some_and(|entry| entry.seq == *seq));
                }
                let sum = cache.values.values().map(|entry| entry.bytes).sum::<usize>();
                prop_assert_eq!(cache.bytes, sum);
            }
        }
    }

    #[test]
    fn pinned_value_survives_eviction_and_update_invalidates_cache() {
        let value = serde_json::json!({"version": 1, "text": "固定引用"});
        let bytes = estimate_json_memory(&value);
        let mut store = PayloadStore::new(bytes, bytes);
        store.insert_value(1, value).unwrap();
        let pinned = store.get_value(1).unwrap();
        assert_eq!(store.memory_stats().pinned_cache_entries, 1);

        store.configure_cache(0, 0);
        assert_eq!(store.memory_stats().parsed_cache_entries, 0);
        assert_eq!(pinned["version"], 1);

        store
            .insert_raw(1, r#"{"version":2,"text":"新值"}"#.as_bytes())
            .unwrap();
        assert_eq!(store.get_value(1).unwrap()["version"], 2);
        assert_eq!(pinned["version"], 1);
    }

    #[test]
    fn concurrent_cache_reads_return_stable_values() {
        let mut store = PayloadStore::new(4096, 4096);
        for id in 1..=8 {
            store
                .insert_value(
                    id,
                    serde_json::json!({"id": id, "name": format!("节点{id}")}),
                )
                .unwrap();
        }
        let store = Arc::new(store);
        let threads = (0..8)
            .map(|_| {
                let store = Arc::clone(&store);
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        for id in 1..=8 {
                            assert_eq!(store.get_value(id).unwrap()["id"], id);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        let stats = store.memory_stats();
        assert!(stats.parsed_cache_bytes <= 4096);
        assert!(stats.cache_hits > 0);
    }

    fn rewrite_file_crc(bytes: &mut [u8]) {
        let footer = bytes.len() - 4;
        let crc = crc32fast::hash(&bytes[..footer]);
        bytes[footer..].copy_from_slice(&crc.to_le_bytes());
    }

    #[test]
    fn sidecar_layout_mutations_fail_closed() {
        let path = test_path("layout-mutations");
        let mut store = PayloadStore::new(0, 0);
        store.insert_value(1, serde_json::json!({"id": 1})).unwrap();
        store.insert_value(2, serde_json::json!({"id": 2})).unwrap();
        store.save_sidecar(&path, 9).unwrap();
        let original = std::fs::read(&path).unwrap();

        let mut mutations = Vec::new();
        for (offset, replacement) in [
            (6, 1u16.to_le_bytes().to_vec()),
            (24, 41u64.to_le_bytes().to_vec()),
            (32, 41u64.to_le_bytes().to_vec()),
            (40 + 20, 1u32.to_le_bytes().to_vec()),
            (40 + 28, 1u32.to_le_bytes().to_vec()),
            (40 + 32, 1u64.to_le_bytes().to_vec()),
        ] {
            let mut bytes = original.clone();
            bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
            rewrite_file_crc(&mut bytes);
            mutations.push(bytes);
        }
        let mut gap = original.clone();
        let second_offset = read_u64(&gap, 40 + 32 + 8, "测试记录偏移").unwrap();
        gap[40 + 32 + 8..40 + 32 + 16].copy_from_slice(&(second_offset + 1).to_le_bytes());
        rewrite_file_crc(&mut gap);
        mutations.push(gap);

        for bytes in mutations {
            std::fs::write(&path, bytes).unwrap();
            assert!(PayloadStore::open_sidecar(&path).is_err());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn fixed_seed_heap_mapped_delta_state_machine_matches_reference() {
        let path = test_path("state-machine");
        let mut store = PayloadStore::new(512, 256);
        let mut reference = std::collections::BTreeMap::new();
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for step in 0..300u64 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let id = state % 23 + 1;
            match state % 4 {
                0 | 1 => {
                    let value = serde_json::json!({
                        "id": id,
                        "step": step,
                        "text": format!("状态-{state:016x}"),
                    });
                    store.insert_value(id, value.clone()).unwrap();
                    reference.insert(id, value);
                }
                2 => {
                    assert_eq!(store.remove(id), reference.remove(&id).is_some());
                }
                _ => {
                    assert_eq!(store.get_value(id).as_deref(), reference.get(&id));
                }
            }

            if step % 37 == 0 {
                store.save_sidecar(&path, step).unwrap();
                let (generation, mmap, records) = PayloadStore::open_sidecar(&path).unwrap();
                assert_eq!(generation, step);
                store.install_mapped(mmap, records).unwrap();
            }
            let actual = store
                .iter()
                .map(|(id, value)| (id, (*value).clone()))
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(actual, reference, "状态机步骤 {step} 不一致");
            assert!(store.memory_stats().parsed_cache_bytes <= 512);
        }
        if path.exists() {
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn duplicate_and_invalid_records_fail_closed() {
        assert!(validate_unique_records(&[(0, 1..2)]).is_err());
        assert!(validate_unique_records(&[(1, Range { start: 2, end: 1 })]).is_err());
        assert!(validate_unique_records(&[(1, 1..2), (1, 2..3)]).is_err());
    }
}
