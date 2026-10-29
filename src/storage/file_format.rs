//! `.tdb/.vec/.pld.<generation>` 与 `.flush_ok` 文件组一致性协议。
//!
//! Rom 主文件内嵌向量与 Payload；Mmap v9 的向量和原始 Payload 使用分离文件，主文件
//! 保存边、slot 和必要元数据。读取校验魔数、版本、几何与 CRC；发布日志保护文件组替换，
//! `.flush_ok` v3 将主文件、向量与 Payload 绑定到同一 generation。

use crate::VectorType;
use crate::database::StorageMode;
use crate::error::{Result, TriviumError};
use crate::index::bq::BqSignature;
use crate::node::{Edge, NodeId};
use crate::storage::fs::robust_rename_and_sync;
use crate::storage::memtable::MemTable;
use crate::storage::payload_store::PayloadStore;
use crate::storage::vec_pool::VecPool;
use memmap2::Mmap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

mod publication;
pub(crate) use publication::recover as recover_publication;

/// 从已关闭的独占暂存库发布全新文件组；目录项以硬链接原子创建，保留目标互斥锁。
pub(crate) fn publish_new_database<T: VectorType>(source: &str, target: &str) -> Result<()> {
    validate_checkpoint_type::<T>(source, &vec_path_from_db(source), source)?;
    let generation = read_marker_generation(&flush_ok_path_from_db(source)).ok_or_else(|| {
        TriviumError::CorruptedFile(
            "迁移暂存库缺少提交标记 (Migration staging database has no commit marker)".into(),
        )
    })?;
    if generation != 1 {
        return Err(TriviumError::InvalidInput(
            "迁移暂存库必须从第一代发布 (Migration staging database must publish generation one)"
                .into(),
        ));
    }
    let mut created = Vec::new();
    let mut committed = false;
    let result = (|| -> Result<()> {
        publication::begin(target, generation, StorageMode::Mmap, false)?;
        let suffixes = crate::storage::snapshot::generation_suffixes(source);
        for suffix in suffixes
            .iter()
            .filter(|suffix| suffix.as_str() != ".flush_ok")
            .map(String::as_str)
            .chain(std::iter::once(".wal"))
        {
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::io_result(crate::test_hooks::IoPoint::MigrationPublishFile)?;
            let destination = format!("{target}{suffix}");
            // 源文件已经 fsync；同盘链接原子创建完整目录项，同时拒绝覆盖任何既有文件。
            std::fs::hard_link(format!("{source}{suffix}"), &destination)?;
            created.push(destination);
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::MigrationFilePublished);
            if created.len() == 1 {
                #[cfg(feature = "test-hooks")]
                crate::test_hooks::io_result(crate::test_hooks::IoPoint::MigrationAfterFirstFile)?;
            }
        }
        crate::storage::fs::sync_parent_directory(Path::new(target))?;
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::io_result(crate::test_hooks::IoPoint::MigrationPublishMarker)?;
        std::fs::hard_link(flush_ok_path_from_db(source), flush_ok_path_from_db(target))?;
        committed = true;
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::io_result(crate::test_hooks::IoPoint::MigrationAfterCommit)?;
        crate::storage::fs::sync_parent_directory(Path::new(target))?;
        publication::cleanup(target)?;
        Ok(())
    })();
    if result.is_err() && !committed {
        // 全新目标的发布只创建目录项；仅移除本次成功创建的链接，保留发生碰撞的外来文件。
        for file in created
            .into_iter()
            .chain(std::iter::once(format!("{target}.publish.tmp")))
        {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if publication::pending(target) {
            publication::cleanup(target)?;
        }
        crate::storage::fs::sync_parent_directory(Path::new(target))?;
    }
    result
}

// ══════ 文件头常量 ══════
const MAGIC: &[u8; 4] = b"TVDB";
pub const CURRENT_VERSION: u16 = 10;
pub const MINIMUM_SUPPORTED_VERSION: u16 = 5;
const LEGACY_HEADER_SIZE: u64 = 58;
const HEADER_SIZE: u64 = 74;
const BQ_BLOCK_MAGIC: &[u8; 4] = b"TBQF";
const BQ_BLOCK_VERSION: u16 = 1;
const BQ_BLOCK_HEADER_SIZE: usize = 16;
const UNIQUE_BLOCK_MAGIC: &[u8; 4] = b"TUQC";
const UNIQUE_BLOCK_VERSION: u16 = 1;

fn vector_dtype<T: VectorType>() -> Result<(u16, &'static str)> {
    use std::any::TypeId;
    let id = TypeId::of::<T>();
    if id == TypeId::of::<f32>() {
        Ok((1, "f32"))
    } else if id == TypeId::of::<half::f16>() {
        Ok((2, "f16"))
    } else if id == TypeId::of::<u64>() {
        Ok((3, "u64"))
    } else {
        Err(TriviumError::InvalidInput(
            "持久化向量类型不受支持 (Unsupported persistent vector dtype)".into(),
        ))
    }
}

/// 新库在接收第一条 WAL 前持久化类型身份；空的 Rom 布局适用于两种存储模式。
pub(crate) fn initialize<T: VectorType>(path: &str, dim: usize) -> Result<()> {
    if crate::storage::wal::Wal::needs_recovery(path) {
        return Err(TriviumError::CorruptedFile("缺少带类型的基础检查点，请用原版本恢复并导出 (Typed checkpoint missing; recover and export with the original version)".into()));
    }
    save_tdb(&mut MemTable::<T>::new(dim), path, 0, false)
}

pub(crate) fn upgrade_empty_checkpoint<T: VectorType>(
    mt: &mut MemTable<T>,
    path: &str,
    mode: StorageMode,
) -> Result<()> {
    use std::io::Read;
    if mt.internal_slot_count() != 0 {
        return Ok(());
    }
    let mut prefix = [0u8; 6];
    File::open(path)?.read_exact(&mut prefix)?;
    if u16::from_le_bytes([prefix[4], prefix[5]]) < 10 {
        // 空库仍可能持有索引定义，按文件组协议同步发布绑定主文件的 sidecar。
        save(mt, path, mode)?;
        mt.mark_checkpoint_clean();
    }
    Ok(())
}

fn validate_vector_identity<T: VectorType>(
    bytes: &[u8],
    version: u16,
    dim: usize,
    count: usize,
    vector_offset: usize,
    edge_offset: usize,
    path: &str,
    vec_path: &str,
) -> Result<()> {
    let (requested_tag, requested) = vector_dtype::<T>()?;
    let elements = count.checked_mul(dim).ok_or_else(|| {
        TriviumError::CorruptedFile("向量几何溢出 (Vector geometry overflow)".into())
    })?;
    let vector_bytes = if vector_offset == 0 {
        match std::fs::metadata(vec_path) {
            Ok(metadata) => usize::try_from(metadata.len()).map_err(|_| {
                TriviumError::CorruptedFile("向量长度溢出 (Vector length overflow)".into())
            })?,
            Err(e) if count == 0 && e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(TriviumError::CorruptedFile(
                    "检查点向量文件缺失 (Checkpoint vector file is missing)".into(),
                ));
            }
            Err(e) => return Err(e.into()),
        }
    } else {
        edge_offset.checked_sub(vector_offset).ok_or_else(|| {
            TriviumError::CorruptedFile("向量块边界倒置 (Reversed vector block bounds)".into())
        })?
    };
    let stored_tag = if version >= 10 {
        if bytes.len() < HEADER_SIZE as usize
            || &bytes[58..62] != b"VTYP"
            || read_u32_le(bytes, 66, "dtype reserved")? != 0
            || crc32fast::hash(&bytes[..70]) != read_u32_le(bytes, 70, "header CRC")?
        {
            return Err(TriviumError::CorruptedFile(
                "类型文件头校验失败 (Typed header validation failed)".into(),
            ));
        }
        read_u16_le(bytes, 62, "dtype")?
    } else if elements > 0 {
        // 历史格式按完整向量块几何推导；三种受支持类型的元素宽度互不相同。
        match vector_bytes
            .checked_div(elements)
            .filter(|width| elements.checked_mul(*width) == Some(vector_bytes))
        {
            Some(4) => 1,
            Some(2) => 2,
            Some(8) => 3,
            _ => {
                return Err(TriviumError::CorruptedFile(
                    "历史向量块几何无效 (Invalid legacy vector geometry)".into(),
                ));
            }
        }
    } else {
        if crate::storage::wal::Wal::needs_recovery(path) {
            return Err(TriviumError::CorruptedFile("历史空检查点无法证明 WAL 类型，请用原版本恢复 (Legacy empty checkpoint cannot establish WAL dtype; recover with the original version)".into()));
        }
        requested_tag
    };
    let (stored, width) = match stored_tag {
        1 => ("f32", 4),
        2 => ("f16", 2),
        3 => ("u64", 8),
        _ => {
            return Err(TriviumError::CorruptedFile(
                "未知向量类型 (Unknown vector dtype)".into(),
            ));
        }
    };
    if version >= 10 && usize::from(read_u16_le(bytes, 64, "element width")?) != width {
        return Err(TriviumError::CorruptedFile(
            "向量类型与元素宽度不符 (Vector dtype and element width disagree)".into(),
        ));
    }
    if stored_tag != requested_tag {
        return Err(TriviumError::DtypeMismatch { stored, requested });
    }
    if elements.checked_mul(width) != Some(vector_bytes) {
        return Err(TriviumError::CorruptedFile(
            "向量块精确长度不符 (Vector block exact length mismatch)".into(),
        ));
    }
    Ok(())
}

