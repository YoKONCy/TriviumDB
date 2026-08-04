use crate::VectorType;
use crate::database::StorageMode;
use crate::error::{Result, TriviumError};
use crate::index::bq::BqSignature;
use crate::node::{Edge, NodeId};
use crate::storage::memtable::MemTable;
use crate::storage::vec_pool::VecPool;
use memmap2::Mmap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// Windows 下应对杀毒软件瞬态文件锁定的原子重命名
///
/// 杀毒软件（Windows Defender / 火绒等）会在文件关闭的瞬间以独占模式扫描，
/// 导致紧随其后的 rename 操作遇到 ERROR_SHARING_VIOLATION (32) 或
/// ERROR_ACCESS_DENIED (5)。此函数通过短暂指数退避重试来等待杀软释放锁。
///
/// 在非 Windows 平台上直接调用 std::fs::rename，零开销。
fn robust_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    {
        std::fs::rename(from, to)
    }

    #[cfg(windows)]
    {
        let max_retries = 10;
        let mut delay = std::time::Duration::from_millis(1);
        for attempt in 0..max_retries {
            match std::fs::rename(from, to) {
                Ok(()) => return Ok(()),
                Err(e) if attempt < max_retries - 1 => {
                    let os_err = e.raw_os_error();
                    // ERROR_ACCESS_DENIED (5) 或 ERROR_SHARING_VIOLATION (32)
                    if os_err == Some(5) || os_err == Some(32) {
                        tracing::debug!(
                            "robust_rename: 第 {} 次重试失败 (attempt {} failed), os_error={:?}, {:?} 后重试 (retrying in {:?})",
                            attempt + 1,
                            attempt + 1,
                            os_err,
                            delay,
                            delay
                        );
                        std::thread::sleep(delay);
                        delay = (delay * 2).min(std::time::Duration::from_millis(50));
                        continue;
                    }
                    return Err(e);
                }
                Err(e) => return Err(e),
            }
        }
        // 逻辑上不可达：循环必定 return。防御性返回避免审查标记。
        Err(std::io::Error::other("robust_rename 重试次数耗尽 (exhausted retries)"))
    }
}

// ══════ 文件头常量 ══════
const MAGIC: &[u8; 4] = b"TVDB";
const VERSION: u16 = 5; // v5: 新增 BQ Metadata Block 持久化，header 扩展至 58 字节
const HEADER_SIZE: u64 = 58;

/// 从字节切片中安全读取小端序整数（军工级：禁止裸 unwrap）
///
/// GJB-5000B 条款 6.3.2 要求：所有反序列化路径必须对畸形输入返回明确错误，
/// 不得触发 panic 导致进程终止。
#[inline]
fn read_u16_le(bytes: &[u8], offset: usize, field: &str) -> Result<u16> {
    bytes
        .get(offset..offset + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| TriviumError::CorruptedFile(format!("{} at offset {}", field, offset)))
}

#[inline]
fn read_u32_le(bytes: &[u8], offset: usize, field: &str) -> Result<u32> {
    bytes
        .get(offset..offset + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| TriviumError::CorruptedFile(format!("{} at offset {}", field, offset)))
}

#[inline]
fn read_u64_le(bytes: &[u8], offset: usize, field: &str) -> Result<u64> {
    bytes
        .get(offset..offset + 8)
        .and_then(|s| s.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| TriviumError::CorruptedFile(format!("{} at offset {}", field, offset)))
}

#[inline]
fn read_f32_le(bytes: &[u8], offset: usize, field: &str) -> Result<f32> {
    bytes
        .get(offset..offset + 4)
        .and_then(|s| s.try_into().ok())
        .map(f32::from_le_bytes)
        .ok_or_else(|| TriviumError::CorruptedFile(format!("{} at offset {}", field, offset)))
}

/// 向量文件路径（.tdb → .vec）
fn vec_path_from_db(db_path: &str) -> String {
    format!("{}.vec", db_path)
}

/// 刷新标记文件路径（.tdb → .flush_ok）
/// 该文件是 Mmap 双文件写入的"提交点"，内含 .tdb 和 .vec 的文件大小
fn flush_ok_path_from_db(db_path: &str) -> String {
    format!("{}.flush_ok", db_path)
}

/// QuIVer 索引文件路径（.tdb → .tdb.quiver）
fn quiver_path_from_db(db_path: &str) -> String {
    format!("{}.quiver", db_path)
}

