//! TriviumDB Server HTTP 协议、OCC 前置条件与稳定错误模型。

use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::{
    Deserialize, Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq, SerializeStruct},
};
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionColumn {
    pub name: String,
    pub kind: &'static str,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchGetRequest {
    #[serde(deserialize_with = "deserialize_ids")]
    pub ids: Vec<u64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum InputId {
    Number(u64),
    Decimal(String),
}

impl InputId {
    fn into_u64<E: serde::de::Error>(self) -> Result<u64, E> {
        match self {
            Self::Number(value) => Ok(value),
            Self::Decimal(value)
                if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                value.parse().map_err(E::custom)
            }
            Self::Decimal(_) => Err(E::custom(
                "节点 ID 必须是十进制 u64 (Node ID must be a decimal u64)",
            )),
        }
    }
}

fn deserialize_ids<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u64>, D::Error> {
    Vec::<InputId>::deserialize(deserializer)?
        .into_iter()
        .map(InputId::into_u64)
        .collect()
}

fn deserialize_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    InputId::deserialize(deserializer)?.into_u64()
}

fn deserialize_optional_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    Option::<InputId>::deserialize(deserializer)?
        .map(InputId::into_u64)
        .transpose()
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
    #[serde(deserialize_with = "deserialize_ids")]
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
    #[serde(default, deserialize_with = "deserialize_optional_id")]
    pub id: Option<u64>,
    pub vector: Vec<f32>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BulkEdgeRecord {
    #[serde(deserialize_with = "deserialize_id")]
    pub source: u64,
    #[serde(deserialize_with = "deserialize_id")]
    pub target: u64,
    pub label: String,
    pub weight: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgePrecondition {
    #[serde(deserialize_with = "deserialize_id")]
    pub source: u64,
    #[serde(deserialize_with = "deserialize_id")]
    pub target: u64,
    pub label: String,
    pub etag: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum TransactionOperation {
    Insert {
        #[serde(default, deserialize_with = "deserialize_optional_id")]
        id: Option<u64>,
        vector: Vec<f32>,
        payload: serde_json::Value,
    },
    UpdatePayload {
        #[serde(deserialize_with = "deserialize_id")]
        id: u64,
        payload: serde_json::Value,
    },
    Delete {
        #[serde(deserialize_with = "deserialize_id")]
        id: u64,
    },
    Link {
        #[serde(deserialize_with = "deserialize_id")]
        source: u64,
        #[serde(deserialize_with = "deserialize_id")]
        target: u64,
        label: String,
        weight: f32,
    },
    Unlink {
        #[serde(deserialize_with = "deserialize_id")]
        source: u64,
        #[serde(deserialize_with = "deserialize_id")]
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
    #[serde(rename = "schema_version")]
    pub schema_version: u32,
    pub r#type: String,
    pub title: &'static str,
    pub status: u16,
    pub code: &'static str,
    pub detail: String,
    pub message: String,
    pub details: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<triviumdb::error::QueryErrorStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<triviumdb::error::ErrorTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write: Option<triviumdb::error::WriteErrorInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_code: Option<&'static str>,
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
    details: serde_json::Value,
    stage: Option<triviumdb::error::QueryErrorStage>,
    target: Option<Box<triviumdb::error::ErrorTarget>>,
    write: Option<triviumdb::error::WriteErrorInfo>,
    legacy_code: Option<&'static str>,
    retryable: bool,
}

impl ApiError {
    pub(crate) fn at_field(mut self, field: &str) -> Self {
        if self.target.is_none() {
            self.target = Some(Box::new(triviumdb::error::ErrorTarget::Field {
                field: field.into(),
            }));
        }
        self
    }

    pub(crate) fn with_write_info(mut self, write: triviumdb::error::WriteErrorInfo) -> Self {
        if self.write.is_none() {
            self.write = Some(write);
        }
        self.retryable = false;
        self
    }

    pub fn new(
        status: StatusCode,
        code: &'static str,
        title: &'static str,
        detail: impl Into<String>,
        retryable: bool,
    ) -> Self {
        let detail = detail.into();
        let (canonical, details) = match code {
            "INVALID_REQUEST" => ("TDB_INVALID_INPUT", serde_json::json!({"reason":detail})),
            "WRITE_QUEUE_FULL" => ("TDB_BUSY", serde_json::json!({"resource":"write_queue"})),
            "SERVICE_UNAVAILABLE" => ("TDB_SERVER_UNAVAILABLE", serde_json::json!({})),
            "INTERNAL_ERROR" => ("TDB_SERVER_INTERNAL", serde_json::json!({})),
            "PAYLOAD_TOO_LARGE" => ("TDB_HTTP_REQUEST_TOO_LARGE", serde_json::json!({})),
            "RESPONSE_TOO_LARGE" => ("TDB_HTTP_RESPONSE_TOO_LARGE", serde_json::json!({})),
            "UNSUPPORTED_MEDIA_TYPE" => ("TDB_HTTP_UNSUPPORTED_MEDIA_TYPE", serde_json::json!({})),
            "ROUTE_NOT_FOUND" => ("TDB_HTTP_ROUTE_NOT_FOUND", serde_json::json!({})),
            "METHOD_NOT_ALLOWED" => ("TDB_HTTP_METHOD_NOT_ALLOWED", serde_json::json!({})),
            "PREPARED_NOT_FOUND" => ("TDB_PREPARED_NOT_FOUND", serde_json::json!({})),
            "IDEMPOTENCY_KEY_REUSED" => ("TDB_IDEMPOTENCY_CONFLICT", serde_json::json!({})),
            "WRITE_CONFLICT" => ("TDB_HTTP_PRECONDITION_FAILED", serde_json::json!({})),
            _ => (code, serde_json::json!({})),
        };
        Self {
            status,
            code: canonical,
            title,
            detail,
            details,
            stage: None,
            target: None,
            write: None,
            legacy_code: (canonical != code).then_some(code),
            retryable,
        }
    }

    pub(crate) fn metric(&self) -> Option<ErrorMetric> {
        match self.code {
            "REQUEST_TIMEOUT" | "TDB_QUERY_DEADLINE_EXCEEDED" => Some(ErrorMetric::Timeout),
            "WRITE_CONFLICT"
            | "TDB_WRITE_CONFLICT"
            | "TDB_DOCUMENT_REVISION_MISMATCH"
            | "TDB_DOCUMENT_UPLOAD_CONFLICT"
            | "TDB_HTTP_PRECONDITION_FAILED" => Some(ErrorMetric::OccConflict),
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
        Self::from(TriviumError::QueryDeadlineExceeded)
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
        let info = error.info();
        let mut mapped =
            match error {
                TriviumError::QueryParameter { source, .. }
                | TriviumError::QuerySource { source, .. }
                | TriviumError::QueryParameterSource { source, .. }
                | TriviumError::ApiField { source, .. }
                | TriviumError::WriteFailure { source, .. } => Self::from(*source),
                TriviumError::QueryParse(detail)
                | TriviumError::QuerySyntax { reason: detail, .. } => Self::new(
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
                TriviumError::QueryCancelled => Self::new(
                    StatusCode::REQUEST_TIMEOUT,
                    "REQUEST_TIMEOUT",
                    "查询已取消 (Query cancelled)",
                    error.to_string(),
                    false,
                ),
                TriviumError::QueryDeadlineExceeded => Self::new(
                    StatusCode::GATEWAY_TIMEOUT,
                    "REQUEST_TIMEOUT",
                    "查询执行期限已到 (Query deadline exceeded)",
                    error.to_string(),
                    true,
                ),
                TriviumError::TransactionFinished => Self::new(
                    StatusCode::CONFLICT,
                    "TDB_TRANSACTION_FINISHED",
                    "事务已结束 (Transaction finished)",
                    error.to_string(),
                    false,
                ),
                TriviumError::MissingQueryParameter { .. }
                | TriviumError::UnexpectedQueryParameter { .. } => {
                    Self::invalid_request(error.to_string())
                }
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
                TriviumError::DocumentUploadNotFound { .. } => Self::new(
                    StatusCode::NOT_FOUND,
                    info.code,
                    "文档上传不存在 (Document upload does not exist)",
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
                TriviumError::InvalidInput(detail)
                | TriviumError::InvalidVector { reason: detail } => Self::invalid_request(detail),
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
                TriviumError::DatabaseClosed => Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "SERVICE_UNAVAILABLE",
                    "数据库已关闭 (Database closed)",
                    error.to_string(),
                    false,
                ),
                TriviumError::DatabaseBusy { .. } => Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "TDB_BUSY",
                    "数据库请求繁忙 (Database request busy)",
                    error.to_string(),
                    true,
                ),
                TriviumError::ReentrantAccess { .. } => Self::new(
                    StatusCode::CONFLICT,
                    "TDB_REENTRANT_ACCESS",
                    "数据库重入被拒绝 (Database reentrant access rejected)",
                    error.to_string(),
                    false,
                ),
                TriviumError::WriteConflict { .. }
                | TriviumError::DocumentRevisionMismatch { .. }
                | TriviumError::DocumentUploadConflict { .. }
                | TriviumError::IdempotencyConflict
                | TriviumError::WriteOutcomeUnknown => Self::new(
                    StatusCode::CONFLICT,
                    info.code,
                    "事务请求冲突 (Transaction request conflict)",
                    error.to_string(),
                    false,
                ),
                TriviumError::PayloadQueryBudgetExceeded { .. } => Self::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    info.code,
                    "查询预算耗尽 (Query budget exceeded)",
                    error.to_string(),
                    false,
                ),
                TriviumError::ResourceBudgetExceeded { .. } => Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    info.code,
                    "资源准入被拒绝 (Resource admission rejected)",
                    error.to_string(),
                    false,
                ),
                _ => Self::internal(error.to_string()),
            };
        if mapped.code != info.code && mapped.legacy_code.is_none() {
            mapped.legacy_code = Some(mapped.code);
        }
        mapped.code = info.code;
        mapped.detail = info.message;
        mapped.details = triviumdb::sdk::json_error_details(info.details);
        mapped.stage = info.stage;
        mapped.target = info.target.map(Box::new);
        mapped.write = info.write;
        if mapped.write.is_some() {
            // 重新准入及重放由调用方结合结果和事务状态决定。
            mapped.retryable = false;
        }
        mapped
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
            schema_version: triviumdb::error::ERROR_SCHEMA_VERSION,
            r#type: format!(
                "https://triviumdb.dev/errors/{}",
                self.code.to_ascii_lowercase().replace('_', "-")
            ),
            title: self.title,
            status: self.status.as_u16(),
            code: self.code,
            message: self.detail.clone(),
            detail: self.detail,
            details: self.details,
            stage: self.stage,
            target: self.target.map(|target| *target),
            write: self.write,
            legacy_code: self.legacy_code,
            retryable: self.retryable,
        };
        let mut response = (self.status, Json(body)).into_response();
        if let Some(metric) = metric {
            response.extensions_mut().insert(metric);
        }
        response
    }
}

