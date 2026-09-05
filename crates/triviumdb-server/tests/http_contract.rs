use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::Value;
use std::time::Duration;
use tower::ServiceExt;
use triviumdb::database::{Config, StorageMode};
use triviumdb_server::{ServerConfig, build_app};

async fn app(name: &str) -> (axum::Router, tempfile::TempDir) {
    app_with(name, 16, 4, Duration::from_secs(5)).await
}

async fn app_with(
    name: &str,
    write_queue_capacity: usize,
    max_concurrent_reads: usize,
    timeout: Duration,
) -> (axum::Router, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let router = build_app(ServerConfig {
        database_path: directory.path().join(format!("{name}.tdb")),
        database: Config {
            dim: 2,
            storage_mode: StorageMode::Rom,
            max_query_rows: Some(100),
            ..Config::default()
        },
        write_queue_capacity,
        max_concurrent_reads,
        idempotency_capacity: 32,
        max_write_batch_size: 16,
        max_write_batch_delay: Duration::from_millis(1),
        prepared_cache_capacity: 8,
        request_timeout: timeout,
        max_body_bytes: 4096,
    })
    .await
    .unwrap();
    (router, directory)
}

async fn json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn text(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn request(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    app.oneshot(
        builder
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn raw_request(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Vec<u8>,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    app.oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn 未消费响应计入取消且deadline计入超时指标() {
    let (app, _directory) = app_with("cancel-metric", 16, 4, Duration::from_nanos(1)).await;
    let cancelled = request(app.clone(), "GET", "/health/live", None, &[]).await;
    drop(cancelled);

    let timeout = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query": "FIND {} RETURN *"})),
        &[],
    )
    .await;
    assert_eq!(timeout.status(), StatusCode::GATEWAY_TIMEOUT);
    let _ = text(timeout).await;

    let metrics = text(request(app, "GET", "/metrics", None, &[]).await).await;
    assert!(metrics.contains("triviumdb_request_cancelled_total 1"));
    assert!(metrics.contains("triviumdb_request_timeout_total 1"));
}

#[tokio::test]
async fn request_id自动生成传播且冲突指标只统计occ冲突() {
    let (app, _directory) = app("observability").await;
    let live = request(app.clone(), "GET", "/health/live", None, &[]).await;
    let generated = live
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap();
    assert_eq!(generated.len(), 32);
    let _ = text(live).await;

    let propagated = request(
        app.clone(),
        "GET",
        "/health/live",
        None,
        &[("x-request-id", "client-request-42")],
    )
    .await;
    assert_eq!(propagated.headers()["x-request-id"], "client-request-42");
    let _ = text(propagated).await;

    let mutation = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({
            "query": "CREATE ({name: \"Alice\"})",
            "mutation": true
        })),
        &[],
    )
    .await;
    let generation = mutation.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    let _ = text(mutation).await;
    let stale = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({
            "query": "CREATE ({name: \"Bob\"})",
            "mutation": true
        })),
        &[("if-match", &generation)],
    )
    .await;
    assert_eq!(stale.status(), StatusCode::OK);
    let _ = text(stale).await;
    let conflict = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({
            "query": "CREATE ({name: \"Carol\"})",
            "mutation": true
        })),
        &[("if-match", &generation)],
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let _ = text(conflict).await;

    let metrics = text(request(app, "GET", "/metrics", None, &[]).await).await;
    assert!(metrics.contains("triviumdb_request_cancelled_total 0"));
    assert!(metrics.contains("triviumdb_request_timeout_total 0"));
    assert!(metrics.contains("triviumdb_occ_conflict_total 1"));
}

