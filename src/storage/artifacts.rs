//! 数据库制品的唯一目录。存储发布、快照与宿主路径保护共享名称和归属规则。

use crate::{Result, TriviumError};
use std::path::{Path, PathBuf};

/// 文件组日志中的顺序属于磁盘协议，已有条目只能在末尾扩展。
pub(crate) const CHECKPOINT_SUFFIXES: [&str; 10] = [
    "",
    ".vec",
    ".flush_ok",
    ".quiver",
    ".quiver.meta",
    ".text",
    ".text.meta",
    ".pidx",
    ".gidx",
    ".idx",
];

/// 统一容器中的组件顺序及其历史独立文件名。
pub(crate) const INDEX_SUFFIXES: [&str; 9] = [
    ".quiver",
    ".quiver.meta",
    ".text",
    ".text.meta",
    ".pidx",
    ".gidx",
    ".qssd",
    ".pelorus.meta",
    ".pelorus",
];

/// 固定制品及其暂存、代际名称族。用于路径保护，不能据此无条件删除文件。
pub const RESERVED_SUFFIXES: &[&str] = &[
    ".vec",
    ".wal",
    ".flush_ok",
    ".lock",
    ".quiver",
    ".quiver.meta",
    ".text",
    ".text.meta",
    ".pidx",
    ".gidx",
    ".idx",
    ".manifest.json",
    ".pld",
    ".publish",
    ".migration",
    ".qssd",
    ".tmp",
];

/// 判断同一目录中的名称是否属于数据库保留名称族。
pub fn is_reserved_name(candidate: &str, database: &str) -> bool {
    #[cfg(windows)]
    let candidate = candidate.trim_end_matches(['.', ' ']);
    if names_equal(candidate, database) {
        return true;
    }
    candidate.match_indices('.').any(|(at, _)| {
        let (stem, suffix) = candidate.split_at(at);
        if !names_equal(stem, database) {
            return false;
        }
        RESERVED_SUFFIXES
            .iter()
            .any(|prefix| names_equal(suffix, prefix))
            || suffix.match_indices('.').skip(1).any(|(end, _)| {
                RESERVED_SUFFIXES
                    .iter()
                    .any(|prefix| names_equal(&suffix[..end], prefix))
            })
            || suffix
                .split_once('-')
                .is_some_and(|(prefix, _)| names_equal(prefix, ".qssd"))
    })
}

pub(crate) fn names_equal(a: &str, b: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
        if a.is_ascii() && b.is_ascii() {
            return a.eq_ignore_ascii_case(b);
        }
        let a = a.encode_utf16().collect::<Vec<_>>();
        let b = b.encode_utf16().collect::<Vec<_>>();
        let (Ok(a_len), Ok(b_len)) = (i32::try_from(a.len()), i32::try_from(b.len())) else {
            return false;
        };
        // SAFETY: 两个 UTF-16 缓冲由本函数持有，长度在 i32 范围内，调用只读且不保留指针。
        unsafe { CompareStringOrdinal(a.as_ptr(), a_len, b.as_ptr(), b_len, 1) == CSTR_EQUAL }
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

/// 核对目标和源库制品的名称与文件身份，返回可供发布使用的绝对目标路径。
pub fn validate_destination(source: &Path, destination: &Path) -> Result<PathBuf> {
    let source = source.canonicalize()?;
    let source_parent = parent(&source);
    let database = name(&source)?;
    let requested = resolve_missing(parent(destination))?.join(name(destination)?);
    let target = resolve_missing(&requested)?;
    let conflict = || {
        TriviumError::InvalidInput(format!(
            "目标与源库制品冲突 (Destination conflicts with a source database artifact): {}",
            destination.display()
        ))
        .at_field("destination")
    };
    for path in [&requested, &target] {
        for candidate in path.ancestors() {
            if let Some(leaf) = candidate.file_name().and_then(|name| name.to_str())
                && is_reserved_name(leaf, database)
                && let Some(directory) = candidate.parent()
                && (directory == source_parent
                    || directory.try_exists()?
                        && same_file::is_same_file(directory, source_parent)?)
            {
                return Err(conflict());
            }
        }
    }
    // 已存在的导出目标可能是源库制品的硬链接，而且可能位于其他目录。
    if target.try_exists()? {
        for entry in std::fs::read_dir(source_parent)? {
            let entry = entry?;
            if is_reserved_name(&entry.file_name().to_string_lossy(), database)
                && entry.path().try_exists()?
                && same_file::is_same_file(&target, entry.path())?
            {
                return Err(conflict());
            }
        }
    }
    Ok(target)
}

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn name(path: &Path) -> Result<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            TriviumError::InvalidInput(
                "路径须包含 UTF-8 文件名 (Path must include a UTF-8 file name)".into(),
            )
        })
}

fn resolve_missing(path: &Path) -> Result<PathBuf> {
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        match ancestor.canonicalize() {
            Ok(mut resolved) => {
                resolved.extend(missing.iter().rev());
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // 悬空链接必须拒绝，不能当作尚未创建的路径继续拼接。
                if ancestor.symlink_metadata().is_ok() {
                    return Err(error.into());
                }
                missing.push(name(ancestor)?);
                ancestor = parent(ancestor);
            }
            Err(error) => return Err(error.into()),
        }
    }
}
