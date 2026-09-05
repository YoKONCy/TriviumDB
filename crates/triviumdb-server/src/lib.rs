//! TriviumDB Server HTTP 应用：并发读、Writer Actor、幂等写与 OCC。

mod engine;
mod protocol;

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use engine::{EngineConfig, EngineHandle};
use protocol::{
    ApiError, BulkEdgeRecord, BulkNodeRecord, ConditionalMutationRequest, DeleteManyRequest,
    ExecutePreparedRequest, HealthDetailsResponse, HealthResponse, IndexRequest,
    IndexedLookupRequest, PrepareRequest, SubstringLookupRequest, TqlRequest, TransactionOperation,
    TransactionRequest, TransactionResponse, encode_node, encode_row, encode_rows,
};
use serde::Deserialize;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use triviumdb::database::{Config, StorageMode};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub database_path: PathBuf,
    pub database: Config,
    pub write_queue_capacity: usize,
    pub max_concurrent_reads: usize,
    pub idempotency_capacity: usize,
    pub max_write_batch_size: usize,
    pub max_write_batch_delay: Duration,
    pub prepared_cache_capacity: usize,
    pub request_timeout: Duration,
    pub max_body_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            database_path: PathBuf::from("triviumdb-server.tdb"),
            database: Config {
                storage_mode: StorageMode::Mmap,
                max_query_rows: Some(10_000),
                ..Config::default()
            },
            write_queue_capacity: 256,
            max_concurrent_reads: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(4)
                .max(1),
            idempotency_capacity: 4096,
            max_write_batch_size: 64,
            max_write_batch_delay: Duration::from_micros(500),
            prepared_cache_capacity: 1024,
            request_timeout: Duration::from_secs(30),
            max_body_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
struct RequestContext {
    id: String,
    started: Instant,
    telemetry: Arc<engine::RequestTelemetry>,
}

impl RequestContext {
    fn new(id: String) -> Self {
        Self {
            id,
            started: Instant::now(),
            telemetry: Arc::new(engine::RequestTelemetry::default()),
        }
    }
}

#[derive(Default)]
struct RequestMetrics {
    cancelled_total: Arc<AtomicU64>,
    timeout_total: AtomicU64,
    conflict_total: AtomicU64,
}

#[derive(Clone)]
struct AppState {
    engine: EngineHandle,
    request_timeout: Duration,
    request_metrics: Arc<RequestMetrics>,
}

pub async fn build_app(config: ServerConfig) -> Result<Router, ApiError> {
    if config.max_body_bytes == 0 || config.request_timeout.is_zero() {
        return Err(ApiError::invalid_request(
            "请求体上限和超时必须大于 0 (Request body limit and timeout must be greater than zero)",
        ));
    }
    let engine = EngineHandle::start(EngineConfig {
        database_path: config.database_path,
        database: config.database,
        write_queue_capacity: config.write_queue_capacity,
        max_concurrent_reads: config.max_concurrent_reads,
        idempotency_capacity: config.idempotency_capacity,
        max_write_batch_size: config.max_write_batch_size,
        max_write_batch_delay: config.max_write_batch_delay,
        prepared_cache_capacity: config.prepared_cache_capacity,
    })
    .await?;
    let state = Arc::new(AppState {
        engine,
        request_timeout: config.request_timeout,
        request_metrics: Arc::new(RequestMetrics::default()),
    });

    Ok(Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/health/details", get(health_details))
        .route("/metrics", get(metrics))
        .route("/v1/indexes", get(list_indexes).post(create_index))
        .route("/v1/indexes/delete", delete(drop_index))
        .route("/v1/indexes/quiver", get(quiver_status).post(build_quiver))
        .route("/v1/lookup/equality", post(indexed_lookup))
        .route("/v1/lookup/substring", post(substring_lookup))
        .route("/v1/import/nodes", post(import_nodes))
        .route("/v1/import/edges", post(import_edges))
        .route("/v1/tql", post(tql))
        .route("/v1/prepared", post(prepare))
        .route("/v1/prepared/{id}/execute", post(execute_prepared))
        .route("/v1/search/vector", post(search_vector))
        .route("/v1/transactions", post(transaction))
        .route("/v1/nodes/delete-many", post(delete_many))
        .route(
            "/v1/nodes/{id}/compare-and-set",
            post(compare_and_set_payload_field),
        )
        .route("/v1/nodes/{id}", get(get_node))
        .fallback(not_found)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            access_log,
        ))
        .layer(axum::middleware::from_fn(map_body_limit_error))
        .layer(DefaultBodyLimit::max(config.max_body_bytes))
        .layer(ServiceBuilder::new().layer(CatchPanicLayer::new()))
        .with_state(state))
}