/// 发布恢复写入前核对可信基础文件的类型；备份向量使用独立路径。
fn validate_checkpoint_type<T: VectorType>(
    main: &str,
    vectors: &str,
    wal_base: &str,
) -> Result<()> {
    use std::io::Read;
    let mut bytes = [0u8; HEADER_SIZE as usize];
    let mut file = File::open(main)?;
    file.read_exact(&mut bytes[..LEGACY_HEADER_SIZE as usize])?;
    let version = read_u16_le(&bytes, 4, "version")?;
    if version >= 10 {
        file.read_exact(&mut bytes[LEGACY_HEADER_SIZE as usize..])?;
    }
    validate_vector_identity::<T>(
        &bytes,
        version,
        read_u32_le(&bytes, 6, "dim")? as usize,
        usize::try_from(read_u64_le(&bytes, 18, "slots")?).map_err(|_| {
            TriviumError::CorruptedFile("槽位数量溢出 (Slot count overflow)".into())
        })?,
        usize::try_from(read_u64_le(&bytes, 34, "vectors")?).map_err(|_| {
            TriviumError::CorruptedFile("向量偏移溢出 (Vector offset overflow)".into())
        })?,
        usize::try_from(read_u64_le(&bytes, 42, "edges")?)
            .map_err(|_| TriviumError::CorruptedFile("边偏移溢出 (Edge offset overflow)".into()))?,
        wal_base,
        vectors,
    )
}

// ══════ flush_ok 提交标记常量 ══════
/// 标记魔数：Trivium Flush Marker
const FLUSH_MARKER_MAGIC: &[u8; 4] = b"TFMK";
/// 当前标记版本记录 `.tdb/.vec/.pld` 的长度、整文件 CRC32 和 marker 自身 CRC32。
const FLUSH_MARKER_VERSION: u8 = 3;
const FLUSH_MARKER_V1_SIZE: usize = 29;
const FLUSH_MARKER_V2_SIZE: usize = 41;
/// magic(4) + version(1) + generation(8) + sizes(24) + file CRCs(12) + marker CRC(4)
const FLUSH_MARKER_SIZE: usize = 53;

/// flush_ok v3 提交标记：generation、三份事实文件的长度与 CRC，以及标记自身的 CRC。
/// 历史 v1/v2 标记按各自字段布局解码。
struct FlushMarker {
    generation: u64,
    tdb_size: u64,
    vec_size: u64,
    payload_size: Option<u64>,
    tdb_crc32: Option<u32>,
    vec_crc32: Option<u32>,
    payload_crc32: Option<u32>,
}

/// 编码 flush marker 为固定字节数组
fn encode_flush_marker(marker: &FlushMarker) -> [u8; FLUSH_MARKER_SIZE] {
    let mut bytes = [0u8; FLUSH_MARKER_SIZE];
    bytes[0..4].copy_from_slice(FLUSH_MARKER_MAGIC);
    bytes[4] = FLUSH_MARKER_VERSION;
    bytes[5..13].copy_from_slice(&marker.generation.to_le_bytes());
    bytes[13..21].copy_from_slice(&marker.tdb_size.to_le_bytes());
    bytes[21..29].copy_from_slice(&marker.vec_size.to_le_bytes());
    bytes[29..37].copy_from_slice(&marker.payload_size.unwrap_or_default().to_le_bytes());
    bytes[37..41].copy_from_slice(&marker.tdb_crc32.unwrap_or_default().to_le_bytes());
    bytes[41..45].copy_from_slice(&marker.vec_crc32.unwrap_or_default().to_le_bytes());
    bytes[45..49].copy_from_slice(&marker.payload_crc32.unwrap_or_default().to_le_bytes());
    let marker_crc = crc32fast::hash(&bytes[..49]);
    bytes[49..53].copy_from_slice(&marker_crc.to_le_bytes());
    bytes
}

