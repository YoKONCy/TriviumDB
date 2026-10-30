//! 控制台用真实数据库验证共享租约、原子事实、跨实例隔离及卸载交错。

use super::support::TestDir;
use serde_json::{Value, json};
use std::sync::{Arc, Barrier, Mutex, mpsc};
use triviumdb::{ConsoleHost, ConsoleOptions, ConsoleSession, Database, TriviumError};

fn request(command: Value) -> String {
    json!({"version":1,"requestId":"验收请求","command":command}).to_string()
}
fn call(db: &mut Database<f32>, session: &ConsoleSession, command: Value) -> Value {
    serde_json::from_str::<Value>(&db.console_request(session, &request(command)).unwrap()).unwrap()
        ["data"]
        .clone()
}
fn seed(path: &str) -> Database<f32> {
    let mut db = Database::open(path, 2).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"kind":"alpha"}))
        .unwrap();
    db.insert_with_id(2, &[0., 1.], json!({"kind":"beta"}))
        .unwrap();
    db
}

fn probe(path: &str, locked: bool) {
    let executable = std::env::current_exe().unwrap();
    let mut command = if std::env::var_os("TRIVIUM_TEST_QEMU_AARCH64").is_some() {
        let mut command = std::process::Command::new("qemu-aarch64");
        command
            .args(["-L", "/usr/aarch64-linux-gnu"])
            .arg(executable);
        command
    } else {
        std::process::Command::new(executable)
    };
    let output = command
        .args([
            "--ignored",
            "--exact",
            "console_host::控制台文件锁子进程探针",
            "--nocapture",
        ])
        .env("TDB_CONSOLE_PROBE_PATH", path)
        .env("TDB_CONSOLE_PROBE_LOCKED", if locked { "1" } else { "0" })
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "子进程锁验证失败：{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "由父用例传入独占路径与预期锁状态"]
fn 控制台文件锁子进程探针() {
    let path = std::env::var("TDB_CONSOLE_PROBE_PATH").expect("子进程必须收到数据库路径");
    let locked = std::env::var("TDB_CONSOLE_PROBE_LOCKED").expect("子进程必须收到锁预期");
    let result = Database::<f32>::open(&path, 2);
    if locked == "1" {
        assert!(matches!(result, Err(TriviumError::DatabaseLocked(_))));
    } else {
        assert_eq!(locked, "0");
        let mut db = result.unwrap();
        assert_eq!(db.get_payload(1).unwrap(), Some(json!({"kind":"alpha"})));
        assert_eq!(db.get_payload(2).unwrap(), Some(json!({"kind":"beta"})));
        db.close().unwrap();
    }
}

#[test]
fn 三端共享控制台请求及原子事实契约() {
    let fixture: Value =
        serde_json::from_str(include_str!("../contracts/console_cases.json")).unwrap();
    let root = TestDir::new("console_host");
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let file = root.0.as_path().join(format!("case-{index}.tdb"));
        let mut db = seed(file.to_str().unwrap());
        let session = ConsoleSession::from_json(&case["options"].to_string()).unwrap();
        if case.get("detached") == Some(&Value::Bool(true)) {
            session.detach();
        }
        let actual = db.console_request(&session, &request(case["command"].clone()));
        if let Some(error) = case.get("error") {
            let actual = actual.unwrap_err();
            assert_eq!(
                actual.code(),
                error["code"].as_str().unwrap(),
                "{}",
                case["name"]
            );
            assert_eq!(actual.details(), error["details"], "{}", case["name"]);
        } else {
            let actual: Value = serde_json::from_str(&actual.unwrap()).unwrap();
            assert_eq!(actual["version"], 1);
            assert_eq!(actual["requestId"], "验收请求");
            assert_eq!(actual["data"], case["expected"], "{}", case["name"]);
        }
        let verify = |db: &Database<f32>| {
            for expected in case
                .get("verify")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let node = db
                    .get(expected["id"].as_str().unwrap().parse().unwrap())
                    .unwrap()
                    .unwrap();
                assert_eq!(json!(node.vector), expected["vector"]);
                assert_eq!(node.payload, expected["payload"]);
                if let Some(expected_edges) = expected.get("edges") {
                    let edges: Vec<_> = node
                        .edges
                        .iter()
                        .map(|edge| {
                            json!({
                                "target":edge.target_id.to_string(),"label":edge.label,
                                "weight":f64::from(edge.weight),"metadata":edge.metadata
                            })
                        })
                        .collect();
                    assert_eq!(json!(edges), *expected_edges);
                }
            }
            for missing in case
                .get("absent")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                assert!(
                    db.get(missing.as_str().unwrap().parse().unwrap())
                        .unwrap()
                        .is_none()
                );
            }
        };
        verify(&db);
        db.close().unwrap();
        let mut reopened = Database::open(file.to_str().unwrap(), 2).unwrap();
        verify(&reopened);
        reopened.close().unwrap();
    }
}

