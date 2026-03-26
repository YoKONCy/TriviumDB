/// 查询执行器：将 AST 在 MemTable 上执行，返回匹配结果。
///
/// 核心算法：
/// 1. 从第一个 NodePattern 确定候选起点集合
/// 2. 沿 EdgePattern 链逐层遍历邻接表
/// 3. 每一层的节点都绑定到对应的变量名
/// 4. 最终对所有"完整路径"应用 WHERE 过滤
/// 5. 提取 RETURN 变量对应的节点返回

use super::ast::*;
use crate::node::Node;
use crate::storage::memtable::MemTable;
use crate::VectorType;
use std::collections::HashMap;

/// 单条查询结果：变量名 → 节点快照
pub type QueryResult<T> = Vec<HashMap<String, Node<T>>>;

/// 执行一个已解析的 Query，返回匹配到的所有变量绑定
pub fn execute<T: VectorType>(query: &Query, memtable: &MemTable<T>) -> QueryResult<T> {
    let pattern = &query.pattern;

    // 步骤 1：确定起始候选节点
    let first_node_pat = &pattern.nodes[0];
    let start_candidates = find_candidates(first_node_pat, memtable);

    // 步骤 2：从起点出发，逐层匹配 edge → node → edge → node ...
    let mut bindings_set: Vec<HashMap<String, Node<T>>> = Vec::new();

    for start_node in start_candidates {
        let mut binding: HashMap<String, Node<T>> = HashMap::new();
        if let Some(var) = &first_node_pat.var {
            binding.insert(var.clone(), start_node.clone());
        }
        bindings_set.push(binding);
    }

    // 逐层扩展
    for i in 0..pattern.edges.len() {
        let edge_pat = &pattern.edges[i];
        let next_node_pat = &pattern.nodes[i + 1];
        let mut next_bindings = Vec::new();

        for binding in &bindings_set {
            // 找到当前层最后一个有名字的节点
            let current_node_pat = &pattern.nodes[i];
            let current_node = if let Some(var) = &current_node_pat.var {
                binding.get(var)
            } else {
                None
            };

            let current_id = match current_node {
                Some(n) => n.id,
                None => continue,
            };

            // 沿边扩展
            if let Some(edges) = memtable.get_edges(current_id) {
                for edge in edges {
                    // 边标签过滤
                    if let Some(ref label) = edge_pat.label {
                        if &edge.label != label {
                            continue;
                        }
                    }

                    // 获取目标节点
                    let target_id = edge.target_id;
                    let target_node = match build_node(target_id, memtable) {
                        Some(n) => n,
                        None => continue,
                    };

                    // 节点属性内联过滤
                    if !matches_node_props(&target_node, next_node_pat) {
                        continue;
                    }

                    let mut new_binding = binding.clone();
                    if let Some(var) = &next_node_pat.var {
                        new_binding.insert(var.clone(), target_node);
                    }
                    next_bindings.push(new_binding);
                }
            }
        }

        bindings_set = next_bindings;
    }

    // 步骤 3：应用 WHERE 过滤
    if let Some(ref condition) = query.where_clause {
        bindings_set.retain(|binding| eval_condition(condition, binding));
    }

    // 步骤 4：仅保留 RETURN 中请求的变量
    let return_vars = &query.return_vars;
    bindings_set.iter().map(|binding: &HashMap<String, Node<T>>| {
        let mut filtered: HashMap<String, Node<T>> = HashMap::new();
        for var in return_vars {
            if let Some(node) = binding.get(var) {
                filtered.insert(var.clone(), node.clone());
            }
        }
        filtered
    }).collect()
}

/// 查找匹配节点模式的候选节点
fn find_candidates<T: VectorType>(node_pat: &NodePattern, memtable: &MemTable<T>) -> Vec<Node<T>> {
    let all_ids = memtable.all_node_ids();
    let mut candidates = Vec::new();

    for id in all_ids {
        if let Some(node) = build_node(id, memtable) {
            // 检查内联属性过滤
            if matches_node_props(&node, node_pat) {
                candidates.push(node);
            }
        }
    }

    candidates
}

/// 从 MemTable 构建完整 Node
fn build_node<T: VectorType>(id: u64, memtable: &MemTable<T>) -> Option<Node<T>> {
    let vector = memtable.get_vector(id)?;
    let payload = memtable.get_payload(id)?;
    let edges = memtable.get_edges(id).map(|e| e.to_vec()).unwrap_or_default();
    Some(Node {
        id,
        vector: vector.to_vec(),
        payload: payload.clone(),
        edges,
    })
}

