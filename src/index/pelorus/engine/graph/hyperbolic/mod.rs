//! 原生双曲图构建、极坐标导航与精确测地距离精排。
//! 原生黎曼双曲（Poincaré 球流形 B^d_c = R_{>=0} ×_{sinh(sqrt(c)R)} S^{d-1}）构图与导航子系统。
//!
//! 第四代黎曼 QuIVer（A8: Adaptive-Shell & Stage-0.5 De-biased Asymmetric SM-Poincaré）核心组件：
//! 1. 两级符号-幅值（Sign-Magnitude, SM）球面重构算子与 3-to-2 CSA 加速位核；
//! 2. 分布自适应分位数极坐标锥-壳路由图谱（PolarRouter + 32B PolarNodeMeta）与测地凸支撑投影顶点 d* = clamp(atanh((tanh(d_q)cos)_+), d_min, d_max) 选种；
//! 3. 主循环 5-popcount 对称 SM-Poincaré 高效遍历 + Stage-0.5 非对称校准过滤器有界精筛（开销和截断损失由实验测量）；
//! 4. 带舍入保护的径向 Cauchy-Schwarz 下界预滤与跨页流式冷存预取；
//! 5. 深度与壳厚自适应四向黎曼罗盘剪枝（4-Way Adaptive Riemannian Compass）。

use super::{Bq2Signature, Bq2Store, EMPTY_NB, QuIVer, SearchConfig, SparseBitset};
use crate::index::pelorus::engine::geometry::{
    GeometryCode, calibrate_bq2_cosine, sm_calibrated_cosine_fast,
};
use crate::index::pelorus::engine::metric::{norm_squared_f64, poincare_distance, poincare_proxy};
use crate::index::pelorus::engine::quantization::{AsymQuerySm, SmCodeParams};
use rayon::prelude::*;
use std::cell::RefCell;
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::sync::Arc;
#[cfg(feature = "research")]
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU64, Ordering as AtomicOrdering};
mod settings;
pub(crate) use settings::*;

mod metadata;
pub(crate) use metadata::*;

mod sketch;
pub(crate) use sketch::*;

mod storage;
pub(crate) use storage::*;

mod residual;
pub(crate) use residual::*;

mod routing;
pub(crate) use routing::*;

mod space;
pub(crate) use space::*;

mod scratch;
pub(crate) use scratch::*;

mod representation;

mod search;

mod beam;

mod construction;

mod diagnostics;

#[cfg(feature = "research")]
pub use settings::{
    ExecutionConfig, GeometryComponents, HyperbolicStrategy, RecallConfig, SearchLossStats,
    SearchStageProfile, StorageConfig, benchmark_sign_magnitude_kernels,
    benchmark_sign_magnitude_workload, execution_config, hyperbolic_strategy, recall_config,
    set_execution_config, set_geometry_components, set_hyperbolic_strategy, set_recall_config,
    set_storage_config, storage_config,
};
pub use settings::{SketchFilter, TraversalProfile};
