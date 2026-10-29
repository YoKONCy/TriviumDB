//! 所有查询入口共用的表达式、分组、去重、排序与分页。
//! 节点绑定和返回列分别保存，整节点仅在最终输出时物化。

use super::*;

pub(super) enum Input<'a, T> {
    Nodes(
        &'a HashMap<String, Node<T>>,
        Option<(&'a str, &'a EdgeView)>,
    ),
    Pipeline(&'a crate::query::pipeline::NodeRow),
}

pub(super) struct Scope<'a, T: VectorType> {
    pub memory: &'a QueryMemory,
    pub query: &'a TqlQuery,
    pub mt: &'a MemTable<T>,
    pub limits: &'a TqlLimits,
    pub pipeline: Option<(&'a str, &'a std::collections::BTreeSet<String>)>,
}

pub(super) struct Row<T> {
    pub source: Option<usize>,
    pub values: HashMap<String, TqlValue<T>>,
}

use super::return_column_name as column_name;

fn runtime_json(value: RuntimeValue) -> serde_json::Value {
    match value {
        RuntimeValue::Int(v) => v.into(),
        RuntimeValue::UInt(v) => v.into(),
        RuntimeValue::Float(v) => serde_json::json!(v),
        RuntimeValue::Str(v) => v.into(),
        RuntimeValue::Bool(v) => v.into(),
        RuntimeValue::Path(v) => serde_json::json!(v),
        RuntimeValue::Json(v) => v,
        RuntimeValue::Node(id) => serde_json::json!(id),
        RuntimeValue::Edge(edge) => edge_json(&edge),
        RuntimeValue::Null => serde_json::Value::Null,
    }
}

fn value_json<T>(value: &TqlValue<T>) -> serde_json::Value {
    match value {
        TqlValue::Node(node) => serde_json::json!(node.id),
        TqlValue::Edge(edge) => {
            serde_json::json!({"source":edge.source_id,"target":edge.target_id,"label":edge.label})
        }
        TqlValue::Int(v) => (*v).into(),
        TqlValue::UInt(v) => (*v).into(),
        TqlValue::Float(v) => serde_json::json!(v),
        TqlValue::String(v) => v.clone().into(),
        TqlValue::Bool(v) => (*v).into(),
        TqlValue::Path(v) => serde_json::json!(v),
        TqlValue::List(v) => serde_json::Value::Array(v.clone()),
        TqlValue::Object(v) => serde_json::Value::Object(v.clone()),
        TqlValue::Null => serde_json::Value::Null,
    }
}

/// 整数与相等的整值浮点共用签名；元组逐项编码以保留字符串边界。
fn canonical_json(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Number(ref n) if n.is_f64() => {
            let f = n.as_f64().unwrap_or(f64::NAN);
            if f.fract() == 0.0 && f >= i64::MIN as f64 && f < 18_446_744_073_709_551_616.0 {
                if f < 0.0 {
                    serde_json::json!(f as i64)
                } else {
                    serde_json::json!(f as u64)
                }
            } else {
                value
            }
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_json).collect()),
        Value::Object(values) => {
            let sorted = values.into_iter().collect::<BTreeMap<_, _>>();
            Value::Object(
                sorted
                    .into_iter()
                    .map(|(k, v)| (k, canonical_json(v)))
                    .collect(),
            )
        }
        _ => value,
    }
}

fn signature(value: serde_json::Value) -> Result<String, TriviumError> {
    serde_json::to_string(&canonical_json(value)).map_err(|error| {
        TriviumError::QueryExecution(format!(
            "返回值无法编码 (Cannot encode return value): {error}"
        ))
    })
}

