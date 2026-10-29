#![allow(non_snake_case)]
//! 按领域聚合的集成测试入口。
#[cfg(feature = "test-hooks")]
#[path = "../support/mod.rs"]
mod support;

#[cfg(feature = "test-hooks")]
mod deterministic_failpoint;
mod io_fault;
#[cfg(feature = "test-hooks")]
mod retained_publication;
mod sector_tearing;
#[cfg(feature = "test-hooks")]
mod text_publication;
mod wal_midwrite;

#[cfg(feature = "test-hooks")]
mod snapshot_shutdown;

#[cfg(feature = "test-hooks")]
mod detach_batch;
#[cfg(feature = "test-hooks")]
mod lifecycle_resources;
#[cfg(feature = "test-hooks")]
mod migration_publication;
#[cfg(feature = "test-hooks")]
mod schema_publication;
#[cfg(feature = "test-hooks")]
mod wal_failure_state;