pub fn projection_columns(
    rows: &TqlValueResult<f32>,
    projected: Vec<String>,
) -> Vec<ProjectionColumn> {
    let mut columns: BTreeMap<_, _> = projected.into_iter().map(|name| (name, "null")).collect();
    for row in rows {
        for (name, value) in row {
            let kind = match value {
                TqlValue::Node(_) => "node",
                TqlValue::Edge(_) => "edge",
                TqlValue::Int(_) => "int",
                TqlValue::UInt(_) => "uint",
                TqlValue::Float(_) => "float",
                TqlValue::String(_) => "string",
                TqlValue::Bool(_) => "bool",
                TqlValue::Path(_) => "path",
                TqlValue::List(_) => "list",
                TqlValue::Object(_) => "object",
                TqlValue::Null => "null",
            };
            columns
                .entry(name.clone())
                .and_modify(|current| {
                    if *current == "null" {
                        *current = kind;
                    }
                })
                .or_insert(kind);
        }
    }
    columns
        .into_iter()
        .map(|(name, kind)| ProjectionColumn { name, kind })
        .collect()
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

const JS_MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

struct DecimalId(u64);

impl Serialize for DecimalId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

struct EdgeList<'a>(&'a [triviumdb::node::Edge]);

impl Serialize for EdgeList<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct NodeEdge<'a> {
            target_id: DecimalId,
            label: &'a str,
            weight: f32,
            metadata: &'a serde_json::Value,
        }

        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for edge in self.0 {
            sequence.serialize_element(&NodeEdge {
                target_id: DecimalId(edge.target_id),
                label: &edge.label,
                weight: edge.weight,
                metadata: &edge.metadata,
            })?;
        }
        sequence.end()
    }
}