async fn live() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        message: "服务存活 (Service is alive)",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let readiness = state.engine.readiness();
    if readiness.ready {
        (
            StatusCode::OK,
            Json(HealthResponse {
                status: "ready",
                message: "数据库已就绪 (Database is ready)",
                version: env!("CARGO_PKG_VERSION"),
            }),
        )
            .into_response()
    } else {
        ApiError::unavailable(format!(
            "数据库尚未就绪，原因：{} (Database is not ready; reason: {})",
            readiness.reason, readiness.reason
        ))
        .into_response()
    }
}

async fn health_details(State(state): State<Arc<AppState>>) -> Json<HealthDetailsResponse> {
    let readiness = state.engine.readiness();
    let metrics = readiness.metrics;
    Json(HealthDetailsResponse {
        status: if readiness.ready {
            "ready"
        } else {
            "unavailable"
        },
        reason: readiness.reason,
        version: env!("CARGO_PKG_VERSION"),
        write_queue_depth: metrics.queue_depth,
        write_queue_capacity: metrics.queue_capacity,
        active_reads: metrics.active_reads,
        waiting_reads: metrics.waiting_reads,
        waiting_writers: metrics.waiting_writers,
        active_blocking_tasks: metrics.active_blocking_tasks,
        writer_alive: metrics.writer_alive,
        writer_failed: metrics.writer_failed,
        quiver_warmup: metrics.warmup_state.as_str(),
    })
}

async fn metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let metrics = state.engine.metrics();
    let body = format!(
        concat!(
            "# TYPE triviumdb_write_queue_depth gauge\n",
            "triviumdb_write_queue_depth {}\n",
            "# TYPE triviumdb_write_queue_capacity gauge\n",
            "triviumdb_write_queue_capacity {}\n",
            "# TYPE triviumdb_write_queued_total counter\n",
            "triviumdb_write_queued_total {}\n",
            "# TYPE triviumdb_write_rejected_total counter\n",
            "triviumdb_write_rejected_total {}\n",
            "# TYPE triviumdb_write_batches_total counter\n",
            "triviumdb_write_batches_total {}\n",
            "# TYPE triviumdb_write_batched_requests_total counter\n",
            "triviumdb_write_batched_requests_total {}\n",
            "# TYPE triviumdb_write_batch_max gauge\n",
            "triviumdb_write_batch_max {}\n",
            "# TYPE triviumdb_wal_fsync_total counter\n",
            "triviumdb_wal_fsync_total {}\n",
            "# TYPE triviumdb_active_reads gauge\n",
            "triviumdb_active_reads {}\n",
            "# TYPE triviumdb_waiting_reads gauge\n",
            "triviumdb_waiting_reads {}\n",
            "# TYPE triviumdb_waiting_writers gauge\n",
            "triviumdb_waiting_writers {}\n",
            "# TYPE triviumdb_active_blocking_tasks gauge\n",
            "triviumdb_active_blocking_tasks {}\n",
            "# TYPE triviumdb_writer_alive gauge\n",
            "triviumdb_writer_alive {}\n",
            "# TYPE triviumdb_writer_failed gauge\n",
            "triviumdb_writer_failed {}\n",
            "# TYPE triviumdb_request_cancelled_total counter\n",
            "triviumdb_request_cancelled_total {}\n",
            "# TYPE triviumdb_request_timeout_total counter\n",
            "triviumdb_request_timeout_total {}\n",
            "# TYPE triviumdb_occ_conflict_total counter\n",
            "triviumdb_occ_conflict_total {}\n"
        ),
        metrics.queue_depth,
        metrics.queue_capacity,
        metrics.queued_total,
        metrics.rejected_total,
        metrics.batches_total,
        metrics.batched_writes_total,
        metrics.max_observed_batch_size,
        metrics.wal_sync_total,
        metrics.active_reads,
        metrics.waiting_reads,
        metrics.waiting_writers,
        metrics.active_blocking_tasks,
        usize::from(metrics.writer_alive),
        usize::from(metrics.writer_failed),
        state
            .request_metrics
            .cancelled_total
            .load(Ordering::Relaxed),
        state.request_metrics.timeout_total.load(Ordering::Relaxed),
        state.request_metrics.conflict_total.load(Ordering::Relaxed),
    );
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

