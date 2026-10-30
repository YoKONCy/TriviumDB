#![allow(non_snake_case)]
//! 按领域聚合的集成测试入口。

mod boundary;
mod console_host;
mod database_advanced;
mod endian_limit;
mod engine_deep;
mod index_graph_boundaries;
mod lifecycle_resources;
mod proptest_core;
mod public_api_alignment;
mod query_numeric_indexes;
mod quiver_threshold;
mod retrieval_migration;
mod retrieval_validation;
mod runtime_types;
mod search;
mod search_boundaries;
mod system_boundaries;
mod tql_aliases;
mod tql_boundaries;
mod tql_semantics;
mod transaction;
mod transaction_state;
mod typed_facade;
mod vector_types;
mod workflow;
mod write_boundaries;

#[path = "../support/mod.rs"]
mod support;
