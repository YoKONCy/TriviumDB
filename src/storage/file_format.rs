use crate::VectorType;
use crate::database::StorageMode;
use crate::error::{Result, TriviumError};
use crate::index::bq::BqSignature;
use crate::node::{Edge, NodeId};
use crate::storage::fs::robust_rename_and_sync;
use crate::storage::memtable::MemTable;
use crate::storage::vec_pool::VecPool;
use memmap2::Mmap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

// ══════ 文件头常量 ══════
const MAGIC: &[u8; 4] = b"TVDB";
const VERSION: u16 = 5; // v5: 新增 BQ Metadata Block 持久化，header 扩展至 58 字节
const HEADER_SIZE: u64 = 58;

// ══════ flush_ok 提交标记常量 ══════
/// 标记魔数：Trivium Flush Marker
const FLUSH_MARKER_MAGIC: &[u8; 4] = b"TFMK";
/// 标记版本号（u8，单字节）
const FLUSH_MARKER_VERSION: u8 = 1;
/// 标记总长度：magic(4) + version(1) + generation(8) + tdb_size(8) + vec_size(8) = 29
const FLUSH_MARKER_SIZE: usize = 29;

/// flush_ok 提交标记：记录 .tdb 和 .vec 的文件大小及单调递增的 generation 号
///
/// 格式：magic(u32) + version(u8) + generation(u64) + tdb_size(u64) + vec_size(u64)
struct FlushMarker {
    generation: u64,
    tdb_size: u64,
    vec_size: u64,
}

/// 编码 flush marker 为固定字节数组
fn encode_flush_marker(marker: &FlushMarker) -> [u8; FLUSH_MARKER_SIZE] {
    let mut bytes = [0u8; FLUSH_MARKER_SIZE];
    bytes[0..4].copy_from_slice(FLUSH_MARKER_MAGIC);
    bytes[4] = FLUSH_MARKER_VERSION;
    bytes[5..13].copy_from_slice(&marker.generation.to_le_bytes());
    bytes[13..21].copy_from_slice(&marker.tdb_size.to_le_bytes());
    bytes[21..29].copy_from_slice(&marker.vec_size.to_le_bytes());
    bytes
}

/// 解码 flush marker，校验 magic 和 version，不匹配时返回错误
fn decode_flush_marker(bytes: &[u8]) -> Result<FlushMarker> {
    if bytes.len() != FLUSH_MARKER_SIZE {
        return Err(TriviumError::CorruptedFile(format!(
            "flush marker 长度无效：期望 {} 字节，实际 {} 字节",
            FLUSH_MARKER_SIZE,
            bytes.len()
        )));
    }
    if &bytes[0..4] != FLUSH_MARKER_MAGIC {
        return Err(TriviumError::CorruptedFile("flush marker 魔数无效".into()));
    }
    let version = bytes[4];
    if version != FLUSH_MARKER_VERSION {
        return Err(TriviumError::CorruptedFile(format!(
            "flush marker 版本无效：期望 {}，实际 {}",
            FLUSH_MARKER_VERSION, version
        )));
    }
    Ok(FlushMarker {
        generation: read_u64_le(bytes, 5, "flush marker generation")?,
        tdb_size: read_u64_le(bytes, 13, "flush marker tdb_size")?,
        vec_size: read_u64_le(bytes, 21, "flush marker vec_size")?,
    })
}

/// 读取现有 flush_ok 标记的 generation 号（用于单调递增）
/// 如果标记不存在或无效，返回 None
fn read_marker_generation(marker_path: &str) -> Option<u64> {
    let bytes = std::fs::read(marker_path).ok()?;
    decode_flush_marker(&bytes).ok().map(|m| m.generation)
}

