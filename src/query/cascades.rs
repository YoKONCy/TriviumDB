//! TQL Cascades 查询优化器。
//!
//! 本模块把查询入口与每个 PipelineStage 放入 Memo Group，为逻辑表达式生成
//! 有界的物理候选，并结合节点数、属性统计、图统计、页读取和临时内存估算选择计划。
//! 优化过程强调确定性与预算可控：候选按稳定顺序比较，超过 group/expression
//! 上限时立即剪枝；它是统计感知的成本优化器，而非穷举意义上的全局最优搜索。

use super::tql_ast::{
    GraphAlgorithmKind, PipelineStage, Predicate, QueryEntry, ReturnClause, ReturnExprKind,
    TqlCompOp, TqlExpr, TqlLiteral, TqlQuery,
};
use crate::VectorType;
use crate::storage::memtable::MemTable;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

pub type GroupId = usize;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicalOperator {
    SearchSource,
    With,
    Expand,
    Filter,
    Rank,
    TextSearch,
    DppDiversify,
    FistaResidual,
    NmfTopics,
    SaPpr,
    GraphAlgorithm,
    Return,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationStatus {
    Complete,
    Fallback,
    BudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderingProperty {
    Unordered,
    NodeId,
    SimilarityDesc,
    PropertyAsc(String),
    PropertyDesc(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExactnessProperty {
    Exact,
    Approximate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterializationProperty {
    Streaming,
    Materialized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoreProperty {
    None,
    ApproximateSimilarity,
    ExactSimilarity,
    Graph,
    Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathProperty {
    Unavailable,
    Available,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhysicalProperties {
    pub ordering: OrderingProperty,
    pub exactness: ExactnessProperty,
    pub materialization: MaterializationProperty,
    pub available_columns: BTreeSet<String>,
    pub score: ScoreProperty,
    pub path: PathProperty,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicalOperator {
    ExactVectorSearch,
    QuiverVectorSearch,
    NodeScan,
    PropertyHashLookup,
    PropertyOrderedLookup,
    PropertyCompositeLookup,
    PropertyBitmapLookup,
    PropertyIndexIntersection,
    GraphFirstSource,
    TextFirstSource,
    ScopeProjection,
    GraphExpandSerial,
    GraphExpandParallel,
    GraphExpandIncoming,
    GraphExpandLabelDirectory,
    ExpandExactRerank,
    PayloadFilterScan,
    ScalarFilter,
    ExactRerankHeap,
    RankAlreadyOrdered,
    AnnExactRerank,
    DppDiversify,
    FistaResidualRecall,
    NmfTopics,
    SaPprDepthBounded,
    GraphAlgorithm,
    WeightedDijkstra,
    YenKShortestPaths,
    NodeSimilarity,
    ReturnProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct GroupExpression {
    pub operator: LogicalOperator,
    pub children: Vec<GroupId>,
    pub stage_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PhysicalAlternative {
    pub operator: PhysicalOperator,
    pub estimated_cost: f64,
    pub estimated_rows: usize,
    pub temp_bytes: usize,
    pub vector_page_reads: usize,
    pub payload_page_reads: usize,
    pub graph_page_reads: usize,
    pub exact: bool,
    pub materialized: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoGroup {
    pub id: GroupId,
    pub expressions: Vec<GroupExpression>,
    pub alternatives: Vec<PhysicalAlternative>,
    pub best_alternative: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedStage {
    pub stage_index: Option<usize>,
    pub operator: PhysicalOperator,
    pub properties: PhysicalProperties,
    pub estimated_rows: usize,
    pub estimated_cost: f64,
    pub temp_bytes: usize,
    pub vector_page_reads: usize,
    pub payload_page_reads: usize,
    pub graph_page_reads: usize,
    pub budget_bytes: usize,
    pub exact: bool,
    pub materialized: bool,
    pub estimate_source: EstimateSource,
    pub estimate_confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimateSource {
    Exact,
    IndexStatistics,
    CachedColumnPair,
    Sampled,
    Heuristic,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StatsRequirement {
    pub graph: bool,
    pub property: bool,
    pub column_pairs: bool,
    pub cross_modal: bool,
}

impl StatsRequirement {
    fn for_query(query: &TqlQuery) -> Self {
        let mut requirement = Self {
            graph: false,
            property: false,
            column_pairs: false,
            cross_modal: false,
        };
        let mut property_origin_available = false;
        for stage in &query.pipeline {
            match stage {
                PipelineStage::Expand(_)
                | PipelineStage::GraphAlgorithm(_)
                | PipelineStage::AllPaths(_)
                | PipelineStage::ShortestPaths(_)
                | PipelineStage::WeightedPaths(_)
                | PipelineStage::YenPaths(_)
                | PipelineStage::NodeSimilarity(_)
                | PipelineStage::Iterate(_) => {
                    requirement.graph = true;
                    requirement.cross_modal |= property_origin_available;
                    property_origin_available = false;
                }
                PipelineStage::Filter(predicate) => {
                    if predicate_is_property_only(predicate) {
                        requirement.property = true;
                        let mut fields = Vec::new();
                        collect_predicate_fields(predicate, &mut fields);
                        fields.sort();
                        fields.dedup();
                        requirement.column_pairs |= fields.len() > 1;
                    }
                    property_origin_available = predicate_property_origin(predicate).is_some();
                }
                PipelineStage::With(_) => {}
                PipelineStage::Rank(_)
                | PipelineStage::SetCombine(_)
                | PipelineStage::Diversify(_)
                | PipelineStage::Residual(_)
                | PipelineStage::Topics(_) => {
                    property_origin_available = false;
                }
                PipelineStage::SaPpr(_) => {
                    requirement.graph = true;
                    property_origin_available = false;
                }
            }
        }
        requirement
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanningProfile {
    pub total_ns: u64,
    pub stats_ns: u64,
    pub memo_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppliedRule {
    pub name: &'static str,
    pub stage_index: usize,
    pub applied: bool,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct CascadesPlan {
    pub status: OptimizationStatus,
    pub groups: Vec<MemoGroup>,
    pub stages: Vec<PlannedStage>,
    pub elided_stages: BTreeSet<usize>,
    pub merged_filter_pairs: BTreeSet<(usize, usize)>,
    pub exact_rerank_after: BTreeSet<usize>,
    pub rules: Vec<AppliedRule>,
    pub explored_expressions: usize,
    pub pruned_expressions: usize,
    pub total_estimated_cost: f64,
    pub estimated_memo_bytes: usize,
    pub stats_requirement: StatsRequirement,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planning_profile: Option<PlanningProfile>,
}

#[derive(Debug, Clone, Copy)]
pub struct OptimizerBudget {
    pub max_groups: usize,
    pub max_expressions: usize,
    pub query_memory_bytes: usize,
}

impl Default for OptimizerBudget {
    fn default() -> Self {
        Self {
            max_groups: 128,
            max_expressions: 512,
            query_memory_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Cascades 风格记忆化优化器。每个逻辑阶段进入 Memo Group，物理实现按成本竞争。
pub fn optimize_pipeline<T: VectorType>(
    query: &TqlQuery,
    mt: &MemTable<T>,
    budget: OptimizerBudget,
) -> CascadesPlan {
    optimize_pipeline_with_profile(query, mt, budget, false)
}

/// 生成 Cascades 计划，并按需采集规划阶段耗时。
pub fn optimize_pipeline_with_profile<T: VectorType>(
    query: &TqlQuery,
    mt: &MemTable<T>,
    budget: OptimizerBudget,
    profile: bool,
) -> CascadesPlan {
    let total_started = profile.then(Instant::now);
    let requirements = StatsRequirement::for_query(query);
    let stats_started = profile.then(Instant::now);
    let graph = requirements.graph.then(|| mt.graph_stats());
    let property_stats = if requirements.property {
        mt.property_index_stats()
            .into_iter()
            .map(|stats| (stats.field.clone(), stats))
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };
    let pair_stats = if requirements.column_pairs {
        mt.column_pair_stats()
            .into_iter()
            .map(|stats| {
                let key = ordered_field_pair(&stats.left_field, &stats.right_field);
                (key, stats)
            })
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };
    let stats_ns = stats_started.map_or(0, |started| elapsed_ns(started.elapsed()));
    let memo_started = profile.then(Instant::now);
    let mut input_origin = query_entry_property_origin(query);
    let mut groups = Vec::new();
    let mut stages = Vec::new();
    let mut status = OptimizationStatus::Complete;
    let mut exact_rerank_after = BTreeSet::new();
    let mut input_rows = source_rows(query, mt.node_count());
    let mut parent = None;
    let mut explored = 0usize;
    let mut pruned = 0usize;

    let source_alt = source_alternative(query, mt, input_rows);
    push_group(
        &mut groups,
        GroupExpression {
            operator: LogicalOperator::SearchSource,
            children: Vec::new(),
            stage_index: None,
        },
        vec![source_alt.clone()],
        &mut parent,
        budget,
        &mut explored,
        &mut pruned,
    );
    stages.push(planned(None, &source_alt, 0));

    for (stage_index, stage) in query.pipeline.iter().enumerate() {
        if groups.len() >= budget.max_groups || explored >= budget.max_expressions {
            status = if budget.max_groups == 0 || budget.max_expressions == 0 {
                OptimizationStatus::BudgetExceeded
            } else {
                OptimizationStatus::Fallback
            };
            pruned = pruned.saturating_add(query.pipeline.len().saturating_sub(stage_index));
            append_fallback_stages(query, stage_index, &mut stages, mt);
            break;
        }
        let (logical, mut alternatives) = match stage {
            PipelineStage::With(_) => (
                LogicalOperator::With,
                vec![PhysicalAlternative {
                    operator: PhysicalOperator::ScopeProjection,
                    estimated_cost: input_rows as f64 * 0.01,
                    estimated_rows: input_rows,
                    temp_bytes: 0,
                    vector_page_reads: 0,
                    payload_page_reads: 0,
                    graph_page_reads: 0,
                    exact: true,
                    materialized: false,
                }],
            ),
            PipelineStage::Expand(expand) => {
                let fallback_graph;
                let graph = if let Some(graph) = graph.as_ref() {
                    graph
                } else {
                    fallback_graph = empty_graph_stats();
                    &fallback_graph
                };
                let base_fanout = if expand.expand.labels.is_empty() {
                    histogram_fanout(graph)
                } else {
                    expand
                        .expand
                        .labels
                        .iter()
                        .filter_map(|label| graph.label_stats.get(label))
                        .map(|stats| {
                            stats.edge_count as f64 / stats.distinct_source_count.max(1) as f64
                        })
                        .sum::<f64>()
                        .max(1.0)
                };
                let fanout = input_origin
                    .as_ref()
                    .and_then(|(field, value)| mt.cross_modal_stats(field, value))
                    .filter(|stats| stats.sampled >= 2)
                    .map_or(base_fanout, |stats| {
                        (base_fanout * stats.fanout_skew)
                            .clamp(0.0, graph.max_out_degree.max(1) as f64)
                    });
                let hops = expand.expand.max_depth.max(1);
                let estimated = (input_rows as f64 * fanout.powi(hops.min(8) as i32))
                    .ceil()
                    .min(mt.node_count() as f64) as usize;
                (
                    LogicalOperator::Expand,
                    vec![PhysicalAlternative {
                        operator: expand_operator(expand, estimated),
                        estimated_cost: estimated as f64 * fanout,
                        estimated_rows: estimated,
                        temp_bytes: estimated
                            .saturating_mul(std::mem::size_of::<crate::query::pipeline::NodeRow>()),
                        vector_page_reads: 0,
                        payload_page_reads: 0,
                        graph_page_reads: pages(estimated.saturating_mul(32)),
                        exact: true,
                        materialized: true,
                    }],
                )
            }
            PipelineStage::Filter(predicate) => {
                let property_only = predicate_is_property_only(predicate);
                let selectivity = estimate_predicate_selectivity(
                    predicate,
                    mt.node_count(),
                    &property_stats,
                    &pair_stats,
                );
                let estimated = (input_rows as f64 * selectivity).ceil() as usize;
                let mut alternatives = vec![PhysicalAlternative {
                    operator: if property_only {
                        PhysicalOperator::PayloadFilterScan
                    } else {
                        PhysicalOperator::ScalarFilter
                    },
                    estimated_cost: input_rows as f64 * if property_only { 1.0 } else { 0.2 },
                    estimated_rows: estimated,
                    temp_bytes: 0,
                    vector_page_reads: 0,
                    payload_page_reads: usize::from(property_only).saturating_mul(input_rows),
                    graph_page_reads: 0,
                    exact: true,
                    materialized: false,
                }];
                if property_only {
                    for operator in indexed_filter_operators(predicate, mt) {
                        alternatives.push(PhysicalAlternative {
                            operator,
                            estimated_cost: estimated as f64 * 0.2 + 8.0,
                            estimated_rows: estimated,
                            temp_bytes: estimated.saturating_mul(std::mem::size_of::<u64>()),
                            vector_page_reads: 0,
                            payload_page_reads: estimated,
                            graph_page_reads: 0,
                            exact: true,
                            materialized: true,
                        });
                    }
                }
                (LogicalOperator::Filter, alternatives)
            }
            PipelineStage::Diversify(stage) => (
                LogicalOperator::DppDiversify,
                vec![PhysicalAlternative {
                    operator: PhysicalOperator::DppDiversify,
                    estimated_cost: input_rows as f64 * stage.top_k as f64,
                    estimated_rows: stage.top_k.min(input_rows),
                    temp_bytes: input_rows.saturating_mul(mt.dim()).saturating_mul(4),
                    vector_page_reads: pages(input_rows.saturating_mul(mt.dim()).saturating_mul(4)),
                    payload_page_reads: 0,
                    graph_page_reads: 0,
                    exact: false,
                    materialized: true,
                }],
            ),
            PipelineStage::Residual(stage) => (
                LogicalOperator::FistaResidual,
                vec![PhysicalAlternative {
                    operator: PhysicalOperator::FistaResidualRecall,
                    estimated_cost: input_rows as f64 * input_rows as f64 * stage.iterations as f64,
                    estimated_rows: stage.top_k.min(mt.node_count()),
                    temp_bytes: input_rows.saturating_mul(input_rows).saturating_mul(4),
                    vector_page_reads: pages(
                        mt.node_count().saturating_mul(mt.dim()).saturating_mul(4),
                    ),
                    payload_page_reads: 0,
                    graph_page_reads: 0,
                    exact: true,
                    materialized: true,
                }],
            ),
            PipelineStage::Topics(stage) => (
                LogicalOperator::NmfTopics,
                vec![PhysicalAlternative {
                    operator: PhysicalOperator::NmfTopics,
                    estimated_cost: input_rows as f64
                        * mt.dim() as f64
                        * stage.topics as f64
                        * stage.iterations as f64,
                    estimated_rows: input_rows,
                    temp_bytes: input_rows.saturating_mul(stage.topics).saturating_mul(4),
                    vector_page_reads: pages(input_rows.saturating_mul(mt.dim()).saturating_mul(4)),
                    payload_page_reads: 0,
                    graph_page_reads: 0,
                    exact: false,
                    materialized: true,
                }],
            ),
            PipelineStage::SaPpr(_) => (
                LogicalOperator::SaPpr,
                vec![PhysicalAlternative {
                    operator: PhysicalOperator::SaPprDepthBounded,
                    estimated_cost: input_rows as f64
                        * graph
                            .as_ref()
                            .map_or(1.0, |stats| stats.avg_out_degree.max(1.0)),
                    estimated_rows: mt.node_count().min(input_rows.saturating_mul(4)),
                    temp_bytes: input_rows
                        .saturating_mul(std::mem::size_of::<crate::query::pipeline::NodeRow>()),
                    vector_page_reads: 0,
                    payload_page_reads: 0,
                    graph_page_reads: pages(input_rows.saturating_mul(32)),
                    exact: false,
                    materialized: true,
                }],
            ),
            PipelineStage::GraphAlgorithm(stage) => {
                let average_degree = graph
                    .as_ref()
                    .map_or(1.0, |stats| stats.avg_out_degree.max(1.0));
                let edge_work = input_rows as f64 * average_degree;
                let multiplier = match stage.algorithm {
                    GraphAlgorithmKind::TriangleCount => average_degree,
                    GraphAlgorithmKind::Hits => 20.0,
                    _ => 1.0,
                };
                let metric_bytes = match stage.algorithm {
                    GraphAlgorithmKind::TriangleCount | GraphAlgorithmKind::Hits => {
                        std::mem::size_of::<(u64, f64)>() * 2
                    }
                    _ => std::mem::size_of::<u64>(),
                };
                (
                    LogicalOperator::GraphAlgorithm,
                    vec![PhysicalAlternative {
                        operator: PhysicalOperator::GraphAlgorithm,
                        estimated_cost: edge_work * multiplier,
                        estimated_rows: input_rows,
                        temp_bytes: input_rows.saturating_mul(
                            std::mem::size_of::<crate::query::pipeline::NodeRow>()
                                .saturating_add(metric_bytes),
                        ),
                        vector_page_reads: 0,
                        payload_page_reads: 0,
                        graph_page_reads: pages(input_rows.saturating_mul(32)),
                        exact: true,
                        materialized: true,
                    }],
                )
            }
            PipelineStage::WeightedPaths(_) => (
                LogicalOperator::GraphAlgorithm,
                vec![PhysicalAlternative {
                    operator: PhysicalOperator::WeightedDijkstra,
                    estimated_cost: input_rows as f64
                        * graph
                            .as_ref()
                            .map_or(1.0, |stats| stats.avg_out_degree.max(1.0)),
                    estimated_rows: input_rows,
                    temp_bytes: input_rows
                        .saturating_mul(std::mem::size_of::<crate::query::pipeline::NodeRow>() * 2),
                    vector_page_reads: 0,
                    payload_page_reads: 0,
                    graph_page_reads: pages(input_rows.saturating_mul(48)),
                    exact: true,
                    materialized: true,
                }],
            ),
            PipelineStage::YenPaths(stage) => (
                LogicalOperator::GraphAlgorithm,
                vec![PhysicalAlternative {
                    operator: PhysicalOperator::YenKShortestPaths,
                    estimated_cost: input_rows as f64
                        * stage.k as f64
                        * graph
                            .as_ref()
                            .map_or(1.0, |stats| stats.avg_out_degree.max(1.0)),
                    estimated_rows: input_rows.saturating_mul(stage.k),
                    temp_bytes: input_rows
                        .saturating_mul(stage.k)
                        .saturating_mul(std::mem::size_of::<crate::query::pipeline::NodeRow>()),
                    vector_page_reads: 0,
                    payload_page_reads: 0,
                    graph_page_reads: pages(input_rows.saturating_mul(stage.k).saturating_mul(48)),
                    exact: true,
                    materialized: true,
                }],
            ),
            PipelineStage::NodeSimilarity(stage) => (
                LogicalOperator::GraphAlgorithm,
                vec![PhysicalAlternative {
                    operator: PhysicalOperator::NodeSimilarity,
                    estimated_cost: (input_rows.saturating_mul(input_rows.saturating_sub(1)) / 2)
                        as f64,
                    estimated_rows: stage.top_k,
                    temp_bytes: stage
                        .top_k
                        .saturating_mul(std::mem::size_of::<crate::query::pipeline::NodeRow>()),
                    vector_page_reads: 0,
                    payload_page_reads: 0,
                    graph_page_reads: pages(input_rows.saturating_mul(32)),
                    exact: true,
                    materialized: true,
                }],
            ),
            PipelineStage::AllPaths(_)
            | PipelineStage::ShortestPaths(_)
            | PipelineStage::SetCombine(_)
            | PipelineStage::Iterate(_) => (
                LogicalOperator::GraphAlgorithm,
                vec![PhysicalAlternative {
                    operator: PhysicalOperator::GraphAlgorithm,
                    estimated_cost: input_rows as f64
                        * graph
                            .as_ref()
                            .map_or(1.0, |stats| stats.avg_out_degree.max(1.0)),
                    estimated_rows: input_rows,
                    temp_bytes: input_rows
                        .saturating_mul(std::mem::size_of::<crate::query::pipeline::NodeRow>()),
                    vector_page_reads: 0,
                    payload_page_reads: 0,
                    graph_page_reads: pages(input_rows.saturating_mul(32)),
                    exact: true,
                    materialized: true,
                }],
            ),
            PipelineStage::Rank(rank) => (
                LogicalOperator::Rank,
                vec![PhysicalAlternative {
                    operator: PhysicalOperator::ExactRerankHeap,
                    estimated_cost: input_rows as f64 * mt.dim() as f64,
                    estimated_rows: input_rows.min(rank.top_k),
                    temp_bytes: input_rows.saturating_mul(std::mem::size_of::<(u64, f32)>()),
                    vector_page_reads: pages(
                        input_rows
                            .saturating_mul(mt.dim())
                            .saturating_mul(std::mem::size_of::<T>()),
                    ),
                    payload_page_reads: 0,
                    graph_page_reads: 0,
                    exact: true,
                    materialized: true,
                }],
            ),
        };
        if matches!(stage, PipelineStage::Expand(_))
            && matches!(
                query.pipeline.get(stage_index + 1),
                Some(PipelineStage::Rank(_))
            )
        {
            let mut fused = alternatives[0].clone();
            fused.operator = PhysicalOperator::ExpandExactRerank;
            fused.estimated_cost *= 0.8;
            fused.vector_page_reads = pages(
                fused
                    .estimated_rows
                    .saturating_mul(mt.dim())
                    .saturating_mul(std::mem::size_of::<T>()),
            );
            alternatives.push(fused);
        }
        if matches!(stage, PipelineStage::Rank(_))
            && stages.last().is_some_and(|previous| {
                previous.properties.ordering == OrderingProperty::SimilarityDesc
                    && previous.properties.exactness == ExactnessProperty::Exact
            })
        {
            alternatives.push(PhysicalAlternative {
                operator: PhysicalOperator::RankAlreadyOrdered,
                estimated_cost: 0.0,
                estimated_rows: input_rows,
                temp_bytes: 0,
                vector_page_reads: 0,
                payload_page_reads: 0,
                graph_page_reads: 0,
                exact: true,
                materialized: false,
            });
        }
        let best = alternatives
            .iter()
            .enumerate()
            .min_by(|left, right| {
                left.1
                    .estimated_cost
                    .total_cmp(&right.1.estimated_cost)
                    .then_with(|| left.0.cmp(&right.0))
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        explored = explored.saturating_add(alternatives.len());
        let selected = alternatives[best].clone();
        input_rows = selected.estimated_rows;
        match stage {
            PipelineStage::Filter(predicate) => {
                input_origin = predicate_property_origin(predicate);
            }
            PipelineStage::With(_) => {}
            _ => input_origin = None,
        }
        let child = parent.into_iter().collect();
        let group_id = groups.len();
        groups.push(MemoGroup {
            id: group_id,
            expressions: vec![GroupExpression {
                operator: logical,
                children: child,
                stage_index: Some(stage_index),
            }],
            alternatives,
            best_alternative: best,
        });
        parent = Some(group_id);
        let mut planned_stage = planned(Some(stage_index), &selected, 0);
        if let PipelineStage::Filter(predicate) = stage {
            if predicate_uses_pair_stats(predicate, &pair_stats) {
                planned_stage.estimate_source = EstimateSource::CachedColumnPair;
                planned_stage.estimate_confidence = 0.8;
            } else if predicate_has_index_stats(predicate, &property_stats) {
                planned_stage.estimate_source = EstimateSource::IndexStatistics;
                planned_stage.estimate_confidence = 0.9;
            } else {
                planned_stage.estimate_source = EstimateSource::Heuristic;
                planned_stage.estimate_confidence = 0.35;
            }
        }
        stages.push(planned_stage);

        if matches!(stage, PipelineStage::Expand(_))
            && downstream_requires_similarity(query, stage_index + 1)
        {
            exact_rerank_after.insert(stage_index);
            let rerank = PhysicalAlternative {
                operator: PhysicalOperator::ExactRerankHeap,
                estimated_cost: input_rows as f64 * mt.dim() as f64,
                estimated_rows: input_rows,
                temp_bytes: input_rows.saturating_mul(std::mem::size_of::<(u64, f32)>()),
                vector_page_reads: pages(
                    input_rows
                        .saturating_mul(mt.dim())
                        .saturating_mul(std::mem::size_of::<T>()),
                ),
                payload_page_reads: 0,
                graph_page_reads: 0,
                exact: true,
                materialized: true,
            };
            stages.push(planned(Some(stage_index), &rerank, 0));
        }
    }

    let total_weight = stages
        .iter()
        .map(|stage| stage.temp_bytes.max(4096))
        .sum::<usize>()
        .max(1);
    for stage in &mut stages {
        stage.budget_bytes = budget
            .query_memory_bytes
            .saturating_mul(stage.temp_bytes.max(4096))
            / total_weight;
    }
    let rules = evaluate_rules(query);
    let elided_stages = rules
        .iter()
        .filter(|rule| rule.applied && rule.name == "eliminate_identity_with")
        .map(|rule| rule.stage_index)
        .collect();
    let merged_filter_pairs = rules
        .iter()
        .filter(|rule| rule.applied && rule.name == "merge_adjacent_filters")
        .map(|rule| (rule.stage_index, rule.stage_index + 1))
        .collect();
    let total_estimated_cost = stages.iter().map(|stage| stage.estimated_cost).sum();
    let estimated_memo_bytes = estimate_memo_bytes(&groups);
    let memo_ns = memo_started.map_or(0, |started| elapsed_ns(started.elapsed()));
    let planning_profile = total_started.map(|started| PlanningProfile {
        total_ns: elapsed_ns(started.elapsed()),
        stats_ns,
        memo_ns,
    });
    CascadesPlan {
        status,
        groups,
        stages,
        elided_stages,
        merged_filter_pairs,
        exact_rerank_after,
        rules,
        explored_expressions: explored,
        pruned_expressions: pruned,
        total_estimated_cost,
        estimated_memo_bytes,
        stats_requirement: requirements,
        planning_profile,
    }
}

fn elapsed_ns(elapsed: std::time::Duration) -> u64 {
    elapsed.as_nanos().min(u64::MAX as u128) as u64
}

fn estimate_memo_bytes(groups: &[MemoGroup]) -> usize {
    groups.iter().fold(0usize, |bytes, group| {
        bytes
            .saturating_add(std::mem::size_of::<MemoGroup>())
            .saturating_add(
                group
                    .expressions
                    .len()
                    .saturating_mul(std::mem::size_of::<GroupExpression>()),
            )
            .saturating_add(
                group
                    .alternatives
                    .len()
                    .saturating_mul(std::mem::size_of::<PhysicalAlternative>()),
            )
    })
}

fn empty_graph_stats() -> crate::storage::memtable::GraphStats {
    crate::storage::memtable::GraphStats {
        node_count: 0,
        edge_count: 0,
        isolated_node_count: 0,
        label_count: 0,
        avg_out_degree: 0.0,
        avg_in_degree: 0.0,
        max_out_degree: 0,
        max_in_degree: 0,
        label_stats: BTreeMap::new(),
        out_degree_histogram: Vec::new(),
        in_degree_histogram: Vec::new(),
    }
}

fn histogram_fanout(graph: &crate::storage::memtable::GraphStats) -> f64 {
    if graph.node_count == 0 || graph.edge_count == 0 {
        return 1.0;
    }
    let percentile_target = graph.node_count.saturating_mul(9).div_ceil(10);
    let mut cumulative = 0usize;
    let mut first_moment = 0.0;
    let mut second_moment = 0.0;
    let mut percentile = None;
    for bucket in &graph.out_degree_histogram {
        let representative = if bucket.upper_bound == usize::MAX {
            graph.max_out_degree
        } else {
            bucket.upper_bound
        };
        let degree = representative as f64;
        first_moment += degree * bucket.node_count as f64;
        second_moment += degree * degree * bucket.node_count as f64;
        cumulative = cumulative.saturating_add(bucket.node_count);
        if cumulative >= percentile_target && percentile.is_none() {
            percentile = Some(representative);
        }
    }
    let size_biased = if first_moment > 0.0 {
        second_moment / first_moment
    } else {
        graph.avg_out_degree
    };
    graph
        .avg_out_degree
        .max(percentile.unwrap_or(graph.max_out_degree) as f64)
        .max(size_biased)
        .min(graph.max_out_degree.max(1) as f64)
        .max(1.0)
}

fn evaluate_rules(query: &TqlQuery) -> Vec<AppliedRule> {
    let mut rules = Vec::new();
    for (index, stage) in query.pipeline.iter().enumerate() {
        if let PipelineStage::With(with) = stage {
            let identity = with.items.len() == 1
                && matches!(&with.items[0].expr, TqlExpr::Variable(variable) if variable == &with.items[0].alias);
            rules.push(AppliedRule {
                name: "eliminate_identity_with",
                stage_index: index,
                applied: identity,
                reason: if identity {
                    "恒等 WITH 不改变作用域或可用列"
                } else {
                    "WITH 包含重命名或标量投影"
                },
            });
        }
        if matches!(stage, PipelineStage::Filter(_))
            && matches!(
                query.pipeline.get(index + 1),
                Some(PipelineStage::Filter(_))
            )
        {
            rules.push(AppliedRule {
                name: "merge_adjacent_filters",
                stage_index: index,
                applied: true,
                reason: "相邻过滤按 AND 合并且保持求值顺序",
            });
        }
        let next = query
            .pipeline
            .iter()
            .enumerate()
            .skip(index + 1)
            .find(|(_, candidate)| !matches!(candidate, PipelineStage::With(_)));
        let Some((next_index, next_stage)) = next else {
            continue;
        };
        match (stage, next_stage) {
            (PipelineStage::Expand(_), PipelineStage::Filter(predicate)) => {
                let property_only = predicate_is_property_only(predicate);
                rules.push(AppliedRule {
                    name: "push_filter_below_expand",
                    stage_index: next_index,
                    applied: false,
                    reason: if property_only {
                        "过滤目标是 EXPAND 输出变量，跨越扩展会改变语义"
                    } else {
                        "分数过滤依赖 EXPAND 输出列"
                    },
                });
            }
            (PipelineStage::Filter(predicate), PipelineStage::Expand(_)) => {
                rules.push(AppliedRule {
                    name: "retain_filter_before_expand",
                    stage_index: index,
                    applied: predicate_is_property_only(predicate),
                    reason: if predicate_is_property_only(predicate) {
                        "属性过滤已位于扩展前，可减少锚点"
                    } else {
                        "标量过滤保持原序以保护列依赖"
                    },
                });
            }
            (PipelineStage::Expand(_), PipelineStage::Rank(_)) => {
                rules.push(AppliedRule {
                    name: "rank_expand_commute",
                    stage_index: index,
                    applied: false,
                    reason: "RANK 与 EXPAND 不可交换：候选集合与 Recall 语义不同",
                });
            }
            (PipelineStage::Rank(_), PipelineStage::Expand(_)) => {
                rules.push(AppliedRule {
                    name: "retain_rank_before_expand",
                    stage_index: index,
                    applied: true,
                    reason: "原查询显式要求先精排锚点再扩展",
                });
            }
            _ => {}
        }
    }
    rules
}

fn push_group(
    groups: &mut Vec<MemoGroup>,
    expression: GroupExpression,
    alternatives: Vec<PhysicalAlternative>,
    parent: &mut Option<GroupId>,
    budget: OptimizerBudget,
    explored: &mut usize,
    pruned: &mut usize,
) {
    if groups.len() >= budget.max_groups || *explored >= budget.max_expressions {
        *pruned += 1;
        return;
    }
    *explored += alternatives.len();
    let best = alternatives
        .iter()
        .enumerate()
        .min_by(|left, right| left.1.estimated_cost.total_cmp(&right.1.estimated_cost))
        .map(|(index, _)| index)
        .unwrap_or(0);
    let id = groups.len();
    groups.push(MemoGroup {
        id,
        expressions: vec![expression],
        alternatives,
        best_alternative: best,
    });
    *parent = Some(id);
}

fn planned(
    stage_index: Option<usize>,
    alternative: &PhysicalAlternative,
    budget_bytes: usize,
) -> PlannedStage {
    PlannedStage {
        stage_index,
        operator: alternative.operator.clone(),
        properties: properties_for(&alternative.operator),
        estimated_rows: alternative.estimated_rows,
        estimated_cost: alternative.estimated_cost,
        temp_bytes: alternative.temp_bytes,
        vector_page_reads: alternative.vector_page_reads,
        payload_page_reads: alternative.payload_page_reads,
        graph_page_reads: alternative.graph_page_reads,
        budget_bytes,
        exact: alternative.exact,
        materialized: alternative.materialized,
        estimate_source: estimate_source_for(&alternative.operator),
        estimate_confidence: estimate_confidence_for(&alternative.operator),
    }
}

fn estimate_source_for(operator: &PhysicalOperator) -> EstimateSource {
    match operator {
        PhysicalOperator::ExactVectorSearch
        | PhysicalOperator::QuiverVectorSearch
        | PhysicalOperator::NodeScan
        | PhysicalOperator::ScopeProjection
        | PhysicalOperator::RankAlreadyOrdered => EstimateSource::Exact,
        PhysicalOperator::PropertyHashLookup
        | PhysicalOperator::PropertyOrderedLookup
        | PhysicalOperator::PropertyCompositeLookup
        | PhysicalOperator::PropertyBitmapLookup
        | PhysicalOperator::PropertyIndexIntersection
        | PhysicalOperator::GraphExpandIncoming
        | PhysicalOperator::GraphExpandLabelDirectory => EstimateSource::IndexStatistics,
        PhysicalOperator::GraphExpandSerial
        | PhysicalOperator::GraphExpandParallel
        | PhysicalOperator::ExpandExactRerank => EstimateSource::Sampled,
        _ => EstimateSource::Heuristic,
    }
}

fn estimate_confidence_for(operator: &PhysicalOperator) -> f32 {
    match estimate_source_for(operator) {
        EstimateSource::Exact => 1.0,
        EstimateSource::IndexStatistics => 0.9,
        EstimateSource::CachedColumnPair => 0.8,
        EstimateSource::Sampled => 0.7,
        EstimateSource::Heuristic => 0.35,
    }
}

fn source_rows(query: &TqlQuery, node_count: usize) -> usize {
    match &query.entry {
        QueryEntry::Search { top_k, .. } => (*top_k).min(node_count),
        QueryEntry::Text { clause } => clause.top_k.min(node_count),
        QueryEntry::Find { .. } | QueryEntry::Match { .. } | QueryEntry::OptionalMatch { .. } => {
            node_count
        }
    }
}

fn pages(bytes: usize) -> usize {
    bytes.div_ceil(4096)
}

fn downstream_requires_similarity(query: &TqlQuery, start: usize) -> bool {
    query.pipeline[start..].iter().any(|stage| match stage {
        PipelineStage::With(with) => with
            .items
            .iter()
            .any(|item| expr_has_similarity(&item.expr)),
        PipelineStage::Filter(predicate) => predicate_has_similarity(predicate),
        PipelineStage::Rank(_) => true,
        PipelineStage::Expand(_)
        | PipelineStage::GraphAlgorithm(_)
        | PipelineStage::Diversify(_)
        | PipelineStage::Residual(_)
        | PipelineStage::Topics(_)
        | PipelineStage::SaPpr(_)
        | PipelineStage::AllPaths(_)
        | PipelineStage::ShortestPaths(_)
        | PipelineStage::WeightedPaths(_)
        | PipelineStage::YenPaths(_)
        | PipelineStage::NodeSimilarity(_)
        | PipelineStage::SetCombine(_)
        | PipelineStage::Iterate(_) => false,
    }) || query
        .order_by
        .iter()
        .any(|order| expr_has_similarity(&order.expr))
        || matches!(&query.returns, ReturnClause::Expressions(items) if items.iter().any(|item| match &item.kind {
            ReturnExprKind::Scalar(expr) => expr_has_similarity(expr),
            ReturnExprKind::Aggregate(_, inner) => return_kind_has_similarity(inner),
            _ => false,
        }))
}

fn return_kind_has_similarity(kind: &ReturnExprKind) -> bool {
    match kind {
        ReturnExprKind::Scalar(expr) => expr_has_similarity(expr),
        ReturnExprKind::Aggregate(_, inner) => return_kind_has_similarity(inner),
        _ => false,
    }
}

fn expr_has_similarity(expr: &TqlExpr) -> bool {
    match expr {
        TqlExpr::Similarity { .. } => true,
        TqlExpr::Binary { left, right, .. } => {
            expr_has_similarity(left) || expr_has_similarity(right)
        }
        TqlExpr::Coalesce(values) => values.iter().any(expr_has_similarity),
        TqlExpr::IsNull { expr, .. } => expr_has_similarity(expr),
        _ => false,
    }
}

fn predicate_has_similarity(predicate: &Predicate) -> bool {
    match predicate {
        Predicate::Compare { left, right, .. } => {
            expr_has_similarity(left) || expr_has_similarity(right)
        }
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            predicate_has_similarity(left) || predicate_has_similarity(right)
        }
        Predicate::Not(inner) => predicate_has_similarity(inner),
        Predicate::DocFilter { .. } => false,
    }
}

fn predicate_is_property_only(predicate: &Predicate) -> bool {
    match predicate {
        Predicate::Compare { left, right, .. } => {
            matches!(left, TqlExpr::Property { .. } | TqlExpr::Literal(_))
                && matches!(right, TqlExpr::Property { .. } | TqlExpr::Literal(_))
        }
        Predicate::DocFilter { .. } => true,
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            predicate_is_property_only(left) && predicate_is_property_only(right)
        }
        Predicate::Not(inner) => predicate_is_property_only(inner),
    }
}

fn estimate_predicate_selectivity(
    predicate: &Predicate,
    node_count: usize,
    stats: &BTreeMap<String, crate::index::property::PropertyIndexStats>,
    pair_stats: &BTreeMap<(String, String), crate::index::property::ColumnPairStats>,
) -> f64 {
    match predicate {
        Predicate::Compare { left, .. } => {
            if let TqlExpr::Property { field, .. } = left
                && let Some(stats) = stats.get(field)
            {
                return (1.0 / stats.distinct_count.max(1) as f64)
                    .clamp(1.0 / node_count.max(1) as f64, 1.0);
            }
            0.25
        }
        Predicate::DocFilter { .. } => 0.1,
        Predicate::And(left, right) => {
            let left_selectivity =
                estimate_predicate_selectivity(left, node_count, stats, pair_stats);
            let right_selectivity =
                estimate_predicate_selectivity(right, node_count, stats, pair_stats);
            let independent = left_selectivity * right_selectivity;
            let dependency = predicate_property_field(left)
                .zip(predicate_property_field(right))
                .and_then(|(left, right)| pair_stats.get(&ordered_field_pair(left, right)))
                .filter(|pair| pair.sampled_rows > 0)
                .map_or(0.0, crate::index::property::ColumnPairStats::dependency);
            let dependent = left_selectivity.min(right_selectivity);
            (independent * (1.0 - dependency) + dependent * dependency).clamp(0.0, 1.0)
        }
        Predicate::Or(left, right) => {
            let left = estimate_predicate_selectivity(left, node_count, stats, pair_stats);
            let right = estimate_predicate_selectivity(right, node_count, stats, pair_stats);
            (left + right - left * right).clamp(0.0, 1.0)
        }
        Predicate::Not(inner) => (1.0
            - estimate_predicate_selectivity(inner, node_count, stats, pair_stats))
        .clamp(0.0, 1.0),
    }
}

fn predicate_uses_pair_stats(
    predicate: &Predicate,
    pair_stats: &BTreeMap<(String, String), crate::index::property::ColumnPairStats>,
) -> bool {
    match predicate {
        Predicate::And(left, right) => {
            predicate_property_field(left)
                .zip(predicate_property_field(right))
                .is_some_and(|(left, right)| {
                    pair_stats.contains_key(&ordered_field_pair(left, right))
                })
                || predicate_uses_pair_stats(left, pair_stats)
                || predicate_uses_pair_stats(right, pair_stats)
        }
        Predicate::Or(left, right) => {
            predicate_uses_pair_stats(left, pair_stats)
                || predicate_uses_pair_stats(right, pair_stats)
        }
        Predicate::Not(inner) => predicate_uses_pair_stats(inner, pair_stats),
        _ => false,
    }
}

fn predicate_has_index_stats(
    predicate: &Predicate,
    stats: &BTreeMap<String, crate::index::property::PropertyIndexStats>,
) -> bool {
    match predicate {
        Predicate::Compare { left, right, .. } => [left, right].iter().any(|expression| {
            matches!(expression, TqlExpr::Property { field, .. } if stats.contains_key(field))
        }),
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            predicate_has_index_stats(left, stats) || predicate_has_index_stats(right, stats)
        }
        Predicate::Not(inner) => predicate_has_index_stats(inner, stats),
        Predicate::DocFilter { .. } => false,
    }
}

fn ordered_field_pair(left: &str, right: &str) -> (String, String) {
    if left <= right {
        (left.to_owned(), right.to_owned())
    } else {
        (right.to_owned(), left.to_owned())
    }
}

fn predicate_property_field(predicate: &Predicate) -> Option<&str> {
    match predicate {
        Predicate::Compare {
            left: TqlExpr::Property { field, .. },
            op: TqlCompOp::Eq,
            right: TqlExpr::Literal(_),
        }
        | Predicate::Compare {
            left: TqlExpr::Literal(_),
            op: TqlCompOp::Eq,
            right: TqlExpr::Property { field, .. },
        } => Some(field),
        _ => None,
    }
}

fn literal_value(literal: &TqlLiteral) -> serde_json::Value {
    match literal {
        TqlLiteral::Int(value) => (*value).into(),
        TqlLiteral::Float(value) => serde_json::Number::from_f64(*value)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        TqlLiteral::Str(value) => value.clone().into(),
        TqlLiteral::Bool(value) => (*value).into(),
        TqlLiteral::Null => serde_json::Value::Null,
    }
}

fn predicate_property_origin(predicate: &Predicate) -> Option<(String, serde_json::Value)> {
    match predicate {
        Predicate::Compare {
            left: TqlExpr::Property { field, .. },
            op: TqlCompOp::Eq,
            right: TqlExpr::Literal(value),
        }
        | Predicate::Compare {
            left: TqlExpr::Literal(value),
            op: TqlCompOp::Eq,
            right: TqlExpr::Property { field, .. },
        } => Some((field.clone(), literal_value(value))),
        _ => None,
    }
}

fn query_entry_property_origin(_query: &TqlQuery) -> Option<(String, serde_json::Value)> {
    None
}

fn source_alternative<T: VectorType>(
    query: &TqlQuery,
    mt: &MemTable<T>,
    rows: usize,
) -> PhysicalAlternative {
    let operator = match &query.entry {
        QueryEntry::Search { .. } if mt.quiver().is_some() => PhysicalOperator::QuiverVectorSearch,
        QueryEntry::Search { .. } => PhysicalOperator::ExactVectorSearch,
        QueryEntry::Find { filter } => match filter {
            crate::filter::Filter::Eq(field, _) if mt.has_property_index(field) => {
                PhysicalOperator::PropertyHashLookup
            }
            _ => PhysicalOperator::NodeScan,
        },
        QueryEntry::Match { pattern } | QueryEntry::OptionalMatch { pattern }
            if pattern
                .nodes
                .first()
                .and_then(|node| node.filter.as_ref())
                .is_some() =>
        {
            PhysicalOperator::GraphFirstSource
        }
        QueryEntry::Text { .. } => PhysicalOperator::TextFirstSource,
        QueryEntry::Match { .. } | QueryEntry::OptionalMatch { .. } => PhysicalOperator::NodeScan,
    };
    let approximate = operator == PhysicalOperator::QuiverVectorSearch;
    let text_source = operator == PhysicalOperator::TextFirstSource;
    PhysicalAlternative {
        operator,
        estimated_cost: if approximate {
            rows.max(1).ilog2() as f64 * mt.dim() as f64
        } else if text_source {
            rows as f64
        } else {
            mt.node_count() as f64 * mt.dim().max(1) as f64
        },
        estimated_rows: rows,
        temp_bytes: rows.saturating_mul(std::mem::size_of::<crate::query::pipeline::NodeRow>()),
        vector_page_reads: pages(
            mt.node_count()
                .saturating_mul(mt.dim())
                .saturating_mul(std::mem::size_of::<T>()),
        ),
        payload_page_reads: 0,
        graph_page_reads: 0,
        exact: !approximate,
        materialized: true,
    }
}

fn expand_operator(
    expand: &super::tql_ast::PipelineExpandStage,
    estimated_rows: usize,
) -> PhysicalOperator {
    if expand.expand.direction == super::tql_ast::EdgeDirection::Backward {
        PhysicalOperator::GraphExpandIncoming
    } else if !expand.expand.labels.is_empty() {
        PhysicalOperator::GraphExpandLabelDirectory
    } else if estimated_rows >= 4096 {
        PhysicalOperator::GraphExpandParallel
    } else {
        PhysicalOperator::GraphExpandSerial
    }
}

fn indexed_filter_operators<T: VectorType>(
    predicate: &Predicate,
    mt: &MemTable<T>,
) -> Vec<PhysicalOperator> {
    let mut fields = Vec::new();
    collect_predicate_fields(predicate, &mut fields);
    fields.sort();
    fields.dedup();
    let mut output = Vec::new();
    if fields.iter().any(|field| mt.has_property_index(field)) {
        output.push(PhysicalOperator::PropertyHashLookup);
    }
    if fields
        .iter()
        .any(|field| mt.has_ordered_property_index(field))
    {
        output.push(PhysicalOperator::PropertyOrderedLookup);
    }
    if fields.len() > 1 && fields.iter().all(|field| mt.has_property_index(field)) {
        output.extend([
            PhysicalOperator::PropertyCompositeLookup,
            PhysicalOperator::PropertyBitmapLookup,
            PhysicalOperator::PropertyIndexIntersection,
        ]);
    }
    output
}

fn collect_predicate_fields(predicate: &Predicate, output: &mut Vec<String>) {
    match predicate {
        Predicate::Compare { left, right, .. } => {
            for expression in [left, right] {
                if let TqlExpr::Property { field, .. } = expression {
                    output.push(field.clone());
                }
            }
        }
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            collect_predicate_fields(left, output);
            collect_predicate_fields(right, output);
        }
        Predicate::Not(inner) => collect_predicate_fields(inner, output),
        Predicate::DocFilter { .. } => {}
    }
}

fn properties_for(operator: &PhysicalOperator) -> PhysicalProperties {
    let (ordering, exactness, materialization, score, path) = match operator {
        PhysicalOperator::ExactVectorSearch | PhysicalOperator::ExactRerankHeap => (
            OrderingProperty::SimilarityDesc,
            ExactnessProperty::Exact,
            MaterializationProperty::Materialized,
            ScoreProperty::ExactSimilarity,
            PathProperty::Unavailable,
        ),
        PhysicalOperator::QuiverVectorSearch | PhysicalOperator::AnnExactRerank => (
            OrderingProperty::SimilarityDesc,
            ExactnessProperty::Approximate,
            MaterializationProperty::Materialized,
            ScoreProperty::ApproximateSimilarity,
            PathProperty::Unavailable,
        ),
        PhysicalOperator::GraphExpandSerial
        | PhysicalOperator::GraphExpandParallel
        | PhysicalOperator::GraphExpandIncoming
        | PhysicalOperator::GraphExpandLabelDirectory
        | PhysicalOperator::GraphFirstSource => (
            OrderingProperty::NodeId,
            ExactnessProperty::Exact,
            MaterializationProperty::Materialized,
            ScoreProperty::Graph,
            PathProperty::Unavailable,
        ),
        PhysicalOperator::GraphAlgorithm
        | PhysicalOperator::WeightedDijkstra
        | PhysicalOperator::YenKShortestPaths
        | PhysicalOperator::NodeSimilarity => (
            OrderingProperty::NodeId,
            ExactnessProperty::Exact,
            MaterializationProperty::Materialized,
            ScoreProperty::Graph,
            PathProperty::Available,
        ),
        PhysicalOperator::ExpandExactRerank => (
            OrderingProperty::SimilarityDesc,
            ExactnessProperty::Exact,
            MaterializationProperty::Materialized,
            ScoreProperty::ExactSimilarity,
            PathProperty::Unavailable,
        ),
        PhysicalOperator::RankAlreadyOrdered => (
            OrderingProperty::SimilarityDesc,
            ExactnessProperty::Exact,
            MaterializationProperty::Streaming,
            ScoreProperty::ExactSimilarity,
            PathProperty::Unavailable,
        ),
        _ => (
            OrderingProperty::NodeId,
            ExactnessProperty::Exact,
            MaterializationProperty::Streaming,
            ScoreProperty::None,
            PathProperty::Unavailable,
        ),
    };
    PhysicalProperties {
        ordering,
        exactness,
        materialization,
        available_columns: BTreeSet::from(["node_id".to_owned(), "payload".to_owned()]),
        score,
        path,
    }
}

fn fallback_alternative<T: VectorType>(
    stage: &PipelineStage,
    mt: &MemTable<T>,
) -> PhysicalAlternative {
    let operator = match stage {
        PipelineStage::With(_) => PhysicalOperator::ScopeProjection,
        PipelineStage::Expand(expand) => expand_operator(expand, mt.node_count()),
        PipelineStage::Filter(predicate) => {
            if predicate_is_property_only(predicate) {
                PhysicalOperator::PayloadFilterScan
            } else {
                PhysicalOperator::ScalarFilter
            }
        }
        PipelineStage::Rank(_) => PhysicalOperator::ExactRerankHeap,
        _ => PhysicalOperator::GraphAlgorithm,
    };
    PhysicalAlternative {
        operator,
        estimated_cost: mt.node_count() as f64,
        estimated_rows: mt.node_count(),
        temp_bytes: mt
            .node_count()
            .saturating_mul(std::mem::size_of::<crate::query::pipeline::NodeRow>()),
        vector_page_reads: 0,
        payload_page_reads: 0,
        graph_page_reads: 0,
        exact: true,
        materialized: true,
    }
}

fn append_fallback_stages<T: VectorType>(
    query: &TqlQuery,
    start: usize,
    stages: &mut Vec<PlannedStage>,
    mt: &MemTable<T>,
) {
    for (stage_index, stage) in query.pipeline.iter().enumerate().skip(start) {
        let alternative = fallback_alternative(stage, mt);
        stages.push(planned(Some(stage_index), &alternative, 0));
    }
}