async fn list_indexes(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "indexes": state.engine.index_info() }))
}

async fn create_index(
    State(state): State<Arc<AppState>>,
    request: Result<Json<IndexRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    manage_index(state, request, true).await
}

async fn drop_index(
    State(state): State<Arc<AppState>>,
    request: Result<Json<IndexRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    manage_index(state, request, false).await
}

async fn manage_index(
    state: Arc<AppState>,
    request: Result<Json<IndexRequest>, JsonRejection>,
    create: bool,
) -> Result<impl IntoResponse, ApiError> {
    let Json(request) = decode_json(request)?;
    let version = state
        .engine
        .manage_index(request, create, Instant::now() + state.request_timeout)
        .await?;
    Ok(Json(serde_json::json!({
        "status": if create { "created" } else { "deleted" },
        "generation": version.global_etag(),
    })))
}

async fn quiver_status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let metrics = state.engine.metrics();
    Json(serde_json::json!({ "status": metrics.warmup_state.as_str() }))
}

async fn build_quiver(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let published = state
        .engine
        .build_quiver(Instant::now() + state.request_timeout)
        .await?;
    Ok(Json(serde_json::json!({ "published": published })))
}

const MAX_IMPORT_RECORDS: usize = 10_000;
const MAX_IMPORT_LINE_BYTES: usize = 1024 * 1024;

fn decode_ndjson<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<Vec<T>, ApiError> {
    let mut records = Vec::new();
    for (line_index, line) in body.split(|byte| *byte == b'\n').enumerate() {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_IMPORT_LINE_BYTES {
            return Err(ApiError::payload_too_large());
        }
        if records.len() >= MAX_IMPORT_RECORDS {
            return Err(ApiError::invalid_request(
                "批量导入记录数超过上限 (Bulk import record count exceeds limit)",
            ));
        }
        records.push(serde_json::from_slice(line).map_err(|error| {
            ApiError::invalid_request(format!(
                "NDJSON 第 {} 行无效 (Invalid NDJSON line {}): {error}",
                line_index + 1,
                line_index + 1
            ))
        })?);
    }
    if records.is_empty() {
        return Err(ApiError::invalid_request(
            "NDJSON 导入不得为空 (NDJSON import must not be empty)",
        ));
    }
    Ok(records)
}

