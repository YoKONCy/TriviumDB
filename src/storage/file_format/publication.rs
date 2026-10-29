//! 主数据与索引文件组的发布撤销日志；先保留旧代，再替换正式文件。
//!
//! 日志与备份只供可写打开恢复，不能用来掩盖没有发布日志的文件损坏。
//! 恢复复制而不消耗备份，直到所有恢复文件 fsync 后才移除日志，支持恢复途中再次断电。
//! Mmap 在索引同步后提交 marker；Rom 使用携带新主文件校验的日志提交。

use super::*;
use crate::storage::fs::sync_parent_directory;
use std::io::Read;

// 魔数(4) + 版本(1) + 目标代(8) + 三个文件的存在标记/长度/CRC(39) + 日志 CRC(4)。
const JOURNAL_SIZE: usize = 56;
const GROUP_SUFFIXES: [&str; 9] = [
    "",
    ".vec",
    ".flush_ok",
    ".quiver",
    ".quiver.meta",
    ".text",
    ".text.meta",
    ".pidx",
    ".gidx",
];
const GROUP_JOURNAL_SIZE: usize = 15 + GROUP_SUFFIXES.len() * 13 + 4;

struct Journal {
    generation: u64,
    rom: bool,
    committed: bool,
    files: Vec<Option<SavedFile>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SavedFile {
    present: bool,
    size: u64,
    crc: u32,
}

fn journal_path(path: &str) -> String {
    format!("{path}.publish")
}

fn backup_path(path: &str, index: usize) -> String {
    format!("{path}.publish.{index}")
}

pub(super) fn pending(path: &str) -> bool {
    Path::new(&journal_path(path)).exists()
}

/// 同一句柄重试时沿用已校验的旧代备份，不能把半发布的新文件再次当作旧代备份。
pub(super) fn resume<T: VectorType>(path: &str, preserve_text: bool) -> Result<Option<u64>> {
    if !pending(path) {
        return Ok(None);
    }
    match recover::<T>(path, false) {
        Err(TriviumError::RecoveryRequired { .. }) => {
            let journal = read_journal(path)?;
            if !preserve_text
                && journal.files.len() == GROUP_SUFFIXES.len()
                && journal.files[5].is_none()
            {
                // 失败后调用方可继续登记文本；首次改写前把原本保持不变的文本纳入撤销组。
                let saved = identity(&format!("{path}.text"))?;
                if saved.present {
                    copy_synced(&format!("{path}.text"), &backup_path(path, 5))?;
                }
                let mut bytes = std::fs::read(journal_path(path))?;
                let offset = 15 + 5 * 13;
                bytes[offset] = if saved.present { 2 } else { 1 };
                bytes[offset + 1..offset + 9].copy_from_slice(&saved.size.to_le_bytes());
                bytes[offset + 9..offset + 13].copy_from_slice(&saved.crc.to_le_bytes());
                let crc_offset = GROUP_JOURNAL_SIZE - 4;
                let crc = crc32fast::hash(&bytes[..crc_offset]);
                bytes[crc_offset..].copy_from_slice(&crc.to_le_bytes());
                let temporary = format!("{}.tmp", journal_path(path));
                let mut output = File::create(&temporary)?;
                output.write_all(&bytes)?;
                output.sync_all()?;
                drop(output);
                robust_rename_and_sync(Path::new(&temporary), Path::new(&journal_path(path)))?;
            }
            Ok(Some(journal.generation))
        }
        Ok(_) => {
            cleanup(path)?;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn identity(path: &str) -> Result<SavedFile> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(SavedFile {
            present: true,
            size: meta.len(),
            crc: file_crc32(path)?,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SavedFile {
            present: false,
            size: 0,
            crc: 0,
        }),
        Err(error) => Err(error.into()),
    }
}

fn copy_synced(source: &str, destination: &str) -> Result<()> {
    let temporary = format!("{destination}.tmp");
    let mut source = File::open(source)?;
    let mut output = File::create(&temporary)?;
    // 固定大小的流式复制，不把整库读入堆；不能用硬链接备份会追加写入的 .vec。
    std::io::copy(&mut source, &mut output)?;
    output.sync_all()?;
    drop(output);
    robust_rename_and_sync(Path::new(&temporary), Path::new(destination))?;
    Ok(())
}

pub(super) fn begin(
    path: &str,
    generation: u64,
    mode: StorageMode,
    preserve_text: bool,
) -> Result<()> {
    let mut bytes = [0u8; GROUP_JOURNAL_SIZE];
    bytes[..4].copy_from_slice(b"TPUB");
    bytes[4] = 2;
    bytes[5] = u8::from(mode == StorageMode::Rom);
    bytes[7..15].copy_from_slice(&generation.to_le_bytes());
    for (index, suffix) in GROUP_SUFFIXES.iter().enumerate() {
        // 纯向量保存保持文本文件原样，省去大文本快照的复制。
        if *suffix == ".text" && preserve_text {
            continue;
        }
        let source = format!("{path}{suffix}");
        let saved = identity(&source)?;
        if saved.present {
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::io_result(crate::test_hooks::IoPoint::PublicationBackupCopy)?;
            copy_synced(&source, &backup_path(path, index))?;
        }
        let offset = 15 + index * 13;
        bytes[offset] = if saved.present { 2 } else { 1 };
        bytes[offset + 1..offset + 9].copy_from_slice(&saved.size.to_le_bytes());
        bytes[offset + 9..offset + 13].copy_from_slice(&saved.crc.to_le_bytes());
    }
    let crc = crc32fast::hash(&bytes[..GROUP_JOURNAL_SIZE - 4]);
    bytes[GROUP_JOURNAL_SIZE - 4..].copy_from_slice(&crc.to_le_bytes());
    let journal = journal_path(path);
    let temporary = format!("{journal}.tmp");
    let mut output = File::create(&temporary)?;
    output.write_all(&bytes)?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::PublicationJournalSync)?;
    output.sync_all()?;
    drop(output);
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::PublicationJournalRename)?;
    robust_rename_and_sync(Path::new(&temporary), Path::new(&journal))?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::AfterPublicationPrepared);
    Ok(())
}