fn expression<T: VectorType>(
    expr: &TqlExpr,
    input: &Input<'_, T>,
    scope: &Scope<'_, T>,
) -> RuntimeValue {
    if let Input::Pipeline(row) = input {
        return eval_pipeline_row_expr(expr, row, scope.mt);
    }
    let Input::Nodes(nodes, edge) = input else {
        return RuntimeValue::Null;
    };
    match expr {
        TqlExpr::Binary { left, op, right } => eval_binary(
            expression(left, input, scope),
            *op,
            expression(right, input, scope),
        ),
        TqlExpr::Coalesce(values) => values
            .iter()
            .map(|v| expression(v, input, scope))
            .find(|v| !matches!(v, RuntimeValue::Null))
            .unwrap_or(RuntimeValue::Null),
        TqlExpr::IsNull { expr, negated } => RuntimeValue::Bool(
            matches!(expression(expr, input, scope), RuntimeValue::Null) != *negated,
        ),
        TqlExpr::Property { var, field } if edge.is_some_and(|(name, _)| name == var) => {
            let Some((_, edge)) = edge else {
                return RuntimeValue::Null;
            };
            match field.as_str() {
                "weight" => RuntimeValue::Float(f64::from(edge.weight)),
                "label" => RuntimeValue::Str(edge.label.clone()),
                "source" | "source_id" => runtime_u64(edge.source_id),
                "target" | "target_id" => runtime_u64(edge.target_id),
                "metadata" => json_to_runtime(&edge.metadata),
                _ => json_to_runtime(&edge.metadata[field]),
            }
        }
        TqlExpr::Variable(var) => {
            if let Some((name, edge)) = edge
                && *name == var
            {
                RuntimeValue::Edge((*edge).clone())
            } else {
                nodes
                    .get(var)
                    .map_or(RuntimeValue::Null, |node| RuntimeValue::Node(node.id))
            }
        }
        TqlExpr::Property { var, field } => {
            let node = nodes.get(var).or_else(|| {
                if matches!(scope.query.entry, QueryEntry::Find { .. }) {
                    nodes.values().next()
                } else {
                    None
                }
            });
            node.map_or(RuntimeValue::Null, |node| {
                if field == "id" {
                    runtime_u64(node.id)
                } else {
                    json_to_runtime(&node.payload[field])
                }
            })
        }
        _ => extract_order_key(expr, nodes),
    }
}

fn kind_runtime<T: VectorType>(
    kind: &ReturnExprKind,
    input: &Input<'_, T>,
    scope: &Scope<'_, T>,
) -> Result<RuntimeValue, TriviumError> {
    Ok(match kind {
        ReturnExprKind::Var(var) => {
            if matches!(scope.query.entry, QueryEntry::Find { .. })
                && let Input::Nodes(nodes, _) = input
                && let Some(value) = nodes.get(var).and_then(|node| node.payload.get(var))
            {
                json_to_runtime(value)
            } else {
                expression(&TqlExpr::Variable(var.clone()), input, scope)
            }
        }
        ReturnExprKind::Property(var, field) => expression(
            &TqlExpr::Property {
                var: var.clone(),
                field: field.clone(),
            },
            input,
            scope,
        ),
        ReturnExprKind::Scalar(expr) => expression(expr, input, scope),
        ReturnExprKind::Aggregate(..) => {
            return Err(TriviumError::QueryExecution(
                "聚合需要分组上下文 (Aggregates require a grouping context)".into(),
            ));
        }
    })
}

fn runtime_signature(value: RuntimeValue) -> Result<String, TriviumError> {
    let (kind, value) = match value {
        RuntimeValue::Node(id) => ("node", serde_json::json!(id)),
        RuntimeValue::Edge(edge) => (
            "edge",
            serde_json::json!([edge.source_id, edge.target_id, edge.label]),
        ),
        RuntimeValue::Path(path) => ("path", serde_json::json!(path)),
        other => ("value", runtime_json(other)),
    };
    signature(serde_json::json!([kind, canonical_json(value)]))
}

