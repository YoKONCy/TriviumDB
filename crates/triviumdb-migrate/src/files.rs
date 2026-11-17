use crate::{Failure, Owner, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub(crate) fn utf8(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        Failure::new(
            "TDB_INVALID_INPUT",
            "路径必须为有效 UTF-8 (Path must be valid UTF-8)",
        )
    })
}

pub(crate) struct Lease {
    _file: File,
}
impl Lease {
    fn acquire(path: &Path, create: bool, shared: bool) -> Result<Self> {
        if let Ok(metadata) = std::fs::symlink_metadata(path)
            && !metadata.file_type().is_file()
        {
            return Err(Failure::new(
                "TDB_INVALID_INPUT",
                "协调文件必须是普通文件 (Coordination path must be a regular file)",
            ));
        }
        let file = OpenOptions::new().read(true).write(create).create(create).truncate(false).open(path).map_err(|error| {
            if !create && error.kind() == std::io::ErrorKind::NotFound {
                Failure::new("TDB_MIGRATION_LEASE_REQUIRED", "检查模式需要已有数据库锁文件；先用来源版本正常打开并关闭数据库 (Check mode requires an existing database lock file; open and close it with the source version first)")
            } else { error.into() }
        })?;
        let result = if shared {
            FileExt::try_lock_shared(&file)
        } else {
            FileExt::try_lock_exclusive(&file)
        };
        result.map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
            {
                Failure::new(
                    "TDB_DATABASE_LOCKED",
                    "数据库或迁移目标正被使用 (Database or migration destination is in use)",
                )
            } else {
                error.into()
            }
        })?;
        Ok(Self { _file: file })
    }
    pub fn source(path: &Path, create: bool) -> Result<Self> {
        if std::fs::symlink_metadata(format!("{}.migration", utf8(path)?)).is_ok() {
            return Err(Failure::new(
                "TDB_DATABASE_LOCKED",
                "来源仍处于迁移发布阶段 (Source is still being published by a migration)",
            ));
        }
        let lease = Self::acquire(
            &PathBuf::from(format!("{}.lock", utf8(path)?)),
            create,
            false,
        )?;
        if std::fs::symlink_metadata(format!("{}.migration", utf8(path)?)).is_ok() {
            return Err(Failure::new(
                "TDB_DATABASE_LOCKED",
                "来源仍处于迁移发布阶段 (Source is still being published by a migration)",
            ));
        }
        Ok(lease)
    }
    pub fn target(path: &Path) -> Result<Self> {
        let leaf = path.file_name().ok_or_else(|| {
            Failure::new(
                "TDB_INVALID_INPUT",
                "目标缺少名称 (Destination has no name)",
            )
        })?;
        Self::acquire(
            &path.with_file_name(format!(".{}.migration.lock", utf8(Path::new(leaf))?)),
            true,
            false,
        )
    }
    pub fn published(path: &Path) -> Result<Self> {
        Self::acquire(&PathBuf::from(format!("{}.lock", utf8(path)?)), false, true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceFile {
    pub bytes: u64,
    pub sha256: String,
}

pub(crate) fn inventory(source: &Path, maximum: u64) -> Result<BTreeMap<String, SourceFile>> {
    let parent = source.parent().ok_or_else(|| {
        Failure::new(
            "TDB_INVALID_INPUT",
            "源路径缺少父目录 (Source path has no parent)",
        )
    })?;
    let base = utf8(Path::new(source.file_name().ok_or_else(|| {
        Failure::new(
            "TDB_INVALID_INPUT",
            "源路径缺少名称 (Source path has no name)",
        )
    })?))?;
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    for item in std::fs::read_dir(parent)? {
        let item = item?;
        let name = item.file_name();
        let Some(name) = name.to_str() else { continue };
        if !triviumdb::storage::artifacts::is_reserved_name(name, base) {
            continue;
        }
        let suffix = &name[base.len()..];
        if suffix.eq_ignore_ascii_case(".lock") {
            continue;
        }
        if !item.file_type()?.is_file() {
            return Err(Failure::new(
                "TDB_INVALID_INPUT",
                "来源制品必须是普通文件 (Source artifacts must be regular files)",
            ));
        }
        if files.len() >= 4096 {
            return Err(Failure::new(
                "TDB_MIGRATION_BUDGET",
                "来源制品数量超过 4096 (Source artifact count exceeds 4096)",
            ));
        }
        let mut file = File::open(item.path())?;
        let bytes = file.metadata()?.len();
        total = total.checked_add(bytes).ok_or_else(|| {
            Failure::new(
                "TDB_MIGRATION_BUDGET",
                "来源长度溢出 (Source size overflow)",
            )
        })?;
        if total > maximum {
            return Err(Failure::new(
                "TDB_MIGRATION_BUDGET",
                "来源超过字节预算 (Source exceeds byte budget)",
            ));
        }
        let mut hash = Sha256::new();
        let mut buffer = [0; 65536];
        let mut copied = 0u64;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            copied = copied.checked_add(count as u64).ok_or_else(|| {
                Failure::new(
                    "TDB_MIGRATION_CONFLICT",
                    "来源长度变化 (Source size changed)",
                )
            })?;
            if copied > bytes {
                return Err(Failure::new(
                    "TDB_MIGRATION_CONFLICT",
                    "来源在校验期间增长 (Source grew during validation)",
                ));
            }
            hash.update(&buffer[..count]);
        }
        if copied != bytes {
            return Err(Failure::new(
                "TDB_MIGRATION_CONFLICT",
                "来源在校验期间缩短 (Source shrank during validation)",
            ));
        }
        files.insert(
            suffix.to_owned(),
            SourceFile {
                bytes,
                sha256: format!("{:x}", hash.finalize()),
            },
        );
    }
    if !files.contains_key("") {
        return Err(Failure::new(
            "TDB_CORRUPTED_FILE",
            "来源缺少主检查点 (Source lacks its main checkpoint)",
        ));
    }
    Ok(files)
}

pub(crate) fn identity(source: &Path, requested: Option<u16>) -> Result<(u16, u16, usize)> {
    let mut input = File::open(source)?;
    let mut bytes = [0u8; 74];
    let read_header = |input: &mut File, bytes: &mut [u8]| -> Result<()> {
        input.read_exact(bytes).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                Failure::new(
                    "TDB_CORRUPTED_FILE",
                    "来源文件头被截断 (Source header is truncated)",
                )
            } else {
                error.into()
            }
        })
    };
    read_header(&mut input, &mut bytes[..6])?;
    if &bytes[..4] != b"TVDB" {
        return Err(Failure::new(
            "TDB_CORRUPTED_FILE",
            "来源魔数无效 (Invalid source magic)",
        ));
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if !(5..=15).contains(&version) {
        return Err(Failure::new(
            "TDB_MIGRATION_UNSUPPORTED_SOURCE",
            "迁移工具支持主格式 5 至 15 (Migration tool supports main formats 5 through 15)",
        ));
    }
    read_header(&mut input, &mut bytes[6..58])?;
    let dim = u32::from_le_bytes(bytes[6..10].try_into().map_err(std::io::Error::other)?) as usize;
    if !(1..=65536).contains(&dim) || requested.is_some_and(|dtype| !(1..=3).contains(&dtype)) {
        return Err(Failure::new(
            "TDB_INVALID_INPUT",
            "来源维度或指定类型无效 (Invalid source dimension or requested dtype)",
        ));
    }
    let dtype = if version >= 10 {
        read_header(&mut input, &mut bytes[58..74])?;
        if &bytes[58..62] != b"VTYP"
            || bytes[66..70] != [0; 4]
            || crc32fast::hash(&bytes[..70])
                != u32::from_le_bytes(bytes[70..74].try_into().map_err(std::io::Error::other)?)
        {
            return Err(Failure::new(
                "TDB_CORRUPTED_FILE",
                "来源类型头无效 (Invalid source dtype header)",
            ));
        }
        let dtype = u16::from_le_bytes([bytes[62], bytes[63]]);
        let width = u16::from_le_bytes([bytes[64], bytes[65]]);
        if !matches!((dtype, width), (1, 4) | (2, 2) | (3, 8)) {
            return Err(Failure::new(
                "TDB_CORRUPTED_FILE",
                "来源类型与宽度不一致 (Source dtype and width disagree)",
            ));
        }
        dtype
    } else {
        let slots = u64::from_le_bytes(bytes[18..26].try_into().map_err(std::io::Error::other)?);
        if slots == 0 {
            requested.ok_or_else(|| Failure::new("TDB_MIGRATION_DTYPE_REQUIRED", "旧空检查点需要 --source-dtype f32/f16/u64 (Legacy empty checkpoints require --source-dtype f32/f16/u64)"))?
        } else {
            let offset =
                u64::from_le_bytes(bytes[34..42].try_into().map_err(std::io::Error::other)?);
            let edges =
                u64::from_le_bytes(bytes[42..50].try_into().map_err(std::io::Error::other)?);
            let length = if offset == 0 {
                std::fs::metadata(format!("{}.vec", utf8(source)?))?.len()
            } else {
                edges.checked_sub(offset).ok_or_else(|| {
                    Failure::new(
                        "TDB_CORRUPTED_FILE",
                        "来源向量范围倒置 (Reversed source vector bounds)",
                    )
                })?
            };
            let elements = slots.checked_mul(dim as u64).ok_or_else(|| {
                Failure::new(
                    "TDB_CORRUPTED_FILE",
                    "来源向量几何溢出 (Source vector geometry overflow)",
                )
            })?;
            match length
                .checked_div(elements)
                .filter(|width| elements.checked_mul(*width) == Some(length))
            {
                Some(4) => 1,
                Some(2) => 2,
                Some(8) => 3,
                _ => {
                    return Err(Failure::new(
                        "TDB_CORRUPTED_FILE",
                        "来源向量几何无效 (Invalid source vector geometry)",
                    ));
                }
            }
        }
    };
    if let Some(requested) = requested.filter(|value| *value != dtype) {
        let name = |tag| match tag {
            1 => "f32",
            2 => "f16",
            _ => "u64",
        };
        return Err(triviumdb::TriviumError::DtypeMismatch {
            stored: name(dtype),
            requested: name(requested),
        }
        .into());
    }
    Ok((version, dtype, dim))
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
        return Err(Failure::new(
            "TDB_INVALID_INPUT",
            "迁移归属记录类型或长度无效 (Invalid migration ownership record type or length)",
        ));
    }
    Ok(serde_json::from_reader(File::open(path)?)?)
}

