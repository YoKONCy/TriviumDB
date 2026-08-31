//! 向量、属性与文本索引子系统。
//!
//! 主文件和 MemTable 是权威数据，所有索引均为可重建加速层。各 sidecar 独立版本化，
//! 加载失败按访问模式和 MissingIndexPolicy 决定回退、内存构建或 fail-closed。

pub(crate) mod art;
pub mod bq;
pub mod brute_force;
pub mod exact;
pub mod property;
pub mod quiver;
pub mod text;