pub(super) fn matches_predicate<T: VectorType>(
    predicate: &Predicate,
    input: &Input<'_, T>,
    scope: &Scope<'_, T>,
) -> bool {
    match predicate {
        Predicate::Compare { left, op, right } => compare_runtime(
            &expression(left, input, scope),
            op,
            &expression(right, input, scope),
        ),
        Predicate::And(a, b) => {
            matches_predicate(a, input, scope) && matches_predicate(b, input, scope)
        }
        Predicate::Or(a, b) => {
            matches_predicate(a, input, scope) || matches_predicate(b, input, scope)
        }
        Predicate::Not(value) => !matches_predicate(value, input, scope),
        Predicate::DocFilter { var, filter } => {
            let Input::Nodes(nodes, edge) = input else {
                return false;
            };
            if let Some((name, edge)) = edge
                && var.as_deref() == Some(*name)
            {
                return filter.matches(&edge_json(edge));
            }
            let node = var.as_ref().and_then(|var| nodes.get(var)).or_else(|| {
                if var.is_none() {
                    nodes
                        .iter()
                        .min_by_key(|(name, _)| *name)
                        .map(|(_, node)| node)
                } else {
                    None
                }
            });
            node.is_some_and(|node| filter.matches(&node.payload))
        }
    }
}

fn kind_value<T: VectorType>(
    kind: &ReturnExprKind,
    input: &Input<'_, T>,
    scope: &Scope<'_, T>,
) -> Result<TqlValue<T>, TriviumError> {
    if let Input::Pipeline(row) = input {
        return pipeline_kind_value(kind, row, scope.mt, scope.memory);
    }
    let Input::Nodes(nodes, edge) = input else {
        return Ok(TqlValue::Null);
    };
    Ok(match kind {
        ReturnExprKind::Var(var) => {
            if let Some((name, edge)) = edge
                && *name == var
            {
                TqlValue::Edge((*edge).clone())
            } else if matches!(scope.query.entry, QueryEntry::Find { .. })
                && let Some(v) = nodes.get(var).and_then(|n| n.payload.get(var))
            {
                json_to_tql_value(v.clone())
            } else {
                nodes
                    .get(var)
                    .cloned()
                    .map_or(TqlValue::Null, TqlValue::Node)
            }
        }
        ReturnExprKind::Property(var, field) => {
            if edge.is_some_and(|(name, _)| name == var) {
                runtime_to_tql_value(
                    expression(
                        &TqlExpr::Property {
                            var: var.clone(),
                            field: field.clone(),
                        },
                        input,
                        scope,
                    ),
                    scope.mt,
                    scope.memory,
                )?
            } else {
                nodes.get(var).map_or(TqlValue::Null, |node| {
                    json_to_tql_value(if field == "id" {
                        serde_json::json!(node.id)
                    } else {
                        node.payload
                            .get(field)
                            .cloned()
                            .unwrap_or(serde_json::Value::Null)
                    })
                })
            }
        }
        ReturnExprKind::Scalar(expr) => {
            runtime_to_tql_value(expression(expr, input, scope), scope.mt, scope.memory)?
        }
        ReturnExprKind::Aggregate(..) => {
            return Err(TriviumError::QueryExecution(
                "聚合只能在分组阶段求值 (Aggregates must be evaluated in the grouping stage)"
                    .into(),
            ));
        }
    })
}