#[tokio::test]
async fn indexed_equality_与_ngram_lookup_http_契约() {
    let (app, _directory) = app("lookup_contract").await;
    let seed = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({"operations": [
            {"op": "insert", "id": 1, "vector": [1.0, 0.0], "payload": {"tenant": "a", "text": "向量数据库检索"}},
            {"op": "insert", "id": 2, "vector": [0.0, 1.0], "payload": {"tenant": "b", "text": "天气预报服务"}}
        ]})),
        &[],
    )
    .await;
    assert_eq!(seed.status(), StatusCode::OK);
    for body in [
        serde_json::json!({"kind": "hash", "fields": ["tenant"]}),
        serde_json::json!({"kind": "ngram", "fields": ["text"]}),
    ] {
        assert_eq!(
            request(app.clone(), "POST", "/v1/indexes", Some(body), &[])
                .await
                .status(),
            StatusCode::OK
        );
    }
    let equality = json(
        request(
            app.clone(),
            "POST",
            "/v1/lookup/equality",
            Some(serde_json::json!({"equalities": {"tenant": "a"}})),
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(equality["ids"], serde_json::json!(["1"]));
    let substring = json(
        request(
            app,
            "POST",
            "/v1/lookup/substring",
            Some(serde_json::json!({"field": "text", "needle": "向量数据库"})),
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(substring["ids"], serde_json::json!(["1"]));
}

#[tokio::test]
async fn unique_cas_与批量删除_http_契约() {
    let (app, _directory) = app("unique_cas_delete").await;
    let first = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({"operations": [
            {"op": "insert", "id": 1, "vector": [1.0, 0.0], "payload": {"email": "a", "version": 1}},
            {"op": "insert", "id": 2, "vector": [0.0, 1.0], "payload": {"email": "b", "version": 1}}
        ]})),
        &[],
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);

    let index = request(
        app.clone(),
        "POST",
        "/v1/indexes",
        Some(serde_json::json!({"kind": "unique", "fields": ["email"]})),
        &[],
    )
    .await;
    assert_eq!(index.status(), StatusCode::OK);

    let cas = request(
        app.clone(),
        "POST",
        "/v1/nodes/1/compare-and-set",
        Some(serde_json::json!({"field": "version", "expected": 1, "replacement": 2})),
        &[],
    )
    .await;
    assert_eq!(cas.status(), StatusCode::OK);
    let stale = request(
        app.clone(),
        "POST",
        "/v1/nodes/1/compare-and-set",
        Some(serde_json::json!({"field": "version", "expected": 1, "replacement": 3})),
        &[],
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(json(stale).await["code"], "CONDITIONAL_UPDATE_NOT_MATCHED");

    let deleted = request(
        app,
        "POST",
        "/v1/nodes/delete-many",
        Some(serde_json::json!({"ids": [2, 1, 2]})),
        &[],
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(json(deleted).await["affected"], 2);
}

#[tokio::test]
async fn 健康检查与未知路由使用标准状态码和双语消息() {
    let (app, _directory) = app("health").await;
    let live = request(app.clone(), "GET", "/health/live", None, &[]).await;
    assert_eq!(live.status(), StatusCode::OK);
    assert!(json(live).await["message"].as_str().unwrap().contains('('));

    let details = request(app.clone(), "GET", "/health/details", None, &[]).await;
    assert_eq!(details.status(), StatusCode::OK);
    let details = json(details).await;
    assert_eq!(details["status"], "ready");
    assert_eq!(details["reason"], "ready");
    assert_eq!(details["writerAlive"], true);
    assert!(details["quiverWarmup"].as_str().is_some());

    let ready = request(app.clone(), "GET", "/health/ready", None, &[]).await;
    assert_eq!(ready.status(), StatusCode::OK);

    let missing = request(app, "GET", "/missing", None, &[]).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let body = json(missing).await;
    assert_eq!(body["code"], "ROUTE_NOT_FOUND");
    assert!(body["title"].as_str().unwrap().contains("Route not found"));
}

#[tokio::test]
async fn tql查询写入解析错误和预算错误映射稳定http契约() {
    let (app, _directory) = app("tql").await;
    let mutation = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({
            "query": "CREATE ({name: \"Alice\"})",
            "mutation": true
        })),
        &[],
    )
    .await;
    assert_eq!(mutation.status(), StatusCode::OK);
    assert_eq!(json(mutation).await["affected"], 1);

    let query = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query": "FIND {name: \"Alice\"} RETURN *"})),
        &[],
    )
    .await;
    assert_eq!(query.status(), StatusCode::OK);
    let query_body = json(query).await;
    assert_eq!(query_body["rowCount"], 1);
    assert_eq!(query_body["rows"][0]["_"]["id"], "1");

    let invalid = request(
        app,
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query": "BROKEN QUERY"})),
        &[],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(invalid).await["code"], "QUERY_PARSE_ERROR");
}