#[test]
fn 图向量索引及写查询均使用现有实例() {
    let root = TestDir::new("console_host");
    let mut db = seed(root.0.as_path().join("图向量.tdb").to_str().unwrap());
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    assert_eq!(
        call(
            &mut db,
            &session,
            json!({"operation":"importEdges","records":[{"source":"1","target":"2","label":"next","weight":0.5,"metadata":{"事实":true}}]})
        ),
        json!({"imported":1})
    );
    let edges = call(
        &mut db,
        &session,
        json!({"operation":"edges","node":"2","incoming":true}),
    );
    assert_eq!(
        edges,
        json!({"edges":[{"source":"1","target":"2","label":"next","weight":0.5,"metadata":{"事实":true}}]})
    );
    assert_eq!(
        call(
            &mut db,
            &session,
            json!({"operation":"path","source":"1","target":"2","maxDepth":2})
        ),
        json!({"path":["1","2"]})
    );
    let graph = call(
        &mut db,
        &session,
        json!({"operation":"subgraph","source":"1","maxNodes":10}),
    );
    assert_eq!(
        graph["nodes"],
        json!([{"id":"1","payload":{"kind":"alpha"}},{"id":"2","payload":{"kind":"beta"}}])
    );
    assert_eq!(graph["edges"], edges["edges"]);
    assert_eq!(graph["truncated"], false);
    let hits = call(
        &mut db,
        &session,
        json!({"operation":"search","vector":[1,0],"topK":2,"minScore":-1}),
    );
    assert_eq!(
        hits,
        json!({"hits":[{"id":"1","score":1.0,"payload":{"kind":"alpha"}},{"id":"2","score":0.0,"payload":{"kind":"beta"}}]})
    );
    assert_eq!(
        call(
            &mut db,
            &session,
            json!({"operation":"manageIndex","kind":"hash","fields":["kind"],"create":true})
        ),
        json!({"changed":true})
    );
    assert_eq!(
        db.indexed_lookup(&[("kind".into(), json!("alpha"))], 10)
            .unwrap()[0],
        1
    );
    let indexes = call(&mut db, &session, json!({"operation":"indexes"}));
    assert_eq!(indexes["indexes"].as_array().unwrap().len(), 1);
    call(
        &mut db,
        &session,
        json!({"operation":"manageIndex","kind":"hash","fields":["kind"],"create":false}),
    );
    assert_eq!(db.list_indexes().unwrap(), Vec::<String>::new());
    let result = call(
        &mut db,
        &session,
        json!({"operation":"mutate","query":"CREATE (n {kind: \"third\"})"}),
    );
    assert_eq!(result, json!({"affected":1,"createdIds":["3"]}));
    assert_eq!(db.get_payload(3).unwrap(), Some(json!({"kind":"third"})));
    db.close().unwrap();
}

#[test]
fn 卸载互不干扰且持有旧会话也能释放文件锁() {
    let root = TestDir::new("console_host");
    let file = root.0.as_path().join("共享.tdb");
    let path = file.to_str().unwrap();
    let database = Arc::new(Mutex::new(seed(path)));
    let host = ConsoleHost::attach(&database, ConsoleOptions::default()).unwrap();
    let other = ConsoleHost::attach(&database, ConsoleOptions::default()).unwrap();
    probe(path, true);
    assert!(matches!(
        Database::<f32>::open(path, 2),
        Err(TriviumError::DatabaseLocked(_))
    ));
    let guard = database.lock().unwrap();
    assert!(
        matches!(host.request(&request(json!({"operation":"info"}))),Err(TriviumError::Io(error)) if error.kind()==std::io::ErrorKind::WouldBlock)
    );
    host.session().detach();
    assert!(matches!(
        host.request(&request(json!({"operation":"info"}))),
        Err(TriviumError::ConsoleSessionClosed)
    ));
    drop(guard);
    assert!(
        other
            .request(&request(json!({"operation":"getNodes","ids":["1"]})))
            .is_ok()
    );
    let old_session = other.session();
    let prepared = database
        .lock()
        .unwrap()
        .prepare_tql("MATCH (n) RETURN n")
        .unwrap();
    database.lock().unwrap().close().unwrap();
    probe(path, false);
    assert!(matches!(
        other.request(&request(json!({"operation":"info"}))),
        Err(TriviumError::DatabaseClosed)
    ));
    let mut new = Database::<f32>::open(path, 2).unwrap();
    assert!(matches!(
        new.console_request(&old_session, &request(json!({"operation":"info"}))),
        Err(TriviumError::InvalidInput(_))
    ));
    assert_eq!(new.get_payload(1).unwrap(), Some(json!({"kind":"alpha"})));
    new.close().unwrap();
    // 继续持有关闭的实例、会话与 Prepared 对象，Windows 仍须允许移动所有数据库制品。
    let moved = root.0.as_path().join("已释放");
    std::fs::create_dir(&moved).unwrap();
    for entry in std::fs::read_dir(root.0.as_path()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::rename(entry.path(), moved.join(entry.file_name())).unwrap();
        }
    }
    drop((prepared, host, other, database));
}

