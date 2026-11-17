#![allow(non_snake_case)]
//! TriviumDB 单元测试统一入口
//!
//! 所有外部单元测试按模块组织在此目录下。
//! 每个子模块对应一个源码模块的点到点测试。

mod cognitive;
mod core;
mod database;
mod filter;
mod index;
mod memtable;
mod tql_ast;
mod traversal;
mod vector;
mod wal;

#[path = "../support/cleanup.rs"]
mod cleanup_files;
