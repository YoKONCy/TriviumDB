#![allow(non_snake_case)]
//! 按领域聚合的集成测试入口。
#[path = "../support/mod.rs"]
mod support;

mod compaction_core;
mod format_migration;
mod graph_block_persistence;
mod immutable;
mod ordered_property_index;
mod property_index_persistence;
mod publication_recovery;
mod quiver_facts;
mod read_only;
mod recovery;
mod sidecar_facts;
mod snapshot;
mod text_index_lifecycle;
mod vector_memory;
mod wal_integrity;
mod wal_semantics;

mod index_schema_boundaries;
mod kernel_boundaries;
