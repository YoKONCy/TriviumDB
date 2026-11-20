//! 面向 JSON/JavaScript 传输的错误字段编码；Rust/Python 保持原生整数。

use serde_json::Value;

/// 批量普通 API 的临时事务由绑定持有，公开结果只保留内核阶段和提交事实。
pub(crate) fn direct_write_error(mut error: crate::TriviumError) -> crate::TriviumError {
    let mut source = &mut error;
    loop {
        match source {
            crate::TriviumError::WriteFailure { write, .. } => {
                write.transaction_consumed = None;
                break;
            }
            crate::TriviumError::ApiField { source: inner, .. }
            | crate::TriviumError::QuerySource { source: inner, .. }
            | crate::TriviumError::QueryParameterSource { source: inner, .. }
            | crate::TriviumError::QueryParameter { source: inner, .. } => source = inner,
            _ => break,
        }
    }
    error
}

/// 顶层参数保留公开名称，成员位置使用 RFC 6901 路径。
#[cfg(any(feature = "python", feature = "nodejs"))]
pub(crate) fn field_member(field: &str, member: &str) -> String {
    let prefix = if field.starts_with('/') {
        field.to_owned()
    } else {
        format!("/{field}")
    };
    format!("{prefix}/{}", member.replace('~', "~0").replace('/', "~1"))
}

/// 语言适配层按公开签名判定调用形状，SDK 统一编码字段与已知写入结果。
#[cfg(any(feature = "python", feature = "nodejs"))]
pub(crate) fn call_argument_info(
    owner: &str,
    operation: &str,
    field: &str,
    kind: &str,
) -> crate::Result<crate::error::ErrorInfo> {
    use crate::error::TriviumError;
    let reason = match kind {
        "missing" => "缺少必需实参 (Missing required argument)",
        "unknown_keyword" => {
            "关键字未在公开签名中声明 (Keyword is not declared in the public signature)"
        }
        "multiple_values" => "同一参数收到多个值 (Multiple values for one parameter)",
        "positional_only" => {
            "仅限位置的参数收到关键字 (Positional-only parameter received a keyword)"
        }
        "too_many_positional" => "位置实参数量超过公开签名 (Too many positional arguments)",
        "invalid_arguments" => "实参不符合公开签名 (Arguments do not match the public signature)",
        _ => {
            return Err(TriviumError::InvalidInput(
                "调用形状诊断类别无效 (Invalid call argument diagnostic kind)".into(),
            ));
        }
    };
    let mut error = TriviumError::InvalidInput(reason.into()).at_field(field);
    if let Some(write) = call_write_info(owner, operation) {
        error = error.with_write_info(write);
    }
    let mut info = error.info();
    info.details = serde_json::json!({"reason": reason, "argument_kind": kind});
    Ok(info)
}

#[cfg(any(feature = "python", feature = "nodejs"))]
pub(crate) fn request_operation(owner: &str, operation: &str) -> bool {
    match owner {
        "TriviumDB" => !matches!(
            operation,
            "close"
                | "close_timeout"
                | "dispose"
                | "request_shutdown"
                | "shutdown_status"
                | "__exit__"
                | "__repr__"
                | "dtype"
                | "dim"
                | "resource_stats"
                | "instance_stats"
                | "storage_write_stats"
                | "index_memory_stats"
                | "payload_memory_stats"
        ),
        "Transaction" => !matches!(
            operation,
            "rollback" | "pending_count" | "__exit__" | "__repr__"
        ),
        "module" => matches!(operation, "verify_snapshot" | "restore_snapshot"),
        _ => false,
    }
}

#[cfg(any(feature = "python", feature = "nodejs"))]
pub(crate) fn request_admission(
    error: crate::TriviumError,
    owner: &str,
    operation: &str,
) -> crate::TriviumError {
    match call_write_info(owner, operation) {
        Some(mut write) => {
            write.phase = crate::error::WritePhase::Admission;
            error.with_write_info(write)
        }
        None => error,
    }
}