/// 检查节点是否匹配内联属性过滤 {id: 42, name: "Alice"}
fn matches_node_props<T: VectorType>(node: &Node<T>, pat: &NodePattern) -> bool {
    for prop in &pat.props {
        match prop.key.as_str() {
            "id" => {
                // 特殊处理 id 字段
                if let LitValue::Int(target_id) = &prop.value {
                    if node.id != *target_id as u64 {
                        return false;
                    }
                }
            }
            field => {
                // 从 Payload JSON 中取值比较
                let json_val = &node.payload[field];
                if !lit_matches_json(&prop.value, json_val) {
                    return false;
                }
            }
        }
    }
    true
}

/// 字面量值与 JSON 值比较
fn lit_matches_json(lit: &LitValue, json: &serde_json::Value) -> bool {
    match lit {
        LitValue::Int(n) => json.as_i64() == Some(*n),
        LitValue::Float(f) => json.as_f64() == Some(*f),
        LitValue::Str(s) => json.as_str() == Some(s),
        LitValue::Bool(b) => json.as_bool() == Some(*b),
    }
}

/// 评估 WHERE 条件
fn eval_condition<T: VectorType>(cond: &Condition, binding: &HashMap<String, Node<T>>) -> bool {
    match cond {
        Condition::Compare { left, op, right } => {
            let lval = eval_expr(left, binding);
            let rval = eval_expr(right, binding);
            compare_values(&lval, op, &rval)
        }
        Condition::And(a, b) => {
            eval_condition(a, binding) && eval_condition(b, binding)
        }
        Condition::Or(a, b) => {
            eval_condition(a, binding) || eval_condition(b, binding)
        }
    }
}

/// 评估表达式 → 运行时值
fn eval_expr<T: VectorType>(expr: &Expr, binding: &HashMap<String, Node<T>>) -> RuntimeValue {
    match expr {
        Expr::Property { var, field } => {
            if let Some(node) = binding.get(var) {
                if field == "id" {
                    return RuntimeValue::Int(node.id as i64);
                }
                json_to_runtime(&node.payload[field])
            } else {
                RuntimeValue::Null
            }
        }
        Expr::Literal(lit) => lit_to_runtime(lit),
    }
}

#[derive(Debug, Clone)]
enum RuntimeValue {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
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

fn lit_to_runtime(lit: &LitValue) -> RuntimeValue {
    match lit {
        LitValue::Int(n) => RuntimeValue::Int(*n),
        LitValue::Float(f) => RuntimeValue::Float(*f),
        LitValue::Str(s) => RuntimeValue::Str(s.clone()),
        LitValue::Bool(b) => RuntimeValue::Bool(*b),
    }
}

fn compare_values(lhs: &RuntimeValue, op: &CompOp, rhs: &RuntimeValue) -> bool {
    match (lhs, rhs) {
        (RuntimeValue::Int(a), RuntimeValue::Int(b)) => cmp_ord(a, op, b),
        (RuntimeValue::Float(a), RuntimeValue::Float(b)) => cmp_f64(*a, op, *b),
        (RuntimeValue::Int(a), RuntimeValue::Float(b)) => cmp_f64(*a as f64, op, *b),
        (RuntimeValue::Float(a), RuntimeValue::Int(b)) => cmp_f64(*a, op, *b as f64),
        (RuntimeValue::Str(a), RuntimeValue::Str(b)) => cmp_ord(a, op, b),
        (RuntimeValue::Bool(a), RuntimeValue::Bool(b)) => {
            match op {
                CompOp::Eq => a == b,
                CompOp::Ne => a != b,
                _ => false,
            }
        }
        _ => false,
    }
}

fn cmp_ord<T: Ord>(a: &T, op: &CompOp, b: &T) -> bool {
    match op {
        CompOp::Eq => a == b,
        CompOp::Ne => a != b,
        CompOp::Gt => a > b,
        CompOp::Gte => a >= b,
        CompOp::Lt => a < b,
        CompOp::Lte => a <= b,
    }
}

fn cmp_f64(a: f64, op: &CompOp, b: f64) -> bool {
    match op {
        CompOp::Eq => (a - b).abs() < f64::EPSILON,
        CompOp::Ne => (a - b).abs() >= f64::EPSILON,
        CompOp::Gt => a > b,
        CompOp::Gte => a >= b,
        CompOp::Lt => a < b,
        CompOp::Lte => a <= b,
    }
}
