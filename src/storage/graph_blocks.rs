//! 业务图 `.gidx` sidecar 的持久化与校验后映射。
//!
//! 文件保存按源节点分块的出边副本，并携带入边和 Label 派生目录。打开阶段验证
//! 所有边记录、目录和主文件事实，成功后发布完整只读视图。

use crate::error::{Result, TriviumError};
use crate::node::{Edge, NodeId};
use crate::storage::fs::robust_rename_and_sync;
use memmap2::Mmap;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

const MAGIC: &[u8; 4] = b"TGIX";
pub const VERSION: u16 = 3;
const HEADER_SIZE: usize = 24;

#[derive(Debug)]
struct GraphBlock {
    edges: Vec<Edge>,
}

#[derive(Debug)]
pub struct MappedGraphStore {
    mmap: Mmap,
    blocks: HashMap<NodeId, GraphBlock>,
    incoming: HashMap<NodeId, Vec<NodeId>>,
    labels: HashMap<String, Vec<(NodeId, NodeId)>>,
    edge_count: usize,
    heap_bytes: usize,
}

impl MappedGraphStore {
    pub fn open(db_path: &str, expected_nodes: usize) -> Result<Option<Self>> {
        Self::open_with_admission(db_path, expected_nodes, |_| Ok(()))
    }

