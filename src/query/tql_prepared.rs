//! Prepared TQL 的参数收集与严格绑定。

use super::tql_ast::*;
use crate::error::TriviumError;
use std::collections::{BTreeSet, HashMap};

/// Prepared TQL 支持的标量参数值。
#[derive(Debug, Clone, PartialEq)]
pub enum TqlParamValue {
    Int(i64),
    UInt(u64),
    Float(f64),
    String(String),
    Bool(bool),
    Null,
}

impl TqlParamValue {
    pub fn from_json(value: &serde_json::Value) -> Result<Self, TriviumError> {
        match value {
            serde_json::Value::Null => Ok(Self::Null),
            serde_json::Value::Bool(value) => Ok(Self::Bool(*value)),
            serde_json::Value::String(value) => Ok(Self::String(value.clone())),
            serde_json::Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    Ok(Self::Int(value))
                } else if let Some(value)=value.as_u64() {Ok(Self::UInt(value))
                } else if let Some(value) = value.as_f64().filter(|value| value.is_finite()) {
                    Ok(Self::Float(value))
                } else {
                    Err(TriviumError::InvalidInput(
                        "Prepared TQL 参数必须是有限数值 (Prepared TQL parameter must be finite)"
                            .into(),
                    ))
                }
            }
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                Err(TriviumError::InvalidInput(
                    "Prepared TQL 参数仅支持 null/bool/string/number (Prepared TQL parameters only support null/bool/string/number)".into(),
                ))
            }
        }
    }
}

/// 已完成语法分析、可重复绑定执行的只读 TQL。
#[derive(Debug, Clone)]
pub struct PreparedTql {
    query: TqlQuery,
    parameters: BTreeSet<String>,
}

impl PreparedTql {
    /// 从已解析的只读查询创建 Prepared TQL。
    pub fn from_query(mut query: TqlQuery) -> Self {
        if let ReturnClause::Expressions(items) = &mut query.returns {
            for item in items {
                if item.alias.is_none() {
                    item.alias = Some(super::tql_executor::return_column_name(item));
                }
            }
        }
        let mut parameters = BTreeSet::new();
        collect_query_parameters(&query, &mut parameters);
        Self { query, parameters }
    }

    /// 返回按名称稳定排序的必需参数。
    pub fn parameter_names(&self) -> Vec<&str> {
        self.parameters.iter().map(String::as_str).collect()
    }

    /// 绑定全部参数；缺参、额外参数和非有限浮点均 fail-closed。
    pub fn bind(&self, values: &HashMap<String, TqlParamValue>) -> Result<TqlQuery, TriviumError> {
        if let Some(name) = self
            .parameters
            .iter()
            .find(|name| !values.contains_key(*name))
        {
            return Err(TriviumError::MissingQueryParameter {
                parameter: name.clone(),
            });
        }
        if let Some(name) = values
            .keys()
            .filter(|name| !self.parameters.contains(*name))
            .min()
        {
            return Err(TriviumError::UnexpectedQueryParameter {
                parameter: name.clone(),
            });
        }
        let mut query = self.query.clone();
        bind_query(&mut query, values)?;
        Ok(query)
    }
}

pub(crate) fn collect_query_parameters(query: &TqlQuery, output: &mut BTreeSet<String>) {
    if let QueryEntry::Search {
        vector_parameters, ..
    } = &query.entry
    {
        output.extend(vector_parameters.iter().map(|(_, name)| name.clone()));
    }
    for stage in &query.pipeline {
        match stage {
            PipelineStage::With(with) => {
                for item in &with.items {
                    collect_expr_parameters(&item.expr, output);
                }
            }
            PipelineStage::Filter(predicate) => collect_predicate_parameters(predicate, output),
            _ => {}
        }
    }
    if let Some(predicate) = &query.predicate {
        collect_predicate_parameters(predicate, output);
    }
    for order in &query.order_by {
        collect_expr_parameters(&order.expr, output);
    }
    if let ReturnClause::Expressions(items) = &query.returns {
        for item in items {
            collect_kind_parameters(&item.kind, output);
        }
    }
}

fn collect_kind_parameters(kind: &ReturnExprKind, output: &mut BTreeSet<String>) {
    match kind {
        ReturnExprKind::Scalar(expr) => collect_expr_parameters(expr, output),
        ReturnExprKind::Aggregate(_, inner) => collect_kind_parameters(inner, output),
        ReturnExprKind::Var(_) | ReturnExprKind::Property(_, _) => {}
    }
}

fn collect_predicate_parameters(predicate: &Predicate, output: &mut BTreeSet<String>) {
    match predicate {
        Predicate::Compare { left, right, .. } => {
            collect_expr_parameters(left, output);
            collect_expr_parameters(right, output);
        }
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            collect_predicate_parameters(left, output);
            collect_predicate_parameters(right, output);
        }
        Predicate::Not(inner) => collect_predicate_parameters(inner, output),
        Predicate::DocFilter { .. } => {}
    }
}

fn collect_expr_parameters(expr: &TqlExpr, output: &mut BTreeSet<String>) {
    match expr {
        TqlExpr::Parameter(name) => {
            output.insert(name.clone());
        }
        TqlExpr::Binary { left, right, .. } => {
            collect_expr_parameters(left, output);
            collect_expr_parameters(right, output);
        }
        TqlExpr::Coalesce(values) => {
            for value in values {
                collect_expr_parameters(value, output);
            }
        }
        TqlExpr::IsNull { expr, .. } => collect_expr_parameters(expr, output),
        _ => {}
    }
}

