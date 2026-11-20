//! Poincaré 度量及 Pelorus 派生索引。
//!
//! 算法、量化和评分来自 Pelorus 3d18859abcb1dd938afac3825c50dce0b93959ba。
//! 存储内核负责事实、代际、预算、谓词和资源生命周期。

mod engine;
mod io;
pub(crate) mod persistence;
mod search;
mod wire;

pub use engine::{
    BuildConfig, PoincareNavigation, RerankConfig, RerankPrecision, RerankRepresentation,
    ScalarQuantization,
};
pub use io::{PelorusResidency, PelorusRuntimeOptions};
pub(crate) use search::{SearchWork, distance, search, search_bounded};
pub(crate) use wire::{WireSearch, WireSearchOptions, convert_query};

use crate::{NodeId, Result, TriviumError, VectorType};
use serde::{Deserialize, Serialize};

const MAX_CHANGED_NODES: usize = 65_536;

/// Pelorus Poincaré 图的构建配置。曲率绝对值为 `curvature`。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PelorusConfig {
    pub curvature: f64,
    pub navigation: PoincareNavigation,
    pub m: usize,
    pub ef_construction: usize,
    pub alpha: f32,
    pub rerank: RerankConfig,
}

impl Default for PelorusConfig {
    fn default() -> Self {
        let build = BuildConfig::default();
        Self {
            curvature: 1.0,
            navigation: PoincareNavigation::GeometryBq2,
            m: build.m,
            ef_construction: build.ef_construction,
            alpha: build.alpha,
            rerank: RerankConfig::default(),
        }
    }
}

impl PelorusConfig {
    pub(crate) fn validate(self, dim: usize) -> Result<()> {
        let build = self.build_config();
        engine::validate_build_config(dim, &build).map_err(map_error)?;
        engine::Metric::Poincare {
            curvature: self.curvature,
        }
        .validate()
        .map_err(|error| map_error(error).at_field("curvature"))?;
        self.rerank
            .validate()
            .map_err(|error| map_error(error).at_field("rerank"))
    }

    fn build_config(self) -> BuildConfig {
        BuildConfig {
            m: self.m,
            ef_construction: self.ef_construction,
            alpha: self.alpha,
        }
    }
}

/// 只用于显式请求的 Poincaré 向量检索。
#[derive(Debug, Clone)]
pub struct PelorusSearchOptions {
    pub top_k: usize,
    pub ef_search: Option<usize>,
    pub rerank_limit: Option<usize>,
    pub max_distance: Option<f64>,
    pub exact: bool,
    pub payload_filter: Option<crate::Filter>,
}

impl Default for PelorusSearchOptions {
    fn default() -> Self {
        Self {
            top_k: 10,
            ef_search: None,
            rerank_limit: None,
            max_distance: None,
            exact: false,
            payload_filter: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PelorusHit {
    pub id: NodeId,
    pub distance: f64,
    pub payload: serde_json::Value,
}

pub(crate) struct PelorusState {
    pub config: PelorusConfig,
    pub index: Option<engine::Index>,
    ssd: Option<io::PelorusSsd>,
    pub(crate) changed: std::collections::HashSet<NodeId>,
}

impl PelorusState {
    pub fn build(
        config: PelorusConfig,
        dim: usize,
        ids: &[NodeId],
        vectors: &[f32],
    ) -> Result<Self> {
        let mut index = engine::Index::build_poincare(
            dim,
            ids,
            vectors,
            &config.build_config(),
            config.curvature,
            config.navigation,
        )
        .map_err(map_error)?;
        index
            .set_hot_rerank_config(config.rerank)
            .map_err(map_error)?;
        Ok(Self {
            config,
            index: Some(index),
            ssd: None,
            changed: Default::default(),
        })
    }

    pub fn heap_bytes(&self) -> usize {
        self.index.as_ref().map_or(0, |index| {
            let storage = index.vector_storage_stats();
            index
                .navigation_storage_bytes()
                .saturating_add(index.len().saturating_mul(112))
                .saturating_add(storage.canonical_heap_bytes)
                .saturating_add(storage.rerank_heap_bytes)
                .saturating_add(storage.shared_metadata_bytes)
                .saturating_add(storage.rerank_metadata_bytes)
                .saturating_add(self.changed.capacity().saturating_mul(16))
        })
    }

    pub fn reserved_io_bytes(&self) -> usize {
        self.ssd.as_ref().map_or(0, io::PelorusSsd::reserved_bytes)
    }

    pub(crate) fn discard_index(&mut self) {
        self.ssd.take();
        self.index.take();
        self.changed = Default::default();
    }

    pub(crate) fn record_change(&mut self, id: NodeId) {
        let Some(index) = &self.index else {
            return;
        };
        if self.changed.contains(&id) {
            return;
        }
        let limit = index.len().div_ceil(4).clamp(64, MAX_CHANGED_NODES);
        if self.changed.len() >= limit || self.changed.try_reserve(1).is_err() {
            self.discard_index();
        } else {
            self.changed.insert(id);
        }
    }
}

pub(crate) fn validate_dtype<T: VectorType>() -> Result<()> {
    if std::any::TypeId::of::<T>() == std::any::TypeId::of::<f32>()
        || std::any::TypeId::of::<T>() == std::any::TypeId::of::<half::f16>()
    {
        return Ok(());
    }
    Err(TriviumError::InvalidInput(
        "Pelorus 支持 f32 和 f16 向量 (Pelorus supports f32 and f16 vectors)".into(),
    )
    .at_field("dtype"))
}

#[inline]
pub(crate) fn boundary_factor<T: VectorType>(curvature: f64, vector: &[T]) -> Result<f64> {
    if std::any::TypeId::of::<T>() == std::any::TypeId::of::<f32>() {
        return engine::Metric::Poincare { curvature }
            .boundary_factor(bytemuck::cast_slice(vector), 0)
            .map_err(map_error);
    }
    let norm_squared = vector
        .iter()
        .map(|x| f64::from(x.to_f32()).powi(2))
        .sum::<f64>();
    let boundary = 1.0 - curvature * norm_squared;
    if boundary.is_finite() && boundary > 0.0 {
        Ok(boundary.recip())
    } else {
        Err(map_error(engine::Error::OutsideManifold { index: 0 }))
    }
}

pub(crate) fn map_error(error: engine::Error) -> TriviumError {
    use engine::Error;
    match error {
        Error::OutsideManifold { .. } => TriviumError::InvalidVector {
            reason: "向量必须位于开 Poincaré 球内 (Vector must lie inside the open Poincaré ball)"
                .into(),
        },
        Error::NonFiniteVector { .. } | Error::VectorLength { .. } => TriviumError::InvalidVector {
            reason: format!("Pelorus 向量无效 (Invalid Pelorus vector): {error}"),
        },
        Error::CapacityExceeded => TriviumError::CapacityAllocationFailed {
            reason: "Pelorus 索引容量超出上限 (Pelorus index capacity exceeded)".into(),
        },
        Error::Io { kind, message } => std::io::Error::new(kind, message).into(),
        Error::Host(error) => std::sync::Arc::try_unwrap(error)
            .unwrap_or_else(|error| std::io::Error::other(error.to_string()).into()),
        Error::CorruptIndex(_) | Error::UnsupportedVersion(_) => TriviumError::CorruptedFile(
            format!("Pelorus 索引无效 (Invalid Pelorus index): {error}"),
        ),
        _ => TriviumError::InvalidInput(format!(
            "Pelorus 操作失败 (Pelorus operation failed): {error}"
        )),
    }
}
