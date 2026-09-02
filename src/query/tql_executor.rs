//! TQL 统一查询执行器
//!
//! 将 TqlQuery AST 在 MemTable 上执行，支持三种入口：
//! - MATCH: 图模式匹配（含可变长路径、多标签边）
//! - FIND: 文档过滤扫描
//! - SEARCH: 向量检索 + 图扩散（桥接到现有管线）
//!
//! DML 写操作通过 execute_tql_mutation() 生成 MutationOp 指令，
//! 由 Database 层统一执行并写入 WAL。

use super::planner::{
    AccessPath, plan_filter, plan_filter_ordered, plan_filter_with_limit, plan_match,
};
use super::tql_ast::*;
use crate::VectorType;
use crate::error::TriviumError;
use crate::filter::Filter;
use crate::node::{Node, NodeId};
use crate::storage::memtable::MemTable;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct QueryControl {
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
}

impl QueryControl {
    pub fn with_deadline(deadline: Instant) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: Some(deadline),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn check(&self) -> Result<(), TriviumError> {
        if self.cancelled.load(Ordering::Acquire)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Err(TriviumError::QueryCancelled)
        } else {
            Ok(())
        }
    }
}

/// 兼容的节点查询结果：每行是一组变量绑定 → 节点快照。
pub type TqlResult<T> = Vec<HashMap<String, Node<T>>>;

/// TQL 一等查询值。
#[derive(Debug, Clone)]
pub enum TqlValue<T> {
    Node(Node<T>),
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Path(Vec<NodeId>),
    List(Vec<serde_json::Value>),
    Null,
}

impl<T> TqlValue<T> {
    /// 返回节点值；标量值返回 `None`。
    pub fn as_node(&self) -> Option<&Node<T>> {
        match self {
            Self::Node(node) => Some(node),
            _ => None,
        }
    }

    /// 消费并提取节点值；标量值原样返回。
    pub fn into_node(self) -> Result<Node<T>, Self> {
        match self {
            Self::Node(node) => Ok(node),
            value => Err(value),
        }
    }
}

/// 支持节点与标量列的统一查询结果。
pub type TqlValueResult<T> = Vec<HashMap<String, TqlValue<T>>>;

/// TQL 写操作结果
#[derive(Debug, Clone)]
pub struct TqlMutResult {
    /// 受影响的行数
    pub affected: usize,
    /// 新创建的节点 ID 列表
    pub created_ids: Vec<NodeId>,
}

/// 写操作指令（由执行器生成，Database 层应用）
#[derive(Debug, Clone)]
pub enum MutationOp<T: VectorType> {
    /// 创建节点：(变量名, 零向量, payload)
    InsertNode {
        var: String,
        vector: Vec<T>,
        payload: serde_json::Value,
    },
    /// 创建边
    LinkEdge {
        src_id: NodeId,
        dst_id: NodeId,
        src_var: String,
        dst_var: String,
        label: String,
        weight: f32,
    },
    /// 更新字段
    UpdatePayload {
        id: NodeId,
        payload: serde_json::Value,
    },
    /// 删除节点（detach=true 时自动断边）
    DeleteNode { id: NodeId, detach: bool },
}

/// 执行器步数预算；与结果行上限分离。
const MAX_BUDGET: usize = 100_000;
const DEFAULT_EDGE_ROW_LIMIT: usize = 5_000;

#[derive(Debug, Clone, Default)]
pub struct TqlLimits {
    pub max_query_rows: Option<usize>,
    pub row_overflow: crate::database::RowOverflowPolicy,
    pub memory_limit: usize,
    pub query_control: Option<QueryControl>,
}

impl TqlLimits {
    const UNLIMITED: Self = Self {
        max_query_rows: Some(0),
        row_overflow: crate::database::RowOverflowPolicy::Throw,
        memory_limit: 0,
        query_control: None,
    };

    fn check_cancelled(&self) -> Result<(), TriviumError> {
        self.query_control
            .as_ref()
            .map_or(Ok(()), QueryControl::check)
    }
}

fn memory_row_cap<T: VectorType>(memory_limit: usize, mt: &MemTable<T>) -> usize {
    if memory_limit == 0 {
        return usize::MAX;
    }
    let per_row = mt
        .dim()
        .saturating_mul(std::mem::size_of::<T>())
        .saturating_add(256)
        .max(1);
    memory_limit / per_row
}

fn query_has_edges(query: &TqlQuery) -> bool {
    matches!(
        &query.entry,
        QueryEntry::Match { pattern } | QueryEntry::OptionalMatch { pattern }
            if !pattern.edges.is_empty()
    )
}

fn requested_row_limit<T: VectorType>(
    query: &TqlQuery,
    requires_full_input: bool,
    limits: &TqlLimits,
    mt: &MemTable<T>,
) -> Result<(usize, bool), TriviumError> {
    let configured_cap = match limits.max_query_rows {
        Some(0) => usize::MAX,
        Some(value) => value,
        None if query_has_edges(query) => DEFAULT_EDGE_ROW_LIMIT,
        None => usize::MAX,
    };
    let memory_cap = memory_row_cap(limits.memory_limit, mt);
    let user_limited = !requires_full_input && query.limit.is_some();
    let requested = if user_limited {
        query
            .limit
            .unwrap_or(0)
            .checked_add(query.offset.unwrap_or(0))
            .ok_or(TriviumError::QueryRowBudgetExceeded { budget: memory_cap })?
    } else {
        configured_cap.min(memory_cap)
    };
    if user_limited && requested > memory_cap {
        return Err(TriviumError::QueryRowBudgetExceeded { budget: memory_cap });
    }
    Ok((requested, user_limited))
}

// ═══════════════════════════════════════════════════════════════════════
//  公开入口
// ═══════════════════════════════════════════════════════════════════════

/// 执行一个已解析的 TqlQuery
pub fn execute_tql<T: VectorType>(
    query: &TqlQuery,
    memtable: &MemTable<T>,
) -> Result<TqlResult<T>, TriviumError> {
    execute_tql_with_limits(query, memtable, TqlLimits::default())
}

fn ensure_search_vector_bound(query: &TqlQuery) -> Result<(), TriviumError> {
    if matches!(&query.entry, QueryEntry::Search { vector_parameters, .. } if !vector_parameters.is_empty())
    {
        return Err(TriviumError::QueryExecution(
            "SEARCH VECTOR 参数必须通过 Prepared TQL 绑定后执行 (SEARCH VECTOR parameters must be bound through Prepared TQL before execution)".into(),
        ));
    }
    Ok(())
}

pub fn execute_tql_with_limits<T: VectorType>(
    query: &TqlQuery,
    memtable: &MemTable<T>,
    limits: TqlLimits,
) -> Result<TqlResult<T>, TriviumError> {
    limits.check_cancelled()?;
    ensure_search_vector_bound(query)?;
    if query.explain {
        if query.analyze {
            let started = std::time::Instant::now();
            let mut executable = query.clone();
            executable.explain = false;
            executable.analyze = false;
            let rows = execute_tql_with_limits(&executable, memtable, limits)?;
            return Ok(generate_explain_plan(
                query,
                memtable,
                Some((rows.len(), started.elapsed().as_secs_f64() * 1000.0)),
            ));
        }
        return Ok(generate_explain_plan(query, memtable, None));
    }
    if !query.pipeline.is_empty() {
        return execute_pipeline_node_query(query, memtable);
    }

    let ordered_find = matches!(
        &query.entry,
        QueryEntry::Find { filter } if find_ordered_find_plan(query, filter, memtable).is_some()
    );
    let requires_full_input = query.rank.is_some()
        || (!query.order_by.is_empty() && !ordered_find)
        || matches!(&query.returns, ReturnClause::Expressions(exprs) if exprs.iter().any(|expr| is_aggregate(&expr.kind) || expr.distinct));
    let (row_limit, user_limited) =
        requested_row_limit(query, requires_full_input, &limits, memtable)?;
    let scan_limit = row_limit.saturating_add(1);

    let (mut results, ordered_output) = match &query.entry {
        QueryEntry::Find { filter } => execute_find(
            filter,
            query,
            memtable,
            scan_limit,
            limits.query_control.as_ref(),
        )?,
        QueryEntry::Match { pattern } => (
            execute_match(
                pattern,
                query,
                memtable,
                scan_limit,
                false,
                limits.query_control.as_ref(),
            )?,
            false,
        ),
        QueryEntry::OptionalMatch { pattern } => (
            execute_match(
                pattern,
                query,
                memtable,
                scan_limit,
                true,
                limits.query_control.as_ref(),
            )?,
            false,
        ),
        QueryEntry::Search {
            vector,
            top_k,
            expand,
            ..
        } => (
            execute_search(vector, *top_k, expand.as_ref(), query, memtable, scan_limit)?,
            false,
        ),
    };

    let truncated = results.len() > row_limit;
    if truncated {
        results.truncate(row_limit);
    }
    if truncated && !user_limited {
        let message =
            format!("TQL 结果超过 {row_limit} 行上限 (TQL result exceeds row limit {row_limit})");
        if requires_full_input || limits.row_overflow == crate::database::RowOverflowPolicy::Throw {
            return Err(TriviumError::QueryExecution(message));
        }
        tracing::warn!("{}；返回明确的部分结果", message);
    }

    if let Some(rank) = &query.rank {
        results = apply_graph_first_rank(results, rank, memtable)?;
    }

    results = apply_aggregation_and_distinct(
        &query.returns,
        results,
        matches!(&query.entry, QueryEntry::Find { .. }),
    )?;

    if !query.order_by.is_empty() && !ordered_output {
        sort_results(&mut results, &query.order_by, memtable);
    }

    // OFFSET 偏移
    if let Some(off) = query.offset {
        if off < results.len() {
            results = results.into_iter().skip(off).collect();
        } else {
            results.clear();
        }
    }

    // LIMIT 截断（排序后再截断）
    if let Some(lim) = query.limit {
        results.truncate(lim);
    }

    // 投影裁剪：对仅属性引用的变量，剥离 vector + edges 节省内存
    apply_projection_pruning(&query.returns, &mut results);

    Ok(results)
}

fn test_disable_fusion() -> bool {
    #[cfg(feature = "test-hooks")]
    {
        return matches!(
            crate::test_hooks::query_strategy(),
            crate::test_hooks::QueryExecutionStrategy::DisableFusion
        );
    }
    #[cfg(not(feature = "test-hooks"))]
    false
}

fn test_parallelism_budget() -> crate::query::parallel::QueryParallelismBudget {
    #[cfg(feature = "test-hooks")]
    {
        return match crate::test_hooks::query_strategy() {
            crate::test_hooks::QueryExecutionStrategy::ForceSerial => {
                crate::query::parallel::QueryParallelismBudget {
                    max_threads: 1,
                    min_parallel_rows: usize::MAX,
                }
            }
            crate::test_hooks::QueryExecutionStrategy::ForceParallel => {
                crate::query::parallel::QueryParallelismBudget {
                    max_threads: 2,
                    min_parallel_rows: 0,
                }
            }
            _ => crate::query::parallel::QueryParallelismBudget::default(),
        };
    }
    #[cfg(not(feature = "test-hooks"))]
    crate::query::parallel::QueryParallelismBudget::default()
}

fn execute_pipeline_set<T: VectorType>(
    query: &TqlQuery,
    mt: &MemTable<T>,
) -> Result<
    (
        crate::query::pipeline::NodeSet,
        String,
        HashMap<String, TqlExpr>,
    ),
    TriviumError,
