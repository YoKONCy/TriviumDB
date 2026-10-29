//! TQL 统一查询执行器
//!
//! 将 TqlQuery AST 在 MemTable 上执行，支持三种入口：
//! - MATCH: 图模式匹配（含可变长路径、多标签边）
//! - FIND: 文档过滤扫描
//! - SEARCH: 向量检索 + 图扩散（桥接到现有管线）
//!
//! DML 写操作通过 execute_tql_mutation() 生成 MutationOp 指令，
//! 由 Database 层统一执行并写入 WAL。

#[path = "tql_match.rs"]
mod matching;
#[path = "tql_memory.rs"]
mod memory;
#[path = "tql_return.rs"]
mod returning;
use memory::QueryMemory;

use super::pipeline::BindingValue as RuntimeValue;
use super::planner::{
    AccessPath, NodeAccessPlan, plan_filter, plan_filter_ordered, plan_filter_with_limit,
    plan_match,
};
use super::tql_ast::*;
use crate::VectorType;
use crate::error::TriviumError;
use crate::filter::Filter;
use crate::node::{EdgeView, Node, NodeId};
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
    Edge(EdgeView),
    Int(i64),
    UInt(u64),
    Float(f64),
    String(String),
    Bool(bool),
    Path(Vec<NodeId>),
    List(Vec<serde_json::Value>),
    Object(serde_json::Map<String, serde_json::Value>),
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
    /// 更新向量
    UpdateVector { id: NodeId, vector: Vec<T> },
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
    if let Some(limit) = query.limit
        && limit
            .checked_add(query.offset.unwrap_or(0))
            .is_none_or(|rows| rows > memory_cap)
    {
        return Err(TriviumError::QueryRowBudgetExceeded { budget: memory_cap });
    }
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

fn requires_full_input(query: &TqlQuery, ordered_find: bool) -> bool {
    query.rank.is_some()
        || (!query.order_by.is_empty() && !ordered_find)
        || matches!(&query.returns, ReturnClause::Expressions(exprs) if exprs.iter().any(|expr| is_aggregate(&expr.kind) || expr.distinct))
}

fn is_aggregate(kind: &ReturnExprKind) -> bool {
    matches!(kind, ReturnExprKind::Aggregate(..))
}

fn admit_input_rows<T: VectorType>(
    query: &TqlQuery,
    mt: &MemTable<T>,
    limits: &TqlLimits,
    count: usize,
) -> Result<usize, TriviumError> {
    let full = requires_full_input(query, false);
    let (cap, user_limited) = requested_row_limit(query, full, limits, mt)?;
    if count > cap && !user_limited {
        if full || limits.row_overflow == crate::database::RowOverflowPolicy::Throw {
            return Err(TriviumError::QueryRowBudgetExceeded { budget: cap });
        }
        tracing::warn!(
            budget = cap,
            "TQL 返回预算内的部分结果 (TQL returns partial results within the row budget)"
        );
    }
    Ok(count.min(cap))
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
    if let QueryEntry::Match { pattern } | QueryEntry::OptionalMatch { pattern } = &query.entry {
        validate_pattern(pattern)?;
    }
    super::tql_parser::validate_pipeline_scope(query).map_err(TriviumError::QueryExecution)?;
    validate_return_columns(&query.returns)?;
    if matches!(&query.entry, QueryEntry::Search { vector_parameters, .. } if !vector_parameters.is_empty())
    {
        return Err(TriviumError::QueryExecution(
            "SEARCH VECTOR 参数必须通过 Prepared TQL 绑定后执行 (SEARCH VECTOR parameters must be bound through Prepared TQL before execution)".into(),
        ));
    }
    let mut unbound = std::collections::BTreeSet::new();
    super::tql_prepared::collect_query_parameters(query, &mut unbound);
    if !unbound.is_empty() {
        return Err(TriviumError::QueryExecution("查询参数必须通过 Prepared TQL 绑定 (Query parameters must be bound through Prepared TQL)".into()));
    }
    Ok(())
}

pub(crate) fn validate_pattern(pattern: &TqlPattern) -> Result<(), TriviumError> {
    if pattern.nodes.is_empty() || pattern.edges.len().checked_add(1) != Some(pattern.nodes.len()) {
        return Err(TriviumError::QueryExecution(
            "图模式的节点与边数量不匹配 (Graph pattern node and edge counts do not match)".into(),
        ));
    }
    if pattern
        .edges
        .iter()
        .any(|edge| edge.hop_range.is_some_and(|range| range.min > range.max))
    {
        return Err(TriviumError::QueryExecution(
            "路径最小跳数超过最大跳数 (Path minimum hops exceed maximum hops)".into(),
        ));
    }
    let nodes = pattern
        .nodes
        .iter()
        .filter_map(|node| node.var.as_deref())
        .collect::<HashSet<_>>();
    for name in pattern.edges.iter().filter_map(|edge| edge.var.as_deref()) {
        if nodes.contains(name) {
            return Err(TriviumError::QueryExecution(format!(
                "变量 {name} 同时绑定为节点和边 (Variable {name} is bound as both a node and an edge)"
            )));
        }
    }
    Ok(())
}

pub fn execute_tql_with_limits<T: VectorType>(
    query: &TqlQuery,
    memtable: &MemTable<T>,
    limits: TqlLimits,
) -> Result<TqlResult<T>, TriviumError> {
    let memory = QueryMemory::new(limits.memory_limit, memtable);
    limits.check_cancelled()?;
    ensure_search_vector_bound(query)?;
    if query.explain {
        // EXPLAIN 与执行共用候选规划及行数预算，不能另走忽略 LIMIT 的路径。
        let find_plan = if query.pipeline.is_empty()
            && let QueryEntry::Find { filter } = &query.entry
        {
            let ordered = find_ordered_find_plan(query, filter, memtable);
            let (row_limit, _) = requested_row_limit(
                query,
                requires_full_input(query, ordered.is_some()),
                &limits,
                memtable,
            )?;
            Some(ordered.unwrap_or_else(|| {
                plan_find_filter(query, filter, memtable, row_limit.saturating_add(1))
            }))
        } else {
            None
        };
        if query.analyze {
            let started = std::time::Instant::now();
            let mut executable = query.clone();
            executable.explain = false;
            executable.analyze = false;
            if !executable.pipeline.is_empty() {
                let (set, name, aliases, profile) =
                    execute_pipeline_set(&executable, memtable, true, &limits)?;
                let mut rows = set.into_rows();
                rows.truncate(admit_input_rows(
                    &executable,
                    memtable,
                    &limits,
                    rows.len(),
                )?);
                let inputs = rows
                    .iter()
                    .map(returning::Input::Pipeline)
                    .collect::<Vec<_>>();
                let scope = returning::Scope {
                    memory: &memory,
                    query: &executable,
                    mt: memtable,
                    limits: &limits,
                    pipeline: Some((&name, &aliases)),
                };
                let returned = returning::finish(&inputs, &scope)?;
                return Ok(generate_explain_plan(
                    query,
                    memtable,
                    Some((
                        returned.len(),
                        started.elapsed().as_secs_f64() * 1000.0,
                        profile,
                    )),
                    find_plan,
                ));
            }
            let rows = execute_tql_values_with_limits(&executable, memtable, limits)?;
            return Ok(generate_explain_plan(
                query,
                memtable,
                Some((rows.len(), started.elapsed().as_secs_f64() * 1000.0, None)),
                find_plan,
            ));
        }
        return Ok(generate_explain_plan(query, memtable, None, find_plan));
    }
    if !query.pipeline.is_empty() {
        return execute_pipeline_node_query(query, memtable, limits, &memory);
    }

    if let QueryEntry::Match { pattern } | QueryEntry::OptionalMatch { pattern } = &query.entry
        && pattern.edges.iter().any(|edge| edge.var.is_some())
    {
        let expanded = collect_single_edge_rows(query, pattern, memtable, &limits, &memory)?;
        let edge_var = pattern.edges[0].var.as_deref().ok_or_else(|| {
            TriviumError::QueryExecution("缺少边变量 (Missing edge variable)".into())
        })?;
        let inputs = expanded
            .iter()
            .map(|(nodes, edge)| {
                returning::Input::Nodes(nodes, edge.as_ref().map(|edge| (edge_var, edge)))
            })
            .collect::<Vec<_>>();
        let scope = returning::Scope {
            memory: &memory,
            query,
            mt: memtable,
            limits: &limits,
            pipeline: None,
        };
        return returning::into_nodes(returning::finish(&inputs, &scope)?, &inputs, &scope);
    }
    let (results, _) = execute_binding_rows(query, memtable, &limits, &memory)?;
    let inputs = results
        .iter()
        .map(|row| returning::Input::Nodes(row, None))
        .collect::<Vec<_>>();
    let scope = returning::Scope {
        memory: &memory,
        query,
        mt: memtable,
        limits: &limits,
        pipeline: None,
    };
    returning::into_nodes(returning::finish(&inputs, &scope)?, &inputs, &scope)
}

fn execute_binding_rows<T: VectorType>(
    query: &TqlQuery,
    memtable: &MemTable<T>,
    limits: &TqlLimits,
    memory: &QueryMemory,
) -> Result<(TqlResult<T>, bool), TriviumError> {
    let ordered_find = matches!(
        &query.entry,
        QueryEntry::Find { filter } if find_ordered_find_plan(query, filter, memtable).is_some()
    );
    let requires_full_input = requires_full_input(query, ordered_find);
    let (row_limit, user_limited) =
        requested_row_limit(query, requires_full_input, limits, memtable)?;
    let scan_limit = row_limit.saturating_add(1);

    let mut snapshot_query = query.clone();
    if matches!(
        query.entry,
        QueryEntry::Match { .. } | QueryEntry::OptionalMatch { .. }
    ) {
        snapshot_query.returns = ReturnClause::All;
    }
    let (mut results, ordered_output) = match &query.entry {
        QueryEntry::Find { filter } => execute_find(
            filter,
            query,
            memtable,
            scan_limit,
            limits.query_control.as_ref(),
            memory,
        )?,
        QueryEntry::Match { pattern } => (
            execute_match(
                pattern,
                &snapshot_query,
                memtable,
                scan_limit,
                false,
                limits,
                memory,
            )?,
            false,
        ),
        QueryEntry::OptionalMatch { pattern } => (
            execute_match(
                pattern,
                &snapshot_query,
                memtable,
                scan_limit,
                true,
                limits,
                memory,
            )?,
            false,
        ),
        QueryEntry::Search {
            vector,
            top_k,
            expand,
            ..
        } => (
            execute_search(
                vector,
                *top_k,
                expand.as_ref(),
                query,
                memtable,
                scan_limit,
                memory,
            )?,
            false,
        ),
        QueryEntry::Text { .. } => {
            return Err(TriviumError::QueryExecution(
                "TEXT 入口必须通过可组合管线执行 (TEXT entry requires composable pipeline execution)"
                    .into(),
            ));
        }
    };

    let truncated = results.len() > row_limit;
    if truncated {
        results.truncate(row_limit);
    }
    if truncated && !user_limited {
        let message =
            format!("TQL 结果超过 {row_limit} 行上限 (TQL result exceeds row limit {row_limit})");
        if requires_full_input || limits.row_overflow == crate::database::RowOverflowPolicy::Throw {
            return Err(TriviumError::QueryRowBudgetExceeded { budget: row_limit });
        }
        tracing::warn!("{}；返回明确的部分结果", message);
    }

    if let Some(rank) = &query.rank {
        results = apply_graph_first_rank(results, rank, memtable)?;
    }

    Ok((results, ordered_output))
}