struct BlockingHook {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}
impl triviumdb::SearchHook for BlockingHook {
    fn on_pre_search(
        &self,
        _query: &mut Vec<f32>,
        _config: &mut triviumdb::SearchConfig,
        _ctx: &mut triviumdb::HookContext,
    ) {
        self.entered.wait();
        self.release.wait();
    }
}

#[test]
fn 卸载在真实请求持锁时取消且宿主关闭等待真实退出() {
    let root = TestDir::new("console_host");
    let file = root.0.as_path().join("并发关闭.tdb");
    let path = file.to_str().unwrap();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let mut db = seed(path);
    db.set_hook(BlockingHook {
        entered: entered.clone(),
        release: release.clone(),
    })
    .unwrap();
    let database = Arc::new(Mutex::new(db));
    let host = ConsoleHost::attach(&database, ConsoleOptions::default()).unwrap();
    let worker_host = host.clone();
    let worker = std::thread::spawn(move || {
        worker_host.request(&request(
            json!({"operation":"search","vector":[1,0],"topK":2}),
        ))
    });
    entered.wait();
    host.session().detach();
    let (started_tx, started_rx) = mpsc::channel();
    let closing_db = database.clone();
    let closing = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        closing_db.lock().unwrap().close()
    });
    started_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(matches!(
        Database::<f32>::open(path, 2),
        Err(TriviumError::DatabaseLocked(_))
    ));
    release.wait();
    assert!(matches!(
        worker.join().unwrap(),
        Err(TriviumError::QueryCancelled)
    ));
    closing.join().unwrap().unwrap();
    let mut reopened = Database::<f32>::open(path, 2).unwrap();
    assert_eq!(
        reopened.get_payload(1).unwrap(),
        Some(json!({"kind":"alpha"}))
    );
    reopened.close().unwrap();
    assert!(!host.session().cancel("验收请求"));
}

#[test]
fn 组件弱引用不会延长宿主文件锁寿命() {
    let root = TestDir::new("console_host");
    let file = root.0.as_path().join("弱引用.tdb");
    let path = file.to_str().unwrap();
    let database = Arc::new(Mutex::new(seed(path)));
    let host = ConsoleHost::attach(&database, ConsoleOptions::default()).unwrap();
    drop(database);
    assert!(matches!(
        host.request(&request(json!({"operation":"info"}))),
        Err(TriviumError::DatabaseClosed)
    ));
    let mut reopened = Database::<f32>::open(path, 2).unwrap();
    assert_eq!(
        reopened.get_payload(2).unwrap(),
        Some(json!({"kind":"beta"}))
    );
    reopened.close().unwrap();
}