> {
    let source_vector = match &query.entry {
        QueryEntry::Search { vector, .. } => Some(vector.as_slice()),
        _ => None,
    };
    let plan = crate::query::cascades::optimize_pipeline(
        query,
        mt,
        crate::query::cascades::OptimizerBudget::default(),
    );
    let mut operators: Vec<Box<dyn crate::query::pipeline::PipelineOperator<T>>> = match (
        &query.entry,
        plan.stages.first().map(|stage| &stage.operator),
    ) {
        (
            QueryEntry::Search {
                vector,
                top_k,
                expand,
                ..
            },
            Some(crate::query::cascades::PhysicalOperator::QuiverVectorSearch),
        ) => {
            let mut operators: Vec<Box<dyn crate::query::pipeline::PipelineOperator<T>>> =
                vec![Box::new(crate::query::pipeline::QuiverVectorSearch {
                    query: vector
                        .iter()
                        .map(|value| T::from_f32(*value as f32))
                        .collect(),
                    top_k: *top_k,
                    ef_search: top_k.saturating_mul(8).max(64),
                })];
            if let Some(expand) = expand {
                operators.extend(
                    crate::query::pipeline::lower_search_entry::<T>(vector, 0, Some(expand))
                        .into_iter()
                        .skip(1),
                );
            }
            operators
        }
        (
            QueryEntry::Search {
                vector,
                top_k,
                expand,
                ..
            },
            _,
        ) => crate::query::pipeline::lower_search_entry(vector, *top_k, expand.as_ref()),
        (
            QueryEntry::Find {
                filter: crate::filter::Filter::Eq(field, value),
            },
            Some(crate::query::cascades::PhysicalOperator::PropertyHashLookup),
        ) => vec![Box::new(crate::query::pipeline::PropertyLookup {
            field: field.clone(),
            value: value.clone(),
        })],
        _ => {
            let mut source_query = query.clone();
            source_query.pipeline.clear();
            source_query.predicate = None;
            source_query.rank = None;
            source_query.order_by.clear();
            source_query.limit = None;
            source_query.offset = None;
            source_query.returns = ReturnClause::All;
            let ids = execute_tql(&source_query, mt)?
                .into_iter()
                .flat_map(|row| row.into_values().map(|node| node.id))
                .collect();
            vec![Box::new(crate::query::pipeline::NodeIdsSource { ids })]
        }
    };
    let disable_fusion = cfg!(feature = "test-hooks") && test_disable_fusion();
    let mut current_name = "_".to_owned();
    let mut scalar_aliases = HashMap::<String, TqlExpr>::new();
    let physical_stage = |stage_index: usize| {
        plan.stages
            .iter()
            .find(|stage| stage.stage_index == Some(stage_index))
            .map(|stage| &stage.operator)
    };
    let mut skip_filter_stage = None;
    for (stage_index, stage) in query.pipeline.iter().enumerate() {
        if skip_filter_stage == Some(stage_index) {
            continue;
        }
        if plan.elided_stages.contains(&stage_index) {
            continue;
        }
        match stage {
            PipelineStage::With(with) => {
                let previous_aliases = scalar_aliases.clone();
                scalar_aliases.clear();
                for item in &with.items {
                    match &item.expr {
                        TqlExpr::Variable(var) if var == &current_name || var == "_" => {
                            current_name = item.alias.clone();
                        }
                        expr => {
                            scalar_aliases.insert(
                                item.alias.clone(),
                                substitute_scalar_aliases(expr, &previous_aliases),
                            );
                        }
                    }
                }
            }
            PipelineStage::Expand(stage) => {
                if stage.input != current_name {
                    return Err(TriviumError::QueryExecution(format!(
                        "EXPAND 输入 {} 不是当前 NodeSet {} (EXPAND input {} is not current NodeSet {})",
                        stage.input, current_name, stage.input, current_name
                    )));
                }
                let direction = match stage.expand.direction {
                    EdgeDirection::Forward => {
                        crate::graph::reachability::ReachabilityDirection::Outgoing
                    }
                    EdgeDirection::Backward => {
                        crate::graph::reachability::ReachabilityDirection::Incoming
                    }
                    EdgeDirection::Both => crate::graph::reachability::ReachabilityDirection::Both,
                };
                let expand = crate::query::pipeline::Expand {
                    min_depth: stage.expand.min_depth,
                    max_depth: stage.expand.max_depth,
                    labels: (!stage.expand.labels.is_empty()).then(|| stage.expand.labels.clone()),
                    direction,
                    include_input: false,
                };
                let fused = matches!(
                    physical_stage(stage_index),
                    Some(crate::query::cascades::PhysicalOperator::ExpandExactRerank)
                );
                if !disable_fusion
                    && fused
                    && let Some(PipelineStage::Rank(rank)) = query.pipeline.get(stage_index + 1)
                {
                    operators.push(Box::new(crate::query::pipeline::ExpandExactRerank {
                        expand,
                        query: rank
                            .vector
                            .iter()
                            .map(|value| T::from_f32(*value as f32))
                            .collect(),
                        top_k: rank.top_k,
                    }));
                } else {
                    operators.push(Box::new(expand));
                    if !disable_fusion && plan.exact_rerank_after.contains(&stage_index) {
                        let vector = source_vector.ok_or_else(|| {
                            TriviumError::QueryExecution(
                                "当前管线没有查询向量，不能计算 similarity() (Pipeline has no query vector for similarity())".into(),
                            )
                        })?;
                        operators.push(Box::new(crate::query::pipeline::ExactRerank {
                            query: vector
                                .iter()
                                .map(|value| T::from_f32(*value as f32))
                                .collect(),
                            top_k: None,
                        }));
                    }
                }
                current_name = stage.output.clone();
            }
            PipelineStage::GraphAlgorithm(graph) => {
                if graph.input != current_name {
                    return Err(TriviumError::QueryExecution(format!(
                        "图算法输入 {} 不是当前 NodeSet {} (Graph algorithm input {} is not current NodeSet {})",
                        graph.input, current_name, graph.input, current_name
                    )));
                }
                let mode = crate::query::pipeline::GraphSubsetMode::Induced;
                match graph.algorithm {
                    GraphAlgorithmKind::PageRank => {
                        operators.push(Box::new(crate::query::pipeline::PageRankOperator {
                            mode,
                            config: Default::default(),
                            label_filter: None,
                        }))
                    }
                    GraphAlgorithmKind::Wcc => {
                        operators.push(Box::new(crate::query::pipeline::WccOperator {
                            mode,
                            label_filter: None,
                        }))
                    }
                    GraphAlgorithmKind::Degree => {
                        operators.push(Box::new(crate::query::pipeline::DegreeCentralityOperator {
                            mode,
                            label_filter: None,
                        }))
                    }
                    GraphAlgorithmKind::LabelPropagation => {
                        operators.push(Box::new(crate::query::pipeline::LabelPropagationOperator {
                            mode,
                            config: crate::graph::subset::LabelPropagationConfig {
                                max_iterations: 32,
                                min_community_size: 1,
                            },
                            label_filter: None,
                        }))
                    }
                    GraphAlgorithmKind::Leiden => {
                        operators.push(Box::new(crate::query::pipeline::LeidenOperator {
                            mode,
                            config: crate::graph::leiden::LeidenConfig {
                                min_community_size: 1,
                                max_iterations: 32,
                                compute_centroids: false,
                            },
                        }))
                    }
                    GraphAlgorithmKind::SaPpr => {
                        operators.push(Box::new(crate::query::pipeline::SaPprOperator {
                            max_depth: 4,
                            restart_alpha: 0.15,
                            labels: None,
                            max_edges_per_node: 64,
                            min_edge_weight: 0.0,
                        }))
                    }
                }
                current_name = graph.output.clone();
            }
            PipelineStage::ShortestPaths(paths) => {
                if paths.input != current_name {
                    return Err(TriviumError::QueryExecution(format!(
                        "SHORTEST_PATHS 输入 {} 不是当前 NodeSet {} (SHORTEST_PATHS input {} is not current NodeSet {})",
                        paths.input, current_name, paths.input, current_name
                    )));
                }
                operators.push(Box::new(crate::query::pipeline::BatchShortestPaths {
                    targets: paths.targets.clone(),
                    label_filter: paths.label.clone(),
                }));
                current_name = paths.output.clone();
            }
            PipelineStage::SetCombine(combine) => {
                if combine.input != current_name {
                    return Err(TriviumError::QueryExecution(format!(
                        "集合运算输入 {} 不是当前 NodeSet {} (Set operation input {} is not current NodeSet {})",
                        combine.input, current_name, combine.input, current_name
                    )));
                }
                operators.push(Box::new(TqlSetCombineOperator {
                    ids: combine.other_ids.clone(),
                    operation: combine.operation,
                }));
                current_name = combine.output.clone();
            }
            PipelineStage::AllPaths(paths) => {
                if paths.input != current_name {
                    return Err(TriviumError::QueryExecution(format!(
                        "ALL_PATHS 输入 {} 不是当前 NodeSet {} (ALL_PATHS input {} is not current NodeSet {})",
                        paths.input, current_name, paths.input, current_name
                    )));
                }
                let aggregation = match paths.aggregation {
                    PathAggregation::MaxProduct => {
                        crate::query::pipeline::PathStrengthAggregation::MaxProduct
                    }
                    PathAggregation::SumProduct => {
                        crate::query::pipeline::PathStrengthAggregation::SumProduct
                    }
                    PathAggregation::AverageWeight => {
                        crate::query::pipeline::PathStrengthAggregation::AverageWeight
                    }
                };
                operators.push(Box::new(crate::query::pipeline::BoundedAllPaths {
                    targets: paths.targets.clone(),
                    config: crate::graph::pathfinding::BoundedPathConfig {
                        max_depth: paths.max_depth,
                        max_paths: paths.max_paths,
                        label_sequence: paths.label_sequence.clone(),
                        forbidden_nodes: paths.forbidden_nodes.iter().copied().collect(),
                    },
                    aggregation,
                }));
                current_name = paths.output.clone();
            }
            PipelineStage::Iterate(iterate) => {
                if iterate.input != current_name {
                    return Err(TriviumError::QueryExecution(format!(
                        "ITERATE 输入 {} 不是当前 NodeSet {} (ITERATE input {} is not current NodeSet {})",
                        iterate.input, current_name, iterate.input, current_name
                    )));
                }
                let direction = match iterate.expand.direction {
                    EdgeDirection::Forward => {
                        crate::graph::reachability::ReachabilityDirection::Outgoing
                    }
                    EdgeDirection::Backward => {
                        crate::graph::reachability::ReachabilityDirection::Incoming
                    }
                    EdgeDirection::Both => crate::graph::reachability::ReachabilityDirection::Both,
                };
                operators.push(Box::new(crate::query::pipeline::BoundedIterate {
                    operators: vec![Box::new(crate::query::pipeline::Expand {
                        min_depth: iterate.expand.min_depth,
                        max_depth: iterate.expand.max_depth,
                        labels: (!iterate.expand.labels.is_empty())
                            .then(|| iterate.expand.labels.clone()),
                        direction,
                        include_input: false,
                    })],
                    max_iterations: iterate.max_iterations,
                    stop_on_fixed_point: iterate.stop_on_fixed_point,
                }));
                current_name = iterate.output.clone();
            }
            PipelineStage::Filter(predicate) => {
                let predicate = if plan
                    .merged_filter_pairs
                    .contains(&(stage_index, stage_index + 1))
                {
                    let Some(PipelineStage::Filter(next)) = query.pipeline.get(stage_index + 1)
                    else {
                        return Err(TriviumError::QueryExecution(
                            "Cascades 相邻过滤计划与 AST 不一致".into(),
                        ));
                    };
                    skip_filter_stage = Some(stage_index + 1);
                    Predicate::And(Box::new(predicate.clone()), Box::new(next.clone()))
                } else {
                    predicate.clone()
                };
                let physical = physical_stage(stage_index);
                if let Some(strategy) = physical.and_then(property_index_strategy) {
                    let mut equalities = Vec::new();
                    collect_predicate_equalities(&predicate, &mut equalities);
                    if !equalities.is_empty() {
                        operators.push(Box::new(crate::query::pipeline::PropertyIndexFilter {
                            equalities,
                            strategy,
                        }));
                    }
                }
                operators.push(Box::new(PipelinePredicateFilter {
                    predicate: substitute_predicate_aliases(&predicate, &scalar_aliases),
                }));
            }
            PipelineStage::Rank(rank) => {
                // 与前一个 EXPAND 相邻时，已由 ExpandExactRerank 安全融合并执行，避免重复重排。
                if !matches!(
                    physical_stage(stage_index),
                    Some(crate::query::cascades::PhysicalOperator::RankAlreadyOrdered)
                ) && (disable_fusion
                    || !matches!(
                        stage_index.checked_sub(1).and_then(&physical_stage),
                        Some(crate::query::cascades::PhysicalOperator::ExpandExactRerank)
                    ))
                {
                    operators.push(Box::new(crate::query::pipeline::ExactRerank {
                        query: rank
                            .vector
                            .iter()
                            .map(|value| T::from_f32(*value as f32))
                            .collect(),
                        top_k: Some(rank.top_k),
                    }))
                }
            }
        }
    }
    let mut context = crate::query::pipeline::PipelineContext::new(
        mt,
        crate::query::pipeline::PipelineBudget {
            max_nodes: MAX_BUDGET,
            max_node_set_bytes: MAX_BUDGET
                .saturating_mul(std::mem::size_of::<crate::query::pipeline::NodeRow>()),
            max_vector_read_bytes: mt
                .node_count()
                .saturating_mul(mt.dim())
                .saturating_mul(std::mem::size_of::<T>())
                .saturating_mul(query.pipeline.len().saturating_add(1)),
            traversal: crate::graph::budget::TraversalBudget {
                max_visited_nodes: MAX_BUDGET,
                max_examined_edges: MAX_BUDGET,
                max_frontier_size: MAX_BUDGET,
                max_depth: 64,
                exhaustion_policy: crate::graph::budget::BudgetExhaustionPolicy::Error,
            },
            parallelism: test_parallelism_budget(),
            ..Default::default()
        },
    );
    let set = crate::query::pipeline::execute_pipeline(&mut context, &operators)?;
    Ok((set, current_name, scalar_aliases))
}

fn execute_pipeline_node_query<T: VectorType>(
    query: &TqlQuery,
    mt: &MemTable<T>,
) -> Result<TqlResult<T>, TriviumError> {
    if matches!(&query.returns, ReturnClause::Expressions(items) if items.iter().any(|item| matches!(item.kind, ReturnExprKind::Scalar(_))))
    {
        return Err(TriviumError::QueryExecution(
            "节点结果 API 无法承载标量列，请使用 tql_values (Node-only result API cannot carry scalar columns; use tql_values)".into(),
        ));
    }
    let (set, current_name, scalar_aliases) = execute_pipeline_set(query, mt)?;
    let mut pipeline_rows = set.rows().to_vec();
    sort_pipeline_rows(&mut pipeline_rows, &query.order_by, &scalar_aliases, mt);
    apply_offset_limit(&mut pipeline_rows, query.offset, query.limit);
    let results = pipeline_rows
        .iter()
        .filter_map(|pipeline_row| {
            let node = build_node(pipeline_row.id, mt)?;
            Some(HashMap::from([(current_name.clone(), node)]))
        })
        .collect();
    Ok(results)
}

fn json_to_tql_value<T>(value: serde_json::Value) -> TqlValue<T> {
    match value {
        serde_json::Value::Null => TqlValue::Null,
        serde_json::Value::Bool(value) => TqlValue::Bool(value),
        serde_json::Value::Number(value) => value.as_i64().map_or_else(
            || TqlValue::Float(value.as_f64().unwrap_or(f64::NAN)),
            TqlValue::Int,
        ),
        serde_json::Value::String(value) => TqlValue::String(value),
        serde_json::Value::Array(value) => TqlValue::List(value),
        serde_json::Value::Object(value) => {
            TqlValue::String(serde_json::to_string(&value).unwrap_or_else(|_| "{}".into()))
        }
    }
}

fn project_legacy_value_row<T: VectorType>(
    query: &TqlQuery,
    row: HashMap<String, Node<T>>,
    mt: &MemTable<T>,
) -> Result<HashMap<String, TqlValue<T>>, TriviumError> {
    let ReturnClause::Expressions(items) = &query.returns else {
        return Ok(row
            .into_iter()
            .map(|(name, node)| (name, TqlValue::Node(node)))
            .collect());
    };
    let is_find_entry = matches!(query.entry, QueryEntry::Find { .. });
    let mut output = HashMap::new();
    for item in items {
        let name = item
            .alias
            .clone()
            .unwrap_or_else(|| format_return_expr_kind(&item.kind));
        let value = match &item.kind {
            ReturnExprKind::Aggregate(_, _) => row
                .get(&name)
                .and_then(|node| node.payload.get(&name).cloned())
                .map_or(TqlValue::Null, json_to_tql_value),
            ReturnExprKind::Var(var) => {
                if is_find_entry
                    && let Some(node) = row.get(var)
                    && let Some(value) = node.payload.get(var).cloned()
                {
                    json_to_tql_value(value)
                } else {
                    row.get(var).cloned().map_or(TqlValue::Null, TqlValue::Node)
                }
            }
            ReturnExprKind::Property(var, field) => row
                .get(var)
                .and_then(|node| node.payload.get(field).cloned())
                .map_or(TqlValue::Null, json_to_tql_value),
            ReturnExprKind::Scalar(expr) => runtime_to_tql_value(match &query.entry {
                QueryEntry::Find { .. } => row.get("_").map_or(RuntimeValue::Null, |node| {
                    eval_tql_expr_single(expr, node.id, mt)
                }),
                _ => {
                    let mut names = row.keys().cloned().collect::<Vec<_>>();
                    names.sort();
                    let env = names
                        .iter()
                        .map(|name| row.get(name).map(|node| node.id))
                        .collect::<Vec<_>>();
                    let var_map = names
                        .into_iter()
                        .enumerate()
                        .map(|(index, name)| (name, index))
                        .collect::<HashMap<_, _>>();
                    eval_tql_expr(expr, &env, &var_map, mt)
                }
            }),
        };
        output.insert(name, value);
    }
    Ok(output)
}

