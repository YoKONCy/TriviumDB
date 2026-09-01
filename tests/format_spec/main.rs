#![allow(non_snake_case)]
//! 磁盘格式规格测试入口。规格模型独立于生产解析器，用字段级变异验证 loader 的边界与完整性契约。

mod cross_generation;
mod fixture;
mod mutation;
mod replay;
mod sidecar_spec;
mod snapshot_spec;
mod spec;
mod wal_spec;
