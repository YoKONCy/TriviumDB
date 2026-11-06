//! HTTP 组件只借用宿主实例，验证实际内容、背压、断开和文件锁释放。

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::{Arc, Barrier, Mutex};
use tower::ServiceExt;
use triviumdb::{ConsoleHost, ConsoleOptions, Database, HookContext, SearchConfig, SearchHook};
use triviumdb_server::console::{ConsoleHttpOptions, build_console_app};

fn wire(id: &str, command: Value) -> String {
    json!({"version":1,"requestId":id,"command":command}).to_string()
}
async fn post(app: Router, body: String) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::post("/v1/console")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn setup(path: &str) -> Arc<Mutex<Database<f32>>> {
    let mut db = Database::open(path, 2).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"host":true}))
        .unwrap();
    Arc::new(Mutex::new(db))
}

#[tokio::test]
async fn 请求体期限使用独立错误码并归还并发许可() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("body-deadline.tdb");
    let database = setup(path.to_str().unwrap());
    let before = std::fs::read(format!("{}.wal", path.to_str().unwrap())).unwrap();
    let host = ConsoleHost::attach(
        &database,
        ConsoleOptions {
            query_timeout_ms: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let app = build_console_app(host, ConsoleHttpOptions { max_in_flight: 1 }).unwrap();
    // 永不完成的外部请求体只验证期限契约，第二次请求必须再次取得唯一许可。
    for _ in 0..2 {
        let body = Body::from_stream(futures_util::stream::pending::<
            Result<bytes::Bytes, std::io::Error>,
        >());
        let response = app
            .clone()
            .oneshot(Request::post("/v1/console").body(body).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["error"]["schema_version"], 1);
        assert_eq!(result["error"]["code"], "TDB_QUERY_DEADLINE_EXCEEDED");
        assert_eq!(result["error"]["details"], json!({}));
        assert!(result["error"].get("write").is_none());
    }
    assert_eq!(
        std::fs::read(format!("{}.wal", path.to_str().unwrap())).unwrap(),
        before
    );
    let live = build_console_app(
        ConsoleHost::attach(&database, ConsoleOptions::default()).unwrap(),
        ConsoleHttpOptions::default(),
    )
    .unwrap();
    let (status, result) = post(
        live,
        wire("有效读取", json!({"operation":"getNodes","ids":["1"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        result["data"],
        json!({"nodes":[{"id":"1","vector":[1.,0.],
        "payload":{"host":true},"edges":[]}],"missingIds":[]})
    );
    database.lock().unwrap().close().unwrap();
}

#[tokio::test]
async fn 索引发布失败保留提交阶段并在持有旧router时恢复完整事实() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("index-publication.tdb");
    let path = file.to_str().unwrap();
    let database = setup(path);
    {
        let mut core = database.lock().unwrap();
        core.create_index("host").unwrap();
        core.flush().unwrap();
    }
    let host = ConsoleHost::attach(
        &database,
        ConsoleOptions {
            read_only: false,
            ..Default::default()
        },
    )
    .unwrap();
    let app = build_console_app(host, ConsoleHttpOptions::default()).unwrap();
    // 排他临时目录阻断属性索引发布，故障限定于本用例自己的文件组。
    let obstacle = root.path().join("index-publication.tdb.pidx.tmp");
    std::fs::create_dir(&obstacle).unwrap();
    let (status, body) = post(
        app.clone(),
        wire(
            "publish-index",
            json!({
                "operation":"manageIndex","kind":"unique","fields":["host"],"create":true
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"]["schema_version"], 1);
    assert_eq!(body["error"]["code"], "TDB_IO");
    assert!(
        body["error"]["details"]["io_kind"]
            .as_str()
            .is_some_and(|kind| !kind.is_empty())
    );
    assert_eq!(
        body["error"]["write"],
        json!({"phase":"publish","outcome":"unknown"})
    );
    assert_eq!(
        database.lock().unwrap().node_count().unwrap_err().code(),
        "TDB_WAL_CLOSED"
    );
    std::fs::remove_dir(&obstacle).unwrap();
    drop(database);
    let mut recovered = Database::<f32>::open(path, 2).unwrap();
    for _ in 0..2 {
        assert_eq!(recovered.all_node_ids().unwrap(), vec![1]);
        let node = recovered.get(1).unwrap().unwrap();
        assert_eq!(node.vector, vec![1., 0.]);
        assert_eq!(node.payload, json!({"host":true}));
        assert!(node.edges.is_empty());
        let indexes = recovered.index_info().unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].fields, vec!["host".to_owned()]);
        assert!(!indexes[0].unique);
        recovered.flush().unwrap();
        recovered.close().unwrap();
        recovered = Database::<f32>::open(path, 2).unwrap();
    }
    recovered.close().unwrap();
    drop(app);
}

#[tokio::test]
async fn 嵌入http保留查询阶段及源码位置并隔离拒绝写入() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("diagnostics.tdb");
    let database = setup(file.to_str().unwrap());
    let host = ConsoleHost::attach(
        &database,
        ConsoleOptions {
            read_only: false,
            ..Default::default()
        },
    )
    .unwrap();
    let app = build_console_app(host, ConsoleHttpOptions::default()).unwrap();
    let cases: Value = serde_json::from_str(include_str!(
        "../../../tests/contracts/query_diagnostics.json"
    ))
    .unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let (status, body) = post(app.clone(), wire(case["name"].as_str().unwrap(), json!({
            "operation":if case["mutation"] == true {"mutate"} else {"query"},"query":case["query"]
        }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{case}: {body}");
        let error = &body["error"];
        assert_eq!(error["schema_version"], 1);
        assert_eq!(error["code"], "TDB_QUERY_PARSE");
        assert_eq!(error["stage"], case["stage"], "{case}");
        assert_eq!(error["target"]["query_span"], case["span"], "{case}");
        if case["mutation"] == true {
            assert_eq!(
                error["write"],
                json!({"phase":"validation","outcome":"not_committed"})
            );
        }
        if case["span"].is_null() {
            assert!(error.get("target").is_none());
        }
        assert!(error["details"]["reason"].is_string());
    }
    let cases: Value = serde_json::from_str(include_str!(
        "../../../tests/contracts/prepared_diagnostics.json"
    ))
    .unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let (status, body) = post(
            app.clone(),
            wire(
                case["name"].as_str().unwrap(),
                json!({
                    "operation":"query","query":case["query"],"parameters":case["parameters"]
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{case}");
        let error = &body["error"];
        assert_eq!(error["code"], case["code"]);
        assert_eq!(error["stage"], "bind");
        assert_eq!(error["target"], json!({"parameter":case["parameter"]}));
        if let Some(details) = case.get("details") {
            assert_eq!(&error["details"], details);
        } else {
            assert_eq!(error["details"].as_object().unwrap().len(), 1);
            assert!(!error["details"]["reason"].as_str().unwrap().is_empty());
        }
    }
    let (status, body) = post(app.clone(), wire("write-outcome", json!({
        "operation":"transaction","operations":[{"op":"insert","vector":[1],"payload":{"state":"rejected"}}]
    }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "TDB_DIMENSION_MISMATCH");
    assert_eq!(body["error"]["details"], json!({"expected":2,"got":1}));
    assert_eq!(
        body["error"]["write"],
        json!({"phase":"validation","outcome":"not_committed","transaction_consumed":true})
    );
    database.lock().unwrap().close().unwrap();
    let mut reopened = Database::<f32>::open(file.to_str().unwrap(), 2).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), vec![1]);
    let node = reopened.get(1).unwrap().unwrap();
    assert_eq!(node.vector, vec![1., 0.]);
    assert_eq!(node.payload, json!({"host":true}));
    assert!(node.edges.is_empty());
    reopened.close().unwrap();
}

#[tokio::test]
async fn 嵌入http编辑冲突及原事务确认沿用core契约() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("editing.tdb");
    let database = setup(file.to_str().unwrap());
    let host = ConsoleHost::attach(
        &database,
        ConsoleOptions {
            read_only: false,
            ..Default::default()
        },
    )
    .unwrap();
    let app = build_console_app(host, ConsoleHttpOptions::default()).unwrap();
    let (status, read) = post(
        app.clone(),
        wire(
            "读取",
            json!({"operation":"getNodes","ids":["1"],"withRevision":true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    database
        .lock()
        .unwrap()
        .update_payload(1, json!({"owner":"host"}))
        .unwrap();
    let mut command = json!({"operation":"guardedTransaction","expectedRevision":read["data"]["revision"],"idempotencyKey":"http-edit","operations":[
        {"op":"insert","vector":[0,1],"payload":{"owner":"new"}},
        {"op":"updatePayload","id":"1","payload":{"owner":"console"}}
    ]});
    let (status, rejected) = post(app.clone(), wire("冲突", command.clone())).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(rejected["error"]["code"], "TDB_WRITE_CONFLICT");
    assert_eq!(database.lock().unwrap().node_count().unwrap(), 1);
    let (status, read) = post(
        app.clone(),
        wire(
            "重新读取",
            json!({"operation":"getNodes","ids":["1"],"withRevision":true}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    command["expectedRevision"] = read["data"]["revision"].clone();
    let (status, committed) = post(app.clone(), wire("提交", command.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(committed["data"]["createdIds"], json!(["2"]));
    assert_eq!(committed["data"]["replayed"], false);
    let (status, replayed) = post(app.clone(), wire("重试", command.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        replayed["data"],
        json!({"createdIds":["2"],"replayed":true,"revision":committed["data"]["revision"]})
    );
    command["operation"] = json!("transactionStatus");
    let (status, receipt) = post(app.clone(), wire("确认", command.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        receipt["data"],
        json!({"outcome":"committed","revision":committed["data"]["revision"],"createdIds":["2"]})
    );
    command["operations"][0]["payload"] = json!({"owner":"different"});
    let (status, conflict) = post(app.clone(), wire("错配确认", command)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["error"]["code"], "TDB_IDEMPOTENCY_CONFLICT");
    database.lock().unwrap().close().unwrap();
    let mut reopened = Database::<f32>::open(file.to_str().unwrap(), 2).unwrap();
    assert_eq!(reopened.node_count().unwrap(), 2);
    assert_eq!(
        reopened.get_payload(1).unwrap().unwrap(),
        json!({"owner":"console"})
    );
    assert_eq!(
        reopened.get_payload(2).unwrap().unwrap(),
        json!({"owner":"new"})
    );
    assert_eq!(reopened.get(2).unwrap().unwrap().vector, vec![0., 1.]);
    reopened.close().unwrap();
}

#[tokio::test]
async fn http复用已打开实例且路由存活不延长文件锁() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("宿主.tdb");
    let path = file.to_str().unwrap();
    let database = setup(path);
    let host = ConsoleHost::attach(&database, ConsoleOptions::default()).unwrap();
    let app = build_console_app(host, ConsoleHttpOptions::default()).unwrap();
    let (status, result) = post(
        app.clone(),
        wire("读", json!({"operation":"getNodes","ids":["1"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        result,
        json!({"version":1,"requestId":"读","data":{"nodes":[{"id":"1","vector":[1.0,0.0],"payload":{"host":true},"edges":[]}],"missingIds":[]}})
    );
    let (status, result) = post(
        app.clone(),
        wire(
            "拒绝",
            json!({"operation":"transaction","operations":[{"op":"delete","id":"1"}]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(result["error"]["code"], "TDB_READ_ONLY");
    assert_eq!(
        database.lock().unwrap().get_payload(1).unwrap(),
        Some(json!({"host":true}))
    );
    database.lock().unwrap().close().unwrap();
    let (status, result) = post(app.clone(), wire("已关闭", json!({"operation":"info"}))).await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(result["error"]["code"], "TDB_DATABASE_CLOSED");
    let mut replacement = Database::<f32>::open(path, 2).unwrap();
    assert_eq!(
        replacement.get_payload(1).unwrap(),
        Some(json!({"host":true}))
    );
    replacement.close().unwrap();
    drop(database);
    assert_eq!(
        post(app, wire("宿主消失", json!({"operation":"info"})))
            .await
            .0,
        StatusCode::GONE
    );
}

struct HoldRead {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}
impl SearchHook for HoldRead {
    fn on_pre_search(
        &self,
        _query: &mut Vec<f32>,
        _config: &mut SearchConfig,
        _context: &mut HookContext,
    ) {
        self.entered.wait();
        self.release.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 客户端退出后许可保留到真实任务退出() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("取消.tdb");
    let database = setup(file.to_str().unwrap());
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    database
        .lock()
        .unwrap()
        .set_hook(HoldRead {
            entered: entered.clone(),
            release: release.clone(),
        })
        .unwrap();
    let host = ConsoleHost::attach(&database, ConsoleOptions::default()).unwrap();
    let app = build_console_app(host, ConsoleHttpOptions { max_in_flight: 1 }).unwrap();
    let first = app.clone();
    let running = tokio::spawn(async move {
        post(
            first,
            wire(
                "持有",
                json!({"operation":"search","vector":[1,0],"topK":1}),
            ),
        )
        .await
    });
    entered.wait();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    let (status, error) = post(app.clone(), wire("背压", json!({"operation":"info"}))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["error"]["code"], "TDB_BUSY");
    assert_eq!(
        error["error"]["details"],
        json!({"resource":"console_in_flight"})
    );
    release.wait();
    // 仅对受控读取退出后的背压重试；必须观察真实成功及完整记录，不以时间推断退出。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let result = loop {
        assert!(
            std::time::Instant::now() < deadline,
            "实际请求退出后必须归还许可"
        );
        let (status, result) = post(
            app.clone(),
            wire("完成", json!({"operation":"getNodes","ids":["1"]})),
        )
        .await;
        if status == StatusCode::OK {
            break result;
        }
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(result["error"]["code"], "TDB_BUSY");
        assert_eq!(
            result["error"]["details"],
            json!({"resource":"console_in_flight"})
        );
        tokio::task::yield_now().await;
    };
    assert_eq!(
        result["data"]["nodes"][0],
        json!({"id":"1","vector":[1.0,0.0],"payload":{"host":true},"edges":[]})
    );
    database.lock().unwrap().close().unwrap();
    let mut reopened = Database::<f32>::open(file.to_str().unwrap(), 2).unwrap();
    assert_eq!(reopened.get_payload(1).unwrap(), Some(json!({"host":true})));
    reopened.close().unwrap();
}

#[tokio::test]
async fn 请求体预算拒绝不会修改宿主数据() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("请求预算.tdb");
    let database = setup(file.to_str().unwrap());
    let host = ConsoleHost::attach(
        &database,
        ConsoleOptions {
            max_request_bytes: 128,
            ..Default::default()
        },
    )
    .unwrap();
    let app = build_console_app(host, ConsoleHttpOptions::default()).unwrap();
    let (status, error) = post(app, "x".repeat(129)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error["error"]["code"], "TDB_CONSOLE_REQUEST_TOO_LARGE");
    assert_eq!(
        database.lock().unwrap().get_payload(1).unwrap(),
        Some(json!({"host":true}))
    );
    database.lock().unwrap().close().unwrap();
}

#[tokio::test]
async fn 请求体读取前取得并发许可且退出后归还() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("请求体准入.tdb");
    let database = setup(file.to_str().unwrap());
    let host = ConsoleHost::attach(&database, ConsoleOptions::default()).unwrap();
    let app = build_console_app(host, ConsoleHttpOptions { max_in_flight: 1 }).unwrap();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let body = Body::from_stream(futures_util::stream::once(async move {
        entered.send(()).unwrap();
        std::future::pending::<Result<bytes::Bytes, std::io::Error>>().await
    }));
    let first = app.clone();
    let waiting = tokio::spawn(async move {
        first
            .oneshot(Request::post("/v1/console").body(body).unwrap())
            .await
    });
    ready.await.unwrap();
    let (status, error) = post(app.clone(), wire("并发", json!({"operation":"info"}))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["error"]["code"], "TDB_BUSY");
    assert_eq!(
        error["error"]["details"],
        json!({"resource":"console_in_flight"})
    );
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    let (status, result) = post(
        app,
        wire("归还", json!({"operation":"getNodes","ids":["1"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["data"]["nodes"][0]["payload"], json!({"host":true}));
    database.lock().unwrap().close().unwrap();
}

#[tokio::test]
async fn 已卸载会话在读取请求体前拒绝新请求() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let root = tempfile::tempdir().unwrap();
    let database = setup(root.path().join("卸载入口.tdb").to_str().unwrap());
    let host = ConsoleHost::attach(&database, ConsoleOptions::default()).unwrap();
    host.session().detach();
    let app = build_console_app(host, ConsoleHttpOptions::default()).unwrap();
    let polled = Arc::new(AtomicBool::new(false));
    let body_polled = polled.clone();
    let body = Body::from_stream(futures_util::stream::poll_fn(move |_| {
        body_polled.store(true, Ordering::SeqCst);
        std::task::Poll::Pending::<Option<Result<bytes::Bytes, std::io::Error>>>
    }));
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        app.oneshot(Request::post("/v1/console").body(body).unwrap()),
    )
    .await
    .expect("卸载后不得等待读取请求体")
    .unwrap();
    assert_eq!(response.status(), StatusCode::GONE);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["error"]["code"], "TDB_CONSOLE_SESSION_CLOSED");
    assert!(!polled.load(Ordering::SeqCst));
    assert_eq!(
        database.lock().unwrap().get_payload(1).unwrap(),
        Some(json!({"host":true}))
    );
    database.lock().unwrap().close().unwrap();
}

#[tokio::test]
async fn http错误详情也保留完整高位节点id() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("错误ID.tdb");
    let database = setup(file.to_str().unwrap());
    database
        .lock()
        .unwrap()
        .insert_with_id(9_007_199_254_740_993, &[1., 1.], json!({"原始":true}))
        .unwrap();
    let host = ConsoleHost::attach(
        &database,
        ConsoleOptions {
            read_only: false,
            ..Default::default()
        },
    )
    .unwrap();
    let app = build_console_app(host, ConsoleHttpOptions::default()).unwrap();
    let (status,error)=post(app,wire("重复",json!({"operation":"transaction","operations":[{"op":"insert","id":"9007199254740993","vector":[0,0],"payload":{}}]}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["code"], "TDB_NODE_ALREADY_EXISTS");
    assert_eq!(error["error"]["details"], json!({"id":"9007199254740993"}));
    assert_eq!(
        database
            .lock()
            .unwrap()
            .get_payload(9_007_199_254_740_993)
            .unwrap(),
        Some(json!({"原始":true}))
    );
    database.lock().unwrap().close().unwrap();
}

#[tokio::test]
async fn 继续持有未消费响应也能关闭并移动文件组() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("响应.tdb");
    let database = setup(file.to_str().unwrap());
    let host = ConsoleHost::attach(&database, ConsoleOptions::default()).unwrap();
    let app = build_console_app(host, ConsoleHttpOptions::default()).unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/console")
                .body(Body::from(wire(
                    "持有响应",
                    json!({"operation":"getNodes","ids":["1"]}),
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    database.lock().unwrap().close().unwrap();
    let moved = root.path().join("已移动");
    std::fs::create_dir(&moved).unwrap();
    for entry in std::fs::read_dir(root.path()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::rename(entry.path(), moved.join(entry.file_name())).unwrap();
        }
    }
    let mut reopened = Database::<f32>::open(moved.join("响应.tdb").to_str().unwrap(), 2).unwrap();
    assert_eq!(reopened.get_payload(1).unwrap(), Some(json!({"host":true})));
    reopened.close().unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        body["data"]["nodes"][0],
        json!({"id":"1","vector":[1.0,0.0],"payload":{"host":true},"edges":[]})
    );
}