/// 执行 TQL 并返回可同时承载节点与标量列的一等值结果。
pub fn execute_tql_values<T: VectorType>(
    query: &TqlQuery,
    mt: &MemTable<T>,
) -> Result<TqlValueResult<T>, TriviumError> {
    execute_tql_values_with_limits(query, mt, TqlLimits::default())
}

pub fn execute_tql_values_with_limits<T: VectorType>(
    query: &TqlQuery,
    mt: &MemTable<T>,
    limits: TqlLimits,
) -> Result<TqlValueResult<T>, TriviumError> {
    limits.check_cancelled()?;
    ensure_search_vector_bound(query)?;
    if query.pipeline.is_empty() {
        return execute_tql_with_limits(query, mt, limits)?
            .into_iter()
            .map(|row| project_legacy_value_row(query, row, mt))
            .collect();
    }
    let (set, current_name, scalar_aliases) = execute_pipeline_set(query, mt)?;
    let mut rows = set.rows().to_vec();
    if pipeline_has_aggregation(&query.returns) {
        let mut values =
            aggregate_pipeline_rows(&query.returns, &current_name, &scalar_aliases, &rows, mt)?;
        apply_offset_limit(&mut values, query.offset, query.limit);
        return Ok(values);
    }
    sort_pipeline_rows(&mut rows, &query.order_by, &scalar_aliases, mt);
    apply_offset_limit(&mut rows, query.offset, query.limit);
    rows.into_iter()
        .map(|pipeline_row| {
            project_pipeline_row(query, &current_name, &scalar_aliases, &pipeline_row, mt)
        })
        .collect()
}

fn sort_pipeline_rows<T: VectorType>(
    rows: &mut [crate::query::pipeline::NodeRow],
    order_by: &[OrderExpr],
    scalar_aliases: &HashMap<String, TqlExpr>,
    mt: &MemTable<T>,
) {
    if order_by.is_empty() {
        return;
    }
    rows.sort_by(|left, right| {
        for order in order_by {
            let expr = substitute_scalar_aliases(&order.expr, scalar_aliases);
            let ordering = compare_for_sort(
                &eval_pipeline_row_expr(&expr, left, mt),
                &eval_pipeline_row_expr(&expr, right, mt),
            );
            let ordering = if order.descending {
                ordering.reverse()
            } else {
                ordering
            };
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }
        }
        left.id.cmp(&right.id)
    });
}

fn apply_offset_limit<T>(rows: &mut Vec<T>, offset: Option<usize>, limit: Option<usize>) {
    if let Some(offset) = offset {
        if offset >= rows.len() {
            rows.clear();
        } else {
            rows.drain(..offset);
        }
    }
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
}

fn project_pipeline_row<T: VectorType>(
    query: &TqlQuery,
    current_name: &str,
    scalar_aliases: &HashMap<String, TqlExpr>,
    pipeline_row: &crate::query::pipeline::NodeRow,
    mt: &MemTable<T>,
) -> Result<HashMap<String, TqlValue<T>>, TriviumError> {
    let node = build_node(pipeline_row.id, mt).ok_or_else(|| {
        TriviumError::QueryExecution(format!(
            "管线节点 {} 不存在 (Pipeline node {} does not exist)",
            pipeline_row.id, pipeline_row.id
        ))
    })?;
    let mut output = HashMap::new();
    match &query.returns {
        ReturnClause::All => {
            output.insert(current_name.to_owned(), TqlValue::Node(node));
        }
        ReturnClause::Variables(vars) => {
            for var in vars {
                if var == current_name {
                    output.insert(var.clone(), TqlValue::Node(node.clone()));
                } else if let Some(expr) = scalar_aliases.get(var) {
                    output.insert(
                        var.clone(),
                        runtime_to_tql_value(eval_pipeline_row_expr(expr, pipeline_row, mt)),
                    );
                }
            }
        }
        ReturnClause::Expressions(items) => {
            for item in items {
                let (name, value) = match &item.kind {
                    ReturnExprKind::Var(var) if var == current_name => (
                        item.alias.clone().unwrap_or_else(|| var.clone()),
                        TqlValue::Node(node.clone()),
                    ),
                    ReturnExprKind::Var(alias) => {
                        let Some(expr) = scalar_aliases.get(alias) else {
                            return Err(TriviumError::QueryExecution(format!(
                                "RETURN 节点变量 {alias} 不是当前 NodeSet {current_name} (RETURN node variable {alias} is not current NodeSet {current_name})"
                            )));
                        };
                        (
                            item.alias.clone().unwrap_or_else(|| alias.clone()),
                            runtime_to_tql_value(eval_pipeline_row_expr(expr, pipeline_row, mt)),
                        )
                    }
                    ReturnExprKind::Property(var, field) => (
                        item.alias.clone().unwrap_or_else(|| field.clone()),
                        runtime_to_tql_value(eval_pipeline_row_expr(
                            &TqlExpr::Property {
                                var: var.clone(),
                                field: field.clone(),
                            },
                            pipeline_row,
                            mt,
                        )),
                    ),
                    ReturnExprKind::Scalar(expr) => (
                        item.alias.clone().unwrap_or_else(|| format_tql_expr(expr)),
                        runtime_to_tql_value(eval_pipeline_row_expr(expr, pipeline_row, mt)),
                    ),
                    ReturnExprKind::Aggregate(_, _) => {
                        return Err(TriviumError::QueryExecution(
                            "WITH 管线暂不支持聚合投影 (WITH pipeline aggregation is not supported yet)".into(),
                        ));
                    }
                };
                output.insert(name, value);
            }
        }
    }
    Ok(output)
}

fn pipeline_has_aggregation(returns: &ReturnClause) -> bool {
    matches!(returns, ReturnClause::Expressions(items) if items.iter().any(|item| is_aggregate(&item.kind)))
}

fn aggregate_pipeline_rows<T: VectorType>(
    returns: &ReturnClause,
    current_name: &str,
    scalar_aliases: &HashMap<String, TqlExpr>,
    rows: &[crate::query::pipeline::NodeRow],
    mt: &MemTable<T>,
) -> Result<TqlValueResult<T>, TriviumError> {
    let ReturnClause::Expressions(items) = returns else {
        return Ok(Vec::new());
    };
    let group_items = items
        .iter()
        .filter(|item| !is_aggregate(&item.kind))
        .collect::<Vec<_>>();
    let mut groups = BTreeMap::<String, Vec<&crate::query::pipeline::NodeRow>>::new();
    for row in rows {
        let key = group_items
            .iter()
            .map(|item| {
                pipeline_kind_json(&item.kind, current_name, scalar_aliases, row, mt).map(|value| {
                    serde_json::to_string(&value).unwrap_or_else(|_| "null".to_owned())
                })
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("\u{1f}");
        groups.entry(key).or_default().push(row);
    }
    if rows.is_empty() && group_items.is_empty() {
        groups.insert(String::new(), Vec::new());
    }
    groups
        .into_values()
        .map(|group| {
            let mut output = HashMap::new();
            for item in items {
                let name = item
                    .alias
                    .clone()
                    .unwrap_or_else(|| format_return_expr(item));
                let value = match &item.kind {
                    ReturnExprKind::Aggregate(func, inner) => aggregate_pipeline_value(
                        *func,
                        inner,
                        item.distinct,
                        current_name,
                        scalar_aliases,
                        &group,
                        mt,
                    )?,
                    kind => {
                        let Some(row) = group.first() else {
                            return Err(TriviumError::QueryExecution(
                                "空输入不能产生分组列 (Empty input cannot produce grouping columns)"
                                    .into(),
                            ));
                        };
                        pipeline_kind_value(kind, current_name, scalar_aliases, row, mt)?
                    }
                };
                output.insert(name, value);
            }
            Ok(output)
        })
        .collect()
}

fn aggregate_pipeline_value<T: VectorType>(
    func: AggFunc,
    inner: &ReturnExprKind,
    distinct: bool,
    current_name: &str,
    scalar_aliases: &HashMap<String, TqlExpr>,
    rows: &[&crate::query::pipeline::NodeRow],
    mt: &MemTable<T>,
) -> Result<TqlValue<T>, TriviumError> {
    let mut values = if matches!(inner, ReturnExprKind::Var(name) if name == "*") {
        rows.iter().map(|_| serde_json::Value::Bool(true)).collect()
    } else {
        rows.iter()
            .map(|row| pipeline_kind_json(inner, current_name, scalar_aliases, row, mt))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|value| !value.is_null())
            .collect::<Vec<_>>()
    };
    if distinct {
        let mut seen = HashSet::new();
        values.retain(|value| seen.insert(serde_json::to_string(value).unwrap_or_default()));
    }
    Ok(match func {
        AggFunc::Count => TqlValue::Int(values.len() as i64),
        AggFunc::Collect => TqlValue::List(values),
        AggFunc::Sum | AggFunc::Avg | AggFunc::Min | AggFunc::Max => {
            let numbers = values
                .iter()
                .filter_map(serde_json::Value::as_f64)
                .filter(|value| value.is_finite())
                .collect::<Vec<_>>();
            if numbers.is_empty() {
                TqlValue::Null
            } else {
                let value = match func {
                    AggFunc::Sum => numbers.iter().sum(),
                    AggFunc::Avg => numbers.iter().sum::<f64>() / numbers.len() as f64,
                    AggFunc::Min => numbers.into_iter().fold(f64::INFINITY, f64::min),
                    AggFunc::Max => numbers.into_iter().fold(f64::NEG_INFINITY, f64::max),
                    _ => {
                        return Err(TriviumError::QueryExecution(
                            "聚合函数状态不一致 (Inconsistent aggregate function state)".into(),
                        ));
                    }
                };
                TqlValue::Float(value)
            }
        }
    })
}

fn pipeline_kind_value<T: VectorType>(
    kind: &ReturnExprKind,
    current_name: &str,
    scalar_aliases: &HashMap<String, TqlExpr>,
    row: &crate::query::pipeline::NodeRow,
    mt: &MemTable<T>,
) -> Result<TqlValue<T>, TriviumError> {
    match kind {
        ReturnExprKind::Var(var) if var == current_name => build_node(row.id, mt)
            .map(TqlValue::Node)
            .ok_or_else(|| TriviumError::QueryExecution(format!("管线节点 {} 不存在", row.id))),
        ReturnExprKind::Var(alias) => scalar_aliases
            .get(alias)
            .map(|expr| runtime_to_tql_value(eval_pipeline_row_expr(expr, row, mt)))
            .ok_or_else(|| TriviumError::QueryExecution(format!("未知管线别名 {alias}"))),
        ReturnExprKind::Property(var, field) => Ok(runtime_to_tql_value(eval_pipeline_row_expr(
            &TqlExpr::Property {
                var: var.clone(),
                field: field.clone(),
            },
            row,
            mt,
        ))),
        ReturnExprKind::Scalar(expr) => {
            Ok(runtime_to_tql_value(eval_pipeline_row_expr(expr, row, mt)))
        }
        ReturnExprKind::Aggregate(_, _) => Err(TriviumError::QueryExecution(
            "不允许嵌套聚合 (Nested aggregation is not supported)".into(),
        )),
    }
}

fn pipeline_kind_json<T: VectorType>(
    kind: &ReturnExprKind,
    current_name: &str,
    scalar_aliases: &HashMap<String, TqlExpr>,
    row: &crate::query::pipeline::NodeRow,
    mt: &MemTable<T>,
) -> Result<serde_json::Value, TriviumError> {
    Ok(
        match pipeline_kind_value(kind, current_name, scalar_aliases, row, mt)? {
            TqlValue::Node(node) => serde_json::json!(node.id),
            TqlValue::Int(value) => serde_json::json!(value),
            TqlValue::Float(value) => serde_json::json!(value),
            TqlValue::String(value) => serde_json::json!(value),
            TqlValue::Bool(value) => serde_json::json!(value),
            TqlValue::Path(value) => serde_json::json!(value),
            TqlValue::List(value) => serde_json::Value::Array(value),
            TqlValue::Null => serde_json::Value::Null,
        },
    )
}

struct TqlSetCombineOperator {
    ids: Vec<NodeId>,
    operation: TqlSetOperation,
}

impl<T: VectorType> crate::query::pipeline::PipelineOperator<T> for TqlSetCombineOperator {
    fn name(&self) -> &'static str {
        "tql_set_combine"
    }

    fn apply(
        &self,
        input: crate::query::pipeline::NodeSet,
        context: &mut crate::query::pipeline::PipelineContext<'_, T>,
    ) -> Result<crate::query::pipeline::NodeSet, TriviumError> {
        if let Some(id) = self
            .ids
            .iter()
            .find(|id| context.memtable.get_payload(**id).is_none())
        {
            return Err(TriviumError::QueryExecution(format!(
                "集合运算节点 {id} 不存在 (Set operation node {id} does not exist)"
            )));
        }
        let operation = match self.operation {
            TqlSetOperation::Union => crate::query::pipeline::SetOperation::Union,
            TqlSetOperation::Intersect => crate::query::pipeline::SetOperation::Intersect,
            TqlSetOperation::Except => crate::query::pipeline::SetOperation::Difference,
        };
        Ok(crate::query::pipeline::combine_sets(
            input,
            crate::query::pipeline::NodeSet::from_ids(self.ids.iter().copied()),
            operation,
        ))
    }
}

fn property_index_strategy(
    operator: &crate::query::cascades::PhysicalOperator,
) -> Option<crate::query::pipeline::PropertyIndexStrategy> {
    use crate::query::cascades::PhysicalOperator;
    use crate::query::pipeline::PropertyIndexStrategy;
    match operator {
        PhysicalOperator::PropertyHashLookup => Some(PropertyIndexStrategy::Hash),
        PhysicalOperator::PropertyOrderedLookup => Some(PropertyIndexStrategy::Ordered),
        PhysicalOperator::PropertyCompositeLookup => Some(PropertyIndexStrategy::Composite),
        PhysicalOperator::PropertyBitmapLookup => Some(PropertyIndexStrategy::Bitmap),
        PhysicalOperator::PropertyIndexIntersection => Some(PropertyIndexStrategy::Intersection),
        _ => None,
    }
}

fn collect_predicate_equalities(
    predicate: &Predicate,
    output: &mut Vec<(String, serde_json::Value)>,
) {
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
        } => output.push((field.clone(), tql_literal_to_json(value))),
        Predicate::And(left, right) => {
            collect_predicate_equalities(left, output);
            collect_predicate_equalities(right, output);
        }
        _ => {}
    }
}

struct PipelinePredicateFilter {
    predicate: Predicate,
}

