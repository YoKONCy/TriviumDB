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
    assert_eq!(error["error"]["details"]["io_kind"], "WouldBlock");
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
        assert_eq!(result["error"]["details"]["io_kind"], "WouldBlock");
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
    assert_eq!(error["error"]["details"]["io_kind"], "WouldBlock");
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