async fn import_nodes(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<Json<TransactionResponse>, ApiError> {
    let records: Vec<BulkNodeRecord> = decode_ndjson(&body)?;
    let operations = records
        .into_iter()
        .map(|record| TransactionOperation::Insert {
            id: record.id,
            vector: record.vector,
            payload: record.payload,
        })
        .collect();
    import_operations(state, operations).await
}

async fn import_edges(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<Json<TransactionResponse>, ApiError> {
    let records: Vec<BulkEdgeRecord> = decode_ndjson(&body)?;
    let operations = records
        .into_iter()
        .map(|record| TransactionOperation::Link {
            source: record.source,
            target: record.target,
            label: record.label,
            weight: record.weight,
        })
        .collect();
    import_operations(state, operations).await
}

async fn import_operations(
    state: Arc<AppState>,
    operations: Vec<TransactionOperation>,
) -> Result<Json<TransactionResponse>, ApiError> {
    let result = state
        .engine
        .transaction(
            TransactionRequest {
                expected_generation: None,
                expected_nodes: Default::default(),
                expected_edges: Vec::new(),
                operations,
            },
            None,
            Instant::now() + state.request_timeout,
        )
        .await?;
    Ok(Json(TransactionResponse {
        created_ids: result.created_ids,
        generation: result.version.global_etag(),
        replayed: result.replayed,
    }))
}

#[derive(Debug, Deserialize)]
struct VectorSearchQuery {
    #[serde(default = "default_top_k")]
    top_k: usize,
    #[serde(default)]
    recall_k: usize,
    #[serde(default)]
    rerank_k: usize,
    #[serde(default = "default_min_score")]
    min_score: f32,
    #[serde(default)]
    force_brute_force: bool,
}

fn default_min_score() -> f32 {
    -1.0
}

fn default_top_k() -> usize {
    10
}

async fn prepare(
    State(state): State<Arc<AppState>>,
    request: Result<Json<PrepareRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(request) = decode_json(request)?;
    if request.query.trim().is_empty() {
        return Err(ApiError::invalid_request(
            "query 不得为空 (query must not be empty)",
        ));
    }
    let (id, parameters) = state.engine.prepare(request.query.trim())?;
    Ok(Json(serde_json::json!({
        "preparedId": id,
        "parameters": parameters,
    })))
}

async fn execute_prepared(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    request: Result<Json<ExecutePreparedRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = decode_json(request)?;
    let started = std::time::Instant::now();
    let (rows, version) = state
        .engine
        .execute_prepared(
            &id,
            request.parameters.into_iter().collect(),
            started + state.request_timeout,
        )
        .await?;
    query_response(
        rows,
        version.global_etag(),
        &headers,
        request.profile,
        true,
        started,
        None,
    )
}

async fn search_vector(
    State(state): State<Arc<AppState>>,
    Query(query): Query<VectorSearchQuery>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, ApiError> {
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/vnd.triviumdb.vector+f32")
    {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "UNSUPPORTED_MEDIA_TYPE",
            "媒体类型不受支持 (Unsupported media type)",
            "二进制向量必须使用 application/vnd.triviumdb.vector+f32 (Binary vectors must use application/vnd.triviumdb.vector+f32)",
            false,
        ));
    }
    const MAX_SEARCH_CANDIDATES: usize = 100_000;
    if query.top_k == 0
        || query.top_k > 10_000
        || query.recall_k > MAX_SEARCH_CANDIDATES
        || query.rerank_k > MAX_SEARCH_CANDIDATES
        || !query.min_score.is_finite()
    {
        return Err(ApiError::invalid_request(
            "搜索参数超出服务端上限或包含非有限值 (Search parameters exceed server limits or contain non-finite values)",
        ));
    }
    if body.is_empty() || !body.len().is_multiple_of(std::mem::size_of::<f32>()) {
        return Err(ApiError::invalid_request(
            "二进制向量必须是非空 little-endian f32 数组 (Binary vector must be a non-empty little-endian f32 array)",
        ));
    }
    let vector = body
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect::<Vec<_>>();
    if vector.iter().any(|value| !value.is_finite()) {
        return Err(ApiError::invalid_request(
            "查询向量包含 NaN 或 Infinity (Query vector contains NaN or Infinity)",
        ));
    }
    let search_config = triviumdb::database::SearchConfig {
        top_k: query.top_k,
        recall_k: query.recall_k,
        rerank_k: query.rerank_k,
        min_score: query.min_score,
        force_brute_force: query.force_brute_force,
        ..Default::default()
    };
    let (hits, version) = state
        .engine
        .search_vector(
            vector,
            search_config,
            std::time::Instant::now() + state.request_timeout,
        )
        .await?;
    let etag = version.global_etag();
    let mut response = Json(serde_json::json!({
        "hits": hits.into_iter().map(|hit| serde_json::json!({
            "id": hit.id.to_string(),
            "score": hit.score,
            "payload": hit.payload,
        })).collect::<Vec<_>>(),
        "generation": etag,
    }))
    .into_response();
    insert_etag(response.headers_mut(), &etag)?;
    Ok(response)
}

async fn tql(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Result<Json<TqlRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(request) = decode_json(request)?;
    let query = request.query.trim();
    if query.is_empty() {
        return Err(ApiError::invalid_request(
            "query 不得为空 (query must not be empty)",
        ));
    }
    let deadline = std::time::Instant::now() + state.request_timeout;
    if request.mutation {
        let result = state
            .engine
            .mutate(
                query.to_owned(),
                optional_header(&headers, header::IF_MATCH)?,
                optional_header_name(&headers, "idempotency-key")?,
                deadline,
            )
            .await?;
        let mut response = Json(serde_json::json!({
            "affected": result.mutation.affected,
            "createdIds": result.mutation.created_ids,
            "generation": result.version.global_etag(),
            "replayed": result.replayed,
        }))
        .into_response();
        insert_etag(response.headers_mut(), &result.version.global_etag())?;
        Ok(response)
    } else {
        let started = std::time::Instant::now();
        let (rows, version) = state.engine.query(query.to_owned(), deadline).await?;
        query_response(
            rows,
            version.global_etag(),
            &headers,
            request.profile,
            false,
            started,
            Some((query, state.engine.index_fields())),
        )
    }
}

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

fn request_profile(started: Instant, prepared_cache_hit: bool) -> serde_json::Value {
    let (queue_wait_micros, execution_micros) = engine::REQUEST_TELEMETRY
        .try_with(|telemetry| {
            (
                telemetry.queue_wait().map(|value| value.as_micros()),
                telemetry.execution().map(|value| value.as_micros()),
            )
        })
        .unwrap_or((None, None));
    serde_json::json!({
        "elapsedMicros": started.elapsed().as_micros(),
        "queueWaitMicros": queue_wait_micros,
        "executionMicros": execution_micros,
        "preparedCacheHit": prepared_cache_hit,
    })
}

fn query_response(
    rows: engine::QueryRows,
    etag: String,
    headers: &HeaderMap,
    profile: bool,
    prepared_cache_hit: bool,
    started: std::time::Instant,
    advice_context: Option<(&str, Vec<String>)>,
) -> Result<Response, ApiError> {
    let profile_value = profile.then(|| request_profile(started, prepared_cache_hit));
    let index_advice =
        advice_context.map_or_else(Vec::new, |(query, indexed)| index_advice(query, &indexed));
    let wants_ndjson = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|item| item.trim() == "application/x-ndjson")
        });
    if wants_ndjson {
        let row_count = rows.len();
        let stream_etag = etag.clone();
        let stream = async_stream::stream! {
            yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
                format!("{}\n", serde_json::json!({"type": "meta", "generation": stream_etag}))
            ));
            for row in rows {
                let line = match encode_row(row) {
                    Ok(row) => serde_json::json!({"type": "row", "row": row}),
                    Err(error) => serde_json::json!({"type": "error", "detail": error.to_string()}),
                };
                yield Ok(bytes::Bytes::from(format!("{line}\n")));
            }
            let summary = serde_json::json!({
                "type": "summary",
                "rowCount": row_count,
                "profile": profile_value,
                "indexAdvice": index_advice,
            });
            yield Ok(bytes::Bytes::from(format!("{summary}\n")));
        };
        let mut response = axum::body::Body::from_stream(stream).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-ndjson; charset=utf-8"),
        );
        insert_etag(response.headers_mut(), &etag)?;
        return Ok(response);
    }

    let encoded = encode_rows(rows, etag.clone())?;
    let mut value =
        serde_json::to_value(encoded).map_err(|error| ApiError::internal(error.to_string()))?;
    if let Some(object) = value.as_object_mut() {
        if let Some(profile) = profile_value {
            object.insert("profile".into(), profile);
        }
        object.insert("indexAdvice".into(), serde_json::json!(index_advice));
    }
    let encoded_bytes =
        serde_json::to_vec(&value).map_err(|error| ApiError::internal(error.to_string()))?;
    if encoded_bytes.len() > MAX_RESPONSE_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "RESPONSE_TOO_LARGE",
            "响应体超过上限 (Response too large)",
            "请使用属性投影、LIMIT 或 NDJSON 流式响应 (Use property projection, LIMIT, or NDJSON streaming)",
            false,
        ));
    }
    let mut response = Response::new(Body::from(encoded_bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    insert_etag(response.headers_mut(), &etag)?;
    Ok(response)
}

fn index_advice(query: &str, indexed_fields: &[String]) -> Vec<serde_json::Value> {
    let Some(start) = query.find('{') else {
        return Vec::new();
    };
    let Some(end) = query[start + 1..].find([':', '}']) else {
        return Vec::new();
    };
    let field = query[start + 1..start + 1 + end].trim();
    if field.is_empty()
        || field.starts_with('$')
        || indexed_fields.iter().any(|indexed| indexed == field)
    {
        return Vec::new();
    }
    let kind = if query.contains("$gt")
        || query.contains("$gte")
        || query.contains("$lt")
        || query.contains("$lte")
        || query.contains("ORDER BY")
    {
        "ordered"
    } else {
        "hash"
    };
    vec![serde_json::json!({
        "kind": kind,
        "fields": [field],
        "reason": "查询使用未索引属性过滤 (Query filters on an unindexed property)",
    })]
}

async fn get_node(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u64>,
) -> Result<impl IntoResponse, ApiError> {
    let result = state
        .engine
        .get_node(id, std::time::Instant::now() + state.request_timeout)
        .await?;
    let node_etag = result.version.node_etag(id, result.node_version);
    let edge_versions = result
        .edge_versions
        .into_iter()
        .map(|(target, label, etag)| {
            serde_json::json!({
                "targetId": target.to_string(),
                "label": label,
                "etag": etag,
            })
        })
        .collect::<Vec<_>>();
    let mut response = Json(serde_json::json!({
        "node": encode_node(result.node),
        "generation": result.version.global_etag(),
        "version": node_etag,
        "edgeVersions": edge_versions,
    }))
    .into_response();
    insert_etag(response.headers_mut(), &node_etag)?;
    Ok(response)
}

async fn indexed_lookup(
    State(state): State<Arc<AppState>>,
    request: Result<Json<IndexedLookupRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(request) = decode_json(request)?;
    let ids = state
        .engine
        .indexed_lookup(request, Instant::now() + state.request_timeout)
        .await?;
    Ok(Json(serde_json::json!({ "ids": ids, "count": ids.len() })))
}

async fn substring_lookup(
    State(state): State<Arc<AppState>>,
    request: Result<Json<SubstringLookupRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(request) = decode_json(request)?;
    let ids = state
        .engine
        .substring_lookup(request, Instant::now() + state.request_timeout)
        .await?;
    Ok(Json(serde_json::json!({ "ids": ids, "count": ids.len() })))
}

async fn compare_and_set_payload_field(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u64>,
    request: Result<Json<ConditionalMutationRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(request) = decode_json(request)?;
    let result = state
        .engine
        .conditional_mutation(id, request, Instant::now() + state.request_timeout)
        .await?;
    let etag = result.version.global_etag();
    let mut response = Json(serde_json::json!({
        "affected": result.mutation.affected,
        "generation": etag,
    }))
    .into_response();
    insert_etag(response.headers_mut(), &result.version.global_etag())?;
    Ok(response)
}

async fn delete_many(
    State(state): State<Arc<AppState>>,
    request: Result<Json<DeleteManyRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(request) = decode_json(request)?;
    let result = state
        .engine
        .delete_many(request, Instant::now() + state.request_timeout)
        .await?;
    let etag = result.version.global_etag();
    let mut response = Json(serde_json::json!({
        "affected": result.mutation.affected,
        "generation": etag,
    }))
    .into_response();
    insert_etag(response.headers_mut(), &result.version.global_etag())?;
    Ok(response)
}

async fn transaction(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Result<Json<TransactionRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(mut request) = decode_json(request)?;
    if request.operations.is_empty() {
        return Err(ApiError::invalid_request(
            "事务至少需要一个操作 (A transaction requires at least one operation)",
        ));
    }
    let header_generation = optional_header(&headers, header::IF_MATCH)?;
    match (&request.expected_generation, header_generation) {
        (Some(body), Some(header)) if body != &header => {
            return Err(ApiError::invalid_request(
                "If-Match 与 expectedGeneration 必须一致 (If-Match and expectedGeneration must match)",
            ));
        }
        (None, header) => request.expected_generation = header,
        _ => {}
    }
    let result = state
        .engine
        .transaction(
            request,
            optional_header_name(&headers, "idempotency-key")?,
            std::time::Instant::now() + state.request_timeout,
        )
        .await?;
    let etag = result.version.global_etag();
    let mut response = Json(TransactionResponse {
        created_ids: result.created_ids,
        generation: etag.clone(),
        replayed: result.replayed,
    })
    .into_response();
    insert_etag(response.headers_mut(), &etag)?;
    Ok(response)
}

fn decode_json<T>(request: Result<Json<T>, JsonRejection>) -> Result<Json<T>, ApiError> {
    request.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::payload_too_large()
        } else {
            ApiError::invalid_request(format!(
                "JSON 请求体无效 (Invalid JSON request body): {error}"
            ))
        }
    })
}