pub fn save<T: VectorType>(
    memtable: &mut MemTable<T>,
    path: &str,
    mode: StorageMode,
) -> Result<()> {
    match mode {
        StorageMode::Mmap => save_mmap(memtable, path),
        StorageMode::Rom => save_rom(memtable, path),
    }?;

    // 顺便持久化 QuIVer 索引（如果已构建）
    let quiver_path = quiver_path_from_db(path);
    if let Some(quiver) = memtable.quiver() {
        if let Err(e) = quiver.save_to_file(std::path::Path::new(&quiver_path)) {
            tracing::warn!("QuIVer 索引持久化失败（不影响主数据）(QuIVer persist failed, main data unaffected): {}", e);
        }
    } else {
        // QuIVer 不存在时清理残留文件
        let qp = std::path::Path::new(&quiver_path);
        if qp.exists() {
            std::fs::remove_file(qp).ok();
        }
    }

    Ok(())
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
    let marker_tmp = format!("{}.tmp", marker_path);
    {
        let mut f = File::create(&marker_tmp)?;
        f.write_all(&tdb_size.to_le_bytes())?;
        f.write_all(&vec_size.to_le_bytes())?;
        f.sync_all()?;
    }
    robust_rename(Path::new(&marker_tmp), Path::new(&marker_path))?;

    Ok(())
}