struct PathIds<'a>(&'a [u64]);

impl Serialize for PathIds<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().copied().map(DecimalId))
    }
}

struct SafeJson<'a>(&'a serde_json::Value);

impl Serialize for SafeJson<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            serde_json::Value::Array(values) => serializer.collect_seq(values.iter().map(SafeJson)),
            serde_json::Value::Object(values) => {
                serializer.collect_map(values.iter().map(|(name, value)| (name, SafeJson(value))))
            }
            serde_json::Value::Number(value)
                if value
                    .as_u64()
                    .is_some_and(|value| value > JS_MAX_SAFE_INTEGER)
                    || value
                        .as_i64()
                        .is_some_and(|value| value.unsigned_abs() > JS_MAX_SAFE_INTEGER) =>
            {
                serializer.collect_str(value)
            }
            value => value.serialize(serializer),
        }
    }
}

#[derive(Serialize)]
struct Tagged<V> {
    r#type: &'static str,
    value: V,
}

struct QueryValue<'a>(&'a TqlValue<f32>);

impl Serialize for QueryValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use TqlValue::*;
        match self.0 {
            Node(node) => {
                let mut value = serializer.serialize_struct("Node", 5)?;
                value.serialize_field("type", "node")?;
                value.serialize_field("id", &DecimalId(node.id))?;
                value.serialize_field("vector", &node.vector)?;
                value.serialize_field("payload", &node.payload)?;
                value.serialize_field("edges", &EdgeList(&node.edges))?;
                value.end()
            }
            Edge(edge) => {
                let mut value = serializer
                    .serialize_struct("Edge", 6 + usize::from(edge.graph_id.is_some()))?;
                value.serialize_field("type", "edge")?;
                value.serialize_field("source", &DecimalId(edge.source_id))?;
                value.serialize_field("target", &DecimalId(edge.target_id))?;
                value.serialize_field("label", &edge.label)?;
                value.serialize_field("weight", &edge.weight)?;
                value.serialize_field("metadata", &edge.metadata)?;
                if let Some(graph_id) = &edge.graph_id {
                    value.serialize_field("graph_id", graph_id)?;
                }
                value.end()
            }
            Int(value) if value.unsigned_abs() > JS_MAX_SAFE_INTEGER => Tagged {
                r#type: "int",
                value: value.to_string(),
            }
            .serialize(serializer),
            Int(value) => Tagged {
                r#type: "int",
                value,
            }
            .serialize(serializer),
            UInt(value) => Tagged {
                r#type: "uint",
                value: DecimalId(*value),
            }
            .serialize(serializer),
            Float(value) => Tagged {
                r#type: "float",
                value,
            }
            .serialize(serializer),
            String(value) => Tagged {
                r#type: "string",
                value,
            }
            .serialize(serializer),
            Bool(value) => Tagged {
                r#type: "bool",
                value,
            }
            .serialize(serializer),
            Path(ids) => Tagged {
                r#type: "path",
                value: PathIds(ids),
            }
            .serialize(serializer),
            List(values) => {
                struct ListValue<'a>(&'a [serde_json::Value]);
                impl Serialize for ListValue<'_> {
                    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                        serializer.collect_seq(self.0.iter().map(SafeJson))
                    }
                }
                Tagged {
                    r#type: "list",
                    value: ListValue(values),
                }
                .serialize(serializer)
            }
            Object(values) => {
                struct ObjectValue<'a>(&'a serde_json::Map<std::string::String, serde_json::Value>);
                impl Serialize for ObjectValue<'_> {
                    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                        serializer
                            .collect_map(self.0.iter().map(|(name, value)| (name, SafeJson(value))))
                    }
                }
                Tagged {
                    r#type: "object",
                    value: ObjectValue(values),
                }
                .serialize(serializer)
            }
            Null => {
                let mut value = serializer.serialize_struct("Null", 1)?;
                value.serialize_field("type", "null")?;
                value.end()
            }
        }
    }
}

