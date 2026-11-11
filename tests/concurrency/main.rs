#![allow(non_snake_case)]
//! 按领域聚合的集成测试入口。

mod concurrency_invariants;
mod concurrent;
#[cfg(feature = "test-hooks")]
mod console_cancellation;
mod deterministic;
mod deterministic_interleaving;
mod opening;
mod resources;

#[cfg(feature = "test-hooks")]
#[path = "../support/mod.rs"]
mod support;