fn optional_header(
    headers: &HeaderMap,
    name: header::HeaderName,
) -> Result<Option<String>, ApiError> {
    let mut values = headers.get_all(&name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(ApiError::invalid_request(format!(
            "Header {} 不得重复 (Header {} must not be repeated)",
            name.as_str(),
            name.as_str()
        )));
    }
    let value = value.to_str().map(str::to_owned).map_err(|_| {
        ApiError::invalid_request("HTTP Header 不是有效 ASCII (HTTP header is not valid ASCII)")
    })?;
    if name == header::IF_MATCH && !is_strong_etag(&value) {
        return Err(ApiError::invalid_request(
            "If-Match 必须包含一个强 ETag (If-Match must contain one strong ETag)",
        ));
    }
    Ok(Some(value))
}

fn optional_header_name(
    headers: &HeaderMap,
    name: &'static str,
) -> Result<Option<String>, ApiError> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(ApiError::invalid_request(format!(
            "Header {name} 不得重复 (Header {name} must not be repeated)"
        )));
    }
    value.to_str().map(str::to_owned).map(Some).map_err(|_| {
        ApiError::invalid_request("HTTP Header 不是有效 ASCII (HTTP header is not valid ASCII)")
    })
}

fn is_strong_etag(value: &str) -> bool {
    value.len() >= 2
        && value.starts_with('"')
        && value.ends_with('"')
        && !value[1..value.len() - 1]
            .bytes()
            .any(|byte| byte == b'"' || byte < 0x21 || byte == 0x7f)
}