fn read_journal(path: &str) -> Result<Journal> {
    let mut input = File::open(journal_path(path))?;
    let size = input.metadata()?.len();
    if size != JOURNAL_SIZE as u64 && size != GROUP_JOURNAL_SIZE as u64 {
        return Err(TriviumError::CorruptedFile("发布恢复日志长度无效".into()));
    }
    let mut bytes = vec![0; size as usize];
    input.read_exact(&mut bytes)?;
    if size == JOURNAL_SIZE as u64 {
        let (generation, files) = decode(&bytes)?;
        return Ok(Journal {
            generation,
            rom: false,
            committed: false,
            files: files.into_iter().map(Some).collect(),
        });
    }
    decode_group(&bytes)
}

fn decode_group(bytes: &[u8]) -> Result<Journal> {
    let crc_offset = GROUP_JOURNAL_SIZE - 4;
    if bytes.len() != GROUP_JOURNAL_SIZE
        || &bytes[..5] != b"TPUB\x02"
        || bytes[5] > 1
        || bytes[6] > bytes[5]
        || crc32fast::hash(&bytes[..crc_offset]) != read_u32_le(bytes, crc_offset, "发布日志 CRC")?
    {
        return Err(TriviumError::CorruptedFile(
            "文件组发布日志格式或 CRC 无效".into(),
        ));
    }
    let generation = read_u64_le(bytes, 7, "发布目标代")?;
    if generation == 0 {
        return Err(TriviumError::CorruptedFile("发布目标代不能为零".into()));
    }
    let mut files = Vec::with_capacity(GROUP_SUFFIXES.len());
    for index in 0..GROUP_SUFFIXES.len() {
        let offset = 15 + index * 13;
        let state = bytes[offset];
        let size = read_u64_le(bytes, offset + 1, "备份长度")?;
        let crc = read_u32_le(bytes, offset + 9, "备份 CRC")?;
        if state > 2
            || (state < 2 && (size != 0 || crc != 0))
            || (state == 0 && GROUP_SUFFIXES[index] != ".text")
        {
            return Err(TriviumError::CorruptedFile("文件组备份几何信息无效".into()));
        }
        files.push((state != 0).then_some(SavedFile {
            present: state == 2,
            size,
            crc,
        }));
    }
    Ok(Journal {
        generation,
        rom: bytes[5] == 1,
        committed: bytes[6] == 1,
        files,
    })
}