/// 校验 flush_ok 标记是否有效
/// 检查 magic、version，以及 tdb_size/vec_size 是否与实际文件大小匹配
fn validate_flush_marker(marker_path: &str, tdb_path: &str, vec_path: &str) -> bool {
    let marker_bytes = match std::fs::read(marker_path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let marker = match decode_flush_marker(&marker_bytes) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let actual_tdb = std::fs::metadata(tdb_path).map(|m| m.len()).unwrap_or(0);
    let actual_vec = std::fs::metadata(vec_path).map(|m| m.len()).unwrap_or(0);
    marker.tdb_size == actual_tdb && marker.vec_size == actual_vec
}

/// 从字节切片中安全读取小端序整数（军工级：禁止裸 unwrap）
///
/// GJB-5000B 条款 6.3.2 要求：所有反序列化路径必须对畸形输入返回明确错误，
/// 不得触发 panic 导致进程终止。
#[inline]
fn read_u16_le(bytes: &[u8], offset: usize, field: &str) -> Result<u16> {
    let end = offset.checked_add(2).ok_or_else(|| {
        TriviumError::CorruptedFile(format!("{} offset overflow at {}", field, offset))
    })?;
    bytes
        .get(offset..end)
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
    let end = offset.checked_add(4).ok_or_else(|| {
        TriviumError::CorruptedFile(format!("{} offset overflow at {}", field, offset))
    })?;
    bytes
        .get(offset..end)
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

fn file_crc32(path: impl AsRef<Path>) -> std::io::Result<u32> {
    use std::io::Read;
    let mut reader = std::io::BufReader::new(File::open(path)?);
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize())
}

fn quiver_meta_path_from_db(db_path: &str) -> String {
    format!("{}.quiver.meta", db_path)
}

fn write_quiver_meta<T: VectorType>(memtable: &MemTable<T>, db_path: &str) -> std::io::Result<()> {
    let path = quiver_meta_path_from_db(db_path);
    let tmp = format!("{path}.tmp");
    let tdb_size = std::fs::metadata(db_path)?.len();
    let vec_size = std::fs::metadata(vec_path_from_db(db_path))
        .map(|meta| meta.len())
        .unwrap_or(0);
    let quiver_path = quiver_path_from_db(db_path);
    let quiver_size = std::fs::metadata(&quiver_path)?.len();
    let quiver_crc = file_crc32(&quiver_path)?;
    let mut bytes = Vec::with_capacity(48);
    bytes.extend_from_slice(b"QMET");
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&tdb_size.to_le_bytes());
    bytes.extend_from_slice(&vec_size.to_le_bytes());
    bytes.extend_from_slice(&quiver_size.to_le_bytes());
    bytes.extend_from_slice(&(memtable.node_count() as u64).to_le_bytes());
    bytes.extend_from_slice(&(memtable.dim() as u32).to_le_bytes());
    bytes.extend_from_slice(&quiver_crc.to_le_bytes());
    let mut file = File::create(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    robust_rename_and_sync(Path::new(&tmp), Path::new(&path))
}

fn validate_quiver_meta<T: VectorType>(memtable: &MemTable<T>, db_path: &str) -> bool {
    let Ok(bytes) = std::fs::read(quiver_meta_path_from_db(db_path)) else {
        return false;
    };
    if bytes.len() != 48 || &bytes[0..4] != b"QMET" {
        return false;
    }
    let read_u64 =
        |offset: usize| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap_or([0; 8]));
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap_or([0; 4]));
    let dim = u32::from_le_bytes(bytes[40..44].try_into().unwrap_or([0; 4])) as usize;
    let quiver_crc = u32::from_le_bytes(bytes[44..48].try_into().unwrap_or([0; 4]));
    let actual_quiver_crc = file_crc32(quiver_path_from_db(db_path)).unwrap_or(u32::MAX);
    version == 1
        && read_u64(8)
            == std::fs::metadata(db_path)
                .map(|m| m.len())
                .unwrap_or(u64::MAX)
        && read_u64(16)
            == std::fs::metadata(vec_path_from_db(db_path))
                .map(|m| m.len())
                .unwrap_or(0)
        && read_u64(24)
            == std::fs::metadata(quiver_path_from_db(db_path))
                .map(|m| m.len())
                .unwrap_or(u64::MAX)
        && read_u64(32) == memtable.node_count() as u64
        && dim == memtable.dim()
        && quiver_crc == actual_quiver_crc
}

/// TextIndex 索引文件路径（.tdb → .tdb.text）
fn text_index_path_from_db(db_path: &str) -> String {
    format!("{}.text", db_path)
}

fn text_index_meta_path_from_db(db_path: &str) -> String {
    format!("{}.text.meta", db_path)
}

fn write_text_index_meta(db_path: &str) -> std::io::Result<()> {
    let text_path = text_index_path_from_db(db_path);
    let text_size = std::fs::metadata(&text_path)?.len();
    let mut bytes = Vec::with_capacity(28);
    bytes.extend_from_slice(b"TMET");
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&std::fs::metadata(db_path)?.len().to_le_bytes());
    bytes.extend_from_slice(&text_size.to_le_bytes());
    bytes.extend_from_slice(&file_crc32(&text_path)?.to_le_bytes());
    let meta_path = text_index_meta_path_from_db(db_path);
    let tmp = format!("{meta_path}.tmp");
    let mut file = File::create(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    robust_rename_and_sync(Path::new(&tmp), Path::new(&meta_path))
}