#[test]
fn 响应预算在写入提交前拒绝且保留原有事实() {
    let root = TestDir::new("console_host");
    let mut db = seed(root.0.as_path().join("响应预算.tdb").to_str().unwrap());
    let session = ConsoleSession::from_json(r#"{"readOnly":false,"maxResponseBytes":1}"#).unwrap();
    let read = db
        .console_request(
            &session,
            &request(json!({"operation":"getNodes","ids":["1"]})),
        )
        .unwrap_err();
    assert_eq!(read.details(), json!({"max_bytes":1,"committed":false}));
    let write=db.console_request(&session,&request(json!({"operation":"transaction","operations":[{"op":"updatePayload","id":"1","payload":{"完成":true}}]}))).unwrap_err();
    assert_eq!(write.code(), "TDB_CONSOLE_RESPONSE_TOO_LARGE");
    assert_eq!(write.details(), json!({"max_bytes":1,"committed":false}));
    let write = db
        .console_request(
            &session,
            &request(json!({"operation":"mutate","query":"CREATE (n {done: true})"})),
        )
        .unwrap_err();
    assert_eq!(write.details(), json!({"max_bytes":1,"committed":false}));
    assert_eq!(db.get_payload(1).unwrap(), Some(json!({"kind":"alpha"})));
    assert_eq!(db.all_node_ids().unwrap(), vec![1, 2]);
    db.close().unwrap();
}

#[test]
fn 二进制向量以字符串保留完整位宽() {
    let root = TestDir::new("console_host");
    let mut db =
        Database::<u64>::open(root.0.as_path().join("二进制.tdb").to_str().unwrap(), 2).unwrap();
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    db.console_request(&session,&request(json!({"operation":"importNodes","records":[{"id":"9007199254740993","vector":["18446744073709551615","1"],"payload":{"位宽":64}}]}))).unwrap();
    assert_eq!(
        db.get(9_007_199_254_740_993).unwrap().unwrap().vector,
        vec![u64::MAX, 1]
    );
    let result: Value = serde_json::from_str(
        &db.console_request(
            &session,
            &request(json!({"operation":"getNodes","ids":["9007199254740993"]})),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        result["data"]["nodes"][0],
        json!({"id":"9007199254740993","vector":["18446744073709551615","1"],"payload":{"位宽":64},"edges":[]})
    );
    db.close().unwrap();
}

#[test]
fn 半精度协议使用数值且不把存储位模式当作浮点值() {
    let root = TestDir::new("console_half");
    let mut db = Database::<half::f16>::open(&root.file("半精度.tdb"), 2).unwrap();
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    db.console_request(&session,&request(json!({"operation":"importNodes","records":[{"id":"1","vector":[1.0007,1.0004882812500002],"payload":{}}]}))).unwrap();
    // 入站先取 f32，再按 half 最近偶数舍入：普通上侧值取 0x3c01，f32 中点取 0x3c00。
    assert_eq!(db.get(1).unwrap().unwrap().vector[0].to_bits(), 0x3c01);
    assert_eq!(db.get(1).unwrap().unwrap().vector[1].to_bits(), 0x3c00);
    let result: Value = serde_json::from_str(
        &db.console_request(
            &session,
            &request(json!({"operation":"getNodes","ids":["1"]})),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        result["data"]["nodes"][0]["vector"],
        json!([1.0009765625, 1.0])
    );
    db.close().unwrap();
}

#[test]
fn 写计划准入在wal前拒绝且请求不能夹带宿主配置() {
    let root = TestDir::new("console_admission");
    let path = root.file("准入.tdb");
    let mut db = seed(&path);
    let session = ConsoleSession::from_json(r#"{"readOnly":false,"maxOperations":1}"#).unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    let error = db
        .console_request(
            &session,
            &request(json!({"operation":"mutate","query":"MATCH (n) CREATE (m {kind: \"拒绝\"})"})),
        )
        .unwrap_err();
    assert_eq!(error.details(), json!({"budget":1}));
    let error=db.console_request(&session,&request(json!({"operation":"transaction","operations":[{"op":"insert","id":"3","vector":[1,0],"payload":{},"readOnly":false}]}))).unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert_eq!(db.all_node_ids().unwrap(), vec![1, 2]);
    db.close().unwrap();
}

#[test]
fn 只读宿主即使会话开放写入也保持物理零写() {
    use std::collections::BTreeMap;
    let root = TestDir::new("console_readonly");
    let path = root.file("只读.tdb");
    seed(&path).close().unwrap();
    let snapshot = || {
        std::fs::read_dir(&root.0)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), std::fs::read(entry.path()).unwrap())
            })
            .collect::<BTreeMap<_, _>>()
    };
    let before = snapshot();
    let mut db = Database::<f32>::open_read_only(&path, 2).unwrap();
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let info = call(&mut db, &session, json!({"operation":"info"}));
    assert_eq!(info["readOnly"], true);
    assert_eq!(
        call(
            &mut db,
            &session,
            json!({"operation":"getNodes","ids":["1"]})
        )["nodes"][0]["payload"],
        json!({"kind":"alpha"})
    );
    for command in [
        json!({"operation":"mutate","query":"CREATE (n {name: \"拒绝\"})"}),
        json!({"operation":"transaction","operations":[{"op":"delete","id":"1"}]}),
        json!({"operation":"importNodes","records":[{"vector":[1,0],"payload":{}}]}),
        json!({"operation":"importEdges","records":[{"source":"1","target":"2","label":"拒绝","weight":1}]}),
        json!({"operation":"manageIndex","kind":"hash","fields":["kind"],"create":true}),
        json!({"operation":"buildQuiver"}),
    ] {
        let error = db.console_request(&session, &request(command)).unwrap_err();
        assert_eq!(error.code(), "TDB_READ_ONLY");
    }
    assert_eq!(snapshot(), before);
    db.close().unwrap();
    assert_eq!(snapshot(), before);
}

