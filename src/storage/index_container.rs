//! 统一索引容器。固定目录限定组件范围，64 KiB 对齐支持映射与 Direct I/O。
//!
//! 文本、关键词和索引定义随组件保留。发布只替换完整代际，读取持有原文件句柄。
//! 独立组件读取供发布暂存与外部迁移使用；普通数据库打开要求当前主格式及绑定的 `.idx`。

use super::artifacts::INDEX_SUFFIXES;
use super::fs::robust_rename_and_sync;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const HEADER: usize = 512;
const ALIGNMENT: u64 = 65536;
const MAGIC: &[u8; 8] = b"TDBINDEX";
const BASE_COMPONENTS: usize = 7;
const COMPONENTS: usize = INDEX_SUFFIXES.len();

fn entry_offset(index: usize) -> usize {
    if index < BASE_COMPONENTS {
        32 + index * 32
    } else {
        288 + (index - BASE_COMPONENTS) * 32
    }
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "统一索引容器身份、目录或校验无效 (Invalid index container identity, directory or checksum)",
    )
}

#[derive(Clone, Copy, Default)]
struct Entry {
    offset: u64,
    length: u64,
    crc: u32,
}

struct Directory {
    entries: [Entry; COMPONENTS],
    main_length: u64,
    main_crc: u32,
    database_id: [u8; 16],
    checkpoint_id: [u8; 16],
}

fn number(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

impl Directory {
    fn read(file: &mut File) -> io::Result<Self> {
        let mut bytes = [0u8; HEADER];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut bytes)?;
        if &bytes[..8] != MAGIC
            || bytes[8..12] != 1u32.to_le_bytes()
            || bytes[12..16] != (HEADER as u32).to_le_bytes()
            || crc32fast::hash(&bytes[..508])
                != u32::from_le_bytes(bytes[508..].try_into().unwrap())
            || bytes[28..32] != [0; 4]
            || bytes[entry_offset(COMPONENTS)..508].iter().any(|b| *b != 0)
            || bytes[256..272] == [0; 16]
            || bytes[272..288] == [0; 16]
        {
            return Err(invalid());
        }
        let size = file.metadata()?.len();
        let mut entries = [Entry::default(); COMPONENTS];
        let mut end = HEADER as u64;
        for (index, entry) in entries.iter_mut().enumerate() {
            let at = entry_offset(index);
            let flags = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            if flags == 0 {
                if bytes[at..at + 32].iter().any(|b| *b != 0) {
                    return Err(invalid());
                }
                continue;
            }
            if flags != (index + 1) as u32
                || bytes[at + 4..at + 8] != [0; 4]
                || bytes[at + 28..at + 32] != [0; 4]
            {
                return Err(invalid());
            }
            *entry = Entry {
                offset: number(&bytes, at + 8),
                length: number(&bytes, at + 16),
                crc: u32::from_le_bytes(bytes[at + 24..at + 28].try_into().unwrap()),
            };
            let next = entry.offset.checked_add(entry.length).ok_or_else(invalid)?;
            if entry.length == 0
                || entry.offset < end
                || entry.offset % ALIGNMENT != 0
                || next > size
            {
                return Err(invalid());
            }
            end = next;
        }
        if end != size {
            return Err(invalid());
        }
        if (entries[0].length == 0) != (entries[1].length == 0)
            || (entries[2].length == 0) != (entries[3].length == 0)
            || (entries[8].length != 0 && entries[7].length == 0)
        {
            return Err(invalid());
        }
        Ok(Self {
            entries,
            database_id: bytes[256..272].try_into().unwrap(),
            checkpoint_id: bytes[272..288].try_into().unwrap(),
            main_length: number(&bytes, 16),
            main_crc: u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
        })
    }

    fn bytes(&self) -> [u8; HEADER] {
        let mut bytes = [0; HEADER];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER as u32).to_le_bytes());
        bytes[16..24].copy_from_slice(&self.main_length.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.main_crc.to_le_bytes());
        bytes[256..272].copy_from_slice(&self.database_id);
        bytes[272..288].copy_from_slice(&self.checkpoint_id);
        for (index, entry) in self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.length != 0)
        {
            let at = entry_offset(index);
            bytes[at..at + 4].copy_from_slice(&((index + 1) as u32).to_le_bytes());
            bytes[at + 8..at + 16].copy_from_slice(&entry.offset.to_le_bytes());
            bytes[at + 16..at + 24].copy_from_slice(&entry.length.to_le_bytes());
            bytes[at + 24..at + 28].copy_from_slice(&entry.crc.to_le_bytes());
        }
        let crc = crc32fast::hash(&bytes[..508]);
        bytes[508..].copy_from_slice(&crc.to_le_bytes());
        bytes
    }
}