impl<T: VectorType> crate::query::pipeline::PipelineOperator<T> for PipelinePredicateFilter {
    fn name(&self) -> &'static str {
        "tql_pipeline_filter"
    }

    fn apply(
        &self,
        mut input: crate::query::pipeline::NodeSet,
        context: &mut crate::query::pipeline::PipelineContext<'_, T>,
    ) -> Result<crate::query::pipeline::NodeSet, TriviumError> {
        let rows = input
            .into_rows()
            .into_iter()
            .filter(|row| eval_pipeline_row_predicate(&self.predicate, row, context.memtable))
            .collect::<Vec<_>>();
        input = crate::query::pipeline::NodeSet::from_rows(rows);
        Ok(input)
    }
}

fn substitute_scalar_aliases(expr: &TqlExpr, aliases: &HashMap<String, TqlExpr>) -> TqlExpr {
    match expr {
        TqlExpr::Variable(alias) if aliases.contains_key(alias) => aliases[alias].clone(),
        TqlExpr::Binary { left, op, right } => TqlExpr::Binary {
            left: Box::new(substitute_scalar_aliases(left, aliases)),
            op: *op,
            right: Box::new(substitute_scalar_aliases(right, aliases)),
        },
        TqlExpr::Coalesce(values) => TqlExpr::Coalesce(
            values
                .iter()
                .map(|value| substitute_scalar_aliases(value, aliases))
                .collect(),
        ),
        TqlExpr::IsNull { expr, negated } => TqlExpr::IsNull {
            expr: Box::new(substitute_scalar_aliases(expr, aliases)),
            negated: *negated,
        },
        _ => expr.clone(),
    }
}

fn substitute_predicate_aliases(
    predicate: &Predicate,
    aliases: &HashMap<String, TqlExpr>,
) -> Predicate {
    match predicate {
        Predicate::Compare { left, op, right } => Predicate::Compare {
            left: substitute_scalar_aliases(left, aliases),
            op: *op,
            right: substitute_scalar_aliases(right, aliases),
        },
        Predicate::DocFilter { var, filter } => Predicate::DocFilter {
            var: var.clone(),
            filter: filter.clone(),
        },
        Predicate::And(left, right) => Predicate::And(
            Box::new(substitute_predicate_aliases(left, aliases)),
            Box::new(substitute_predicate_aliases(right, aliases)),
        ),
        Predicate::Or(left, right) => Predicate::Or(
            Box::new(substitute_predicate_aliases(left, aliases)),
            Box::new(substitute_predicate_aliases(right, aliases)),
        ),
        Predicate::Not(inner) => {
            Predicate::Not(Box::new(substitute_predicate_aliases(inner, aliases)))
        }
    }
}

fn eval_pipeline_row_expr<T: VectorType>(
    expr: &TqlExpr,
    row: &crate::query::pipeline::NodeRow,
    mt: &MemTable<T>,
) -> RuntimeValue {
    match expr {
        TqlExpr::Variable(_) => RuntimeValue::Int(row.id as i64),
        TqlExpr::Property { field, .. } => {
            if field == "id" {
                RuntimeValue::Int(row.id as i64)
            } else {
                mt.get_payload(row.id)
                    .map(|payload| json_to_runtime(&payload[field]))
                    .unwrap_or(RuntimeValue::Null)
            }
        }
        TqlExpr::Similarity { .. } => row
            .similarity
            .map(|score| RuntimeValue::Float(score.value as f64))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::GraphScore { .. } => row
            .graph_score
            .map(|score| RuntimeValue::Float(score.value as f64))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::Depth { .. } => row
            .provenance
            .min_depth
            .map(|depth| RuntimeValue::Int(depth as i64))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::PathStrength { .. } => row
            .path_strength
            .map(|score| RuntimeValue::Float(score.value as f64))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::PathCount { .. } => row
            .path_count
            .map(|count| RuntimeValue::Int(count as i64))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::Community { .. } => row
            .community_id
            .map(|community| RuntimeValue::Int(community as i64))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::Path { .. } => row
            .path
            .clone()
            .map(RuntimeValue::Path)
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::PathLength { .. } => row
            .path
            .as_ref()
            .map(|path| RuntimeValue::Int(path.len().saturating_sub(1) as i64))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::Binary { left, op, right } => eval_binary(
            eval_pipeline_row_expr(left, row, mt),
            *op,
            eval_pipeline_row_expr(right, row, mt),
        ),
        TqlExpr::Coalesce(values) => values
            .iter()
            .map(|value| eval_pipeline_row_expr(value, row, mt))
            .find(|value| !matches!(value, RuntimeValue::Null))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::IsNull { expr, negated } => RuntimeValue::Bool(
            matches!(eval_pipeline_row_expr(expr, row, mt), RuntimeValue::Null) != *negated,
        ),
        TqlExpr::Parameter(_) => RuntimeValue::Null,
        TqlExpr::Literal(literal) => lit_to_runtime(literal),
    }
}

fn eval_pipeline_row_predicate<T: VectorType>(
    predicate: &Predicate,
    row: &crate::query::pipeline::NodeRow,
    mt: &MemTable<T>,
) -> bool {
    match predicate {
        Predicate::Compare { left, op, right } => compare_runtime(
            &eval_pipeline_row_expr(left, row, mt),
            op,
            &eval_pipeline_row_expr(right, row, mt),
        ),
        Predicate::DocFilter { filter, .. } => mt
            .get_payload(row.id)
            .is_some_and(|payload| filter.matches(payload)),
        Predicate::And(left, right) => {
            eval_pipeline_row_predicate(left, row, mt)
                && eval_pipeline_row_predicate(right, row, mt)
        }
        Predicate::Or(left, right) => {
            eval_pipeline_row_predicate(left, row, mt)
                || eval_pipeline_row_predicate(right, row, mt)
        }
        Predicate::Not(inner) => !eval_pipeline_row_predicate(inner, row, mt),
    }
}

fn runtime_to_tql_value<T>(value: RuntimeValue) -> TqlValue<T> {
    match value {
        RuntimeValue::Int(value) => TqlValue::Int(value),
        RuntimeValue::Float(value) => TqlValue::Float(value),
        RuntimeValue::Str(value) => TqlValue::String(value),
        RuntimeValue::Bool(value) => TqlValue::Bool(value),
        RuntimeValue::Path(value) => TqlValue::Path(value),
        RuntimeValue::Null => TqlValue::Null,
    }
}

fn apply_graph_first_rank<T: VectorType>(
    rows: TqlResult<T>,
    rank: &RankClause,
    mt: &MemTable<T>,
) -> Result<TqlResult<T>, TriviumError> {
    let query_vector: Vec<T> = rank
        .vector
        .iter()
        .map(|value| T::from_f32(*value as f32))
        .collect();
    if query_vector.len() != mt.dim() {
        return Err(TriviumError::DimensionMismatch {
            expected: mt.dim(),
            got: query_vector.len(),
        });
    }
    let mut canonical = BTreeMap::<NodeId, (Vec<NodeId>, HashMap<String, Node<T>>)>::new();
    for row in rows {
        let anchor = row.get(&rank.var).ok_or_else(|| {
            TriviumError::QueryExecution(format!("RANK 变量 {} 未在 MATCH 中绑定", rank.var))
        })?;
        let mut bindings: Vec<(&str, NodeId)> = row
            .iter()
            .map(|(var, node)| (var.as_str(), node.id))
            .collect();
        bindings.sort_unstable();
        let binding_key: Vec<NodeId> = bindings.into_iter().map(|(_, id)| id).collect();
        match canonical.entry(anchor.id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert((binding_key, row));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if binding_key < entry.get().0 {
                    entry.insert((binding_key, row));
                }
            }
        }
    }
    if canonical.len() > MAX_BUDGET {
        return Err(TriviumError::QueryExecution(format!(
            "GraphFirst anchor 数量超过预算 {MAX_BUDGET}"
        )));
    }
    let mut ranked = Vec::with_capacity(canonical.len());
    for (anchor_id, (_, row)) in canonical {
        let vector = mt.get_vector(anchor_id).ok_or_else(|| {
            TriviumError::QueryExecution(format!("RANK anchor {anchor_id} 缺少向量"))
        })?;
        let score = T::similarity(&query_vector, vector);
        if score.is_finite() {
            ranked.push((score, anchor_id, row));
        }
    }
    ranked.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    ranked.truncate(rank.top_k);
    Ok(ranked.into_iter().map(|(_, _, row)| row).collect())
}

// ═══════════════════════════════════════════════════════════════════════
//  FIND 执行路径
// ═══════════════════════════════════════════════════════════════════════

fn execute_find<T: VectorType>(
    filter: &Filter,
    query: &TqlQuery,
    mt: &MemTable<T>,
    row_limit: usize,
    control: Option<&QueryControl>,
) -> Result<(TqlResult<T>, bool), TriviumError> {
    let mut results = Vec::new();
    let ordered_plan = find_ordered_find_plan(query, filter, mt);
    let ordered_output = ordered_plan.is_some();
    let candidate_limit = (query.predicate.is_none()
        && (filter_is_single_ordered_range(filter)
            || matches!(filter, Filter::Eq(field, _) if field != "id")))
    .then_some(row_limit);
    let access_plan = ordered_plan
        .clone()
        .unwrap_or_else(|| plan_filter_with_limit(filter, candidate_limit, mt));
    let full_scan = matches!(access_plan.access_path, AccessPath::FullNodeScan);
    let filter_covered = query.predicate.is_none()
        && matches!(
            (&access_plan.access_path, filter),
            (AccessPath::PropertyIndex { field: indexed }, Filter::Eq(field, _)) if indexed == field
        );
    let candidate_ids: Box<dyn Iterator<Item = NodeId> + '_> = if full_scan {
        Box::new(mt.active_node_ids())
    } else {
        Box::new(access_plan.candidates.into_iter())
    };
    for (scanned, id) in candidate_ids.enumerate() {
        if scanned & 0xff == 0
            && let Some(control) = control
        {
            control.check()?;
        }
        if results.len() >= row_limit {
            break;
        }

        let payload = match mt.get_payload(id) {
            Some(p) => p,
            None => continue,
        };

        // 索引已完整覆盖简单等值条件时，只读取 Payload 供投影使用。
        if !filter_covered && !filter.matches(payload) {
            continue;
        }

        // WHERE 二次过滤（FIND 场景下 var=None 作用于当前节点）
        if let Some(pred) = &query.predicate
            && !eval_predicate_single(pred, id, mt)
        {
            continue;
        }

        let node = match build_node(id, mt) {
            Some(n) => n,
            None => continue,
        };

        // RETURN 投影
        let mut row = HashMap::new();
        match &query.returns {
            ReturnClause::All => {
                row.insert("_".into(), node);
            }
            ReturnClause::Variables(vars) => {
                // FIND 场景下只有一个隐式节点，绑定到第一个变量
                if let Some(var) = vars.first() {
                    row.insert(var.clone(), node);
                }
            }
            ReturnClause::Expressions(exprs) => {
                // Expressions 场景：将隐式节点绑定到第一个变量引用
                if let Some(var) = extract_first_var_from_exprs(exprs) {
                    row.insert(var, node);
                } else {
                    row.insert("_".into(), node);
                }
            }
        }
        results.push(row);
    }

    Ok((results, ordered_output))
}
fn filter_is_single_ordered_range(filter: &Filter) -> bool {
    matches!(
        filter,
        Filter::Gt(..) | Filter::Gte(..) | Filter::Lt(..) | Filter::Lte(..) | Filter::Range(..)
    )
}

fn find_ordered_find_plan<T: VectorType>(
    query: &TqlQuery,
    filter: &Filter,
    mt: &MemTable<T>,
) -> Option<super::planner::NodeAccessPlan> {
    if query.order_by.len() != 1 || query.rank.is_some() {
        return None;
    }
    let order = &query.order_by[0];
    let TqlExpr::Property { field, .. } = &order.expr else {
        return None;
    };
    // ORDER BY 索引只能提供候选顺序；入口 Filter 和 WHERE 都可能继续淘汰候选。
    // 在过滤前按 OFFSET + LIMIT 截断会造成分页缺行，因此这里必须读取完整有序候选。
    plan_filter_ordered(filter, field, order.descending, None, mt)
}

// ═══════════════════════════════════════════════════════════════════════
//  MATCH 执行路径
// ═══════════════════════════════════════════════════════════════════════

fn execute_match<T: VectorType>(
    pattern: &TqlPattern,
    query: &TqlQuery,
    mt: &MemTable<T>,
    row_limit: usize,
    optional: bool,
    control: Option<&QueryControl>,
) -> Result<TqlResult<T>, TriviumError> {
    // 建立变量映射
    let mut var_map: HashMap<String, usize> = HashMap::new();
    for node_pat in &pattern.nodes {
        if let Some(var) = &node_pat.var {
            let next_idx = var_map.len();
            var_map.entry(var.clone()).or_insert(next_idx);
        }
    }

    // 确定返回变量
    let return_vars: Vec<String> = match &query.returns {
        ReturnClause::All => var_map.keys().cloned().collect(),
        ReturnClause::Variables(vars) => vars.clone(),
        ReturnClause::Expressions(exprs) => extract_vars_from_exprs(exprs),
    };

    for var in &return_vars {
        let next_idx = var_map.len();
        var_map.entry(var.clone()).or_insert(next_idx);
    }

    let match_plan = plan_match(pattern, mt, optional);
    let planned_pattern = &match_plan.pattern;
    let full_scan = matches!(match_plan.access_path, AccessPath::FullNodeScan);
    let start_candidates: Box<dyn Iterator<Item = NodeId> + '_> = if full_scan {
        Box::new(mt.active_node_ids())
    } else {
        Box::new(match_plan.candidates.into_iter())
    };

    let mut results = Vec::new();
    let mut budget: usize = 0;
    let step_budget = if pattern.edges.is_empty() {
        usize::MAX
    } else {
        MAX_BUDGET
    };

    for (scanned, start_id) in start_candidates.enumerate() {
        if scanned & 0xff == 0
            && let Some(control) = control
        {
            control.check()?;
        }
        let result_start = results.len();
        let mut env = vec![None; var_map.len()];
        let cont = tql_dfs(
            mt,
            planned_pattern,
            query.predicate.as_ref(),
            &return_vars,
            &var_map,
            0, // layer_idx
            &mut env,
            start_id,
            &mut results,
            &mut budget,
            step_budget,
            row_limit,
        )?;
        if !cont {
            break;
        }
        if optional && results.len() == result_start {
            let mut row = HashMap::new();
            if let Some(var) = &pattern.nodes[0].var
                && return_vars.contains(var)
                && let Some(node) = build_node(start_id, mt)
            {
                row.insert(var.clone(), node);
            }
            results.push(row);
            if results.len() >= row_limit {
                break;
            }
        }
    }

    Ok(results)
}

