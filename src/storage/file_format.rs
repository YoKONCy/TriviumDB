use crate::VectorType;
use crate::database::StorageMode;
use crate::error::{Result, TriviumError};
use crate::node::{Edge, NodeId};
use crate::storage::memtable::MemTable;
use crate::storage::vec_pool::VecPool;
use memmap2::Mmap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

// ══════ 文件头常量 ══════
const MAGIC: &[u8; 4] = b"TVDB";
const VERSION: u16 = 2; // v2：支持分层存储与单文件格式
const HEADER_SIZE: u64 = 50;

/// 向量文件路径（.tdb → .vec）
fn vec_path_from_db(db_path: &str) -> String {
    format!("{}.vec", db_path)
}

/// 刷新标记文件路径（.tdb → .flush_ok）
/// 该文件是 Mmap 双文件写入的"提交点"，内含 .tdb 和 .vec 的文件大小
fn flush_ok_path_from_db(db_path: &str) -> String {
    format!("{}.flush_ok", db_path)
}

pub fn save<T: VectorType>(
    memtable: &mut MemTable<T>,
    path: &str,
    mode: StorageMode,
) -> Result<()> {
    match mode {
        StorageMode::Mmap => save_mmap(memtable, path),
        StorageMode::Rom => save_rom(memtable, path),
    }
}