/// 组件借用限定在同一只读文件句柄的范围内，避免路径替换后的代际混用。
pub(crate) struct Part {
    pub file: File,
    pub offset: u64,
    pub length: u64,
    crc: Option<u32>,
}

impl Part {
    pub(crate) fn staged(file: File) -> io::Result<Self> {
        let length = file.metadata()?.len();
        Ok(Self {
            file,
            offset: 0,
            length,
            crc: None,
        })
    }

    pub fn reader(mut self) -> io::Result<std::io::Take<File>> {
        self.file.seek(SeekFrom::Start(self.offset))?;
        Ok(self.file.take(self.length))
    }

    pub fn checksum(&mut self) -> io::Result<u32> {
        self.file.seek(SeekFrom::Start(self.offset))?;
        let mut remaining = self.length;
        let mut buffer = [0; 65536];
        let mut hash = crc32fast::Hasher::new();
        while remaining > 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            self.file.read_exact(&mut buffer[..count])?;
            hash.update(&buffer[..count]);
            remaining -= count as u64;
        }
        let crc = hash.finalize();
        if self.crc.is_some_and(|expected| expected != crc) {
            return Err(invalid());
        }
        Ok(crc)
    }

    pub fn map(self) -> io::Result<memmap2::Mmap> {
        let length = usize::try_from(self.length).map_err(|_| invalid())?;
        // SAFETY: 目录已验证 offset + length 在文件内，映射仅覆盖该只读组件。
        // memmap2 处理平台映射粒度；返回映射独占其生命周期，发布使用完整文件替换。
        let map = unsafe {
            memmap2::MmapOptions::new()
                .offset(self.offset)
                .len(length)
                .map(&self.file)?
        };
        if self.crc.is_some_and(|crc| crc32fast::hash(&map) != crc) {
            return Err(invalid());
        }
        Ok(map)
    }
}