/// 解码 flush marker，校验 magic 和 version，不匹配时返回错误
fn decode_flush_marker(bytes: &[u8]) -> Result<FlushMarker> {
    if !matches!(
        bytes.len(),
        FLUSH_MARKER_V1_SIZE | FLUSH_MARKER_V2_SIZE | FLUSH_MARKER_SIZE
    ) {
        return Err(TriviumError::CorruptedFile(format!(
            "flush marker 长度无效：实际 {} 字节",
            bytes.len()
        )));
    }
    if &bytes[0..4] != FLUSH_MARKER_MAGIC {
        return Err(TriviumError::CorruptedFile("flush marker 魔数无效".into()));
    }
    let version = bytes[4];
    if !matches!(version, 1 | 2 | FLUSH_MARKER_VERSION) {
        return Err(TriviumError::CorruptedFile(format!(
            "flush marker 版本无效：支持 1..={}，实际 {}",
            FLUSH_MARKER_VERSION, version
        )));
    }
    if version == 1 && bytes.len() != FLUSH_MARKER_V1_SIZE {
        return Err(TriviumError::CorruptedFile(
            "flush marker v1 长度无效".into(),
        ));
    }
    if version >= 2 {
        let expected_size = if version == 2 {
            FLUSH_MARKER_V2_SIZE
        } else {
            FLUSH_MARKER_SIZE
        };
        if bytes.len() != expected_size {
            return Err(TriviumError::CorruptedFile(
                "flush marker v2 长度无效".into(),
            ));
        }
        let crc_offset = if version == 2 { 37 } else { 49 };
        let stored_crc = read_u32_le(bytes, crc_offset, "flush marker crc")?;
        if crc32fast::hash(&bytes[..crc_offset]) != stored_crc {
            return Err(TriviumError::CorruptedFile(
                "flush marker CRC 不匹配".into(),
            ));
        }
    }
    Ok(FlushMarker {
        generation: read_u64_le(bytes, 5, "flush marker generation")?,
        tdb_size: read_u64_le(bytes, 13, "flush marker tdb_size")?,
        vec_size: read_u64_le(bytes, 21, "flush marker vec_size")?,
        payload_size: (version >= 3)
            .then(|| read_u64_le(bytes, 29, "flush marker payload_size"))
            .transpose()?,
        tdb_crc32: (version >= 2)
            .then(|| {
                read_u32_le(
                    bytes,
                    if version == 2 { 29 } else { 37 },
                    "flush marker tdb_crc32",
                )
            })
            .transpose()?,
        vec_crc32: (version >= 2)
            .then(|| {
                read_u32_le(
                    bytes,
                    if version == 2 { 33 } else { 41 },
                    "flush marker vec_crc32",
                )
            })
            .transpose()?,
        payload_crc32: (version >= 3)
            .then(|| read_u32_le(bytes, 45, "flush marker payload_crc32"))
            .transpose()?,
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
fn validate_flush_marker(
    marker_path: &str,
    tdb_path: &str,
    vec_path: &str,
    payload_path: &str,
) -> bool {
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
    if marker.tdb_size != actual_tdb || marker.vec_size != actual_vec {
        return false;
    }
    if let Some(payload_size) = marker.payload_size
        && (std::fs::metadata(payload_path).map(|meta| meta.len()).ok() != Some(payload_size)
            || file_crc32(payload_path).ok() != marker.payload_crc32)
    {
        return false;
    }
    match (marker.tdb_crc32, marker.vec_crc32) {
        (Some(tdb_crc), Some(vec_crc)) => {
            file_crc32(tdb_path).ok() == Some(tdb_crc)
                && file_crc32_or_empty(vec_path).ok() == Some(vec_crc)
        }
        (None, None) => true,
        _ => false,
    }
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

fn payload_path_for_generation(db_path: &str, generation: u64) -> String {
    format!("{db_path}.pld.{generation}")
}

/// QuIVer 索引文件路径（.tdb → .tdb.quiver）
fn quiver_path_from_db(db_path: &str) -> String {
    format!("{}.quiver", db_path)
}

pub(crate) fn file_crc32(path: impl AsRef<Path>) -> std::io::Result<u32> {
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

fn file_crc32_or_empty(path: impl AsRef<Path>) -> std::io::Result<u32> {
    match file_crc32(path) {
        Ok(crc) => Ok(crc),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(crc32fast::hash(&[])),
        Err(error) => Err(error),
    }
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
    let mut bytes = Vec::with_capacity(60);
    bytes.extend_from_slice(b"QMET");
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&tdb_size.to_le_bytes());
    bytes.extend_from_slice(&vec_size.to_le_bytes());
    bytes.extend_from_slice(&quiver_size.to_le_bytes());
    bytes.extend_from_slice(&(memtable.node_count() as u64).to_le_bytes());
    bytes.extend_from_slice(&(memtable.dim() as u32).to_le_bytes());
    bytes.extend_from_slice(&quiver_crc.to_le_bytes());
    // 主文件绑定节点身份和 slot 映射；分离向量文件绑定完整向量事实。
    bytes.extend_from_slice(&file_crc32(db_path)?.to_le_bytes());
    bytes.extend_from_slice(&file_crc32_or_empty(vec_path_from_db(db_path))?.to_le_bytes());
    bytes.extend_from_slice(&crc32fast::hash(&bytes).to_le_bytes());
    let mut file = File::create(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    robust_rename_and_sync(Path::new(&tmp), Path::new(&path))
}

fn validate_quiver_meta<T: VectorType>(memtable: &MemTable<T>, db_path: &str) -> bool {
    use std::io::Read;
    let Ok(mut file) = File::open(quiver_meta_path_from_db(db_path)) else {
        return false;
    };
    let mut bytes = [0u8; 60];
    if file.metadata().map(|metadata| metadata.len()).ok() != Some(60)
        || file.read_exact(&mut bytes).is_err()
        || &bytes[0..4] != b"QMET"
    {
        return false;
    }
    let read_u64 =
        |offset: usize| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap_or([0; 8]));
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap_or([0; 4]));
    let dim = u32::from_le_bytes(bytes[40..44].try_into().unwrap_or([0; 4])) as usize;
    let quiver_crc = u32::from_le_bytes(bytes[44..48].try_into().unwrap_or([0; 4]));
    let actual_quiver_crc = file_crc32(quiver_path_from_db(db_path)).unwrap_or(u32::MAX);
    version == 2
        && crc32fast::hash(&bytes[..56])
            == u32::from_le_bytes(bytes[56..60].try_into().unwrap_or([0; 4]))
        && file_crc32(db_path).ok()
            == Some(u32::from_le_bytes(
                bytes[48..52].try_into().unwrap_or([0; 4]),
            ))
        && file_crc32_or_empty(vec_path_from_db(db_path)).ok()
            == Some(u32::from_le_bytes(
                bytes[52..56].try_into().unwrap_or([0; 4]),
            ))
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
    write_text_index_meta_bytes(db_path, &bytes)
}

fn write_text_index_meta_bytes(db_path: &str, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::TextMetadataWrite)?;
    let meta_path = text_index_meta_path_from_db(db_path);
    let tmp = format!("{meta_path}.tmp");
    let mut file = File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    robust_rename_and_sync(Path::new(&tmp), Path::new(&meta_path))
}

pub(crate) fn deferred_text_metadata(db_path: &str) -> Option<[u8; 28]> {
    use std::io::Read;
    let mut file = File::open(text_index_meta_path_from_db(db_path)).ok()?;
    if file.metadata().ok()?.len() != 28 {
        return None;
    }
    let mut meta = [0u8; 28];
    file.read_exact(&mut meta).ok()?;
    (meta[..4] == *b"TMET" && meta[4..8] == 1u32.to_le_bytes()).then_some(meta)
}

pub fn save<T: VectorType>(
    memtable: &mut MemTable<T>,
    path: &str,
    mode: StorageMode,
) -> Result<()> {
    let runtime = memtable.resource_runtime().clone();
    let peak = memtable
        .estimated_memory_bytes()
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(1024 * 1024))
        .ok_or_else(|| TriviumError::CapacityAllocationFailed {
            reason: "保存工作区估算溢出".into(),
        })?;
    let _workspace = if runtime.is_maintenance_task() {
        runtime.reserve_maintenance_workspace(peak)?
    } else {
        runtime.reserve_workspace(peak)?
    };
    memtable.ensure_readable()?;
    let result = save_inner(memtable, path, mode);
    if result.is_err() && memtable.vec_pool().recovery_required() {
        memtable.require_recovery();
    }
    result
}

fn save_inner<T: VectorType>(
    memtable: &mut MemTable<T>,
    path: &str,
    mode: StorageMode,
) -> Result<()> {
    // 在主文件发布前处理延期操作；失败时保留原 sidecar 与所有待合并记录。
    let mut release_text_after_save =
        memtable.text_index_is_deferred() && memtable.text_index_has_pending_changes();
    if release_text_after_save {
        memtable.prepare_text_index()?;
    } else if memtable.text_index_is_deferred()
        && memtable.deferred_text_metadata().is_none()
        && !Path::new(&text_index_meta_path_from_db(path)).exists()
    {
        // 历史文本快照经完整解码后补齐校验元数据；异常原文件继续保留。
        match memtable.prepare_text_index() {
            Ok(()) => release_text_after_save = true,
            Err(error) => tracing::warn!("历史文本索引校验失败，保留原文件：{error}"),
        }
    }
    // 保留原 CRC，不以重新计算校验和的方式“认可”未加载文件中的损坏。
    let preserved_text_meta = memtable.deferred_text_metadata();
    if mode == StorageMode::Rom && publication::pending(path) {
        memtable.vec_pool_mut().detach_mmap();
        publication::recover::<T>(path, true)?;
    }
    let retry = publication::resume::<T>(path, memtable.text_index_is_deferred())?;
    let generation = match retry.or_else(|| read_marker_generation(&flush_ok_path_from_db(path))) {
        Some(generation) if retry.is_some() => generation,
        Some(previous) => previous
            .checked_add(1)
            .ok_or_else(|| TriviumError::InvalidInput("flush generation 已耗尽".into()))?,
        None => 1,
    };
    if retry.is_none() {
        publication::begin(path, generation, mode, memtable.text_index_is_deferred())?;
    }
    let marker = match mode {
        StorageMode::Mmap => Some(save_mmap(memtable, path, generation, retry.is_some())?),
        StorageMode::Rom => {
            save_rom(memtable, path)?;
            None
        }
    };

    // 顺便持久化 QuIVer 索引（如果已构建）
    let quiver_path = quiver_path_from_db(path);
    if let Some(quiver) = memtable.quiver() {
        if let Err(e) = quiver.save_to_file(std::path::Path::new(&quiver_path)) {
            tracing::warn!(
                "QuIVer 索引持久化失败（不影响主数据）(QuIVer persist failed, main data unaffected): {}",
                e
            );
        } else if let Err(error) = write_quiver_meta(memtable, path) {
            tracing::warn!(
                "QuIVer 元数据持久化失败 (QuIVer metadata persistence failed): {}",
                error
            );
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
    let mut updated_text_meta = None;
    if memtable.text_index_is_deferred() {
        if let Some(mut meta) = preserved_text_meta {
            meta[8..16].copy_from_slice(&std::fs::metadata(path)?.len().to_le_bytes());
            write_text_index_meta_bytes(path, &meta)?;
            updated_text_meta = Some(meta);
        }
    } else {
        memtable.text_index().save_to_file(Path::new(&text_path))?;
        if Path::new(&text_path).exists() {
            write_text_index_meta(path)?;
        } else {
            match std::fs::remove_file(text_index_meta_path_from_db(path)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }

    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::PropertySidecarWrite)?;
    let payload_fingerprint = if memtable.property_indexes().is_empty() {
        (0, 0)
    } else {
        memtable.payload_fingerprint()
    };
    crate::index::property::save_sidecar(
        memtable.property_indexes(),
        path,
        memtable.node_count(),
        payload_fingerprint,
    )?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::GraphSidecarWrite)?;
    crate::storage::graph_blocks::save(memtable, path)?;

    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit_for_path(
        crate::test_hooks::ConcurrencyPoint::AfterSidecarsPersisted,
        path,
    );
    if let Some(marker) = marker {
        publish_mmap_marker(path, &marker)?;
    } else {
        publication::commit_rom(path)?;
    }
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit_for_path(
        crate::test_hooks::ConcurrencyPoint::AfterPublicationCommitted,
        path,
    );
    publication::cleanup(path).ok();
    if generation > 1 {
        std::fs::remove_file(payload_path_for_generation(path, generation - 1)).ok();
    }
    if let Some(meta) = updated_text_meta {
        memtable.update_deferred_text_metadata(meta);
    }
    if release_text_after_save {
        memtable.defer_text_index(path);
    }

    Ok(())
}

/// Mmap 模式保存：分离向量到 .vec 文件，.tdb 纯元数据
fn save_mmap<T: VectorType>(
    memtable: &mut MemTable<T>,
    path: &str,
    generation: u64,
    retry: bool,
) -> Result<FlushMarker> {
    if retry {
        // 保留原来的撤销基础；重写从当前 COW 映射与 delta 读取，不在堆中展开全库。
        memtable.vec_pool_mut().require_full_rewrite();
    }
    let vec_file_path = vec_path_from_db(path);
    let vec_count = memtable.vec_pool_mut().flush(Path::new(&vec_file_path))?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::AfterVecPersisted);

    save_tdb(memtable, path, vec_count, true)?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::AfterTdbPersisted);

    // ═══ 跨文件一致性标记（提交点） ═══
    // 旧代已由恢复日志保留；新文件全部持久化后才发布 marker。
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::MarkerMetadata)?;
    let payload_path = payload_path_for_generation(path, generation);
    memtable.save_payload_sidecar(Path::new(&payload_path), generation)?;
    let tdb_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let vec_size = std::fs::metadata(&vec_file_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let marker = FlushMarker {
        generation,
        tdb_size,
        vec_size,
        payload_size: Some(std::fs::metadata(&payload_path)?.len()),
        tdb_crc32: Some(file_crc32(path)?),
        vec_crc32: Some(file_crc32_or_empty(&vec_file_path)?),
        payload_crc32: Some(file_crc32(&payload_path)?),
    };
    Ok(marker)
}

fn publish_mmap_marker(path: &str, marker: &FlushMarker) -> Result<()> {
    let marker_path = flush_ok_path_from_db(path);
    let marker_bytes = encode_flush_marker(marker);
    let marker_tmp = format!("{}.tmp", marker_path);
    {
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::io_result(crate::test_hooks::IoPoint::MarkerCreate)?;
        let mut f = File::create(&marker_tmp)?;
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::io_result(crate::test_hooks::IoPoint::MarkerWrite)?;
        f.write_all(&marker_bytes)?;
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::io_result(crate::test_hooks::IoPoint::MarkerSync)?;
        f.sync_all()?;
    }
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit_for_path(
        crate::test_hooks::ConcurrencyPoint::BeforeFlushMarkerRename,
        path,
    );
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::MarkerRename)?;

    robust_rename_and_sync(Path::new(&marker_tmp), Path::new(&marker_path))?;

    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::AfterFlushMarkerRename);

    Ok(())
}