type Row = std::collections::HashMap<String, TqlValue<f32>>;

pub struct QueryRow<'a>(pub &'a Row);

impl Serialize for QueryRow<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in self.0 {
            map.serialize_entry(name, &QueryValue(value))?;
        }
        map.end()
    }
}

pub struct QueryRows<'a>(pub &'a [Row]);

impl Serialize for QueryRows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(QueryRow))
    }
}

#[cfg(test)]
mod structured_error_tests {
    use super::*;

    #[test]
    fn 查询边编码保留图身份与完整整数() {
        let edge = triviumdb::node::EdgeView {
            graph_id: Some("events".into()),
            source_id: u64::MAX - 1,
            target_id: u64::MAX - 2,
            label: "rel".into(),
            weight: 2.0,
            metadata: serde_json::json!({"owner":"event"}),
        };
        assert_eq!(
            serde_json::to_value(QueryValue(&TqlValue::Edge(edge.clone()))).unwrap(),
            serde_json::json!({
                "type":"edge","source":"18446744073709551614",
                "target":"18446744073709551613","label":"rel","weight":2.0,
                "metadata":{"owner":"event"},"graph_id":"events"
            })
        );
        let default = triviumdb::node::EdgeView {
            graph_id: None,
            ..edge
        };
        assert_eq!(
            serde_json::to_value(QueryValue(&TqlValue::Edge(default))).unwrap(),
            serde_json::json!({
                "type":"edge","source":"18446744073709551614",
                "target":"18446744073709551613","label":"rel","weight":2.0,
                "metadata":{"owner":"event"}
            })
        );
    }

