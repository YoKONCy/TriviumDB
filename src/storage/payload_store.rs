//! Payload 冷热分层存储。
//!
//! NodeId 目录常驻内存；已发布 raw JSON 可引用只读 mmap，新增和更新保存在堆增量层。
//! 解析结果进入有硬字节上限的 LRU cache，并通过 `Arc<Value>` 安全跨越淘汰边界。

use crate::error::{Result, TriviumError};
use crate::node::NodeId;
use crate::observability::PayloadMemoryStats;
use crate::storage::fs::robust_rename_and_sync;
use memmap2::Mmap;
use std::collections::{BTreeSet, HashMap, VecDeque};
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

#[derive(Default)]
struct ParsedCache {
    values: HashMap<NodeId, (Arc<serde_json::Value>, usize)>,
    lru: VecDeque<NodeId>,
    bytes: usize,
    hits: u64,
    misses: u64,
    evictions: u64,
    lookups: u64,
    parsed_bytes: u64,
}

type MappedPayloadRecords = Vec<(NodeId, Range<usize>)>;

pub struct PayloadStore {
    entries: HashMap<NodeId, PayloadEntry>,
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

    pub(crate) fn open_sidecar(path: &Path) -> Result<(u64, Mmap, MappedPayloadRecords)> {
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
        Self::evict_to_budget(&mut cache, max_bytes);
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
        self.entries.remove(&id).is_some()
    }

    pub fn raw(&self, id: NodeId) -> Option<&[u8]> {
        self.entries.get(&id).map(|entry| entry.raw.bytes())
    }

    pub fn get_value(&self, id: NodeId) -> Option<Arc<serde_json::Value>> {
        {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            cache.lookups = cache.lookups.saturating_add(1);
            if let Some((value, _)) = cache.values.get(&id).cloned() {
                cache.hits = cache.hits.saturating_add(1);
                touch(&mut cache.lru, id);
                return Some(value);
            }
            cache.misses = cache.misses.saturating_add(1);
        }
        let raw = self.raw(id)?;
        let value = Arc::new(serde_json::from_slice(raw).ok()?);
        {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            cache.parsed_bytes = cache.parsed_bytes.saturating_add(raw.len() as u64);
        }
        self.cache_insert(id, Arc::clone(&value));
        Some(value)
    }

    pub fn iter(&self) -> impl Iterator<Item = (NodeId, Arc<serde_json::Value>)> + '_ {
        self.ids()
            .filter_map(|id| self.get_value(id).map(|value| (id, value)))
    }

    pub fn values(&self) -> impl Iterator<Item = Arc<serde_json::Value>> + '_ {
        self.ids().filter_map(|id| self.get_value(id))
    }

    pub fn memory_stats(&self) -> PayloadMemoryStats {
        let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        PayloadMemoryStats {
            directory_bytes: self
                .entries
                .capacity()
                .saturating_mul(std::mem::size_of::<(NodeId, PayloadEntry)>()),
            delta_raw_bytes: self
                .entries
                .values()
                .map(|entry| entry.raw.heap_bytes())
                .sum(),
            parsed_cache_bytes: cache.bytes,
            parsed_cache_entries: cache.values.len(),
            pinned_cache_entries: cache
                .values
                .values()
                .filter(|(value, _)| Arc::strong_count(value) > 1)
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
        if let Some((_, bytes)) = cache.values.remove(&id) {
            cache.bytes = cache.bytes.saturating_sub(bytes);
        }
        cache.lru.retain(|candidate| *candidate != id);
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
        if let Some((_, previous)) = cache.values.remove(&id) {
            cache.bytes = cache.bytes.saturating_sub(previous);
        }
        touch(&mut cache.lru, id);
        cache.bytes = cache.bytes.saturating_add(bytes);
        cache.values.insert(id, (value, bytes));
        Self::evict_to_budget(&mut cache, self.cache_max_bytes);
    }

    fn evict_to_budget(cache: &mut ParsedCache, max_bytes: usize) {
        while cache.bytes > max_bytes {
            let Some(id) = cache.lru.pop_front() else {
                break;
            };
            if let Some((_, bytes)) = cache.values.remove(&id) {
                cache.bytes = cache.bytes.saturating_sub(bytes);
                cache.evictions = cache.evictions.saturating_add(1);
            }
        }
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

fn touch(lru: &mut VecDeque<NodeId>, id: NodeId) {
    lru.retain(|candidate| *candidate != id);
    lru.push_back(id);
}

fn validate_json(raw: &[u8]) -> Result<()> {
    use serde::Deserialize;
    let mut deserializer = serde_json::Deserializer::from_slice(raw);
    serde::de::IgnoredAny::deserialize(&mut deserializer)
        .map_err(|error| TriviumError::CorruptedFile(format!("Payload JSON 无效: {error}")))?;
    deserializer
        .end()
        .map_err(|error| TriviumError::CorruptedFile(format!("Payload JSON 尾部无效: {error}")))
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
