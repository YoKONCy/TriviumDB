//! TQL、Prepared 参数与查询结果的语言适配。

use super::arguments::{parse_management_string, parse_string_argument};
use super::database::{DbBackend, TriviumDB};
use super::errors::to_napi_error;
use super::json::NodeJson;
use napi::bindgen_prelude::Either;
use napi::{Env, JsObject, JsUnknown, ValueType};
use napi_derive::napi;

pub(super) fn node_tql_scalar_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Number(ref number)
            if number
                .as_i64()
                .is_some_and(|n| n.unsigned_abs() > 9_007_199_254_740_991)
                || number.as_u64().is_some_and(|n| n > 9_007_199_254_740_991) =>
        {
            serde_json::Value::String(number.to_string())
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(node_tql_scalar_json).collect())
        }
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .into_iter()
                .map(|(name, value)| (name, node_tql_scalar_json(value)))
                .collect(),
        ),
        value => value,
    }
}

fn node_tql_row_to_json<T: crate::VectorType>(
    row: std::collections::HashMap<String, crate::query::tql_executor::TqlValue<T>>,
) -> NodeJson {
    use crate::query::tql_executor::TqlValue;
    let mut obj = serde_json::Map::new();
    for (name, value) in row {
        let value = match value {
            TqlValue::Node(node) => serde_json::json!({
                "id": node.id.to_string(),
                "payload": node.payload,
                "numEdges": node.edges.len(),
            }),
            TqlValue::Edge(edge) => edge.query_json(
                edge.source_id.to_string().into(),
                edge.target_id.to_string().into(),
            ),
            TqlValue::Int(value) => node_tql_scalar_json(serde_json::json!(value)),
            TqlValue::UInt(value) => node_tql_scalar_json(serde_json::json!(value)),
            TqlValue::Float(value) => serde_json::json!(value),
            TqlValue::String(value) => serde_json::json!(value),
            TqlValue::Bool(value) => serde_json::json!(value),
            TqlValue::Path(value) => serde_json::json!(
                value
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
            ),
            TqlValue::List(value) => node_tql_scalar_json(serde_json::Value::Array(value)),
            TqlValue::Object(value) => node_tql_scalar_json(serde_json::Value::Object(value)),
            TqlValue::Null => serde_json::Value::Null,
        };
        obj.insert(name, value);
    }
    serde_json::Value::Object(obj).into()
}

#[napi]
pub struct JsPreparedTql {
    inner: crate::query::tql_prepared::PreparedTql,
}

#[napi]
impl JsPreparedTql {
    #[napi]
    pub fn parameter_names(&self) -> Vec<String> {
        self.inner
            .parameter_names()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
}

fn collect_prepared_parameters(
    env: Env,
    parameters: JsUnknown,
    prepared: &crate::query::tql_prepared::PreparedTql,
) -> napi::Result<std::collections::HashMap<String, crate::query::tql_prepared::TqlParamValue>> {
    let invalid_container = || {
        to_napi_error(
            env,
            crate::TriviumError::InvalidInput(
                "Prepared TQL 参数必须是对象 (Prepared TQL parameters must be an object)".into(),
            )
            .at_field("parameters"),
        )
    };
    if parameters.get_type()? != ValueType::Object {
        return Err(invalid_container());
    }
    let object = JsObject::try_from(parameters)?;
    if object.is_array()? {
        return Err(invalid_container());
    }
    let keys = object.get_property_names()?;
    let mut values = std::collections::HashMap::new();
    for index in 0..keys.get_array_length()? {
        let name = keys
            .get_element::<napi::JsString>(index)?
            .into_utf8()?
            .as_str()?
            .to_owned();
        // 枚举与 getter 可执行宿主代码，原异常直接传播。
        let value: JsUnknown = object.get_property(env.create_string(&name)?)?;
        let invalid_value = || {
            to_napi_error(
                    env,
                    prepared.locate_parameter_error(crate::TriviumError::InvalidInput(
                        "参数值无法表示为 Prepared 标量 (Value cannot be represented as a Prepared scalar)".into(),
                    )
                    .at_query_parameter(&name)),
                )
        };
        match value.get_type()? {
            ValueType::Null | ValueType::Boolean | ValueType::Number | ValueType::String => {}
            // 复合值在标量边界拒绝，避免遍历无效参数中的循环或宿主 getter。
            _ => return Err(invalid_value()),
        }
        let json = NodeJson::from_js(env, value).map_err(|error| {
            if error.status == napi::Status::InvalidArg {
                invalid_value()
            } else {
                error
            }
        })?;
        let value = prepared
            .parameter_from_json(&name, &json)
            .map_err(|error| to_napi_error(env, error))?;
        values.insert(name, value);
    }
    Ok(values)
}

#[napi]
impl TriviumDB {
    // ── TQL 统一查询 ──