fn test_disable_fusion() -> bool {
    #[cfg(feature = "test-hooks")]
    {
        matches!(
            crate::test_hooks::query_strategy(),
            crate::test_hooks::QueryExecutionStrategy::DisableFusion
        )
    }
    #[cfg(not(feature = "test-hooks"))]
    false
}

fn test_parallelism_budget() -> crate::query::parallel::QueryParallelismBudget {
    #[cfg(feature = "test-hooks")]
    {
        match crate::test_hooks::query_strategy() {
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
        }
    }
    #[cfg(not(feature = "test-hooks"))]
    crate::query::parallel::QueryParallelismBudget::default()
}

#[derive(Debug, Clone)]
struct PipelineExecutionProfile {
    plan: crate::query::cascades::CascadesPlan,
    metrics: Vec<crate::query::pipeline::PipelineStageMetrics>,
}

type PipelineSetResult = (
    crate::query::pipeline::NodeSet,
    String,
    std::collections::BTreeSet<String>,
    Option<PipelineExecutionProfile>,
);

fn execute_pipeline_set<T: VectorType>(
    query: &TqlQuery,
    mt: &MemTable<T>,
    profile: bool,
    limits: &TqlLimits,
) -> Result<PipelineSetResult, TriviumError> {
    limits.check_cancelled()?;
    requested_row_limit(query, requires_full_input(query, false), limits, mt)?;
    let source_vector = match &query.entry {
        QueryEntry::Search { vector, .. } => Some(vector.as_slice()),
        _ => None,
    };
    let plan = crate::query::cascades::optimize_pipeline_with_profile(
        query,
        mt,
        crate::query::cascades::OptimizerBudget::default(),
        profile,
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
                        .map(T::from_json_number)
                        .collect::<Result<Vec<_>, _>>()?,
                    top_k: *top_k,
                    ef_search: top_k.saturating_mul(8).max(64),
                })];
            if let Some(expand) = expand {
                operators.extend(
                    crate::query::pipeline::lower_search_entry::<T>(vector, 0, Some(expand))?
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
        ) => crate::query::pipeline::lower_search_entry(vector, *top_k, expand.as_ref())?,
        (QueryEntry::Text { clause }, _) => {
            vec![Box::new(crate::query::pipeline::TextSearchSource {
                query: clause.query.clone(),
                top_k: clause.top_k,
                k1: clause.k1,
                b: clause.b,
                ac_weight: clause.ac_weight,
                include_bm25: matches!(
                    clause.kind,
                    crate::query::tql_ast::TextSearchKind::Bm25
                        | crate::query::tql_ast::TextSearchKind::Hybrid
                ),
                include_ac: matches!(
                    clause.kind,
                    crate::query::tql_ast::TextSearchKind::Ac
                        | crate::query::tql_ast::TextSearchKind::Hybrid
                ),
            })]
        }
        (
            QueryEntry::Find {
                filter: crate::filter::Filter::Eq(field, value),
            },
            Some(crate::query::cascades::PhysicalOperator::PropertyHashLookup),
        ) => vec![Box::new(crate::query::pipeline::PropertyLookup {
            field: field.clone(),
            value: value.clone(),
        })],
        (QueryEntry::Find { filter }, Some(crate::query::cascades::PhysicalOperator::NodeScan)) => {
            vec![Box::new(crate::query::pipeline::FilteredNodeScanSource {
                filter: filter.clone(),
            })]
        }
        _ => {
            let mut source_query = query.clone();
            source_query.pipeline.clear();
            source_query.predicate = None;
            source_query.rank = None;
            source_query.order_by.clear();
            source_query.limit = None;
            source_query.offset = None;
            source_query.returns = ReturnClause::All;
            let ids = execute_tql_with_limits(
                &source_query,
                mt,
                TqlLimits {
                    max_query_rows: Some(0),
                    memory_limit: limits.memory_limit,
                    query_control: limits.query_control.clone(),
                    ..Default::default()
                },
            )?
            .into_iter()
            .flat_map(|row| row.into_values().map(|node| node.id))
            .collect();
            vec![Box::new(crate::query::pipeline::NodeIdsSource { ids })]
        }
    };
    let disable_fusion = cfg!(feature = "test-hooks") && test_disable_fusion();
    let mut current_name = "_".to_owned();
    let mut columns = std::collections::BTreeSet::<String>::from(["_".into()]);
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
        if plan.elided_stages.contains(&stage_index) && !matches!(stage, PipelineStage::With(_)) {
            continue;
        }
        match stage {
            PipelineStage::With(with) => {
                let preferred = query
                    .pipeline
                    .get(stage_index + 1)
                    .and_then(|next| match next {
                        PipelineStage::Expand(stage) => Some(stage.input.as_str()),
                        PipelineStage::GraphAlgorithm(stage) => Some(stage.input.as_str()),
                        _ => None,
                    });
                let primary = with.items.iter().find(|item| preferred == Some(item.alias.as_str()) && matches!(item.expr, TqlExpr::Variable(_)))
                    .map(|item| item.alias.clone()).or_else(|| with.items.iter().find_map(|item| {
                    matches!(&item.expr, TqlExpr::Variable(var) if var == &current_name || var == "_")
                        .then(|| item.alias.clone())
                }));
                operators.push(Box::new(TqlScopeProjection {
                    projection: with.clone(),
                    primary: primary.clone(),
                }));
                if let Some(primary) = primary {
                    current_name = primary;
                }
                columns = with.items.iter().map(|item| item.alias.clone()).collect();
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
                            .map(T::from_json_number)
                            .collect::<Result<Vec<_>, _>>()?,
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
                                .map(T::from_json_number)
                                .collect::<Result<Vec<_>, _>>()?,
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
                let subset_direction = |direction: EdgeDirection| match direction {
                    EdgeDirection::Forward => {
                        crate::graph::reachability::ReachabilityDirection::Outgoing
                    }
                    EdgeDirection::Backward => {
                        crate::graph::reachability::ReachabilityDirection::Incoming
                    }
                    EdgeDirection::Both => crate::graph::reachability::ReachabilityDirection::Both,
                };
                let mode = match &graph.subset {
                    crate::query::tql_ast::GraphSubsetSpec::Induced => {
                        crate::query::pipeline::GraphSubsetMode::Induced
                    }
                    crate::query::tql_ast::GraphSubsetSpec::Expand {
                        hops,
                        labels,
                        direction,
                    } => crate::query::pipeline::GraphSubsetMode::Expand {
                        hops: *hops,
                        labels: labels.clone(),
                        direction: subset_direction(*direction),
                    },
                    crate::query::tql_ast::GraphSubsetSpec::Boundary {
                        hops,
                        labels,
                        direction,
                    } => crate::query::pipeline::GraphSubsetMode::Boundary {
                        hops: *hops,
                        labels: labels.clone(),
                        direction: subset_direction(*direction),
                    },
                };
                match graph.algorithm {
                    GraphAlgorithmKind::PageRank => {
                        operators.push(Box::new(crate::query::pipeline::PageRankOperator {
                            mode,
                            config: Default::default(),
                            label_filter: graph.label_filter.clone(),
                        }))
                    }
                    GraphAlgorithmKind::Wcc => {
                        operators.push(Box::new(crate::query::pipeline::WccOperator {
                            mode,
                            label_filter: graph.label_filter.clone(),
                        }))
                    }
                    GraphAlgorithmKind::Degree => {
                        operators.push(Box::new(crate::query::pipeline::DegreeCentralityOperator {
                            mode,
                            label_filter: graph.label_filter.clone(),
                        }))
                    }
                    GraphAlgorithmKind::LabelPropagation => {
                        operators.push(Box::new(crate::query::pipeline::LabelPropagationOperator {
                            mode,
                            config: crate::graph::subset::LabelPropagationConfig {
                                max_iterations: 32,
                                min_community_size: 1,
                            },
                            label_filter: graph.label_filter.clone(),
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
                    GraphAlgorithmKind::Scc
                    | GraphAlgorithmKind::KCore
                    | GraphAlgorithmKind::ArticulationPoints
                    | GraphAlgorithmKind::TriangleCount
                    | GraphAlgorithmKind::Hits
                    | GraphAlgorithmKind::HarmonicCentrality => {
                        let algorithm = match graph.algorithm {
                            GraphAlgorithmKind::Scc => {
                                crate::query::pipeline::StructuralAlgorithm::Scc
                            }
                            GraphAlgorithmKind::KCore => {
                                crate::query::pipeline::StructuralAlgorithm::KCore
                            }
                            GraphAlgorithmKind::ArticulationPoints => {
                                crate::query::pipeline::StructuralAlgorithm::ArticulationPoints
                            }
                            GraphAlgorithmKind::TriangleCount => {
                                crate::query::pipeline::StructuralAlgorithm::TriangleCount
                            }
                            GraphAlgorithmKind::Hits => {
                                crate::query::pipeline::StructuralAlgorithm::Hits
                            }
                            GraphAlgorithmKind::HarmonicCentrality => {
                                crate::query::pipeline::StructuralAlgorithm::HarmonicCentrality
                            }
                            _ => {
                                return Err(TriviumError::QueryExecution(
                                    "图算法 lowering 不一致".into(),
                                ));
                            }
                        };
                        operators.push(Box::new(crate::query::pipeline::StructuralGraphOperator {
                            mode,
                            algorithm,
                            label_filter: graph.label_filter.clone(),
                            max_iterations: 20,
                            tolerance: 1e-6,
                        }));
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
            PipelineStage::WeightedPaths(paths) => {
                if paths.input != current_name {
                    return Err(TriviumError::QueryExecution(format!(
                        "WEIGHTED_PATHS 输入 {} 不是当前 NodeSet {}",
                        paths.input, current_name
                    )));
                }
                operators.push(Box::new(
                    crate::query::pipeline::WeightedShortestPathsOperator {
                        targets: paths.targets.clone(),
                        label_filter: paths.label.clone(),
                    },
                ));
                current_name = paths.output.clone();
            }
            PipelineStage::YenPaths(paths) => {
                if paths.input != current_name {
                    return Err(TriviumError::QueryExecution(format!(
                        "YEN_PATHS 输入 {} 不是当前 NodeSet {}",
                        paths.input, current_name
                    )));
                }
                operators.push(Box::new(
                    crate::query::pipeline::YenKShortestPathsOperator {
                        targets: paths.targets.clone(),
                        label_filter: paths.label.clone(),
                        k: paths.k,
                    },
                ));
                current_name = paths.output.clone();
            }
            PipelineStage::NodeSimilarity(stage) => {
                if stage.input != current_name {
                    return Err(TriviumError::QueryExecution(format!(
                        "NODE_SIMILARITY 输入 {} 不是当前 NodeSet {}",
                        stage.input, current_name
                    )));
                }
                operators.push(Box::new(crate::query::pipeline::NodeSimilarityOperator {
                    label_filter: stage.label.clone(),
                    top_k: stage.top_k,
                    cutoff: stage.cutoff,
                }));
                current_name = stage.output.clone();
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
            PipelineStage::Diversify(stage) => {
                operators.push(Box::new(crate::query::pipeline::DppDiversify {
                    top_k: stage.top_k,
                    quality_weight: stage.quality_weight,
                }));
                current_name = stage.output.clone();
            }
            PipelineStage::Residual(stage) => {
                operators.push(Box::new(crate::query::pipeline::FistaResidualRecall {
                    query: stage
                        .vector
                        .iter()
                        .map(T::from_json_number)
                        .collect::<Result<Vec<_>, _>>()?,
                    top_k: stage.top_k,
                    lambda: stage.lambda,
                    threshold: stage.threshold,
                    iterations: stage.iterations,
                }));
                current_name = stage.output.clone();
            }
            PipelineStage::Topics(stage) => {
                operators.push(Box::new(crate::query::pipeline::NmfTopics {
                    topics: stage.topics,
                    iterations: stage.iterations,
                }));
                current_name = stage.output.clone();
            }
            PipelineStage::SaPpr(stage) => {
                operators.push(Box::new(crate::query::pipeline::SaPprOperator {
                    max_depth: stage.max_depth,
                    restart_alpha: stage.restart_alpha,
                    labels: stage.labels.clone(),
                    max_edges_per_node: stage.max_edges_per_node,
                    min_edge_weight: stage.min_edge_weight,
                }));
                current_name = stage.output.clone();
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
                    collect_predicate_equalities(&predicate, &current_name, &mut equalities);
                    if !equalities.is_empty() {
                        if equalities.iter().any(|(_, value)| value.is_number()) {
                            operators.push(Box::new(TqlNumericIndexFilter { equalities }));
                        } else {
                            operators.push(Box::new(crate::query::pipeline::PropertyIndexFilter {
                                equalities,
                                strategy,
                            }));
                        }
                    }
                }
                operators.push(Box::new(PipelinePredicateFilter { predicate }));
            }
            PipelineStage::Rank(rank) => {
                // 与前一个 EXPAND 相邻时，已由 ExpandExactRerank 安全融合并执行，避免重复重排。
                if !matches!(
                    physical_stage(stage_index),
                    Some(crate::query::cascades::PhysicalOperator::RankAlreadyOrdered)
                ) && (disable_fusion
                    || !matches!(
                        stage_index.checked_sub(1).and_then(physical_stage),
                        Some(crate::query::cascades::PhysicalOperator::ExpandExactRerank)
                    ))
                {
                    operators.push(Box::new(crate::query::pipeline::ExactRerank {
                        query: rank
                            .vector
                            .iter()
                            .map(T::from_json_number)
                            .collect::<Result<Vec<_>, _>>()?,
                        top_k: Some(rank.top_k),
                    }))
                }
            }
        }
        if !matches!(
            stage,
            PipelineStage::With(_) | PipelineStage::Filter(_) | PipelineStage::Rank(_)
        ) {
            let inner = operators.pop().ok_or_else(|| {
                TriviumError::QueryExecution(
                    "管线输出算子缺失 (Missing pipeline output operator)".into(),
                )
            })?;
            columns.insert(current_name.clone());
            operators.push(Box::new(TqlOutputBinding {
                inner,
                name: current_name.clone(),
            }));
        }
    }
    let mut context = crate::query::pipeline::PipelineContext::with_profile(
        mt,
        crate::query::pipeline::PipelineBudget {
            max_nodes: MAX_BUDGET.min(memory_row_cap(limits.memory_limit, mt)),
            max_node_set_bytes: MAX_BUDGET
                .saturating_mul(std::mem::size_of::<crate::query::pipeline::NodeRow>())
                .min(if limits.memory_limit == 0 {
                    usize::MAX
                } else {
                    limits.memory_limit
                }),
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
        profile,
    );
    context.query_control = limits.query_control.clone();
    context.workspace_limit = limits.memory_limit;
    let set = crate::query::pipeline::execute_pipeline(&mut context, &operators)?;
    limits.check_cancelled()?;
    let execution_profile = profile.then_some(PipelineExecutionProfile {
        plan,
        metrics: context.metrics,
    });
    Ok((set, current_name, columns, execution_profile))
}

fn execute_pipeline_node_query<T: VectorType>(
    query: &TqlQuery,
    mt: &MemTable<T>,
    limits: TqlLimits,
    memory: &QueryMemory,
) -> Result<TqlResult<T>, TriviumError> {
    if matches!(&query.returns, ReturnClause::Expressions(items) if items.iter().any(|item| matches!(item.kind, ReturnExprKind::Scalar(_))))
    {
        return Err(TriviumError::QueryExecution(
            "节点结果 API 无法承载标量列，请使用 tql_values (Node-only result API cannot carry scalar columns; use tql_values)".into(),
        ));
    }
    let (set, current_name, columns, _) = execute_pipeline_set(query, mt, false, &limits)?;
    let mut rows = set.into_rows();
    rows.truncate(admit_input_rows(query, mt, &limits, rows.len())?);
    let inputs = rows
        .iter()
        .map(returning::Input::Pipeline)
        .collect::<Vec<_>>();
    let scope = returning::Scope {
        memory,
        query,
        mt,
        limits: &limits,
        pipeline: Some((&current_name, &columns)),
    };
    returning::into_nodes(returning::finish(&inputs, &scope)?, &inputs, &scope)
}

fn json_to_tql_value<T>(value: serde_json::Value) -> TqlValue<T> {
    match value {
        serde_json::Value::Null => TqlValue::Null,
        serde_json::Value::Bool(value) => TqlValue::Bool(value),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                TqlValue::Int(value)
            } else if let Some(value) = value.as_u64() {
                TqlValue::UInt(value)
            } else {
                TqlValue::Float(value.as_f64().unwrap_or(f64::NAN))
            }
        }
        serde_json::Value::String(value) => TqlValue::String(value),
        serde_json::Value::Array(value) => TqlValue::List(value),
        serde_json::Value::Object(value) => TqlValue::Object(value),
    }
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
    let memory = QueryMemory::new(limits.memory_limit, mt);
    limits.check_cancelled()?;
    ensure_search_vector_bound(query)?;
    if !query.explain
        && query.pipeline.is_empty()
        && let QueryEntry::Match { pattern } | QueryEntry::OptionalMatch { pattern } = &query.entry
        && pattern.edges.iter().any(|edge| edge.var.is_some())
    {
        return execute_single_edge_values(query, pattern, mt, limits, &memory);
    }
    if query.explain {
        return execute_tql_with_limits(query, mt, limits).map(|rows| {
            rows.into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|(name, node)| (name, TqlValue::Node(node)))
                        .collect()
                })
                .collect()
        });
    }
    if query.pipeline.is_empty() {
        let (rows, _) = execute_binding_rows(query, mt, &limits, &memory)?;
        let inputs = rows
            .iter()
            .map(|row| returning::Input::Nodes(row, None))
            .collect::<Vec<_>>();
        let scope = returning::Scope {
            memory: &memory,
            query,
            mt,
            limits: &limits,
            pipeline: None,
        };
        return returning::finish(&inputs, &scope)
            .map(|rows| rows.into_iter().map(|row| row.values).collect());
    }
    let (set, current_name, columns, _) = execute_pipeline_set(query, mt, false, &limits)?;
    let mut rows = set.into_rows();
    rows.truncate(admit_input_rows(query, mt, &limits, rows.len())?);
    let inputs = rows
        .iter()
        .map(returning::Input::Pipeline)
        .collect::<Vec<_>>();
    let scope = returning::Scope {
        memory: &memory,
        query,
        mt,
        limits: &limits,
        pipeline: Some((&current_name, &columns)),
    };
    returning::finish(&inputs, &scope).map(|rows| rows.into_iter().map(|row| row.values).collect())
}

fn execute_single_edge_values<T: VectorType>(
    query: &TqlQuery,
    pattern: &TqlPattern,
    mt: &MemTable<T>,
    limits: TqlLimits,
    memory: &QueryMemory,
) -> Result<TqlValueResult<T>, TriviumError> {
    let expanded = collect_single_edge_rows(query, pattern, mt, &limits, memory)?;
    let edge_var = pattern.edges[0]
        .var
        .as_deref()
        .ok_or_else(|| TriviumError::QueryExecution("缺少边变量 (Missing edge variable)".into()))?;
    let inputs = expanded
        .iter()
        .map(|(nodes, edge)| {
            returning::Input::Nodes(nodes, edge.as_ref().map(|edge| (edge_var, edge)))
        })
        .collect::<Vec<_>>();
    let scope = returning::Scope {
        memory,
        query,
        mt,
        limits: &limits,
        pipeline: None,
    };
    returning::finish(&inputs, &scope).map(|rows| rows.into_iter().map(|row| row.values).collect())
}

type EdgeBindingRow<T> = (HashMap<String, Node<T>>, Option<EdgeView>);

fn collect_single_edge_rows<T: VectorType>(
    query: &TqlQuery,
    pattern: &TqlPattern,
    mt: &MemTable<T>,
    limits: &TqlLimits,
    memory: &QueryMemory,
) -> Result<Vec<EdgeBindingRow<T>>, TriviumError> {
    if pattern.edges.len() != 1 || pattern.edges[0].hop_range.is_some() || pattern.nodes.len() != 2
    {
        return Err(TriviumError::QueryExecution(
            "具名边要求单跳模式 (Named edges require a single-hop pattern)".into(),
        ));
    }
    let optional = matches!(query.entry, QueryEntry::OptionalMatch { .. });
    let plan = plan_match(pattern, mt, optional);
    let left = &plan.pattern.nodes[0];
    let right = &plan.pattern.nodes[1];
    let edge_pattern = &plan.pattern.edges[0];
    let edge_var = edge_pattern
        .var
        .as_deref()
        .ok_or_else(|| TriviumError::QueryExecution("缺少边变量 (Missing edge variable)".into()))?;
    let candidates: Box<dyn Iterator<Item = NodeId> + '_> = if matches!(
        plan.access_path,
        AccessPath::FullNodeScan | AccessPath::ColdPayloadScan
    ) {
        Box::new(mt.active_node_ids())
    } else {
        Box::new(plan.candidates.iter().copied())
    };
    let (row_limit, _) = requested_row_limit(query, requires_full_input(query, false), limits, mt)?;
    let scope = returning::Scope {
        memory,
        query,
        mt,
        limits,
        pipeline: None,
    };
    let mut expanded = Vec::new();
    let mut budget = matching::WorkBudget::new(MAX_BUDGET, limits.query_control.as_ref());
    'anchors: for start in candidates {
        limits.check_cancelled()?;
        if left
            .filter
            .as_ref()
            .is_some_and(|filter| !matches_filter_with_id(filter, start, mt))
        {
            continue;
        }
        // 与匿名边共享物理边枚举、取消和步数预算；排序保留具名边的确定性。
        let mut edges = BTreeMap::new();
        let mut neighbors = matching::Neighbors::new(mt, start, edge_pattern.direction);
        while let Some((source, edge)) = neighbors.next(mt, &edge_pattern.labels, &mut budget)? {
            let bytes = edges
                .len()
                .saturating_add(1)
                .saturating_mul(std::mem::size_of::<(NodeId, NodeId, &str)>())
                .saturating_mul(4);
            if limits.memory_limit > 0 && bytes > limits.memory_limit {
                return Err(TriviumError::QueryRowBudgetExceeded {
                    budget: edges.len(),
                });
            }
            edges.insert((source, edge.target_id, edge.label.as_str()), edge);
        }
        let mut matched = false;
        for ((source, target, _), edge) in edges {
            let end = if source == start { target } else { source };
            if left.var.is_some() && left.var == right.var && start != end {
                continue;
            }
            if right
                .filter
                .as_ref()
                .is_some_and(|filter| !matches_filter_with_id(filter, end, mt))
            {
                continue;
            }
            let mut nodes = HashMap::new();
            for (pattern, id) in [(left, start), (right, end)] {
                if let Some(var) = &pattern.var {
                    nodes.insert(var.clone(), memory.snapshot(id, mt)?);
                }
            }
            memory.retain(
                edge.label
                    .len()
                    .saturating_add(crate::storage::payload_store::query_clone_peak(
                        &edge.metadata,
                    ))
                    .saturating_add(256),
            )?;
            let view = EdgeView {
                source_id: source,
                target_id: target,
                label: edge.label.clone(),
                weight: edge.weight,
                metadata: edge.metadata.clone(),
            };
            let input = returning::Input::Nodes(&nodes, Some((edge_var, &view)));
            if query
                .predicate
                .as_ref()
                .is_some_and(|predicate| !returning::matches_predicate(predicate, &input, &scope))
            {
                continue;
            }
            matched = true;
            expanded.push((nodes, Some(view)));
            if expanded.len() > row_limit {
                break 'anchors;
            }
        }
        if optional && !matched {
            let mut nodes = HashMap::new();
            if let Some(var) = &left.var {
                nodes.insert(var.clone(), memory.snapshot(start, mt)?);
            }
            expanded.push((nodes, None));
            if expanded.len() > row_limit {
                break;
            }
        }
    }
    expanded.truncate(admit_input_rows(query, mt, limits, expanded.len())?);
    if let Some(rank) = &query.rank {
        expanded = rank_rows_by_node(expanded, rank, mt, |row| &row.0)?;
    }
    Ok(expanded)
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

