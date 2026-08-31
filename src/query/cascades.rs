//! TQL Cascades 查询优化器。
//!
//! 本模块把查询入口与每个 PipelineStage 放入 Memo Group，为逻辑表达式生成
//! 有界的物理候选，并结合节点数、属性统计、图统计、页读取和临时内存估算选择计划。
//! 优化过程强调确定性与预算可控：候选按稳定顺序比较，超过 group/expression
//! 上限时立即剪枝；它是统计感知的成本优化器，而非穷举意义上的全局最优搜索。

use super::tql_ast::{
    PipelineStage, Predicate, QueryEntry, ReturnClause, ReturnExprKind, TqlCompOp, TqlExpr,
    TqlLiteral, TqlQuery,
};
use crate::VectorType;
use crate::storage::memtable::MemTable;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub type GroupId = usize;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicalOperator {
    SearchSource,
    With,
    Expand,
    Filter,
    Rank,
    GraphAlgorithm,
    Return,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicalOperator {
    ExactVectorSearch,
    ScopeProjection,
    GraphExpand,
    PayloadFilter,
    ScalarFilter,
    ExactRerank,
    GraphAlgorithm,
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
    pub estimated_rows: usize,
    pub estimated_cost: f64,
    pub temp_bytes: usize,
    pub vector_page_reads: usize,
    pub payload_page_reads: usize,
    pub graph_page_reads: usize,
    pub budget_bytes: usize,
    pub exact: bool,
    pub materialized: bool,
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
    pub groups: Vec<MemoGroup>,
    pub stages: Vec<PlannedStage>,
    pub exact_rerank_after: BTreeSet<usize>,
    pub rules: Vec<AppliedRule>,
    pub explored_expressions: usize,
    pub pruned_expressions: usize,
    pub total_estimated_cost: f64,
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
    let graph = mt.graph_stats();
    let property_stats = mt
        .property_index_stats()
        .into_iter()
        .map(|stats| (stats.field.clone(), stats))
        .collect::<BTreeMap<_, _>>();
    let pair_stats = mt
        .column_pair_stats()
        .into_iter()
        .map(|stats| {
            let key = ordered_field_pair(&stats.left_field, &stats.right_field);
            (key, stats)
        })
        .collect::<BTreeMap<_, _>>();
    let mut input_origin = query_entry_property_origin(query);
    let mut groups = Vec::new();
    let mut stages = Vec::new();
    let mut exact_rerank_after = BTreeSet::new();
    let mut input_rows = source_rows(query, mt.node_count());
    let mut parent = None;
    let mut explored = 0usize;
    let mut pruned = 0usize;

    let source_alt = PhysicalAlternative {
        operator: PhysicalOperator::ExactVectorSearch,
        estimated_cost: mt.node_count() as f64 * mt.dim() as f64,
        estimated_rows: input_rows,
        temp_bytes: input_rows
            .saturating_mul(std::mem::size_of::<crate::query::pipeline::NodeRow>()),
        vector_page_reads: pages(
            mt.node_count()
                .saturating_mul(mt.dim())
                .saturating_mul(std::mem::size_of::<T>()),
        ),
        payload_page_reads: 0,
        graph_page_reads: 0,
        exact: true,
        materialized: true,
    };
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
            pruned = pruned.saturating_add(query.pipeline.len().saturating_sub(stage_index));
            break;
        }
        let (logical, alternatives) = match stage {
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
                let base_fanout = if expand.expand.labels.is_empty() {
                    histogram_fanout(&graph)
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
                    .filter(|stats| stats.generation == mt.generation() && stats.sampled >= 2)
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
                        operator: PhysicalOperator::GraphExpand,
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
                        PhysicalOperator::PayloadFilter
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
                if property_only && predicate_has_index(predicate, &property_stats) {
                    alternatives.push(PhysicalAlternative {
                        operator: PhysicalOperator::PayloadFilter,
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
                (LogicalOperator::Filter, alternatives)
            }
            PipelineStage::GraphAlgorithm(_)
            | PipelineStage::AllPaths(_)
            | PipelineStage::ShortestPaths(_)
            | PipelineStage::SetCombine(_)
            | PipelineStage::Iterate(_) => (
                LogicalOperator::GraphAlgorithm,
                vec![PhysicalAlternative {
                    operator: PhysicalOperator::GraphAlgorithm,
                    estimated_cost: input_rows as f64 * graph.avg_out_degree.max(1.0),
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
                    operator: PhysicalOperator::ExactRerank,
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
        stages.push(planned(Some(stage_index), &selected, 0));

        if matches!(stage, PipelineStage::Expand(_))
            && downstream_requires_similarity(query, stage_index + 1)
        {
            exact_rerank_after.insert(stage_index);
            let rerank = PhysicalAlternative {
                operator: PhysicalOperator::ExactRerank,
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
    let total_estimated_cost = stages.iter().map(|stage| stage.estimated_cost).sum();
    CascadesPlan {
        groups,
        stages,
        exact_rerank_after,
        rules,
        explored_expressions: explored,
        pruned_expressions: pruned,
        total_estimated_cost,
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
        estimated_rows: alternative.estimated_rows,
        estimated_cost: alternative.estimated_cost,
        temp_bytes: alternative.temp_bytes,
        vector_page_reads: alternative.vector_page_reads,
        payload_page_reads: alternative.payload_page_reads,
        graph_page_reads: alternative.graph_page_reads,
        budget_bytes,
        exact: alternative.exact,
        materialized: alternative.materialized,
    }
}

fn source_rows(query: &TqlQuery, node_count: usize) -> usize {
    match &query.entry {
        QueryEntry::Search { top_k, .. } => (*top_k).min(node_count),
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
        | PipelineStage::AllPaths(_)
        | PipelineStage::ShortestPaths(_)
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

fn predicate_has_index(
    predicate: &Predicate,
    stats: &BTreeMap<String, crate::index::property::PropertyIndexStats>,
) -> bool {
    match predicate {
        Predicate::Compare { left, .. } => {
            matches!(left, TqlExpr::Property { field, .. } if stats.contains_key(field))
        }
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            predicate_has_index(left, stats) || predicate_has_index(right, stats)
        }
        Predicate::Not(inner) => predicate_has_index(inner, stats),
        Predicate::DocFilter { .. } => false,
    }
}