/// Rom 以完整文件组同步后的日志替换作为提交点。
pub(super) fn commit_rom(path: &str) -> Result<()> {
    let journal = read_journal(path)?;
    if !journal.rom {
        return Err(TriviumError::CorruptedFile("Rom 发布日志模式无效".into()));
    }
    let mut bytes = std::fs::read(journal_path(path))?;
    bytes[6] = 1;
    // 已提交的 Rom 日志记录新主文件身份，恢复时据此验证已提交事实。
    let main = identity(path)?;
    bytes[15] = if main.present { 2 } else { 1 };
    bytes[16..24].copy_from_slice(&main.size.to_le_bytes());
    bytes[24..28].copy_from_slice(&main.crc.to_le_bytes());
    let crc_offset = GROUP_JOURNAL_SIZE - 4;
    let crc = crc32fast::hash(&bytes[..crc_offset]);
    bytes[crc_offset..].copy_from_slice(&crc.to_le_bytes());
    let target = journal_path(path);
    let temporary = format!("{target}.tmp");
    let mut output = File::create(&temporary)?;
    output.write_all(&bytes)?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::PublicationCommitSync)?;
    output.sync_all()?;
    drop(output);
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::PublicationCommitRename)?;
    robust_rename_and_sync(Path::new(&temporary), Path::new(&target))?;
    Ok(())
}

fn decode(bytes: &[u8]) -> Result<(u64, [SavedFile; 3])> {
    if bytes.len() != JOURNAL_SIZE
        || &bytes[..4] != b"TPUB"
        || bytes[4] != 1
        || crc32fast::hash(&bytes[..52]) != read_u32_le(bytes, 52, "发布日志 CRC")?
    {
        return Err(TriviumError::CorruptedFile(
            "发布恢复日志格式或 CRC 无效".into(),
        ));
    }
    let generation = read_u64_le(bytes, 5, "发布目标代")?;
    if generation == 0 {
        return Err(TriviumError::CorruptedFile("发布目标代不能为零".into()));
    }
    let mut files = [SavedFile {
        present: false,
        size: 0,
        crc: 0,
    }; 3];
    for (index, saved) in files.iter_mut().enumerate() {
        let offset = 13 + index * 13;
        *saved = SavedFile {
            present: bytes[offset] == 1,
            size: read_u64_le(bytes, offset + 1, "备份长度")?,
            crc: read_u32_le(bytes, offset + 9, "备份 CRC")?,
        };
        if bytes[offset] > 1 || (!saved.present && (saved.size != 0 || saved.crc != 0)) {
            return Err(TriviumError::CorruptedFile("发布备份几何信息无效".into()));
        }
    }
    Ok((generation, files))
}

