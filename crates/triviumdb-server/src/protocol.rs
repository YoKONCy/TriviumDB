//! TriviumDB Server HTTP 协议、OCC 前置条件与稳定错误模型。

use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use triviumdb::error::TriviumError;
use triviumdb::query::tql_executor::{TqlMutResult, TqlValue, TqlValueResult};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthDetailsResponse {
    pub status: &'static str,
    pub reason: &'static str,
    pub version: &'static str,
    pub write_queue_depth: usize,
    pub write_queue_capacity: usize,
    pub active_reads: usize,
    pub waiting_reads: usize,
    pub waiting_writers: usize,
    pub active_blocking_tasks: usize,
    pub writer_alive: bool,
    pub writer_failed: bool,
    pub quiver_warmup: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TqlRequest {
    pub query: String,
    #[serde(default)]
    pub mutation: bool,
    #[serde(default)]
    pub profile: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareRequest {
    pub query: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutePreparedRequest {
    #[serde(default)]
    pub parameters: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub profile: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TqlResponse {
    pub rows: Vec<BTreeMap<String, serde_json::Value>>,
    pub row_count: usize,
    pub generation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationResponse {
    pub affected: usize,
    pub created_ids: Vec<String>,
}

impl From<TqlMutResult> for MutationResponse {
    fn from(result: TqlMutResult) -> Self {
        Self {
            affected: result.affected,
            created_ids: result
                .created_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransactionRequest {
    #[serde(default)]
    pub expected_generation: Option<String>,
    #[serde(default)]
    pub expected_nodes: BTreeMap<u64, String>,
    #[serde(default)]
    pub expected_edges: Vec<EdgePrecondition>,
    pub operations: Vec<TransactionOperation>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexKind {
    Hash,
    Ordered,
    Composite,
    Bitmap,
    Ngram,
    Unique,
    UniqueComposite,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexRequest {
    pub kind: IndexKind,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConditionalMutationRequest {
    pub field: String,
    pub expected: serde_json::Value,
    pub replacement: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteManyRequest {
    pub ids: Vec<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexedLookupRequest {
    pub equalities: BTreeMap<String, serde_json::Value>,
    #[serde(default = "default_lookup_limit")]
    pub max_results: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubstringLookupRequest {
    pub field: String,
    pub needle: String,
    #[serde(default = "default_lookup_limit")]
    pub max_results: usize,
}

fn default_lookup_limit() -> usize {
    10_000
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BulkNodeRecord {
    #[serde(default)]
    pub id: Option<u64>,
    pub vector: Vec<f32>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BulkEdgeRecord {
    pub source: u64,
    pub target: u64,
    pub label: String,
    pub weight: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgePrecondition {
    pub source: u64,
    pub target: u64,
    pub label: String,
    pub etag: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum TransactionOperation {
    Insert {
        #[serde(default)]
        id: Option<u64>,
        vector: Vec<f32>,
        payload: serde_json::Value,
    },
    UpdatePayload {
        id: u64,
        payload: serde_json::Value,
    },
    Delete {
        id: u64,
    },
    Link {
        source: u64,
        target: u64,
        label: String,
        weight: f32,
    },
    Unlink {
        source: u64,
        target: u64,
        #[serde(default)]
        label: Option<String>,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionResponse {
    pub created_ids: Vec<String>,
    pub generation: String,
    pub replayed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: &'static str,
    pub message: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiErrorBody {
    pub r#type: String,
    pub title: &'static str,
    pub status: u16,
    pub code: &'static str,
    pub detail: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ErrorMetric {
    Timeout,
    OccConflict,
}

#[derive(Debug, Clone)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    title: &'static str,
    detail: String,
    retryable: bool,
}

impl ApiError {
    pub fn new(
        status: StatusCode,
        code: &'static str,
        title: &'static str,
        detail: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            status,
            code,
            title,
            detail: detail.into(),
            retryable,
        }
    }

    pub(crate) fn metric(&self) -> Option<ErrorMetric> {
        match self.code {
            "REQUEST_TIMEOUT" => Some(ErrorMetric::Timeout),
            "WRITE_CONFLICT" => Some(ErrorMetric::OccConflict),
            _ => None,
        }
    }

    pub fn invalid_request(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "请求无效 (Invalid request)",
            detail,
            false,
        )
    }

    pub fn payload_too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "PAYLOAD_TOO_LARGE",
            "请求体过大 (Payload too large)",
            "请求体超过服务端字节上限 (The request body exceeds the server byte limit)",
            false,
        )
    }

    pub fn timeout() -> Self {
        Self::new(
            StatusCode::GATEWAY_TIMEOUT,
            "REQUEST_TIMEOUT",
            "请求超时 (Request timeout)",
            "请求超过服务端执行时限 (The request exceeded the server execution deadline)",
            true,
        )
    }

    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "SERVICE_UNAVAILABLE",
            "服务暂不可用 (Service unavailable)",
            detail,
            true,
        )
    }

    pub fn queue_full() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "WRITE_QUEUE_FULL",
            "写队列已满 (Write queue is full)",
            "请求已被背压保护拒绝，请稍后重试 (The request was rejected by backpressure; retry later)",
            true,
        )
    }

    pub fn write_conflict(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "WRITE_CONFLICT",
            "写入冲突 (Write conflict)",
            detail,
            true,
        )
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            "服务器内部错误 (Internal server error)",
            detail,
            false,
        )
    }
}

impl From<TriviumError> for ApiError {
    fn from(error: TriviumError) -> Self {
        match error {
            TriviumError::QueryParse(detail) => Self::new(
                StatusCode::BAD_REQUEST,
                "QUERY_PARSE_ERROR",
                "查询解析失败 (Query parse error)",
                detail,
                false,
            ),
            TriviumError::QueryExecution(detail) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "QUERY_EXECUTION_ERROR",
                "查询执行失败 (Query execution error)",
                detail,
                false,
            ),
            TriviumError::QueryCancelled => Self::timeout(),
            TriviumError::QueryRowBudgetExceeded { budget } => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "QUERY_ROW_BUDGET_EXCEEDED",
                "查询行预算耗尽 (Query row budget exceeded)",
                format!("最多允许处理 {budget} 行 (At most {budget} rows may be processed)"),
                false,
            ),
            TriviumError::TraversalBudgetExceeded { .. }
            | TriviumError::CapacityReservationRejected { .. }
            | TriviumError::CapacityAllocationFailed { .. } => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "RESOURCE_BUDGET_EXCEEDED",
                "资源预算耗尽 (Resource budget exceeded)",
                error.to_string(),
                false,
            ),
            TriviumError::NodeNotFound(id) => Self::new(
                StatusCode::NOT_FOUND,
                "NODE_NOT_FOUND",
                "节点不存在 (Node not found)",
                format!("节点 {id} 不存在 (Node {id} does not exist)"),
                false,
            ),
            TriviumError::NodeAlreadyExists(id) => Self::new(
                StatusCode::CONFLICT,
                "NODE_ALREADY_EXISTS",
                "节点已存在 (Node already exists)",
                format!("节点 {id} 已存在 (Node {id} already exists)"),
                false,
            ),
            TriviumError::UniqueConstraintViolation { .. } => Self::new(
                StatusCode::CONFLICT,
                "UNIQUE_CONSTRAINT_VIOLATION",
                "唯一约束冲突 (Unique constraint violation)",
                error.to_string(),
                false,
            ),
            TriviumError::ConditionalUpdateNotMatched { .. } => Self::new(
                StatusCode::CONFLICT,
                "CONDITIONAL_UPDATE_NOT_MATCHED",
                "条件更新未匹配 (Conditional update did not match)",
                error.to_string(),
                false,
            ),
            TriviumError::ReadOnlyViolation { .. } => Self::new(
                StatusCode::FORBIDDEN,
                "READ_ONLY_VIOLATION",
                "只读操作被拒绝 (Read-only operation denied)",
                error.to_string(),
                false,
            ),
            TriviumError::InvalidInput(detail) | TriviumError::InvalidVector { reason: detail } => {
                Self::invalid_request(detail)
            }
            TriviumError::DimensionMismatch { .. } | TriviumError::PayloadTooLarge { .. } => {
                Self::invalid_request(error.to_string())
            }
            TriviumError::DatabaseLocked(_) => Self::new(
                StatusCode::LOCKED,
                "DATABASE_LOCKED",
                "数据库已锁定 (Database locked)",
                error.to_string(),
                true,
            ),
            TriviumError::DatabaseClosed => Self::unavailable(error.to_string()),
            _ => Self::internal(error.to_string()),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.title, self.detail)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let metric = self.metric();
        let body = ApiErrorBody {
            r#type: format!(
                "https://triviumdb.dev/errors/{}",
                self.code.to_ascii_lowercase().replace('_', "-")
            ),
            title: self.title,
            status: self.status.as_u16(),
            code: self.code,
            detail: self.detail,
            retryable: self.retryable,
        };
        let mut response = (self.status, Json(body)).into_response();
        if let Some(metric) = metric {
            response.extensions_mut().insert(metric);
        }
        response
    }
}

pub fn encode_row(
    row: std::collections::HashMap<String, TqlValue<f32>>,
) -> Result<BTreeMap<String, serde_json::Value>, ApiError> {
    row.into_iter()
        .map(|(name, value)| Ok((name, encode_value(value)?)))
        .collect()
}

pub fn encode_rows(rows: TqlValueResult<f32>, generation: String) -> Result<TqlResponse, ApiError> {
    let row_count = rows.len();
    let rows = rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|(name, value)| Ok((name, encode_value(value)?)))
                .collect::<Result<BTreeMap<_, _>, ApiError>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TqlResponse {
        rows,
        row_count,
        generation,
    })
}

pub fn encode_node(node: triviumdb::node::NodeView<f32>) -> serde_json::Value {
    serde_json::json!({
        "type": "node",
        "id": node.id.to_string(),
        "vector": node.vector,
        "payload": node.payload,
        "edges": node.edges.into_iter().map(|edge| serde_json::json!({
            "targetId": edge.target_id.to_string(),
            "label": edge.label,
            "weight": edge.weight,
            "metadata": edge.metadata,
        })).collect::<Vec<_>>(),
    })
}

fn encode_value(value: TqlValue<f32>) -> Result<serde_json::Value, ApiError> {
    Ok(match value {
        TqlValue::Node(node) => encode_node(node),
        TqlValue::Int(value) => serde_json::json!({"type": "int", "value": value}),
        TqlValue::Float(value) => serde_json::json!({"type": "float", "value": value}),
        TqlValue::String(value) => serde_json::json!({"type": "string", "value": value}),
        TqlValue::Bool(value) => serde_json::json!({"type": "bool", "value": value}),
        TqlValue::Path(ids) => serde_json::json!({
            "type": "path",
            "value": ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>()
        }),
        TqlValue::List(value) => serde_json::json!({"type": "list", "value": value}),
        TqlValue::Null => serde_json::json!({"type": "null"}),
    })
}