pub(crate) struct Stage {
    root: PathBuf,
    pub database: PathBuf,
    pub source: PathBuf,
    pub facts: PathBuf,
}
impl Stage {
    pub fn prepare(
        parent: &Path,
        leaf: &std::ffi::OsStr,
        owner: &Owner,
        check: bool,
    ) -> Result<Self> {
        let stem = utf8(Path::new(leaf))?;
        let root = if check {
            parent.join(format!(".{stem}.migration-check-{}", uuid::Uuid::new_v4()))
        } else {
            parent.join(format!(".{stem}.migration-stage"))
        };
        let stage = Self {
            database: root.join("database"),
            source: root.join("source"),
            facts: root.join("facts"),
            root,
        };
        if std::fs::symlink_metadata(&stage.root).is_ok() {
            stage.cleanup(owner)?;
        }
        std::fs::create_dir(&stage.root)?;
        write_json(&stage.root.join("owner.json"), owner)?;
        std::fs::create_dir(&stage.database)?;
        std::fs::create_dir(&stage.source)?;
        std::fs::create_dir(&stage.facts)?;
        Ok(stage)
    }
    pub fn cleanup(&self, expected: &Owner) -> Result<()> {
        fn plain_tree(path: &Path) -> Result<()> {
            let meta = std::fs::symlink_metadata(path)?;
            if meta.is_dir() && !meta.file_type().is_symlink() {
                for child in std::fs::read_dir(path)? {
                    plain_tree(&child?.path())?;
                }
                Ok(())
            } else if meta.is_file() {
                Ok(())
            } else {
                Err(Failure::new(
                    "TDB_INVALID_INPUT",
                    "暂存目录包含链接或特殊文件，保留现场 (Staging contains a link or special file; preserved for inspection)",
                ))
            }
        }
        plain_tree(&self.root)?;
        let owner: Owner = read_json(&self.root.join("owner.json"))?;
        if &owner != expected {
            return Err(Failure::new(
                "TDB_MIGRATION_CONFLICT",
                "暂存归属不匹配，保留现场 (Staging ownership mismatch; preserved for inspection)",
            ));
        }
        // 路径由规范父目录和固定目标叶名称生成；删除前校验全部后代均为普通文件或目录。
        std::fs::remove_dir_all(&self.root)?;
        Ok(())
    }
}