fn pipeline_kind_value<T: VectorType>(
    kind: &ReturnExprKind,
    row: &crate::query::pipeline::NodeRow,
    mt: &MemTable<T>,
    memory: &QueryMemory,
) -> Result<TqlValue<T>, TriviumError> {
    match kind {
        ReturnExprKind::Var(var) => runtime_to_tql_value(
            eval_pipeline_row_expr(&TqlExpr::Variable(var.clone()), row, mt),
            mt,
            memory,
        ),
        ReturnExprKind::Property(var, field) => runtime_to_tql_value(
            eval_pipeline_row_expr(
                &TqlExpr::Property {
                    var: var.clone(),
                    field: field.clone(),
                },
                row,
                mt,
            ),
            mt,
            memory,
        ),
        ReturnExprKind::Scalar(expr) => {
            runtime_to_tql_value(eval_pipeline_row_expr(expr, row, mt), mt, memory)
        }
        ReturnExprKind::Aggregate(_, _) => Err(TriviumError::QueryExecution(
            "不允许嵌套聚合 (Nested aggregation is not supported)".into(),
        )),
    }
}

struct TqlScopeProjection {
    projection: WithStage,
    primary: Option<String>,
}

impl<T: VectorType> crate::query::pipeline::PipelineOperator<T> for TqlScopeProjection {
    fn name(&self) -> &'static str {
        "scope_projection"
    }
    fn apply(
        &self,
        input: crate::query::pipeline::NodeSet,
        context: &mut crate::query::pipeline::PipelineContext<'_, T>,
    ) -> Result<crate::query::pipeline::NodeSet, TriviumError> {
        let mut rows = Vec::with_capacity(input.len());
        for mut row in input.into_rows() {
            // 同一 WITH 的所有右值读取进入阶段时的环境，再同时安装新作用域。
            let values: BTreeMap<_, _> = self
                .projection
                .items
                .iter()
                .map(|item| {
                    (
                        item.alias.clone(),
                        eval_pipeline_row_expr(&item.expr, &row, context.memtable),
                    )
                })
                .collect();
            if let Some(RuntimeValue::Node(id)) =
                self.primary.as_ref().and_then(|name| values.get(name))
            {
                row.id = *id;
            }
            row.bindings = Some(Arc::new(values));
            rows.push(row);
        }
        let output = crate::query::pipeline::NodeSet::from_rows(rows);
        if output.estimated_bytes() > context.budget.max_node_set_bytes {
            return Err(TriviumError::QueryExecution(
                "WITH 投影超过内存预算 (WITH projection exceeds memory budget)".into(),
            ));
        }
        Ok(output)
    }
}