#[cfg(any(feature = "python", feature = "nodejs"))]
fn call_write_info(owner: &str, operation: &str) -> Option<crate::error::WriteErrorInfo> {
    use crate::error::WriteErrorInfo;
    let transaction = owner == "Transaction" && !matches!(operation, "pending_count" | "__repr__");
    let direct_write = (owner == "TriviumDB"
        && matches!(
            operation,
            "insert"
                | "insert_with_id"
                | "upsert_with_id"
                | "batch_insert"
                | "batch_insert_with_ids"
                | "delete"
                | "delete_many_atomic"
                | "update_payload"
                | "patch_payload"
                | "update_vector"
                | "compare_and_set_payload_field"
                | "link"
                | "upsert_edge"
                | "update_edge"
                | "unlink"
                | "tql_mut"
                | "commit_transaction"
                | "index_text"
                | "index_keyword"
                | "build_text_index"
                | "rebuild_text_index"
                | "create_index"
                | "create_ordered_index"
                | "create_composite_index"
                | "create_bitmap_index"
                | "create_ngram_index"
                | "drop_index"
                | "drop_ordered_index"
                | "drop_composite_index"
                | "drop_bitmap_index"
                | "drop_ngram_index"
                | "create_unique_index"
                | "create_unique_composite_index"
                | "repair_graph_indexes"
                | "set_sync_mode"
                | "set_memory_limit"
                | "reserve_nodes"
                | "set_auto_build_quiver"
                | "enable_auto_compaction"
                | "disable_auto_compaction"
                | "compact"
                | "flush"
                | "publish_generation_manifest"
                | "create_snapshot"
                | "migrate"
        ))
        || (owner == "module" && operation == "restore_snapshot");
    (transaction || direct_write).then_some(WriteErrorInfo {
        transaction_consumed: (transaction || operation == "commit_transaction").then_some(false),
        ..WriteErrorInfo::MAINTENANCE_VALIDATION
    })
}

/// 将超出 JavaScript 安全整数域的错误字段编码为十进制字符串。
pub fn json_error_details(value: Value) -> Value {
    match value {
        Value::Number(number)
            if number
                .as_u64()
                .is_some_and(|value| value > 9_007_199_254_740_991)
                || number
                    .as_i64()
                    .is_some_and(|value| value.unsigned_abs() > 9_007_199_254_740_991) =>
        {
            Value::String(number.to_string())
        }
        Value::Array(values) => Value::Array(values.into_iter().map(json_error_details).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, json_error_details(value)))
                .collect(),
        ),
        other => other,
    }
}

/// JavaScript 数量字段使用 number，须在整数域内检查后再转换。
#[cfg(any(feature = "nodejs", test))]
pub(crate) fn js_count(count: usize, field: &'static str) -> crate::Result<f64> {
    if count as u64 > 9_007_199_254_740_991 {
        return Err(crate::TriviumError::InvalidInput(
            "计数超过 JavaScript 安全整数范围 (Count exceeds the JavaScript safe integer range)"
                .into(),
        )
        .at_field(field));
    }
    Ok(count as f64)
}

#[cfg(all(test, target_pointer_width = "64"))]
#[test]
fn 数量转换保留完整安全整数并拒绝越界值() {
    assert_eq!(
        js_count(1usize << 32, "nodeCount").unwrap(),
        4_294_967_296.0
    );
    assert_eq!(
        js_count((1usize << 53) - 1, "nodeCount").unwrap(),
        9_007_199_254_740_991.0
    );
    assert_eq!(
        js_count(1usize << 53, "nodeCount").unwrap_err().code(),
        "TDB_INVALID_INPUT"
    );
}

#[cfg(all(test, any(feature = "python", feature = "nodejs")))]
mod tests {
    use super::direct_write_error;
    use crate::TriviumError;
    use crate::error::{WriteErrorInfo, WriteOutcome, WritePhase};

    #[test]
    fn direct_batch_preserves_kernel_failure_and_omits_internal_consumption() {
        for (phase, outcome) in [
            (WritePhase::Validation, WriteOutcome::NotCommitted),
            (WritePhase::Wal, WriteOutcome::Unknown),
            (WritePhase::Apply, WriteOutcome::Unknown),
            (WritePhase::Publish, WriteOutcome::Committed),
        ] {
            let error = TriviumError::NodeNotFound(7)
                .with_write_info(WriteErrorInfo {
                    phase,
                    outcome,
                    transaction_consumed: Some(true),
                })
                .at_field("ids");
            let error = direct_write_error(error);
            assert!(matches!(error.underlying(), TriviumError::NodeNotFound(7)));
            let info = error.info();
            assert_eq!(info.code, "TDB_NODE_NOT_FOUND");
            assert_eq!(info.details, serde_json::json!({"id": 7}));
            assert_eq!(
                serde_json::to_value(info.target).unwrap(),
                serde_json::json!({"field": "ids"})
            );
            assert_eq!(
                info.write,
                Some(WriteErrorInfo {
                    phase,
                    outcome,
                    transaction_consumed: None,
                })
            );
        }
    }
}