/// Rom 模式保存：把向量合并，写单文件，抛弃 .vec
fn save_rom<T: VectorType>(memtable: &mut MemTable<T>, path: &str) -> Result<()> {
    // 1. 确保在纯内存中获取到完整的合并数组（Rom 单文件持久化必须物化 flat）
    memtable.ensure_vectors_cache(true);
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
    // 始终确保 BQ 签名和向量缓存已构建（v5 持久化需要写入 BQ Block）。
    // 仅 Rom 单文件模式（!is_mmap_mode）需要写出连续 flat 向量块，才物化 merged；
    // Mmap 分离模式下向量已在 .vec 文件，无需把整库复制入堆。
    memtable.ensure_vectors_cache(!is_mmap_mode);

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

    // 预计算 Edge Block 大小，以便确定 BQ Block 的 offset
    let mut edge_block_size: u64 = 0;
    for (_src_id, edge) in &all_edges {
        // src(8) + dst(8) + label_len(2) + label_bytes + weight(4)
        edge_block_size += 8 + 8 + 2 + edge.label.len() as u64 + 4;
    }
    let bq_offset = edge_offset + edge_block_size;

    // 1. Header (v5: 58 字节)
    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    w.write_all(&(dim as u32).to_le_bytes())?;
    w.write_all(&memtable.next_id_value().to_le_bytes())?;
    w.write_all(&node_count.to_le_bytes())?;
    w.write_all(&payload_offset.to_le_bytes())?;
    w.write_all(&vector_offset.to_le_bytes())?;
    w.write_all(&edge_offset.to_le_bytes())?;
    w.write_all(&bq_offset.to_le_bytes())?; // v5 新增

    // 2. Payload Block 包含 Tombstones
    for &nid in internal_indices {
        if nid != 0
            && let Some(p) = memtable.get_payload(nid)
        {
            let json_bytes = serde_json::to_vec(p).unwrap_or_default();
            w.write_all(&nid.to_le_bytes())?;
            w.write_all(&(json_bytes.len() as u32).to_le_bytes())?;
            w.write_all(&json_bytes)?;
            continue;
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

    // 5. BQ Metadata Block（v5 新增）
    //    确保 BQ 签名已构建，然后 bytemuck 零拷贝写入
    let bq_sigs = memtable.bq_signatures_slice();
    let bq_count = bq_sigs.len() as u64;
    w.write_all(&bq_count.to_le_bytes())?; // 8 字节：签名数量
    if !bq_sigs.is_empty() {
        // SAFETY: BqSignature 实现了 Pod + Zeroable，且为 #[repr(C)]，
        // bytemuck::cast_slice 保证合法的字节级重新解释
        w.write_all(bytemuck::cast_slice(bq_sigs))?;
    }

    w.flush()?;
    let file = w
        .into_inner()
        .map_err(|e| TriviumError::Io(e.into_error()))?;
    file.sync_all()?;
    drop(file);

    robust_rename(Path::new(&tmp_path), Path::new(path))?;

    tracing::info!(
        "持久化完成 (Flush completed): {} 个槽位(含删除), {} 个向量, {} 个 BQ 签名, 模式 (Mode): {}",
        node_count,
        vec_count,
        bq_count,
        if is_mmap_mode { "Mmap" } else { "Rom" }
    );

    Ok(())
}

pub fn load<T: VectorType>(path: &str, _mode: StorageMode) -> Result<MemTable<T>> {
    let file = File::open(path).map_err(TriviumError::Io)?;

    let mmap = unsafe { Mmap::map(&file) }.map_err(TriviumError::Io)?;

    if mmap.len() < HEADER_SIZE as usize {
        return Err(TriviumError::CorruptedFile(
            "文件头过小 (File too small for header)".into(),
        ));
    }

    let bytes = &mmap[..];
    if &bytes[0..4] != MAGIC {
        return Err(TriviumError::CorruptedFile(format!(
            "文件魔数无效 (Invalid file magic): 期望 TVDB，实际 {:?}",
            &bytes[0..4]
        )));
    }

    let version = read_u16_le(bytes, 4, "header version")?;
    let dim = read_u32_le(bytes, 6, "header dim")? as usize;
    let next_id = read_u64_le(bytes, 10, "header next_id")?;
    let node_count = read_u64_le(bytes, 18, "header node_count")? as usize;
    let payload_offset = read_u64_le(bytes, 26, "header payload_offset")? as usize;
    let vector_offset = read_u64_le(bytes, 34, "header vector_offset")? as usize;
    let edge_offset = read_u64_le(bytes, 42, "header edge_offset")? as usize;

    // v5: 新增 BQ Block offset；v4 及以下兼容旧格式
    let bq_offset = if version >= 5 && mmap.len() >= 58 {
        read_u64_le(bytes, 50, "header bq_offset")? as usize
    } else {
        0 // 0 表示无 BQ Block
    };

    // ═══ 文件结构完整性校验 ═══
    // 防止引擎静默加载被截断的 .tdb 文件（扇区撕裂 / 断电 / 外部篡改）。
    // 通过 header 中声明的各 block offset 计算期望的最小文件大小，
    // 与实际文件大小比对。任何不一致都意味着文件被截断。
    let file_len = mmap.len();

    if payload_offset > file_len {
        return Err(TriviumError::CorruptedFile(format!(
            "payload_offset ({}) 超出文件大小 ({})，文件被截断 (file truncated)",
            payload_offset, file_len
        )));
    }
    if edge_offset > file_len {
        return Err(TriviumError::CorruptedFile(format!(
            "edge_offset ({}) 超出文件大小 ({})，文件被截断 (file truncated)",
            edge_offset, file_len
        )));
    }
    if bq_offset > 0 {
        if bq_offset > file_len {
            return Err(TriviumError::CorruptedFile(format!(
                "bq_offset ({}) 超出文件大小 ({})，文件被截断 (file truncated)",
                bq_offset, file_len
            )));
        }
        // BQ Block 完整性：读取 bq_count 并验证整个 Block 未被截断
        if bq_offset + 8 <= file_len {
            let bq_count = u64::from_le_bytes(
                bytes[bq_offset..bq_offset + 8].try_into().unwrap_or([0; 8]),
            ) as usize;
            if bq_count > 0 {
                let sig_size = std::mem::size_of::<BqSignature>();
                let expected_bq_end = bq_offset + 8 + bq_count * sig_size;
                if expected_bq_end > file_len {
                    return Err(TriviumError::CorruptedFile(format!(
                        "BQ 块被截断 (BQ block truncated): 期望 {} 字节 (offset {} + 8 + {} × {})，\
                         实际文件大小 {} 字节",
                        expected_bq_end, bq_offset, bq_count, sig_size, file_len
                    )));
                }
            }
        }
    }

    // 兼容旧版 V3 及以下的冗余区块
    let edge_limit_offset = if version >= 4 {
        // v4/v5: edge block 的上限由 bq_offset（v5）或 文件末尾（v4）决定
        if version >= 5 && bq_offset > 0 {
            bq_offset
        } else {
            mmap.len()
        }
    } else if mmap.len() >= 58 {
        read_u64_le(bytes, 50, "header edge_limit_offset")? as usize
    } else {
        mmap.len()
    };

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
            let mut mt = load_v2(
                bytes,
                dim,
                next_id,
                node_count,
                payload_offset,
                edge_offset,
                edge_limit_offset,
                &vec_file_path,
                &mmap,
            )?;
            // 尝试从 BQ Block 恢复签名
            load_bq_block(&mut mt, bytes, bq_offset, mmap.len());
            load_quiver_index(&mut mt, path);
            Ok(mt)
        } else {
            tracing::warn!(
                "检测到 .tdb/.vec 跨文件撕裂 (Cross-file tear detected)（.flush_ok 标记缺失或不匹配），\
                 将尝试按当前文件恢复，失败后再降级为仅加载 .tdb 元数据"
            );
            match load_v2(
                bytes,
                dim,
                next_id,
                node_count,
                payload_offset,
                edge_offset,
                edge_limit_offset,
                &vec_file_path,
                &mmap,
            ) {
                Ok(mut mt) => {
                    load_bq_block(&mut mt, bytes, bq_offset, mmap.len());
                    load_quiver_index(&mut mt, path);
                    Ok(mt)
                }
                Err(e) => {
                    tracing::warn!("当前 .tdb/.vec 组合不可用，进入安全降级恢复 (Falling back to metadata-only recovery): {}", e);
                    let mut mt = load_v2_metadata_only(
                        bytes,
                        dim,
                        next_id,
                        node_count,
                        payload_offset,
                        edge_offset,
                        edge_limit_offset,
                    )?;
                    load_bq_block(&mut mt, bytes, bq_offset, mmap.len());
                    load_quiver_index(&mut mt, path);
                    Ok(mt)
                }
            }
        }
    } else {
        let mut mt = load_v1_rom(
            bytes,
            dim,
            next_id,
            node_count,
            payload_offset,
            vector_offset,
            edge_offset,
            edge_limit_offset,
            &mmap,
        )?;
        // 尝试从 BQ Block 恢复签名
        load_bq_block(&mut mt, bytes, bq_offset, mmap.len());
        // 尝试加载 QuIVer 索引
        load_quiver_index(&mut mt, path);
        Ok(mt)
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
    edge_limit_offset: usize,
    vec_file_path: &str,
    _tdb_mmap: &Mmap,
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
    load_edges(&mut memtable, bytes, edge_offset, edge_limit_offset)?;
    Ok(memtable)
}

/// 分离向量模式的安全降级加载：只读取 .tdb 元数据，向量由 WAL 回放补齐
fn load_v2_metadata_only<T: VectorType>(
    bytes: &[u8],
    dim: usize,
    next_id: u64,
    node_count: usize,
    payload_offset: usize,
    edge_offset: usize,
    edge_limit_offset: usize,
) -> Result<MemTable<T>> {
    let mut memtable = MemTable::new_with_next_id(dim, next_id);
    load_payloads(
        &mut memtable,
        bytes,
        node_count,
        payload_offset,
        edge_offset,
    )?;
    let zero = vec![T::zero(); dim];
    for id in memtable.internal_indices().to_vec() {
        if id != 0
            && let Some(payload) = memtable.get_payload(id).cloned()
        {
            memtable.raw_insert(id, &zero, payload)?;
        }
    }
    load_edges(&mut memtable, bytes, edge_offset, edge_limit_offset)?;
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
    edge_limit_offset: usize,
    tdb_mmap: &Mmap,
) -> Result<MemTable<T>> {
    let mut memtable = MemTable::new_with_next_id(dim, next_id);
    let vector_bytes_per_elem = std::mem::size_of::<T>();
    let expected_vec_size = node_count * dim * vector_bytes_per_elem;

    if vector_offset + expected_vec_size > tdb_mmap.len() {
        return Err(TriviumError::CorruptedFile(
            "向量块超出文件大小 (Vector block exceeds file size)".into(),
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
    let is_aligned = (vec_block.as_ptr() as usize).is_multiple_of(std::mem::align_of::<T>());

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

    load_edges(&mut memtable, bytes, edge_offset, edge_limit_offset)?;
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
        if cursor.saturating_add(12) > end_offset {
            return Err(TriviumError::CorruptedFile("Payload 块溢出 (Payload block overflow)".into()));
        }
        let nid = read_u64_le(bytes, cursor, "payload node_id")?;
        cursor += 8;
        let json_len = read_u32_le(bytes, cursor, "payload json_len")? as usize;
        cursor += 4;

        if nid == 0 && json_len == 0 {
            memtable.register_tombstone()?;
            continue;
        }

        if cursor.saturating_add(json_len) > end_offset {
            return Err(TriviumError::CorruptedFile("JSON 数据溢出 (JSON data overflow)".into()));
        }
        let payload: serde_json::Value = serde_json::from_slice(&bytes[cursor..cursor + json_len])
            .map_err(|e| TriviumError::CorruptedFile(format!("JSON 解析错误 (JSON parse error): {}", e)))?;
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
    while cursor.saturating_add(18) <= file_len {
        let src_id = read_u64_le(bytes, cursor, "edge src_id")?;
        cursor += 8;
        let dst_id = read_u64_le(bytes, cursor, "edge dst_id")?;
        cursor += 8;
        let label_len = read_u16_le(bytes, cursor, "edge label_len")? as usize;
        cursor += 2;
        if cursor.saturating_add(label_len).saturating_add(4) > file_len {
            break;
        }
        let label = String::from_utf8(bytes[cursor..cursor + label_len].to_vec())
            .map_err(|e| TriviumError::CorruptedFile(format!("标签解码错误 (Label decode error): {}", e)))?;
        cursor += label_len;
        let weight = read_f32_le(bytes, cursor, "edge weight")?;
        cursor += 4;
        memtable.link(src_id, dst_id, label, weight)?;
    }
    Ok(())
}

/// 从 .tdb 的 BQ Block 中恢复 BQ 签名数组（v5+ 格式）
///
/// 如果 bq_offset 为 0 或数据不完整，静默跳过（首次查询时惰性重建）。
fn load_bq_block<T: VectorType>(
    memtable: &mut MemTable<T>,
    bytes: &[u8],
    bq_offset: usize,
    file_len: usize,
) {
    if bq_offset == 0 || bq_offset + 8 > file_len {
        return; // 无 BQ Block 或文件不完整
    }

    let bq_count =
        u64::from_le_bytes(bytes[bq_offset..bq_offset + 8].try_into().unwrap_or([0; 8])) as usize;

    if bq_count == 0 {
        return;
    }

    let sig_size = std::mem::size_of::<BqSignature>();
    let data_start = bq_offset + 8;
    let data_end = data_start + bq_count * sig_size;

    if data_end > file_len {
        tracing::warn!(
            "BQ Block 数据不完整 (BQ Block data incomplete)（需要 {} 字节，文件仅剩 {} 字节），跳过恢复",
            bq_count * sig_size,
            file_len.saturating_sub(data_start)
        );
        return;
    }

    let bq_bytes = &bytes[data_start..data_end];

    // 检查对齐（BqSignature 为 [u64; 32]，需要 8 字节对齐）
    let is_aligned =
        (bq_bytes.as_ptr() as usize).is_multiple_of(std::mem::align_of::<BqSignature>());

    let sigs: Vec<BqSignature> = if is_aligned {
        // SAFETY: BqSignature: Pod + Zeroable + #[repr(C)]，对齐已验证，长度精确
        let slice: &[BqSignature] = bytemuck::cast_slice(bq_bytes);
        slice.to_vec()
    } else {
        // 不对齐时逐个 pod_read_unaligned
        let mut v = Vec::with_capacity(bq_count);
        for i in 0..bq_count {
            let off = i * sig_size;
            let sig: BqSignature = bytemuck::pod_read_unaligned(&bq_bytes[off..off + sig_size]);
            v.push(sig);
        }
        v
    };

    memtable.set_bq_signatures(sigs);
    tracing::info!("从 .tdb 恢复了 {} 个 BQ 签名 (Restored {} BQ signatures from .tdb)（零拷贝加载）", bq_count, bq_count);
}

/// 尝试从 .tdb.quiver 文件加载 QuIVer 索引
///
/// 如果文件不存在或加载失败，静默跳过（首次查询时惰性重建）。
fn load_quiver_index<T: VectorType>(memtable: &mut MemTable<T>, db_path: &str) {
    use crate::index::quiver::QuIVer;

    let quiver_path = quiver_path_from_db(db_path);
    let qp = std::path::Path::new(&quiver_path);
    if !qp.exists() {
        return;
    }

    match QuIVer::load_from_file(qp) {
        Ok(quiver) => {
            memtable.set_quiver_index(quiver);
            // 加载后即进入 QuIVer 检索路径：冷向量随机按需读取，
            // 提示 OS 关闭顺序预读以降低 PageCache 常驻
            memtable.vec_pool_mut().advise_random();
        }
        Err(e) => {
            tracing::warn!(
                "QuIVer 索引加载失败 (QuIVer index load failed)（将在首次查询时自动重建）: {}",
                e
            );
            // 删除损坏的文件
            std::fs::remove_file(qp).ok();
        }
    }
}
