//! TQL 统一查询执行器
//!
//! 将 TqlQuery AST 在 MemTable 上执行，支持三种入口：
//! - MATCH: 图模式匹配（含可变长路径、多标签边）
//! - FIND: 文档过滤扫描
//! - SEARCH: 向量检索 + 图扩散（桥接到现有管线）
//!
//! DML 写操作通过 execute_tql_mutation() 生成 MutationOp 指令，
//! 由 Database 层统一执行并写入 WAL。

use super::tql_ast::*;
use crate::VectorType;
use crate::error::TriviumError;
use crate::filter::Filter;
use crate::node::{Node, NodeId};
use crate::storage::memtable::MemTable;
use std::collections::{BTreeMap, HashMap, HashSet};

/// TQL 查询结果：每行是一组变量绑定 → 节点快照
pub type TqlResult<T> = Vec<HashMap<String, Node<T>>>;

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

/// 执行器配额配置
const MAX_BUDGET: usize = 100_000;
const DEFAULT_ROW_LIMIT: usize = 5_000;

// ═══════════════════════════════════════════════════════════════════════
//  公开入口
// ═══════════════════════════════════════════════════════════════════════

/// 执行一个已解析的 TqlQuery
pub fn execute_tql<T: VectorType>(
    query: &TqlQuery,
    memtable: &MemTable<T>,
) -> Result<TqlResult<T>, TriviumError> {
    // EXPLAIN 模式：输出查询计划而不执行
    if query.explain {
        return Ok(generate_explain_plan(query, memtable));
    }

    let requires_full_input = query.rank.is_some()
        || !query.order_by.is_empty()
        || matches!(&query.returns, ReturnClause::Expressions(exprs) if exprs.iter().any(|expr| is_aggregate(&expr.kind) || expr.distinct));
    let row_limit = if requires_full_input {
        DEFAULT_ROW_LIMIT
    } else {
        query
            .limit
            .and_then(|limit| limit.checked_add(query.offset.unwrap_or(0)))
            .unwrap_or(DEFAULT_ROW_LIMIT)
            .min(DEFAULT_ROW_LIMIT)
    };

    let execution_row_limit = if query.rank.is_some() {
        MAX_BUDGET.saturating_add(1)
    } else {
        row_limit
    };
    let mut results = match &query.entry {
        QueryEntry::Find { filter } => execute_find(filter, query, memtable, execution_row_limit)?,
        QueryEntry::Match { pattern } => {
            execute_match(pattern, query, memtable, execution_row_limit, false)?
        }
        QueryEntry::OptionalMatch { pattern } => {
            execute_match(pattern, query, memtable, execution_row_limit, true)?
        }
        QueryEntry::Search {
            vector,
            top_k,
            expand,
        } => execute_search(
            vector,
            *top_k,
            expand.as_ref(),
            query,
            memtable,
            execution_row_limit,
        )?,
    };

    if let Some(rank) = &query.rank {
        results = apply_graph_first_rank(results, rank, memtable)?;
    }

    results = apply_aggregation_and_distinct(
        &query.returns,
        results,
        matches!(&query.entry, QueryEntry::Find { .. }),
    )?;

    if !query.order_by.is_empty() {
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
) -> Result<TqlResult<T>, TriviumError> {
    let mut results = Vec::new();
    for id in mt.all_node_ids() {
        if results.len() >= row_limit {
            break;
        }

        let payload = match mt.get_payload(id) {
            Some(p) => p,
            None => continue,
        };

        // FIND 入口过滤
        if !filter.matches(payload) {
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

    Ok(results)
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

    // 确定起始候选集（含标签索引下推优化）
    let start_candidates = if optional {
        find_tql_candidates(&pattern.nodes[0], mt)
    } else {
        find_tql_candidates_optimized(&pattern.nodes[0], pattern.edges.first(), mt)
    };

    let mut results = Vec::new();
    let mut budget: usize = 0;

    for start_id in start_candidates {
        let result_start = results.len();
        let mut env = vec![None; var_map.len()];
        let cont = tql_dfs(
            mt,
            pattern,
            query.predicate.as_ref(),
            &return_vars,
            &var_map,
            0, // layer_idx
            &mut env,
            start_id,
            &mut results,
            &mut budget,
            MAX_BUDGET,
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
    // 向量相似度搜索：对全量节点计算相似度并取 top_k
    let query_vec: Vec<T> = vector.iter().map(|v| T::from_f32(*v as f32)).collect();
    let dim = query_vec.len();

    let mut scored: Vec<(NodeId, f32)> = mt
        .all_node_ids()
        .iter()
        .filter_map(|&id| {
            let vec = mt.get_vector(id)?;
            if vec.len() != dim {
                return None;
            }
            let score = T::similarity(&query_vec, vec);
            Some((id, score))
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);

    let mut candidates: Vec<NodeId> = scored.iter().map(|s| s.0).collect();

    // EXPAND: 确定性 Reachability
    if let Some(ex) = expand {
        let mut expanded = BTreeMap::<NodeId, usize>::new();
        for &seed in &candidates {
            expanded.insert(seed, 0);
        }
        for &seed in &candidates {
            let direction = match ex.direction {
                EdgeDirection::Forward => {
                    crate::graph::reachability::ReachabilityDirection::Outgoing
                }
                EdgeDirection::Backward => {
                    crate::graph::reachability::ReachabilityDirection::Incoming
                }
                EdgeDirection::Both => crate::graph::reachability::ReachabilityDirection::Both,
            };
            let config = crate::graph::reachability::ReachabilityConfig {
                min_depth: ex.min_depth,
                max_depth: ex.max_depth,
                labels: (!ex.labels.is_empty()).then(|| ex.labels.clone()),
                direction,
                max_visited_nodes: MAX_BUDGET,
                max_results: MAX_BUDGET,
                max_edges: MAX_BUDGET,
            };
            for reached in crate::graph::reachability::traverse(mt, seed, &config)? {
                expanded
                    .entry(reached.target_id)
                    .and_modify(|depth| *depth = (*depth).min(reached.depth))
                    .or_insert(reached.depth);
                if expanded.len() > MAX_BUDGET {
                    return Err(TriviumError::QueryExecution(format!(
                        "SEARCH EXPAND 超过候选预算 {MAX_BUDGET}"
                    )));
                }
            }
        }
        candidates = expanded.into_keys().collect();
    }

    // 对候选集应用 WHERE 过滤
    let mut results = Vec::new();
    for id in candidates {
        if results.len() >= row_limit {
            break;
        }

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
    Null,
}

fn eval_tql_expr<T: VectorType>(
    expr: &TqlExpr,
    env: &[Option<u64>],
    var_map: &HashMap<String, usize>,
    mt: &MemTable<T>,
) -> RuntimeValue {
    match expr {
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
//  候选集发现 & 排序
// ═══════════════════════════════════════════════════════════════════════

/// TQL 版本的候选节点发现
fn find_tql_candidates<T: VectorType>(node_pat: &TqlNodePattern, mt: &MemTable<T>) -> Vec<NodeId> {
    // 🏆 O(1) 主键索引短路：检测 Filter 是否包含 id = N 的精确匹配
    if let Some(filter) = &node_pat.filter {
        if let Some(target_id) = extract_id_from_filter(filter) {
            if mt.contains(target_id) && matches_filter_with_id(filter, target_id, mt) {
                return vec![target_id];
            }
            return vec![];
        }

        // 🏆 属性二级索引短路：检测 Filter 是否包含已索引字段的等值匹配
        if let Some(indexed_ids) = try_property_index_lookup(filter, mt) {
            // 索引命中，但仍需过滤其他条件
            let mut candidates = Vec::new();
            for id in indexed_ids {
                if matches_filter_with_id(filter, id, mt) {
                    candidates.push(id);
                }
            }
            return candidates;
        }
    }

    let all_ids = mt.all_node_ids();
    let mut candidates = Vec::new();

    for id in all_ids {
        // Filter 精确校验
        if let Some(filter) = &node_pat.filter
            && !matches_filter_with_id(filter, id, mt)
        {
            continue;
        }

        candidates.push(id);
    }

    candidates
}

/// 尝试从 Filter 中找到可以使用属性索引的等值条件
/// 返回 Some(ids) 表示索引命中，None 表示无法使用索引
fn try_property_index_lookup<T: VectorType>(
    filter: &Filter,
    mt: &MemTable<T>,
) -> Option<Vec<NodeId>> {
    match filter {
        Filter::Eq(key, val) if key != "id" => {
            mt.find_by_property_index(key, val).map(|ids| ids.to_vec())
        }
        Filter::And(filters) => {
            let mut indexed_sets = Vec::new();
            for f in filters {
                if let Filter::Eq(key, val) = f
                    && key != "id"
                    && let Some(ids) = mt.find_by_property_index(key, val)
                {
                    indexed_sets.push(ids);
                }
            }
            indexed_sets.sort_by_key(|ids| ids.len());
            let first = indexed_sets.first()?;
            let mut intersection: Vec<NodeId> = first.to_vec();
            for ids in indexed_sets.iter().skip(1) {
                let set: HashSet<NodeId> = ids.iter().copied().collect();
                intersection.retain(|id| set.contains(id));
            }
            Some(intersection)
        }
        _ => None,
    }
}

/// 优化版起始候选集生成
///
/// 优化规则：标签索引下推
/// 当起始节点无 Filter 约束、但第一条边有标签约束时，
/// 使用 `mt.get_edges_by_label()` 获取候选 src 集合，避免 O(N) 全扫描。
fn find_tql_candidates_optimized<T: VectorType>(
    node_pat: &TqlNodePattern,
    first_edge: Option<&TqlEdgePattern>,
    mt: &MemTable<T>,
) -> Vec<NodeId> {
    // 如果起始节点有 Filter 约束，走原始路径（已有 ID 短路优化）
    if node_pat.filter.is_some() {
        return find_tql_candidates(node_pat, mt);
    }

    // 标签索引下推：起始节点无约束 + 第一条边有单标签
    if let Some(edge_pat) = first_edge
        && !edge_pat.labels.is_empty()
    {
        let mut src_set: HashSet<NodeId> = HashSet::new();
        for label in &edge_pat.labels {
            for &(src, _dst) in mt.get_edges_by_label(label) {
                src_set.insert(src);
            }
        }
        let mut candidates: Vec<NodeId> = src_set.into_iter().collect();
        candidates.sort_unstable(); // 稳定输出顺序
        return candidates;
    }

    // 兜底：全表扫描
    find_tql_candidates(node_pat, mt)
}

/// 从 Filter 中提取 id 等值匹配的目标 ID（用于 O(1) 短路）
fn extract_id_from_filter(filter: &Filter) -> Option<NodeId> {
    match filter {
        Filter::Eq(key, val) if key == "id" => val.as_i64().map(|n| n as NodeId),
        Filter::And(filters) => {
            for f in filters {
                if let Some(id) = extract_id_from_filter(f) {
                    return Some(id);
                }
            }
            None
        }
        _ => None,
    }
}

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
        ReturnExprKind::Aggregate(_, inner) => collect_vars_from_kind(inner, out),
    }
}

fn first_var_from_kind(kind: &ReturnExprKind) -> Option<String> {
    match kind {
        ReturnExprKind::Var(v) => Some(v.clone()),
        ReturnExprKind::Property(v, _) => Some(v.clone()),
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
        ReturnExprKind::Aggregate(_, _) => serde_json::Value::Null,
    }
}

/// 聚合计算
///
/// 非聚合列作为分组键，聚合列按组计算。
/// 结果中聚合值写入节点的 payload 字段中（以 alias 或生成名为 key）。
fn apply_aggregation<T: VectorType>(results: TqlResult<T>, exprs: &[ReturnExpr]) -> TqlResult<T> {
    if results.is_empty() {
        return Vec::new();
    }

    // 分离分组列和聚合列
    let group_exprs: Vec<&ReturnExpr> = exprs.iter().filter(|e| !is_aggregate(&e.kind)).collect();
    let agg_exprs: Vec<&ReturnExpr> = exprs.iter().filter(|e| is_aggregate(&e.kind)).collect();

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
            let count = rows
                .iter()
                .filter(|r| resolve_inner(inner, r).is_some())
                .count();
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
fn generate_explain_plan<T: VectorType>(query: &TqlQuery, mt: &MemTable<T>) -> TqlResult<T> {
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

    // 候选集策略
    let strategy = match &query.entry {
        QueryEntry::Find { .. } => "full_scan".to_string(),
        QueryEntry::Match { pattern } | QueryEntry::OptionalMatch { pattern } => {
            analyze_candidate_strategy(&pattern.nodes[0], pattern.edges.first(), mt)
        }
        QueryEntry::Search { .. } => "vector_index".to_string(),
    };
    plan.insert("candidate_strategy".into(), serde_json::json!(strategy));

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
    if strategy.contains("id_shortcut") {
        optimizations.push("ID O(1) shortcut");
    }
    if strategy.contains("label_index") {
        optimizations.push("label index pushdown");
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

/// 分析候选集选取策略
fn analyze_candidate_strategy<T: VectorType>(
    node_pat: &TqlNodePattern,
    first_edge: Option<&TqlEdgePattern>,
    _mt: &MemTable<T>,
) -> String {
    if let Some(filter) = &node_pat.filter {
        if extract_id_from_filter(filter).is_some() {
            return "id_shortcut O(1)".to_string();
        }
        return "filter_scan (with inline filter)".to_string();
    }
    if let Some(edge_pat) = first_edge
        && !edge_pat.labels.is_empty()
    {
        return format!(
            "label_index pushdown (labels: [{}])",
            edge_pat.labels.join(", ")
        );
    }
    "full_scan O(N)".to_string()
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

fn format_return_expr_kind(kind: &ReturnExprKind) -> String {
    match kind {
        ReturnExprKind::Var(v) => v.clone(),
        ReturnExprKind::Property(v, f) => format!("{}.{}", v, f),
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

    // 运行 MATCH 查询获取匹配行
    let query = build_match_query(&source.pattern, source.predicate.as_ref());
    let results = execute_tql(&query, mt)?;

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
    let results = execute_tql(&query, mt)?;

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
        entry: QueryEntry::Match {
            pattern: pattern.clone(),
        },
        predicate: predicate.cloned(),
        rank: None,
        returns: ReturnClause::All,
        order_by: Vec::new(),
        limit: None,
        offset: None,
    }
}
