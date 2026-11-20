//! QuIVer — QUantized Index for Vector Retrieval
//!
//! 量化图索引，支持余弦与原生双曲几何。
//! 使用 Rayon 线程池并行构建。
//!
//! 核心架构：
//!   - 2-bit Sign-Magnitude 量化 (pos + strong)
//!   - Vamana α-diversity 剪枝构图
//!   - Symmetric 寻路 (XOR+Popcount) + 可选精度的冷向量精排
//!
//! 设计要点：
//!   - Bitset visited: 可复用位向量，2.5KB/20K 节点
//!   - Flat 邻接表: 连续内存，固定步长
//!   - BQ 签名连续存储: SoA 布局，预取友好

use crate::index::pelorus::engine::geometry::GeometryCode;
use crate::index::pelorus::engine::quantization::{Bq2Signature, Bq2Store};
use crate::index::pelorus::engine::vector::{
    cosine_query_norm_f32_384, cosine_similarity_f32, cosine_similarity_f32_384_with_query_norm,
};
use rayon::prelude::*;
use std::cell::UnsafeCell;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

mod hyperbolic;
mod snapshot;
pub(crate) use hyperbolic::HotSmStore;
pub(crate) use hyperbolic::NativeSpace;
#[cfg(feature = "research")]
mod diagnostics;
#[cfg(feature = "research")]
pub use diagnostics::{GraphDiagnostics, NodeDiagnostics};
#[cfg(feature = "research")]
pub use hyperbolic::{
    ExecutionConfig, GeometryComponents, HyperbolicStrategy, RecallConfig, SearchLossStats,
    SearchStageProfile, SketchFilter, StorageConfig, TraversalProfile,
    benchmark_sign_magnitude_kernels, benchmark_sign_magnitude_workload, execution_config,
    hyperbolic_strategy, recall_config, set_execution_config, set_geometry_components,
    set_hyperbolic_strategy, set_recall_config, set_storage_config, storage_config,
};

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct NonNanF32(pub f32);

