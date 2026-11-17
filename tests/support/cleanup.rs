//! 测试数据库清理复用制品归属目录，覆盖检查点、暂存与发布恢复文件。

pub fn database(path: &str) {
    let path = std::path::Path::new(path);
    let Some(parent) = path.parent() else { return };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let candidate = entry.file_name();
        let Some(candidate) = candidate.to_str() else {
            continue;
        };
        if triviumdb::storage::artifacts::is_reserved_name(candidate, name)
            || candidate == format!("{name}.ready")
        {
            // 仅清理测试传入数据库的同目录文件，不递归进入其他目录。
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
