//! 独立指定错误码和字段的样例同时用于核心与 Python 映射验证。

use super::TriviumError as E;
use serde_json::{Value, json};

pub(crate) fn cases() -> Vec<(E, &'static str, Value)> {
    vec![
        (
            E::DtypeMismatch {
                stored: "f32",
                requested: "f16",
            },
            "TDB_DTYPE_MISMATCH",
            json!({"stored":"f32","requested":"f16"}),
        ),
        (
            E::Io(std::io::Error::from_raw_os_error(2)),
            "TDB_IO",
            json!({"io_kind":"NotFound","os_code":2}),
        ),
        (
            E::Serialization(Box::new(bincode::ErrorKind::Custom("编码失败".into()))),
            "TDB_SERIALIZATION",
            json!({"reason":"编码失败"}),
        ),
        (
            E::DimensionMismatch {
                expected: 2,
                got: 1,
            },
            "TDB_DIMENSION_MISMATCH",
            json!({"expected":2,"got":1}),
        ),
        (
            E::NodeNotFound(u64::MAX - 1),
            "TDB_NODE_NOT_FOUND",
            json!({"id":18446744073709551614u64}),
        ),
        (
            E::InvalidVector {
                reason: "非有限值".into(),
            },
            "TDB_INVALID_VECTOR",
            json!({"reason":"非有限值"}),
        ),
        (
            E::PayloadTooLarge {
                size_bytes: 9,
                max_bytes: 8,
            },
            "TDB_PAYLOAD_TOO_LARGE",
            json!({"size_bytes":9,"max_bytes":8}),
        ),
        (
            E::UniqueConstraintViolation {
                fields: vec!["email".into()],
                existing_id: 1,
                conflicting_id: 2,
            },
            "TDB_UNIQUE_CONSTRAINT",
            json!({"fields":["email"],"existing_id":1,"conflicting_id":2}),
        ),
        (
            E::ConditionalUpdateNotMatched { id: 1 },
            "TDB_CONDITIONAL_UPDATE_NOT_MATCHED",
            json!({"id":1}),
        ),
        (
            E::NodeAlreadyExists(1),
            "TDB_NODE_ALREADY_EXISTS",
            json!({"id":1}),
        ),
        (
            E::DatabaseLocked("路径".into()),
            "TDB_DATABASE_LOCKED",
            json!({"reason":"路径"}),
        ),
        (
            E::ReadOnlyViolation {
                operation: "insert",
            },
            "TDB_READ_ONLY",
            json!({"operation":"insert"}),
        ),
        (
            E::RecoveryRequired {
                wal_path: "数据.wal".into(),
            },
            "TDB_RECOVERY_REQUIRED",
            json!({"wal_path":"数据.wal"}),
        ),
        (
            E::ImmutableArtifactInvalid {
                reason: "代际错误".into(),
            },
            "TDB_IMMUTABLE_ARTIFACT",
            json!({"reason":"代际错误"}),
        ),
        (
            E::GenerationBusy {
                generation_id: "g1".into(),
            },
            "TDB_GENERATION_BUSY",
            json!({"generation_id":"g1"}),
        ),
        (
            E::CorruptedFile("校验错误".into()),
            "TDB_CORRUPTED_FILE",
            json!({"reason":"校验错误"}),
        ),
        (
            E::QueryParse("语法错误".into()),
            "TDB_QUERY_PARSE",
            json!({"reason":"语法错误"}),
        ),
        (
            E::QueryExecution("执行失败".into()),
            "TDB_QUERY_EXECUTION",
            json!({"reason":"执行失败"}),
        ),
        (
            E::MissingQueryParameter {
                parameter: "a".into(),
            },
            "TDB_MISSING_QUERY_PARAMETER",
            json!({"parameter":"a"}),
        ),
        (
            E::UnexpectedQueryParameter {
                parameter: "b".into(),
            },
            "TDB_UNEXPECTED_QUERY_PARAMETER",
            json!({"parameter":"b"}),
        ),
        (E::QueryCancelled, "TDB_QUERY_CANCELLED", json!({})),
        (
            E::QueryRowBudgetExceeded { budget: 2 },
            "TDB_QUERY_BUDGET",
            json!({"budget":2}),
        ),
        (
            E::PayloadQueryBudgetExceeded {
                dimension: "bytes",
                used: u64::MAX,
                budget: 9,
            },
            "TDB_PAYLOAD_BUDGET_EXCEEDED",
            json!({"dimension":"bytes","used":18446744073709551615u64,"budget":9}),
        ),
        (
            E::TraversalBudgetExceeded {
                dimension: crate::graph::budget::BudgetDimension::ExaminedEdges,
                visited_nodes: 2,
                examined_edges: 3,
                peak_frontier_size: 1,
                depth_reached: 1,
            },
            "TDB_TRAVERSAL_BUDGET_EXCEEDED",
            json!({"dimension":"examined_edges","visited_nodes":2,"examined_edges":3,"peak_frontier_size":1,"depth_reached":1}),
        ),
        (
            E::HookExecutionError("回调失败".into()),
            "TDB_HOOK_EXECUTION",
            json!({"reason":"回调失败"}),
        ),
        (
            E::HookLoadError("加载失败".into()),
            "TDB_HOOK_LOAD",
            json!({"reason":"加载失败"}),
        ),
        (E::WalClosed, "TDB_WAL_CLOSED", json!({})),
        (
            E::CapacityReservationRejected {
                requested_nodes: 1,
                estimated_bytes: 8,
                current_bytes: 9,
                memory_limit: 10,
            },
            "TDB_CAPACITY_RESERVATION_REJECTED",
            json!({"requested_nodes":1,"estimated_bytes":8,"current_bytes":9,"memory_limit":10}),
        ),
        (
            E::CapacityAllocationFailed {
                reason: "容量溢出".into(),
            },
            "TDB_CAPACITY_ALLOCATION_FAILED",
            json!({"reason":"容量溢出"}),
        ),
        (
            E::ResourceBudgetExceeded {
                resource: "active_tasks",
                requested: 1,
                used: 1,
                limit: 1,
            },
            "QueueFull",
            json!({"resource":"active_tasks","requested":1,"used":1,"limit":1}),
        ),
        (
            E::SnapshotDestinationExists {
                path: "目标".into(),
            },
            "TDB_SNAPSHOT_EXISTS",
            json!({"path":"目标"}),
        ),
        (
            E::SnapshotPublicationUncertain {
                path: "目标".into(),
                reason: "同步失败".into(),
            },
            "TDB_SNAPSHOT_PUBLICATION_UNCERTAIN",
            json!({"path":"目标","reason":"同步失败"}),
        ),
        (
            E::ShutdownFailed {
                reason: "保存失败".into(),
            },
            "TDB_SHUTDOWN_FAILED",
            json!({"reason":"保存失败"}),
        ),
        (E::DatabaseClosed, "TDB_DATABASE_CLOSED", json!({})),
        (
            E::UnsupportedWalVersion {
                found: 4,
                supported: 3,
            },
            "TDB_UNSUPPORTED_WAL_VERSION",
            json!({"found":4,"supported":3}),
        ),
        (
            E::UnsupportedDatabaseVersion {
                found: 11,
                minimum_supported: 5,
                current: 10,
            },
            "TDB_UNSUPPORTED_DATABASE_VERSION",
            json!({"found":11,"minimum_supported":5,"current":10}),
        ),
        (
            E::ApiMigrationRequired {
                removed_api: "old",
                replacement: "new",
            },
            "TDB_API_MIGRATION_REQUIRED",
            json!({"removed_api":"old","replacement":"new"}),
        ),
        (
            E::UnsupportedSidecarVersion {
                component: "pidx",
                found: 8,
                minimum_supported: 1,
                current: 7,
                action: "重建",
            },
            "TDB_UNSUPPORTED_SIDECAR_VERSION",
            json!({"component":"pidx","found":8,"minimum_supported":1,"current":7,"action":"重建"}),
        ),
        (
            E::InvalidInput("无效值".into()),
            "TDB_INVALID_INPUT",
            json!({"reason":"无效值"}),
        ),
        (
            E::Generic("其他错误".into()),
            "TDB_ERROR",
            json!({"reason":"其他错误"}),
        ),
    ]
}

#[test]
fn 错误描述符保持预期类别与完整精度() {
    let mut codes = std::collections::BTreeSet::new();
    for (error, code, details) in cases() {
        assert_eq!(error.code(), code);
        assert_eq!(error.details(), details, "{code}");
        assert!(codes.insert(code), "不同错误类别不应共享错误码");
    }
}
