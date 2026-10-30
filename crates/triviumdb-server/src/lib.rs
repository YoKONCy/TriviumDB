//! TriviumDB Server HTTP 应用：并发读、Writer Actor、幂等写与 OCC。

pub mod console;
mod engine;
mod protocol;

use axum::{
    Json, Router,
    body::Body,
    extract::{
        DefaultBodyLimit, FromRequestParts, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use engine::{EngineConfig, EngineHandle};
use protocol::{
    ApiError, BatchGetRequest, BulkEdgeRecord, BulkNodeRecord, ConditionalMutationRequest,
    DeleteManyRequest, ExecutePreparedRequest, HealthDetailsResponse, HealthResponse, IndexRequest,
    IndexedLookupRequest, PrepareRequest, SubstringLookupRequest, TqlRequest, TransactionOperation,
    TransactionRequest, TransactionResponse, encode_node, encode_row, encode_rows,
    projection_columns,
};
use serde::Deserialize;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::{
    CompressionLayer,
    predicate::{DefaultPredicate, Predicate},
};
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NeighborsQuery {
    #[serde(default = "default_graph_depth")]
    depth: usize,
    #[serde(default = "default_graph_limit")]
    limit: usize,
    #[serde(default)]
    labels: Option<String>,
    #[serde(default)]
    types: Option<String>,
    #[serde(default)]
    direction: GraphDirection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EdgesQuery {
    #[serde(default)]
    direction: GraphDirection,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_graph_limit")]
    limit: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PathsQuery {
    #[serde(default = "default_path_hops")]
    max_hops: usize,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum GraphDirection {
    #[default]
    Out,
    In,
    Both,
}

fn default_graph_depth() -> usize {
    1
}
fn default_graph_limit() -> usize {
    1_000
}
fn default_path_hops() -> usize {
    4
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
        .route("/", get(webui_index))
        .route("/ui", get(webui_index))
        .route("/ui/", get(webui_index))
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
        .route("/v1/nodes/batch-get", post(batch_get_nodes))
        .route("/v1/nodes/{id}/neighbors", get(get_neighbors))
        .route("/v1/nodes/{id}/edges", get(get_edges))
        .route("/v1/nodes/{from}/paths/to/{to}", get(get_paths))
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
        // 最外层：按 Accept-Encoding 压缩（HTML ≈780KB，弱网首屏差距明显）。
        // 304 例外：RFC 9110 §15.4.5 规定 304 不得有响应体，而压缩层会把空体
        // 包装成 gzip 帧（实测 20 字节）并附带 Content-Encoding，故在谓词里排除。
        .layer(
            CompressionLayer::new().compress_when(DefaultPredicate::new().and(
                |status: StatusCode,
                 _version: axum::http::Version,
                 _headers: &HeaderMap,
                 _extensions: &axum::http::Extensions| {
                    status != StatusCode::NOT_MODIFIED
                },
            )),
        )
        .with_state(state))
}

const WEBUI_HTML: &str = include_str!("../web/index.html");

/// 导出现有控制台资源；传输接入与容器挂载由宿主及前端适配器负责。
pub fn console_html() -> &'static str {
    WEBUI_HTML
}

/// 内嵌页面内容指纹（构建期固定、进程内一次性计算）。
/// 用弱 ETag：同一份内容可能按 gzip/identity 两种编码返回，语义等价。
fn webui_etag() -> &'static str {
    static ETAG: OnceLock<String> = OnceLock::new();
    ETAG.get_or_init(|| {
        let mut hasher = DefaultHasher::new();
        WEBUI_HTML.hash(&mut hasher);
        format!("W/\"{:016x}-{}\"", hasher.finish(), WEBUI_HTML.len())
    })
}

async fn webui_index(headers: HeaderMap) -> Response {
    let etag = webui_etag();
    // RFC 9110 §13.1.2：If-None-Match 必须使用弱比较（忽略 W/ 前缀，只比 opaque-tag），
    // 否则浏览器/代理把同一内容的强形式 "tag" 回传时会错误地命中 200。
    let not_modified = headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| if_none_match_matches(value, etag));
    if not_modified {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Ok(value) = HeaderValue::from_str(etag) {
            response.headers_mut().insert(header::ETAG, value);
        }
        // 304 必须带上与 200 相同的缓存指令，否则中间缓存可能把「未修改」当成可长期缓存
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        return response;
    }
    let mut response = WEBUI_HTML.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    // 页面在编译期内嵌，升级 server 后必须让浏览器重新拉取，避免旧 UI 与新接口不匹配；
    // 带 ETag 后重复加载走 304，不再每次重传整页
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
    response
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

async fn list_indexes(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        serde_json::json!({ "indexes": state.engine.index_info()? }),
    ))
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
    ApiQuery(query): ApiQuery<VectorSearchQuery>,
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
    // mutation 标志是「允许写入」，不是「强制走写路径」：读语句即使带了 mutation=true
    // 也必须走读查询。否则 tql_mut(read query) 会以 500「API 已移除」返回，
    // 而 UI 的写入模式复选框是粘性的，用户写过一次 CREATE 后所有查询都会 500。
    let read_statement = matches!(
        triviumdb::query::tql_parser::parse_tql_statement(query),
        Ok(triviumdb::query::tql_ast::TqlStatement::Query(_))
    );
    if request.mutation && !read_statement {
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
            Some((query, state.engine.index_fields()?)),
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
        let columns = projection_columns(&rows);
        let stream_etag = etag.clone();
        let stream = async_stream::stream! {
            yield Ok::<_, std::convert::Infallible>(bytes::Bytes::from(
                format!("{}\n", serde_json::json!({
                    "type": "meta",
                    "generation": stream_etag,
                    "columns": columns,
                }))
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

fn reachability_direction(
    direction: GraphDirection,
) -> triviumdb::graph::reachability::ReachabilityDirection {
    match direction {
        GraphDirection::Out => triviumdb::graph::reachability::ReachabilityDirection::Outgoing,
        GraphDirection::In => triviumdb::graph::reachability::ReachabilityDirection::Incoming,
        GraphDirection::Both => triviumdb::graph::reachability::ReachabilityDirection::Both,
    }
}

fn graph_config(
    depth: usize,
    limit: usize,
    labels: Option<String>,
    direction: GraphDirection,
) -> Result<triviumdb::graph::reachability::ReachabilityConfig, ApiError> {
    if !(1..=3).contains(&depth) || !(1..=10_000).contains(&limit) {
        return Err(ApiError::invalid_request(
            "depth 必须为 1..=3，limit 必须为 1..=10000 (depth must be 1..=3 and limit must be 1..=10000)",
        ));
    }
    let labels = labels.map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect()
    });
    Ok(triviumdb::graph::reachability::ReachabilityConfig {
        min_depth: 1,
        max_depth: depth,
        labels,
        direction: reachability_direction(direction),
        max_visited_nodes: limit.saturating_add(1),
        max_results: limit,
        max_edges: limit.saturating_mul(16).max(1),
        max_frontier_size: limit,
        exhaustion_policy: triviumdb::graph::budget::BudgetExhaustionPolicy::Partial,
    })
}

async fn get_neighbors(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u64>,
    ApiQuery(query): ApiQuery<NeighborsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let config = graph_config(query.depth, query.limit, query.labels, query.direction)?;
    let (mut result, version) = state
        .engine
        .query_subgraph(id, config, Instant::now() + state.request_timeout)
        .await?;
    if let Some(types) = query.types {
        let types = types
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<std::collections::HashSet<_>>();
        let retained = result
            .nodes
            .iter()
            .filter(|node| {
                node.id == id
                    || node
                        .payload
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|value| types.contains(value))
            })
            .map(|node| node.id)
            .collect::<std::collections::HashSet<_>>();
        result.nodes.retain(|node| retained.contains(&node.id));
        result.edges.retain(|edge| {
            retained.contains(&edge.source_id) && retained.contains(&edge.target_id)
        });
    }
    Ok(Json(serde_json::json!({
        "nodes": result.nodes.into_iter().map(|node| serde_json::json!({
            "type": "node", "id": node.id.to_string(), "payload": node.payload
        })).collect::<Vec<_>>(),
        "edges": result.edges.into_iter().map(encode_graph_edge).collect::<Vec<_>>(),
        "visitedNodes": result.visited_nodes,
        "traversedEdges": result.traversed_edges,
        "truncated": result.truncated,
        "generation": version.global_etag(),
    })))
}

async fn get_edges(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u64>,
    ApiQuery(query): ApiQuery<EdgesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if query.limit == 0 || query.limit > 10_000 {
        return Err(ApiError::invalid_request(
            "limit 必须为 1..=10000 (limit must be 1..=10000)",
        ));
    }
    let (edges, version) = state
        .engine
        .list_edges(
            id,
            reachability_direction(query.direction),
            query.label,
            Instant::now() + state.request_timeout,
        )
        .await?;
    let total = edges.len();
    let edges = edges
        .into_iter()
        .skip(query.offset)
        .take(query.limit)
        .map(encode_graph_edge)
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!({
        "edges": edges,
        "total": total,
        "offset": query.offset,
        "limit": query.limit,
        "hasMore": query.offset.saturating_add(edges.len()) < total,
        "generation": version.global_etag(),
    })))
}