fn bind_query(
    query: &mut TqlQuery,
    values: &HashMap<String, TqlParamValue>,
) -> Result<(), TriviumError> {
    if let QueryEntry::Search {
        vector,
        vector_parameters,
        ..
    } = &mut query.entry
    {
        let mut slots = std::collections::HashSet::new();
        for (index, name) in vector_parameters.iter() {
            if !slots.insert(*index) {
                return Err(TriviumError::QueryExecution(
                    "Prepared 向量槽位重复 (Duplicate prepared vector slot)".into(),
                ));
            }
            let value = values
                .get(name)
                .ok_or_else(|| TriviumError::MissingQueryParameter {
                    parameter: name.clone(),
                })?;
            let slot = vector.get_mut(*index).ok_or_else(|| {
                TriviumError::QueryExecution(
                    "Prepared 向量槽位越界 (Prepared vector slot is out of bounds)".into(),
                )
            })?;
            *slot = match value {
                TqlParamValue::Int(value) => (*value).into(),
                TqlParamValue::UInt(value) => (*value).into(),
                TqlParamValue::Float(value) if value.is_finite() => {
                    serde_json::Number::from_f64(*value).ok_or_else(|| {
                        TriviumError::InvalidInput(
                            "Prepared 向量分量必须有限 (Prepared vector components must be finite)"
                                .into(),
                        )
                    })?
                }
                _ => {
                    return Err(TriviumError::QueryExecution(format!(
                        "SEARCH VECTOR 参数 ${name} 必须是有限数值 (SEARCH VECTOR parameter ${name} must be a finite number)"
                    )));
                }
            };
        }
        vector_parameters.clear();
    }
    for stage in &mut query.pipeline {
        match stage {
            PipelineStage::With(with) => {
                for item in &mut with.items {
                    bind_expr(&mut item.expr, values)?;
                }
            }
            PipelineStage::Filter(predicate) => bind_predicate(predicate, values)?,
            _ => {}
        }
    }
    if let Some(predicate) = &mut query.predicate {
        bind_predicate(predicate, values)?;
    }
    for order in &mut query.order_by {
        bind_expr(&mut order.expr, values)?;
    }
    if let ReturnClause::Expressions(items) = &mut query.returns {
        for item in items {
            bind_kind(&mut item.kind, values)?;
        }
    }
    Ok(())
}

fn bind_kind(
    kind: &mut ReturnExprKind,
    values: &HashMap<String, TqlParamValue>,
) -> Result<(), TriviumError> {
    match kind {
        ReturnExprKind::Scalar(expr) => bind_expr(expr, values),
        ReturnExprKind::Aggregate(_, inner) => bind_kind(inner, values),
        ReturnExprKind::Var(_) | ReturnExprKind::Property(_, _) => Ok(()),
    }
}

fn bind_predicate(
    predicate: &mut Predicate,
    values: &HashMap<String, TqlParamValue>,
) -> Result<(), TriviumError> {
    match predicate {
        Predicate::Compare { left, right, .. } => {
            bind_expr(left, values)?;
            bind_expr(right, values)
        }
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            bind_predicate(left, values)?;
            bind_predicate(right, values)
        }
        Predicate::Not(inner) => bind_predicate(inner, values),
        Predicate::DocFilter { .. } => Ok(()),
    }
}

fn bind_expr(
    expr: &mut TqlExpr,
    values: &HashMap<String, TqlParamValue>,
) -> Result<(), TriviumError> {
    match expr {
        TqlExpr::Parameter(name) => {
            let value = values
                .get(name)
                .ok_or_else(|| TriviumError::MissingQueryParameter {
                    parameter: name.clone(),
                })?;
            *expr = TqlExpr::Literal(match value {
                TqlParamValue::Int(value) => TqlLiteral::Int(*value),
                TqlParamValue::UInt(value) => TqlLiteral::UInt(*value),
                TqlParamValue::Float(value) if value.is_finite() => TqlLiteral::Float(*value),
                TqlParamValue::Float(_) => {
                    return Err(TriviumError::QueryExecution(format!(
                        "Prepared TQL 参数 ${name} 必须是有限浮点数 (Prepared TQL parameter ${name} must be finite)"
                    )));
                }
                TqlParamValue::String(value) => TqlLiteral::Str(value.clone()),
                TqlParamValue::Bool(value) => TqlLiteral::Bool(*value),
                TqlParamValue::Null => TqlLiteral::Null,
            });
            Ok(())
        }
        TqlExpr::Binary { left, right, .. } => {
            bind_expr(left, values)?;
            bind_expr(right, values)
        }
        TqlExpr::Coalesce(values_expr) => {
            for value in values_expr {
                bind_expr(value, values)?;
            }
            Ok(())
        }
        TqlExpr::IsNull { expr, .. } => bind_expr(expr, values),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod parameter_error_tests {
    use super::*;

    #[test]
    fn 参数错误按名称稳定报告且包含具体字段() {
        let query =
            super::super::tql_parser::parse_tql("MATCH (n) RETURN $a AS first, $b AS second")
                .unwrap();
        let prepared = PreparedTql::from_query(query);
        let missing = prepared.bind(&HashMap::new()).unwrap_err();
        assert!(
            matches!(missing, TriviumError::MissingQueryParameter { parameter } if parameter == "a")
        );
        for extras in [["z", "c"], ["c", "z"]] {
            let mut values = HashMap::from([
                ("a".into(), TqlParamValue::Int(1)),
                ("b".into(), TqlParamValue::Int(2)),
            ]);
            for name in extras {
                values.insert(name.into(), TqlParamValue::Int(0));
            }
            assert!(
                matches!(prepared.bind(&values), Err(TriviumError::UnexpectedQueryParameter { parameter }) if parameter == "c")
            );
        }
    }
}