pub(crate) fn copy_source(
    source: &Path,
    target: &Path,
    files: &BTreeMap<String, SourceFile>,
) -> Result<()> {
    for (suffix, expected) in files {
        let mut input = File::open(format!("{}{suffix}", utf8(source)?))?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(format!("{}{suffix}", utf8(target)?))?;
        let count = std::io::copy(&mut (&mut input).take(expected.bytes), &mut output)?;
        if count != expected.bytes {
            return Err(Failure::new(
                "TDB_MIGRATION_CONFLICT",
                "来源在复制期间变化 (Source changed while copying)",
            ));
        }
        output.sync_all()?;
    }
    if inventory(target, u64::MAX)? != *files {
        return Err(Failure::new(
            "TDB_MIGRATION_CONFLICT",
            "来源副本摘要不一致 (Copied source digest mismatch)",
        ));
    }
    Ok(())
}

pub(crate) fn retain_current_group(database: &Path) -> Result<()> {
    let marker = match std::fs::read(format!("{}.flush_ok", utf8(database)?)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let payload = if marker.len() == 53 && &marker[..5] == b"TFMK\x03" {
        Some(format!(
            ".pld.{}",
            u64::from_le_bytes(marker[5..13].try_into().map_err(std::io::Error::other)?)
        ))
    } else {
        None
    };
    let base = utf8(Path::new(database.file_name().ok_or_else(|| {
        Failure::new(
            "TDB_INVALID_INPUT",
            "目标缺少名称 (Destination has no name)",
        )
    })?))?;
    let parent = database.parent().ok_or_else(|| {
        Failure::new(
            "TDB_INVALID_INPUT",
            "目标缺少父目录 (Destination has no parent)",
        )
    })?;
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(suffix) = name.to_str().and_then(|s| s.strip_prefix(base)) else {
            return Err(Failure::new(
                "TDB_INVALID_INPUT",
                "暂存目录出现未知文件 (Unexpected file in staging directory)",
            ));
        };
        if !matches!(
            suffix,
            "" | ".vec" | ".idx" | ".wal" | ".lock" | ".flush_ok"
        ) && Some(suffix) != payload.as_deref()
        {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct Publication {
    owner: Owner,
    files: BTreeMap<String, SourceFile>,
}

pub(crate) fn prepare_publication(database: &Path, owner: &Owner) -> Result<()> {
    let files = inventory(database, u64::MAX)?;
    write_json(
        &PathBuf::from(format!("{}.migration", utf8(database)?)),
        &Publication {
            owner: owner.clone(),
            files,
        },
    )
}

/// 调用方已持有目标数据库共享锁，标记移除后写入者仍须等待本次复核结束。
pub(crate) fn finish_publication(database: &Path, owner: &Owner) -> Result<()> {
    let marker = PathBuf::from(format!("{}.migration", utf8(database)?));
    if !marker.try_exists()? {
        return Ok(());
    }
    let record: Publication = read_json(&marker)?;
    let mut files = inventory(database, u64::MAX)?;
    files.remove(".migration");
    if record.owner != *owner || files != record.files {
        return Err(Failure::new(
            "TDB_CORRUPTED_FILE",
            "迁移目标制品与已验证清单不一致，保留发布标记 (Migration artifacts differ from the verified manifest; publication marker retained)",
        ));
    }
    std::fs::remove_file(marker)?;
    triviumdb::database::inspection::sync_directory(database.parent().ok_or_else(|| {
        Failure::new(
            "TDB_INVALID_INPUT",
            "目标缺少父目录 (Destination has no parent)",
        )
    })?)?;
    Ok(())
}