    pub(crate) fn open_with_admission(
        db_path: &str,
        expected_nodes: usize,
        mut admit: impl FnMut(usize) -> Result<()>,
    ) -> Result<Option<Self>> {
        let path = format!("{db_path}.gidx");
        if !Path::new(&path).exists() {
            return Ok(None);
        }
        let file = std::fs::File::open(&path)?;
        // SAFETY: 只读文件映射由本结构持有，所有切片均受 mmap 生命周期约束。
        let mmap = unsafe { Mmap::map(&file) }.map_err(TriviumError::Io)?;
        if mmap.len() < HEADER_SIZE + 4 || &mmap[..4] != MAGIC {
            return Err(corrupted(
                "图块 sidecar 文件头无效 (Invalid graph block sidecar header)",
            ));
        }
        let payload_end = mmap.len() - 4;
        let whole_crc = read_u32(&mmap, payload_end, "whole_crc")?;
        if crc32fast::hash(&mmap[..payload_end]) != whole_crc {
            return Err(corrupted(
                "图块 sidecar CRC32 不匹配 (Graph block sidecar CRC32 mismatch)",
            ));
        }
        let version = read_u16(&mmap, 4, "version")?;
        if read_u16(&mmap, 6, "flags")? != 0 {
            return Err(corrupted(
                "图索引保留位无效 (Invalid graph index reserved bits)",
            ));
        }
        if !(1..=VERSION).contains(&version) {
            return Err(corrupted(
                "图块 sidecar 版本不受支持 (Unsupported graph block sidecar version)",
            ));
        }
        let node_count = read_u64(&mmap, 8, "node_count")? as usize;
        let block_count = read_u64(&mmap, 16, "block_count")? as usize;
        if node_count != expected_nodes || block_count > expected_nodes {
            return Err(corrupted(
                "图块 sidecar generation 不匹配 (Graph block generation mismatch)",
            ));
        }
        if block_count > payload_end.saturating_sub(HEADER_SIZE) / 24 {
            return Err(corrupted(
                "图块数量超出文件边界 (Graph block count exceeds file bounds)",
            ));
        }

        let mut cursor = HEADER_SIZE;
        if version >= 3 {
            let main_size = read_u64(&mmap, cursor, "main_size")?;
            cursor += 12;
            if main_size != std::fs::metadata(db_path)?.len() {
                return Err(corrupted(
                    "图索引与主文件事实不匹配 (Graph index does not match main file facts)",
                ));
            }
        } else {
            // 旧格式缺少事实绑定，由调用方保留主文件中已读取的权威图。
            return Ok(None);
        }
        let main_file = std::fs::File::open(db_path)?;
        // SAFETY: 主文件只读映射仅在本次校验期间使用，所有偏移均检查范围。
        let main = unsafe { Mmap::map(&main_file) }.map_err(TriviumError::Io)?;
        if main.len() as u64 != read_u64(&mmap, HEADER_SIZE, "main_size")?
            || crc32fast::hash(&main) != read_u32(&mmap, HEADER_SIZE + 8, "main_crc")?
        {
            return Err(corrupted(
                "图索引与主文件事实不匹配 (Graph index does not match main file facts)",
            ));
        }
        if main.len() < 58 || &main[..4] != b"TVDB" {
            return Err(corrupted(
                "图索引对应主文件头无效 (Invalid graph index main header)",
            ));
        }
        let main_version = read_u16(&main, 4, "main_version")?;
        if !(crate::storage::file_format::MINIMUM_SUPPORTED_VERSION
            ..=crate::storage::file_format::CURRENT_VERSION)
            .contains(&main_version)
            || (main_version >= 10
                && (main.len() < 74
                    || crc32fast::hash(&main[..70]) != read_u32(&main, 70, "main_header_crc")?))
        {
            return Err(corrupted(
                "图索引对应主文件版本或校验无效 (Invalid graph index main version or checksum)",
            ));
        }
        let mut blocks = HashMap::with_capacity(block_count);
        let mut incoming: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut labels: HashMap<String, Vec<(NodeId, NodeId)>> = HashMap::new();
        let mut actual_incoming: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut actual_labels: HashMap<String, Vec<(NodeId, NodeId)>> = HashMap::new();
        let mut edge_count = 0usize;
        for _ in 0..block_count {
            let src = read_u64(&mmap, cursor, "source")?;
            cursor += 8;
            let count = read_u32(&mmap, cursor, "edge_count")? as usize;
            cursor += 4;
            let len = read_u32(&mmap, cursor, "block_len")? as usize;
            cursor += 4;
            let crc = read_u32(&mmap, cursor, "block_crc")?;
            cursor += 4;
            let reserved = read_u32(&mmap, cursor, "reserved")?;
            cursor += 4;
            if src == 0 || count == 0 || reserved != 0 || count > len / 20 {
                return Err(corrupted(
                    "图块源节点、数量或保留位无效 (Invalid graph block source, count or reserved bits)",
                ));
            }
            let end = cursor
                .checked_add(len)
                .ok_or_else(|| corrupted("图块偏移溢出"))?;
            let data = mmap
                .get(cursor..end)
                .ok_or_else(|| corrupted("图块被截断 (Graph block truncated)"))?;
            if crc32fast::hash(data) != crc {
                return Err(corrupted("图块 CRC32 不匹配 (Graph block CRC32 mismatch)"));
            }
            admit(len.saturating_mul(64))?;
            let mut decoded = decode_edges(data, count)?;
            decoded.sort_by(|a, b| a.target_id.cmp(&b.target_id).then(a.label.cmp(&b.label)));
            if decoded.windows(2).any(|pair| {
                pair[0].target_id == pair[1].target_id && pair[0].label == pair[1].label
            }) {
                return Err(corrupted(
                    "图块包含重复边 (Graph block contains duplicate edges)",
                ));
            }
            for edge in &decoded {
                actual_incoming.entry(edge.target_id).or_default().push(src);
                actual_labels
                    .entry(edge.label.clone())
                    .or_default()
                    .push((src, edge.target_id));
            }
            if blocks.insert(src, GraphBlock { edges: decoded }).is_some() {
                return Err(corrupted(
                    "图块包含重复源节点 (Duplicate graph source block)",
                ));
            }
            edge_count = edge_count
                .checked_add(count)
                .ok_or_else(|| corrupted("图边计数溢出 (Graph edge count overflow)"))?;
            cursor = end;
        }
        if version >= 2 {
            let directory_len = read_u64(&mmap, cursor, "directory_len")? as usize;
            cursor = cursor
                .checked_add(8)
                .ok_or_else(|| corrupted("目录偏移溢出"))?;
            let directory_crc = read_u32(&mmap, cursor, "directory_crc")?;
            cursor = cursor
                .checked_add(4)
                .ok_or_else(|| corrupted("目录偏移溢出"))?;
            if read_u32(&mmap, cursor, "directory_reserved")? != 0 {
                return Err(corrupted(
                    "图目录保留位无效 (Invalid graph directory reserved bits)",
                ));
            }
            cursor = cursor
                .checked_add(4)
                .ok_or_else(|| corrupted("目录偏移溢出"))?;
            let directory_end = cursor
                .checked_add(directory_len)
                .ok_or_else(|| corrupted("目录长度溢出"))?;
            let directory = mmap
                .get(cursor..directory_end)
                .ok_or_else(|| corrupted("图目录块被截断 (Graph directory block truncated)"))?;
            if crc32fast::hash(directory) != directory_crc {
                return Err(corrupted(
                    "图目录块 CRC32 不匹配 (Graph directory block CRC32 mismatch)",
                ));
            }
            let mut directory_cursor = 0usize;
            let incoming_count = read_u64(directory, directory_cursor, "incoming_count")? as usize;
            directory_cursor = checked_advance(directory_cursor, 8, "incoming_count")?;
            if incoming_count > expected_nodes || incoming_count > edge_count {
                return Err(corrupted("反向目录条目数量不合理"));
            }
            let mut total_incoming = 0usize;
            for _ in 0..incoming_count {
                let target = read_u64(directory, directory_cursor, "incoming_target")?;
                directory_cursor = checked_advance(directory_cursor, 8, "incoming_target")?;
                let count = read_u32(directory, directory_cursor, "incoming_sources")? as usize;
                directory_cursor = checked_advance(directory_cursor, 4, "incoming_sources")?;
                if read_u32(directory, directory_cursor, "incoming_reserved")? != 0 {
                    return Err(corrupted(
                        "反向目录保留位无效 (Invalid incoming reserved bits)",
                    ));
                }
                directory_cursor = checked_advance(directory_cursor, 4, "incoming_reserved")?;
                total_incoming = total_incoming
                    .checked_add(count)
                    .ok_or_else(|| corrupted("反向目录计数溢出"))?;
                if total_incoming > edge_count {
                    return Err(corrupted("反向目录边数量超过图块边数量"));
                }
                let required = count
                    .checked_mul(8)
                    .ok_or_else(|| corrupted("反向目录长度溢出"))?;
                if directory_cursor.saturating_add(required) > directory.len() {
                    return Err(corrupted("反向目录被截断"));
                }
                let mut sources = Vec::with_capacity(count);
                for _ in 0..count {
                    sources.push(read_u64(directory, directory_cursor, "incoming_source")?);
                    directory_cursor = checked_advance(directory_cursor, 8, "incoming_source")?;
                }
                sources.sort_unstable();
                if sources.is_empty() || sources.windows(2).any(|pair| pair[0] == pair[1]) {
                    return Err(corrupted(
                        "反向目录为空或重复 (Empty or duplicated incoming directory)",
                    ));
                }
                if incoming.insert(target, sources).is_some() {
                    return Err(corrupted("反向目录包含重复目标节点"));
                }
            }
            let label_count = read_u32(directory, directory_cursor, "label_count")? as usize;
            directory_cursor = checked_advance(directory_cursor, 4, "label_count")?;
            if read_u32(directory, directory_cursor, "label_reserved")? != 0 {
                return Err(corrupted(
                    "标签目录保留位无效 (Invalid label reserved bits)",
                ));
            }
            directory_cursor = checked_advance(directory_cursor, 4, "label_reserved")?;
            if label_count > edge_count {
                return Err(corrupted("标签目录条目数量不合理"));
            }
            let mut total_pairs = 0usize;
            for _ in 0..label_count {
                let len = read_u32(directory, directory_cursor, "label_len")? as usize;
                directory_cursor = checked_advance(directory_cursor, 4, "label_len")?;
                let count = read_u32(directory, directory_cursor, "label_pairs")? as usize;
                directory_cursor = checked_advance(directory_cursor, 4, "label_pairs")?;
                total_pairs = total_pairs
                    .checked_add(count)
                    .ok_or_else(|| corrupted("标签目录计数溢出"))?;
                if total_pairs > edge_count {
                    return Err(corrupted("标签目录边数量超过图块边数量"));
                }
                let end = directory_cursor
                    .checked_add(len)
                    .ok_or_else(|| corrupted("标签目录溢出"))?;
                let label = std::str::from_utf8(
                    directory
                        .get(directory_cursor..end)
                        .ok_or_else(|| corrupted("标签目录截断"))?,
                )
                .map_err(|_| corrupted("标签目录不是 UTF-8"))?
                .to_owned();
                directory_cursor = end;
                let required = count
                    .checked_mul(16)
                    .ok_or_else(|| corrupted("标签目录长度溢出"))?;
                if directory_cursor.saturating_add(required) > directory.len() {
                    return Err(corrupted("标签目录边对被截断"));
                }
                let mut pairs = Vec::with_capacity(count);
                for _ in 0..count {
                    let source = read_u64(directory, directory_cursor, "label_source")?;
                    directory_cursor = checked_advance(directory_cursor, 8, "label_source")?;
                    let target = read_u64(directory, directory_cursor, "label_target")?;
                    directory_cursor = checked_advance(directory_cursor, 8, "label_target")?;
                    pairs.push((source, target));
                }
                pairs.sort_unstable();
                if pairs.is_empty() || pairs.windows(2).any(|pair| pair[0] == pair[1]) {
                    return Err(corrupted(
                        "标签目录为空或重复 (Empty or duplicated label directory)",
                    ));
                }
                if labels.insert(label, pairs).is_some() {
                    return Err(corrupted("标签目录包含重复标签"));
                }
            }
            if directory_cursor != directory.len() {
                return Err(corrupted("图目录块存在尾部垃圾数据"));
            }
            cursor = directory_end;
        }
        if cursor != payload_end {
            return Err(corrupted(
                "图块 sidecar 存在尾部垃圾数据 (Graph block sidecar has trailing bytes)",
            ));
        }
        for sources in incoming.values_mut() {
            sources.sort_unstable();
            sources.dedup();
        }
        for pairs in labels.values_mut() {
            pairs.sort_unstable();
            pairs.dedup();
        }
        for sources in actual_incoming.values_mut() {
            sources.sort_unstable();
            sources.dedup();
        }
        for pairs in actual_labels.values_mut() {
            pairs.sort_unstable();
        }
        if incoming != actual_incoming || labels != actual_labels {
            return Err(corrupted(
                "图目录与边块不一致 (Graph directories do not match edge blocks)",
            ));
        }
        let mut main_cursor = usize::try_from(read_u64(&main, 42, "main_edge_offset")?)
            .map_err(|_| corrupted("主文件边偏移溢出 (Main edge offset overflow)"))?;
        let main_end = usize::try_from(read_u64(&main, 50, "main_bq_offset")?)
            .map_err(|_| corrupted("主文件边边界溢出 (Main edge boundary overflow)"))?;
        if main_cursor < 58 || main_cursor > main_end || main_end > main.len() {
            return Err(corrupted(
                "主文件边块范围无效 (Invalid main edge block range)",
            ));
        }
        let mut verified = HashSet::new();
        while main_cursor < main_end {
            let (source, edge) = crate::storage::file_format::read_graph_edge(
                &main,
                &mut main_cursor,
                main_end,
                main_version,
                &mut admit,
            )?;
            let block = blocks.get(&source).ok_or_else(|| {
                corrupted("图索引缺少主文件源节点 (Graph index is missing a main source)")
            })?;
            let position = block
                .edges
                .binary_search_by(|candidate| {
                    candidate
                        .target_id
                        .cmp(&edge.target_id)
                        .then(candidate.label.cmp(&edge.label))
                })
                .map_err(|_| {
                    corrupted("图索引缺少主文件边 (Graph index is missing a main edge)")
                })?;
            let candidate = &block.edges[position];
            if !verified.insert((source, position))
                || candidate.weight.to_bits() != edge.weight.to_bits()
                || candidate.metadata != edge.metadata
            {
                return Err(corrupted(
                    "图索引内容与主文件事实不一致 (Graph index content does not match main facts)",
                ));
            }
        }
        if verified.len() != edge_count {
            return Err(corrupted(
                "图索引包含额外边 (Graph index contains extra edges)",
            ));
        }
        let heap_bytes = blocks.capacity() * std::mem::size_of::<(NodeId, GraphBlock)>()
            + incoming.capacity() * std::mem::size_of::<(NodeId, Vec<NodeId>)>()
            + labels.capacity() * std::mem::size_of::<(String, Vec<(NodeId, NodeId)>)>()
            + incoming
                .values()
                .map(|sources| sources.capacity() * std::mem::size_of::<NodeId>())
                .sum::<usize>()
            + labels
                .iter()
                .map(|(label, pairs)| {
                    label.capacity() + pairs.capacity() * std::mem::size_of::<(NodeId, NodeId)>()
                })
                .sum::<usize>()
            + blocks
                .values()
                .map(|block| {
                    block.edges.capacity() * std::mem::size_of::<Edge>()
                        + block
                            .edges
                            .iter()
                            .map(|edge| {
                                edge.label.capacity()
                                    + crate::storage::payload_store::estimate_json_memory(
                                        &edge.metadata,
                                    )
                                    .saturating_sub(std::mem::size_of::<serde_json::Value>())
                            })
                            .sum::<usize>()
                })
                .sum::<usize>();
        Ok(Some(Self {
            mmap,
            blocks,
            incoming,
            labels,
            edge_count,
            heap_bytes,
        }))
    }