fn validate_text_index_meta(db_path: &str) -> bool {
    let Ok(meta) = std::fs::read(text_index_meta_path_from_db(db_path)) else {
        return false;
    };
    let text_path = text_index_path_from_db(db_path);
    let Ok(text_size_actual) = std::fs::metadata(&text_path).map(|meta| meta.len()) else {
        return false;
    };
    if meta.len() != 28 || &meta[0..4] != b"TMET" {
        return false;
    }
    let version = u32::from_le_bytes(meta[4..8].try_into().unwrap_or([0; 4]));
    let tdb_size = u64::from_le_bytes(meta[8..16].try_into().unwrap_or([0; 8]));
    let text_size = u64::from_le_bytes(meta[16..24].try_into().unwrap_or([0; 8]));
    let text_crc = u32::from_le_bytes(meta[24..28].try_into().unwrap_or([0; 4]));
    version == 1
        && tdb_size
            == std::fs::metadata(db_path)
                .map(|m| m.len())
                .unwrap_or(u64::MAX)
        && text_size == text_size_actual
        && text_crc == file_crc32(&text_path).unwrap_or(u32::MAX)
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
            tracing::warn!(
                "QuIVer 索引持久化失败（不影响主数据）(QuIVer persist failed, main data unaffected): {}",
                e
            );
        } else if let Err(error) = write_quiver_meta(memtable, path) {
            tracing::warn!("QuIVer 元数据持久化失败: {}", error);
        }
    } else {
        // QuIVer 不存在时清理残留文件
        for stale in [&quiver_path, &quiver_meta_path_from_db(path)] {
            let qp = std::path::Path::new(stale);
            if qp.exists() {
                std::fs::remove_file(qp).ok();
            }
        }
    }

    let text_path = text_index_path_from_db(path);
    if let Err(error) = memtable
        .text_index()
        .save_to_file(std::path::Path::new(&text_path))
    {
        tracing::warn!(
            "TextIndex 持久化失败（不影响主数据）(TextIndex persist failed): {}",
            error
        );
    } else if std::path::Path::new(&text_path).exists() {
        if let Err(error) = write_text_index_meta(path) {
            tracing::warn!("TextIndex 元数据持久化失败: {}", error);
        }
    } else {
        std::fs::remove_file(text_index_meta_path_from_db(path)).ok();
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

    // 读取上一次的 generation 号，单调递增；不存在或无效时从 1 开始
    let generation = read_marker_generation(&marker_path)
        .map(|g| g.wrapping_add(1))
        .unwrap_or(1);

    let marker = FlushMarker {
        generation,
        tdb_size,
        vec_size,
    };
    let marker_bytes = encode_flush_marker(&marker);
    let marker_tmp = format!("{}.tmp", marker_path);
    {
        let mut f = File::create(&marker_tmp)?;
        f.write_all(&marker_bytes)?;
        f.sync_all()?;
    }
    robust_rename_and_sync(Path::new(&marker_tmp), Path::new(&marker_path))?;

    Ok(())
}