fn project<T: VectorType>(
    input: &Input<'_, T>,
    scope: &Scope<'_, T>,
) -> Result<HashMap<String, TqlValue<T>>, TriviumError> {
    let mut values = HashMap::new();
    match &scope.query.returns {
        ReturnClause::Expressions(items) => {
            for item in items {
                values.insert(column_name(item), kind_value(&item.kind, input, scope)?);
            }
        }
        ReturnClause::Variables(vars) => {
            for var in vars {
                values.insert(
                    var.clone(),
                    kind_value(&ReturnExprKind::Var(var.clone()), input, scope)?,
                );
            }
        }
        ReturnClause::All => match input {
            Input::Nodes(nodes, edge) => {
                values.extend(
                    nodes
                        .iter()
                        .map(|(name, node)| (name.clone(), TqlValue::Node(node.clone()))),
                );
                if let Some((name, edge)) = edge {
                    values.insert((*name).to_owned(), TqlValue::Edge((*edge).clone()));
                }
                if let QueryEntry::OptionalMatch { pattern } = &scope.query.entry {
                    for name in pattern
                        .nodes
                        .iter()
                        .filter_map(|node| node.var.as_ref())
                        .chain(pattern.edges.iter().filter_map(|edge| edge.var.as_ref()))
                    {
                        values.entry(name.clone()).or_insert(TqlValue::Null);
                    }
                }
            }
            Input::Pipeline(_) => {
                let names = scope
                    .pipeline
                    .ok_or_else(|| {
                        TriviumError::QueryExecution(
                            "管线作用域缺失 (Missing pipeline scope)".into(),
                        )
                    })?
                    .1;
                for name in names {
                    values.insert(
                        name.clone(),
                        kind_value(&ReturnExprKind::Var(name.clone()), input, scope)?,
                    );
                }
            }
        },
    }
    Ok(values)
}