    #[tokio::test]
    async fn 普通字段上下文保留规范类别与请求结果() {
        let error = TriviumError::WriteFailure {
            source: Box::new(
                TriviumError::InvalidInput("容量必须为正 (Capacity must be positive)".into())
                    .at_field("additional"),
            ),
            write: triviumdb::error::WriteErrorInfo {
                phase: triviumdb::error::WritePhase::Validation,
                outcome: triviumdb::error::WriteOutcome::NotCommitted,
                transaction_consumed: None,
            },
        };
        let response = ApiError::from(error).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["schema_version"], 1);
        assert_eq!(body["code"], "TDB_INVALID_INPUT");
        assert_eq!(body["target"], serde_json::json!({"field":"additional"}));
        assert_eq!(
            body["write"],
            serde_json::json!({"phase":"validation","outcome":"not_committed"})
        );
        assert_eq!(body["retryable"], false);
    }

    #[test]
    fn prepared参数错误作为无效请求返回() {
        for (error, code, parameter) in [
            (
                TriviumError::MissingQueryParameter {
                    parameter: "required".into(),
                },
                "TDB_MISSING_QUERY_PARAMETER",
                "required",
            ),
            (
                TriviumError::UnexpectedQueryParameter {
                    parameter: "extra".into(),
                },
                "TDB_UNEXPECTED_QUERY_PARAMETER",
                "extra",
            ),
        ] {
            let response = ApiError::from(error);
            assert_eq!(response.status, StatusCode::BAD_REQUEST);
            assert_eq!(response.code, code);
            assert_eq!(
                response.details,
                serde_json::json!({"parameter": parameter})
            );
            assert_eq!(response.legacy_code, Some("INVALID_REQUEST"));
            assert!(!response.retryable);
        }
    }

    #[tokio::test]
    async fn 内核锁错误及完整整数通过http保持结构化() {
        for (error, code, status, details, retryable) in [
            (
                TriviumError::TransactionFinished,
                "TDB_TRANSACTION_FINISHED",
                StatusCode::CONFLICT,
                serde_json::json!({}),
                false,
            ),
            (
                TriviumError::DatabaseBusy {
                    resource: "shared_database",
                },
                "TDB_BUSY",
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({"resource":"shared_database"}),
                true,
            ),
            (
                TriviumError::ReentrantAccess {
                    operation: "shared_write",
                },
                "TDB_REENTRANT_ACCESS",
                StatusCode::CONFLICT,
                serde_json::json!({"operation":"shared_write"}),
                false,
            ),
            (
                TriviumError::HostLockPoisoned {
                    resource: "shared_database",
                },
                "TDB_HOST_LOCK_POISONED",
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"resource":"shared_database"}),
                false,
            ),
            (
                TriviumError::NodeNotFound(u64::MAX),
                "TDB_NODE_NOT_FOUND",
                StatusCode::NOT_FOUND,
                serde_json::json!({"id":"18446744073709551615"}),
                false,
            ),
            (
                TriviumError::DocumentUploadNotFound {
                    upload_id: "upload-identity".into(),
                },
                "TDB_DOCUMENT_UPLOAD_NOT_FOUND",
                StatusCode::NOT_FOUND,
                serde_json::json!({"upload_id":"upload-identity"}),
                false,
            ),
            (
                TriviumError::DocumentUploadConflict {
                    field: "sequence".into(),
                },
                "TDB_DOCUMENT_UPLOAD_CONFLICT",
                StatusCode::CONFLICT,
                serde_json::json!({"field":"sequence"}),
                false,
            ),
            (
                TriviumError::QueryCancelled,
                "TDB_QUERY_CANCELLED",
                StatusCode::REQUEST_TIMEOUT,
                serde_json::json!({}),
                false,
            ),
            (
                TriviumError::QueryDeadlineExceeded,
                "TDB_QUERY_DEADLINE_EXCEEDED",
                StatusCode::GATEWAY_TIMEOUT,
                serde_json::json!({}),
                true,
            ),
        ] {
            let response = ApiError::from(error).into_response();
            assert_eq!(response.status(), status);
            let bytes = axum::body::to_bytes(response.into_body(), 16384)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["schema_version"], 1);
            assert_eq!(body["code"], code);
            assert_eq!(body["details"], details);
            assert_eq!(body["retryable"], retryable);
            assert_eq!(body["message"], body["detail"]);
        }
    }
}