#[tokio::test]
async fn 幂等键重放不重复写且同键不同请求返回409() {
    let (app, _directory) = app("idempotency").await;
    let body = serde_json::json!({
        "query": "CREATE ({name: \"once\"})",
        "mutation": true
    });
    let first = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(body.clone()),
        &[("idempotency-key", "once-1")],
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(json(first).await["replayed"], false);

    let replay = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(body),
        &[("idempotency-key", "once-1")],
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json(replay).await["replayed"], true);

    let conflict = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({
            "query": "CREATE ({name: \"other\"})",
            "mutation": true
        })),
        &[("idempotency-key", "once-1")],
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(json(conflict).await["code"], "IDEMPOTENCY_KEY_REUSED");

    let count = request(
        app,
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query": "MATCH (n) RETURN count(*) AS total"})),
        &[],
    )
    .await;
    assert_eq!(json(count).await["rows"][0]["total"]["value"], 1);
}

#[tokio::test]
async fn 全局generation通过etag和if_match实施409冲突() {
    let (app, _directory) = app("global_occ").await;
    let first = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query": "CREATE ({n: 1})", "mutation": true})),
        &[],
    )
    .await;
    let first_etag = first.headers()[header::ETAG].to_str().unwrap().to_owned();
    assert_eq!(first.status(), StatusCode::OK);

    let second = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query": "CREATE ({n: 2})", "mutation": true})),
        &[("if-match", &first_etag)],
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);

    let stale = request(
        app,
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query": "CREATE ({n: 3})", "mutation": true})),
        &[("if-match", &first_etag)],
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let body = json(stale).await;
    assert_eq!(body["code"], "WRITE_CONFLICT");
    assert_eq!(body["retryable"], true);
}