/// Rom 模式保存：把向量合并，写单文件，抛弃 .vec
fn save_rom<T: VectorType>(memtable: &mut MemTable<T>, path: &str) -> Result<()> {
    // 1. Rom 单文件持久化必须物化完整 flat，但不得隐式构建 ANN。
    memtable.prepare_persistence_cache(true);
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
    // 持久化只准备磁盘格式需要的数据，禁止隐式触发 ANN 构建。
    // Mmap 分离模式下向量已在 .vec 文件，无需把整库复制入堆。
    memtable.prepare_persistence_cache(!is_mmap_mode);

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
            if let Some(payload_raw) = memtable.get_payload_raw(nid) {
                payload_size += 8 + 4 + payload_raw.len() as u64;
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
            && let Some(payload_raw) = memtable.get_payload_raw(nid)
        {
            w.write_all(&nid.to_le_bytes())?;
            w.write_all(&(payload_raw.len() as u32).to_le_bytes())?;
            w.write_all(payload_raw)?;
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

    robust_rename_and_sync(Path::new(&tmp_path), Path::new(path))?;

    tracing::info!(
        "持久化完成 (Flush completed): {} 个槽位(含删除), {} 个向量, {} 个 BQ 签名, 模式 (Mode): {}",
        node_count,
        vec_count,
        bq_count,
        if is_mmap_mode { "Mmap" } else { "Rom" }
    );

    Ok(())
}

pub fn load<T: VectorType>(
    path: &str,
    _mode: StorageMode,
    load_text_sidecar: bool,
) -> Result<MemTable<T>> {
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
        let count_end = bq_offset.checked_add(8).ok_or_else(|| {
            TriviumError::CorruptedFile("BQ 块计数偏移溢出 (BQ count offset overflow)".into())
        })?;
        if count_end > file_len {
            return Err(TriviumError::CorruptedFile(
                "BQ 块缺少计数字段 (BQ count field truncated)".into(),
            ));
        }
        let bq_count_u64 = read_u64_le(bytes, bq_offset, "BQ count")?;
        let bq_count = usize::try_from(bq_count_u64).map_err(|_| {
            TriviumError::CorruptedFile(format!(
                "BQ 签名数量超出平台寻址范围 (BQ count out of range): {bq_count_u64}"
            ))
        })?;
        let sig_size = std::mem::size_of::<BqSignature>();
        let block_bytes = bq_count.checked_mul(sig_size).ok_or_else(|| {
            TriviumError::CorruptedFile(format!(
                "BQ 块长度溢出 (BQ block size overflow): {bq_count} 个签名 × {sig_size} 字节"
            ))
        })?;
        let expected_bq_end = count_end.checked_add(block_bytes).ok_or_else(|| {
            TriviumError::CorruptedFile("BQ 块结束偏移溢出 (BQ end offset overflow)".into())
        })?;
        if expected_bq_end > file_len {
            return Err(TriviumError::CorruptedFile(format!(
                "BQ 块被截断 (BQ block truncated): 期望 {} 字节 (offset {} + 8 + {} × {})，实际文件大小 {} 字节",
                expected_bq_end, bq_offset, bq_count, sig_size, file_len
            )));
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
        // 检查 .flush_ok 标记是否存在且 magic/version/大小均吻合，防止撕裂写入。
        // marker 无效时直接走 metadata-only 路径，依赖 WAL 恢复数据，
        // 不再尝试加载可能不一致的 .vec 文件。
        let marker_path = flush_ok_path_from_db(path);
        let flush_ok_valid = validate_flush_marker(&marker_path, path, &vec_file_path);

        if flush_ok_valid {
            // marker 有效：安全加载 .vec
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
            if load_text_sidecar {
                load_text_index(&mut mt, path);
            }
            Ok(mt)
        } else {
            // marker 缺失或不匹配：直接安全降级，仅加载 .tdb 元数据，依赖 WAL 回放
            tracing::warn!(
                "检测到 .tdb/.vec 跨文件撕裂 (Cross-file tear detected)（.flush_ok 标记缺失或不匹配），\
                 直接进入安全降级恢复 (metadata-only)，依赖 WAL 回放数据"
            );
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
            if load_text_sidecar {
                load_text_index(&mut mt, path);
            }
            Ok(mt)
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
        if load_text_sidecar {
            load_text_index(&mut mt, path);
        }
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
    // register_node/register_tombstone 只注册元数据映射，不向 VecPool 推入向量。
    // 安全降级时 .vec 不被信任，为保持 indices_to_ids 与 vec_pool 索引严格对齐，
    // 为每个槽位（含 tombstone 占位）推入零向量，后续由 WAL 回放覆盖为真实向量。
    let zero = vec![T::zero(); dim];
    let slot_count = memtable.internal_slot_count();
    for _ in 0..slot_count {
        memtable.vec_pool_mut().push(&zero);
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
    let vector_elements = node_count.checked_mul(dim).ok_or_else(|| {
        TriviumError::CorruptedFile("向量元素数量溢出 (Vector element count overflow)".into())
    })?;
    let expected_vec_size = vector_elements
        .checked_mul(vector_bytes_per_elem)
        .ok_or_else(|| {
            TriviumError::CorruptedFile("向量块长度溢出 (Vector block size overflow)".into())
        })?;
    let vector_end = vector_offset
        .checked_add(expected_vec_size)
        .ok_or_else(|| {
            TriviumError::CorruptedFile("向量块结束偏移溢出 (Vector block end overflow)".into())
        })?;

    if vector_end > tdb_mmap.len() {
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

    let vec_block = &bytes[vector_offset..vector_end];
    let is_aligned = (vec_block.as_ptr() as usize).is_multiple_of(std::mem::align_of::<T>());

    // 因为 load_payloads 已经按内部索引位置推了占位符（包含 Tombstone），
    // 接下来我们只需要把所有的 vector_block 推入 VecPool！
    if is_aligned {
        let t_slice =
            unsafe { std::slice::from_raw_parts(vec_block.as_ptr() as *const T, vector_elements) };
        memtable.vec_pool_mut().push(t_slice);
    } else {
        // 不对齐
        let mut v = Vec::new();
        v.try_reserve_exact(vector_elements).map_err(|_| {
            TriviumError::CorruptedFile(
                "向量块声明的元素数量无法安全分配 (Vector block allocation rejected)".into(),
            )
        })?;
        for i in 0..vector_elements {
            let off = i.checked_mul(vector_bytes_per_elem).ok_or_else(|| {
                TriviumError::CorruptedFile(
                    "向量元素偏移溢出 (Vector element offset overflow)".into(),
                )
            })?;
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
            return Err(TriviumError::CorruptedFile(
                "Payload 块溢出 (Payload block overflow)".into(),
            ));
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
            return Err(TriviumError::CorruptedFile(
                "JSON 数据溢出 (JSON data overflow)".into(),
            ));
        }
        let payload_raw = &bytes[cursor..cursor + json_len];
        cursor += json_len;

        memtable.register_node_raw(nid, payload_raw)?;
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
        let label = String::from_utf8(bytes[cursor..cursor + label_len].to_vec()).map_err(|e| {
            TriviumError::CorruptedFile(format!("标签解码错误 (Label decode error): {}", e))
        })?;
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
    let Some(count_end) = bq_offset.checked_add(8) else {
        tracing::warn!("BQ 块计数偏移溢出，跳过恢复");
        return;
    };
    if bq_offset == 0 || count_end > file_len {
        return;
    }

    let Ok(bq_count_u64) = read_u64_le(bytes, bq_offset, "BQ count") else {
        return;
    };
    let Ok(bq_count) = usize::try_from(bq_count_u64) else {
        tracing::warn!("BQ 签名数量超出平台寻址范围，跳过恢复");
        return;
    };
    if bq_count == 0 {
        return;
    }

    let sig_size = std::mem::size_of::<BqSignature>();
    let Some(block_bytes) = bq_count.checked_mul(sig_size) else {
        tracing::warn!("BQ 块长度溢出，跳过恢复");
        return;
    };
    let data_start = count_end;
    let Some(data_end) = data_start.checked_add(block_bytes) else {
        tracing::warn!("BQ 块结束偏移溢出，跳过恢复");
        return;
    };

    if data_end > file_len {
        tracing::warn!(
            "BQ Block 数据不完整 (BQ Block data incomplete)（需要 {} 字节，文件仅剩 {} 字节），跳过恢复",
            block_bytes,
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
    tracing::info!(
        "从 .tdb 恢复了 {} 个 BQ 签名 (Restored {} BQ signatures from .tdb)（零拷贝加载）",
        bq_count,
        bq_count
    );
}

fn load_text_index<T: VectorType>(memtable: &mut MemTable<T>, db_path: &str) {
    let text_path = text_index_path_from_db(db_path);
    let path = std::path::Path::new(&text_path);
    if !path.exists() {
        return;
    }
    if !validate_text_index_meta(db_path) {
        tracing::warn!("TextIndex sidecar 元数据缺失或不匹配，已拒绝并清理");
        std::fs::remove_file(path).ok();
        std::fs::remove_file(text_index_meta_path_from_db(db_path)).ok();
        return;
    }
    match crate::index::text::TextIndex::load_from_file(path) {
        Ok(index) => memtable.set_text_index(index),
        Err(error) => {
            tracing::warn!(
                "TextIndex 加载失败，已忽略 sidecar (TextIndex load failed): {}",
                error
            );
        }
    }
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

    if !validate_quiver_meta(memtable, db_path) {
        tracing::warn!("QuIVer sidecar 元数据缺失或不匹配，已拒绝并清理，将在查询时重建");
        std::fs::remove_file(qp).ok();
        std::fs::remove_file(quiver_meta_path_from_db(db_path)).ok();
        return;
    }

    match QuIVer::load_from_file(qp) {
        Ok(quiver) => {
            memtable.set_quiver_index(quiver);
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
