//! TSNG（三信号导航）实验性混合检索研究线。
//!
//! 查询同时声明向量、属性与图信号，执行层提供 exact ground truth、post-filter、
//! graph-union 和工业 AccessPath 等策略，并返回逐信号得分与 I/O/候选成本。所有路径
//! 共用候选、访问节点、扫描边和前沿预算，用于 Recall@K/NDCG@K 对照研究；该 API
//! 尚未语义冻结，不应与生产默认的 TQL/search 管线混为一谈。

use crate::error::{Result, TriviumError};
use crate::filter::Filter;
use crate::graph::reachability::ReachabilityDirection;
use crate::index::quiver::NavigationScorer;
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use crate::vector::VectorType;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, PartialEq)]
pub struct GraphSignalQuery {
    pub anchor_id: NodeId,
    pub direction: ReachabilityDirection,
    pub labels: Option<Vec<String>>,
    pub min_edge_weight: f32,
    pub max_hops: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TsngWeights {
    pub vector: f32,
    pub property: f32,
    pub graph: f32,
}

impl Default for TsngWeights {
    fn default() -> Self {
        Self {
            vector: 1.0,
            property: 0.0,
            graph: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TsngBudget {
    pub max_candidates: usize,
    pub max_visited_nodes: usize,
    pub max_examined_edges: usize,
    pub max_frontier_size: usize,
}

impl Default for TsngBudget {
    fn default() -> Self {
        Self {
            max_candidates: 1_000_000,
            max_visited_nodes: 1_000_000,
            max_examined_edges: 5_000_000,
            max_frontier_size: 1_000_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TsngQuery<'a, T> {
    pub vector: &'a [T],
    pub payload_filter: Option<&'a Filter>,
    pub graph: Option<GraphSignalQuery>,
    pub top_k: usize,
    pub weights: TsngWeights,
    pub budget: TsngBudget,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TsngHit {
    pub id: NodeId,
    pub final_score: f32,
    pub vector_similarity: f32,
    pub vector_signal: f32,
    pub property_signal: f32,
    pub graph_signal: f32,
    pub graph_depth: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TsngCost {
    pub candidates_scanned: usize,
    pub payload_checks: usize,
    pub vector_comparisons: usize,
    pub graph_visited_nodes: usize,
    pub graph_examined_edges: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TsngGroundTruth {
    pub hits: Vec<TsngHit>,
    pub cost: TsngCost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TsngSearchConfig {
    pub ef_search: usize,
    pub candidate_pool: usize,
    /// 元数据最多可抵消的向量距离比例，范围 0..=1_000_000。
    pub metadata_bonus_cap_ppm: u32,
    /// 双队列中 signal 队列的扩展配额，范围 0..=1_000_000。
    pub signal_queue_quota_ppm: u32,
    /// 注入 signal 队列的业务图候选上限；0 表示关闭图通道。
    pub graph_seed_limit: usize,
}

impl TsngSearchConfig {
    pub fn for_top_k(top_k: usize) -> Self {
        Self {
            ef_search: top_k.saturating_mul(16).max(64),
            candidate_pool: top_k.saturating_mul(16).max(64),
            metadata_bonus_cap_ppm: 200_000,
            signal_queue_quota_ppm: 0,
            graph_seed_limit: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TsngSearchMetrics {
    pub navigation_scores: usize,
    pub navigation_property_checks: usize,
    pub navigation_graph_checks: usize,
    pub candidates_reranked: usize,
    pub graph_seeds_injected: usize,
    pub access_path: IndustrialAccessPath,
    pub candidate_peak: usize,
    pub estimated_temp_bytes: usize,
    pub estimated_vector_page_reads: usize,
    pub estimated_payload_page_reads: usize,
    pub estimated_graph_page_reads: usize,
    pub adaptive_ef_search: usize,
    pub vector_density_skew_ppm: u32,
    pub property_path_cost: u64,
    pub graph_path_cost: u64,
    pub filtered_ann_path_cost: u64,
    pub bq_posting_lookup_ns: u64,
    pub bq_node_mapping_ns: u64,
    pub bq_heap_scan_ns: u64,
    pub bq_output_sort_ns: u64,
    pub bq_slot_cache_hits: usize,
    pub bq_slot_cache_misses: usize,
    pub bq_mapped_candidates: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndustrialAccessPath {
    PropertyFirst,
    PropertyFilteredAnn,
    GraphFirst,
    PropertyVectorUnion,
    GraphVectorUnion,
    #[default]
    AnnPostFilter,
    ExactFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryMemoryBudget {
    pub max_candidate_id_bytes: usize,
    pub max_union_bytes: usize,
    pub max_rerank_vector_bytes: usize,
    pub max_estimated_page_reads: usize,
}

impl QueryMemoryBudget {
    pub fn max_candidate_ids(self) -> usize {
        self.max_candidate_id_bytes / std::mem::size_of::<NodeId>()
    }

    pub fn max_union_ids(self) -> usize {
        self.max_union_bytes / std::mem::size_of::<(NodeId, f32)>()
    }

    pub fn max_rerank_vectors<T: VectorType>(self, dim: usize) -> usize {
        self.max_rerank_vector_bytes / dim.saturating_mul(std::mem::size_of::<T>()).max(1)
    }
}

impl Default for QueryMemoryBudget {
    fn default() -> Self {
        Self {
            max_candidate_id_bytes: 8 * 1_000_000,
            max_union_bytes: 16 * 100_000,
            max_rerank_vector_bytes: 256 * 1024 * 1024,
            max_estimated_page_reads: 1_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeamAdaptation {
    Fixed,
    Selectivity,
    SelectivityAndDensity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndustrialSearchConfig {
    pub ann: TsngSearchConfig,
    pub memory: QueryMemoryBudget,
    pub direct_rerank_bytes: usize,
    pub union_rerank_bytes: usize,
    pub beam_adaptation: BeamAdaptation,
}

impl IndustrialSearchConfig {
    pub fn for_top_k(top_k: usize) -> Self {
        Self {
            ann: TsngSearchConfig::for_top_k(top_k),
            memory: QueryMemoryBudget::default(),
            direct_rerank_bytes: 12 * 1024 * 1024,
            union_rerank_bytes: 128 * 1024 * 1024,
            beam_adaptation: BeamAdaptation::Selectivity,
        }
    }

    pub fn direct_candidate_limit<T: VectorType>(self, dim: usize, top_k: usize) -> usize {
        let bytes_per_vector = dim.saturating_mul(std::mem::size_of::<T>()).max(1);
        (self.direct_rerank_bytes / bytes_per_vector)
            .max(top_k)
            .min(self.memory.max_candidate_ids())
            .min(self.memory.max_union_ids())
            .min(self.memory.max_rerank_vectors::<T>(dim))
    }

    pub fn union_candidate_limit<T: VectorType>(self, dim: usize, top_k: usize) -> usize {
        let bytes_per_vector = dim.saturating_mul(std::mem::size_of::<T>()).max(1);
        (self.union_rerank_bytes / bytes_per_vector)
            .max(top_k)
            .min(self.memory.max_candidate_ids())
            .min(self.memory.max_union_ids())
            .min(self.memory.max_rerank_vectors::<T>(dim))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TsngSearchResult {
    pub hits: Vec<TsngHit>,
    pub metrics: TsngSearchMetrics,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TsngQualityMetrics {
    pub recall_at_k: f64,
    pub ndcg_at_k: f64,
}

const NAVIGATION_SCALE: u32 = 1_000_000;

#[derive(Default)]
struct CountingBqScorer {
    scores: usize,
}

impl NavigationScorer for CountingBqScorer {
    fn score(&mut self, _node_id: NodeId, bq_distance: u32) -> u32 {
        self.scores += 1;
        bq_distance
    }
}

struct FilteringBqScorer<'a> {
    scores: usize,
    accepted: &'a HashSet<NodeId>,
}

impl NavigationScorer for FilteringBqScorer<'_> {
    fn score(&mut self, _node_id: NodeId, bq_distance: u32) -> u32 {
        self.scores += 1;
        bq_distance
    }

    fn accept_result(&mut self, node_id: NodeId) -> bool {
        self.accepted.contains(&node_id)
    }
}

pub(crate) struct TsngNavigationScorer<'a> {
    max_bq_distance: u32,
    property_weight: u32,
    graph_weight: u32,
    metadata_bonus_cap_ppm: u32,
    property_match: &'a dyn Fn(NodeId) -> bool,
    graph_signals: &'a HashMap<NodeId, (f32, usize)>,
    metrics: TsngSearchMetrics,
}

impl<'a> TsngNavigationScorer<'a> {
    fn new(
        dim: usize,
        weights: TsngWeights,
        metadata_bonus_cap_ppm: u32,
        property_match: &'a dyn Fn(NodeId) -> bool,
        graph_signals: &'a HashMap<NodeId, (f32, usize)>,
    ) -> Self {
        let sum = weights.vector + weights.property + weights.graph;
        let fixed = |weight: f32| ((weight / sum) * NAVIGATION_SCALE as f32).round() as u32;
        Self {
            max_bq_distance: u32::try_from(dim.saturating_mul(2))
                .unwrap_or(u32::MAX)
                .max(1),
            property_weight: fixed(weights.property),
            graph_weight: fixed(weights.graph),
            metadata_bonus_cap_ppm,
            property_match,
            graph_signals,
            metrics: TsngSearchMetrics::default(),
        }
    }

    fn finish(self, candidates_reranked: usize) -> TsngSearchMetrics {
        TsngSearchMetrics {
            candidates_reranked,
            ..self.metrics
        }
    }
}

impl NavigationScorer for TsngNavigationScorer<'_> {
    fn score(&mut self, node_id: NodeId, bq_distance: u32) -> u32 {
        self.metrics.navigation_scores += 1;
        let vector_distance = u64::from(bq_distance.min(self.max_bq_distance))
            * u64::from(NAVIGATION_SCALE)
            / u64::from(self.max_bq_distance);
        let property_signal = if self.property_weight > 0 {
            self.metrics.navigation_property_checks += 1;
            u64::from((self.property_match)(node_id)) * u64::from(NAVIGATION_SCALE)
        } else {
            0
        };
        let graph_signal = if self.graph_weight > 0 {
            self.metrics.navigation_graph_checks += 1;
            self.graph_signals
                .get(&node_id)
                .map(|(signal, _)| {
                    (signal.clamp(0.0, 1.0) * NAVIGATION_SCALE as f32).round() as u64
                })
                .unwrap_or(0)
        } else {
            0
        };
        let property_bonus =
            u64::from(self.property_weight) * property_signal / u64::from(NAVIGATION_SCALE);
        let graph_bonus = u64::from(self.graph_weight) * graph_signal / u64::from(NAVIGATION_SCALE);
        let metadata_bonus = property_bonus.saturating_add(graph_bonus);
        let bonus_cap =
            vector_distance * u64::from(self.metadata_bonus_cap_ppm) / u64::from(NAVIGATION_SCALE);
        vector_distance
            .saturating_sub(metadata_bonus.min(bonus_cap))
            .min(u64::from(NAVIGATION_SCALE)) as u32
    }
}

#[derive(Debug, Default)]
struct ExactGraphSignals {
    values: HashMap<NodeId, (f32, usize)>,
    visited_nodes: usize,
    examined_edges: usize,
}

fn validate_search_config(config: TsngSearchConfig) -> Result<()> {
    if config.ef_search == 0 || config.candidate_pool == 0 {
        return Err(TriviumError::InvalidInput(
            "TSNG ef_search 和 candidate_pool 必须大于零 (TSNG ef_search and candidate_pool must be greater than zero)".into(),
        ));
    }
    if config.metadata_bonus_cap_ppm > NAVIGATION_SCALE
        || config.signal_queue_quota_ppm > NAVIGATION_SCALE
    {
        return Err(TriviumError::InvalidInput(
            "TSNG bonus cap 和 signal queue quota 均不得超过 1000000 (TSNG bonus cap and signal queue quota must not exceed 1000000)".into(),
        ));
    }
    Ok(())
}

fn intersect_ids(left: Vec<NodeId>, right: &[NodeId]) -> Vec<NodeId> {
    let mut result = Vec::with_capacity(left.len().min(right.len()));
    let (mut left_index, mut right_index) = (0, 0);
    while left_index < left.len() && right_index < right.len() {
        match left[left_index].cmp(&right[right_index]) {
            std::cmp::Ordering::Less => left_index += 1,
            std::cmp::Ordering::Greater => right_index += 1,
            std::cmp::Ordering::Equal => {
                result.push(left[left_index]);
                left_index += 1;
                right_index += 1;
            }
        }
    }
    result
}

fn indexed_filter_identity(filter: &Filter) -> u64 {
    fn mix(hash: u64, bytes: &[u8]) -> u64 {
        bytes.iter().fold(hash, |value, byte| {
            (value ^ *byte as u64).wrapping_mul(0x100_0000_01b3)
        })
    }
    match filter {
        Filter::Eq(field, value) => {
            let hash = mix(0xcbf2_9ce4_8422_2325, b"eq");
            let hash = mix(hash, field.as_bytes());
            mix(hash, value.to_string().as_bytes())
        }
        Filter::And(filters) => filters
            .iter()
            .fold(mix(0xcbf2_9ce4_8422_2325, b"and"), |hash, child| {
                hash.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ indexed_filter_identity(child)
            }),
        _ => 0,
    }
}

fn industrial_access_path_key(path: IndustrialAccessPath) -> u8 {
    match path {
        IndustrialAccessPath::PropertyFirst => 0,
        IndustrialAccessPath::PropertyVectorUnion => 1,
        IndustrialAccessPath::GraphVectorUnion => 2,
        IndustrialAccessPath::PropertyFilteredAnn => 3,
        IndustrialAccessPath::GraphFirst => 4,
        IndustrialAccessPath::AnnPostFilter => 5,
        IndustrialAccessPath::ExactFallback => 6,
    }
}

fn indexed_filter_origin(filter: &Filter) -> Option<(&str, &serde_json::Value)> {
    match filter {
        Filter::Eq(field, value) => Some((field, value)),
        Filter::And(filters) if filters.len() == 1 => indexed_filter_origin(&filters[0]),
        _ => None,
    }
}

fn indexed_filter_candidates<T: VectorType>(
    memtable: &MemTable<T>,
    filter: &Filter,
) -> Option<Vec<NodeId>> {
    match filter {
        Filter::Eq(field, value) => memtable.find_by_property_index(field, value),
        Filter::And(filters) => {
            let mut indexed = filters
                .iter()
                .filter_map(|filter| indexed_filter_candidates(memtable, filter))
                .collect::<Vec<_>>();
            indexed.sort_by_key(Vec::len);
            let mut candidates = indexed.into_iter();
            let first = candidates.next()?;
            Some(candidates.fold(first, |left, right| intersect_ids(left, &right)))
        }
        _ => None,
    }
}

fn checked_candidate_metrics<T: VectorType>(
    candidate_count: usize,
    vector_dim: usize,
    graph_signals: &ExactGraphSignals,
    budget: QueryMemoryBudget,
) -> Result<TsngSearchMetrics> {
    let max_candidate_ids = budget.max_candidate_ids();
    let max_rerank_vectors = budget.max_rerank_vectors::<T>(vector_dim);
    if candidate_count > max_candidate_ids || candidate_count > max_rerank_vectors {
        return Err(TriviumError::QueryExecution(format!(
            "工业查询候选数 {candidate_count} 超过字节预算：候选上限 {max_candidate_ids}，精排上限 {max_rerank_vectors} (Industrial query candidate count {candidate_count} exceeds byte budgets: candidate limit {max_candidate_ids}, rerank limit {max_rerank_vectors})"
        )));
    }
    let estimated_temp_bytes = candidate_count
        .checked_mul(std::mem::size_of::<(NodeId, f32)>())
        .ok_or_else(|| {
            TriviumError::QueryExecution(
                "工业查询临时内存估算溢出 (Industrial query temporary memory estimate overflow)"
                    .into(),
            )
        })?;
    let vector_bytes = candidate_count
        .checked_mul(vector_dim)
        .and_then(|elements| elements.checked_mul(std::mem::size_of::<T>()))
        .unwrap_or(usize::MAX);
    let estimated_vector_page_reads = vector_bytes.saturating_add(4095) / 4096;
    let estimated_payload_page_reads = candidate_count;
    let estimated_graph_page_reads = graph_signals.examined_edges.saturating_add(255) / 256;
    let estimated_page_reads = estimated_vector_page_reads
        .saturating_add(estimated_payload_page_reads)
        .saturating_add(estimated_graph_page_reads);
    if estimated_page_reads > budget.max_estimated_page_reads {
        return Err(TriviumError::QueryExecution(format!(
            "工业查询预计页读取 {estimated_page_reads} 超过预算 {} (Industrial query estimated page reads {estimated_page_reads} exceed budget {})",
            budget.max_estimated_page_reads, budget.max_estimated_page_reads
        )));
    }
    Ok(TsngSearchMetrics {
        candidates_reranked: candidate_count,
        candidate_peak: candidate_count,
        estimated_temp_bytes,
        estimated_vector_page_reads,
        estimated_payload_page_reads,
        estimated_graph_page_reads,
        ..TsngSearchMetrics::default()
    })
}

pub(crate) fn industrial_search<T: VectorType>(
    memtable: &MemTable<T>,
    query: &TsngQuery<'_, T>,
    config: IndustrialSearchConfig,
) -> Result<TsngSearchResult> {
    validate_query(memtable, query)?;
    validate_search_config(config.ann)?;
    let direct_candidate_limit = config.direct_candidate_limit::<T>(memtable.dim(), query.top_k);
    let union_candidate_limit = config.union_candidate_limit::<T>(memtable.dim(), query.top_k);
    if config.direct_rerank_bytes > config.union_rerank_bytes
        || direct_candidate_limit > union_candidate_limit
    {
        return Err(TriviumError::InvalidInput(
            "工业查询精排字节阈值顺序无效或超过总预算 (Industrial rerank byte thresholds are invalid or exceed the total budget)".into(),
        ));
    }
    let Some(quiver) = memtable.quiver() else {
        let exact = exact_ground_truth(memtable, query)?;
        let mut metrics = checked_candidate_metrics::<T>(
            exact.cost.vector_comparisons,
            memtable.dim(),
            &ExactGraphSignals::default(),
            config.memory,
        )?;
        metrics.access_path = IndustrialAccessPath::ExactFallback;
        return Ok(TsngSearchResult {
            hits: exact.hits,
            metrics,
        });
    };
    let graph_signals = exact_graph_signals(memtable, query)?;
    let property_candidates = query
        .payload_filter
        .and_then(|filter| indexed_filter_candidates(memtable, filter));
    let property_filter_ids = property_candidates
        .as_ref()
        .map(|ids| ids.iter().copied().collect::<HashSet<_>>());
    let graph_candidates = query.graph.as_ref().map(|_| {
        let mut ids = graph_signals.values.keys().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    });
    let property_count = property_candidates.as_ref().map_or(usize::MAX, Vec::len);
    let direct = property_count <= direct_candidate_limit;
    let bq_prefilter = !direct && property_count <= config.memory.max_candidate_ids();
    let vector_work = memtable.dim().max(1) as u64;
    let ann_work = config.ann.ef_search.max(config.ann.candidate_pool) as u64;
    let property_path_cost = if direct {
        property_count as u64 * vector_work
    } else if bq_prefilter {
        property_count as u64 * 2 + config.ann.candidate_pool as u64 * vector_work
    } else {
        u64::MAX
    };
    let graph_path_cost = graph_candidates.as_ref().map_or(u64::MAX, |ids| {
        ann_work
            .saturating_mul(2)
            .saturating_add(ids.len() as u64 * vector_work)
    });
    let selectivity_ef = if property_count == 0 || property_count == usize::MAX {
        config.ann.ef_search
    } else {
        config
            .ann
            .candidate_pool
            .saturating_mul(memtable.node_count())
            .div_ceil(property_count)
            .max(config.ann.ef_search)
    };
    let filtered_ann_path_cost = if property_filter_ids.is_some() {
        selectivity_ef as u64 * 2 + config.ann.candidate_pool as u64 * vector_work
    } else {
        u64::MAX
    };
    let mut alternatives = Vec::new();
    if property_path_cost != u64::MAX {
        alternatives.push((
            property_path_cost,
            if direct {
                IndustrialAccessPath::PropertyFirst
            } else {
                IndustrialAccessPath::PropertyVectorUnion
            },
        ));
    }
    if graph_path_cost != u64::MAX {
        alternatives.push((graph_path_cost, IndustrialAccessPath::GraphVectorUnion));
    }
    if filtered_ann_path_cost != u64::MAX {
        alternatives.push((
            filtered_ann_path_cost,
            IndustrialAccessPath::PropertyFilteredAnn,
        ));
    }
    alternatives.sort_by_key(|(cost, path)| (*cost, industrial_access_path_key(*path)));
    let access_path = alternatives
        .first()
        .map_or(IndustrialAccessPath::AnnPostFilter, |(_, path)| *path);
    let signal_ids = match access_path {
        IndustrialAccessPath::PropertyFirst | IndustrialAccessPath::PropertyVectorUnion => {
            property_candidates.clone()
        }
        IndustrialAccessPath::GraphVectorUnion => graph_candidates,
        _ => None,
    };
    let vector_density_skew = query
        .payload_filter
        .and_then(indexed_filter_origin)
        .and_then(|(field, value)| memtable.cross_modal_stats(field, value))
        .and_then(|stats| stats.vector_density_skew)
        .unwrap_or(1.0);
    let uses_adaptive_ann = matches!(
        access_path,
        IndustrialAccessPath::PropertyFilteredAnn | IndustrialAccessPath::AnnPostFilter
    );
    let adaptive_ef = property_filter_ids
        .as_ref()
        .map_or(config.ann.ef_search, |ids| {
            if !uses_adaptive_ann
                || ids.is_empty()
                || config.beam_adaptation == BeamAdaptation::Fixed
            {
                config.ann.ef_search
            } else {
                let selectivity_ef = config
                    .ann
                    .candidate_pool
                    .saturating_mul(memtable.node_count())
                    .div_ceil(ids.len())
                    .max(config.ann.ef_search);
                let density_adjusted =
                    if config.beam_adaptation == BeamAdaptation::SelectivityAndDensity {
                        (selectivity_ef as f64 * vector_density_skew.sqrt()).ceil() as usize
                    } else {
                        selectivity_ef
                    };
                density_adjusted
                    .max(config.ann.ef_search)
                    .min(selectivity_ef.saturating_mul(2))
                    .min(config.memory.max_candidate_ids())
                    .min(config.memory.max_rerank_vectors::<T>(memtable.dim()))
            }
        });
    let search_config = crate::index::quiver::QuIVerSearchConfig {
        top_k: config.ann.candidate_pool,
        ef_search: adaptive_ef.max(config.ann.candidate_pool),
        rerank_limit: Some(adaptive_ef),
    };
    let mut navigation_scores = 0;
    let mut bq_prefilter_metrics = None;
    let mut candidates = HashMap::new();
    let selected_bq_prefilter = access_path == IndustrialAccessPath::PropertyVectorUnion;
    let selected_ann = matches!(
        access_path,
        IndustrialAccessPath::PropertyFilteredAnn
            | IndustrialAccessPath::GraphVectorUnion
            | IndustrialAccessPath::AnnPostFilter
    );
    if selected_bq_prefilter {
        let query_f32 = query
            .vector
            .iter()
            .map(|value| value.to_f32())
            .collect::<Vec<_>>();
        if let Some(ids) = signal_ids.as_ref() {
            navigation_scores = ids.len();
            let prefilter_limit = config
                .ann
                .candidate_pool
                .max(query.top_k)
                .min(union_candidate_limit);
            let filter_identity = query
                .payload_filter
                .map(indexed_filter_identity)
                .unwrap_or_default();
            let (prefiltered, prefilter_metrics) = quiver.prefilter_candidates_profiled(
                &query_f32,
                ids,
                filter_identity,
                memtable.property_generation(),
                prefilter_limit,
            );
            for (id, _) in prefiltered {
                if let Some(vector) = memtable.get_vector(id) {
                    candidates.insert(id, T::similarity(query.vector, vector));
                }
            }
            bq_prefilter_metrics = Some(prefilter_metrics);
        }
    } else if selected_ann {
        let query_f32 = query
            .vector
            .iter()
            .map(|value| value.to_f32())
            .collect::<Vec<_>>();
        let mut scorer = CountingBqScorer::default();
        let found = if let Some(accepted) = property_filter_ids.as_ref() {
            let mut filtering = FilteringBqScorer {
                scores: 0,
                accepted,
            };
            let found = quiver.search_with_scorer(
                &query_f32,
                |slot, buffer| {
                    let Some(vector) = memtable.vec_pool().get(slot) else {
                        return false;
                    };
                    buffer.clear();
                    buffer.extend(vector.iter().map(|value| value.to_f32()));
                    true
                },
                &search_config,
                &mut filtering,
            );
            navigation_scores = filtering.scores;
            found
        } else {
            let found = quiver.search_with_scorer(
                &query_f32,
                |slot, buffer| {
                    let Some(vector) = memtable.vec_pool().get(slot) else {
                        return false;
                    };
                    buffer.clear();
                    buffer.extend(vector.iter().map(|value| value.to_f32()));
                    true
                },
                &search_config,
                &mut scorer,
            );
            navigation_scores = scorer.scores;
            found
        };
        for (id, similarity) in found {
            candidates.insert(id, similarity);
        }
    }
    if matches!(
        access_path,
        IndustrialAccessPath::PropertyFirst
            | IndustrialAccessPath::PropertyVectorUnion
            | IndustrialAccessPath::GraphVectorUnion
    ) && !selected_bq_prefilter
        && let Some(ids) = signal_ids
    {
        let projected = candidates.len().saturating_add(ids.len());
        if projected > union_candidate_limit {
            return Err(TriviumError::QueryExecution(format!(
                "工业查询候选并集预计 {projected} 超过维度推导上限 {union_candidate_limit} (Industrial candidate union estimate {projected} exceeds dimension-derived limit {union_candidate_limit})"
            )));
        }
        for id in ids {
            if candidates.contains_key(&id) {
                continue;
            }
            if let Some(vector) = memtable.get_vector(id) {
                candidates.insert(id, T::similarity(query.vector, vector));
            }
        }
    }
    let candidates = candidates.into_iter().collect::<Vec<_>>();
    let mut metrics = checked_candidate_metrics::<T>(
        candidates.len(),
        memtable.dim(),
        &graph_signals,
        config.memory,
    )?;
    metrics.navigation_scores = navigation_scores;
    metrics.access_path = access_path;
    metrics.adaptive_ef_search = adaptive_ef;
    metrics.vector_density_skew_ppm =
        (vector_density_skew * NAVIGATION_SCALE as f64).round() as u32;
    metrics.property_path_cost = property_path_cost;
    metrics.graph_path_cost = graph_path_cost;
    metrics.filtered_ann_path_cost = filtered_ann_path_cost;
    if let Some(profile) = bq_prefilter_metrics {
        metrics.bq_posting_lookup_ns = profile.posting_lookup_ns;
        metrics.bq_node_mapping_ns = profile.node_mapping_ns;
        metrics.bq_heap_scan_ns = profile.bq_heap_scan_ns;
        metrics.bq_output_sort_ns = profile.output_sort_ns;
        metrics.bq_slot_cache_hits = usize::from(profile.cache_hit);
        metrics.bq_slot_cache_misses = usize::from(!profile.cache_hit);
        metrics.bq_mapped_candidates = profile.mapped_candidates;
    }
    let hits = exact_rerank_candidates(memtable, query, &graph_signals.values, candidates);
    Ok(TsngSearchResult { hits, metrics })
}

pub(crate) fn bq_prefilter_search<T: VectorType>(
    memtable: &MemTable<T>,
    query: &TsngQuery<'_, T>,
    config: IndustrialSearchConfig,
) -> Result<TsngSearchResult> {
    validate_query(memtable, query)?;
    let Some(quiver) = memtable.quiver() else {
        return industrial_search(memtable, query, config);
    };
    let graph_signals = exact_graph_signals(memtable, query)?;
    let Some(filter) = query.payload_filter else {
        return industrial_search(memtable, query, config);
    };
    let Some(ids) = indexed_filter_candidates(memtable, filter) else {
        return industrial_search(memtable, query, config);
    };
    let limit = config
        .ann
        .candidate_pool
        .max(query.top_k)
        .min(config.union_candidate_limit::<T>(memtable.dim(), query.top_k));
    let query_f32 = query
        .vector
        .iter()
        .map(|value| value.to_f32())
        .collect::<Vec<_>>();
    let (prefiltered, profile) = quiver.prefilter_candidates_profiled(
        &query_f32,
        &ids,
        indexed_filter_identity(filter),
        memtable.property_generation(),
        limit,
    );
    let candidates = prefiltered
        .into_iter()
        .filter_map(|(id, _)| {
            let vector = memtable.get_vector(id)?;
            Some((id, T::similarity(query.vector, vector)))
        })
        .collect::<Vec<_>>();
    let mut metrics = checked_candidate_metrics::<T>(
        candidates.len(),
        memtable.dim(),
        &graph_signals,
        config.memory,
    )?;
    metrics.navigation_scores = ids.len();
    metrics.access_path = IndustrialAccessPath::PropertyVectorUnion;
    metrics.bq_posting_lookup_ns = profile.posting_lookup_ns;
    metrics.bq_node_mapping_ns = profile.node_mapping_ns;
    metrics.bq_heap_scan_ns = profile.bq_heap_scan_ns;
    metrics.bq_output_sort_ns = profile.output_sort_ns;
    metrics.bq_slot_cache_hits = usize::from(profile.cache_hit);
    metrics.bq_slot_cache_misses = usize::from(!profile.cache_hit);
    metrics.bq_mapped_candidates = profile.mapped_candidates;
    Ok(TsngSearchResult {
        hits: exact_rerank_candidates(memtable, query, &graph_signals.values, candidates),
        metrics,
    })
}

pub(crate) fn graph_union_search<T: VectorType>(
    memtable: &MemTable<T>,
    query: &TsngQuery<'_, T>,
    config: TsngSearchConfig,
) -> Result<TsngSearchResult> {
    validate_query(memtable, query)?;
    validate_search_config(config)?;
    let Some(quiver) = memtable.quiver() else {
        let exact = exact_ground_truth(memtable, query)?;
        return Ok(TsngSearchResult {
            metrics: TsngSearchMetrics {
                candidates_reranked: exact.cost.vector_comparisons,
                ..TsngSearchMetrics::default()
            },
            hits: exact.hits,
        });
    };
    let graph_signals = exact_graph_signals(memtable, query)?;
    let search_config = crate::index::quiver::QuIVerSearchConfig {
        top_k: config.candidate_pool,
        ef_search: config.ef_search.max(config.candidate_pool),
        rerank_limit: Some(config.candidate_pool),
    };
    let query_f32 = query
        .vector
        .iter()
        .map(|value| value.to_f32())
        .collect::<Vec<_>>();
    let vec_pool = memtable.vec_pool();
    let mut scorer = CountingBqScorer::default();
    let ann_candidates = quiver.search_with_scorer(
        &query_f32,
        |slot, buffer| {
            let Some(vector) = vec_pool.get(slot) else {
                return false;
            };
            buffer.clear();
            buffer.extend(vector.iter().map(|value| value.to_f32()));
            true
        },
        &search_config,
        &mut scorer,
    );
    let mut candidates = ann_candidates.into_iter().collect::<HashMap<_, _>>();
    let mut graph_candidates = graph_signals
        .values
        .iter()
        .map(|(&id, &(signal, depth))| (id, signal, depth))
        .collect::<Vec<_>>();
    graph_candidates.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.0.cmp(&right.0))
    });
    graph_candidates.truncate(config.graph_seed_limit);
    for &(id, _, _) in &graph_candidates {
        if candidates.contains_key(&id) {
            continue;
        }
        if let Some(vector) = memtable.get_vector(id) {
            candidates.insert(id, T::similarity(query.vector, vector));
        }
    }
    let candidates = candidates.into_iter().collect::<Vec<_>>();
    let candidates_reranked = candidates.len();
    Ok(TsngSearchResult {
        hits: exact_rerank_candidates(memtable, query, &graph_signals.values, candidates),
        metrics: TsngSearchMetrics {
            navigation_scores: scorer.scores,
            candidates_reranked,
            graph_seeds_injected: graph_candidates.len(),
            ..TsngSearchMetrics::default()
        },
    })
}

pub(crate) fn post_filter_search<T: VectorType>(
    memtable: &MemTable<T>,
    query: &TsngQuery<'_, T>,
    config: TsngSearchConfig,
) -> Result<TsngSearchResult> {
    validate_query(memtable, query)?;
    validate_search_config(config)?;
    let Some(quiver) = memtable.quiver() else {
        let exact = exact_ground_truth(memtable, query)?;
        return Ok(TsngSearchResult {
            metrics: TsngSearchMetrics {
                candidates_reranked: exact.cost.vector_comparisons,
                ..TsngSearchMetrics::default()
            },
            hits: exact.hits,
        });
    };
    let graph_signals = exact_graph_signals(memtable, query)?;
    let search_config = crate::index::quiver::QuIVerSearchConfig {
        top_k: config.candidate_pool,
        ef_search: config.ef_search.max(config.candidate_pool),
        rerank_limit: Some(config.candidate_pool),
    };
    let query_f32 = query
        .vector
        .iter()
        .map(|value| value.to_f32())
        .collect::<Vec<_>>();
    let vec_pool = memtable.vec_pool();
    let mut scorer = CountingBqScorer::default();
    let candidates = quiver.search_with_scorer(
        &query_f32,
        |slot, buffer| {
            let Some(vector) = vec_pool.get(slot) else {
                return false;
            };
            buffer.clear();
            buffer.extend(vector.iter().map(|value| value.to_f32()));
            true
        },
        &search_config,
        &mut scorer,
    );
    let candidates_reranked = candidates.len();
    Ok(TsngSearchResult {
        hits: exact_rerank_candidates(memtable, query, &graph_signals.values, candidates),
        metrics: TsngSearchMetrics {
            navigation_scores: scorer.scores,
            candidates_reranked,
            ..TsngSearchMetrics::default()
        },
    })
}

pub(crate) fn approximate_search<T: VectorType>(
    memtable: &MemTable<T>,
    query: &TsngQuery<'_, T>,
    config: TsngSearchConfig,
) -> Result<TsngSearchResult> {
    validate_query(memtable, query)?;
    validate_search_config(config)?;
    let Some(quiver) = memtable.quiver() else {
        let exact = exact_ground_truth(memtable, query)?;
        return Ok(TsngSearchResult {
            metrics: TsngSearchMetrics {
                candidates_reranked: exact.cost.vector_comparisons,
                ..TsngSearchMetrics::default()
            },
            hits: exact.hits,
        });
    };
    let graph_signals = exact_graph_signals(memtable, query)?;
    let property_mask = query
        .payload_filter
        .map(Filter::extract_must_have_mask)
        .unwrap_or(0);
    let property_match = |id: NodeId| {
        if property_mask != 0 {
            return memtable
                .fast_tag_for_id(id)
                .is_some_and(|tag| tag & property_mask == property_mask);
        }
        query.payload_filter.is_none_or(|filter| {
            memtable
                .get_payload(id)
                .is_some_and(|payload| filter.matches(payload))
        })
    };
    let search_config = crate::index::quiver::QuIVerSearchConfig {
        top_k: config.candidate_pool,
        ef_search: config.ef_search.max(config.candidate_pool),
        rerank_limit: Some(config.candidate_pool),
    };
    let query_f32 = query
        .vector
        .iter()
        .map(|value| value.to_f32())
        .collect::<Vec<_>>();
    let vec_pool = memtable.vec_pool();
    let mut fill_vector = |slot: usize, buffer: &mut Vec<f32>| {
        let Some(vector) = vec_pool.get(slot) else {
            return false;
        };
        buffer.clear();
        buffer.extend(vector.iter().map(|value| value.to_f32()));
        true
    };
    let pure_vector = query.weights.property == 0.0 && query.weights.graph == 0.0;
    let (candidates, metrics) = if pure_vector {
        let candidates = quiver.search(&query_f32, &mut fill_vector, &search_config);
        let reranked = candidates.len();
        (
            candidates,
            TsngSearchMetrics {
                candidates_reranked: reranked,
                ..TsngSearchMetrics::default()
            },
        )
    } else {
        let mut scorer = TsngNavigationScorer::new(
            memtable.dim(),
            query.weights,
            config.metadata_bonus_cap_ppm,
            &property_match,
            &graph_signals.values,
        );
        let mut graph_seeds = if config.graph_seed_limit > 0 {
            graph_signals
                .values
                .iter()
                .map(|(&id, &(signal, depth))| (id, signal, depth))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        graph_seeds.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.2.cmp(&right.2))
                .then_with(|| left.0.cmp(&right.0))
        });
        graph_seeds.truncate(config.graph_seed_limit);
        let graph_seed_ids = graph_seeds.iter().map(|&(id, _, _)| id).collect::<Vec<_>>();
        let candidates = if config.signal_queue_quota_ppm > 0 || !graph_seed_ids.is_empty() {
            quiver.search_dual_queue(
                &query_f32,
                &mut fill_vector,
                &search_config,
                &mut scorer,
                config.signal_queue_quota_ppm,
                &graph_seed_ids,
            )
        } else {
            quiver.search_with_scorer(&query_f32, &mut fill_vector, &search_config, &mut scorer)
        };
        let reranked = candidates.len();
        let mut metrics = scorer.finish(reranked);
        metrics.graph_seeds_injected = graph_seed_ids.len();
        (candidates, metrics)
    };
    let hits = exact_rerank_candidates(memtable, query, &graph_signals.values, candidates);
    Ok(TsngSearchResult { hits, metrics })
}

fn exact_rerank_candidates<T: VectorType>(
    memtable: &MemTable<T>,
    query: &TsngQuery<'_, T>,
    graph_signals: &HashMap<NodeId, (f32, usize)>,
    candidates: Vec<(NodeId, f32)>,
) -> Vec<TsngHit> {
    let weight_sum = query.weights.vector + query.weights.property + query.weights.graph;
    let mut hits = candidates
        .into_iter()
        .filter_map(|(id, vector_similarity)| {
            let payload = memtable.get_payload(id)?;
            let property_signal = if let Some(filter) = query.payload_filter {
                if !filter.matches(payload) {
                    return None;
                }
                1.0
            } else {
                0.0
            };
            let (graph_signal, graph_depth) = graph_signals
                .get(&id)
                .copied()
                .map_or((0.0, None), |(signal, depth)| (signal, Some(depth)));
            let vector_signal = ((vector_similarity + 1.0) * 0.5).clamp(0.0, 1.0);
            let final_score = (query.weights.vector * vector_signal
                + query.weights.property * property_signal
                + query.weights.graph * graph_signal)
                / weight_sum;
            Some(TsngHit {
                id,
                final_score,
                vector_similarity,
                vector_signal,
                property_signal,
                graph_signal,
                graph_depth,
            })
        })
        .collect::<Vec<_>>();
    hits.sort_by(|left, right| {
        right
            .final_score
            .total_cmp(&left.final_score)
            .then_with(|| left.id.cmp(&right.id))
    });
    hits.truncate(query.top_k);
    hits
}

pub(crate) fn exact_ground_truth<T: VectorType>(
    memtable: &MemTable<T>,
    query: &TsngQuery<'_, T>,
) -> Result<TsngGroundTruth> {
    validate_query(memtable, query)?;
    let graph_signals = exact_graph_signals(memtable, query)?;
    let ids = memtable.all_node_ids();
    if ids.len() > query.budget.max_candidates {
        return Err(TriviumError::QueryExecution(format!(
            "TSNG 精确候选数 {} 超过预算 {} (TSNG exact candidate count {} exceeds budget {})",
            ids.len(),
            query.budget.max_candidates,
            ids.len(),
            query.budget.max_candidates
        )));
    }
    let weight_sum = query.weights.vector + query.weights.property + query.weights.graph;
    let mut cost = TsngCost {
        graph_visited_nodes: graph_signals.visited_nodes,
        graph_examined_edges: graph_signals.examined_edges,
        ..TsngCost::default()
    };
    let mut hits = Vec::with_capacity(ids.len().min(query.top_k.saturating_mul(4)));
    for id in ids {
        cost.candidates_scanned += 1;
        let Some(payload) = memtable.get_payload(id) else {
            continue;
        };
        let property_signal = if let Some(filter) = query.payload_filter {
            cost.payload_checks += 1;
            if !filter.matches(payload) {
                continue;
            }
            1.0
        } else {
            0.0
        };
        let Some(vector) = memtable.get_vector(id) else {
            continue;
        };
        cost.vector_comparisons += 1;
        let vector_similarity = T::similarity(query.vector, vector);
        let vector_signal = ((vector_similarity + 1.0) * 0.5).clamp(0.0, 1.0);
        let (graph_signal, graph_depth) = graph_signals
            .values
            .get(&id)
            .copied()
            .map_or((0.0, None), |(signal, depth)| (signal, Some(depth)));
        let final_score = (query.weights.vector * vector_signal
            + query.weights.property * property_signal
            + query.weights.graph * graph_signal)
            / weight_sum;
        hits.push(TsngHit {
            id,
            final_score,
            vector_similarity,
            vector_signal,
            property_signal,
            graph_signal,
            graph_depth,
        });
    }
    hits.sort_by(|left, right| {
        right
            .final_score
            .total_cmp(&left.final_score)
            .then_with(|| left.id.cmp(&right.id))
    });
    hits.truncate(query.top_k);
    Ok(TsngGroundTruth { hits, cost })
}

fn validate_query<T: VectorType>(memtable: &MemTable<T>, query: &TsngQuery<'_, T>) -> Result<()> {
    if query.vector.len() != memtable.dim() {
        return Err(TriviumError::DimensionMismatch {
            expected: memtable.dim(),
            got: query.vector.len(),
        });
    }
    if query.top_k == 0 {
        return Err(TriviumError::InvalidInput(
            "TSNG top_k 必须大于零 (TSNG top_k must be greater than zero)".into(),
        ));
    }
    let weights = [
        query.weights.vector,
        query.weights.property,
        query.weights.graph,
    ];
    if weights
        .iter()
        .any(|weight| !weight.is_finite() || *weight < 0.0)
        || weights.iter().sum::<f32>() <= 0.0
    {
        return Err(TriviumError::InvalidInput(
            "TSNG 权重必须是非负有限数且总和大于零 (TSNG weights must be finite, non-negative, and have a positive sum)".into(),
        ));
    }
    if query.weights.property > 0.0 && query.payload_filter.is_none() {
        return Err(TriviumError::InvalidInput(
            "属性权重大于零时必须提供 payload_filter (payload_filter is required when property weight is positive)".into(),
        ));
    }
    if query.weights.graph > 0.0 && query.graph.is_none() {
        return Err(TriviumError::InvalidInput(
            "图权重大于零时必须提供图信号查询 (graph signal query is required when graph weight is positive)".into(),
        ));
    }
    if let Some(graph) = &query.graph {
        if graph.max_hops == 0 {
            return Err(TriviumError::InvalidInput(
                "图信号 max_hops 必须大于零 (graph signal max_hops must be greater than zero)"
                    .into(),
            ));
        }
        if !graph.min_edge_weight.is_finite() {
            return Err(TriviumError::InvalidInput(
                "图信号最小边权必须是有限数 (graph signal minimum edge weight must be finite)"
                    .into(),
            ));
        }
    }
    Ok(())
}

fn exact_graph_signals<T: VectorType>(
    memtable: &MemTable<T>,
    query: &TsngQuery<'_, T>,
) -> Result<ExactGraphSignals> {
    let Some(graph) = &query.graph else {
        return Ok(ExactGraphSignals::default());
    };
    if memtable.get_payload(graph.anchor_id).is_none() {
        return Err(TriviumError::NodeNotFound(graph.anchor_id));
    }
    let mut queue = VecDeque::from([(graph.anchor_id, 0usize)]);
    let mut depths = HashMap::from([(graph.anchor_id, 0usize)]);
    let mut examined_edges = 0usize;
    let mut peak_frontier = 1usize;
    while let Some((current, depth)) = queue.pop_front() {
        if depth >= graph.max_hops {
            continue;
        }
        let mut neighbors = exact_graph_neighbors(memtable, current, graph);
        neighbors.sort_unstable();
        neighbors.dedup();
        for next in neighbors {
            if examined_edges >= query.budget.max_examined_edges {
                return Err(TriviumError::QueryExecution(format!(
                    "TSNG 精确图遍历超过边预算 {} (TSNG exact graph traversal exceeded edge budget {})",
                    query.budget.max_examined_edges, query.budget.max_examined_edges
                )));
            }
            examined_edges += 1;
            if depths.contains_key(&next) {
                continue;
            }
            if depths.len() >= query.budget.max_visited_nodes {
                return Err(TriviumError::QueryExecution(format!(
                    "TSNG 精确图遍历超过节点预算 {} (TSNG exact graph traversal exceeded node budget {})",
                    query.budget.max_visited_nodes, query.budget.max_visited_nodes
                )));
            }
            let next_depth = depth + 1;
            depths.insert(next, next_depth);
            queue.push_back((next, next_depth));
            peak_frontier = peak_frontier.max(queue.len());
            if peak_frontier > query.budget.max_frontier_size {
                return Err(TriviumError::QueryExecution(format!(
                    "TSNG 精确图遍历超过前沿预算 {} (TSNG exact graph traversal exceeded frontier budget {})",
                    query.budget.max_frontier_size, query.budget.max_frontier_size
                )));
            }
        }
    }
    depths.remove(&graph.anchor_id);
    let visited_nodes = depths.len().saturating_add(1);
    let values = depths
        .into_iter()
        .map(|(id, depth)| (id, (1.0 / depth as f32, depth)))
        .collect();
    Ok(ExactGraphSignals {
        values,
        visited_nodes,
        examined_edges,
    })
}

fn exact_graph_neighbors<T: VectorType>(
    memtable: &MemTable<T>,
    current: NodeId,
    graph: &GraphSignalQuery,
) -> Vec<NodeId> {
    let label_matches = |label: &str| {
        graph
            .labels
            .as_ref()
            .is_none_or(|labels| labels.iter().any(|allowed| allowed == label))
    };
    let mut neighbors = Vec::new();
    if matches!(
        graph.direction,
        ReachabilityDirection::Outgoing | ReachabilityDirection::Both
    ) && let Some(edges) = memtable.get_edges(current)
    {
        neighbors.extend(
            edges
                .iter()
                .filter(|edge| edge.weight >= graph.min_edge_weight && label_matches(&edge.label))
                .map(|edge| edge.target_id),
        );
    }
    if matches!(
        graph.direction,
        ReachabilityDirection::Incoming | ReachabilityDirection::Both
    ) {
        for &source in memtable.get_incoming_sources(current) {
            if memtable.get_edges(source).is_some_and(|edges| {
                edges.iter().any(|edge| {
                    edge.target_id == current
                        && edge.weight >= graph.min_edge_weight
                        && label_matches(&edge.label)
                })
            }) {
                neighbors.push(source);
            }
        }
    }
    neighbors
}

pub fn quality_metrics(
    exact: &[TsngHit],
    candidate_ids: &[NodeId],
    k: usize,
) -> TsngQualityMetrics {
    if k == 0 {
        return TsngQualityMetrics {
            recall_at_k: 1.0,
            ndcg_at_k: 1.0,
        };
    }
    let exact_top = exact.iter().take(k).collect::<Vec<_>>();
    if exact_top.is_empty() {
        return TsngQualityMetrics {
            recall_at_k: if candidate_ids.is_empty() { 1.0 } else { 0.0 },
            ndcg_at_k: if candidate_ids.is_empty() { 1.0 } else { 0.0 },
        };
    }
    let expected_ids = exact_top.iter().map(|hit| hit.id).collect::<HashSet<_>>();
    let unique_candidates = candidate_ids
        .iter()
        .take(k)
        .copied()
        .collect::<HashSet<_>>();
    let recalled = expected_ids.intersection(&unique_candidates).count();
    let recall_at_k = recalled as f64 / exact_top.len() as f64;
    let relevance = exact_top
        .iter()
        .enumerate()
        .map(|(rank, hit)| (hit.id, exact_top.len() - rank))
        .collect::<BTreeMap<_, _>>();
    let mut ranked_seen = HashSet::new();
    let dcg = candidate_ids
        .iter()
        .take(k)
        .enumerate()
        .map(|(rank, id)| {
            let gain = if ranked_seen.insert(*id) {
                relevance.get(id).copied().unwrap_or(0) as f64
            } else {
                0.0
            };
            gain / ((rank + 2) as f64).log2()
        })
        .sum::<f64>();
    let ideal_dcg = (0..exact_top.len())
        .map(|rank| (exact_top.len() - rank) as f64 / ((rank + 2) as f64).log2())
        .sum::<f64>();
    TsngQualityMetrics {
        recall_at_k,
        ndcg_at_k: if ideal_dcg == 0.0 {
            1.0
        } else {
            dcg / ideal_dcg
        },
    }
}

#[cfg(test)]
mod bounded_bonus_tests {
    use super::*;

    fn score(cap: u32, property_match: bool, bq_distance: u32) -> u32 {
        let property = move |_id| property_match;
        let graph = HashMap::new();
        let mut scorer = TsngNavigationScorer::new(
            100,
            TsngWeights {
                vector: 0.7,
                property: 0.3,
                graph: 0.0,
            },
            cap,
            &property,
            &graph,
        );
        scorer.score(1, bq_distance)
    }

    #[test]
    fn bounded_bonus_零上限严格保持向量距离() {
        for distance in [0, 1, 50, 100, 200] {
            let expected = u64::from(distance) * u64::from(NAVIGATION_SCALE) / 200;
            assert_eq!(u64::from(score(0, true, distance)), expected);
            assert_eq!(score(0, true, distance), score(0, false, distance));
        }
    }

    #[test]
    fn bounded_bonus_上限增强时命中节点距离单调不增() {
        let scores =
            [0, 50_000, 100_000, 200_000, 500_000, 1_000_000].map(|cap| score(cap, true, 120));
        assert!(scores.windows(2).all(|pair| pair[1] <= pair[0]));
        assert_eq!(score(1_000_000, false, 120), score(0, false, 120));
    }

    #[test]
    fn bounded_bonus_实际抵消量不超过配置比例() {
        let original = score(0, true, 120);
        for cap in [50_000, 100_000, 200_000, 500_000] {
            let bounded = score(cap, true, 120);
            let reduction = original - bounded;
            let allowed = u64::from(original) * u64::from(cap) / u64::from(NAVIGATION_SCALE);
            assert!(u64::from(reduction) <= allowed);
        }
    }
}
