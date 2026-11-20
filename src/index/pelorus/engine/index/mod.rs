//! 支持内存与 SSD 冷向量的独立索引封装。

use crate::index::pelorus::engine::geometry::PoincareNavigation;
use crate::index::pelorus::engine::graph::NativeSpace;
use crate::index::pelorus::engine::graph::QuIVer;
use crate::index::pelorus::engine::metric::Metric;
use crate::index::pelorus::engine::storage::{Buffer, ColdStore};
use crate::index::pelorus::engine::{BuildConfig, SearchConfig};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
const NEXT_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// 自带原始向量存储、可直接构建、维护和持久化的 QuIVer 索引。
pub struct Index {
    graph: QuIVer,
    vectors: Buffer<f32>,
    rerank: Option<ColdStore>,
    storage_mode: crate::index::pelorus::engine::StorageMode,
    ids: Vec<u64>,
    active: Vec<bool>,
    id_to_slot: HashMap<u64, usize>,
    dimensions: usize,
    config: BuildConfig,
    next_seed: u64,
    metric: Metric,
    boundary_factors: Vec<f64>,
    navigation: PoincareNavigation,
}
impl Index {
    pub fn build(
        dimensions: usize,
        ids: &[u64],
        vectors: &[f32],
        config: &BuildConfig,
    ) -> Result<Self> {
        Self::build_with_metric(dimensions, ids, vectors, config, Metric::Cosine)
    }

    /// 使用精确双曲距离构图和导航，或使用原有余弦路径。
    pub fn build_with_metric(
        dimensions: usize,
        ids: &[u64],
        vectors: &[f32],
        config: &BuildConfig,
        metric: Metric,
    ) -> Result<Self> {
        Self::build_impl(
            dimensions,
            ids,
            vectors,
            config,
            metric,
            PoincareNavigation::Exact,
        )
    }

    /// 显式选择双曲构图、剪枝和导航表示，默认以原始 FP32 坐标精排。
    pub fn build_poincare(
        dimensions: usize,
        ids: &[u64],
        vectors: &[f32],
        config: &BuildConfig,
        curvature: f64,
        navigation: PoincareNavigation,
    ) -> Result<Self> {
        Self::build_impl(
            dimensions,
            ids,
            vectors,
            config,
            Metric::Poincare { curvature },
            navigation,
        )
    }

    fn build_impl(
        dimensions: usize,
        ids: &[u64],
        vectors: &[f32],
        config: &BuildConfig,
        metric: Metric,
        navigation: PoincareNavigation,
    ) -> Result<Self> {
        Self::build_impl_with_options(dimensions, ids, vectors, config, metric, navigation, false)
    }

    fn build_impl_with_options(
        dimensions: usize,
        ids: &[u64],
        vectors: &[f32],
        config: &BuildConfig,
        metric: Metric,
        navigation: PoincareNavigation,
        force_residual: bool,
    ) -> Result<Self> {
        validate_build_config(dimensions, config)?;
        metric.validate()?;
        let count = validate_dataset(dimensions, ids, vectors)?;
        let boundary_factors = if matches!(metric, Metric::Poincare { .. }) {
            // 百万级宽向量下并行计算庞加莱边界因子，发生流形越界时仍确定性返回最小越界下标
            if count >= 2048 {
                let mut factors = vec![0.0f64; count];
                let first_err = std::sync::Mutex::new(None::<(usize, Error)>);
                factors.par_iter_mut().enumerate().for_each(|(i, out)| {
                    let v = &vectors[i * dimensions..(i + 1) * dimensions];
                    match metric.boundary_factor(v, i) {
                        Ok(f) => *out = f,
                        Err(e) => {
                            let mut guard = first_err.lock().unwrap();
                            if guard.as_ref().is_none_or(|(min_i, _)| i < *min_i) {
                                *guard = Some((i, e));
                            }
                        }
                    }
                });
                if let Some((_, err)) = first_err.into_inner().unwrap() {
                    return Err(err);
                }
                factors
            } else {
                vectors
                    .chunks_exact(dimensions)
                    .enumerate()
                    .map(|(i, v)| metric.boundary_factor(v, i))
                    .collect::<Result<Vec<_>>>()?
            }
        } else {
            Vec::new()
        };
        let mut graph = QuIVer::new(dimensions, config);
        let next_seed = NEXT_SEED;
        match metric {
            Metric::Cosine => {
                let slots = (0..count).collect::<Vec<_>>();
                graph.build_parallel(vectors, ids, &slots);
            }
            Metric::Poincare { curvature } => {
                let space = NativeSpace {
                    vectors,
                    delta: &[],
                    rerank: None,
                    boundary_factors: &boundary_factors,
                    dimensions,
                    curvature,
                    meter: None,
                };
                let slots = (0..count).collect::<Vec<_>>();
                graph.batch_build_native_with_residual(
                    &space,
                    ids,
                    &slots,
                    navigation == PoincareNavigation::GeometryBq2,
                    force_residual,
                );
            }
        }
        Ok(Self {
            graph,
            vectors: Buffer::owned(vectors.to_vec()),
            rerank: None,
            storage_mode: crate::index::pelorus::engine::StorageMode::Memory,
            ids: ids.to_vec(),
            active: vec![true; count],
            id_to_slot: ids
                .iter()
                .enumerate()
                .map(|(slot, &id)| (id, slot))
                .collect(),
            dimensions,
            config: *config,
            next_seed,
            metric,
            boundary_factors,
            navigation,
        })
    }

    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    pub fn len(&self) -> usize {
        self.id_to_slot.len()
    }

    pub fn is_empty(&self) -> bool {
        self.id_to_slot.is_empty()
    }

    pub fn contains(&self, id: u64) -> bool {
        self.id_to_slot.contains_key(&id)
    }

    pub fn ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.ids
            .iter()
            .zip(&self.active)
            .filter_map(|(&id, &active)| active.then_some(id))
    }

    pub fn vector(&self, id: u64) -> Option<&[f32]> {
        let &slot = self.id_to_slot.get(&id)?;
        Some(self.vectors.row(slot, self.dimensions))
    }

    pub fn config(&self) -> BuildConfig {
        self.config
    }

    pub fn metric(&self) -> Metric {
        self.metric
    }

    pub fn needs_rebuild(&self) -> bool {
        self.graph.needs_rebuild()
    }
}

mod snapshot;
mod storage;

mod error;
pub use error::{Error, Result};

mod search;

mod diagnostics;

mod validation;
pub(in crate::index::pelorus) use validation::validate_build_config;
use validation::*;