/// MATCH 的 DFS 遍历
fn tql_dfs<T: VectorType>(
    mt: &MemTable<T>,
    pattern: &TqlPattern,
    predicate: Option<&Predicate>,
    return_vars: &[String],
    var_map: &HashMap<String, usize>,
    layer_idx: usize,
    env: &mut Vec<Option<u64>>,
    current: u64,
    results: &mut TqlResult<T>,
    budget: &mut usize,
    max_budget: usize,
    row_limit: usize,
) -> Result<bool, TriviumError> {
    *budget += 1;
    if *budget > max_budget {
        return Err(TriviumError::QueryExecution(format!(
            "Query exceeded budget of {} steps",
            max_budget
        )));
    }

    let node_pat = &pattern.nodes[layer_idx];

    // 内联 Filter 校验（Q1-B: 支持 Mongo 操作符）
    if let Some(filter) = &node_pat.filter
        && !matches_filter_with_id(filter, current, mt)
    {
        return Ok(true); // 不匹配，剪枝
    }

    // 环境入栈
    let old_val = if let Some(var) = &node_pat.var {
        let idx = var_map[var];
        let old = env[idx];
        env[idx] = Some(current);
        Some((idx, old))
    } else {
        None
    };

    if layer_idx == pattern.edges.len() {
        // 路径收敛 → 评估 WHERE
        let passed = match predicate {
            Some(pred) => eval_predicate_env(pred, env, var_map, mt),
            None => true,
        };

        if passed {
            let mut row = HashMap::new();
            for var in return_vars {
                if let Some(&idx) = var_map.get(var)
                    && let Some(id) = env[idx]
                    && let Some(node) = build_node(id, mt)
                {
                    row.insert(var.clone(), node);
                }
            }
            results.push(row);
            if results.len() >= row_limit {
                if let Some((idx, old)) = old_val {
                    env[idx] = old;
                }
                return Ok(false);
            }
        }
    } else {
        let edge_pat = &pattern.edges[layer_idx];

        if let Some(hop) = &edge_pat.hop_range {
            // 可变长路径：使用 DFS 展开 [min..max] 跳
            let mut visited = HashSet::new();
            visited.insert(current);
            let cont = tql_dfs_variable_length(
                mt,
                pattern,
                predicate,
                return_vars,
                var_map,
                layer_idx,
                env,
                current,
                &edge_pat.labels,
                hop.min,
                hop.max,
                0,
                &mut visited,
                results,
                budget,
                max_budget,
                row_limit,
                edge_pat.direction,
            )?;
            if !cont {
                if let Some((idx, old)) = old_val {
                    env[idx] = old;
                }
                return Ok(false);
            }
        } else {
            // 单跳：根据方向遍历
            let neighbors = collect_neighbors(mt, current, &edge_pat.labels, edge_pat.direction);
            for next_id in neighbors {
                let cont = tql_dfs(
                    mt,
                    pattern,
                    predicate,
                    return_vars,
                    var_map,
                    layer_idx + 1,
                    env,
                    next_id,
                    results,
                    budget,
                    max_budget,
                    row_limit,
                )?;
                if !cont {
                    if let Some((idx, old)) = old_val {
                        env[idx] = old;
                    }
                    return Ok(false);
                }
            }
        }
    }

    // 环境回溯
    if let Some((idx, old)) = old_val {
        env[idx] = old;
    }

    Ok(true)
}

/// 可变长路径 DFS
fn tql_dfs_variable_length<T: VectorType>(
    mt: &MemTable<T>,
    pattern: &TqlPattern,
    predicate: Option<&Predicate>,
    return_vars: &[String],
    var_map: &HashMap<String, usize>,
    layer_idx: usize,
    env: &mut Vec<Option<u64>>,
    current: u64,
    labels: &[String],
    min_depth: usize,
    max_depth: usize,
    current_depth: usize,
    visited: &mut HashSet<u64>,
    results: &mut TqlResult<T>,
    budget: &mut usize,
    max_budget: usize,
    row_limit: usize,
    direction: EdgeDirection,
) -> Result<bool, TriviumError> {
    // 当前深度在有效范围内 → 继续到下一层（匹配后续节点模式）
    if current_depth >= min_depth {
        let cont = tql_dfs(
            mt,
            pattern,
            predicate,
            return_vars,
            var_map,
            layer_idx + 1,
            env,
            current,
            results,
            budget,
            max_budget,
            row_limit,
        )?;
        if !cont {
            return Ok(false);
        }
    }

    // 未达最大深度 → 继续展开
    if current_depth < max_depth {
        let neighbors = collect_neighbors(mt, current, labels, direction);
        for next in neighbors {
            if visited.contains(&next) {
                continue;
            }

            visited.insert(next);
            let cont = tql_dfs_variable_length(
                mt,
                pattern,
                predicate,
                return_vars,
                var_map,
                layer_idx,
                env,
                next,
                labels,
                min_depth,
                max_depth,
                current_depth + 1,
                visited,
                results,
                budget,
                max_budget,
                row_limit,
                direction,
            )?;
            visited.remove(&next);

            if !cont {
                return Ok(false);
            }
        }
    }

    Ok(true)
}

/// 根据方向收集邻居节点（带标签过滤）
fn collect_neighbors<T: VectorType>(
    mt: &MemTable<T>,
    current: u64,
    labels: &[String],
    direction: EdgeDirection,
) -> Vec<u64> {
    let mut neighbors = Vec::new();

    // 正向邻居：current 的出边目标
    if (direction == EdgeDirection::Forward || direction == EdgeDirection::Both)
        && let Some(edges) = mt.get_edges(current)
    {
        for edge in edges {
            if !labels.is_empty() && !labels.contains(&edge.label) {
                continue;
            }
            neighbors.push(edge.target_id);
        }
    }

    // 反向邻居：指向 current 的源节点
    if direction == EdgeDirection::Backward || direction == EdgeDirection::Both {
        for &src_id in mt.get_incoming_sources(current) {
            // 需要验证 src → current 的边是否匹配标签
            if labels.is_empty() {
                neighbors.push(src_id);
            } else if let Some(edges) = mt.get_edges(src_id) {
                for edge in edges {
                    if edge.target_id == current && labels.contains(&edge.label) {
                        neighbors.push(src_id);
                        break;
                    }
                }
            }
        }
    }

    neighbors
}

// ═══════════════════════════════════════════════════════════════════════
//  SEARCH 执行路径 (桥接到向量管线)
// ═══════════════════════════════════════════════════════════════════════

fn execute_search<T: VectorType>(
    vector: &[f64],
    top_k: usize,
    expand: Option<&ExpandClause>,
    query: &TqlQuery,
    mt: &MemTable<T>,
    row_limit: usize,
) -> Result<TqlResult<T>, TriviumError> {
    let operators = crate::query::pipeline::lower_search_entry(vector, top_k, expand);
    let mut context = crate::query::pipeline::PipelineContext::new(
        mt,
        crate::query::pipeline::PipelineBudget {
            max_nodes: MAX_BUDGET,
            max_node_set_bytes: MAX_BUDGET
                .saturating_mul(std::mem::size_of::<crate::query::pipeline::NodeRow>()),
            max_vector_read_bytes: mt
                .node_count()
                .saturating_mul(mt.dim())
                .saturating_mul(std::mem::size_of::<T>()),
            traversal: crate::graph::budget::TraversalBudget {
                max_visited_nodes: MAX_BUDGET,
                max_examined_edges: MAX_BUDGET,
                max_frontier_size: MAX_BUDGET,
                max_depth: expand.map_or(1, |clause| clause.max_depth),
                exhaustion_policy: crate::graph::budget::BudgetExhaustionPolicy::Error,
            },
            ..crate::query::pipeline::PipelineBudget::default()
        },
    );
    let candidates = crate::query::pipeline::execute_pipeline(&mut context, &operators)?;

    let mut results = Vec::new();
    for row_data in candidates.rows() {
        if results.len() >= row_limit {
            break;
        }
        let id = row_data.id;
        if let Some(pred) = &query.predicate
            && !eval_predicate_single(pred, id, mt)
        {
            continue;
        }
        if let Some(node) = build_node(id, mt) {
            let mut row = HashMap::new();
            row.insert("_".into(), node);
            results.push(row);
        }
    }
    Ok(results)
}

// ═══════════════════════════════════════════════════════════════════════
//  统一 Predicate 评估器
// ═══════════════════════════════════════════════════════════════════════

/// 在多变量环境下评估谓词（MATCH 场景）
fn eval_predicate_env<T: VectorType>(
    pred: &Predicate,
    env: &[Option<u64>],
    var_map: &HashMap<String, usize>,
    mt: &MemTable<T>,
) -> bool {
    match pred {
        Predicate::Compare { left, op, right } => {
            let lval = eval_tql_expr(left, env, var_map, mt);
            let rval = eval_tql_expr(right, env, var_map, mt);
            compare_runtime(&lval, op, &rval)
        }

        Predicate::DocFilter { var, filter } => {
            let id = match var {
                Some(v) => {
                    if let Some(&idx) = var_map.get(v) {
                        env[idx]
                    } else {
                        None
                    }
                }
                None => {
                    // 无变量绑定，尝试用第一个非空变量
                    env.iter().find(|o| o.is_some()).copied().flatten()
                }
            };

            match id {
                Some(nid) => match mt.get_payload(nid) {
                    Some(payload) => filter.matches(payload),
                    None => false,
                },
                None => false,
            }
        }

        Predicate::And(a, b) => {
            eval_predicate_env(a, env, var_map, mt) && eval_predicate_env(b, env, var_map, mt)
        }
        Predicate::Or(a, b) => {
            eval_predicate_env(a, env, var_map, mt) || eval_predicate_env(b, env, var_map, mt)
        }
        Predicate::Not(inner) => !eval_predicate_env(inner, env, var_map, mt),
    }
}