struct TqlOutputBinding<T: VectorType> {
    inner: Box<dyn crate::query::pipeline::PipelineOperator<T>>,
    name: String,
}
impl<T: VectorType> crate::query::pipeline::PipelineOperator<T> for TqlOutputBinding<T> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn apply(
        &self,
        input: crate::query::pipeline::NodeSet,
        context: &mut crate::query::pipeline::PipelineContext<'_, T>,
    ) -> Result<crate::query::pipeline::NodeSet, TriviumError> {
        let mut origins = BTreeMap::<NodeId, Vec<_>>::new();
        for row in input.rows() {
            if let Some(bindings) = &row.bindings {
                origins.entry(row.id).or_default().push(bindings.clone());
            }
        }
        let output = self.inner.apply(input, context)?;
        let mut output_counts = BTreeMap::<NodeId, usize>::new();
        for row in output.rows() {
            *output_counts.entry(row.id).or_default() += 1;
        }
        let mut rows = Vec::new();
        for row in output.into_rows() {
            let lost_contexts = origins
                .get(&row.id)
                .is_some_and(|values| values.len() > output_counts[&row.id]);
            let inherited = if row.bindings.is_some() && !lost_contexts {
                Vec::new()
            } else if let Some(values) = origins.get(&row.id) {
                values.clone()
            } else {
                let source = row
                    .path
                    .as_ref()
                    .and_then(|path| path.first())
                    .copied()
                    .or_else(|| {
                        (row.provenance.source_ids.len() == 1).then(|| row.provenance.source_ids[0])
                    });
                source
                    .and_then(|source| origins.get(&source))
                    .cloned()
                    .unwrap_or_default()
            };
            let candidates = if inherited.is_empty() {
                vec![row]
            } else {
                inherited
                    .into_iter()
                    .map(|bindings| {
                        let mut inherited = row.clone();
                        inherited.bindings = Some(bindings);
                        inherited
                    })
                    .collect()
            };
            for mut row in candidates {
                let bindings = row
                    .bindings
                    .get_or_insert_with(|| Arc::new(BTreeMap::new()));
                Arc::make_mut(bindings).insert(self.name.clone(), RuntimeValue::Node(row.id));
                rows.push(row);
            }
        }
        Ok(crate::query::pipeline::NodeSet::from_rows(rows))
    }
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
    current_name: &str,
    output: &mut Vec<(String, serde_json::Value)>,
) {
    match predicate {
        Predicate::Compare {
            left: TqlExpr::Property { var, field },
            op: TqlCompOp::Eq,
            right: TqlExpr::Literal(value),
        }
        | Predicate::Compare {
            left: TqlExpr::Literal(value),
            op: TqlCompOp::Eq,
            right: TqlExpr::Property { var, field },
        } if field != "id" && var == current_name => {
            output.push((field.clone(), tql_literal_to_json(value)))
        }
        Predicate::And(left, right) => {
            collect_predicate_equalities(left, current_name, output);
            collect_predicate_equalities(right, current_name, output);
        }
        _ => {}
    }
}

fn filter_is_equality_conjunction(filter: &Filter) -> bool {
    match filter {
        Filter::Eq(field, _) => field != "id",
        Filter::And(filters) => {
            !filters.is_empty() && filters.iter().all(filter_is_equality_conjunction)
        }
        _ => false,
    }
}

fn collect_filter_equalities(filter: &Filter, output: &mut Vec<(String, serde_json::Value)>) {
    match filter {
        Filter::Eq(field, value) if field != "id" => {
            output.push((field.clone(), value.clone()));
        }
        Filter::And(filters) => {
            for filter in filters {
                collect_filter_equalities(filter, output);
            }
        }
        _ => {}
    }
}

struct PipelinePredicateFilter {
    predicate: Predicate,
}

fn numeric_equality_keys(value: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut keys = vec![value.clone()];
    let Some(number) = value.as_number() else {
        return keys;
    };
    let Some(float) = number.as_f64() else {
        return keys;
    };
    // TQL 数值相等跨整数和浮点表示；属性索引仍保留 JSON 的类型键。
    let mut candidates = Vec::with_capacity(3);
    if let Some(candidate) = serde_json::Number::from_f64(float) {
        candidates.push(candidate);
    }
    if float.fract() == 0.0 {
        if (i64::MIN as f64..-(i64::MIN as f64)).contains(&float) {
            candidates.push(serde_json::Number::from(float as i64));
        }
        if (0.0..18_446_744_073_709_551_616.0).contains(&float) {
            candidates.push(serde_json::Number::from(float as u64));
        }
    }
    for candidate in candidates {
        if crate::filter::numeric_ordering(number, &candidate) == std::cmp::Ordering::Equal {
            let candidate = serde_json::Value::Number(candidate);
            if !keys.contains(&candidate) {
                keys.push(candidate);
            }
        }
    }
    keys
}

struct TqlNumericIndexFilter {
    equalities: Vec<(String, serde_json::Value)>,
}