/// Mmap 模式保存：分离向量到 .vec 文件，.tdb 纯元数据
fn save_mmap<T: VectorType>(memtable: &mut MemTable<T>, path: &str) -> Result<()> {
    let vec_file_path = vec_path_from_db(path);
    let vec_count = memtable.vec_pool_mut().flush(Path::new(&vec_file_path))?;
    save_tdb(memtable, path, vec_count, true)?;

    // ═══ 跨文件一致性标记（提交点） ═══
    // .vec 和 .tdb 都已原子替换成功后，才写入 .flush_ok 标记。
    // 加载时校验此标记来检测撕裂写入。
    let tdb_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let vec_size = std::fs::metadata(&vec_file_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let marker_path = flush_ok_path_from_db(path);
    let marker_tmp = format!("{}.tmp", &marker_path);
    {
        let mut f = File::create(&marker_tmp)?;
        f.write_all(&tdb_size.to_le_bytes())?;
        f.write_all(&vec_size.to_le_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&marker_tmp, &marker_path)?;

    Ok(())
}

/// Rom 模式保存：把向量合并，写单文件，抛弃 .vec
fn save_rom<T: VectorType>(memtable: &mut MemTable<T>, path: &str) -> Result<()> {
    // 1. 确保在纯内存中获取到完整的合并数组
    memtable.ensure_vectors_cache();
    let total_vectors = memtable.internal_indices().len();

    // 2. 将数据合并写入单文件
    save_tdb(memtable, path, total_vectors, false)?;

    // 3. 将现有的 mmap (如果有) 剥离到内存 delta 中，避免锁住已或将被删除的 .vec
    memtable.vec_pool_mut().detach_mmap();

    // 4. 清理残留的 .vec 和 .flush_ok
    let vec_file_path = vec_path_from_db(path);
    if Path::new(&vec_file_path).exists() {
        std::fs::remove_file(vec_file_path).ok();
    }
    let marker_path = flush_ok_path_from_db(path);
    if Path::new(&marker_path).exists() {
        std::fs::remove_file(marker_path).ok();
    }

    Ok(())
}

/// 核心通用写入逻辑：将 MemTable (Payload & Edge) 写入 .tdb
fn save_tdb<T: VectorType>(
    memtable: &mut MemTable<T>,
    path: &str,
    vec_count: usize,
    is_mmap_mode: bool,
) -> Result<()> {
    if !is_mmap_mode {
        memtable.ensure_vectors_cache();
    }

    let tmp_path = format!("{}.tmp", path);
    let file = File::create(&tmp_path)?;
    let mut w = BufWriter::new(file);

    let dim = memtable.dim();

    // 我们必须按照内部索引数组以防止在重载时 NodeID/Vector 错位
    let internal_indices = memtable.internal_indices();
    // 实际写入的记录数量等于从向量池生成的记录数（包括空洞 Tombstones）
    let node_count = internal_indices.len() as u64;

    let mut all_edges: Vec<(NodeId, &Edge)> = Vec::new();
    let mut payload_size: u64 = 0;

    // 计算 Payload 块大小并收集边
    for &nid in internal_indices {
        if nid != 0 {
            // 有效节点
            if let Some(p) = memtable.get_payload(nid) {
                let json_bytes = serde_json::to_vec(p).unwrap_or_default();
                payload_size += 8 + 4 + json_bytes.len() as u64;
            } else {
                // tombstone 占位符结构：NodeId (0) + len (0) = 12 bytes
                payload_size += 12;
            }
            if let Some(edges) = memtable.get_edges(nid) {
                for edge in edges {
                    all_edges.push((nid, edge));
                }
            }
        } else {
            // 空洞（由于节点被彻底移除，保留内部索引占位）
            payload_size += 12;
        }
    }

    let payload_offset = HEADER_SIZE;
    let vector_offset = if is_mmap_mode {
        0
    } else {
        payload_offset + payload_size
    };
    let vector_size = if is_mmap_mode {
        0
    } else {
        node_count * (dim as u64) * (std::mem::size_of::<T>() as u64)
    };
    let edge_offset = payload_offset + payload_size + vector_size;

    // 1. Header
    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    w.write_all(&(dim as u32).to_le_bytes())?;
    w.write_all(&memtable.next_id_value().to_le_bytes())?;
    w.write_all(&node_count.to_le_bytes())?;
    w.write_all(&payload_offset.to_le_bytes())?;
    w.write_all(&vector_offset.to_le_bytes())?;
    w.write_all(&edge_offset.to_le_bytes())?;

    // 2. Payload Block 包含 Tombstones
    for &nid in internal_indices {
        if nid != 0 {
            if let Some(p) = memtable.get_payload(nid) {
                let json_bytes = serde_json::to_vec(p).unwrap_or_default();
                w.write_all(&nid.to_le_bytes())?;
                w.write_all(&(json_bytes.len() as u32).to_le_bytes())?;
                w.write_all(&json_bytes)?;
                continue;
            }
        }
        // Tombstone
        w.write_all(&0u64.to_le_bytes())?;
        w.write_all(&0u32.to_le_bytes())?;
    }

    // 3. Vector Block (Rom 用)
    if !is_mmap_mode {
        let flat = memtable.flat_vectors();
        w.write_all(bytemuck::cast_slice(flat))?;
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

    w.flush()?;
    let file = w
        .into_inner()
        .map_err(|e| TriviumError::Io(e.into_error()))?;
    file.sync_all()?;
    drop(file);

    std::fs::rename(&tmp_path, path)?;

    tracing::info!(
        "持久化完成: {} 个槽位(含删除), {} 个向量, Mode: {}",
        node_count,
        vec_count,
        if is_mmap_mode { "Mmap" } else { "Rom" }
    );

    Ok(())
}

pub fn load<T: VectorType>(path: &str, _mode: StorageMode) -> Result<MemTable<T>> {
    let file = File::open(path).map_err(TriviumError::Io)?;

    let mmap = unsafe { Mmap::map(&file) }.map_err(|e| TriviumError::Io(e))?;

    if mmap.len() < HEADER_SIZE as usize {
        return Err(TriviumError::Generic("File too small for header".into()));
    }

    let bytes = &mmap[..];
    if &bytes[0..4] != MAGIC {
        return Err(TriviumError::Generic(format!(
            "Invalid file magic: expected TVDB, got {:?}",
            &bytes[0..4]
        )));
    }

    let dim = u32::from_le_bytes(bytes[6..10].try_into().unwrap()) as usize;
    let next_id = u64::from_le_bytes(bytes[10..18].try_into().unwrap());
    let node_count = u64::from_le_bytes(bytes[18..26].try_into().unwrap()) as usize;
    let payload_offset = u64::from_le_bytes(bytes[26..34].try_into().unwrap()) as usize;
    let vector_offset = u64::from_le_bytes(bytes[34..42].try_into().unwrap()) as usize;
    let edge_offset = u64::from_le_bytes(bytes[42..50].try_into().unwrap()) as usize;

    let vec_file_path = vec_path_from_db(path);

    // 如果 vector_offset 是 0 说明是分离架构，且存在 .vec 则按 Mmap 加载
    // 无论目前 config 设置的模式是什么，如果在初始化加载时已经存在可用的 .vec 结构，应当正确恢复它
    // 由下一次 flush 再按照最新的 StorageMode 决定写出格式
    if vector_offset == 0 && Path::new(&vec_file_path).exists() {
        // ═══ 跨文件一致性校验 ═══
        // 检查 .flush_ok 标记是否存在且文件大小吻合，防止撕裂写入
        let marker_path = flush_ok_path_from_db(path);
        let flush_ok_valid = (|| -> Option<bool> {
            let marker_bytes = std::fs::read(&marker_path).ok()?;
            if marker_bytes.len() < 16 {
                return Some(false);
            }
            let stored_tdb = u64::from_le_bytes(marker_bytes[0..8].try_into().ok()?);
            let stored_vec = u64::from_le_bytes(marker_bytes[8..16].try_into().ok()?);
            let actual_tdb = std::fs::metadata(path).ok()?.len();
            let actual_vec = std::fs::metadata(&vec_file_path).ok()?.len();
            Some(stored_tdb == actual_tdb && stored_vec == actual_vec)
        })()
        .unwrap_or(false);

        if flush_ok_valid {
            load_v2(
                bytes,
                dim,
                next_id,
                node_count,
                payload_offset,
                edge_offset,
                &vec_file_path,
                &mmap,
            )
        } else {
            tracing::warn!(
                "检测到 .tdb/.vec 跨文件撕裂（.flush_ok 标记缺失或不匹配），\
                 降级为忽略 .vec 的安全模式加载，增量数据将由 WAL 回放恢复"
            );
            // 降级：忽略 .vec，仅从 .tdb 的 payload/edge 恢复骨架
            // node_count 仍有效，但向量数据丢失，需要 WAL 或下次 flush 重建
            load_v1_rom(
                bytes,
                dim,
                next_id,
                node_count,
                payload_offset,
                vector_offset,
                edge_offset,
                &mmap,
            )
        }
    } else {
        load_v1_rom(
            bytes,
            dim,
            next_id,
            node_count,
            payload_offset,
            vector_offset,
            edge_offset,
            &mmap,
        )
    }
}

/// 分离向量 .vec 文件的加载
fn load_v2<T: VectorType>(
    bytes: &[u8],
    dim: usize,
    next_id: u64,
    node_count: usize,
    payload_offset: usize,
    edge_offset: usize,
    vec_file_path: &str,
    tdb_mmap: &Mmap,
) -> Result<MemTable<T>> {
    let vec_pool = VecPool::<T>::open(Path::new(vec_file_path), dim, node_count)?;
    let mut memtable = MemTable::new_with_vec_pool(dim, next_id, vec_pool);
    load_payloads(
        &mut memtable,
        bytes,
        node_count,
        payload_offset,
        edge_offset,
    )?;
    load_edges(&mut memtable, bytes, edge_offset, tdb_mmap.len())?;
    Ok(memtable)
}

/// 单文件内存向量的加载
fn load_v1_rom<T: VectorType>(
    bytes: &[u8],
    dim: usize,
    next_id: u64,
    node_count: usize,
    payload_offset: usize,
    vector_offset: usize,
    edge_offset: usize,
    tdb_mmap: &Mmap,
) -> Result<MemTable<T>> {
    let mut memtable = MemTable::new_with_next_id(dim, next_id);
    let vector_bytes_per_elem = std::mem::size_of::<T>();
    let expected_vec_size = node_count * dim * vector_bytes_per_elem;

    if vector_offset + expected_vec_size > tdb_mmap.len() {
        return Err(TriviumError::Generic(
            "Vector block exceeds file size".into(),
        ));
    }

    // 先恢复映射位置和 Payload
    load_payloads(
        &mut memtable,
        bytes,
        node_count,
        payload_offset,
        vector_offset,
    )?;

    let vec_block = &bytes[vector_offset..vector_offset + expected_vec_size];
    let is_aligned = (vec_block.as_ptr() as usize) % std::mem::align_of::<T>() == 0;

    // 因为 load_payloads 已经按内部索引位置推了占位符（包含 Tombstone），
    // 接下来我们只需要把所有的 vector_block 推入 VecPool！
    if is_aligned {
        let t_slice =
            unsafe { std::slice::from_raw_parts(vec_block.as_ptr() as *const T, node_count * dim) };
        memtable.vec_pool_mut().push(t_slice);
    } else {
        // 不对齐
        let mut v = Vec::with_capacity(node_count * dim);
        for i in 0..(node_count * dim) {
            let off = i * vector_bytes_per_elem;
            let chunk = &vec_block[off..off + vector_bytes_per_elem];
            let elem: T = bytemuck::pod_read_unaligned(chunk);
            v.push(elem);
        }
        memtable.vec_pool_mut().push(&v);
    }

    load_edges(&mut memtable, bytes, edge_offset, tdb_mmap.len())?;
    Ok(memtable)
}

/// 解析 Payload Block，处理 Tombstone
fn load_payloads<T: VectorType>(
    memtable: &mut MemTable<T>,
    bytes: &[u8],
    node_count: usize,
    offset: usize,
    end_offset: usize,
) -> Result<()> {
    let mut cursor = offset;
    for _ in 0..node_count {
        if cursor + 12 > end_offset {
            return Err(TriviumError::Generic("Payload block overflow".into()));
        }
        let nid = u64::from_le_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let json_len = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;

        if nid == 0 && json_len == 0 {
            memtable.register_tombstone()?;
            continue;
        }

        if cursor + json_len > end_offset {
            return Err(TriviumError::Generic("JSON data overflow".into()));
        }
        let payload: serde_json::Value = serde_json::from_slice(&bytes[cursor..cursor + json_len])
            .map_err(|e| TriviumError::Generic(format!("JSON parse error: {}", e)))?;
        cursor += json_len;

        memtable.register_node(nid, payload)?;
    }
    Ok(())
}

fn load_edges<T: VectorType>(
    memtable: &mut MemTable<T>,
    bytes: &[u8],
    edge_offset: usize,
    file_len: usize,
) -> Result<()> {
    let mut cursor = edge_offset;
    while cursor + 18 <= file_len {
        let src_id = u64::from_le_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let dst_id = u64::from_le_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let label_len = u16::from_le_bytes(bytes[cursor..cursor + 2].try_into().unwrap()) as usize;
        cursor += 2;
        if cursor + label_len + 4 > file_len {
            break;
        }
        let label = String::from_utf8(bytes[cursor..cursor + label_len].to_vec())
            .map_err(|e| TriviumError::Generic(format!("Label decode error: {}", e)))?;
        cursor += label_len;
        let weight = f32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
        cursor += 4;
        memtable.link(src_id, dst_id, label, weight)?;
    }
    Ok(())
}
