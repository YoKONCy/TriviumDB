//! 通用辅助函数。

/// 读取文件大小（字节），文件不存在时返回 `None`。
pub fn file_size(path: &str) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// 把字节数格式化为人类可读字符串（B / KB / MB / GB ...）。
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

/// 把可选文件大小格式化为人类可读字符串，缺失时显示 `—`。
pub fn human_bytes_opt(n: Option<u64>) -> String {
    match n {
        Some(b) => human_bytes(b),
        None => "—".to_string(),
    }
}