async fn batch_get_nodes(
    State(state): State<Arc<AppState>>,
    request: Result<Json<BatchGetRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(request) = decode_json(request)?;
    if request.ids.is_empty() || request.ids.len() > 10_000 {
        return Err(ApiError::invalid_request(
            "ids 数量必须为 1..=10000 (ids count must be 1..=10000)",
        ));
    }
    let requested = request.ids.clone();
    let (nodes, version) = state
        .engine
        .batch_get_nodes(request.ids, Instant::now() + state.request_timeout)
        .await?;
    let found = nodes
        .iter()
        .map(|node| node.id)
        .collect::<std::collections::HashSet<_>>();
    let missing_ids = requested
        .into_iter()
        .filter(|id| !found.contains(id))
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!({
        "nodes": nodes.into_iter().map(encode_node).collect::<Vec<_>>(),
        "missingIds": missing_ids,
        "generation": version.global_etag(),
    })))
}

async fn get_paths(
    State(state): State<Arc<AppState>>,
    Path((from, to)): Path<(u64, u64)>,
    ApiQuery(query): ApiQuery<PathsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if query.max_hops == 0 || query.max_hops > 64 {
        return Err(ApiError::invalid_request(
            "maxHops 必须为 1..=64 (maxHops must be 1..=64)",
        ));
    }
    let config = triviumdb::graph::reachability::ReachabilityConfig {
        min_depth: 1,
        max_depth: query.max_hops,
        labels: query.label.map(|label| vec![label]),
        direction: triviumdb::graph::reachability::ReachabilityDirection::Outgoing,
        ..Default::default()
    };
    let (result, version) = state
        .engine
        .reachable(from, config, Instant::now() + state.request_timeout)
        .await?;
    let paths = result
        .results
        .into_iter()
        .filter(|result| result.target_id == to)
        .map(|result| {
            serde_json::json!({
                "nodes": result.path.into_iter().map(|id| id.to_string()).collect::<Vec<_>>(),
                "edges": result.steps.into_iter().map(|step| serde_json::json!({
                    "type": "edge",
                    "source": step.edge_source.to_string(),
                    "target": step.edge_target.to_string(),
                    "label": step.label,
                    "weight": step.weight,
                    "metadata": step.metadata,
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!({
        "paths": paths,
        "truncated": result.truncated,
        "generation": version.global_etag(),
    })))
}

fn encode_graph_edge(edge: triviumdb::graph::reachability::SubgraphEdge) -> serde_json::Value {
    serde_json::json!({
        "type": "edge",
        "source": edge.source_id.to_string(),
        "target": edge.target_id.to_string(),
        "label": edge.label,
        "weight": edge.weight,
        "metadata": edge.metadata,
    })
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

/// 查询参数提取器：axum 默认把 Query 反序列化失败渲染成 text/plain，
/// 与项目统一的 problem JSON（ApiError）契约不一致。这里只替换拒绝响应的形状，
/// 校验范围、400 状态与业务语义仍由 axum/serde 决定。
struct ApiQuery<T>(T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state)
            .await
            .map_err(query_rejection)?;
        Ok(Self(value))
    }
}

fn query_rejection(rejection: QueryRejection) -> ApiError {
    ApiError::invalid_request(format!("查询参数无效 (Invalid query string): {rejection}"))
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

/// 解析单个 entity-tag，返回去掉 `W/` 前缀后的 opaque-tag（保留引号，便于精确比较）。
/// RFC 9110 §8.8.3：weak = %s"W/"，opaque-tag = DQUOTE *etagc DQUOTE。
fn parse_entity_tag(value: &str) -> Option<&str> {
    let value = value.trim();
    let opaque = value.strip_prefix("W/").unwrap_or(value);
    if opaque.len() >= 2 && opaque.starts_with('"') && opaque.ends_with('"') {
        // etagc 不含 DQUOTE：内部再出现引号说明不是合法的 opaque-tag
        if !opaque[1..opaque.len() - 1].contains('"') {
            return Some(opaque);
        }
    }
    None
}

/// 按逗号切分 If-None-Match 列表；引号内的逗号属于 opaque-tag，不能当分隔符。
fn if_none_match_tags(value: &str) -> Vec<&str> {
    let mut tags = Vec::new();
    let mut start = 0usize;
    let mut in_quotes = false;
    for (index, byte) in value.bytes().enumerate() {
        match byte {
            b'"' => in_quotes = !in_quotes,
            b',' if !in_quotes => {
                tags.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    tags.push(&value[start..]);
    tags
}

/// RFC 9110 §13.1.2 弱比较：`W/"x"` 与 `"x"` 视为同一实体；`*` 匹配任意当前表示。
fn if_none_match_matches(header: &str, current_etag: &str) -> bool {
    if header.trim() == "*" {
        return true;
    }
    let Some(current) = parse_entity_tag(current_etag) else {
        return false;
    };
    if_none_match_tags(header)
        .into_iter()
        .filter_map(parse_entity_tag)
        .any(|candidate| candidate == current)
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

#[cfg(test)]
mod etag_weak_comparison_tests {
    use super::{if_none_match_matches, parse_entity_tag};

    #[test]
    fn 弱比较忽略weak前缀() {
        assert!(if_none_match_matches("W/\"abc\"", "W/\"abc\""));
        assert!(if_none_match_matches("\"abc\"", "W/\"abc\""));
        assert!(if_none_match_matches("W/\"abc\"", "\"abc\""));
        assert!(!if_none_match_matches("\"abcd\"", "W/\"abc\""));
    }

    #[test]
    fn 标签列表与通配符() {
        assert!(if_none_match_matches(
            "\"x\", W/\"abc\", \"y\"",
            "W/\"abc\""
        ));
        assert!(if_none_match_matches("W/\"abc\", \"y\"", "\"abc\""));
        assert!(!if_none_match_matches("\"x\", \"y\"", "W/\"abc\""));
        assert!(if_none_match_matches("*", "W/\"abc\""));
        assert!(if_none_match_matches(" * ", "\"abc\""));
        // 引号内的逗号属于 opaque-tag，不能被当分隔符
        assert!(if_none_match_matches("\"a,b\"", "W/\"a,b\""));
        assert!(!if_none_match_matches("\"a\", \"b\"", "W/\"a,b\""));
    }

    #[test]
    fn 非法标签不参与比较() {
        assert!(!if_none_match_matches("abc", "W/\"abc\""));
        assert!(!if_none_match_matches("\"abc", "W/\"abc\""));
        assert!(!if_none_match_matches("\"a\"b\"", "W/\"a\"b\""));
        assert_eq!(parse_entity_tag(" w/\"abc\" "), None);
        assert_eq!(parse_entity_tag("W/\"abc\""), Some("\"abc\""));
    }
}