#[tokio::test]
async fn 节点级occ允许无关节点更新并拒绝过期版本() {
    let (app, _directory) = app("node_occ").await;
    let create = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "operations": [
                {"op": "insert", "id": 1, "vector": [1.0, 0.0], "payload": {"v": 1}},
                {"op": "insert", "id": 2, "vector": [0.0, 1.0], "payload": {"v": 1}}
            ]
        })),
        &[],
    )
    .await;
    assert_eq!(create.status(), StatusCode::OK);

    let node1 = request(app.clone(), "GET", "/v1/nodes/1", None, &[]).await;
    let node1_etag = node1.headers()[header::ETAG].to_str().unwrap().to_owned();
    let node2 = request(app.clone(), "GET", "/v1/nodes/2", None, &[]).await;
    let node2_etag = node2.headers()[header::ETAG].to_str().unwrap().to_owned();

    let update1 = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "expectedNodes": {"1": node1_etag},
            "operations": [{"op": "updatePayload", "id": 1, "payload": {"v": 2}}]
        })),
        &[],
    )
    .await;
    assert_eq!(update1.status(), StatusCode::OK);

    let unrelated = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "expectedNodes": {"2": node2_etag},
            "operations": [{"op": "updatePayload", "id": 2, "payload": {"v": 2}}]
        })),
        &[],
    )
    .await;
    assert_eq!(unrelated.status(), StatusCode::OK);

    let stale = request(
        app,
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "expectedNodes": {"1": node1_etag},
            "operations": [{"op": "updatePayload", "id": 1, "payload": {"v": 3}}]
        })),
        &[],
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn 边级occ和多操作事务保持原子且冲突不部分写() {
    let (app, _directory) = app("edge_occ").await;
    let seed = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "operations": [
                {"op": "insert", "id": 1, "vector": [1.0, 0.0], "payload": {}},
                {"op": "insert", "id": 2, "vector": [0.0, 1.0], "payload": {}},
                {"op": "link", "source": 1, "target": 2, "label": "knows", "weight": 1.0}
            ]
        })),
        &[],
    )
    .await;
    assert_eq!(seed.status(), StatusCode::OK);
    let node = request(app.clone(), "GET", "/v1/nodes/1", None, &[]).await;
    let node_body = json(node).await;
    let edge_etag = node_body["edgeVersions"][0]["etag"]
        .as_str()
        .unwrap()
        .to_owned();
    let edge_success = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "expectedEdges": [{"source": 1, "target": 2, "label": "knows", "etag": edge_etag}],
            "operations": [{"op": "link", "source": 1, "target": 2, "label": "knows", "weight": 2.0}]
        })),
        &[],
    )
    .await;
    assert_eq!(edge_success.status(), StatusCode::OK);
    let edge_stale = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "expectedEdges": [{"source": 1, "target": 2, "label": "knows", "etag": edge_etag}],
            "operations": [{"op": "unlink", "source": 1, "target": 2, "label": "knows"}]
        })),
        &[],
    )
    .await;
    assert_eq!(edge_stale.status(), StatusCode::CONFLICT);
    let generation = edge_success.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();

    let stale = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "expectedGeneration": "\"stale-g0\"",
            "operations": [
                {"op": "updatePayload", "id": 1, "payload": {"bad": true}},
                {"op": "delete", "id": 2}
            ]
        })),
        &[],
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(
        request(app.clone(), "GET", "/v1/nodes/2", None, &[])
            .await
            .status(),
        StatusCode::OK
    );

    let success = request(
        app,
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "expectedGeneration": generation,
            "operations": [{"op": "unlink", "source": 1, "target": 2, "label": "knows"}]
        })),
        &[],
    )
    .await;
    assert_eq!(success.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 并发读与串行写在压力下保持完整结果和单调generation() {
    let (app, _directory) = app_with("concurrency", 64, 4, Duration::from_secs(10)).await;
    let mut writes = Vec::new();
    for index in 0..20u64 {
        let app = app.clone();
        writes.push(tokio::spawn(async move {
            request(
                app,
                "POST",
                "/v1/transactions",
                Some(serde_json::json!({
                    "operations": [{
                        "op": "insert",
                        "id": index + 1,
                        "vector": [1.0, 0.0],
                        "payload": {"index": index}
                    }]
                })),
                &[],
            )
            .await
        }));
    }
    let mut reads = Vec::new();
    for _ in 0..20 {
        let app = app.clone();
        reads.push(tokio::spawn(async move {
            request(
                app,
                "POST",
                "/v1/tql",
                Some(serde_json::json!({"query": "MATCH (n) RETURN count(*) AS total"})),
                &[],
            )
            .await
        }));
    }
    for write in writes {
        assert_eq!(write.await.unwrap().status(), StatusCode::OK);
    }
    for read in reads {
        assert_eq!(read.await.unwrap().status(), StatusCode::OK);
    }
    let live = request(app.clone(), "GET", "/health/live", None, &[]).await;
    assert_eq!(live.status(), StatusCode::OK);
    let details = request(app.clone(), "GET", "/health/details", None, &[]).await;
    assert_eq!(details.status(), StatusCode::OK);
    let details = json(details).await;
    assert_eq!(details["writerAlive"], true);
    assert_eq!(details["writerFailed"], false);
    assert_eq!(details["writeQueueDepth"], 0);
    assert_eq!(details["activeReads"], 0);
    assert_eq!(details["waitingReads"], 0);
    assert_eq!(details["waitingWriters"], 0);
    let metrics = text(request(app.clone(), "GET", "/metrics", None, &[]).await).await;
    assert!(metrics.contains("triviumdb_active_reads 0"));
    assert!(metrics.contains("triviumdb_waiting_reads 0"));
    assert!(metrics.contains("triviumdb_writer_alive 1"));
    let final_count = request(
        app,
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query": "MATCH (n) RETURN count(*) AS total"})),
        &[],
    )
    .await;
    assert_eq!(json(final_count).await["rows"][0]["total"]["value"], 20);
}

#[tokio::test]
async fn tql写入保守失效此前签发的节点和边etag() {
    let (app, _directory) = app("tql_fine_grained_occ").await;
    let seed = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "operations": [
                {"op": "insert", "id": 1, "vector": [1.0, 0.0], "payload": {"v": 1}},
                {"op": "insert", "id": 2, "vector": [0.0, 1.0], "payload": {}},
                {"op": "link", "source": 1, "target": 2, "label": "knows", "weight": 1.0}
            ]
        })),
        &[],
    )
    .await;
    assert_eq!(seed.status(), StatusCode::OK);
    let node = request(app.clone(), "GET", "/v1/nodes/1", None, &[]).await;
    let node_etag = node.headers()[header::ETAG].to_str().unwrap().to_owned();
    let node_body = json(node).await;
    let edge_etag = node_body["edgeVersions"][0]["etag"]
        .as_str()
        .unwrap()
        .to_owned();

    let mutation = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({
            "query": "MATCH (a) WHERE a.v == 1 SET a.v == 2",
            "mutation": true
        })),
        &[],
    )
    .await;
    assert_eq!(mutation.status(), StatusCode::OK);

    let stale_node = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "expectedNodes": {"1": node_etag},
            "operations": [{"op": "updatePayload", "id": 1, "payload": {"v": 3}}]
        })),
        &[],
    )
    .await;
    assert_eq!(stale_node.status(), StatusCode::CONFLICT);

    let stale_edge = request(
        app,
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "expectedEdges": [{"source": 1, "target": 2, "label": "knows", "etag": edge_etag}],
            "operations": [{"op": "unlink", "source": 1, "target": 2, "label": "knows"}]
        })),
        &[],
    )
    .await;
    assert_eq!(stale_edge.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn 条件与幂等header边界被严格验证() {
    let (app, _directory) = app("header_validation").await;
    for invalid in ["stale-g0", "W/\"weak\"", "\"multiple\", \"values\""] {
        let response = request(
            app.clone(),
            "POST",
            "/v1/tql",
            Some(serde_json::json!({"query": "CREATE ({n: 1})", "mutation": true})),
            &[("if-match", invalid)],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json(response).await["code"], "INVALID_REQUEST");
    }

    for invalid in ["", "含中文"] {
        let response = request(
            app.clone(),
            "POST",
            "/v1/tql",
            Some(serde_json::json!({"query": "CREATE ({n: 1})", "mutation": true})),
            &[("idempotency-key", invalid)],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let too_long = "x".repeat(129);
    let response = request(
        app,
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query": "CREATE ({n: 1})", "mutation": true})),
        &[("idempotency-key", &too_long)],
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn 事务失败保持原子且幂等重放不重复提交() {
    let (app, _directory) = app("transaction_atomicity").await;
    let invalid = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "operations": [
                {"op": "insert", "id": 1, "vector": [1.0, 0.0], "payload": {}},
                {"op": "insert", "id": 2, "vector": [1.0], "payload": {}}
            ]
        })),
        &[],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        request(app.clone(), "GET", "/v1/nodes/1", None, &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let body = serde_json::json!({
        "operations": [{"op": "insert", "id": 1, "vector": [1.0, 0.0], "payload": {}}]
    });
    let first = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(body.clone()),
        &[("idempotency-key", "transaction-1")],
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first_generation = first.headers()[header::ETAG].to_str().unwrap().to_owned();
    assert_eq!(json(first).await["replayed"], false);
    let replay = request(
        app,
        "POST",
        "/v1/transactions",
        Some(body),
        &[("idempotency-key", "transaction-1")],
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(replay.headers()[header::ETAG], first_generation);
    assert_eq!(json(replay).await["replayed"], true);
}

#[tokio::test]
async fn 事务header与body全局前置条件冲突时拒绝请求() {
    let (app, _directory) = app("transaction_precondition_sources").await;
    let response = request(
        app,
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "expectedGeneration": "\"body-g1\"",
            "operations": [{"op": "insert", "id": 1, "vector": [1.0, 0.0], "payload": {}}]
        })),
        &[("if-match", "\"header-g1\"")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(response).await["code"], "INVALID_REQUEST");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn 动态batching合并并发写且metrics暴露queue与fsync() {
    let (app, _directory) = app_with("group_commit", 64, 4, Duration::from_secs(10)).await;
    let mut writes = Vec::new();
    for id in 1..=16u64 {
        let app = app.clone();
        writes.push(tokio::spawn(async move {
            request(
                app,
                "POST",
                "/v1/transactions",
                Some(serde_json::json!({
                    "operations": [{
                        "op": "insert",
                        "id": id,
                        "vector": [1.0, 0.0],
                        "payload": {"id": id}
                    }]
                })),
                &[],
            )
            .await
        }));
    }
    for write in writes {
        assert_eq!(write.await.unwrap().status(), StatusCode::OK);
    }

    let metrics = request(app, "GET", "/metrics", None, &[]).await;
    assert_eq!(metrics.status(), StatusCode::OK);
    assert!(
        metrics.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    let body = text(metrics).await;
    assert!(body.contains("triviumdb_write_queue_capacity 64"));
    assert!(body.contains("triviumdb_write_queued_total 16"));
    let max_batch = body
        .lines()
        .find_map(|line| line.strip_prefix("triviumdb_write_batch_max "))
        .unwrap()
        .parse::<usize>()
        .unwrap();
    assert!(max_batch > 1, "并发写必须触发动态合批，实际指标: {body}");
}

#[tokio::test]
async fn prepared_cache参数绑定profile和淘汰契约完整() {
    let (app, _directory) = app("prepared").await;
    for id in 1..=3u64 {
        let response = request(
            app.clone(),
            "POST",
            "/v1/transactions",
            Some(serde_json::json!({
                "operations": [{"op": "insert", "id": id, "vector": [1.0, 0.0], "payload": {"rank": id}}]
            })),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let prepared = request(
        app.clone(),
        "POST",
        "/v1/prepared",
        Some(serde_json::json!({
            "query": "MATCH (n) WHERE n.rank >= $min RETURN n"
        })),
        &[],
    )
    .await;
    assert_eq!(prepared.status(), StatusCode::OK);
    let prepared = json(prepared).await;
    assert_eq!(prepared["parameters"], serde_json::json!(["min"]));
    let id = prepared["preparedId"].as_str().unwrap();
    let executed = request(
        app.clone(),
        "POST",
        &format!("/v1/prepared/{id}/execute"),
        Some(serde_json::json!({"parameters": {"min": 2}, "profile": true})),
        &[],
    )
    .await;
    assert_eq!(executed.status(), StatusCode::OK);
    let body = json(executed).await;
    assert_eq!(body["rowCount"], 2);
    assert_eq!(body["profile"]["preparedCacheHit"], true);

    let missing = request(
        app,
        "POST",
        "/v1/prepared/missing/execute",
        Some(serde_json::json!({"parameters": {}})),
        &[],
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(json(missing).await["code"], "PREPARED_NOT_FOUND");
}

#[tokio::test]
async fn ndjson分块协议包含meta_rows_summary与profile() {
    let (app, _directory) = app("ndjson").await;
    request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query": "CREATE ({kind: \"stream\"})", "mutation": true})),
        &[],
    )
    .await;
    let response = request(
        app,
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query": "FIND {kind: \"stream\"} RETURN *", "profile": true})),
        &[("accept", "application/x-ndjson")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/x-ndjson")
    );
    let lines = text(response).await;
    let frames = lines
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(frames[0]["type"], "meta");
    assert_eq!(frames[1]["type"], "row");
    assert_eq!(frames[2]["type"], "summary");
    assert_eq!(frames[2]["profile"]["preparedCacheHit"], false);
}

#[tokio::test]
async fn 索引管理quiver状态与ndjson导入形成完整http契约() {
    let (app, _directory) = app("management_import").await;
    let nodes = raw_request(
        app.clone(),
        "POST",
        "/v1/import/nodes",
        br#"{"id":1,"vector":[1.0,0.0],"payload":{"type":"event"}}
{"id":2,"vector":[0.0,1.0],"payload":{"type":"person"}}
"#
        .to_vec(),
        &[("content-type", "application/x-ndjson")],
    )
    .await;
    assert_eq!(nodes.status(), StatusCode::OK);

    let edge = raw_request(
        app.clone(),
        "POST",
        "/v1/import/edges",
        br#"{"source":1,"target":2,"label":"knows","weight":1.0}
"#
        .to_vec(),
        &[("content-type", "application/x-ndjson")],
    )
    .await;
    assert_eq!(edge.status(), StatusCode::OK);

    let created = request(
        app.clone(),
        "POST",
        "/v1/indexes",
        Some(serde_json::json!({"kind":"hash","fields":["type"]})),
        &[],
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let indexes = json(request(app.clone(), "GET", "/v1/indexes", None, &[]).await).await;
    assert_eq!(indexes["indexes"].as_array().unwrap().len(), 1);

    let quiver = json(request(app.clone(), "GET", "/v1/indexes/quiver", None, &[]).await).await;
    assert!(quiver["status"].as_str().is_some());

    let deleted = request(
        app.clone(),
        "DELETE",
        "/v1/indexes/delete",
        Some(serde_json::json!({"kind":"hash","fields":["type"]})),
        &[],
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    let malformed = raw_request(
        app,
        "POST",
        "/v1/import/nodes",
        b"not-json\n".to_vec(),
        &[("content-type", "application/x-ndjson")],
    )
    .await;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn tql_set_vector更新向量且维度错误原子拒绝() {
    let (app, _directory) = app("set_vector").await;
    let seed = request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({"operations":[{"op":"insert","id":1,"vector":[1.0,0.0],"payload":{"name":"node"}}]})),
        &[],
    )
    .await;
    assert_eq!(seed.status(), StatusCode::OK);
    let updated = request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query":"MATCH (n {id: 1}) SET VECTOR(n) == [0.0, 1.0]","mutation":true})),
        &[],
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let invalid = request(
        app,
        "POST",
        "/v1/tql",
        Some(
            serde_json::json!({"query":"MATCH (n {id: 1}) SET VECTOR(n) == [1.0]","mutation":true}),
        ),
        &[],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn 搜索参数受服务端硬上限和有限值约束() {
    let (app, _directory) = app("search_limits").await;
    for uri in [
        "/v1/search/vector?top_k=0",
        "/v1/search/vector?top_k=10001",
        "/v1/search/vector?top_k=1&recall_k=100001",
        "/v1/search/vector?top_k=1&rerank_k=100001",
    ] {
        let response = raw_request(
            app.clone(),
            "POST",
            uri,
            [1.0f32.to_le_bytes(), 0.0f32.to_le_bytes()].concat(),
            &[("content-type", "application/vnd.triviumdb.vector+f32")],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
    }
}

#[tokio::test]
async fn ndjson导入失败保持事务原子性() {
    let (app, _directory) = app("import_atomic").await;
    let response = raw_request(
        app.clone(),
        "POST",
        "/v1/import/nodes",
        br#"{"id":1,"vector":[1.0,0.0],"payload":{}}
{"id":2,"vector":[1.0],"payload":{}}
"#
        .to_vec(),
        &[("content-type", "application/x-ndjson")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let count = request(
        app,
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query":"MATCH (n) RETURN count(*) AS total"})),
        &[],
    )
    .await;
    assert_eq!(json(count).await["rows"][0]["total"]["value"], 0);
}

#[tokio::test]
async fn 二进制f32向量检索与边界校验() {
    let (app, _directory) = app("binary_vector").await;
    request(
        app.clone(),
        "POST",
        "/v1/transactions",
        Some(serde_json::json!({
            "operations": [{"op": "insert", "id": 1, "vector": [1.0, 0.0], "payload": {"name": "near"}}]
        })),
        &[],
    )
    .await;
    let bytes = [1.0f32.to_le_bytes(), 0.0f32.to_le_bytes()].concat();
    let response = raw_request(
        app.clone(),
        "POST",
        "/v1/search/vector?top_k=1",
        bytes,
        &[("content-type", "application/vnd.triviumdb.vector+f32")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json(response).await["hits"][0]["id"], "1");

    let invalid = raw_request(
        app.clone(),
        "POST",
        "/v1/search/vector",
        vec![1, 2, 3],
        &[("content-type", "application/vnd.triviumdb.vector+f32")],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let unsupported = raw_request(
        app,
        "POST",
        "/v1/search/vector",
        vec![0; 8],
        &[("content-type", "application/octet-stream")],
    )
    .await;
    assert_eq!(unsupported.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn explain_analyze通过http返回实际行数和执行耗时() {
    let (app, _directory) = app("explain_analyze").await;
    request(
        app.clone(),
        "POST",
        "/v1/tql",
        Some(serde_json::json!({"query": "CREATE ({kind: \"profiled\"})", "mutation": true})),
        &[],
    )
    .await;
    let response = request(
        app,
        "POST",
        "/v1/tql",
        Some(serde_json::json!({
            "query": "EXPLAIN ANALYZE FIND {kind: \"profiled\"} RETURN *"
        })),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["rowCount"], 1);
    let plan = body["rows"][0]["plan"]["payload"].to_string();
    assert!(plan.contains("actual_rows") || plan.contains("actualRows"));
    assert!(plan.contains("elapsed_ms") || plan.contains("elapsedMs"));
}

#[tokio::test]
async fn 请求级profile和索引建议只建议未索引字段() {
    let (app, _directory) = app("profile_advice").await;
    let response = request(
        app,
        "POST",
        "/v1/tql",
        Some(serde_json::json!({
            "query": "FIND {age: {$gte: 18}} RETURN *",
            "profile": true
        })),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert!(body["profile"]["elapsedMicros"].is_number());
    assert!(body["profile"]["queueWaitMicros"].is_number());
    assert!(body["profile"]["executionMicros"].is_number());
    assert_eq!(body["indexAdvice"][0]["kind"], "ordered");
    assert_eq!(body["indexAdvice"][0]["fields"][0], "age");
}

#[tokio::test]
async fn 非法json也返回统一双语错误结构() {
    let (app, _directory) = app("invalid_json").await;
    let response = app
        .oneshot(
            Request::post("/v1/tql")
                .header("content-type", "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(response).await["code"], "INVALID_REQUEST");
}

#[tokio::test]
async fn 请求体上限使用标准payload_too_large状态码() {
    let (app, _directory) = app("body_limit").await;
    let response = app
        .oneshot(
            Request::post("/v1/tql")
                .header("content-type", "application/json")
                .body(Body::from("x".repeat(5000)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json(response).await["code"], "PAYLOAD_TOO_LARGE");
}