fn aggregate<T: VectorType>(
    func: AggFunc,
    inner: &ReturnExprKind,
    distinct: bool,
    group: &[usize],
    inputs: &[Input<'_, T>],
    scope: &Scope<'_, T>,
) -> Result<TqlValue<T>, TriviumError> {
    let star = matches!(inner,ReturnExprKind::Var(v) if v == "*");
    let mut values = Vec::new();
    let mut seen = HashSet::new();
    for &index in group {
        scope.limits.check_cancelled()?;
        let value = if star {
            RuntimeValue::Bool(true)
        } else {
            kind_runtime(inner, &inputs[index], scope)?
        };
        if matches!(value, RuntimeValue::Null) {
            continue;
        }
        if distinct && !seen.insert(runtime_signature(value.clone())?) {
            continue;
        }
        if matches!(
            func,
            AggFunc::Sum | AggFunc::Avg | AggFunc::Min | AggFunc::Max
        ) {
            if let Some(number) = runtime_number(&value) {
                values.push(serde_json::Value::Number(number));
            }
        } else {
            values.push(runtime_json(value));
        }
    }
    Ok(match func {
        AggFunc::Count => json_to_tql_value(serde_json::json!(values.len())),
        AggFunc::Collect => TqlValue::List(values),
        _ => json_to_tql_value(aggregate_numbers(
            func,
            values.into_iter().filter_map(|v| v.as_number().cloned()),
        )),
    })
}

fn order_value<T: VectorType>(
    expr: &TqlExpr,
    row: &Row<T>,
    inputs: &[Input<'_, T>],
    scope: &Scope<'_, T>,
) -> Result<RuntimeValue, TriviumError> {
    Ok(match expr {
        TqlExpr::Property { var, field } => {
            if let Some(value) = row.values.get(var) {
                runtime_property(&tql_value_runtime(value), field, scope.mt)
            } else if let ReturnClause::Expressions(items) = &scope.query.returns
                && let Some(item) = items.iter().find(|item| item.alias.as_ref() == Some(var))
                && let Some(index) = row.source
            {
                runtime_property(
                    &kind_runtime(&item.kind, &inputs[index], scope)?,
                    field,
                    scope.mt,
                )
            } else {
                row.source.map_or(RuntimeValue::Null, |index| {
                    expression(expr, &inputs[index], scope)
                })
            }
        }
        TqlExpr::Variable(alias) => {
            if let Some(value) = row.values.get(alias) {
                tql_value_runtime(value)
            } else if let ReturnClause::Expressions(items) = &scope.query.returns
                && let Some(item) = items.iter().find(|item| item.alias.as_ref() == Some(alias))
                && let Some(index) = row.source
            {
                kind_runtime(&item.kind, &inputs[index], scope)?
            } else {
                row.source.map_or(RuntimeValue::Null, |index| {
                    expression(expr, &inputs[index], scope)
                })
            }
        }
        TqlExpr::Binary { left, op, right } => eval_binary(
            order_value(left, row, inputs, scope)?,
            *op,
            order_value(right, row, inputs, scope)?,
        ),
        TqlExpr::Coalesce(values) => {
            let mut found = RuntimeValue::Null;
            for expr in values {
                let value = order_value(expr, row, inputs, scope)?;
                if !matches!(value, RuntimeValue::Null) {
                    found = value;
                    break;
                }
            }
            found
        }
        TqlExpr::IsNull { expr, negated } => RuntimeValue::Bool(
            matches!(order_value(expr, row, inputs, scope)?, RuntimeValue::Null) != *negated,
        ),
        _ => row.source.map_or(RuntimeValue::Null, |index| {
            expression(expr, &inputs[index], scope)
        }),
    })
}

pub(super) fn finish<T: VectorType>(
    inputs: &[Input<'_, T>],
    scope: &Scope<'_, T>,
) -> Result<Vec<Row<T>>, TriviumError> {
    validate_return_columns(&scope.query.returns)?;
    admit_return_workspace(inputs, scope)?;
    let expressions = match &scope.query.returns {
        ReturnClause::Expressions(items) => Some(items.as_slice()),
        _ => None,
    };
    let has_agg =
        expressions.is_some_and(|items| items.iter().any(|item| is_aggregate(&item.kind)));
    if has_agg
        && matches!(scope.query.entry, QueryEntry::Find { .. })
        && scope.pipeline.is_none()
        && let Some(var) = expressions
            .into_iter()
            .flatten()
            .find_map(|item| match &item.kind {
                ReturnExprKind::Var(v) if v != "_" && v != "*" => Some(v),
                _ => None,
            })
    {
        return Err(TriviumError::QueryExecution(format!(
            "FIND 聚合字段请写 _.{var} (Use _.{var} for a FIND aggregate field)"
        )));
    }
    let mut rows = Vec::new();
    if has_agg {
        let items = expressions.unwrap_or_default();
        let group_items = items
            .iter()
            .filter(|item| !is_aggregate(&item.kind))
            .collect::<Vec<_>>();
        let mut groups = BTreeMap::<Vec<String>, Vec<usize>>::new();
        for (index, input) in inputs.iter().enumerate() {
            scope.limits.check_cancelled()?;
            let key = group_items
                .iter()
                .map(|item| kind_runtime(&item.kind, input, scope).and_then(runtime_signature))
                .collect::<Result<Vec<_>, _>>()?;
            groups.entry(key).or_default().push(index);
        }
        if inputs.is_empty() && group_items.is_empty() {
            groups.insert(Vec::new(), Vec::new());
        }
        for group in groups.into_values() {
            let source = group.first().copied();
            let mut values = HashMap::new();
            for item in items {
                let value = match &item.kind {
                    ReturnExprKind::Aggregate(func, inner) => {
                        aggregate(*func, inner, item.distinct, &group, inputs, scope)?
                    }
                    kind => source
                        .map(|index| kind_value(kind, &inputs[index], scope))
                        .transpose()?
                        .unwrap_or(TqlValue::Null),
                };
                values.insert(column_name(item), value);
            }
            rows.push(Row { source, values });
        }
    } else {
        let distinct = expressions
            .into_iter()
            .flatten()
            .filter(|item| item.distinct)
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        for (index, input) in inputs.iter().enumerate() {
            scope.limits.check_cancelled()?;
            if !distinct.is_empty() {
                let key = distinct
                    .iter()
                    .map(|item| kind_runtime(&item.kind, input, scope).and_then(runtime_signature))
                    .collect::<Result<Vec<_>, _>>()?;
                if !seen.insert(key) {
                    continue;
                }
            }
            rows.push(Row {
                source: Some(index),
                values: HashMap::new(),
            });
        }
    }
    if !scope.query.order_by.is_empty() {
        let mut sorted = rows
            .into_iter()
            .map(|row| {
                let keys = scope
                    .query
                    .order_by
                    .iter()
                    .map(|order| order_value(&order.expr, &row, inputs, scope))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((row, keys))
            })
            .collect::<Result<Vec<_>, TriviumError>>()?;
        sorted.sort_by(|(left, a), (right, b)| {
            for ((a, b), order) in a.iter().zip(b).zip(&scope.query.order_by) {
                let cmp = compare_for_sort(a, b);
                let cmp = if order.descending { cmp.reverse() } else { cmp };
                if !cmp.is_eq() {
                    return cmp;
                }
            }
            left.source.cmp(&right.source)
        });
        rows = sorted.into_iter().map(|(row, _)| row).collect();
    }
    apply_offset_limit(&mut rows, scope.query.offset, scope.query.limit);
    if !has_agg {
        for row in &mut rows {
            if let Some(index) = row.source {
                row.values = project(&inputs[index], scope)?;
            }
        }
    }
    Ok(rows)
}

/// 节点结果适配保留绑定快照；聚合列按旧节点容器返回，求值始终使用独立返回列。
pub(super) fn into_nodes<T: VectorType>(
    rows: Vec<Row<T>>,
    inputs: &[Input<'_, T>],
    scope: &Scope<'_, T>,
) -> Result<TqlResult<T>, TriviumError> {
    let mut output = Vec::new();
    for row in rows {
        let mut bindings = match row.source.map(|index| &inputs[index]) {
            Some(Input::Nodes(nodes, _)) => (*nodes).clone(),
            Some(Input::Pipeline(input)) => {
                let name = scope.pipeline.map_or("_", |(name, _)| name);
                let names = match &scope.query.returns {
                    ReturnClause::All => scope.pipeline.map_or_else(
                        || vec![name.to_owned()],
                        |(_, names)| names.iter().cloned().collect(),
                    ),
                    ReturnClause::Variables(names) => names.clone(),
                    ReturnClause::Expressions(items) => extract_vars_from_exprs(items),
                };
                let mut bindings = HashMap::new();
                for name in names {
                    if let RuntimeValue::Node(id) =
                        eval_pipeline_row_expr(&TqlExpr::Variable(name.clone()), input, scope.mt)
                    {
                        let projected = row.values.values().find_map(|value| match value {
                            TqlValue::Node(node) if node.id == id => Some(node.clone()),
                            _ => None,
                        });
                        let node = match projected {
                            Some(node) => node,
                            None => scope.memory.snapshot(id, scope.mt)?,
                        };
                        bindings.insert(name, node);
                    }
                }
                bindings
            }
            None => HashMap::new(),
        };
        let retained = match &scope.query.returns {
            ReturnClause::All => None,
            ReturnClause::Variables(vars) => Some(vars.clone()),
            ReturnClause::Expressions(items) => {
                let grouping = items.iter().any(|item| is_aggregate(&item.kind));
                let selected = items
                    .iter()
                    .filter(|item| !grouping || !is_aggregate(&item.kind))
                    .cloned()
                    .collect::<Vec<_>>();
                Some(extract_vars_from_exprs(&selected))
            }
        };
        if let Some(retained) = retained {
            bindings.retain(|name, _| retained.contains(name));
        }
        if let ReturnClause::Expressions(items) = &scope.query.returns {
            for item in items.iter().filter(|item| is_aggregate(&item.kind)) {
                let column = column_name(item);
                let value = row.values.get(&column).map(value_json).ok_or_else(|| {
                    TriviumError::QueryExecution(
                        "聚合返回列缺失 (Missing aggregate return column)".into(),
                    )
                })?;
                let name = item.alias.clone().unwrap_or_else(|| match &item.kind {
                    ReturnExprKind::Aggregate(func, _) => format!("{func:?}").to_lowercase(),
                    _ => column.clone(),
                });
                bindings.insert(
                    name.clone(),
                    Node {
                        id: 0,
                        vector: Vec::new(),
                        payload: serde_json::json!({name:value}),
                        edges: Vec::new(),
                    },
                );
            }
        }
        output.push(bindings);
    }
    apply_projection_pruning(&scope.query.returns, &mut output);
    Ok(output)
}

// 返回、分组、去重与排序可能同时持有输入快照、键及输出副本，先准入再复制。
fn admit_return_workspace<T: VectorType>(
    inputs: &[Input<'_, T>],
    scope: &Scope<'_, T>,
) -> Result<(), TriviumError> {
    let columns = match &scope.query.returns {
        ReturnClause::Expressions(items) => items.len(),
        ReturnClause::Variables(names) => names.len(),
        ReturnClause::All => 1,
    };
    let grouped = matches!(&scope.query.returns, ReturnClause::Expressions(items) if items.iter().any(|item| is_aggregate(&item.kind) || item.distinct));
    let copies = columns
        .saturating_add(scope.query.order_by.len().saturating_mul(8))
        .saturating_add(if grouped { 8 } else { 3 });
    let mut bytes = inputs.len().checked_mul(512).ok_or_else(memory::overflow)?;
    for input in inputs {
        match input {
            Input::Nodes(nodes, edge) => {
                for node in nodes.values() {
                    bytes = bytes
                        .checked_add(memory::snapshot_bytes(node)?)
                        .ok_or_else(memory::overflow)?;
                }
                if let Some((_, edge)) = edge {
                    bytes = bytes
                        .checked_add(crate::storage::payload_store::query_clone_peak(
                            &edge.metadata,
                        ))
                        .and_then(|v| v.checked_add(edge.label.len()))
                        .ok_or_else(memory::overflow)?;
                }
            }
            Input::Pipeline(row) => {
                bytes = bytes
                    .checked_add(pipeline_return_bytes(row, scope)?)
                    .ok_or_else(memory::overflow)?;
            }
        }
    }
    scope
        .memory
        .retain(bytes.checked_mul(copies).ok_or_else(memory::overflow)?)
}

fn pipeline_binding_bytes<T: VectorType>(
    row: &crate::query::pipeline::NodeRow,
    var: &str,
    field: Option<&str>,
    scope: &Scope<'_, T>,
) -> Result<usize, TriviumError> {
    let value = row.bindings.as_ref().and_then(|bindings| bindings.get(var));
    let node = match value {
        Some(RuntimeValue::Node(id)) => Some(*id),
        None => Some(row.id),
        _ => None,
    };
    if let Some(id) = node {
        return if !scope.mt.contains(id) || field == Some("id") {
            Ok(64)
        } else if field.is_some() {
            scope.mt.payload_query_peak(id)
        } else {
            memory::node_bytes(id, scope.mt)
        };
    }
    Ok(match (value, field) {
        (Some(RuntimeValue::Json(value)), field) => field
            .map_or(Some(value), |key| value.get(key))
            .map_or(64, crate::storage::payload_store::query_clone_peak),
        (Some(RuntimeValue::Str(value)), None) => value.len().saturating_add(64),
        (Some(RuntimeValue::Path(value)), None) => value.len().saturating_mul(8).saturating_add(64),
        (Some(RuntimeValue::Edge(edge)), None | Some("metadata")) => {
            crate::storage::payload_store::query_clone_peak(&edge.metadata)
                .saturating_add(edge.label.len())
                .saturating_add(256)
        }
        (Some(RuntimeValue::Edge(edge)), Some("label")) => edge.label.len().saturating_add(64),
        _ => 64,
    })
}

fn pipeline_expr_bytes<T: VectorType>(
    expr: &TqlExpr,
    row: &crate::query::pipeline::NodeRow,
    scope: &Scope<'_, T>,
) -> Result<usize, TriviumError> {
    match expr {
        TqlExpr::Variable(var) => pipeline_binding_bytes(row, var, None, scope),
        TqlExpr::Property { var, field } => pipeline_binding_bytes(row, var, Some(field), scope),
        TqlExpr::Literal(TqlLiteral::Str(value)) => Ok(value.len().saturating_add(64)),
        TqlExpr::Binary { left, right, .. } => pipeline_expr_bytes(left, row, scope)?
            .checked_add(pipeline_expr_bytes(right, row, scope)?)
            .ok_or_else(memory::overflow),
        TqlExpr::Coalesce(items) => items.iter().try_fold(0usize, |sum, item| {
            sum.checked_add(pipeline_expr_bytes(item, row, scope)?)
                .ok_or_else(memory::overflow)
        }),
        TqlExpr::IsNull { expr, .. } => pipeline_expr_bytes(expr, row, scope),
        TqlExpr::Path { .. } => Ok(row
            .path
            .as_ref()
            .map_or(64, |path| path.len().saturating_mul(8).saturating_add(64))),
        TqlExpr::Similarity { .. } => Ok(scope
            .mt
            .dim()
            .saturating_mul(std::mem::size_of::<T>())
            .saturating_add(256)),
        _ => Ok(64),
    }
}

fn pipeline_kind_bytes<T: VectorType>(
    kind: &ReturnExprKind,
    row: &crate::query::pipeline::NodeRow,
    scope: &Scope<'_, T>,
) -> Result<usize, TriviumError> {
    match kind {
        ReturnExprKind::Var(var) if var == "*" => Ok(64),
        ReturnExprKind::Var(var) => pipeline_binding_bytes(row, var, None, scope),
        ReturnExprKind::Property(var, field) => {
            pipeline_binding_bytes(row, var, Some(field), scope)
        }
        ReturnExprKind::Scalar(expr) => pipeline_expr_bytes(expr, row, scope),
        ReturnExprKind::Aggregate(_, inner) => pipeline_kind_bytes(inner, row, scope),
    }
}

// 管线只为实际求值的列计量；常量、标量别名和节点 ID 投影保持轻量。
fn pipeline_return_bytes<T: VectorType>(
    row: &crate::query::pipeline::NodeRow,
    scope: &Scope<'_, T>,
) -> Result<usize, TriviumError> {
    let mut bytes = 256usize;
    match &scope.query.returns {
        ReturnClause::Expressions(items) => {
            for item in items {
                bytes = bytes
                    .checked_add(pipeline_kind_bytes(&item.kind, row, scope)?)
                    .ok_or_else(memory::overflow)?;
            }
        }
        ReturnClause::Variables(names) => {
            for name in names {
                bytes = bytes
                    .checked_add(pipeline_binding_bytes(row, name, None, scope)?)
                    .ok_or_else(memory::overflow)?;
            }
        }
        ReturnClause::All => {
            if let Some((_, names)) = scope.pipeline {
                for name in names {
                    bytes = bytes
                        .checked_add(pipeline_binding_bytes(row, name, None, scope)?)
                        .ok_or_else(memory::overflow)?;
                }
            }
        }
    }
    for order in &scope.query.order_by {
        let alias = if let (TqlExpr::Variable(name), ReturnClause::Expressions(items)) =
            (&order.expr, &scope.query.returns)
        {
            items.iter().find(|item| item.alias.as_ref() == Some(name))
        } else {
            None
        };
        let cost = match alias {
            Some(item) => pipeline_kind_bytes(&item.kind, row, scope)?,
            None => pipeline_expr_bytes(&order.expr, row, scope)?,
        };
        bytes = bytes.checked_add(cost).ok_or_else(memory::overflow)?;
    }
    Ok(bytes)
}
