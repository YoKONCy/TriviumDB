//! TriviumDB 嵌入式三模数据库核心库。
//!
//! 一个稳定 NodeId 同时关联向量、JSON Payload 与带标签业务图；Database 提供单写多读
//! 存储，TQL 提供可自由编排的混合查询，QuIVer/属性/文本/图索引负责加速。所有公共
//! 路径共享确定性、预算 fail-closed、版本化磁盘格式和 ReadOnly/Immutable 零写约束。

#![allow(clippy::too_many_arguments)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::suspicious_open_options)]
#![allow(clippy::unnecessary_sort_by)]

pub mod database;
pub mod error;
pub mod filter;
pub mod graph;
pub mod hook;
pub mod index;
pub mod node;
pub mod observability;
pub mod query;
pub mod storage;
pub mod tsng;

#[cfg(feature = "test-hooks")]
pub mod test_hooks;

pub mod cognitive;

/// FFI 绑定层（Python / Node.js）
pub mod bindings;

pub use database::{BatchSearchConfig, EdgeDirection, SearchConfig};
pub use database::{Database, DatabaseReader, DatabaseWriter};
pub use error::{Result, TriviumError};
pub use filter::Filter;
pub use graph::reachability::{
    ReachabilityConfig, ReachabilityDirection, ReachabilityOutput, ReachabilityResult,
    ReachabilityStep, SubgraphEdge, SubgraphNode, SubgraphResult,
};
pub use hook::{CompositeHook, FfiHook, HookContext, NoopHook, SearchHook};
pub use node::{Edge, GroupedSearchResult, NodeId, NodeView, SearchHit};
pub use storage::generation::{CurrentGeneration, GenerationReader, GenerationStore};
pub use storage::memtable::{GraphIntegrityReport, GraphRepairReport, GraphStats};
pub use tsng::{
    BeamAdaptation, GraphSignalQuery, IndustrialAccessPath, IndustrialSearchConfig,
    QueryMemoryBudget, TsngBudget, TsngCost, TsngGroundTruth, TsngHit, TsngQualityMetrics,
    TsngQuery, TsngSearchConfig, TsngSearchMetrics, TsngSearchResult, TsngWeights, quality_metrics,
};
pub mod vector;
pub use vector::VectorType;

// PyO3 模块入口：当 maturin 构建 cdylib 时，Python import 会调用此处
#[cfg(feature = "python")]
pub use bindings::python::python::triviumdb;
