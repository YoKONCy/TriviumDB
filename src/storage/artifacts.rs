//! 数据库制品的唯一目录。存储发布、快照与宿主路径保护共享名称和归属规则。

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
    let folded = (
        candidate.to_ascii_lowercase(),
        database.to_ascii_lowercase(),
    );
    #[cfg(windows)]
    let (candidate, database) = (folded.0.as_str(), folded.1.as_str());
    if candidate == database {
        return true;
    }
    let Some(suffix) = candidate.strip_prefix(database) else {
        return false;
    };
    RESERVED_SUFFIXES.iter().any(|prefix| {
        suffix == *prefix
            || suffix
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.starts_with('.'))
    }) || suffix.starts_with(".qssd-")
}
