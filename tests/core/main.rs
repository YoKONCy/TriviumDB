#![allow(non_snake_case)]
//! 按领域聚合的集成测试入口。

mod boundary;
mod console_editing;
mod console_host;
mod database_advanced;
mod document_compressed;
mod document_fields;
mod document_positions;
mod document_retrieval;
mod document_vectors;
mod documents;
mod endian_limit;
mod engine_deep;
mod index_graph_boundaries;
mod index_maintenance;
mod lifecycle_resources;
mod management;
mod named_graphs;
mod proptest_core;
mod public_api_alignment;
mod query_cancellation;
mod query_numeric_indexes;
mod quiver_ssd;
mod quiver_threshold;
mod request_contexts;
mod retrieval_migration;
mod retrieval_validation;
mod runtime_types;
mod search;
mod search_boundaries;
mod shared_sdk;
mod system_boundaries;
mod tql_aliases;
mod tql_boundaries;
mod tql_documents_graphs;
mod tql_semantics;
mod transaction;
mod transaction_state;
mod typed_facade;
mod vector_types;
mod workflow;
mod write_boundaries;
mod write_outcomes;

#[path = "../support/mod.rs"]
mod support;

mod document_ingestion;