impl<T: VectorType> crate::query::pipeline::PipelineOperator<T> for TqlNumericIndexFilter {
    fn name(&self) -> &'static str {
        "tql_numeric_index_filter"
    }

    fn apply(
        &self,
        input: crate::query::pipeline::NodeSet,
        context: &mut crate::query::pipeline::PipelineContext<'_, T>,
    ) -> Result<crate::query::pipeline::NodeSet, TriviumError> {
        let mut rows = input.into_rows();
        for (field, value) in &self.equalities {
            let mut allowed = HashSet::new();
            let mut complete = true;
            for key in numeric_equality_keys(value) {
                let Some(ids) = context.memtable.find_by_property_index(field, &key) else {
                    complete = false;
                    break;
                };
                allowed.extend(ids);
            }
            // 只有完整的等价键并集才可缩小候选；最终谓词统一复核。
            if complete {
                rows.retain(|row| allowed.contains(&row.id));
            }
        }
        Ok(crate::query::pipeline::NodeSet::from_rows(rows))
    }
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

fn eval_pipeline_row_expr<T: VectorType>(
    expr: &TqlExpr,
    row: &crate::query::pipeline::NodeRow,
    mt: &MemTable<T>,
) -> RuntimeValue {
    match expr {
        TqlExpr::Variable(var) => row
            .bindings
            .as_ref()
            .and_then(|bindings| bindings.get(var))
            .cloned()
            .unwrap_or_else(|| {
                if row.bindings.is_none() && var == "_" {
                    RuntimeValue::Node(row.id)
                } else {
                    RuntimeValue::Null
                }
            }),
        TqlExpr::Property { var, field } => {
            if let Some(value) = row.bindings.as_ref().and_then(|bindings| bindings.get(var)) {
                return runtime_property(value, field, mt);
            }
            if row.bindings.is_some() || var != "_" {
                return RuntimeValue::Null;
            }
            if field == "id" {
                runtime_u64(row.id)
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
        TqlExpr::GraphMetric { metric, .. } => {
            crate::query::pipeline::graph_metric_from_name(metric)
                .and_then(|metric| row.graph_metric(metric))
                .map(|score| RuntimeValue::Float(score.value as f64))
                .unwrap_or(RuntimeValue::Null)
        }
        TqlExpr::TextScore { .. } => row
            .text_score
            .map(|score| RuntimeValue::Float(score.value as f64))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::DiversityScore { .. } => row
            .diversity_score
            .map(|score| RuntimeValue::Float(score.value as f64))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::ResidualScore { .. } => row
            .residual_score
            .map(|score| RuntimeValue::Float(score.value as f64))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::Topic { .. } => row
            .topic_id
            .map(|topic| RuntimeValue::Int(topic as i64))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::TopicScore { .. } => row
            .topic_score
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
        TqlExpr::PathRank { .. } => row
            .path_rank
            .map(|rank| RuntimeValue::Int(rank as i64))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::PairLeft { .. } => row
            .pair
            .map(|(left, _)| runtime_u64(left))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::PairRight { .. } => row
            .pair
            .map(|(_, right)| runtime_u64(right))
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
        Predicate::DocFilter { var, filter } => {
            let bound = var
                .as_ref()
                .and_then(|var| row.bindings.as_ref().and_then(|bindings| bindings.get(var)));
            match bound {
                Some(RuntimeValue::Node(id)) => mt
                    .get_payload(*id)
                    .is_some_and(|payload| filter.matches(&payload)),
                Some(RuntimeValue::Json(value)) => filter.matches(value),
                Some(_) => false,
                None => mt
                    .get_payload(row.id)
                    .is_some_and(|payload| filter.matches(&payload)),
            }
        }
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

fn runtime_to_tql_value<T: VectorType>(
    value: RuntimeValue,
    mt: &MemTable<T>,
    memory: &QueryMemory,
) -> Result<TqlValue<T>, TriviumError> {
    Ok(match value {
        RuntimeValue::Int(value) => TqlValue::Int(value),
        RuntimeValue::UInt(value) => TqlValue::UInt(value),
        RuntimeValue::Float(value) => TqlValue::Float(value),
        RuntimeValue::Str(value) => TqlValue::String(value),
        RuntimeValue::Bool(value) => TqlValue::Bool(value),
        RuntimeValue::Path(value) => TqlValue::Path(value),
        RuntimeValue::Json(value) => json_to_tql_value(value),
        RuntimeValue::Node(id) => TqlValue::Node(memory.snapshot(id, mt)?),
        RuntimeValue::Edge(edge) => TqlValue::Edge(edge),
        RuntimeValue::Null => TqlValue::Null,
    })
}

fn apply_graph_first_rank<T: VectorType>(
    rows: TqlResult<T>,
    rank: &RankClause,
    mt: &MemTable<T>,
) -> Result<TqlResult<T>, TriviumError> {
    rank_rows_by_node(rows, rank, mt, |row| row)
}

fn rank_rows_by_node<T: VectorType, R, F: Fn(&R) -> &HashMap<String, Node<T>>>(
    rows: Vec<R>,
    rank: &RankClause,
    mt: &MemTable<T>,
    bindings: F,
) -> Result<Vec<R>, TriviumError> {
    let query_vector: Vec<T> = rank
        .vector
        .iter()
        .map(T::from_json_number)
        .collect::<Result<Vec<_>, _>>()?;
    if query_vector.len() != mt.dim() {
        return Err(TriviumError::DimensionMismatch {
            expected: mt.dim(),
            got: query_vector.len(),
        });
    }
    crate::vector::validate_query_vector(&query_vector, mt.dim())?;
    let mut canonical = BTreeMap::<NodeId, (Vec<NodeId>, R)>::new();
    for row in rows {
        let nodes = bindings(&row);
        let anchor = nodes.get(&rank.var).ok_or_else(|| {
            TriviumError::QueryExecution(format!("RANK 变量 {} 未在 MATCH 中绑定", rank.var))
        })?;
        let mut bindings: Vec<(&str, NodeId)> = nodes
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
    memory: &QueryMemory,
) -> Result<(TqlResult<T>, bool), TriviumError> {
    let mut results = Vec::new();
    let ordered_plan = find_ordered_find_plan(query, filter, mt);
    let ordered_output = ordered_plan.is_some();
    let access_plan = ordered_plan
        .clone()
        .unwrap_or_else(|| plan_find_filter(query, filter, mt, row_limit));
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

        let _decode = memory.temporary(mt.payload_query_peak(id)?)?;
        let payload = match mt.try_get_payload(id)? {
            Some(p) => p,
            None => continue,
        };

        // 索引已完整覆盖简单等值条件时，只读取 Payload 供投影使用。
        if !filter_covered && !filter.matches(&payload) {
            continue;
        }

        // WHERE 二次过滤（FIND 场景下 var=None 作用于当前节点）
        if let Some(pred) = &query.predicate
            && !eval_predicate_single(pred, id, mt)
        {
            continue;
        }

        memory.retain(memory::node_bytes(id, mt)?)?;
        let node = match build_node_with_payload(id, payload, mt) {
            Some(node) => node,
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
fn plan_find_filter<T: VectorType>(
    query: &TqlQuery,
    filter: &Filter,
    mt: &MemTable<T>,
    row_limit: usize,
) -> NodeAccessPlan {
    let candidate_limit = (query.predicate.is_none()
        && !matches!(filter, Filter::Eq(field, _) if field == "id"))
    .then_some(row_limit);
    plan_filter_with_limit(filter, candidate_limit, mt)
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
    if field == "id" {
        return None;
    }
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
    limits: &TqlLimits,
    memory: &QueryMemory,
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
    let full_scan = matches!(
        match_plan.access_path,
        AccessPath::FullNodeScan | AccessPath::ColdPayloadScan
    );
    let start_candidates: Box<dyn Iterator<Item = NodeId> + '_> = if full_scan {
        Box::new(mt.active_node_ids())
    } else {
        Box::new(match_plan.candidates.into_iter())
    };

    let mut results = Vec::new();
    let step_budget = if pattern.edges.is_empty() {
        usize::MAX
    } else {
        MAX_BUDGET
    };

    let bytes = matching::workspace_bytes(planned_pattern, mt.node_count())?;
    if limits.memory_limit > 0 && bytes > limits.memory_limit {
        return Err(TriviumError::ResourceBudgetExceeded {
            resource: "query_memory_bytes",
            requested: bytes,
            used: 0,
            limit: limits.memory_limit,
        });
    }
    let _workspace = mt.resource_runtime().reserve_workspace(bytes)?;
    let mut budget = matching::WorkBudget::new(step_budget, limits.query_control.as_ref());
    for start_id in start_candidates {
        limits.check_cancelled()?;
        if planned_pattern.nodes[0]
            .filter
            .as_ref()
            .is_some_and(|filter| !matches_filter_with_id(filter, start_id, mt))
        {
            continue;
        }
        let result_start = results.len();
        let mut env = vec![None; var_map.len()];
        let cont = matching::walk(
            mt,
            planned_pattern,
            query.predicate.as_ref(),
            &return_vars,
            &var_map,
            &mut env,
            start_id,
            &mut results,
            &mut budget,
            row_limit,
            memory,
        )?;
        if !cont {
            break;
        }
        if optional && results.len() == result_start {
            let mut row = HashMap::new();
            if let Some(var) = &pattern.nodes[0].var
                && return_vars.contains(var)
                && let node = memory.snapshot(start_id, mt)?
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

// ═══════════════════════════════════════════════════════════════════════
//  SEARCH 执行路径 (桥接到向量管线)
// ═══════════════════════════════════════════════════════════════════════

fn execute_search<T: VectorType>(
    vector: &[serde_json::Number],
    top_k: usize,
    expand: Option<&ExpandClause>,
    query: &TqlQuery,
    mt: &MemTable<T>,
    row_limit: usize,
    memory: &QueryMemory,
) -> Result<TqlResult<T>, TriviumError> {
    let mut operators = crate::query::pipeline::lower_search_entry(vector, top_k, expand)?;
    let post_filter = if expand.is_none() {
        query
            .predicate
            .as_ref()
            .and_then(|predicate| match predicate {
                Predicate::DocFilter { filter, .. } if filter_is_equality_conjunction(filter) => {
                    let mut equalities = Vec::new();
                    collect_filter_equalities(filter, &mut equalities);
                    let candidates = if equalities.len() == 1 {
                        mt.find_by_property_index(&equalities[0].0, &equalities[0].1)
                    } else {
                        mt.find_by_composite_property_index(&equalities)
                            .map(|(_, ids)| ids)
                            .or_else(|| mt.find_by_bitmap_intersection(&equalities))
                    }?;
                    Some((filter.clone(), candidates))
                }
                _ => None,
            })
    } else {
        None
    };
    if let Some((filter, candidates)) = post_filter {
        operators = vec![
            Box::new(crate::query::pipeline::NodeIdsSource { ids: candidates }),
            Box::new(crate::query::pipeline::ExactRerank {
                query: vector
                    .iter()
                    .map(T::from_json_number)
                    .collect::<Result<Vec<_>, _>>()?,
                top_k: Some(top_k),
            }),
            Box::new(crate::query::pipeline::PayloadFilter { filter }),
        ];
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
        {
            let node = memory.snapshot(id, mt)?;
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
                    Some(payload) => filter.matches(&payload),
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
            Some(payload) => filter.matches(&payload),
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

fn tql_value_runtime<T>(value: &TqlValue<T>) -> RuntimeValue {
    match value {
        TqlValue::Node(value) => RuntimeValue::Node(value.id),
        TqlValue::Edge(value) => RuntimeValue::Edge(value.clone()),
        TqlValue::Path(value) => RuntimeValue::Path(value.clone()),
        TqlValue::Int(value) => RuntimeValue::Int(*value),
        TqlValue::UInt(value) => RuntimeValue::UInt(*value),
        TqlValue::Float(value) => RuntimeValue::Float(*value),
        TqlValue::String(value) => RuntimeValue::Str(value.clone()),
        TqlValue::Bool(value) => RuntimeValue::Bool(*value),
        TqlValue::List(value) => RuntimeValue::Json(serde_json::Value::Array(value.clone())),
        TqlValue::Object(value) => RuntimeValue::Json(serde_json::Value::Object(value.clone())),
        TqlValue::Null => RuntimeValue::Null,
    }
}

fn runtime_property<T: VectorType>(
    value: &RuntimeValue,
    field: &str,
    mt: &MemTable<T>,
) -> RuntimeValue {
    match value {
        RuntimeValue::Node(id) if field == "id" => runtime_u64(*id),
        RuntimeValue::Node(id) => mt
            .get_payload(*id)
            .map_or(RuntimeValue::Null, |value| json_to_runtime(&value[field])),
        RuntimeValue::Json(value) => json_to_runtime(&value[field]),
        RuntimeValue::Edge(edge) => match field {
            "source" | "source_id" => runtime_u64(edge.source_id),
            "target" | "target_id" => runtime_u64(edge.target_id),
            "label" => RuntimeValue::Str(edge.label.clone()),
            "weight" => RuntimeValue::Float(f64::from(edge.weight)),
            "metadata" => json_to_runtime(&edge.metadata),
            _ => json_to_runtime(&edge.metadata[field]),
        },
        _ => RuntimeValue::Null,
    }
}

fn eval_tql_expr<T: VectorType>(
    expr: &TqlExpr,
    env: &[Option<u64>],
    var_map: &HashMap<String, usize>,
    mt: &MemTable<T>,
) -> RuntimeValue {
    match expr {
        TqlExpr::Variable(var) => var_map
            .get(var)
            .and_then(|index| env.get(*index))
            .copied()
            .flatten()
            .map_or(RuntimeValue::Null, RuntimeValue::Node),
        TqlExpr::Parameter(_) => RuntimeValue::Null,
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
        TqlExpr::Path { .. }
        | TqlExpr::PathLength { .. }
        | TqlExpr::PathRank { .. }
        | TqlExpr::PairLeft { .. }
        | TqlExpr::PairRight { .. } => RuntimeValue::Null,
        TqlExpr::Similarity { .. }
        | TqlExpr::GraphScore { .. }
        | TqlExpr::GraphMetric { .. }
        | TqlExpr::TextScore { .. }
        | TqlExpr::DiversityScore { .. }
        | TqlExpr::ResidualScore { .. }
        | TqlExpr::Topic { .. }
        | TqlExpr::TopicScore { .. }
        | TqlExpr::Depth { .. }
        | TqlExpr::PathStrength { .. }
        | TqlExpr::PathCount { .. }
        | TqlExpr::Community { .. } => RuntimeValue::Null,
        TqlExpr::Property { var, field } => {
            if let Some(&idx) = var_map.get(var)
                && let Some(id) = env[idx]
            {
                if field == "id" {
                    return runtime_u64(id);
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
        TqlExpr::Variable(_) => RuntimeValue::Node(id),
        TqlExpr::Parameter(_) => RuntimeValue::Null,
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
        | TqlExpr::GraphMetric { .. }
        | TqlExpr::TextScore { .. }
        | TqlExpr::DiversityScore { .. }
        | TqlExpr::ResidualScore { .. }
        | TqlExpr::Topic { .. }
        | TqlExpr::TopicScore { .. }
        | TqlExpr::Depth { .. }
        | TqlExpr::PathStrength { .. }
        | TqlExpr::PathCount { .. }
        | TqlExpr::Community { .. }
        | TqlExpr::Path { .. }
        | TqlExpr::PathLength { .. }
        | TqlExpr::PathRank { .. }
        | TqlExpr::PairLeft { .. }
        | TqlExpr::PairRight { .. } => RuntimeValue::Null,
        TqlExpr::Property { field, .. } => {
            if field == "id" {
                return runtime_u64(id);
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
            } else if let Some(value) = n.as_u64() {
                RuntimeValue::UInt(value)
            } else {
                RuntimeValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => RuntimeValue::Str(s.clone()),
        serde_json::Value::Bool(b) => RuntimeValue::Bool(*b),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => RuntimeValue::Json(v.clone()),
        serde_json::Value::Null => RuntimeValue::Null,
    }
}

fn lit_to_runtime(lit: &TqlLiteral) -> RuntimeValue {
    match lit {
        TqlLiteral::Int(n) => RuntimeValue::Int(*n),
        TqlLiteral::UInt(n) => RuntimeValue::UInt(*n),
        TqlLiteral::Float(f) => RuntimeValue::Float(*f),
        TqlLiteral::Str(s) => RuntimeValue::Str(s.clone()),
        TqlLiteral::Bool(b) => RuntimeValue::Bool(*b),
        TqlLiteral::Null => RuntimeValue::Null,
    }
}

fn runtime_u64(value: u64) -> RuntimeValue {
    i64::try_from(value).map_or(RuntimeValue::UInt(value), RuntimeValue::Int)
}

fn runtime_number(value: &RuntimeValue) -> Option<serde_json::Number> {
    match value {
        RuntimeValue::Int(v) => Some((*v).into()),
        RuntimeValue::UInt(v) => Some((*v).into()),
        RuntimeValue::Float(v) => serde_json::Number::from_f64(*v),
        _ => None,
    }
}

fn edge_json(edge: &EdgeView) -> serde_json::Value {
    serde_json::json!({"source":edge.source_id,"target":edge.target_id,"label":edge.label,"weight":edge.weight,"metadata":edge.metadata})
}

/// 容器按完整内容比较，其中所有数字共用精确数值比较。
fn compare_json(left: &serde_json::Value, right: &serde_json::Value) -> std::cmp::Ordering {
    use serde_json::Value;
    let kind = |value: &Value| match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    };
    match (left, right) {
        (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Number(a), Value::Number(b)) => crate::filter::numeric_ordering(a, b),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Array(a), Value::Array(b)) => a
            .iter()
            .zip(b)
            .map(|(a, b)| compare_json(a, b))
            .find(|order| !order.is_eq())
            .unwrap_or_else(|| a.len().cmp(&b.len())),
        (Value::Object(a), Value::Object(b)) => a
            .iter()
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .zip(b.iter().collect::<BTreeMap<_, _>>())
            .map(|((ka, va), (kb, vb))| ka.cmp(kb).then_with(|| compare_json(va, vb)))
            .find(|order| !order.is_eq())
            .unwrap_or_else(|| a.len().cmp(&b.len())),
        _ => kind(left).cmp(&kind(right)),
    }
}

fn integer_runtime(value: i128) -> RuntimeValue {
    if value < 0 {
        i64::try_from(value).map_or(RuntimeValue::Null, RuntimeValue::Int)
    } else {
        u64::try_from(value).map_or(RuntimeValue::Null, runtime_u64)
    }
}

fn eval_binary(left: RuntimeValue, op: TqlBinaryOp, right: RuntimeValue) -> RuntimeValue {
    let integer = |value: &RuntimeValue| match value {
        RuntimeValue::Int(v) => Some(i128::from(*v)),
        RuntimeValue::UInt(v) => Some(i128::from(*v)),
        _ => None,
    };
    if op != TqlBinaryOp::Divide
        && let (Some(left), Some(right)) = (integer(&left), integer(&right))
    {
        let value = match op {
            TqlBinaryOp::Add => left.checked_add(right),
            TqlBinaryOp::Subtract => left.checked_sub(right),
            TqlBinaryOp::Multiply => left.checked_mul(right),
            TqlBinaryOp::Divide => None,
        };
        return value.map_or(RuntimeValue::Null, integer_runtime);
    }
    let Some(left) = runtime_number(&left).and_then(|n| n.as_f64()) else {
        return RuntimeValue::Null;
    };
    let Some(right) = runtime_number(&right).and_then(|n| n.as_f64()) else {
        return RuntimeValue::Null;
    };
    let value = match op {
        TqlBinaryOp::Add => left + right,
        TqlBinaryOp::Subtract => left - right,
        TqlBinaryOp::Multiply => left * right,
        TqlBinaryOp::Divide if right != 0.0 => left / right,
        TqlBinaryOp::Divide => return RuntimeValue::Null,
    };
    if value.is_finite() {
        RuntimeValue::Float(value)
    } else {
        RuntimeValue::Null
    }
}

fn compare_runtime(lhs: &RuntimeValue, op: &TqlCompOp, rhs: &RuntimeValue) -> bool {
    if let (Some(left), Some(right)) = (runtime_number(lhs), runtime_number(rhs)) {
        return cmp_ord(
            &crate::filter::numeric_ordering(&left, &right),
            op,
            &std::cmp::Ordering::Equal,
        );
    }
    match (lhs, rhs) {
        (RuntimeValue::Path(a), RuntimeValue::Path(b)) => match op {
            TqlCompOp::Eq => a == b,
            TqlCompOp::Ne => a != b,
            _ => false,
        },
        (RuntimeValue::Node(a), RuntimeValue::Node(b)) => match op {
            TqlCompOp::Eq => a == b,
            TqlCompOp::Ne => a != b,
            _ => false,
        },
        (RuntimeValue::Edge(a), RuntimeValue::Edge(b)) => {
            let equal =
                (a.source_id, a.target_id, &a.label) == (b.source_id, b.target_id, &b.label);
            match op {
                TqlCompOp::Eq => equal,
                TqlCompOp::Ne => !equal,
                _ => false,
            }
        }
        (RuntimeValue::Json(a), RuntimeValue::Json(b)) => match op {
            TqlCompOp::Eq => compare_json(a, b).is_eq(),
            TqlCompOp::Ne => !compare_json(a, b).is_eq(),
            _ => false,
        },
        (RuntimeValue::Str(a), RuntimeValue::Str(b)) => cmp_ord(a, op, b),
        (RuntimeValue::Bool(a), RuntimeValue::Bool(b)) => match op {
            TqlCompOp::Eq => a == b,
            TqlCompOp::Ne => a != b,
            _ => false,
        },
        (RuntimeValue::Null, _) | (_, RuntimeValue::Null) => false,
        _ => matches!(op, TqlCompOp::Ne),
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

// ═══════════════════════════════════════════════════════════════════════
//  谓词校验与排序
// ═══════════════════════════════════════════════════════════════════════

/// 带 id 感知的 Filter 匹配：将节点的结构 id 注入到 payload 匹配逻辑中
fn matches_filter_with_id<T: VectorType>(
    filter: &Filter,
    node_id: NodeId,
    mt: &MemTable<T>,
) -> bool {
    if filter.field_name() == Some("id") {
        return filter.matches(&serde_json::json!({"id":node_id}));
    }
    match filter {
        // 逻辑组合：递归处理
        Filter::And(filters) => filters
            .iter()
            .all(|f| matches_filter_with_id(f, node_id, mt)),
        Filter::Or(filters) => filters
            .iter()
            .any(|f| matches_filter_with_id(f, node_id, mt)),
        // 其他操作符：回退到标准 payload 匹配
        _ => match mt.get_payload(node_id) {
            Some(payload) => filter.matches(&payload),
            None => false,
        },
    }
}

fn extract_order_key<T>(expr: &TqlExpr, row: &HashMap<String, Node<T>>) -> RuntimeValue {
    match expr {
        TqlExpr::Variable(var) => row
            .get(var)
            .or_else(|| row.get("_"))
            .map_or(RuntimeValue::Null, |node| RuntimeValue::Node(node.id)),
        TqlExpr::Parameter(_)
        | TqlExpr::Similarity { .. }
        | TqlExpr::GraphScore { .. }
        | TqlExpr::GraphMetric { .. }
        | TqlExpr::TextScore { .. }
        | TqlExpr::DiversityScore { .. }
        | TqlExpr::ResidualScore { .. }
        | TqlExpr::Topic { .. }
        | TqlExpr::TopicScore { .. }
        | TqlExpr::Depth { .. }
        | TqlExpr::PathStrength { .. }
        | TqlExpr::PathCount { .. }
        | TqlExpr::Community { .. }
        | TqlExpr::Path { .. }
        | TqlExpr::PathLength { .. }
        | TqlExpr::PathRank { .. }
        | TqlExpr::PairLeft { .. }
        | TqlExpr::PairRight { .. } => RuntimeValue::Null,
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
                    return runtime_u64(node.id);
                }
                return json_to_runtime(&node.payload[field]);
            }
            // FIND/SEARCH 场景下节点绑定到 "_"
            if let Some(node) = row.get("_") {
                if field == "id" {
                    return runtime_u64(node.id);
                }
                return json_to_runtime(&node.payload[field]);
            }
            RuntimeValue::Null
        }
        TqlExpr::Literal(lit) => lit_to_runtime(lit),
    }
}

pub(crate) fn compare_for_sort(a: &RuntimeValue, b: &RuntimeValue) -> std::cmp::Ordering {
    if let (Some(left), Some(right)) = (runtime_number(a), runtime_number(b)) {
        return crate::filter::numeric_ordering(&left, &right);
    }
    match (a, b) {
        (RuntimeValue::Bool(a), RuntimeValue::Bool(b)) => a.cmp(b),
        (RuntimeValue::Path(a), RuntimeValue::Path(b)) => a.cmp(b),
        (RuntimeValue::Json(a), RuntimeValue::Json(b)) => compare_json(a, b),
        (RuntimeValue::Node(a), RuntimeValue::Node(b)) => a.cmp(b),
        (RuntimeValue::Edge(a), RuntimeValue::Edge(b)) => {
            (a.source_id, a.target_id, &a.label).cmp(&(b.source_id, b.target_id, &b.label))
        }
        (RuntimeValue::Str(a), RuntimeValue::Str(b)) => a.cmp(b),
        (RuntimeValue::Null, RuntimeValue::Null) => std::cmp::Ordering::Equal,
        (RuntimeValue::Null, _) => std::cmp::Ordering::Greater, // NULL 排最后
        (_, RuntimeValue::Null) => std::cmp::Ordering::Less,
        _ => runtime_sort_kind(a).cmp(&runtime_sort_kind(b)),
    }
}

fn runtime_sort_kind(value: &RuntimeValue) -> u8 {
    match value {
        RuntimeValue::Int(_) | RuntimeValue::UInt(_) | RuntimeValue::Float(_) => 0,
        RuntimeValue::Str(_) => 1,
        RuntimeValue::Bool(_) => 2,
        RuntimeValue::Json(serde_json::Value::Array(_)) => 3,
        RuntimeValue::Json(_) => 4,
        RuntimeValue::Node(_) => 5,
        RuntimeValue::Edge(_) => 6,
        RuntimeValue::Path(_) => 7,
        RuntimeValue::Null => 8,
    }
}

fn build_node_with_payload<T: VectorType>(
    id: NodeId,
    payload: std::sync::Arc<serde_json::Value>,
    mt: &MemTable<T>,
) -> Option<Node<T>> {
    let vector = mt.get_vector(id)?;
    let edges = mt
        .get_edges(id)
        .map(|edges| edges.to_vec())
        .unwrap_or_default();
    Some(Node {
        id,
        vector: vector.to_vec(),
        payload: (*payload).clone(),
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
        | TqlExpr::GraphMetric { var, .. }
        | TqlExpr::TextScore { var }
        | TqlExpr::DiversityScore { var }
        | TqlExpr::ResidualScore { var }
        | TqlExpr::Topic { var }
        | TqlExpr::TopicScore { var }
        | TqlExpr::Depth { var }
        | TqlExpr::PathStrength { var }
        | TqlExpr::PathCount { var }
        | TqlExpr::Community { var }
        | TqlExpr::Path { var }
        | TqlExpr::PathLength { var }
        | TqlExpr::PathRank { var }
        | TqlExpr::PairLeft { var }
        | TqlExpr::PairRight { var }
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
                | TqlExpr::GraphMetric { var, .. }
                | TqlExpr::TextScore { var }
                | TqlExpr::DiversityScore { var }
                | TqlExpr::ResidualScore { var }
                | TqlExpr::Topic { var }
                | TqlExpr::TopicScore { var }
                | TqlExpr::Depth { var }
                | TqlExpr::PathStrength { var }
                | TqlExpr::PathCount { var }
                | TqlExpr::Community { var }
                | TqlExpr::Path { var }
                | TqlExpr::PathLength { var }
                | TqlExpr::PathRank { var }
                | TqlExpr::PairLeft { var }
                | TqlExpr::PairRight { var }
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

fn aggregate_numbers(
    func: AggFunc,
    values: impl Iterator<Item = serde_json::Number>,
) -> serde_json::Value {
    let values = values.collect::<Vec<_>>();
    if values.is_empty() {
        return serde_json::Value::Null;
    }
    if func == AggFunc::Min || func == AggFunc::Max {
        let selected = values.into_iter().reduce(|left, right| {
            let order = crate::filter::numeric_ordering(&left, &right);
            if (func == AggFunc::Min && order.is_gt()) || (func == AggFunc::Max && order.is_lt()) {
                right
            } else {
                left
            }
        });
        return selected.map_or(serde_json::Value::Null, serde_json::Value::Number);
    }
    if func == AggFunc::Sum
        && values
            .iter()
            .all(|value| value.as_i64().is_some() || value.as_u64().is_some())
    {
        let sum = values.iter().try_fold(0i128, |sum, value| {
            sum.checked_add(
                value
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| value.as_u64().map(i128::from))?,
            )
        });
        return match sum.map(integer_runtime) {
            Some(RuntimeValue::Int(value)) => serde_json::json!(value),
            Some(RuntimeValue::UInt(value)) => serde_json::json!(value),
            _ => serde_json::Value::Null,
        };
    }
    let sum = values
        .iter()
        .filter_map(serde_json::Number::as_f64)
        .sum::<f64>();
    let result = if func == AggFunc::Avg {
        sum / values.len() as f64
    } else {
        sum
    };
    serde_json::Number::from_f64(result).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

// ═══════════════════════════════════════════════════════════════════════
//  EXPLAIN 查询计划生成
// ═══════════════════════════════════════════════════════════════════════

/// 生成查询执行计划（不执行查询），返回单行结果
fn tql_literal_to_json(literal: &TqlLiteral) -> serde_json::Value {
    match literal {
        TqlLiteral::Int(value) => (*value).into(),
        TqlLiteral::UInt(value) => (*value).into(),
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
        } if field != "id" => output.push((field.clone(), tql_literal_to_json(value))),
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
    actual: Option<(usize, f64, Option<PipelineExecutionProfile>)>,
    find_plan: Option<NodeAccessPlan>,
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
        QueryEntry::Text { clause } => (
            format!("TEXT {:?}", clause.kind),
            format!("TOP {}", clause.top_k),
        ),
    };
    plan.insert("entry".into(), serde_json::json!(entry_type));
    plan.insert("detail".into(), serde_json::json!(entry_detail));

    let (access_path, estimated_rows, reversed) = match &query.entry {
        QueryEntry::Find { filter } => {
            let planned = find_plan.unwrap_or_else(|| plan_filter(filter, mt));
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
        QueryEntry::Search { .. } | QueryEntry::Text { .. } => {
            (AccessPath::FullNodeScan, mt.node_count(), false)
        }
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
        AccessPath::NgramPropertyIndex { field } => {
            format!("ngram_property_index ({field})")
        }
        AccessPath::PropertyIndexIntersection { fields } => {
            format!("property_index_intersection ({})", fields.join(", "))
        }
        AccessPath::EdgeLabelIndex { labels } => {
            format!("label_index pushdown (labels: [{}])", labels.join(", "))
        }
        AccessPath::ColdPayloadScan => "cold_payload_scan".to_owned(),
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
            | AccessPath::NgramPropertyIndex { .. }
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
    let ordered_find = matches!(&query.entry, QueryEntry::Find { .. })
        && matches!(access_path, AccessPath::OrderedPropertyIndex { .. })
        && query.order_by.len() == 1;
    if query.limit.is_some()
        && query.pipeline.is_empty()
        && !requires_full_input(query, ordered_find)
    {
        optimizations.push("LIMIT early termination");
    }
    if query.rank.is_some() {
        optimizations.push("GraphFirst exact anchor ranking");
    }
    plan.insert("optimizations".into(), serde_json::json!(optimizations));

    // 统计信息
    plan.insert("total_nodes".into(), serde_json::json!(mt.node_count()));
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
    if let Some((actual_rows, elapsed_ms, pipeline_profile)) = actual {
        plan.insert("analyze".into(), serde_json::json!(true));
        plan.insert("actual_rows".into(), serde_json::json!(actual_rows));
        plan.insert("elapsed_ms".into(), serde_json::json!(elapsed_ms));
        if let Some(profile) = pipeline_profile {
            plan.insert(
                "planning_profile".into(),
                serde_json::to_value(&profile.plan.planning_profile)
                    .unwrap_or(serde_json::Value::Null),
            );
            plan.insert(
                "estimated_memo_bytes".into(),
                serde_json::json!(profile.plan.estimated_memo_bytes),
            );
            let stages = profile
                .metrics
                .iter()
                .enumerate()
                .map(|(index, metrics)| {
                    let planned = profile.plan.stages.get(index);
                    let estimated_rows = planned.map_or(0, |stage| stage.estimated_rows);
                    let error_ratio = if estimated_rows == 0 {
                        None
                    } else {
                        Some(metrics.output_rows as f64 / estimated_rows as f64)
                    };
                    serde_json::json!({
                        "operator": metrics.operator,
                        "input_rows": metrics.input_rows,
                        "actual_rows": metrics.output_rows,
                        "estimated_rows": estimated_rows,
                        "estimate_error_ratio": error_ratio,
                        "elapsed_ns": metrics.elapsed_ns,
                        "node_set_bytes": metrics.node_set_bytes,
                        "vector_read_bytes": metrics.vector_read_bytes,
                        "estimate_source": planned.map(|stage| stage.estimate_source),
                        "estimate_confidence": planned.map(|stage| stage.estimate_confidence),
                    })
                })
                .collect::<Vec<_>>();
            plan.insert("pipeline_stage_actuals".into(), serde_json::json!(stages));
        }
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
        TqlExpr::GraphMetric { var, metric } => format!("{metric}({var})"),
        TqlExpr::TextScore { var } => format!("text_score({var})"),
        TqlExpr::DiversityScore { var } => format!("diversity_score({var})"),
        TqlExpr::ResidualScore { var } => format!("residual_score({var})"),
        TqlExpr::Topic { var } => format!("topic({var})"),
        TqlExpr::TopicScore { var } => format!("topic_score({var})"),
        TqlExpr::Depth { var } => format!("depth({var})"),
        TqlExpr::PathStrength { var } => format!("path_strength({var})"),
        TqlExpr::PathCount { var } => format!("path_count({var})"),
        TqlExpr::Community { var } => format!("community({var})"),
        TqlExpr::Path { var } => format!("path({var})"),
        TqlExpr::PathLength { var } => format!("path_length({var})"),
        TqlExpr::PathRank { var } => format!("path_rank({var})"),
        TqlExpr::PairLeft { var } => format!("pair_left({var})"),
        TqlExpr::PairRight { var } => format!("pair_right({var})"),
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
        TqlExpr::Literal(value) => tql_literal_to_json(value).to_string(),
    }
}

pub(crate) fn return_column_name(item: &ReturnExpr) -> String {
    item.alias
        .clone()
        .unwrap_or_else(|| format_return_expr_kind(&item.kind))
}

pub(crate) fn validate_return_columns(returns: &ReturnClause) -> Result<(), TriviumError> {
    let names = match returns {
        ReturnClause::All => return Ok(()),
        ReturnClause::Variables(vars) => vars.clone(),
        ReturnClause::Expressions(items) => items.iter().map(return_column_name).collect(),
    };
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name.clone()) {
            return Err(TriviumError::QueryExecution(format!(
                "返回列名重复：{name}；请使用不同的 AS 别名 (Duplicate return column {name}; use distinct AS aliases)"
            )));
        }
    }
    Ok(())
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
    execute_tql_mutation_with_limits(mutation, mt, TqlLimits::default())
}

pub fn execute_tql_mutation_with_limits<T: VectorType>(
    mutation: &TqlMutation,
    mt: &MemTable<T>,
    mut limits: TqlLimits,
) -> Result<Vec<MutationOp<T>>, TriviumError> {
    // 写操作的匹配集超限时整批拒绝，禁止使用读查询的部分结果策略。
    limits.row_overflow = crate::database::RowOverflowPolicy::Throw;
    limits.max_query_rows = Some(0);
    let memory = QueryMemory::new(limits.memory_limit, mt);
    limits.check_cancelled()?;
    match &mutation.action {
        MutationAction::Create(create) => {
            execute_create(create, &mutation.source, mt, &limits, &memory)
        }
        MutationAction::Set(assignments) => {
            execute_set(assignments, &mutation.source, mt, &limits, &memory)
        }
        MutationAction::SetVector { var, vector } => {
            execute_set_vector(var, vector, &mutation.source, mt, &limits, &memory)
        }
        MutationAction::Delete { vars, detach } => {
            execute_delete(vars, *detach, &mutation.source, mt, &limits, &memory)
        }
    }
}

/// CREATE 指令生成
fn execute_create<T: VectorType>(
    create: &CreateAction,
    source: &Option<MutationSource>,
    mt: &MemTable<T>,
    limits: &TqlLimits,
    memory: &QueryMemory,
) -> Result<Vec<MutationOp<T>>, TriviumError> {
    let matches = if let Some(source) = source {
        let query = build_match_query(&source.pattern, source.predicate.as_ref());
        execute_tql_with_limits(&query, mt, limits.clone())?
    } else {
        vec![HashMap::new()]
    };
    retain_mutation_rows(&matches, mt, memory)?;
    let mut ops = Vec::new();
    for (row_index, matched) in matches.iter().enumerate() {
        // 每条匹配拥有独立的新建变量作用域，边端点始终引用该行的新节点。
        let mut created = HashMap::new();
        for (node_index, node) in create.nodes.iter().enumerate() {
            if node
                .var
                .as_ref()
                .is_some_and(|var| matched.contains_key(var))
            {
                continue;
            }
            let scoped = format!("__create_{row_index}_{node_index}");
            if let Some(var) = &node.var {
                created.insert(var.clone(), scoped.clone());
            }
            memory.retain(
                crate::storage::payload_store::query_clone_peak(&node.payload)
                    .saturating_add(mt.dim().saturating_mul(std::mem::size_of::<T>()))
                    .saturating_add(512),
            )?;
            ops.push(MutationOp::InsertNode {
                var: scoped,
                vector: vec![T::default(); mt.dim()],
                payload: node.payload.clone(),
            });
        }
        for edge in &create.edges {
            let endpoint = |var: &str| -> Result<(NodeId, String), TriviumError> {
                if let Some(node) = matched.get(var) {
                    Ok((node.id, var.to_owned()))
                } else if let Some(scoped) = created.get(var) {
                    Ok((0, scoped.clone()))
                } else {
                    Err(TriviumError::QueryExecution(format!(
                        "CREATE 引用了未定义变量 {var} (CREATE references undefined variable {var})"
                    )))
                }
            };
            let (src_id, src_var) = endpoint(&edge.src_var)?;
            let (dst_id, dst_var) = endpoint(&edge.dst_var)?;
            ops.push(MutationOp::LinkEdge {
                src_id,
                dst_id,
                src_var,
                dst_var,
                label: edge.label.clone(),
                weight: edge.weight,
            });
        }
    }

    Ok(ops)
}

fn execute_set_vector<T: VectorType>(
    var: &str,
    vector: &[serde_json::Number],
    source: &Option<MutationSource>,
    mt: &MemTable<T>,
    limits: &TqlLimits,
    memory: &QueryMemory,
) -> Result<Vec<MutationOp<T>>, TriviumError> {
    if vector.len() != mt.dim() {
        return Err(TriviumError::DimensionMismatch {
            expected: mt.dim(),
            got: vector.len(),
        });
    }
    let source = source.as_ref().ok_or_else(|| {
        TriviumError::QueryParse("SET VECTOR requires a preceding MATCH clause".into())
    })?;
    let query = build_match_query(&source.pattern, source.predicate.as_ref());
    let results = execute_tql_with_limits(&query, mt, limits.clone())?;
    retain_mutation_rows(&results, mt, memory)?;
    let converted = vector
        .iter()
        .map(T::from_json_number)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results
        .iter()
        .filter_map(|row| row.get(var))
        .map(|node| MutationOp::UpdateVector {
            id: node.id,
            vector: converted.clone(),
        })
        .collect())
}

/// SET 指令生成
fn execute_set<T: VectorType>(
    assignments: &[SetAssignment],
    source: &Option<MutationSource>,
    mt: &MemTable<T>,
    limits: &TqlLimits,
    memory: &QueryMemory,
) -> Result<Vec<MutationOp<T>>, TriviumError> {
    let source = source
        .as_ref()
        .ok_or_else(|| TriviumError::QueryParse("SET requires a preceding MATCH clause".into()))?;

    // DML 匹配集不能因查询行上限发生静默的部分写入。
    let query = build_match_query(&source.pattern, source.predicate.as_ref());
    let results = execute_tql_with_limits(&query, mt, limits.clone())?;
    retain_mutation_rows(&results, mt, memory)?;

    // 同一节点的多字段及重复字段赋值按文本顺序合并，只生成一次替换。
    let mut payloads = BTreeMap::new();
    for row in &results {
        for assign in assignments {
            let node = row.get(&assign.var).ok_or_else(|| {
                TriviumError::QueryExecution(format!(
                    "SET 引用了未绑定变量 {} (SET references unbound variable {})",
                    assign.var, assign.var
                ))
            })?;
            let payload = payloads
                .entry(node.id)
                .or_insert_with(|| node.payload.clone());
            memory.retain(
                crate::storage::payload_store::query_clone_peak(&assign.value)
                    .saturating_add(assign.field.len())
                    .saturating_add(256),
            )?;
            let object = payload.as_object_mut().ok_or_else(|| {
                TriviumError::InvalidInput(
                    "SET 字段要求对象 Payload (SET fields requires an object Payload)".into(),
                )
            })?;
            object.insert(assign.field.clone(), assign.value.clone());
        }
    }
    let ops = payloads
        .into_iter()
        .map(|(id, payload)| MutationOp::UpdatePayload { id, payload })
        .collect();

    Ok(ops)
}

/// DELETE / DETACH DELETE 指令生成
fn execute_delete<T: VectorType>(
    vars: &[String],
    detach: bool,
    source: &Option<MutationSource>,
    mt: &MemTable<T>,
    limits: &TqlLimits,
    memory: &QueryMemory,
) -> Result<Vec<MutationOp<T>>, TriviumError> {
    let source = source.as_ref().ok_or_else(|| {
        TriviumError::QueryParse("DELETE requires a preceding MATCH clause".into())
    })?;

    let query = build_match_query(&source.pattern, source.predicate.as_ref());
    let results = execute_tql_with_limits(&query, mt, limits.clone())?;
    retain_mutation_rows(&results, mt, memory)?;

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

fn retain_mutation_rows<T: VectorType>(
    rows: &TqlResult<T>,
    mt: &MemTable<T>,
    memory: &QueryMemory,
) -> Result<(), TriviumError> {
    let bytes = rows
        .iter()
        .flat_map(|row| row.values())
        .try_fold(0usize, |sum, node| {
            sum.checked_add(
                memory::node_bytes(node.id, mt)?
                    .checked_mul(3)
                    .ok_or_else(memory::overflow)?,
            )
            .ok_or_else(memory::overflow)
        })?;
    memory.retain(bytes)
}
