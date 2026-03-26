use crate::error::{Result, TriviumError};
use crate::node::{Edge, NodeId};
use crate::storage::memtable::MemTable;
use crate::VectorType;
use memmap2::Mmap;
use std::fs::File;
use std::io::{BufWriter, Write};

// ══════ 文件头常量 ══════
const MAGIC: &[u8; 4] = b"TVDB";
const VERSION: u16 = 1;
const HEADER_SIZE: u64 = 50;

/// 将当前 MemTable 的全部内容写入单个 .tdb 二进制文件
/// 
/// 安全写入策略（防断电损坏）：
///   1. 写入 .tdb.tmp 临时文件
///   2. fsync 确保数据落盘
///   3. 原子 rename 替换旧 .tdb 文件
pub fn save<T: VectorType>(memtable: &MemTable<T>, path: &str) -> Result<()> {
    let tmp_path = format!("{}.tmp", path);
    let file = File::create(&tmp_path)?;
    let mut w = BufWriter::new(file);

    let dim = memtable.dim();
    let mut node_ids = memtable.all_node_ids();
    node_ids.sort();
    let node_count = node_ids.len() as u64;

    let mut all_edges: Vec<(NodeId, &Edge)> = Vec::new();
    for &nid in &node_ids {
        if let Some(edges) = memtable.get_edges(nid) {
            for edge in edges {
                all_edges.push((nid, edge));
            }
        }
    }

    let mut payload_size: u64 = 0;
    for &nid in &node_ids {
        if let Some(p) = memtable.get_payload(nid) {
            let json_bytes = serde_json::to_vec(p).unwrap_or_default();
            payload_size += 8 + 4 + json_bytes.len() as u64;
        }
    }
    let vector_size: u64 = node_count * (dim as u64) * (std::mem::size_of::<T>() as u64);

    let payload_offset = HEADER_SIZE;
    let vector_offset = payload_offset + payload_size;
    let edge_offset = vector_offset + vector_size;

    // 1. Header
    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    w.write_all(&(dim as u32).to_le_bytes())?;
    w.write_all(&memtable.next_id_value().to_le_bytes())?;
    w.write_all(&node_count.to_le_bytes())?;
    w.write_all(&payload_offset.to_le_bytes())?;
    w.write_all(&vector_offset.to_le_bytes())?;
    w.write_all(&edge_offset.to_le_bytes())?;

    // 2. Payload Block
    for &nid in &node_ids {
        if let Some(p) = memtable.get_payload(nid) {
            let json_bytes = serde_json::to_vec(p).unwrap_or_default();
            w.write_all(&nid.to_le_bytes())?;
            w.write_all(&(json_bytes.len() as u32).to_le_bytes())?;
            w.write_all(&json_bytes)?;
        }
    }

    // 3. Vector Block (SoA 连续布局，写入后可被 mmap 直接映射)
    for &nid in &node_ids {
        if let Some(vec) = memtable.get_vector(nid) {
            let bytes = bytemuck::cast_slice(vec);
            w.write_all(bytes)?;
        }
    }

    // 4. Edge Block
    for (src_id, edge) in &all_edges {
        w.write_all(&src_id.to_le_bytes())?;
        w.write_all(&edge.target_id.to_le_bytes())?;
        let label_bytes = edge.label.as_bytes();
        w.write_all(&(label_bytes.len() as u16).to_le_bytes())?;
        w.write_all(label_bytes)?;
        w.write_all(&edge.weight.to_le_bytes())?;
    }

    // 5. 刷缓冲 → fsync → 原子 rename
    w.flush()?;
    let file = w.into_inner().map_err(|e| TriviumError::Io(e.into_error()))?;
    file.sync_all()?;  // fsync: 确保数据真正落盘
    drop(file);

    std::fs::rename(&tmp_path, path)?; // 原子替换

    Ok(())
}