/// 返回是否回滚了未提交的文件；只读路径只检查，不创建、删除或修改文件。
pub(crate) fn recover<T: VectorType>(path: &str, writable: bool) -> Result<bool> {
    let journal = journal_path(path);
    let state = match read_journal(path) {
        Ok(state) => state,
        Err(TriviumError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let generation = state.generation;
    let files = state.files;
    let marker = flush_ok_path_from_db(path);
    let committed = if state.rom {
        state.committed
    } else {
        read_marker_generation(&marker) == Some(generation)
    };
    if committed {
        if state.rom
            && (files[0].is_none_or(|main| !main.present) || Some(identity(path)?) != files[0])
        {
            return Err(TriviumError::CorruptedFile(
                "已提交 Rom 发布代的主文件损坏".into(),
            ));
        }
        if !state.rom
            && !validate_flush_marker(
                &marker,
                path,
                &vec_path_from_db(path),
                &payload_path_for_generation(path, generation),
            )
        {
            // marker 已提交后，即使新代损坏也不能回退；WAL 可能已清空。
            return Err(TriviumError::CorruptedFile(
                "已提交发布代的事实文件损坏".into(),
            ));
        }
        if writable {
            validate_checkpoint_type::<T>(path, &vec_path_from_db(path), path)?;
            cleanup(path)?;
        }
        return Ok(false);
    }
    // 只接受仍指向旧代的 marker，拒绝把任意损坏的提交标记当作未提交发布。
    if !state.rom && Some(identity(&marker)?) != files[2] {
        return Err(TriviumError::CorruptedFile(
            "发布提交标记与恢复日志不匹配".into(),
        ));
    }
    let expected_generation = match read_marker_generation(&marker) {
        Some(previous) => previous.checked_add(1),
        None => Some(1),
    };
    if !state.rom && expected_generation != Some(generation) {
        return Err(TriviumError::CorruptedFile(
            "发布目标代与旧提交标记不连续".into(),
        ));
    }
    for (index, saved) in files.iter().enumerate() {
        if let Some(saved) = saved
            && saved.present
            && identity(&backup_path(path, index))? != *saved
        {
            return Err(TriviumError::CorruptedFile("发布恢复备份缺失或损坏".into()));
        }
    }
    if !writable {
        return Err(TriviumError::RecoveryRequired { wal_path: journal });
    }
    if files[0].is_some_and(|main| main.present) {
        validate_checkpoint_type::<T>(&backup_path(path, 0), &backup_path(path, 1), path)?;
    }
    // 所有备份通过校验后才恢复正式文件；备份保留到恢复提交完成。
    for (index, saved) in files.iter().enumerate() {
        let Some(saved) = saved else {
            continue;
        };
        let suffix = GROUP_SUFFIXES[index];
        let destination = format!("{path}{suffix}");
        if saved.present {
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::io_result(crate::test_hooks::IoPoint::PublicationRestoreCopy)?;
            copy_synced(&backup_path(path, index), &destination)?;
        } else {
            match std::fs::remove_file(&destination) {
                Ok(()) => sync_parent_directory(Path::new(&destination))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::AfterPublicationRestore);
    }
    cleanup(path)?;
    Ok(true)
}

pub(super) fn cleanup(path: &str) -> Result<()> {
    let journal = journal_path(path);
    std::fs::remove_file(&journal)?;
    // 日志的删除先持久化，才能删除其引用的备份；否则断电可能留下有日志、无备份的状态。
    sync_parent_directory(Path::new(&journal))?;
    for index in 0..GROUP_SUFFIXES.len() {
        std::fs::remove_file(backup_path(path, index)).ok();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 发布日志拒绝截断_未知版本_无效存在位与损坏crc() {
        // 固定布局的独立字节 oracle，不能由生产编码器生成。
        let mut bytes = [0u8; JOURNAL_SIZE];
        bytes[..5].copy_from_slice(b"TPUB\x01");
        bytes[5] = 1;
        let crc = crc32fast::hash(&bytes[..52]);
        bytes[52..].copy_from_slice(&crc.to_le_bytes());
        assert!(decode(&bytes).is_ok());
        for length in 0..JOURNAL_SIZE {
            assert!(decode(&bytes[..length]).is_err());
        }
        for offset in 0..JOURNAL_SIZE {
            let mut corrupt = bytes;
            corrupt[offset] ^= 1;
            assert!(decode(&corrupt).is_err());
        }
        for (offset, value) in [(4, 2), (5, 0), (13, 2), (14, 1)] {
            let mut corrupt = bytes;
            corrupt[offset] = value;
            let crc = crc32fast::hash(&corrupt[..52]);
            corrupt[52..].copy_from_slice(&crc.to_le_bytes());
            assert!(decode(&corrupt).is_err());
        }
    }

    #[test]
    fn 文件组日志拒绝截断_错误状态_未知版本与损坏crc() {
        let mut bytes = [0u8; GROUP_JOURNAL_SIZE];
        bytes[..5].copy_from_slice(b"TPUB\x02");
        bytes[7] = 1;
        for index in 0..9 {
            bytes[15 + index * 13] = 1;
        }
        let sign = |bytes: &mut [u8; GROUP_JOURNAL_SIZE]| {
            let crc = crc32fast::hash(&bytes[..GROUP_JOURNAL_SIZE - 4]);
            bytes[GROUP_JOURNAL_SIZE - 4..].copy_from_slice(&crc.to_le_bytes());
        };
        sign(&mut bytes);
        assert!(decode_group(&bytes).is_ok());
        for length in 0..GROUP_JOURNAL_SIZE {
            assert!(decode_group(&bytes[..length]).is_err());
        }
        for offset in 0..GROUP_JOURNAL_SIZE {
            let mut corrupt = bytes;
            corrupt[offset] ^= 1;
            assert!(decode_group(&corrupt).is_err());
        }
        for (offset, value) in [(4, 3), (5, 2), (6, 1), (7, 0), (15, 0), (15, 3), (16, 1)] {
            let mut corrupt = bytes;
            corrupt[offset] = value;
            sign(&mut corrupt);
            assert!(decode_group(&corrupt).is_err());
        }
        bytes[15 + 5 * 13] = 0;
        sign(&mut bytes);
        assert!(decode_group(&bytes).unwrap().files[5].is_none());
        bytes[5] = 1;
        bytes[6] = 1;
        sign(&mut bytes);
        assert!(decode_group(&bytes).unwrap().committed);
    }
}