fn insert_etag(headers: &mut HeaderMap, etag: &str) -> Result<(), ApiError> {
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(etag)
            .map_err(|_| ApiError::internal("生成了非法 ETag (Generated an invalid ETag)"))?,
    );
    Ok(())
}

async fn access_log(
    State(state): State<Arc<AppState>>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let request_id_header = HeaderName::from_static("x-request-id");
    let request_id = request
        .headers()
        .get(&request_id_header)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let context = RequestContext::new(request_id);
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    request.extensions_mut().insert(context.clone());

    let telemetry = context.telemetry.clone();
    let mut response = engine::REQUEST_TELEMETRY
        .scope(telemetry, next.run(request))
        .await;
    let status = response.status();
    match response.extensions().get::<protocol::ErrorMetric>() {
        Some(protocol::ErrorMetric::Timeout) => {
            state
                .request_metrics
                .timeout_total
                .fetch_add(1, Ordering::Relaxed);
        }
        Some(protocol::ErrorMetric::OccConflict) => {
            state
                .request_metrics
                .conflict_total
                .fetch_add(1, Ordering::Relaxed);
        }
        None => {}
    }
    if let Ok(value) = HeaderValue::from_str(&context.id) {
        response.headers_mut().insert(request_id_header, value);
    }

    let (parts, body) = response.into_parts();
    let stream = body.into_data_stream();
    let guard = AccessLogGuard {
        context,
        method: method.to_string(),
        path,
        status: status.as_u16(),
        response_bytes: 0,
        completed: false,
        cancelled_total: state.request_metrics.cancelled_total.clone(),
    };
    let stream = async_stream::stream! {
        let mut guard = guard;
        futures_util::pin_mut!(stream);
        while let Some(item) = futures_util::StreamExt::next(&mut stream).await {
            if let Ok(bytes) = &item {
                guard.response_bytes = guard.response_bytes.saturating_add(bytes.len());
            }
            yield item;
        }
        guard.completed = true;
    };
    Response::from_parts(parts, Body::from_stream(stream))
}

