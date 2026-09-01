#![allow(non_snake_case)]
//! 按领域聚合的集成测试入口。

#[cfg(feature = "test-hooks")]
mod deterministic_failpoint;
mod io_fault;
mod sector_tearing;
mod wal_midwrite;