    /// 执行 TQL (Trivium Query Language) 统一查询
    ///
    /// 支持三种入口：MATCH (图遍历) / FIND (文档过滤) / SEARCH (向量检索)
    ///
    /// ```js
    /// // 图遍历
    /// const rows = db.tql('MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b')
    /// // 文档过滤
    /// const rows = db.tql('FIND {type: "event", heat: {$gte: 0.7}} RETURN *')
    /// ```
    #[napi]
    pub fn tql(&self, env: Env, query: Either<String, JsUnknown>) -> napi::Result<Vec<NodeJson>> {
        let query = parse_string_argument(env, query, "query", None)?;
        match &self.inner {
            DbBackend::F32(db) => db
                .tql_values(&query)
                .map_err(|error| to_napi_error(env, error))
                .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
            DbBackend::F16(db) => db
                .tql_values(&query)
                .map_err(|error| to_napi_error(env, error))
                .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
            DbBackend::U64(db) => db
                .tql_values(&query)
                .map_err(|error| to_napi_error(env, error))
                .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
        }
    }

    #[napi]
    pub fn prepare_tql(
        &self,
        env: Env,
        query: Either<String, JsUnknown>,
    ) -> napi::Result<JsPreparedTql> {
        let query = parse_string_argument(env, query, "query", None)?;
        dispatch!(self, db => db.prepare_tql(&query))
            .map(|inner| JsPreparedTql { inner })
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn execute_prepared_tql(
        &self,
        env: Env,
        prepared: &JsPreparedTql,
        parameters: JsUnknown,
    ) -> napi::Result<Vec<NodeJson>> {
        let values = collect_prepared_parameters(env, parameters, &prepared.inner)?;
        match &self.inner {
            DbBackend::F32(db) => db
                .execute_prepared_tql(&prepared.inner, &values)
                .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
            DbBackend::F16(db) => db
                .execute_prepared_tql(&prepared.inner, &values)
                .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
            DbBackend::U64(db) => db
                .execute_prepared_tql(&prepared.inner, &values)
                .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
        }
        .map_err(|error| to_napi_error(env, error))
    }

    /// 执行 TQL 写操作（CREATE / SET / DELETE / DETACH DELETE）
    ///
    /// 返回 { affected: number, createdIds: number[] }
    ///
    /// ```js
    /// const result = db.tqlMut('CREATE (a {name: "Alice", age: 30})')
    /// console.log(result.affected)     // 1
    /// console.log(result.createdIds)   // [1]
    ///
    /// db.tqlMut('MATCH (a {name: "Alice"}) SET a.age == 31')
    /// db.tqlMut('MATCH (a {name: "Alice"}) DELETE a')
    /// ```
    /// 执行内核 TQL 写操作。
    #[napi]
    pub fn tql_mut(
        &mut self,
        env: Env,
        query: Either<String, JsUnknown>,
    ) -> napi::Result<NodeJson> {
        let query = parse_management_string(env, query, "query")?;
        let result =
            dispatch!(self, mut db => db.tql_mut_with_id_limit(&query,9_007_199_254_740_991))
                .map_err(|error| to_napi_error(env, error))?;
        Ok(serde_json::json!({
            "affected": result.affected,
            "createdIds": result.created_ids,
        })
        .into())
    }
}
