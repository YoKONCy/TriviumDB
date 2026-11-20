//! 事务构建、提交及事务对象的语言生命周期。

use super::arguments::{
    argument_object, node_id_to_js, parse_safe_usize, read_array, validate_u64_component,
};
use super::database::{DbBackend, TriviumDB};
use super::errors::{argument_input_error, to_napi_error, with_napi_write_info};
use super::json::{NodeJson, enumerable_own_keys, string_from_js};
use crate::VectorType;
use napi::{Env, JsUnknown, ValueType};
use napi_derive::napi;

fn transaction_arguments(env: Env, operations: JsUnknown) -> napi::Result<Vec<serde_json::Value>> {
    read_array(env, operations, "operations", |env, item, path| {
        let operation = argument_object(env, item, path, None)?;
        let keys = enumerable_own_keys(&operation)?;
        let mut fields = serde_json::Map::new();
        for index in 0..keys.get_array_length()? {
            let key: JsUnknown = keys.get_element(index)?;
            if key.get_type()? == ValueType::Symbol {
                continue;
            }
            let key = string_from_js(key)?;
            let field = crate::sdk::field_member(path, &key);
            let value: JsUnknown = operation.get_property(env.create_string(&key)?)?;
            let value = match key.as_str() {
                "id" | "src" | "dst" | "weight" => transaction_number(env, value, &field)?,
                "vector" => {
                    serde_json::Value::Array(read_array(env, value, &field, transaction_number)?)
                }
                _ => NodeJson::from_js_field(env, value, &field, None)?.0,
            };
            fields.insert(key, value);
        }
        Ok(serde_json::Value::Object(fields))
    })
}

fn transaction_number(env: Env, value: JsUnknown, field: &str) -> napi::Result<serde_json::Value> {
    if value.get_type()? != ValueType::Number {
        return Err(argument_input_error(
            env,
            field,
            "事务参数必须是数字 (Transaction argument must be a number)",
            None,
        ));
    }
    NodeJson::from_js_field(env, value, field, None).map(|value| value.0)
}

fn build_transaction<T: crate::VectorType>(
    env: Env,
    operations: &[serde_json::Value],
    convert: impl Fn(f64, &str) -> napi::Result<T>,
) -> napi::Result<crate::database::TxBuilder<T>> {
    let mut tx = crate::database::TxBuilder::new();
    for (index, operation) in operations.iter().enumerate() {
        let path = crate::sdk::field_member("operations", &index.to_string());
        let invalid = |field: &str| {
            argument_input_error(
                env,
                &crate::sdk::field_member(&path, field),
                "事务操作字段无效或缺失 (Transaction operation field is invalid or missing)",
                None,
            )
        };
        let kind = operation
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid("type"))?;
        let id = |name: &str| -> napi::Result<u64> {
            let value = operation
                .get(name)
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| invalid(name))?;
            Ok(parse_safe_usize(env, value, &crate::sdk::field_member(&path, name))? as u64)
        };
        let vector = || -> napi::Result<Vec<T>> {
            operation
                .get("vector")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| invalid("vector"))?
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let field = crate::sdk::field_member(
                        &crate::sdk::field_member(&path, "vector"),
                        &index.to_string(),
                    );
                    value
                        .as_f64()
                        .ok_or_else(|| {
                            argument_input_error(
                                env,
                                &field,
                                "向量成员必须是数字 (Vector components must be numbers)",
                                None,
                            )
                        })
                        .and_then(|value| convert(value, &field))
                })
                .collect()
        };
        match kind {
            "insert" => tx.insert(
                &vector()?,
                operation.get("payload").cloned().unwrap_or_default(),
            ),
            "insertWithId" => tx.insert_with_id(
                id("id")?,
                &vector()?,
                operation.get("payload").cloned().unwrap_or_default(),
            ),
            "delete" => tx.delete(id("id")?),
            "updatePayload" => tx.update_payload(
                id("id")?,
                operation
                    .get("payload")
                    .cloned()
                    .ok_or_else(|| invalid("payload"))?,
            ),
            "updateVector" => tx.update_vector(id("id")?, &vector()?),
            "upsertEdge" | "link" => tx.upsert_edge(
                id("src")?,
                id("dst")?,
                operation
                    .get("label")
                    .map(|value| value.as_str().ok_or_else(|| invalid("label")))
                    .transpose()?
                    .unwrap_or("related"),
                operation
                    .get("weight")
                    .map(|value| value.as_f64().ok_or_else(|| invalid("weight")))
                    .transpose()?
                    .unwrap_or(1.0) as f32,
                operation.get("metadata").cloned().unwrap_or_default(),
            ),
            "unlink" => tx.unlink(id("src")?, id("dst")?),
            "unlinkLabel" => tx.unlink_label(
                id("src")?,
                id("dst")?,
                operation
                    .get("label")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| invalid("label"))?,
            ),
            _ => {
                return Err(invalid("type"));
            }
        }
    }
    Ok(tx)
}

#[napi]
impl TriviumDB {
    /// 提交缓冲操作，返回新插入的节点 ID。
    #[napi]
    pub fn commit_transaction(
        &mut self,
        env: Env,
        operations: JsUnknown,
    ) -> napi::Result<Vec<f64>> {
        dispatch!(self, db => db.check_transaction_admission()).map_err(|error| {
            to_napi_error(
                env,
                error.with_write_info(crate::error::WriteErrorInfo::TRANSACTION_ADMISSION),
            )
        })?;
        let validation = |error| {
            with_napi_write_info(
                env,
                error,
                crate::error::WriteErrorInfo {
                    phase: crate::error::WritePhase::Validation,
                    ..crate::error::WriteErrorInfo::TRANSACTION_ADMISSION
                },
            )
        };
        let operations = transaction_arguments(env, operations).map_err(validation)?;
        self.check_transaction_ids(env, &operations)
            .map_err(validation)?;
        let ids = match &mut self.inner {
            DbBackend::F32(db) => db.commit_tx(
                build_transaction(env, &operations, |value, _| Ok(value as f32))
                    .map_err(validation)?,
            ),
            DbBackend::F16(db) => db.commit_tx(
                build_transaction(env, &operations, |value, _| {
                    Ok(half::f16::from_f64_component(value))
                })
                .map_err(validation)?,
            ),
            DbBackend::U64(db) => db.commit_tx(
                build_transaction(env, &operations, |value, field| {
                    validate_u64_component(value)
                        .map_err(|error| to_napi_error(env, error.at_field(field)))
                })
                .map_err(validation)?,
            ),
        }
        .map_err(|error| to_napi_error(env, error))?;
        ids.into_iter()
            .map(|value| node_id_to_js(env, value))
            .collect::<napi::Result<_>>()
            .map_err(|error| {
                with_napi_write_info(
                    env,
                    error,
                    crate::error::WriteErrorInfo {
                        phase: crate::error::WritePhase::Response,
                        outcome: crate::error::WriteOutcome::Committed,
                        transaction_consumed: Some(true),
                    },
                )
            })
    }
}