/// Rom 模式保存：把向量合并，写单文件，抛弃 .vec
fn save_rom<T: VectorType>(memtable: &mut MemTable<T>, path: &str) -> Result<()> {
    // 1. 分段写入向量，不创建旧签名或合并数组，也不隐式构建 ANN。
    let total_vectors = memtable.internal_indices().len();

    // 2. 将数据合并写入单文件
    save_tdb(memtable, path, total_vectors, false)?;

    // 3. 将现有的 mmap (如果有) 剥离到内存 delta 中，避免锁住已或将被删除的 .vec
    memtable.vec_pool_mut().detach_mmap();

    // 4. 清理残留的 .vec 和 .flush_ok
    let vec_file_path = vec_path_from_db(path);
    let marker_path = flush_ok_path_from_db(path);
    for obsolete in [&vec_file_path, &marker_path] {
        match std::fs::remove_file(obsolete) {
            Ok(()) => crate::storage::fs::sync_parent_directory(Path::new(obsolete))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
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
    // 两种模式都直接读取已有向量段，不创建全库临时数组或隐式构建 ANN。

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

    // Mmap v9 主文件只保留固定宽度 slot 目录，raw JSON 全部位于 .pld。
    for &nid in internal_indices {
        if nid != 0 {
            if is_mmap_mode {
                payload_size = payload_size.checked_add(12).ok_or_else(|| {
                    TriviumError::InvalidInput("Payload slot 目录大小溢出".into())
                })?;
            } else if let Some(payload_raw) = memtable.get_payload_raw(nid) {
                payload_size = payload_size
                    .checked_add(12 + payload_raw.len() as u64)
                    .ok_or_else(|| TriviumError::InvalidInput("Payload 块大小溢出".into()))?;
            } else {
                payload_size = payload_size
                    .checked_add(12)
                    .ok_or_else(|| TriviumError::InvalidInput("Payload 块大小溢出".into()))?;
            }
            if let Some(edges) = memtable.get_edges(nid) {
                for edge in edges {
                    all_edges.push((nid, edge));
                }
            }
        } else {
            // 空洞（由于节点被彻底移除，保留内部索引占位）
            payload_size = payload_size
                .checked_add(12)
                .ok_or_else(|| TriviumError::InvalidInput("Payload slot 目录大小溢出".into()))?;
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
        let label_len = u16::try_from(edge.label.len()).map_err(|_| {
            TriviumError::InvalidInput("边标签 UTF-8 长度不能超过 65535 字节".into())
        })?;
        let metadata = edge.metadata.to_string();
        let metadata_len = u32::try_from(metadata.len())
            .map_err(|_| TriviumError::InvalidInput("单条边元数据不能超过 4 GiB".into()))?;
        // src(8) + dst(8) + label_len(2) + label + weight(4) + metadata_len(4) + metadata
        edge_block_size = edge_block_size
            .checked_add(8 + 8 + 2 + u64::from(label_len) + 4 + 4 + u64::from(metadata_len))
            .ok_or_else(|| TriviumError::InvalidInput("边块大小溢出".into()))?;
    }
    let bq_offset = edge_offset + edge_block_size;

    // 固定偏移保留历史字段，附加类型、元素宽度与文件头 CRC。
    let (dtype_tag, _) = vector_dtype::<T>()?;
    let mut header = Vec::with_capacity(HEADER_SIZE as usize);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&CURRENT_VERSION.to_le_bytes());
    header.extend_from_slice(&(dim as u32).to_le_bytes());
    header.extend_from_slice(&memtable.next_id_value().to_le_bytes());
    header.extend_from_slice(&node_count.to_le_bytes());
    header.extend_from_slice(&payload_offset.to_le_bytes());
    header.extend_from_slice(&vector_offset.to_le_bytes());
    header.extend_from_slice(&edge_offset.to_le_bytes());
    header.extend_from_slice(&bq_offset.to_le_bytes());
    header.extend_from_slice(b"VTYP");
    header.extend_from_slice(&dtype_tag.to_le_bytes());
    header.extend_from_slice(&(std::mem::size_of::<T>() as u16).to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    let header_crc = crc32fast::hash(&header);
    header.extend_from_slice(&header_crc.to_le_bytes());
    w.write_all(&header)?;

    // 2. Mmap 模式仅写 slot；Rom 模式继续内嵌 Payload，保持单文件语义。
    for &nid in internal_indices {
        if nid != 0 {
            if is_mmap_mode {
                w.write_all(&nid.to_le_bytes())?;
                w.write_all(&0u32.to_le_bytes())?;
                continue;
            }
            if let Some(payload_raw) = memtable.get_payload_raw(nid) {
                w.write_all(&nid.to_le_bytes())?;
                w.write_all(&(payload_raw.len() as u32).to_le_bytes())?;
                w.write_all(payload_raw)?;
                continue;
            }
        }
        // Tombstone
        w.write_all(&0u64.to_le_bytes())?;
        w.write_all(&0u32.to_le_bytes())?;
    }

    // 3. Vector Block (Rom 用)
    if !is_mmap_mode {
        let (base, delta) = memtable.vec_pool().vector_segments();
        w.write_all(bytemuck::cast_slice(base))?;
        w.write_all(bytemuck::cast_slice(delta))?;
    }

    // 4. Edge Block
    for (src_id, edge) in &all_edges {
        w.write_all(&src_id.to_le_bytes())?;
        w.write_all(&edge.target_id.to_le_bytes())?;
        let label_bytes = edge.label.as_bytes();
        let label_len = u16::try_from(label_bytes.len()).map_err(|_| {
            TriviumError::InvalidInput("边标签 UTF-8 长度不能超过 65535 字节".into())
        })?;
        w.write_all(&label_len.to_le_bytes())?;
        w.write_all(label_bytes)?;
        w.write_all(&edge.weight.to_le_bytes())?;
        let metadata = edge.metadata.to_string();
        let metadata_len = u32::try_from(metadata.len())
            .map_err(|_| TriviumError::InvalidInput("单条边元数据不能超过 4 GiB".into()))?;
        w.write_all(&metadata_len.to_le_bytes())?;
        w.write_all(metadata.as_bytes())?;
    }

    // 5. 旧 1-bit BQ 已不参与查询；合法空缓存块保留既有格式的可读性。
    w.write_all(BQ_BLOCK_MAGIC)?;
    w.write_all(&BQ_BLOCK_VERSION.to_le_bytes())?;
    w.write_all(&(BqSignature::MAX_CHUNKS as u16).to_le_bytes())?;
    let bq_count = 0u64;
    w.write_all(&bq_count.to_le_bytes())?;

    // 6. Unique Constraint 权威定义块。Posting 仍在可重建的 .pidx，约束定义属于主数据。
    let unique_definitions = memtable.unique_index_definitions();
    let mut unique_bytes = Vec::new();
    unique_bytes.extend_from_slice(UNIQUE_BLOCK_MAGIC);
    unique_bytes.extend_from_slice(&UNIQUE_BLOCK_VERSION.to_le_bytes());
    unique_bytes.extend_from_slice(
        &u32::try_from(unique_definitions.len())
            .map_err(|_| TriviumError::InvalidInput("唯一约束数量过多".into()))?
            .to_le_bytes(),
    );
    for fields in unique_definitions {
        unique_bytes.extend_from_slice(
            &u16::try_from(fields.len())
                .map_err(|_| TriviumError::InvalidInput("唯一约束字段数量过多".into()))?
                .to_le_bytes(),
        );
        for field in fields {
            let field = field.as_bytes();
            unique_bytes.extend_from_slice(
                &u32::try_from(field.len())
                    .map_err(|_| TriviumError::InvalidInput("唯一约束字段名过长".into()))?
                    .to_le_bytes(),
            );
            unique_bytes.extend_from_slice(field);
        }
    }
    let unique_crc = crc32fast::hash(&unique_bytes);
    unique_bytes.extend_from_slice(&unique_crc.to_le_bytes());
    w.write_all(&unique_bytes)?;

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
    mode: StorageMode,
    load_text_sidecar: bool,
    repair_sidecars: bool,
    missing_index_policy: crate::database::MissingIndexPolicy,
    mmap_property_postings: bool,
) -> Result<MemTable<T>> {
    load_with_memory_limit(
        path,
        mode,
        load_text_sidecar,
        repair_sidecars,
        missing_index_policy,
        mmap_property_postings,
        0,
    )
}

pub(crate) fn load_with_memory_limit<T: VectorType>(
    path: &str,
    mode: StorageMode,
    load_text_sidecar: bool,
    repair_sidecars: bool,
    missing_index_policy: crate::database::MissingIndexPolicy,
    mmap_property_postings: bool,
    memory_limit: usize,
) -> Result<MemTable<T>> {
    let (mut memtable, version) = load_inner(
        path,
        mode,
        load_text_sidecar,
        repair_sidecars,
        missing_index_policy,
        mmap_property_postings,
        memory_limit,
    )?;
    // 索引恢复及完整性扫描结束后，向量映射恢复为按需驻留。
    memtable.finish_vector_loading()?;
    // 当前格式的完整检查点可直接复用；历史格式保留关闭时迁移的契约。
    let text_metadata_needs_repair = load_text_sidecar
        && !memtable.text_index_is_deferred()
        && Path::new(&text_index_path_from_db(path)).exists()
        && deferred_text_metadata(path).is_none_or(|meta| {
            std::fs::metadata(path).is_ok_and(|main| meta[8..16] != main.len().to_le_bytes())
        });
    if version == CURRENT_VERSION && !text_metadata_needs_repair {
        memtable.mark_checkpoint_clean();
    }
    Ok(memtable)
}

/// 主文件与 WAL 检查点共同成功后，才确认内存中的持久化变更已排空。
pub(crate) fn checkpoint<T: VectorType>(
    memtable: &mut MemTable<T>,
    wal: &mut crate::storage::wal::Wal,
    path: &str,
    mode: StorageMode,
    skip_clean: bool,
) -> Result<()> {
    memtable.ensure_readable()?;
    wal.ensure_writable()?;
    if !skip_clean || memtable.needs_checkpoint() {
        if let Err(error) = save(memtable, path, mode) {
            if memtable.ensure_readable().is_err() {
                // 关闭 writer，拦住失效前已准入、仍在等待 MemTable 锁的写操作。
                wal.release_writer();
            }
            return Err(error);
        }
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::io_result(crate::test_hooks::IoPoint::CheckpointWalClear)?;
        wal.clear()?;
        memtable.mark_checkpoint_clean();
    }
    Ok(())
}

fn load_inner<T: VectorType>(
    path: &str,
    _mode: StorageMode,
    load_text_sidecar: bool,
    repair_sidecars: bool,
    missing_index_policy: crate::database::MissingIndexPolicy,
    mmap_property_postings: bool,
    memory_limit: usize,
) -> Result<(MemTable<T>, u16)> {
    let file = File::open(path).map_err(TriviumError::Io)?;

    let mmap = unsafe { Mmap::map(&file) }.map_err(TriviumError::Io)?;

    if mmap.len() < LEGACY_HEADER_SIZE as usize {
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
    if !(MINIMUM_SUPPORTED_VERSION..=CURRENT_VERSION).contains(&version) {
        return Err(TriviumError::UnsupportedDatabaseVersion {
            found: version,
            minimum_supported: MINIMUM_SUPPORTED_VERSION,
            current: CURRENT_VERSION,
        });
    }
    let dim = read_u32_le(bytes, 6, "header dim")? as usize;
    let next_id = read_u64_le(bytes, 10, "header next_id")?;
    let node_count = read_u64_le(bytes, 18, "header node_count")? as usize;
    let payload_offset = read_u64_le(bytes, 26, "header payload_offset")? as usize;
    let vector_offset = read_u64_le(bytes, 34, "header vector_offset")? as usize;
    let edge_offset = read_u64_le(bytes, 42, "header edge_offset")? as usize;

    let bq_offset = read_u64_le(bytes, 50, "header bq_offset")? as usize;

    if dim == 0
        || payload_offset
            < if version >= 10 {
                HEADER_SIZE as usize
            } else {
                LEGACY_HEADER_SIZE as usize
            }
        || (vector_offset != 0 && vector_offset < payload_offset)
        || edge_offset < payload_offset
    {
        return Err(TriviumError::CorruptedFile(
            "文件块几何无效 (Invalid file block geometry)".into(),
        ));
    }
    validate_vector_identity::<T>(
        bytes,
        version,
        dim,
        node_count,
        vector_offset,
        edge_offset,
        path,
        &vec_path_from_db(path),
    )?;

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
    let bq_layout = parse_bq_layout(bytes, version, bq_offset, file_len)?;

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

    // 分离架构由文件头决定，不能因 .vec 缺失就把元数据误当成 Rom 内嵌向量。
    // 空库允许不存在 .vec，但仍须由 marker 证明其长度与 CRC。
    if vector_offset == 0 {
        // ═══ 跨文件一致性校验 ═══
        // 检查 .flush_ok 标记是否存在且 magic/version/大小均吻合，防止撕裂写入。
        // 未完成发布已在可写打开时通过撤销日志恢复；无日志的事实损坏必须拒绝。
        let marker_path = flush_ok_path_from_db(path);
        let marker = std::fs::read(&marker_path)
            .ok()
            .and_then(|bytes| decode_flush_marker(&bytes).ok());
        let payload_path = marker
            .as_ref()
            .map(|marker| payload_path_for_generation(path, marker.generation))
            .unwrap_or_default();
        let flush_ok_valid =
            validate_flush_marker(&marker_path, path, &vec_file_path, &payload_path);

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
                version,
                &vec_file_path,
                &mmap,
                memory_limit,
            )?;
            let payload_generation = std::fs::read(&marker_path)
                .ok()
                .and_then(|bytes| decode_flush_marker(&bytes).ok())
                .filter(|marker| marker.payload_size.is_some())
                .map(|marker| marker.generation);
            install_payload_sidecar_if_present(&mut mt, path, payload_generation)?;
            // 旧 BQ 块仅校验格式几何，不物化；Payload 就绪后恢复现用索引和约束。
            load_property_indexes(
                &mut mt,
                path,
                repair_sidecars,
                missing_index_policy,
                mmap_property_postings,
            )?;
            load_unique_constraints(&mut mt, bytes, version, bq_layout)?;
            if mmap_property_postings {
                load_mapped_graph(&mut mt, path)?;
            }
            load_quiver_index(&mut mt, path, repair_sidecars, missing_index_policy)?;
            if load_text_sidecar {
                load_text_index(&mut mt, path, repair_sidecars, missing_index_policy)?;
            } else if Path::new(&text_index_path_from_db(path)).exists() {
                mt.defer_text_index(path);
            }
            Ok((mt, version))
        } else {
            Err(TriviumError::CorruptedFile(
                "拒绝不完整的 .tdb/.vec generation：.flush_ok 缺失或不匹配，WAL 不能证明基础向量可完整重建"
                    .into(),
            ))
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
            version,
            &mmap,
            memory_limit,
        )?;
        // 旧 BQ 块已校验，无需恢复未被当前查询使用的数组。
        load_property_indexes(
            &mut mt,
            path,
            repair_sidecars,
            missing_index_policy,
            mmap_property_postings,
        )?;
        load_unique_constraints(&mut mt, bytes, version, bq_layout)?;
        // 尝试加载 QuIVer 索引
        load_quiver_index(&mut mt, path, repair_sidecars, missing_index_policy)?;
        if load_text_sidecar {
            load_text_index(&mut mt, path, repair_sidecars, missing_index_policy)?;
        } else if Path::new(&text_index_path_from_db(path)).exists() {
            mt.defer_text_index(path);
        }
        Ok((mt, version))
    }
}

fn install_payload_sidecar_if_present<T: VectorType>(
    memtable: &mut MemTable<T>,
    db_path: &str,
    expected_generation: Option<u64>,
) -> Result<()> {
    let Some(expected_generation) = expected_generation else {
        // v1/v2 marker 从未声明 Payload sidecar；即使目录中残留同名文件也不得采用。
        return Ok(());
    };
    let path = payload_path_for_generation(db_path, expected_generation);
    if !Path::new(&path).exists() {
        return Err(TriviumError::CorruptedFile(
            "提交标记声明的 Payload sidecar 缺失".into(),
        ));
    }
    let (generation, mmap, records) =
        PayloadStore::open_sidecar_limited(Path::new(&path), |extra| {
            memtable.check_load_allocation(extra)
        })?;
    memtable.check_load_allocation(records.len().saturating_mul(192))?;
    if generation != expected_generation {
        return Err(TriviumError::CorruptedFile(
            "Payload sidecar generation 与提交标记不一致".into(),
        ));
    }
    memtable.install_payload_sidecar(mmap, records)
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
    version: u16,
    vec_file_path: &str,
    _tdb_mmap: &Mmap,
    memory_limit: usize,
) -> Result<MemTable<T>> {
    let vec_pool = VecPool::<T>::open(Path::new(vec_file_path), dim, node_count)?;
    let mut memtable = MemTable::new_with_vec_pool(dim, next_id, vec_pool);
    memtable.begin_loading(memory_limit);
    if version >= 9 {
        load_payload_slots(
            &mut memtable,
            bytes,
            node_count,
            payload_offset,
            edge_offset,
        )?;
    } else {
        load_payloads(
            &mut memtable,
            bytes,
            node_count,
            payload_offset,
            edge_offset,
        )?;
    }
    load_edges(
        &mut memtable,
        bytes,
        edge_offset,
        edge_limit_offset,
        version,
    )?;
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
    version: u16,
    tdb_mmap: &Mmap,
    memory_limit: usize,
) -> Result<MemTable<T>> {
    let mut memtable = MemTable::new_with_next_id(dim, next_id);
    memtable.begin_loading(memory_limit);
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

    if vector_end != edge_offset || vector_end > tdb_mmap.len() {
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

    memtable.check_load_allocation(expected_vec_size.saturating_mul(2))?;
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

    load_edges(
        &mut memtable,
        bytes,
        edge_offset,
        edge_limit_offset,
        version,
    )?;
    Ok(memtable)
}

/// 解析 v9 Mmap 固定宽度 slot 目录。非零 NodeId 的长度字段必须为 0，raw JSON 由 .pld 提供。
fn load_payload_slots<T: VectorType>(
    memtable: &mut MemTable<T>,
    bytes: &[u8],
    node_count: usize,
    offset: usize,
    end_offset: usize,
) -> Result<()> {
    let expected_end = node_count
        .checked_mul(12)
        .and_then(|size| offset.checked_add(size))
        .ok_or_else(|| TriviumError::CorruptedFile("Payload slot 目录长度溢出".into()))?;
    if expected_end != end_offset {
        return Err(TriviumError::CorruptedFile(
            "Payload slot 目录长度不匹配".into(),
        ));
    }
    for index in 0..node_count {
        let cursor = offset + index * 12;
        let id = read_u64_le(bytes, cursor, "payload slot node_id")?;
        let length = read_u32_le(bytes, cursor + 8, "payload slot length")?;
        if length != 0 {
            return Err(TriviumError::CorruptedFile(
                "v9 Mmap Payload slot 长度必须为 0".into(),
            ));
        }
        if id == 0 {
            memtable.register_tombstone()?;
        } else {
            memtable.register_payload_slot(id)?;
        }
    }
    Ok(())
}

/// 解析旧格式或 Rom 模式 Payload Block，处理 Tombstone。
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
    version: u16,
) -> Result<()> {
    let mut cursor = edge_offset;
    while cursor < file_len {
        let (src_id, edge) = read_graph_edge(bytes, &mut cursor, file_len, version, |extra| {
            memtable.check_load_allocation(extra)
        })?;
        memtable.check_load_allocation(memtable.estimate_edge_write_bytes(
            src_id,
            edge.target_id,
            &edge.label,
            &edge.metadata,
        )?)?;
        memtable.upsert_edge(
            src_id,
            edge.target_id,
            edge.label,
            edge.weight,
            edge.metadata,
        )?;
    }
    Ok(())
}

/// 主文件加载和图 sidecar 事实比对共用有界解码器，JSON 分配前执行调用方准入。
pub(crate) fn read_graph_edge(
    bytes: &[u8],
    cursor: &mut usize,
    end: usize,
    version: u16,
    mut admit: impl FnMut(usize) -> Result<()>,
) -> Result<(u64, crate::node::Edge)> {
    let bytes = bytes.get(..end).ok_or_else(|| {
        TriviumError::CorruptedFile("边块越界 (Edge block is out of bounds)".into())
    })?;
    let src = read_u64_le(bytes, *cursor, "edge source")?;
    let dst = read_u64_le(bytes, cursor.saturating_add(8), "edge target")?;
    let label_len = read_u16_le(bytes, cursor.saturating_add(16), "edge label length")? as usize;
    *cursor = cursor
        .checked_add(18)
        .ok_or_else(|| TriviumError::CorruptedFile("边偏移溢出 (Edge offset overflow)".into()))?;
    let label_end = cursor.checked_add(label_len).ok_or_else(|| {
        TriviumError::CorruptedFile("边标签长度溢出 (Edge label length overflow)".into())
    })?;
    let label_bytes = bytes.get(*cursor..label_end).ok_or_else(|| {
        TriviumError::CorruptedFile("边标签被截断 (Edge label is truncated)".into())
    })?;
    admit(label_len.saturating_mul(5).saturating_add(1024))?;
    let label = std::str::from_utf8(label_bytes)
        .map_err(|error| {
            TriviumError::CorruptedFile(format!(
                "边标签不是 UTF-8 (Edge label is not UTF-8): {error}"
            ))
        })?
        .to_owned();
    let weight = read_f32_le(bytes, label_end, "edge weight")?;
    *cursor = label_end.checked_add(4).ok_or_else(|| {
        TriviumError::CorruptedFile("边权重偏移溢出 (Edge weight offset overflow)".into())
    })?;
    if !weight.is_finite() {
        return Err(TriviumError::CorruptedFile(
            "边权重不是有限数 (Edge weight is not finite)".into(),
        ));
    }
    let metadata = if version >= 7 {
        let len = read_u32_le(bytes, *cursor, "edge metadata length")? as usize;
        *cursor = cursor.checked_add(4).ok_or_else(|| {
            TriviumError::CorruptedFile("边元数据偏移溢出 (Edge metadata offset overflow)".into())
        })?;
        let metadata_end = cursor.checked_add(len).ok_or_else(|| {
            TriviumError::CorruptedFile("边元数据长度溢出 (Edge metadata length overflow)".into())
        })?;
        let raw = bytes.get(*cursor..metadata_end).ok_or_else(|| {
            TriviumError::CorruptedFile("边元数据被截断 (Edge metadata is truncated)".into())
        })?;
        admit(
            len.saturating_mul(64)
                .saturating_add(label_len.saturating_mul(5))
                .saturating_add(1024),
        )?;
        let value = serde_json::from_slice(raw).map_err(|error| {
            TriviumError::CorruptedFile(format!("边元数据无效 (Invalid edge metadata): {error}"))
        })?;
        *cursor = metadata_end;
        value
    } else {
        serde_json::Value::Null
    };
    Ok((
        src,
        crate::node::Edge {
            target_id: dst,
            label,
            weight,
            metadata,
        },
    ))
}

#[derive(Clone, Copy)]
struct BqDiskLayout {
    end_offset: usize,
}

fn parse_bq_layout(
    bytes: &[u8],
    version: u16,
    bq_offset: usize,
    file_len: usize,
) -> Result<Option<BqDiskLayout>> {
    if bq_offset > file_len {
        return Err(TriviumError::CorruptedFile(format!(
            "bq_offset ({bq_offset}) 超出文件大小 ({file_len})，文件被截断 (file truncated)"
        )));
    }
    if version == 5 {
        let count_end = bq_offset.checked_add(8).ok_or_else(|| {
            TriviumError::CorruptedFile("BQ 块计数偏移溢出 (BQ count offset overflow)".into())
        })?;
        if count_end > file_len {
            return Err(TriviumError::CorruptedFile(
                "BQ 块缺少计数字段 (BQ count field truncated)".into(),
            ));
        }
        let count_u64 = read_u64_le(bytes, bq_offset, "BQ count")?;
        let count = usize::try_from(count_u64).map_err(|_| {
            TriviumError::CorruptedFile(format!(
                "BQ 签名数量超出平台寻址范围 (BQ count out of range): {count_u64}"
            ))
        })?;
        if count == 0 {
            if count_end != file_len {
                return Err(TriviumError::CorruptedFile(
                    "v5 BQ 空块后存在多余数据 (Trailing data after empty v5 BQ block)".into(),
                ));
            }
            return Ok(None);
        }
        let data_bytes = file_len - count_end;
        let signature_bytes = data_bytes
            .checked_div(count)
            .ok_or_else(|| TriviumError::CorruptedFile("v5 BQ 签名大小无法推导".into()))?;
        if !data_bytes.is_multiple_of(count)
            || signature_bytes == 0
            || !signature_bytes.is_multiple_of(8)
        {
            return Err(TriviumError::CorruptedFile(format!(
                "v5 BQ 块长度与签名数量不匹配 (Invalid v5 BQ block geometry): {data_bytes} / {count}"
            )));
        }
        let chunks_per_signature = signature_bytes / 8;
        if !matches!(chunks_per_signature, 32 | 48) {
            return Err(TriviumError::CorruptedFile(format!(
                "v5 BQ 签名布局未知 (Unknown v5 BQ signature layout): {chunks_per_signature} chunks"
            )));
        }
        return Ok(Some(BqDiskLayout {
            end_offset: file_len,
        }));
    }

    let header_end = bq_offset
        .checked_add(BQ_BLOCK_HEADER_SIZE)
        .ok_or_else(|| TriviumError::CorruptedFile("BQ 块头偏移溢出".into()))?;
    if header_end > file_len {
        return Err(TriviumError::CorruptedFile(
            "BQ 格式头被截断 (BQ format header truncated)".into(),
        ));
    }
    if &bytes[bq_offset..bq_offset + 4] != BQ_BLOCK_MAGIC {
        return Err(TriviumError::CorruptedFile(
            "BQ 格式魔数无效 (Invalid BQ format magic)".into(),
        ));
    }
    let block_version = read_u16_le(bytes, bq_offset + 4, "BQ block version")?;
    if block_version != BQ_BLOCK_VERSION {
        return Err(TriviumError::CorruptedFile(format!(
            "不支持的 BQ 块版本 (Unsupported BQ block version): {block_version}"
        )));
    }
    let chunks_per_signature = read_u16_le(bytes, bq_offset + 6, "BQ chunks")? as usize;
    if chunks_per_signature == 0 || chunks_per_signature > BqSignature::MAX_CHUNKS {
        return Err(TriviumError::CorruptedFile(format!(
            "BQ chunks 超出范围 (BQ chunks out of range): {chunks_per_signature}"
        )));
    }
    let count_u64 = read_u64_le(bytes, bq_offset + 8, "BQ count")?;
    let count = usize::try_from(count_u64)
        .map_err(|_| TriviumError::CorruptedFile(format!("BQ count 超出范围: {count_u64}")))?;
    let data_bytes = count
        .checked_mul(chunks_per_signature)
        .and_then(|value| value.checked_mul(8))
        .ok_or_else(|| TriviumError::CorruptedFile("BQ 块长度溢出".into()))?;
    let expected_end = header_end
        .checked_add(data_bytes)
        .ok_or_else(|| TriviumError::CorruptedFile("BQ 块结束偏移溢出".into()))?;
    if (version < 8 && expected_end != file_len) || (version >= 8 && expected_end > file_len) {
        return Err(TriviumError::CorruptedFile(format!(
            "BQ 块长度不匹配 (BQ block length mismatch): 期望 {expected_end}，实际 {file_len}"
        )));
    }
    if count == 0 && version < 8 {
        return Ok(None);
    }
    Ok(Some(BqDiskLayout {
        end_offset: expected_end,
    }))
}

fn load_unique_constraints<T: VectorType>(
    memtable: &mut MemTable<T>,
    bytes: &[u8],
    version: u16,
    layout: Option<BqDiskLayout>,
) -> Result<()> {
    if version < 8 {
        return Ok(());
    }
    let start = layout.map_or(bytes.len(), |layout| layout.end_offset);
    let block = bytes.get(start..).ok_or_else(|| {
        TriviumError::CorruptedFile(
            "唯一约束块偏移无效 (Invalid unique constraint block offset)".into(),
        )
    })?;
    if block.len() < 14 {
        return Err(TriviumError::CorruptedFile(
            "唯一约束块被截断 (Unique constraint block is truncated)".into(),
        ));
    }
    let payload_end = block.len() - 4;
    let expected_crc = u32::from_le_bytes(
        block[payload_end..]
            .try_into()
            .map_err(|_| TriviumError::CorruptedFile("唯一约束块校验和无效".into()))?,
    );
    if crc32fast::hash(&block[..payload_end]) != expected_crc {
        return Err(TriviumError::CorruptedFile(
            "唯一约束块 CRC32 不匹配 (Unique constraint block CRC32 mismatch)".into(),
        ));
    }
    let mut cursor = 0usize;
    if block.get(..4) != Some(UNIQUE_BLOCK_MAGIC.as_slice()) {
        return Err(TriviumError::CorruptedFile(
            "唯一约束块魔数无效 (Invalid unique constraint block magic)".into(),
        ));
    }
    cursor += 4;
    let block_version = read_u16_le(block, cursor, "unique version")?;
    cursor += 2;
    if block_version != UNIQUE_BLOCK_VERSION {
        return Err(TriviumError::CorruptedFile(format!(
            "不支持的唯一约束块版本 (Unsupported unique constraint block version): {block_version}"
        )));
    }
    let count = read_u32_le(block, cursor, "unique count")? as usize;
    cursor += 4;
    memtable.check_load_allocation(
        block
            .len()
            .saturating_mul(64)
            .saturating_add(count.saturating_mul(128)),
    )?;
    if count > payload_end.saturating_sub(cursor) / 6 {
        return Err(TriviumError::CorruptedFile(
            "唯一约束数量超出块边界 (Unique constraint count exceeds block bounds)".into(),
        ));
    }
    let mut definitions = Vec::new();
    definitions.try_reserve_exact(count).map_err(|error| {
        TriviumError::CapacityAllocationFailed {
            reason: format!("恢复唯一约束时分配失败: {error}"),
        }
    })?;
    for _ in 0..count {
        let field_count = read_u16_le(block, cursor, "unique field count")? as usize;
        cursor += 2;
        if field_count == 0 {
            return Err(TriviumError::CorruptedFile("唯一约束字段列表为空".into()));
        }
        if field_count > payload_end.saturating_sub(cursor) / 4 {
            return Err(TriviumError::CorruptedFile(
                "唯一约束字段数量超出块边界 (Unique field count exceeds block bounds)".into(),
            ));
        }
        let mut fields = Vec::new();
        fields.try_reserve_exact(field_count).map_err(|error| {
            TriviumError::CapacityAllocationFailed {
                reason: format!("恢复唯一约束字段时分配失败: {error}"),
            }
        })?;
        for _ in 0..field_count {
            let field_len = read_u32_le(block, cursor, "unique field length")? as usize;
            cursor += 4;
            let field_bytes = block
                .get(cursor..cursor.saturating_add(field_len))
                .ok_or_else(|| TriviumError::CorruptedFile("唯一约束字段被截断".into()))?;
            cursor = cursor.saturating_add(field_len);
            let field = std::str::from_utf8(field_bytes).map_err(|error| {
                TriviumError::CorruptedFile(format!("唯一约束字段不是 UTF-8: {error}"))
            })?;
            if field.is_empty() {
                return Err(TriviumError::CorruptedFile("唯一约束字段不能为空".into()));
            }
            fields.push(field.to_owned());
        }
        definitions.push(fields);
    }
    if cursor != payload_end {
        return Err(TriviumError::CorruptedFile(
            "唯一约束块存在尾随数据 (Trailing unique constraint data)".into(),
        ));
    }
    memtable.check_load_allocation(
        memtable
            .node_count()
            .saturating_mul(count)
            .saturating_mul(256),
    )?;
    memtable.restore_unique_index_definitions(&definitions)
}

fn admit_sidecar<T: VectorType>(mt: &MemTable<T>, path: &str, expansion: usize) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(meta) => mt.check_load_allocation(
            usize::try_from(meta.len())
                .unwrap_or(usize::MAX)
                .saturating_mul(expansion),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn load_mapped_graph<T: VectorType>(memtable: &mut MemTable<T>, db_path: &str) -> Result<()> {
    let peak = match std::fs::metadata(format!("{db_path}.gidx")) {
        Ok(meta) => usize::try_from(meta.len())
            .unwrap_or(usize::MAX)
            .saturating_mul(128),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    memtable.check_load_allocation(peak)?;
    if let Some(graph) = crate::storage::graph_blocks::MappedGraphStore::open_with_admission(
        db_path,
        memtable.node_count(),
        |extra| memtable.check_load_allocation(peak.saturating_add(extra)),
    )? {
        memtable.set_mapped_graph(graph);
    }
    Ok(())
}

fn load_property_indexes<T: VectorType>(
    memtable: &mut MemTable<T>,
    db_path: &str,
    _repair_sidecars: bool,
    missing_index_policy: crate::database::MissingIndexPolicy,
    mmap_property_postings: bool,
) -> Result<()> {
    if !Path::new(&format!("{db_path}.pidx")).exists() {
        return Ok(());
    }
    admit_sidecar(memtable, &format!("{db_path}.pidx"), 128)?;
    match crate::index::property::load_sidecar(
        db_path,
        memtable.node_count(),
        mmap_property_postings,
        memtable.payload_fingerprint(),
    ) {
        Ok(Some(indexes)) => {
            if indexes.key_encoding_version() < 2 || !indexes.fact_bound {
                let definitions = indexes.index_definitions();
                for (kind, fields) in &definitions {
                    memtable.admit_index_rebuild(*kind, fields, false)?;
                }
                memtable.rebuild_property_indexes_from_definitions(&definitions);
                tracing::info!(
                    "属性索引旧键编码已在内存重建；下次显式 flush 发布新编码 (Legacy property index keys rebuilt in memory)"
                );
            } else {
                memtable.set_property_indexes(indexes);
            }
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(error) => {
            if missing_index_policy == crate::database::MissingIndexPolicy::Error {
                return Err(error);
            }
            tracing::warn!(
                "属性索引 sidecar 无效，已回退为无索引打开 (Invalid property index sidecar; opened without property indexes): {}",
                error
            );
            Ok(())
        }
    }
}

pub(crate) fn read_deferred_text_index(
    db_path: &str,
    metadata: Option<[u8; 28]>,
) -> Result<crate::index::text::TextIndex> {
    let text_path = text_index_path_from_db(db_path);
    let invalid = |reason: String| TriviumError::ImmutableArtifactInvalid { reason };
    let text_size = std::fs::metadata(&text_path)
        .map_err(|error| invalid(format!("文本索引文件不可读取：{text_path}：{error}")))?
        .len();
    if let Some(meta) = metadata {
        let expected_size = read_u64_le(&meta, 16, "文本索引长度")?;
        if text_size != expected_size {
            return Err(invalid(format!(
                "文本索引长度不匹配：期望 {expected_size}，实际 {text_size}"
            )));
        }
        let expected_crc = read_u32_le(&meta, 24, "文本索引 CRC")?;
        let actual_crc = file_crc32(&text_path)?;
        if actual_crc != expected_crc {
            return Err(invalid(format!(
                "文本索引 CRC 不匹配：期望 {expected_crc:08x}，实际 {actual_crc:08x}"
            )));
        }
    } else {
        match std::fs::metadata(text_index_meta_path_from_db(db_path)) {
            Ok(_) => return Err(invalid("文本索引元数据格式或版本无效".into())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    // 主库布局变化不改变独立登记的文本事实；内容由文本长度、CRC 与快照格式校验。
    crate::index::text::TextIndex::load_from_file(Path::new(&text_path))
        .map_err(|error| invalid(format!("文本索引快照格式校验失败：{error}")))
}

fn load_text_index<T: VectorType>(
    memtable: &mut MemTable<T>,
    db_path: &str,
    _repair_sidecars: bool,
    policy: crate::database::MissingIndexPolicy,
) -> Result<()> {
    let text_path = text_index_path_from_db(db_path);
    let path = std::path::Path::new(&text_path);
    if !path.exists() {
        if policy == crate::database::MissingIndexPolicy::Error {
            return Err(TriviumError::ImmutableArtifactInvalid {
                reason: "TextIndex sidecar 缺失".into(),
            });
        }
        return Ok(());
    }
    admit_sidecar(memtable, &text_path, 64)?;
    match read_deferred_text_index(db_path, deferred_text_metadata(db_path)) {
        Ok(index) => memtable.set_text_index(index),
        Err(error) => {
            if policy == crate::database::MissingIndexPolicy::Error {
                return Err(error);
            }
            tracing::warn!("文本索引加载失败，已保留原文件供恢复：{error}");
            memtable.defer_text_index(db_path);
        }
    }
    Ok(())
}

/// 尝试从 .tdb.quiver 文件加载 QuIVer 索引
///
/// 如果文件不存在或加载失败，静默跳过（首次查询时惰性重建）。
fn load_quiver_index<T: VectorType>(
    memtable: &mut MemTable<T>,
    db_path: &str,
    repair_sidecars: bool,
    policy: crate::database::MissingIndexPolicy,
) -> Result<()> {
    use crate::index::quiver::QuIVer;

    if crate::vector::is_binary_vector::<T>() {
        return Ok(());
    }

    let quiver_path = quiver_path_from_db(db_path);
    let qp = std::path::Path::new(&quiver_path);
    if !qp.exists() {
        if policy == crate::database::MissingIndexPolicy::Error {
            return Err(TriviumError::ImmutableArtifactInvalid {
                reason: "QuIVer sidecar 缺失".into(),
            });
        }
        return Ok(());
    }

    if !validate_quiver_meta(memtable, db_path) {
        tracing::warn!(
            repair = repair_sidecars,
            "QuIVer sidecar 元数据缺失或不匹配，已拒绝 (QuIVer sidecar metadata missing or mismatched, rejected)"
        );
        if repair_sidecars {
            std::fs::remove_file(qp).ok();
            std::fs::remove_file(quiver_meta_path_from_db(db_path)).ok();
        }
        if policy == crate::database::MissingIndexPolicy::Error {
            return Err(TriviumError::ImmutableArtifactInvalid {
                reason: "QuIVer sidecar 元数据不匹配".into(),
            });
        }
        return Ok(());
    }

    admit_sidecar(memtable, &quiver_path, 8)?;
    match QuIVer::load_from_file(qp) {
        Ok(quiver) if quiver.matches_fact_slots(memtable) => {
            memtable.set_quiver_index(quiver);
            memtable.vec_pool_mut().advise_random();
        }
        Ok(_) => {
            if policy == crate::database::MissingIndexPolicy::Error {
                return Err(TriviumError::ImmutableArtifactInvalid {
                    reason: "QuIVer ID 与 slot 不匹配 (QuIVer ID/slot mapping mismatch)".into(),
                });
            }
            tracing::warn!(
                "QuIVer ID 与 slot 不匹配，已拒绝 (QuIVer ID/slot mapping mismatch, rejected)"
            );
        }
        Err(e) => {
            tracing::warn!(
                "QuIVer 索引加载失败 (QuIVer index load failed)（将在首次查询时自动重建）: {}",
                e
            );
            if policy == crate::database::MissingIndexPolicy::Error {
                return Err(TriviumError::ImmutableArtifactInvalid {
                    reason: format!("QuIVer sidecar 损坏: {e}"),
                });
            }
        }
    }
    Ok(())
}

/// 复用正式头部常量与解析函数，核对快照声明的基础事实，不物化数据库。
pub(crate) fn validate_snapshot_metadata(
    path: &std::path::Path,
    expected_dim: usize,
    expected_nodes: usize,
) -> Result<()> {
    use std::io::Read;
    let mut bytes = [0u8; LEGACY_HEADER_SIZE as usize];
    std::fs::File::open(path)?.read_exact(&mut bytes)?;
    let version = read_u16_le(&bytes, 4, "快照头部版本")?;
    let dim = read_u32_le(&bytes, 6, "快照头部维度")? as usize;
    let nodes = read_u64_le(&bytes, 18, "快照头部节点数")?;
    if &bytes[..4] != MAGIC
        || !(MINIMUM_SUPPORTED_VERSION..=CURRENT_VERSION).contains(&version)
        || dim != expected_dim
        || nodes != expected_nodes as u64
        || dim == 0
        || dim > 65536
    {
        return Err(TriviumError::ImmutableArtifactInvalid {
            reason: "快照主文件头与 manifest 不匹配".into(),
        });
    }
    Ok(())
}