    pub fn edges(&self, source: NodeId) -> Option<&[Edge]> {
        let block = self.blocks.get(&source)?;
        Some(&block.edges)
    }

    pub(crate) fn heap_memory_bytes(&self) -> usize {
        self.heap_bytes
    }

    pub fn incoming(&self, target: NodeId) -> &[NodeId] {
        self.incoming.get(&target).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn by_label(&self, label: &str) -> &[(NodeId, NodeId)] {
        self.labels.get(label).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(crate) fn incoming_index(&self) -> &HashMap<NodeId, Vec<NodeId>> {
        &self.incoming
    }
    pub(crate) fn label_index(&self) -> &HashMap<String, Vec<(NodeId, NodeId)>> {
        &self.labels
    }
    pub(crate) fn sources(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.blocks.keys().copied()
    }

    pub fn mapped_bytes(&self) -> usize {
        self.mmap.len()
    }
    pub fn edge_count(&self) -> usize {
        self.edge_count
    }
}

pub fn save<T: crate::VectorType>(
    memtable: &crate::storage::memtable::MemTable<T>,
    db_path: &str,
) -> Result<()> {
    let path = format!("{db_path}.gidx");
    let mut sources = memtable.all_node_ids();
    sources.retain(|id| {
        memtable
            .get_edges(*id)
            .is_some_and(|edges| !edges.is_empty())
    });
    sources.sort_unstable();
    if sources.is_empty() {
        std::fs::remove_file(path).ok();
        return Ok(());
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&(memtable.node_count() as u64).to_le_bytes());
    bytes.extend_from_slice(&(sources.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&std::fs::metadata(db_path)?.len().to_le_bytes());
    bytes.extend_from_slice(&crate::storage::file_format::file_crc32(db_path)?.to_le_bytes());
    for &source in &sources {
        let edges = memtable.get_edges(source).unwrap_or(&[]);
        let data = encode_edges(edges)?;
        bytes.extend_from_slice(&source.to_le_bytes());
        bytes.extend_from_slice(&(edges.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&crc32fast::hash(&data).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&data);
    }
    let mut incoming = HashMap::<NodeId, Vec<NodeId>>::new();
    let mut labels = HashMap::<String, Vec<(NodeId, NodeId)>>::new();
    for &source in &sources {
        for edge in memtable.get_edges(source).unwrap_or(&[]) {
            incoming.entry(edge.target_id).or_default().push(source);
            labels
                .entry(edge.label.clone())
                .or_default()
                .push((source, edge.target_id));
        }
    }
    for values in incoming.values_mut() {
        values.sort_unstable();
        values.dedup();
    }
    for values in labels.values_mut() {
        values.sort_unstable();
        values.dedup();
    }
    let mut directory = Vec::new();
    directory.extend_from_slice(&(incoming.len() as u64).to_le_bytes());
    let mut incoming = incoming.into_iter().collect::<Vec<_>>();
    incoming.sort_by_key(|entry| entry.0);
    for (target, sources) in incoming {
        directory.extend_from_slice(&target.to_le_bytes());
        directory.extend_from_slice(&(sources.len() as u32).to_le_bytes());
        directory.extend_from_slice(&0u32.to_le_bytes());
        for source in sources {
            directory.extend_from_slice(&source.to_le_bytes());
        }
    }
    let mut labels = labels.into_iter().collect::<Vec<_>>();
    labels.sort_by(|a, b| a.0.cmp(&b.0));
    directory.extend_from_slice(&(labels.len() as u32).to_le_bytes());
    directory.extend_from_slice(&0u32.to_le_bytes());
    for (label, pairs) in labels {
        directory.extend_from_slice(&(label.len() as u32).to_le_bytes());
        directory.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
        directory.extend_from_slice(label.as_bytes());
        for (source, target) in pairs {
            directory.extend_from_slice(&source.to_le_bytes());
            directory.extend_from_slice(&target.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&(directory.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&crc32fast::hash(&directory).to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&directory);
    bytes.extend_from_slice(&crc32fast::hash(&bytes).to_le_bytes());
    let tmp = format!("{path}.tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    robust_rename_and_sync(Path::new(&tmp), Path::new(&path))?;
    Ok(())
}

fn encode_edges(edges: &[Edge]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for edge in edges {
        let label = edge.label.as_bytes();
        let metadata = serde_json::to_vec(&edge.metadata)
            .map_err(|error| TriviumError::InvalidInput(format!("边元数据无法序列化: {error}")))?;
        bytes.extend_from_slice(&edge.target_id.to_le_bytes());
        bytes.extend_from_slice(&(label.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&edge.weight.to_le_bytes());
        bytes.extend_from_slice(label);
        bytes.extend_from_slice(&metadata);
    }
    Ok(bytes)
}

fn decode_edges(bytes: &[u8], expected: usize) -> Result<Vec<Edge>> {
    let mut cursor = 0usize;
    let mut edges = Vec::new();
    while cursor < bytes.len() {
        let target_id = read_u64(bytes, cursor, "target")?;
        cursor += 8;
        let label_len = read_u32(bytes, cursor, "label_len")? as usize;
        cursor += 4;
        if label_len > u16::MAX as usize {
            return Err(corrupted(
                "图块标签超过主文件格式上限 (Graph label exceeds main format limit)",
            ));
        }
        let metadata_len = read_u32(bytes, cursor, "metadata_len")? as usize;
        cursor += 4;
        let weight = f32::from_le_bytes(read_array::<4>(bytes, cursor, "weight")?);
        cursor += 4;
        if !weight.is_finite() {
            return Err(corrupted("图块边权重不是有限数"));
        }
        let label_end = cursor
            .checked_add(label_len)
            .ok_or_else(|| corrupted("图块标签偏移溢出"))?;
        let label = std::str::from_utf8(
            bytes
                .get(cursor..label_end)
                .ok_or_else(|| corrupted("图块标签被截断"))?,
        )
        .map_err(|_| corrupted("图块标签不是 UTF-8"))?
        .to_owned();
        cursor = label_end;
        let metadata_end = cursor
            .checked_add(metadata_len)
            .ok_or_else(|| corrupted("图块元数据偏移溢出"))?;
        let metadata = serde_json::from_slice(
            bytes
                .get(cursor..metadata_end)
                .ok_or_else(|| corrupted("图块元数据被截断"))?,
        )
        .map_err(|error| corrupted(&format!("图块元数据无效: {error}")))?;
        cursor = metadata_end;
        edges.push(Edge {
            target_id,
            label,
            weight,
            metadata,
        });
    }
    if edges.len() != expected {
        return Err(corrupted(
            "图块边数量不匹配 (Graph block edge count mismatch)",
        ));
    }
    Ok(edges)
}

fn checked_advance(offset: usize, amount: usize, field: &str) -> Result<usize> {
    offset
        .checked_add(amount)
        .ok_or_else(|| corrupted(&format!("图块 {field} 偏移溢出")))
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize, field: &str) -> Result<[u8; N]> {
    let end = offset
        .checked_add(N)
        .ok_or_else(|| corrupted(&format!("图块 {field} 偏移溢出")))?;
    bytes
        .get(offset..end)
        .ok_or_else(|| corrupted(&format!("图块 {field} 被截断")))?
        .try_into()
        .map_err(|_| corrupted(&format!("图块 {field} 无效")))
}
fn read_u16(bytes: &[u8], offset: usize, field: &str) -> Result<u16> {
    Ok(u16::from_le_bytes(read_array(bytes, offset, field)?))
}
fn read_u32(bytes: &[u8], offset: usize, field: &str) -> Result<u32> {
    Ok(u32::from_le_bytes(read_array(bytes, offset, field)?))
}
fn read_u64(bytes: &[u8], offset: usize, field: &str) -> Result<u64> {
    Ok(u64::from_le_bytes(read_array(bytes, offset, field)?))
}
fn corrupted(message: &str) -> TriviumError {
    TriviumError::CorruptedFile(message.to_owned())
}