struct AccessLogGuard {
    context: RequestContext,
    method: String,
    path: String,
    status: u16,
    response_bytes: usize,
    completed: bool,
    cancelled_total: Arc<AtomicU64>,
}

impl Drop for AccessLogGuard {
    fn drop(&mut self) {
        let cancelled = !self.completed;
        if cancelled {
            self.cancelled_total.fetch_add(1, Ordering::Relaxed);
        }
        let queue_wait_micros = self
            .context
            .telemetry
            .queue_wait()
            .map(|value| value.as_micros());
        let execution_micros = self
            .context
            .telemetry
            .execution()
            .map(|value| value.as_micros());
        tracing::info!(
            request_id = %self.context.id,
            method = %self.method,
            path = %self.path,
            status = self.status,
            elapsed_micros = self.context.started.elapsed().as_micros(),
            queue_wait_micros = ?queue_wait_micros,
            execution_micros = ?execution_micros,
            response_bytes = self.response_bytes,
            cancelled,
            "HTTP 请求完成 (HTTP request completed)"
        );
    }
}

async fn map_body_limit_error(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let response = next.run(request).await;
    if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::payload_too_large().into_response()
    } else {
        response
    }
}

async fn not_found() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "ROUTE_NOT_FOUND",
        "接口不存在 (Route not found)",
        "请求的 HTTP 接口不存在 (The requested HTTP route does not exist)",
        false,
    )
}