/// 在单节点上下文中评估谓词（FIND / SEARCH 场景）
fn eval_predicate_single<T: VectorType>(pred: &Predicate, id: NodeId, mt: &MemTable<T>) -> bool {
    match pred {
        Predicate::Compare { left, op, right } => {
            // 单节点场景下，属性访问的 var 被忽略，直接用当前 id
            let lval = eval_tql_expr_single(left, id, mt);
            let rval = eval_tql_expr_single(right, id, mt);
            compare_runtime(&lval, op, &rval)
        }

        Predicate::DocFilter { filter, .. } => match mt.get_payload(id) {
            Some(payload) => filter.matches(payload),
            None => false,
        },

        Predicate::And(a, b) => {
            eval_predicate_single(a, id, mt) && eval_predicate_single(b, id, mt)
        }
        Predicate::Or(a, b) => eval_predicate_single(a, id, mt) || eval_predicate_single(b, id, mt),
        Predicate::Not(inner) => !eval_predicate_single(inner, id, mt),
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  表达式求值 & 比较
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
enum RuntimeValue {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Path(Vec<NodeId>),
    Null,
}

fn eval_tql_expr<T: VectorType>(
    expr: &TqlExpr,
    env: &[Option<u64>],
    var_map: &HashMap<String, usize>,
    mt: &MemTable<T>,
) -> RuntimeValue {
    match expr {
        TqlExpr::Variable(_) | TqlExpr::Parameter(_) => RuntimeValue::Null,
        TqlExpr::Binary { left, op, right } => eval_binary(
            eval_tql_expr(left, env, var_map, mt),
            *op,
            eval_tql_expr(right, env, var_map, mt),
        ),
        TqlExpr::Coalesce(values) => values
            .iter()
            .map(|value| eval_tql_expr(value, env, var_map, mt))
            .find(|value| !matches!(value, RuntimeValue::Null))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::IsNull { expr, negated } => RuntimeValue::Bool(
            matches!(eval_tql_expr(expr, env, var_map, mt), RuntimeValue::Null) != *negated,
        ),
        TqlExpr::Path { .. } | TqlExpr::PathLength { .. } => RuntimeValue::Null,
        TqlExpr::Similarity { .. }
        | TqlExpr::GraphScore { .. }
        | TqlExpr::Depth { .. }
        | TqlExpr::PathStrength { .. }
        | TqlExpr::PathCount { .. }
        | TqlExpr::Community { .. } => RuntimeValue::Null,
        TqlExpr::Property { var, field } => {
            if let Some(&idx) = var_map.get(var)
                && let Some(id) = env[idx]
            {
                if field == "id" {
                    return RuntimeValue::Int(id as i64);
                }
                if let Some(payload) = mt.get_payload(id) {
                    return json_to_runtime(&payload[field]);
                }
            }
            RuntimeValue::Null
        }
        TqlExpr::Literal(lit) => lit_to_runtime(lit),
    }
}

fn eval_tql_expr_single<T: VectorType>(
    expr: &TqlExpr,
    id: NodeId,
    mt: &MemTable<T>,
) -> RuntimeValue {
    match expr {
        TqlExpr::Variable(_) | TqlExpr::Parameter(_) => RuntimeValue::Null,
        TqlExpr::Binary { left, op, right } => eval_binary(
            eval_tql_expr_single(left, id, mt),
            *op,
            eval_tql_expr_single(right, id, mt),
        ),
        TqlExpr::Coalesce(values) => values
            .iter()
            .map(|value| eval_tql_expr_single(value, id, mt))
            .find(|value| !matches!(value, RuntimeValue::Null))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::IsNull { expr, negated } => RuntimeValue::Bool(
            matches!(eval_tql_expr_single(expr, id, mt), RuntimeValue::Null) != *negated,
        ),
        TqlExpr::Similarity { .. }
        | TqlExpr::GraphScore { .. }
        | TqlExpr::Depth { .. }
        | TqlExpr::PathStrength { .. }
        | TqlExpr::PathCount { .. }
        | TqlExpr::Community { .. }
        | TqlExpr::Path { .. }
        | TqlExpr::PathLength { .. } => RuntimeValue::Null,
        TqlExpr::Property { field, .. } => {
            if field == "id" {
                return RuntimeValue::Int(id as i64);
            }
            if let Some(payload) = mt.get_payload(id) {
                return json_to_runtime(&payload[field]);
            }
            RuntimeValue::Null
        }
        TqlExpr::Literal(lit) => lit_to_runtime(lit),
    }
}

fn json_to_runtime(v: &serde_json::Value) -> RuntimeValue {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                RuntimeValue::Int(i)
            } else {
                RuntimeValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => RuntimeValue::Str(s.clone()),
        serde_json::Value::Bool(b) => RuntimeValue::Bool(*b),
        _ => RuntimeValue::Null,
    }
}

fn lit_to_runtime(lit: &TqlLiteral) -> RuntimeValue {
    match lit {
        TqlLiteral::Int(n) => RuntimeValue::Int(*n),
        TqlLiteral::Float(f) => RuntimeValue::Float(*f),
        TqlLiteral::Str(s) => RuntimeValue::Str(s.clone()),
        TqlLiteral::Bool(b) => RuntimeValue::Bool(*b),
        TqlLiteral::Null => RuntimeValue::Null,
    }
}

fn eval_binary(left: RuntimeValue, op: TqlBinaryOp, right: RuntimeValue) -> RuntimeValue {
    let left = match left {
        RuntimeValue::Int(value) => value as f64,
        RuntimeValue::Float(value) => value,
        _ => return RuntimeValue::Null,
    };
    let right = match right {
        RuntimeValue::Int(value) => value as f64,
        RuntimeValue::Float(value) => value,
        _ => return RuntimeValue::Null,
    };
    let value = match op {
        TqlBinaryOp::Add => left + right,
        TqlBinaryOp::Subtract => left - right,
        TqlBinaryOp::Multiply => left * right,
        TqlBinaryOp::Divide if right.abs() > f64::EPSILON => left / right,
        TqlBinaryOp::Divide => return RuntimeValue::Null,
    };
    if value.is_finite() {
        RuntimeValue::Float(value)
    } else {
        RuntimeValue::Null
    }
}

fn compare_runtime(lhs: &RuntimeValue, op: &TqlCompOp, rhs: &RuntimeValue) -> bool {
    match (lhs, rhs) {
        (RuntimeValue::Int(a), RuntimeValue::Int(b)) => cmp_ord(a, op, b),
        (RuntimeValue::Float(a), RuntimeValue::Float(b)) => cmp_f64(*a, op, *b),
        (RuntimeValue::Int(a), RuntimeValue::Float(b)) => cmp_f64(*a as f64, op, *b),
        (RuntimeValue::Float(a), RuntimeValue::Int(b)) => cmp_f64(*a, op, *b as f64),
        (RuntimeValue::Str(a), RuntimeValue::Str(b)) => cmp_ord(a, op, b),
        (RuntimeValue::Bool(a), RuntimeValue::Bool(b)) => match op {
            TqlCompOp::Eq => a == b,
            TqlCompOp::Ne => a != b,
            _ => false,
        },
        _ => false,
    }
}

fn cmp_ord<T: Ord>(a: &T, op: &TqlCompOp, b: &T) -> bool {
    match op {
        TqlCompOp::Eq => a == b,
        TqlCompOp::Ne => a != b,
        TqlCompOp::Gt => a > b,
        TqlCompOp::Gte => a >= b,
        TqlCompOp::Lt => a < b,
        TqlCompOp::Lte => a <= b,
    }
}

fn cmp_f64(a: f64, op: &TqlCompOp, b: f64) -> bool {
    match op {
        TqlCompOp::Eq => (a - b).abs() < f64::EPSILON,
        TqlCompOp::Ne => (a - b).abs() >= f64::EPSILON,
        TqlCompOp::Gt => a > b,
        TqlCompOp::Gte => a >= b,
        TqlCompOp::Lt => a < b,
        TqlCompOp::Lte => a <= b,
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  谓词校验与排序
// ═══════════════════════════════════════════════════════════════════════

/// 带 id 感知的 Filter 匹配：将节点的结构 id 注入到 payload 匹配逻辑中
fn matches_filter_with_id<T: VectorType>(
    filter: &Filter,
    node_id: NodeId,
    mt: &MemTable<T>,
) -> bool {
    match filter {
        // id 字段特殊处理：匹配节点的结构 ID，而不是 payload 中的字段
        Filter::Eq(key, val) if key == "id" => {
            val.as_i64().is_some_and(|target| node_id == target as u64)
        }
        // 逻辑组合：递归处理
        Filter::And(filters) => filters
            .iter()
            .all(|f| matches_filter_with_id(f, node_id, mt)),
        Filter::Or(filters) => filters
            .iter()
            .any(|f| matches_filter_with_id(f, node_id, mt)),
        // 其他操作符：回退到标准 payload 匹配
        _ => match mt.get_payload(node_id) {
            Some(payload) => filter.matches(payload),
            None => false,
        },
    }
}

/// ORDER BY 排序
fn sort_results<T: VectorType>(
    results: &mut TqlResult<T>,
    order_by: &[OrderExpr],
    _mt: &MemTable<T>,
) {
    results.sort_by(|a, b| {
        for order in order_by {
            // 从结果行中提取排序键
            let a_val = extract_order_key(&order.expr, a);
            let b_val = extract_order_key(&order.expr, b);

            let cmp = compare_for_sort(&a_val, &b_val);
            let cmp = if order.descending { cmp.reverse() } else { cmp };

            if cmp != std::cmp::Ordering::Equal {
                return cmp;
            }
        }
        std::cmp::Ordering::Equal
    });
}

fn extract_order_key<T>(expr: &TqlExpr, row: &HashMap<String, Node<T>>) -> RuntimeValue {
    match expr {
        TqlExpr::Variable(_)
        | TqlExpr::Parameter(_)
        | TqlExpr::Similarity { .. }
        | TqlExpr::GraphScore { .. }
        | TqlExpr::Depth { .. }
        | TqlExpr::PathStrength { .. }
        | TqlExpr::PathCount { .. }
        | TqlExpr::Community { .. }
        | TqlExpr::Path { .. }
        | TqlExpr::PathLength { .. } => RuntimeValue::Null,
        TqlExpr::Binary { left, op, right } => eval_binary(
            extract_order_key(left, row),
            *op,
            extract_order_key(right, row),
        ),
        TqlExpr::Coalesce(values) => values
            .iter()
            .map(|value| extract_order_key(value, row))
            .find(|value| !matches!(value, RuntimeValue::Null))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::IsNull { expr, negated } => RuntimeValue::Bool(
            matches!(extract_order_key(expr, row), RuntimeValue::Null) != *negated,
        ),
        TqlExpr::Property { var, field } => {
            if let Some(node) = row.get(var) {
                if field == "id" {
                    return RuntimeValue::Int(node.id as i64);
                }
                return json_to_runtime(&node.payload[field]);
            }
            // FIND/SEARCH 场景下节点绑定到 "_"
            if let Some(node) = row.get("_") {
                if field == "id" {
                    return RuntimeValue::Int(node.id as i64);
                }
                return json_to_runtime(&node.payload[field]);
            }
            RuntimeValue::Null
        }
        TqlExpr::Literal(lit) => lit_to_runtime(lit),
    }
}

fn compare_for_sort(a: &RuntimeValue, b: &RuntimeValue) -> std::cmp::Ordering {
    match (a, b) {
        (RuntimeValue::Int(a), RuntimeValue::Int(b)) => a.cmp(b),
        (RuntimeValue::Float(a), RuntimeValue::Float(b)) => {
            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
        }
        (RuntimeValue::Int(a), RuntimeValue::Float(b)) => (*a as f64)
            .partial_cmp(b)
            .unwrap_or(std::cmp::Ordering::Equal),
        (RuntimeValue::Float(a), RuntimeValue::Int(b)) => a
            .partial_cmp(&(*b as f64))
            .unwrap_or(std::cmp::Ordering::Equal),
        (RuntimeValue::Str(a), RuntimeValue::Str(b)) => a.cmp(b),
        (RuntimeValue::Null, RuntimeValue::Null) => std::cmp::Ordering::Equal,
        (RuntimeValue::Null, _) => std::cmp::Ordering::Greater, // NULL 排最后
        (_, RuntimeValue::Null) => std::cmp::Ordering::Less,
        _ => std::cmp::Ordering::Equal,
    }
}

/// 从 MemTable 构建完整 Node
fn build_node<T: VectorType>(id: NodeId, mt: &MemTable<T>) -> Option<Node<T>> {
    let vector = mt.get_vector(id)?;
    let payload = mt.get_payload(id)?;
    let edges = mt.get_edges(id).map(|e| e.to_vec()).unwrap_or_default();
    Some(Node {
        id,
        vector: vector.to_vec(),
        payload: payload.clone(),
        edges,
    })
}

// ═══════════════════════════════════════════════════════════════════════
//  Phase 2: 聚合 + DISTINCT + 辅助函数
// ═══════════════════════════════════════════════════════════════════════

/// 从 ReturnExpr 列表中提取涉及的变量名（用于 MATCH 投影）
fn extract_vars_from_exprs(exprs: &[ReturnExpr]) -> Vec<String> {
    let mut vars = Vec::new();
    for expr in exprs {
        collect_vars_from_kind(&expr.kind, &mut vars);
    }
    vars.dedup();
    vars
}

/// 从 Expressions 中提取第一个变量（用于 FIND 隐式绑定）
fn extract_first_var_from_exprs(exprs: &[ReturnExpr]) -> Option<String> {
    for expr in exprs {
        if let Some(var) = first_var_from_kind(&expr.kind) {
            return Some(var);
        }
    }
    None
}

fn expr_first_var(expr: &TqlExpr) -> Option<&String> {
    match expr {
        TqlExpr::Variable(var)
        | TqlExpr::Similarity { var }
        | TqlExpr::GraphScore { var }
        | TqlExpr::Depth { var }
        | TqlExpr::PathStrength { var }
        | TqlExpr::PathCount { var }
        | TqlExpr::Community { var }
        | TqlExpr::Path { var }
        | TqlExpr::PathLength { var }
        | TqlExpr::Property { var, .. } => Some(var),
        TqlExpr::Binary { left, right, .. } => {
            expr_first_var(left).or_else(|| expr_first_var(right))
        }
        TqlExpr::Coalesce(values) => values.iter().find_map(expr_first_var),
        TqlExpr::IsNull { expr, .. } => expr_first_var(expr),
        TqlExpr::Parameter(_) | TqlExpr::Literal(_) => None,
    }
}

fn collect_vars_from_kind(kind: &ReturnExprKind, out: &mut Vec<String>) {
    match kind {
        ReturnExprKind::Var(v) => {
            if !out.contains(v) {
                out.push(v.clone());
            }
        }
        ReturnExprKind::Property(v, _) => {
            if !out.contains(v) {
                out.push(v.clone());
            }
        }
        ReturnExprKind::Scalar(expr) => {
            let var = match expr {
                TqlExpr::Variable(var)
                | TqlExpr::Similarity { var }
                | TqlExpr::GraphScore { var }
                | TqlExpr::Depth { var }
                | TqlExpr::PathStrength { var }
                | TqlExpr::PathCount { var }
                | TqlExpr::Community { var }
                | TqlExpr::Path { var }
                | TqlExpr::PathLength { var }
                | TqlExpr::Property { var, .. } => Some(var),
                TqlExpr::Binary { left, right, .. } => {
                    expr_first_var(left).or_else(|| expr_first_var(right))
                }
                TqlExpr::Coalesce(values) => values.iter().find_map(expr_first_var),
                TqlExpr::IsNull { expr, .. } => expr_first_var(expr),
                TqlExpr::Parameter(_) | TqlExpr::Literal(_) => None,
            };
            if let Some(var) = var
                && !out.contains(var)
            {
                out.push(var.clone());
            }
        }
        ReturnExprKind::Aggregate(_, inner) => collect_vars_from_kind(inner, out),
    }
}

fn first_var_from_kind(kind: &ReturnExprKind) -> Option<String> {
    match kind {
        ReturnExprKind::Var(v) => Some(v.clone()),
        ReturnExprKind::Property(v, _) => Some(v.clone()),
        ReturnExprKind::Scalar(expr) => expr_first_var(expr).cloned(),
        ReturnExprKind::Aggregate(_, inner) => first_var_from_kind(inner),
    }
}

/// 聚合 + DISTINCT 后处理
///
/// 处理逻辑：
/// - 如果 ReturnClause 不是 Expressions，直接返回原结果
/// - 如果没有聚合函数，只做 DISTINCT 过滤
/// - 如果有聚合函数，按非聚合列分组，对每组计算聚合值
fn apply_aggregation_and_distinct<T: VectorType>(
    returns: &ReturnClause,
    results: TqlResult<T>,
    is_find_entry: bool,
) -> Result<TqlResult<T>, TriviumError> {
    let exprs = match returns {
        ReturnClause::Expressions(exprs) => exprs,
        _ => return Ok(results),
    };

    let has_agg = exprs.iter().any(|e| is_aggregate(&e.kind));
    let has_distinct = exprs.iter().any(|e| e.distinct);

    // FIND 只有隐式节点变量 `_`。聚合上下文中的裸标识符仍表示节点变量，
    // 不能静默猜测为 Payload 字段，否则 MATCH 与 FIND 的同一语法会有两种含义。
    if is_find_entry
        && has_agg
        && let Some(name) = exprs.iter().find_map(|expr| match &expr.kind {
            ReturnExprKind::Var(name) if name != "_" && name != "*" => Some(name.as_str()),
            _ => None,
        })
    {
        return Err(TriviumError::QueryExecution(format!(
            "FIND 聚合中的裸标识符 `{name}` 不是属性；请写 `_.{name}` (Bare identifier `{name}` is not a FIND property; use `_.{name}`)"
        )));
    }

    // 无聚合、无 DISTINCT → 直接返回
    if !has_agg && !has_distinct {
        return Ok(results);
    }

    // 纯 DISTINCT，无聚合
    if !has_agg && has_distinct {
        return Ok(apply_distinct(results, exprs, is_find_entry));
    }

    // 有聚合函数 → 分组计算
    Ok(apply_aggregation(results, exprs))
}

/// 判断表达式是否包含聚合函数
fn is_aggregate(kind: &ReturnExprKind) -> bool {
    matches!(kind, ReturnExprKind::Aggregate(_, _))
}

/// 纯 DISTINCT 去重
fn apply_distinct<T: VectorType>(
    results: TqlResult<T>,
    exprs: &[ReturnExpr],
    is_find_entry: bool,
) -> TqlResult<T> {
    let distinct_exprs: Vec<&ReturnExpr> = exprs.iter().filter(|e| e.distinct).collect();
    let key_exprs = if distinct_exprs.is_empty() {
        exprs.iter().collect()
    } else {
        distinct_exprs
    };

    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for row in results {
        let sig = distinct_signature(&row, &key_exprs, is_find_entry);
        if seen.insert(sig) {
            out.push(row);
        }
    }
    out
}

/// DISTINCT 签名：基于返回表达式的实际值拼接
fn distinct_signature<T: VectorType>(
    row: &HashMap<String, Node<T>>,
    exprs: &[&ReturnExpr],
    is_find_entry: bool,
) -> String {
    exprs
        .iter()
        .map(|expr| {
            format!(
                "{}={}",
                format_return_expr(expr),
                distinct_expr_value(row, expr, is_find_entry)
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// DISTINCT 表达式值：变量按节点身份去重，属性按 payload 值去重
fn distinct_expr_value<T: VectorType>(
    row: &HashMap<String, Node<T>>,
    expr: &ReturnExpr,
    is_find_entry: bool,
) -> serde_json::Value {
    match &expr.kind {
        ReturnExprKind::Var(v) => {
            if is_find_entry
                && let Some(node) = row.get(v)
                && let Some(value) = node.payload.get(v)
            {
                value.clone()
            } else {
                row.get(v)
                    .map(|node| serde_json::json!(node.id))
                    .unwrap_or(serde_json::Value::Null)
            }
        }
        ReturnExprKind::Property(v, field) => row
            .get(v)
            .and_then(|node| node.payload.get(field).cloned())
            .unwrap_or(serde_json::Value::Null),
        ReturnExprKind::Scalar(_) => serde_json::Value::Null,
        ReturnExprKind::Aggregate(_, _) => serde_json::Value::Null,
    }
}

/// 聚合计算
///
/// 非聚合列作为分组键，聚合列按组计算。
/// 结果中聚合值写入节点的 payload 字段中（以 alias 或生成名为 key）。
fn apply_aggregation<T: VectorType>(results: TqlResult<T>, exprs: &[ReturnExpr]) -> TqlResult<T> {
    // 分离分组列和聚合列
    let group_exprs: Vec<&ReturnExpr> = exprs.iter().filter(|e| !is_aggregate(&e.kind)).collect();
    let agg_exprs: Vec<&ReturnExpr> = exprs.iter().filter(|e| is_aggregate(&e.kind)).collect();
    if results.is_empty() {
        if !group_exprs.is_empty() {
            return Vec::new();
        }
        let empty_rows: Vec<&HashMap<String, Node<T>>> = Vec::new();
        let mut result_row = HashMap::new();
        for agg_expr in agg_exprs {
            if let ReturnExprKind::Aggregate(func, inner) = &agg_expr.kind {
                let alias = agg_expr
                    .alias
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", func).to_lowercase());
                let value = compute_aggregate(*func, inner, &empty_rows);
                result_row.insert(
                    alias.clone(),
                    Node {
                        id: 0,
                        vector: Vec::new(),
                        payload: serde_json::json!({ alias: value }),
                        edges: Vec::new(),
                    },
                );
            }
        }
        return vec![result_row];
    }

    // 构建分组键
    let mut groups: BTreeMap<String, Vec<&HashMap<String, Node<T>>>> = BTreeMap::new();
    for row in &results {
        let key = make_group_key(row, &group_exprs);
        groups.entry(key).or_default().push(row);
    }

    // 对每组计算聚合
    let mut output = Vec::new();
    for rows in groups.values() {
        let mut result_row: HashMap<String, Node<T>> = HashMap::new();

        // 保留分组列的值（取组内第一行的绑定）
        if let Some(first_row) = rows.first() {
            for expr in &group_exprs {
                if let Some(var) = first_var_from_kind(&expr.kind)
                    && let Some(node) = first_row.get(&var)
                {
                    result_row.insert(var, node.clone());
                }
            }
        }

        // 计算聚合列
        for agg_expr in &agg_exprs {
            if let ReturnExprKind::Aggregate(func, inner) = &agg_expr.kind {
                let alias = agg_expr
                    .alias
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", func).to_lowercase());
                let agg_val = compute_aggregate(*func, inner, rows);

                // 将聚合值注入到一个合成节点的 payload 中
                let agg_node = Node {
                    id: 0,
                    vector: Vec::new(),
                    payload: serde_json::json!({ &alias: agg_val }),
                    edges: Vec::new(),
                };
                result_row.insert(alias, agg_node);
            }
        }

        output.push(result_row);
    }

    output
}

/// 生成分组键
fn make_group_key<T: VectorType>(
    row: &HashMap<String, Node<T>>,
    group_exprs: &[&ReturnExpr],
) -> String {
    let mut parts = Vec::new();
    for expr in group_exprs {
        match &expr.kind {
            ReturnExprKind::Var(v) => {
                if let Some(node) = row.get(v) {
                    parts.push(format!("{}:{}", v, node.id));
                }
            }
            ReturnExprKind::Property(v, field) => {
                if let Some(node) = row.get(v) {
                    let val = node
                        .payload
                        .get(field)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    parts.push(format!("{}.{}={}", v, field, val));
                }
            }
            _ => {}
        }
    }
    parts.join("|")
}

/// 计算单个聚合函数
fn compute_aggregate<T: VectorType>(
    func: AggFunc,
    inner: &ReturnExprKind,
    rows: &[&HashMap<String, Node<T>>],
) -> serde_json::Value {
    match func {
        AggFunc::Count => {
            let count = if matches!(inner, ReturnExprKind::Var(name) if name == "*") {
                rows.len()
            } else {
                rows.iter()
                    .filter(|row| resolve_inner(inner, row).is_some())
                    .count()
            };
            serde_json::json!(count)
        }
        AggFunc::Sum => {
            let sum: f64 = rows
                .iter()
                .filter_map(|r| resolve_inner_numeric(inner, r))
                .sum();
            serde_json::json!(sum)
        }
        AggFunc::Avg => {
            let vals: Vec<f64> = rows
                .iter()
                .filter_map(|r| resolve_inner_numeric(inner, r))
                .collect();
            if vals.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::json!(vals.iter().sum::<f64>() / vals.len() as f64)
            }
        }
        AggFunc::Min => {
            let min = rows
                .iter()
                .filter_map(|r| resolve_inner_numeric(inner, r))
                .fold(f64::INFINITY, f64::min);
            if min.is_infinite() {
                serde_json::Value::Null
            } else {
                serde_json::json!(min)
            }
        }
        AggFunc::Max => {
            let max = rows
                .iter()
                .filter_map(|r| resolve_inner_numeric(inner, r))
                .fold(f64::NEG_INFINITY, f64::max);
            if max.is_infinite() {
                serde_json::Value::Null
            } else {
                serde_json::json!(max)
            }
        }
        AggFunc::Collect => {
            let vals: Vec<serde_json::Value> = rows
                .iter()
                .filter_map(|r| resolve_inner_json(inner, r))
                .collect();
            serde_json::json!(vals)
        }
    }
}

/// 从行中解析内部表达式引用的节点
fn resolve_inner<'a, T: VectorType>(
    inner: &ReturnExprKind,
    row: &'a HashMap<String, Node<T>>,
) -> Option<&'a Node<T>> {
    match inner {
        ReturnExprKind::Var(v) => row.get(v),
        ReturnExprKind::Property(v, _) => row.get(v),
        _ => None,
    }
}

/// 解析为数值
fn resolve_inner_numeric<T: VectorType>(
    inner: &ReturnExprKind,
    row: &HashMap<String, Node<T>>,
) -> Option<f64> {
    match inner {
        ReturnExprKind::Var(v) => {
            // count 风格: 变量存在就是 1
            row.get(v).map(|_| 1.0)
        }
        ReturnExprKind::Property(v, field) => row
            .get(v)
            .and_then(|node| node.payload.get(field).and_then(|v| v.as_f64())),
        _ => None,
    }
}

/// 解析为 JSON 值
fn resolve_inner_json<T: VectorType>(
    inner: &ReturnExprKind,
    row: &HashMap<String, Node<T>>,
) -> Option<serde_json::Value> {
    match inner {
        ReturnExprKind::Var(v) => row.get(v).map(|n| serde_json::json!(n.id)),
        ReturnExprKind::Property(v, field) => {
            row.get(v).and_then(|node| node.payload.get(field).cloned())
        }
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  EXPLAIN 查询计划生成
// ═══════════════════════════════════════════════════════════════════════

/// 生成查询执行计划（不执行查询），返回单行结果
fn tql_literal_to_json(literal: &TqlLiteral) -> serde_json::Value {
    match literal {
        TqlLiteral::Int(value) => (*value).into(),
        TqlLiteral::Float(value) => serde_json::Number::from_f64(*value)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        TqlLiteral::Str(value) => value.clone().into(),
        TqlLiteral::Bool(value) => (*value).into(),
        TqlLiteral::Null => serde_json::Value::Null,
    }
}

fn collect_cross_modal_origins(
    predicate: &Predicate,
    output: &mut Vec<(String, serde_json::Value)>,
) {
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
        } => output.push((field.clone(), tql_literal_to_json(value))),
        Predicate::And(left, right) => {
            collect_cross_modal_origins(left, output);
            collect_cross_modal_origins(right, output);
        }
        _ => {}
    }
}

fn generate_explain_plan<T: VectorType>(
    query: &TqlQuery,
    mt: &MemTable<T>,
    actual: Option<(usize, f64)>,
) -> TqlResult<T> {
    let mut plan = serde_json::Map::new();

    // 入口类型
    let (entry_type, entry_detail) = match &query.entry {
        QueryEntry::Find { filter } => ("FIND".to_string(), format!("{:?}", filter)),
        QueryEntry::Match { pattern } => {
            let detail = format_pattern_detail(pattern);
            ("MATCH".to_string(), detail)
        }
        QueryEntry::OptionalMatch { pattern } => {
            let detail = format_pattern_detail(pattern);
            ("OPTIONAL MATCH".to_string(), detail)
        }
        QueryEntry::Search { top_k, expand, .. } => {
            let detail = if expand.is_some() {
                format!("TOP {} + EXPAND", top_k)
            } else {
                format!("TOP {}", top_k)
            };
            ("SEARCH".to_string(), detail)
        }
    };
    plan.insert("entry".into(), serde_json::json!(entry_type));
    plan.insert("detail".into(), serde_json::json!(entry_detail));

    let (access_path, estimated_rows, reversed) = match &query.entry {
        QueryEntry::Find { filter } => {
            let planned = plan_filter(filter, mt);
            (planned.access_path, planned.estimated_rows, false)
        }
        QueryEntry::Match { pattern } => {
            let planned = plan_match(pattern, mt, false);
            (
                planned.access_path,
                planned.estimated_rows,
                planned.reversed,
            )
        }
        QueryEntry::OptionalMatch { pattern } => {
            let planned = plan_match(pattern, mt, true);
            (planned.access_path, planned.estimated_rows, false)
        }
        QueryEntry::Search { .. } => (AccessPath::FullNodeScan, mt.node_count(), false),
    };
    plan.insert(
        "access_path".into(),
        serde_json::to_value(&access_path).unwrap_or(serde_json::Value::Null),
    );
    let strategy = match &access_path {
        AccessPath::PrimaryKey { .. } => "id_shortcut O(1)".to_owned(),
        AccessPath::PropertyIndex { field } => format!("property_index ({field})"),
        AccessPath::OrderedPropertyIndex { field, descending } => {
            format!("ordered_property_index ({field}, descending={descending})")
        }
        AccessPath::CompositePropertyIndex { fields } => {
            format!("composite_property_index ({})", fields.join(", "))
        }
        AccessPath::BitmapPropertyIndex { fields } => {
            format!("bitmap_property_index ({})", fields.join(", "))
        }
        AccessPath::PropertyIndexIntersection { fields } => {
            format!("property_index_intersection ({})", fields.join(", "))
        }
        AccessPath::EdgeLabelIndex { labels } => {
            format!("label_index pushdown (labels: [{}])", labels.join(", "))
        }
        AccessPath::FullNodeScan => "full_scan".to_owned(),
    };
    plan.insert("candidate_strategy".into(), serde_json::json!(strategy));
    plan.insert("estimated_rows".into(), serde_json::json!(estimated_rows));
    plan.insert("reversed".into(), serde_json::json!(reversed));
    if !query.pipeline.is_empty() {
        let cascades = crate::query::cascades::optimize_pipeline(
            query,
            mt,
            crate::query::cascades::OptimizerBudget::default(),
        );
        plan.insert("optimizer".into(), serde_json::json!("cascades_memo"));
        plan.insert(
            "optimizer_status".into(),
            serde_json::to_value(cascades.status).unwrap_or(serde_json::Value::Null),
        );
        plan.insert(
            "memo_groups".into(),
            serde_json::json!(cascades.groups.len()),
        );
        plan.insert(
            "explored_expressions".into(),
            serde_json::json!(cascades.explored_expressions),
        );
        plan.insert(
            "pruned_expressions".into(),
            serde_json::json!(cascades.pruned_expressions),
        );
        plan.insert(
            "total_estimated_cost".into(),
            serde_json::json!(cascades.total_estimated_cost),
        );
        plan.insert(
            "pipeline_stages".into(),
            serde_json::to_value(&cascades.stages).unwrap_or(serde_json::Value::Null),
        );
        let mut origins = Vec::new();
        for stage in &query.pipeline {
            if let PipelineStage::Filter(predicate) = stage {
                collect_cross_modal_origins(predicate, &mut origins);
            }
        }
        origins.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.to_string().cmp(&right.1.to_string()))
        });
        origins.dedup();
        let cross_modal = origins
            .into_iter()
            .filter_map(|(field, value)| {
                mt.cross_modal_stats(&field, &value).map(
                    |stats| serde_json::json!({"field": field, "value": value, "stats": stats}),
                )
            })
            .collect::<Vec<_>>();
        plan.insert("cross_modal_stats".into(), serde_json::json!(cross_modal));
        plan.insert(
            "optimizer_rules".into(),
            serde_json::to_value(&cascades.rules).unwrap_or(serde_json::Value::Null),
        );
        plan.insert(
            "exact_rerank_after".into(),
            serde_json::json!(cascades.exact_rerank_after),
        );
    }
    if let QueryEntry::Search { vector, top_k, .. } = &query.entry {
        let estimated_candidates = top_k.saturating_mul(8).max(64).min(mt.node_count());
        let estimated_temp_bytes =
            estimated_candidates.saturating_mul(std::mem::size_of::<(NodeId, f32)>());
        let estimated_vector_page_reads = estimated_candidates
            .saturating_mul(vector.len())
            .saturating_mul(std::mem::size_of::<T>())
            .div_ceil(4096);
        plan.insert(
            "industrial_access_path".into(),
            serde_json::json!(if mt.quiver().is_some() {
                "ann_post_filter"
            } else {
                "exact_fallback"
            }),
        );
        plan.insert(
            "estimated_candidates".into(),
            serde_json::json!(estimated_candidates),
        );
        plan.insert(
            "estimated_temp_bytes".into(),
            serde_json::json!(estimated_temp_bytes),
        );
        plan.insert(
            "estimated_vector_page_reads".into(),
            serde_json::json!(estimated_vector_page_reads),
        );
        plan.insert(
            "estimated_payload_page_reads".into(),
            serde_json::json!(estimated_candidates),
        );
        plan.insert("estimated_graph_page_reads".into(), serde_json::json!(0));
        plan.insert("temporary_spill".into(), serde_json::json!(false));
    }

    // WHERE 谓词
    if let Some(pred) = &query.predicate {
        plan.insert("predicate".into(), serde_json::json!(format!("{:?}", pred)));
    } else {
        plan.insert("predicate".into(), serde_json::json!("none"));
    }

    // RETURN 信息
    let return_info = match &query.returns {
        ReturnClause::All => "ALL (*)".to_string(),
        ReturnClause::Variables(vars) => format!("variables: [{}]", vars.join(", ")),
        ReturnClause::Expressions(exprs) => {
            let descs: Vec<String> = exprs.iter().map(format_return_expr).collect();
            format!("expressions: [{}]", descs.join(", "))
        }
    };
    plan.insert("return".into(), serde_json::json!(return_info));

    // 优化提示
    let mut optimizations = Vec::new();
    if matches!(access_path, AccessPath::PrimaryKey { .. }) {
        optimizations.push("ID O(1) shortcut");
    }
    if matches!(access_path, AccessPath::EdgeLabelIndex { .. }) {
        optimizations.push("label index pushdown");
    }
    if matches!(
        access_path,
        AccessPath::PropertyIndex { .. }
            | AccessPath::OrderedPropertyIndex { .. }
            | AccessPath::PropertyIndexIntersection { .. }
    ) {
        optimizations.push("property index candidates");
    }
    if reversed {
        optimizations.push("MATCH start reversal");
    }
    if let ReturnClause::Expressions(exprs) = &query.returns {
        let prunable = get_prunable_vars(exprs);
        if !prunable.is_empty() {
            optimizations.push("projection pruning");
        }
        if exprs.iter().any(|e| e.distinct) {
            optimizations.push("DISTINCT dedup");
        }
        if exprs.iter().any(|e| is_aggregate(&e.kind)) {
            optimizations.push("aggregation");
        }
    }
    if query.limit.is_some() {
        optimizations.push("LIMIT early termination");
    }
    if query.rank.is_some() {
        optimizations.push("GraphFirst exact anchor ranking");
    }
    plan.insert("optimizations".into(), serde_json::json!(optimizations));

    // 统计信息
    plan.insert(
        "total_nodes".into(),
        serde_json::json!(mt.all_node_ids().len()),
    );
    if let Some(lim) = query.limit {
        plan.insert("limit".into(), serde_json::json!(lim));
    }
    if let Some(off) = query.offset {
        plan.insert("offset".into(), serde_json::json!(off));
    }
    if !query.order_by.is_empty() {
        plan.insert(
            "order_by_count".into(),
            serde_json::json!(query.order_by.len()),
        );
    }

    plan.insert(
        "property_index_stats".into(),
        serde_json::to_value(mt.property_index_stats()).unwrap_or(serde_json::Value::Null),
    );
    if matches!(
        query.entry,
        QueryEntry::Match { .. } | QueryEntry::OptionalMatch { .. }
    ) {
        plan.insert(
            "graph_stats".into(),
            serde_json::to_value(mt.graph_stats()).unwrap_or(serde_json::Value::Null),
        );
        plan.insert(
            "traversal_budget".into(),
            serde_json::json!({
                "max_visited_nodes": MAX_BUDGET,
                "max_examined_edges": MAX_BUDGET,
                "max_frontier_size": MAX_BUDGET,
                "exhaustion_policy": "error",
            }),
        );
    }
    if let Some((actual_rows, elapsed_ms)) = actual {
        plan.insert("analyze".into(), serde_json::json!(true));
        plan.insert("actual_rows".into(), serde_json::json!(actual_rows));
        plan.insert("elapsed_ms".into(), serde_json::json!(elapsed_ms));
    } else {
        plan.insert("analyze".into(), serde_json::json!(false));
    }

    // 封装为单行结果
    let plan_node = Node {
        id: 0,
        vector: Vec::new(),
        payload: serde_json::Value::Object(plan),
        edges: Vec::new(),
    };
    let mut row = HashMap::new();
    row.insert("plan".to_string(), plan_node);
    vec![row]
}

/// 格式化 Pattern 详情
fn format_pattern_detail(pattern: &TqlPattern) -> String {
    let mut parts = Vec::new();
    for (i, node) in pattern.nodes.iter().enumerate() {
        let var = node.var.as_deref().unwrap_or("_");
        let filter = if node.filter.is_some() {
            " {filter}"
        } else {
            ""
        };
        parts.push(format!("({}{})", var, filter));

        if i < pattern.edges.len() {
            let edge = &pattern.edges[i];
            let labels = if edge.labels.is_empty() {
                String::new()
            } else {
                format!(":{}", edge.labels.join("|"))
            };
            let hops = if let Some(hop) = &edge.hop_range {
                format!("*{}..{}", hop.min, hop.max)
            } else {
                String::new()
            };
            parts.push(format!("-[{}{}]->", labels, hops));
        }
    }
    parts.join("")
}

/// 格式化 ReturnExpr 为可读字符串
fn format_return_expr(expr: &ReturnExpr) -> String {
    let mut s = String::new();
    if expr.distinct {
        s.push_str("DISTINCT ");
    }
    s.push_str(&format_return_expr_kind(&expr.kind));
    if let Some(alias) = &expr.alias {
        s.push_str(&format!(" AS {}", alias));
    }
    s
}

fn format_tql_expr(expr: &TqlExpr) -> String {
    match expr {
        TqlExpr::Variable(var) => var.clone(),
        TqlExpr::Property { var, field } => format!("{var}.{field}"),
        TqlExpr::Similarity { var } => format!("similarity({var})"),
        TqlExpr::GraphScore { var } => format!("graph_score({var})"),
        TqlExpr::Depth { var } => format!("depth({var})"),
        TqlExpr::PathStrength { var } => format!("path_strength({var})"),
        TqlExpr::PathCount { var } => format!("path_count({var})"),
        TqlExpr::Community { var } => format!("community({var})"),
        TqlExpr::Path { var } => format!("path({var})"),
        TqlExpr::PathLength { var } => format!("path_length({var})"),
        TqlExpr::Parameter(name) => format!("${name}"),
        TqlExpr::Binary { left, op, right } => format!(
            "({} {:?} {})",
            format_tql_expr(left),
            op,
            format_tql_expr(right)
        ),
        TqlExpr::Coalesce(values) => format!(
            "coalesce({})",
            values
                .iter()
                .map(format_tql_expr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TqlExpr::IsNull { expr, negated } => format!(
            "{} IS {}NULL",
            format_tql_expr(expr),
            if *negated { "NOT " } else { "" }
        ),
        TqlExpr::Literal(_) => "literal".into(),
    }
}

fn format_return_expr_kind(kind: &ReturnExprKind) -> String {
    match kind {
        ReturnExprKind::Var(v) => v.clone(),
        ReturnExprKind::Property(v, f) => format!("{}.{}", v, f),
        ReturnExprKind::Scalar(expr) => format_tql_expr(expr),
        ReturnExprKind::Aggregate(func, inner) => {
            format!("{:?}({})", func, format_return_expr_kind(inner))
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  投影裁剪
// ═══════════════════════════════════════════════════════════════════════

/// 投影裁剪：仅属性引用的变量剥离 vector + edges
///
/// 优化逻辑：
/// - 扫描 ReturnClause::Expressions，识别仅通过 Property(var, field) 引用的变量
/// - 对这些变量的 Node，清空 vector 和 edges（保留 id + payload）
/// - 全变量引用 (Var(v)) 的节点保持完整
fn apply_projection_pruning<T: VectorType>(returns: &ReturnClause, results: &mut TqlResult<T>) {
    let exprs = match returns {
        ReturnClause::Expressions(exprs) => exprs,
        _ => return, // All / Variables 模式不裁剪
    };

    let prunable = get_prunable_vars(exprs);
    if prunable.is_empty() {
        return;
    }

    for row in results.iter_mut() {
        for var in &prunable {
            if let Some(node) = row.get_mut(var) {
                // 清空重量级字段，只保留 id + payload
                node.vector.clear();
                node.edges.clear();
            }
        }
    }
}

/// 找出"仅通过属性访问引用"的变量（可安全裁剪 vector + edges）
///
/// 规则：
/// - 如果一个变量出现在 Var(v) 中 → 完整引用，不可裁剪
/// - 如果一个变量只出现在 Property(v, field) 中 → 仅属性引用，可裁剪
/// - 聚合内部递归检查
fn get_prunable_vars(exprs: &[ReturnExpr]) -> Vec<String> {
    let mut full_vars: HashSet<String> = HashSet::new(); // 完整引用
    let mut prop_vars: HashSet<String> = HashSet::new(); // 属性引用

    for expr in exprs {
        classify_vars(&expr.kind, &mut full_vars, &mut prop_vars);
    }

    // 可裁剪 = 仅属性引用，未被完整引用
    prop_vars.difference(&full_vars).cloned().collect()
}

fn classify_vars(
    kind: &ReturnExprKind,
    full_vars: &mut HashSet<String>,
    prop_vars: &mut HashSet<String>,
) {
    match kind {
        ReturnExprKind::Var(v) => {
            full_vars.insert(v.clone());
        }
        ReturnExprKind::Property(v, _) => {
            prop_vars.insert(v.clone());
        }
        ReturnExprKind::Scalar(expr) => {
            if let Some(var) = expr_first_var(expr) {
                full_vars.insert(var.clone());
            }
        }
        ReturnExprKind::Aggregate(_, inner) => {
            classify_vars(inner, full_vars, prop_vars);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  DML 写操作执行器
// ═══════════════════════════════════════════════════════════════════════

/// 执行 TQL 写操作，生成 MutationOp 指令列表
///
/// 返回的指令由 Database 层逐条应用（含 WAL）。
/// 对于 CREATE 操作，InsertNode 指令中的 var 字段用于后续 LinkEdge 的变量解析。
pub fn execute_tql_mutation<T: VectorType>(
    mutation: &TqlMutation,
    mt: &MemTable<T>,
) -> Result<Vec<MutationOp<T>>, TriviumError> {
    match &mutation.action {
        MutationAction::Create(create) => execute_create(create, &mutation.source, mt),
        MutationAction::Set(assignments) => execute_set(assignments, &mutation.source, mt),
        MutationAction::Delete { vars, detach } => {
            execute_delete(vars, *detach, &mutation.source, mt)
        }
    }
}

/// CREATE 指令生成
fn execute_create<T: VectorType>(
    create: &CreateAction,
    source: &Option<MutationSource>,
    mt: &MemTable<T>,
) -> Result<Vec<MutationOp<T>>, TriviumError> {
    let mut ops = Vec::new();
    let dim = mt.dim();

    // 如果有前置 MATCH（用于创建边引用已有节点）
    let matched_vars: HashMap<String, NodeId> = if let Some(src) = source {
        resolve_match_vars(&src.pattern, src.predicate.as_ref(), mt)?
    } else {
        HashMap::new()
    };

    // 1. 为每个 CreateNode 生成 InsertNode 指令
    //    已在 MATCH 中绑定的变量名 → 不创建新节点
    for node in &create.nodes {
        let var = node.var.as_deref().unwrap_or("_anon");
        if matched_vars.contains_key(var) {
            continue; // 已有节点，跳过创建
        }
        let zero_vec = vec![T::default(); dim];
        ops.push(MutationOp::InsertNode {
            var: var.to_string(),
            vector: zero_vec,
            payload: node.payload.clone(),
        });
    }

    // 2. 为每个 CreateEdge 生成 LinkEdge 指令
    //    src/dst 可能引用 MATCH 变量（已有 ID）或 CREATE 变量（待分配 ID）
    for edge in &create.edges {
        // 如果 src 和 dst 都已匹配到 ID，直接生成 LinkEdge
        let src_id = matched_vars.get(&edge.src_var).copied();
        let dst_id = matched_vars.get(&edge.dst_var).copied();

        if let (Some(s), Some(d)) = (src_id, dst_id) {
            ops.push(MutationOp::LinkEdge {
                src_id: s,
                dst_id: d,
                src_var: edge.src_var.clone(),
                dst_var: edge.dst_var.clone(),
                label: edge.label.clone(),
                weight: edge.weight,
            });
        }
        // 如果引用了 CREATE 变量（尚无 ID），Database 层会在应用
        // InsertNode 后分配 ID，再回填 LinkEdge。这里标记为 ID=0 占位。
        else {
            ops.push(MutationOp::LinkEdge {
                src_id: src_id.unwrap_or(0),
                dst_id: dst_id.unwrap_or(0),
                src_var: edge.src_var.clone(),
                dst_var: edge.dst_var.clone(),
                label: edge.label.clone(),
                weight: edge.weight,
            });
        }
    }

    Ok(ops)
}

/// SET 指令生成
fn execute_set<T: VectorType>(
    assignments: &[SetAssignment],
    source: &Option<MutationSource>,
    mt: &MemTable<T>,
) -> Result<Vec<MutationOp<T>>, TriviumError> {
    let source = source
        .as_ref()
        .ok_or_else(|| TriviumError::QueryParse("SET requires a preceding MATCH clause".into()))?;

    // DML 匹配集不能因查询行上限发生静默的部分写入。
    let query = build_match_query(&source.pattern, source.predicate.as_ref());
    let results = execute_tql_with_limits(&query, mt, TqlLimits::UNLIMITED)?;

    let mut ops = Vec::new();

    for row in &results {
        for assign in assignments {
            if let Some(node) = row.get(&assign.var) {
                // 构建更新后的 payload
                let mut new_payload = node.payload.clone();
                if let Some(obj) = new_payload.as_object_mut() {
                    obj.insert(assign.field.clone(), assign.value.clone());
                }
                ops.push(MutationOp::UpdatePayload {
                    id: node.id,
                    payload: new_payload,
                });
            }
        }
    }

    Ok(ops)
}

/// DELETE / DETACH DELETE 指令生成
fn execute_delete<T: VectorType>(
    vars: &[String],
    detach: bool,
    source: &Option<MutationSource>,
    mt: &MemTable<T>,
) -> Result<Vec<MutationOp<T>>, TriviumError> {
    let source = source.as_ref().ok_or_else(|| {
        TriviumError::QueryParse("DELETE requires a preceding MATCH clause".into())
    })?;

    let query = build_match_query(&source.pattern, source.predicate.as_ref());
    let results = execute_tql_with_limits(&query, mt, TqlLimits::UNLIMITED)?;

    let mut ops = Vec::new();
    let mut deleted: HashSet<NodeId> = HashSet::new();

    for row in &results {
        for var in vars {
            if let Some(node) = row.get(var)
                && deleted.insert(node.id)
            {
                ops.push(MutationOp::DeleteNode {
                    id: node.id,
                    detach,
                });
            }
        }
    }

    Ok(ops)
}

/// 从 MATCH 模式中解析变量绑定（返回第一行匹配的 var → id 映射）
fn resolve_match_vars<T: VectorType>(
    pattern: &TqlPattern,
    predicate: Option<&Predicate>,
    mt: &MemTable<T>,
) -> Result<HashMap<String, NodeId>, TriviumError> {
    let query = build_match_query(pattern, predicate);
    let results = execute_tql(&query, mt)?;

    let mut var_ids = HashMap::new();
    if let Some(first_row) = results.first() {
        for (var, node) in first_row {
            var_ids.insert(var.clone(), node.id);
        }
    }
    Ok(var_ids)
}

/// 从 MutationSource 构建一个用于内部执行的 TqlQuery（RETURN *）
fn build_match_query(pattern: &TqlPattern, predicate: Option<&Predicate>) -> TqlQuery {
    TqlQuery {
        explain: false,
        analyze: false,
        entry: QueryEntry::Match {
            pattern: pattern.clone(),
        },
        pipeline: Vec::new(),
        predicate: predicate.cloned(),
        rank: None,
        returns: ReturnClause::All,
        order_by: Vec::new(),
        limit: None,
        offset: None,
    }
}
