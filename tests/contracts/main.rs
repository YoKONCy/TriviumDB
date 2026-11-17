#![allow(non_snake_case)]
//! 按领域聚合的集成测试入口。

mod errors;
mod ffi_hook;
mod hook_and_search;
mod lifecycle;
mod public_matrix;
mod resources;
mod shared_cases;

#[path = "../support/mod.rs"]
mod support;

#[path = "../support/cleanup.rs"]
mod cleanup_files;
