//! Benchmark 共用基础设施。
#![allow(dead_code)]
//!
//! 本模块只负责与被测算法无关的机械工作：环境变量解析、稳定哈希、百分位数、
//! 报告目录创建和 JSON 落盘。把这些逻辑集中在这里，可以避免不同 benchmark
//! 因复制粘贴产生不一致的默认值、路径或百分位定义。
//!
//! 这里刻意不提供数据集生成器。数据分布本身属于 benchmark 契约，必须留在对应
//! 套件中显式说明，避免一次公共生成器修改静默改变多个已发布基线。

use serde::Serialize;
use std::path::{Path, PathBuf};

/// 所有机器可读 benchmark 报告统一写入此目录。
pub const REPORT_DIRECTORY: &str = "target/bench-reports";

/// 读取正整数环境变量；缺失或解析失败时使用公开默认值。
///
/// Benchmark 的环境变量是复现实验的正式接口，因此不因无效输入 panic；最终采用的
/// 参数必须由各套件写入报告，确保结果仍可审计。
pub fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// 返回当前主机可供 benchmark 使用的逻辑线程数。
pub fn available_threads() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
}

/// FNV-1a 风格的稳定混合函数，用于验证不同计划或线程数的结果一致性。
///
/// 该哈希不是密码学摘要，也不用于持久化格式；它只负责低成本发现 benchmark
/// 输出集合、顺序或浮点位模式发生变化。
pub fn mix_hash(hash: u64, value: u64) -> u64 {
    hash.wrapping_mul(0x100000001b3) ^ value
}

/// 计算已排序样本的离散百分位。
///
/// 调用者必须先排序。离散定义可避免插值产生“从未观测到”的延迟值。
pub fn percentile_sorted(samples: &[f64], percentile: usize) -> f64 {
    assert!(!samples.is_empty(), "百分位数至少需要一个样本");
    assert!(percentile <= 100, "百分位必须位于 0..=100");
    let rank = samples
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1);
    samples[rank.min(samples.len() - 1)]
}

/// 构造统一报告路径，并确保父目录存在。
pub fn report_path(file_name: impl AsRef<Path>) -> PathBuf {
    let directory = Path::new(REPORT_DIRECTORY);
    std::fs::create_dir_all(directory).expect("无法创建 benchmark 报告目录");
    directory.join(file_name)
}

/// 以稳定的 pretty JSON 格式写入 benchmark 报告。
///
/// pretty JSON 便于代码审查和人工比较；字段兼容性由各报告的 `schema_version` 管理。
pub fn write_json_report<T: Serialize>(file_name: impl AsRef<Path>, report: &T) -> PathBuf {
    let path = report_path(file_name);
    let bytes = serde_json::to_vec_pretty(report).expect("benchmark 报告无法序列化");
    std::fs::write(&path, bytes).expect("benchmark 报告无法写入");
    path
}
