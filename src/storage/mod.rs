//! 持久化、WAL、mmap、代际发布与 sidecar 存储子系统。
//!
//! 本层实现数据格式和崩溃一致性，不包含查询语义。所有可写发布遵循写临时文件、fsync、
//! 原子替换；ReadOnly/Immutable 路径禁止修复、补建或创建任何制品。

pub mod compaction;
pub mod file_format;
pub(crate) mod fs;
pub mod generation;
pub mod graph_blocks;
pub mod memtable;
pub(crate) mod payload_store;
pub mod snapshot;
pub mod vec_pool;
pub mod wal;
