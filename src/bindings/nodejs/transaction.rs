//! 事务构建、提交及事务对象的语言生命周期。

use super::arguments::{node_id_to_js, parse_safe_usize, validate_u64_component};
use super::database::{DbBackend, TriviumDB};
use super::errors::{argument_input_error, to_napi_error, with_napi_write_info};
use super::json::NodeJson;
use napi::{Env, JsUnknown};
use napi_derive::napi;

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
                operation.get("payload").cloned().unwrap_or_default(),
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
        let write = crate::error::WriteErrorInfo {
            phase: crate::error::WritePhase::Validation,
            ..crate::error::WriteErrorInfo::TRANSACTION_ADMISSION
        };
        let operations = NodeJson::from_js_field(env, operations, "operations", Some(write))?.0;
        let operations = operations.as_array().ok_or_else(|| {
            argument_input_error(
                env,
                "operations",
                "事务操作必须是数组 (Transaction operations must be an array)",
                Some(write),
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
        self.check_transaction_ids(env, operations)
            .map_err(validation)?;
        let ids = match &mut self.inner {
            DbBackend::F32(db) => db.commit_tx(
                build_transaction(env, operations, |value, _| Ok(value as f32))
                    .map_err(validation)?,
            ),
            DbBackend::F16(db) => db.commit_tx(
                build_transaction(env, operations, |value, _| Ok(half::f16::from_f64(value)))
                    .map_err(validation)?,
            ),
            DbBackend::U64(db) => db.commit_tx(
                build_transaction(env, operations, |value, field| {
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