/// 从 .tdb 文件加载并重建 MemTable。
pub fn load<T: VectorType>(path: &str) -> Result<MemTable<T>> {
    let file = File::open(path).map_err(TriviumError::Io)?;

    let mmap = unsafe { Mmap::map(&file) }
        .map_err(|e| TriviumError::Io(e))?;

    if mmap.len() < HEADER_SIZE as usize {
        return Err(TriviumError::Generic("File too small for header".into()));
    }

    let bytes = &mmap[..];
    if &bytes[0..4] != MAGIC {
        return Err(TriviumError::Generic(format!(
            "Invalid file magic: expected TVDB, got {:?}", &bytes[0..4]
        )));
    }

    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != VERSION {
        return Err(TriviumError::Generic(format!("Unsupported version: {}", version)));
    }

    let dim = u32::from_le_bytes(bytes[6..10].try_into().unwrap()) as usize;
    let next_id = u64::from_le_bytes(bytes[10..18].try_into().unwrap());
    let node_count = u64::from_le_bytes(bytes[18..26].try_into().unwrap()) as usize;
    let payload_offset = u64::from_le_bytes(bytes[26..34].try_into().unwrap()) as usize;
    let vector_offset = u64::from_le_bytes(bytes[34..42].try_into().unwrap()) as usize;
    let edge_offset = u64::from_le_bytes(bytes[42..50].try_into().unwrap()) as usize;

    let vector_bytes_per_elem = std::mem::size_of::<T>();
    let expected_vec_size = node_count * dim * vector_bytes_per_elem;
    if vector_offset + expected_vec_size > mmap.len() {
        return Err(TriviumError::Generic("Vector block exceeds file size".into()));
    }

    let mut memtable = MemTable::new_with_next_id(dim, next_id);

    // Payload Block
    let mut cursor = payload_offset;
    let mut node_ids_in_order = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        if cursor + 12 > vector_offset {
            return Err(TriviumError::Generic("Payload block overflow".into()));
        }
        let nid = u64::from_le_bytes(bytes[cursor..cursor+8].try_into().unwrap());
        cursor += 8;
        let json_len = u32::from_le_bytes(bytes[cursor..cursor+4].try_into().unwrap()) as usize;
        cursor += 4;
        if cursor + json_len > vector_offset {
            return Err(TriviumError::Generic("JSON data overflow".into()));
        }
        let payload: serde_json::Value = serde_json::from_slice(&bytes[cursor..cursor+json_len])
            .map_err(|e| TriviumError::Generic(format!("JSON parse error: {}", e)))?;
        cursor += json_len;
        node_ids_in_order.push((nid, payload));
    }

    // Vector Block
    let vec_block = &bytes[vector_offset..vector_offset + expected_vec_size];
    let is_aligned = (vec_block.as_ptr() as usize) % std::mem::align_of::<T>() == 0;

    for (vec_idx, (nid, payload)) in node_ids_in_order.iter().enumerate() {
        let vec_start = vec_idx * dim;

        let vector: Vec<T> = if is_aligned {
            let t_slice = unsafe {
                std::slice::from_raw_parts(
                    vec_block.as_ptr().add(vec_start * vector_bytes_per_elem) as *const T,
                    dim,
                )
            };
            t_slice.to_vec()
        } else {
            let start = vec_start * vector_bytes_per_elem;
            let mut v = Vec::with_capacity(dim);
            for j in 0..dim {
                let off = start + j * vector_bytes_per_elem;
                let chunk = &vec_block[off..off + vector_bytes_per_elem];
                let elem: T = bytemuck::pod_read_unaligned(chunk);
                v.push(elem);
            }
            v
        };

        memtable.raw_insert(*nid, &vector, payload.clone())?;
    }

    // Edge Block
    let mut cursor = edge_offset;
    while cursor + 18 <= mmap.len() {
        let src_id = u64::from_le_bytes(bytes[cursor..cursor+8].try_into().unwrap());
        cursor += 8;
        let dst_id = u64::from_le_bytes(bytes[cursor..cursor+8].try_into().unwrap());
        cursor += 8;
        let label_len = u16::from_le_bytes(bytes[cursor..cursor+2].try_into().unwrap()) as usize;
        cursor += 2;
        if cursor + label_len + 4 > mmap.len() { break; }
        let label = String::from_utf8(bytes[cursor..cursor+label_len].to_vec())
            .map_err(|e| TriviumError::Generic(format!("Label decode error: {}", e)))?;
        cursor += label_len;
        let weight = f32::from_le_bytes(bytes[cursor..cursor+4].try_into().unwrap());
        cursor += 4;
        memtable.link(src_id, dst_id, label, weight)?;
    }

    Ok(memtable)
}