impl Eq for NonNanF32 {}
impl PartialOrd for NonNanF32 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for NonNanF32 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .partial_cmp(&other.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// 查询时导航评分器。评分越小，候选越优先；该抽象不参与 QuIVer 建图或裁边。
pub trait NavigationScorer {
    fn score(&mut self, node_id: u64, bq_distance: u32) -> u32;

    fn accept_result(&mut self, _node_id: u64) -> bool {
        true
    }
}

/// 与历史 QuIVer 查询完全等价的纯 BQ 导航评分器。
#[derive(Debug, Default, Clone, Copy)]
pub struct BqNavigationScorer;

impl NavigationScorer for BqNavigationScorer {
    #[inline(always)]
    fn score(&mut self, _node_id: u64, bq_distance: u32) -> u32 {
        bq_distance
    }
}

pub struct QuIVer {
    dim: usize,
    n: usize,
    m: usize,
    m0: usize,
    ef_construction: usize,
    ml: f64,
    alpha: f32,

    // Hot（常驻内存）
    bq_store: Bq2Store,
    pub(crate) quantized_native: bool,
    geometry_codes: Vec<GeometryCode>,
    polar_router: Option<hyperbolic::PolarRouter>,
    #[cfg(feature = "research")]
    direction_store: Option<crate::index::pelorus::engine::direction::DirectionStore>,
    layer0: FlatAdj,
    upper_layers: Vec<Vec<Vec<u32>>>,
    node_max_layer: Vec<u8>,

    // ID 映射
    ids: Vec<u64>,                                       // internal_index → NodeId
    slot_indices: Vec<usize>, // internal_index → MemTable slot_index（精排用）
    id_to_internal: std::collections::HashMap<u64, u32>, // NodeId → internal_index（增量操作用）

    entry_point: u32,
    max_level: usize,

    // 增量管理
    tombstones: Vec<bool>, // internal_index → 是否已删除
    dirty_count: usize,    // 增量变更计数（tombstone + 追加节点）

    // 当前 QuIVer 实例范围内的属性 posting → internal slot 派生缓存。
    // QuIVer 重建会替换整个实例；增量变更会清空缓存，不参与持久化或建图。
    prefilter_slot_cache: PrefilterSlotCache,

    visited: Bitset, // 建图期间复用，搜索时每次新建
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrefilterMetrics {
    pub posting_lookup_ns: u64,
    pub node_mapping_ns: u64,
    pub bq_heap_scan_ns: u64,
    pub output_sort_ns: u64,
    pub cache_hit: bool,
    pub mapped_candidates: usize,
}

fn elapsed_ns(started: Instant) -> u64 {
    started.elapsed().as_nanos().min(u64::MAX as u128) as u64
}
impl QuIVer {
    pub fn new(dim: usize, config: &BuildConfig) -> Self {
        assert!(dim > 0, "QuIVer 维度必须大于零");
        let m = config.m;
        let m0 = m * 2;
        Self {
            dim,
            n: 0,
            m,
            m0,
            ef_construction: config.ef_construction,
            ml: 1.0 / (m as f64).ln(),
            alpha: config.alpha,
            bq_store: Bq2Store::new(dim),
            quantized_native: false,
            geometry_codes: Vec::new(),
            #[cfg(feature = "research")]
            direction_store: None,
            polar_router: None,
            layer0: FlatAdj::new(m0 * 2 + 1),
            upper_layers: Vec::new(),
            node_max_layer: Vec::new(),
            ids: Vec::new(),
            slot_indices: Vec::new(),
            id_to_internal: std::collections::HashMap::new(),
            entry_point: 0,
            max_level: 0,
            tombstones: Vec::new(),
            dirty_count: 0,
            prefilter_slot_cache: Mutex::new(PrefilterSlotCacheState::default()),
            visited: Bitset::new(0),
        }
    }

    fn random_level(&self, lcg: &mut u64) -> usize {
        *lcg = lcg
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r = ((*lcg >> 33) as f64 / (1u64 << 31) as f64).max(1e-15);
        (-r.ln() * self.ml).floor() as usize
    }

    pub fn insert(&mut self, vector: &[f32], id: u64, slot_index: usize, lcg: &mut u64) {
        assert_eq!(vector.len(), self.dim);
        let idx = self.n as u32;

        let sig = Bq2Signature::from_vector(vector);
        self.bq_store.push_sig(&sig);
        self.ids.push(id);
        self.slot_indices.push(slot_index);
        self.id_to_internal.insert(id, idx);
        self.tombstones.push(false);

        let level = self.random_level(lcg);
        self.node_max_layer.push(level as u8);

        // 扩展 layer0
        self.layer0.push_empty();

        // 扩展 upper layers
        while self.upper_layers.len() < level {
            // upper_layers[0] = layer 1, [1] = layer 2, ...
            self.upper_layers.push(vec![Vec::new(); self.n]);
        }
        for ul in self.upper_layers.iter_mut() {
            ul.push(Vec::new());
        }

        self.n += 1;

        // 确保 visited bitset 够大
        self.visited.grow(self.n);

        if self.n == 1 {
            self.entry_point = 0;
            self.max_level = level;
            return;
        }

        let my_sig = self.bq_store.get_sig(idx as usize);
        let mut cur_node = self.entry_point;

        // ── 高层贪心下降（BQ 距离，不需要 f32 向量） ──
        for l in ((level + 1)..=self.max_level).rev() {
            let ul_idx = l - 1;
            if ul_idx < self.upper_layers.len() {
                loop {
                    let mut changed = false;
                    let cur_d = self
                        .bq_store
                        .distance(idx as usize, cur_node as usize, self.dim);
                    let mut best_d = cur_d;

                    for &nb in &self.upper_layers[ul_idx][cur_node as usize] {
                        let nd = self.bq_store.distance(idx as usize, nb as usize, self.dim);
                        if nd < best_d {
                            cur_node = nb;
                            best_d = nd;
                            changed = true;
                        }
                    }
                    if !changed {
                        break;
                    }
                }
            }
        }

        // 3. 高层搜索
        let mut visited = Bitset::new(self.n);
        for l in (level..=self.max_level).rev() {
            let res = self.beam_search_upper(&my_sig, l, cur_node, 1, &mut visited);
            if !res.is_empty() {
                cur_node = res[0].1;
            }
        }

        // 4. 新节点插入各层
        for l in (0..=level).rev() {
            let ef = self.ef_construction;
            let candidates_sym = if l == 0 {
                self.beam_search_l0(&my_sig, cur_node, ef, &mut visited)
            } else {
                self.beam_search_upper(&my_sig, l, cur_node, ef, &mut visited)
            };
            let candidates: Vec<(u32, u32)> = candidates_sym
                .into_iter()
                .map(|(_, id)| {
                    (
                        self.bq_store.distance(idx as usize, id as usize, self.dim),
                        id,
                    )
                })
                .collect();

            let max_nb = if l == 0 { self.m0 } else { self.m };
            let selected = self.select_neighbors(idx, &candidates, max_nb);

            if l == 0 {
                // 正向边：idx → selected
                for &nb in &selected {
                    if !self.layer0.contains(idx, nb) {
                        self.layer0.push_neighbor(idx, nb);
                    }
                }

                // ── Vamana 双向剪枝（核心）──
                // 对于每个被选中的邻居 nb，将 idx 加入 nb 的候选集，
                // 然后用 vamana_select 重新决定 nb 应该保留哪些邻居。
                // 这保证了反向边也经过多样性剪枝，而不是盲目堆积。
                for &nb in &selected {
                    // 收集 nb 的当前邻居 + 新候选 idx
                    let mut nb_candidates: Vec<(u32, u32)> = self
                        .layer0
                        .neighbors(nb)
                        .iter()
                        .map(|&n| (self.bq_store.distance(nb as usize, n as usize, self.dim), n))
                        .collect();

                    // 如果 idx 还不在 nb 的邻居中，加入候选
                    if !self.layer0.contains(nb, idx) {
                        let d = self.bq_store.distance(nb as usize, idx as usize, self.dim);
                        nb_candidates.push((d, idx));
                    }

                    // 按距离排序后做 Vamana 剪枝
                    nb_candidates.sort_unstable_by_key(|&(d, _)| d);
                    let pruned = Self::vamana_select(
                        &self.bq_store,
                        nb,
                        &nb_candidates,
                        self.m0,
                        self.dim,
                        self.alpha,
                    );
                    self.layer0.set_neighbors(nb, &pruned);
                }
            } else {
                let ul = l - 1;
                // 正向边
                for &nb in &selected {
                    if !self.upper_layers[ul][idx as usize].contains(&nb) {
                        self.upper_layers[ul][idx as usize].push(nb);
                    }
                }

                // 双向剪枝（上层）
                for &nb in &selected {
                    let mut nb_candidates: Vec<(u32, u32)> = self.upper_layers[ul][nb as usize]
                        .iter()
                        .map(|&n| (self.bq_store.distance(nb as usize, n as usize, self.dim), n))
                        .collect();

                    if !self.upper_layers[ul][nb as usize].contains(&idx) {
                        let d = self.bq_store.distance(nb as usize, idx as usize, self.dim);
                        nb_candidates.push((d, idx));
                    }

                    nb_candidates.sort_unstable_by_key(|&(d, _)| d);
                    let pruned = Self::vamana_select(
                        &self.bq_store,
                        nb,
                        &nb_candidates,
                        self.m,
                        self.dim,
                        self.alpha,
                    );
                    self.upper_layers[ul][nb as usize] = pruned;
                }
            }

            if !candidates.is_empty() {
                cur_node = candidates[0].1;
            }
        }

        if level > self.max_level {
            self.entry_point = idx;
            self.max_level = level;
        }
    }

    /// 获取 Layer 0 节点的邻居列表（**仅限实验**）。
    #[cfg(feature = "research")]
    pub fn layer0_neighbors(&self, node: u32) -> &[u32] {
        self.layer0.neighbors(node)
    }

    /// 获取图的入口节点索引（**仅限实验**）。
    #[cfg(feature = "research")]
    pub fn entry_point(&self) -> u32 {
        self.entry_point
    }

    pub fn stats(&self) -> GraphStats {
        let hot_bq = self.bq_store.hot_bytes();
        let hot_l0 = self.layer0.data.len() * 4;
        let hot_upper: usize = self
            .upper_layers
            .iter()
            .map(|l| l.iter().map(|adj| adj.len() * 4 + 24).sum::<usize>())
            .sum();
        let tombstone_count = self.tombstones.iter().filter(|&&t| t).count();

        GraphStats {
            n: self.n,
            max_level: self.max_level,
            hot_bytes: hot_bq
                + hot_l0
                + hot_upper
                + self.geometry_codes.len() * std::mem::size_of::<GeometryCode>()
                + {
                    #[cfg(feature = "research")]
                    {
                        self.direction_store.as_ref().map_or(0, |s| s.bytes())
                    }
                    #[cfg(not(feature = "research"))]
                    {
                        0
                    }
                }
                + self
                    .polar_router
                    .as_ref()
                    .map_or(0, hyperbolic::PolarRouter::hot_bytes),
            tombstone_count,
            dirty_count: self.dirty_count,
            avg_degree_l0: if self.n > 0 {
                (0..self.n)
                    .map(|i| self.layer0.degree(i as u32))
                    .sum::<usize>() as f64
                    / self.n as f64
            } else {
                0.0
            },
        }
    }

    /// 图连通性诊断
    pub fn debug_connectivity(&self) {
        // 度数分布
        let mut deg0 = 0usize;
        let mut min_deg = usize::MAX;
        let mut max_deg = 0usize;
        for i in 0..self.n {
            let d = self.layer0.degree(i as u32);
            if d == 0 {
                deg0 += 1;
            }
            min_deg = min_deg.min(d);
            max_deg = max_deg.max(d);
        }
        tracing::debug!(
            min_degree = min_deg,
            max_degree = max_deg,
            isolated_nodes = deg0,
            nodes = self.n,
            "QuIVer L0 度数诊断 (QuIVer L0 degree diagnostics)"
        );

        // BFS 从入口点测可达性
        let mut visited = vec![false; self.n];
        let mut queue = std::collections::VecDeque::new();
        visited[self.entry_point as usize] = true;
        queue.push_back(self.entry_point);
        let mut reached = 1usize;
        while let Some(cur) = queue.pop_front() {
            for &nb in self.layer0.neighbors(cur) {
                if !visited[nb as usize] {
                    visited[nb as usize] = true;
                    queue.push_back(nb);
                    reached += 1;
                }
            }
        }
        tracing::debug!(
            reached,
            nodes = self.n,
            reachable_percent = 100.0 * reached as f64 / self.n as f64,
            "QuIVer 入口点可达性 (QuIVer entry-point reachability)"
        );

        // 入口点邻居数
        tracing::debug!(
            entry_point = self.entry_point,
            degree = self.layer0.degree(self.entry_point),
            "QuIVer 入口点诊断 (QuIVer entry-point diagnostics)"
        );
    }

    // ── 增量管理接口 ──

    /// 软删除：标记 tombstone，搜索时跳过（节点仍参与图遍历作为中转）
    pub fn soft_delete(&mut self, id: u64) -> bool {
        if let Some(&idx) = self.id_to_internal.get(&id) {
            if !self.tombstones[idx as usize] {
                self.tombstones[idx as usize] = true;
                self.dirty_count += 1;
                self.prefilter_slot_cache
                    .get_mut()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clear();
            }
            true
        } else {
            false
        }
    }

    /// 是否需要全量重建（增量变更超过 25%）
    #[inline]
    pub fn needs_rebuild(&self) -> bool {
        self.n > 0 && self.dirty_count * 4 > self.n
    }

    /// 活跃节点数（总数 - tombstone 数）
    #[inline]
    pub fn active_count(&self) -> usize {
        self.n - self.tombstones.iter().filter(|&&t| t).count()
    }

    /// 节点总数（含 tombstone）
    #[inline]
    pub fn total_count(&self) -> usize {
        self.n
    }

    /// 递增增量变更计数（增量追加节点时调用）
    #[inline]
    pub fn dirty_count_inc(&mut self) {
        self.dirty_count += 1;
    }
}

pub struct GraphStats {
    pub n: usize,
    pub max_level: usize,
    pub hot_bytes: usize,
    pub tombstone_count: usize,
    pub dirty_count: usize,
    pub avg_degree_l0: f64,
}

#[inline]
pub fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    dot / (na.sqrt() * nb.sqrt()).max(1e-30)
}

use crate::index::pelorus::engine::config::{BuildConfig, SearchConfig};

mod storage;
use storage::*;

mod construction;

mod filter;
use filter::*;

mod search;

mod persistence;

mod build_context;
use build_context::ParallelBuildContext;
