//! Pelorus 原生 ANN 算法与编码。
//!
//! 支持 BQ2 余弦检索，以及精确距离和几何感知 BQ2 两种原生双曲路径。

#![allow(clippy::needless_range_loop)]
#![allow(clippy::unnecessary_sort_by)]
#![allow(dead_code, unexpected_cfgs)]

mod config;
mod geometry;
mod graph;
mod index;
pub(super) mod metric;
mod quantization;
mod storage;
mod vector;

pub use config::{BuildConfig, SearchConfig};
pub use geometry::PoincareNavigation;
pub(super) use index::validate_build_config;
pub use index::{Error, Index, Result};
pub use metric::Metric;
pub use storage::{
    RerankConfig, RerankPrecision, RerankRepresentation, ScalarQuantization, StorageMode,
    VectorStorageStats,
};
pub(crate) use vector::VectorType;