pub(crate) fn open(db_path: &str, suffix: &str) -> io::Result<Option<Part>> {
    let index = INDEX_SUFFIXES
        .iter()
        .position(|item| *item == suffix)
        .ok_or_else(invalid)?;
    match File::open(format!("{db_path}.idx")) {
        Ok(mut file) => {
            let entry = Directory::read(&mut file)?.entries[index];
            Ok((entry.length != 0).then_some(Part {
                file,
                offset: entry.offset,
                length: entry.length,
                crc: Some(entry.crc),
            }))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn standalone(path: &Path) -> io::Result<Option<Part>> {
    match File::open(path) {
        Ok(file) => {
            let length = file.metadata()?.len();
            Ok(Some(Part {
                file,
                offset: 0,
                length,
                crc: None,
            }))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn required(db_path: &str, suffix: &str) -> io::Result<Part> {
    open(db_path, suffix)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "索引组件缺失 (Index component missing)",
        )
    })
}

pub(crate) fn validate(db_path: &str, required: bool) -> io::Result<()> {
    validate_bound(db_path, required, None)
}

/// 已提交恢复在清理原组件和撤销组之前核对全部内容，损坏时保留现场。
pub(crate) fn validate_complete(db_path: &str) -> io::Result<()> {
    validate(db_path, true)?;
    for suffix in INDEX_SUFFIXES {
        if let Some(mut part) = open(db_path, suffix)? {
            part.checksum()?;
        }
    }
    Ok(())
}

pub(crate) fn validate_bound(
    db_path: &str,
    required: bool,
    main_crc: Option<u32>,
) -> io::Result<()> {
    let mut file = match File::open(format!("{db_path}.idx")) {
        Ok(file) => file,
        Err(error) if !required && error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let directory = Directory::read(&mut file)?;
    let (database_id, checkpoint_id) = identities(db_path)?;
    if std::fs::metadata(db_path)?.len() != directory.main_length
        || database_id != directory.database_id
        || checkpoint_id != directory.checkpoint_id
        || match main_crc {
            Some(crc) => crc,
            None => super::file_format::file_crc32(db_path)?,
        } != directory.main_crc
    {
        return Err(invalid());
    }
    Ok(())
}

/// 从本次写出的组件和保持不变的旧文本构建候选。调用方持有数据库写租约及发布日志。
pub(crate) fn publish_checkpoint(
    db_path: &str,
    preserve_text: bool,
    pelorus: [Option<Part>; 2],
) -> io::Result<()> {
    let mut parts = Vec::with_capacity(COMPONENTS);
    for suffix in INDEX_SUFFIXES.into_iter().take(BASE_COMPONENTS) {
        parts.push(if suffix == ".text" && preserve_text || suffix == ".qssd" {
            open(db_path, suffix)?
        } else {
            standalone(Path::new(&format!("{db_path}{suffix}")))?
        });
    }
    parts.extend(pelorus);
    publish(db_path, parts)
}

/// 派生 SSD 更新保持同一事实身份与其他组件，旧 Reader 继续持有旧代文件。
pub(crate) fn publish_ssd(db_path: &str) -> io::Result<()> {
    validate(db_path, true)?;
    let mut parts = Vec::with_capacity(COMPONENTS);
    for suffix in INDEX_SUFFIXES {
        parts.push(if suffix == ".qssd" {
            standalone(Path::new(&format!("{db_path}{suffix}")))?
        } else {
            open(db_path, suffix)?
        });
    }
    publish(db_path, parts)?;
    cleanup_components(db_path)
}

fn identities(db_path: &str) -> io::Result<([u8; 16], [u8; 16])> {
    let mut file = File::open(db_path)?;
    let mut header = [0; 110];
    file.read_exact(&mut header)?;
    if &header[..4] != b"TVDB"
        || u16::from_le_bytes([header[4], header[5]]) != super::file_format::CURRENT_VERSION
        || crc32fast::hash(&header[..106])
            != u32::from_le_bytes(header[106..110].try_into().unwrap())
        || header[74..90] == [0; 16]
        || header[90..106] == [0; 16]
    {
        return Err(invalid());
    }
    Ok((
        header[74..90].try_into().unwrap(),
        header[90..106].try_into().unwrap(),
    ))
}

fn publish(db_path: &str, parts: Vec<Option<Part>>) -> io::Result<()> {
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::IndexContainerWrite)?;
    let path = PathBuf::from(format!("{db_path}.idx"));
    let temporary = PathBuf::from(format!("{db_path}.idx.tmp"));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)?;
    file.write_all(&[0u8; HEADER])?;
    let (database_id, checkpoint_id) = identities(db_path)?;
    let mut directory = Directory {
        entries: [Entry::default(); COMPONENTS],
        database_id,
        checkpoint_id,
        main_length: std::fs::metadata(db_path)?.len(),
        main_crc: super::file_format::file_crc32(db_path)?,
    };
    let mut end = HEADER as u64;
    for (index, part) in parts.into_iter().enumerate() {
        let Some(part) = part else { continue };
        if part.length == 0 {
            return Err(invalid());
        }
        let offset = end.checked_add(ALIGNMENT - 1).ok_or_else(invalid)? / ALIGNMENT * ALIGNMENT;
        file.seek(SeekFrom::Start(offset))?;
        let length = part.length;
        let expected_crc = part.crc;
        let mut reader = part.reader()?;
        let mut hash = crc32fast::Hasher::new();
        let mut remaining = length;
        let mut buffer = [0u8; 65536];
        while remaining != 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            reader.read_exact(&mut buffer[..count])?;
            hash.update(&buffer[..count]);
            file.write_all(&buffer[..count])?;
            remaining -= count as u64;
        }
        let crc = hash.finalize();
        if expected_crc.is_some_and(|expected| expected != crc) {
            return Err(invalid());
        }
        directory.entries[index] = Entry {
            offset,
            length,
            crc,
        };
        end = offset.checked_add(length).ok_or_else(invalid)?;
    }
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&directory.bytes())?;
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::IndexContainerSync)?;
    file.sync_all()?;
    Directory::read(&mut file)?;
    drop(file);
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::IndexContainerPublish)?;
    robust_rename_and_sync(&temporary, &path)
}

/// 只在文件组提交成功之后删除已归入容器的独立组件。
pub(crate) fn cleanup_components(db_path: &str) -> io::Result<()> {
    for suffix in INDEX_SUFFIXES.into_iter().take(BASE_COMPONENTS) {
        match std::fs::remove_file(format!("{db_path}{suffix}")) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    super::fs::sync_parent_directory(Path::new(db_path))
}