#[test]
fn 响应缓冲参与共享工作区准入且失败后归还额度() {
    use triviumdb::database::Config;
    use triviumdb::{ResourceLimits, ResourceRuntime};
    let root = TestDir::new("console_workspace");
    let database_budget = 1024 * 1024;
    let runtime = ResourceRuntime::new(ResourceLimits {
        memory_limit: 16 * 1024 * 1024,
        ..Default::default()
    })
    .unwrap();
    let mut db = Database::<f32>::open_with_runtime(
        &root.file("宿主.tdb"),
        Config {
            dim: 2,
            memory_limit: database_budget,
            ..Default::default()
        },
        runtime.clone(),
    )
    .unwrap();
    let mut blocker = Database::<f32>::open_with_runtime(
        &root.file("配额.tdb"),
        Config {
            dim: 2,
            memory_limit: database_budget,
            ..Default::default()
        },
        runtime.clone(),
    )
    .unwrap();
    let available = runtime.stats().available_foreground_bytes.unwrap();
    // 仅占用账本额度，留下六十四字节；不真实耗尽开发机内存。
    blocker
        .set_memory_limit(
            available
                .checked_add(database_budget)
                .unwrap()
                .checked_sub(64)
                .unwrap(),
        )
        .unwrap();
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let raw = request(
        json!({"operation":"transaction","operations":[{"op":"insert","id":"1","vector":[1,0],"payload":{"quota":"retained"}}]}),
    );
    let before = std::fs::read(root.file("宿主.tdb.wal")).unwrap();
    let error = db.console_request(&session, &raw).unwrap_err();
    assert!(matches!(
        error,
        TriviumError::ResourceBudgetExceeded {
            resource: "memory_bytes",
            ..
        }
    ));
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert_eq!(runtime.stats().active_tasks, 0);
    assert_eq!(db.node_count().unwrap(), 0);
    assert_eq!(std::fs::read(root.file("宿主.tdb.wal")).unwrap(), before);
    blocker.set_memory_limit(database_budget).unwrap();
    let actual: Value = serde_json::from_str(&db.console_request(&session, &raw).unwrap()).unwrap();
    assert_eq!(actual["data"], json!({"createdIds":["1"]}));
    assert_eq!(db.get(1).unwrap().unwrap().vector, vec![1., 0.]);
    assert_eq!(
        db.get_payload(1).unwrap(),
        Some(json!({"quota":"retained"}))
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    db.close().unwrap();
    blocker.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 宿主关闭失败保留同一文件锁且允许原会话重试() {
    let root = TestDir::new("console_close_retry");
    let path = root.file("重试.tdb");
    let database = Arc::new(Mutex::new(seed(&path)));
    let host = ConsoleHost::attach(&database, ConsoleOptions::default()).unwrap();
    {
        let _failure =
            triviumdb::test_hooks::fail_io_at(triviumdb::test_hooks::IoPoint::MarkerCreate);
        assert!(database.lock().unwrap().close().is_err());
    }
    assert!(matches!(
        Database::<f32>::open(&path, 2),
        Err(TriviumError::DatabaseLocked(_))
    ));
    let data: Value = serde_json::from_str(
        &host
            .request(&request(json!({"operation":"getNodes","ids":["1"]})))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(data["data"]["nodes"][0]["payload"], json!({"kind":"alpha"}));
    database.lock().unwrap().close().unwrap();
    let mut reopened = Database::<f32>::open(&path, 2).unwrap();
    assert_eq!(
        reopened.get_payload(1).unwrap(),
        Some(json!({"kind":"alpha"}))
    );
    reopened.close().unwrap();
}
