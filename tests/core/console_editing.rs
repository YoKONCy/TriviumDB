//! 编辑协议验证独立业务写入、原子冲突、重复确认和实例边界。

use super::support::TestDir;
use serde_json::{Value, json};
use triviumdb::database::{Config, StorageMode};
use triviumdb::{ConsoleSession, Database, TriviumError, VectorType};

fn wire(command: Value) -> String {
    json!({"version":1,"requestId":"编辑请求","command":command}).to_string()
}
fn call<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    db: &mut Database<T>,
    session: &ConsoleSession,
    command: Value,
) -> Value {
    serde_json::from_str::<Value>(&db.console_request(session, &wire(command)).unwrap()).unwrap()["data"].clone()
}
fn snapshot<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    db: &mut Database<T>,
    session: &ConsoleSession,
) -> Value {
    call(
        db,
        session,
        json!({"operation":"getNodes","ids":["1"],"withRevision":true}),
    )
}

#[test]
fn 完整WAL封条同时恢复事实与回执且未封条事务全部不可见() {
    let root = TestDir::new("durable-receipt-wal");
    let source = root.0.join("source.tdb");
    let config = Config {
        dim: 2,
        storage_mode: StorageMode::Rom,
        auto_build_quiver: false,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(source.to_str().unwrap(), config).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"state":"before"}))
        .unwrap();
    db.flush().unwrap();
    let base = std::fs::read_dir(&root.0)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name().to_str().unwrap().to_owned(), entry.path())
        })
        .filter_map(|(name, path)| {
            let suffix = name.strip_prefix("source.tdb")?;
            if suffix == ".lock" || suffix == ".wal" {
                return None;
            }
            Some((suffix.to_owned(), std::fs::read(path).unwrap()))
        })
        .collect::<Vec<_>>();
    assert!(base.iter().any(|(suffix, _)| suffix.is_empty()));
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let revision = snapshot(&mut db, &session)["revision"].clone();
    let command = guarded(
        &revision,
        "sealed",
        json!([
            {"op":"updatePayload","id":"1","payload":{"state":"after"}},
            {"op":"insert","vector":[0,1],"payload":{"state":"created"}},
            {"op":"link","source":"1","target":"2","label":"rel","weight":0.5,"metadata":{"fact":true}}
        ]),
    );
    let acknowledged = call(&mut db, &session, command.clone());
    assert_eq!(acknowledged["createdIds"], json!(["2"]));
    let wal = std::fs::read(source.with_extension("tdb.wal")).unwrap();
    assert_eq!(&wal[..6], b"TVWL\x06\0");
    let mut cursor = 6;
    let mut commit_start = 6;
    while cursor < wal.len() {
        commit_start = cursor;
        let len = u32::from_le_bytes(wal[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 8 + len;
    }
    assert_eq!(cursor, wal.len());
    db.close().unwrap();
    for sealed in [false, true] {
        let target = root
            .0
            .join(if sealed { "sealed.tdb" } else { "pending.tdb" });
        for (suffix, bytes) in &base {
            std::fs::write(format!("{}{suffix}", target.display()), bytes).unwrap();
        }
        std::fs::write(
            target.with_extension("tdb.wal"),
            if sealed {
                &wal[..]
            } else {
                &wal[..commit_start]
            },
        )
        .unwrap();
        let mut recovered =
            Database::<f32>::open_with_config(target.to_str().unwrap(), config).unwrap();
        let fresh = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
        let receipt = call(&mut recovered, &fresh, status(command.clone()));
        if sealed {
            assert_eq!(
                receipt,
                json!({"outcome":"committed","createdIds":["2"],"revision":acknowledged["revision"]})
            );
            let before = std::fs::read(target.with_extension("tdb.wal")).unwrap();
            assert_eq!(
                call(&mut recovered, &fresh, command.clone()),
                json!({"createdIds":["2"],"revision":acknowledged["revision"],"replayed":true})
            );
            assert_eq!(
                std::fs::read(target.with_extension("tdb.wal")).unwrap(),
                before
            );
            assert_eq!(recovered.all_node_ids().unwrap(), vec![1, 2]);
            let first = recovered.get(1).unwrap().unwrap();
            assert_eq!(first.vector, vec![1., 0.]);
            assert_eq!(first.payload, json!({"state":"after"}));
            assert_eq!(first.edges.len(), 1);
            assert_eq!(first.edges[0].target_id, 2);
            assert_eq!(first.edges[0].label, "rel");
            assert_eq!(first.edges[0].weight, 0.5);
            assert_eq!(first.edges[0].metadata, json!({"fact":true}));
            let second = recovered.get(2).unwrap().unwrap();
            assert_eq!(second.vector, vec![0., 1.]);
            assert_eq!(second.payload, json!({"state":"created"}));
            assert!(second.edges.is_empty());
        } else {
            assert_eq!(receipt, json!({"outcome":"unknown"}));
            assert_eq!(recovered.all_node_ids().unwrap(), vec![1]);
            let node = recovered.get(1).unwrap().unwrap();
            assert_eq!(node.vector, vec![1., 0.]);
            assert_eq!(node.payload, json!({"state":"before"}));
            assert!(node.edges.is_empty());
        }
        recovered.flush().unwrap();
        recovered.close().unwrap();
        let before = std::fs::read(&target).unwrap();
        let mut readonly = Database::<f32>::open_read_only(target.to_str().unwrap(), 2).unwrap();
        assert_eq!(
            call(
                &mut readonly,
                &ConsoleSession::new(Default::default()).unwrap(),
                status(command.clone())
            ),
            receipt
        );
        readonly.close().unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), before);
    }
}
fn guarded(revision: &Value, key: &str, operations: Value) -> Value {
    json!({"operation":"guardedTransaction","expectedRevision":revision,"idempotencyKey":key,"operations":operations})
}
fn status(mut command: Value) -> Value {
    command["operation"] = json!("transactionStatus");
    command
}
fn shared_contract<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(dtype: &str) {
    let fixture: Value =
        serde_json::from_str(include_str!("../contracts/console_editing_cases.json")).unwrap();
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        let root = TestDir::new(&format!("console-edit-{dtype}-{mode:?}"));
        let path = root.0.join("sample.tdb");
        let mut db = Database::<T>::open_with_config(
            path.to_str().unwrap(),
            Config {
                dim: 2,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
        let vector = [T::from_f32(1.), T::from_f32(0.)];
        let info = call(&mut db, &session, json!({"operation":"info"}));
        assert_eq!(
            info["transport"]["idempotency"]["durable"],
            fixture["receiptDurable"]
        );
        assert_eq!(
            info["transport"]["idempotency"]["persistence"],
            fixture["receiptPersistence"]
        );
        db.insert_with_id(1, &vector, fixture["basePayload"].clone())
            .unwrap();
        let old = snapshot(&mut db, &session);
        assert_eq!(old["nodes"][0]["payload"], fixture["basePayload"]);
        assert_eq!(
            old["nodes"][0]["vector"],
            fixture[if dtype == "u64" {
                "encodedBinaryVector"
            } else {
                "encodedFloatVector"
            }]
        );
        assert_eq!(old["nodes"][0]["edges"], json!([]));
        assert_eq!(old["missingIds"], json!([]));
        db.update_payload(1, fixture["hostPayload"].clone())
            .unwrap();
        let operations = json!([
            {"op":"insert","vector":fixture["vector"],"payload":fixture["createdPayload"]},
            {"op":"updatePayload","id":"1","payload":fixture["draftPayload"]}
        ]);
        let rejected = guarded(&old["revision"], "edit-1", operations.clone());
        assert_eq!(
            db.console_request(&session, &wire(rejected))
                .unwrap_err()
                .code(),
            fixture["conflictCode"]
        );
        assert_eq!(db.all_node_ids().unwrap(), vec![1]);
        assert_eq!(db.get_payload(1).unwrap().unwrap(), fixture["hostPayload"]);
        let current = snapshot(&mut db, &session);
        let command = guarded(&current["revision"], "edit-1", operations);
        let first = call(&mut db, &session, command.clone());
        assert_eq!(first["createdIds"], json!([fixture["createdId"]]));
        assert_eq!(first["replayed"], false);
        assert_ne!(first["revision"], current["revision"]);
        let other = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
        let replay_raw = json!({"version":1,"requestId":"重新确认","command":command}).to_string();
        let replay: Value =
            serde_json::from_str(&db.console_request(&other, &replay_raw).unwrap()).unwrap();
        assert_eq!(replay["requestId"], "重新确认");
        assert_eq!(
            replay["data"],
            json!({"createdIds":["2"],"revision":first["revision"],"replayed":true})
        );
        assert_eq!(
            call(&mut db, &other, status(command.clone())),
            json!({
                "outcome":"committed","revision":first["revision"],"createdIds":["2"]
            })
        );
        let mut changed = command.clone();
        changed["operations"][1]["payload"] = json!({"owner":"换内容"});
        assert_eq!(
            db.console_request(&other, &wire(changed.clone()))
                .unwrap_err()
                .code(),
            fixture["keyConflictCode"]
        );
        assert_eq!(
            db.console_request(&other, &wire(status(changed)))
                .unwrap_err()
                .code(),
            fixture["keyConflictCode"]
        );
        db.flush().unwrap();
        assert_eq!(snapshot(&mut db, &other)["revision"], first["revision"]);
        db.close().unwrap();
        let mut reopened = Database::<T>::open(path.to_str().unwrap(), 2).unwrap();
        let fresh = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
        assert_ne!(
            snapshot(&mut reopened, &fresh)["revision"],
            first["revision"]
        );
        assert_eq!(
            call(&mut reopened, &fresh, command.clone()),
            json!({"createdIds":["2"],"revision":first["revision"],"replayed":true})
        );
        assert_eq!(
            call(&mut reopened, &fresh, status(command)),
            json!({"outcome":"committed","createdIds":["2"],"revision":first["revision"]})
        );
        let mut ids = reopened.all_node_ids().unwrap();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2]);
        for (id, payload) in [
            (1, &fixture["draftPayload"]),
            (2, &fixture["createdPayload"]),
        ] {
            let node = reopened.get(id).unwrap().unwrap();
            assert_eq!(node.payload, *payload);
            assert_eq!(
                node.vector.iter().map(|v| v.to_f32()).collect::<Vec<_>>(),
                vec![1., 0.]
            );
            assert!(node.edges.is_empty());
        }
        reopened.close().unwrap();
    }
}

#[test]
fn 三端共享编辑契约覆盖三种dtype及重开边界() {
    shared_contract::<f32>("f32");
    shared_contract::<half::f16>("f16");
    shared_contract::<u64>("u64");
}

#[test]
fn 各业务写入入口与删除重建均使旧编辑版本失效() {
    for operation in [
        "insert",
        "payload",
        "patch",
        "vector",
        "link",
        "edge",
        "unlink",
        "delete",
        "aba",
        "tql",
        "transaction",
        "cas",
    ] {
        let root = TestDir::new(&format!("edit-host-{operation}"));
        let mut db = Database::<f32>::open(root.0.join("sample.tdb").to_str().unwrap(), 2).unwrap();
        db.insert_with_id(1, &[1., 0.], json!({"value":0})).unwrap();
        db.insert_with_id(2, &[0., 1.], json!({"value":0})).unwrap();
        db.link(1, 2, "next", 1.).unwrap();
        let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
        let old = snapshot(&mut db, &session);
        match operation {
            "insert" => {
                db.insert(&[1., 1.], json!({"value":1})).unwrap();
            }
            "payload" => db.update_payload(2, json!({"value":1})).unwrap(),
            "patch" => db.patch_payload(1, json!({"$set":{"value":1}})).unwrap(),
            "vector" => db.update_vector(1, &[0., 1.]).unwrap(),
            "link" => db.link(2, 1, "reverse", 1.).unwrap(),
            "edge" => db
                .upsert_edge(1, 2, "next", 0.5, json!({"state":"new"}))
                .unwrap(),
            "unlink" => db.unlink(1, 2).unwrap(),
            "delete" => db.delete(1).unwrap(),
            "aba" => {
                db.delete(1).unwrap();
                db.insert_with_id(1, &[1., 0.], json!({"value":0})).unwrap();
            }
            "tql" => {
                db.tql_mut("MATCH (n) WHERE n.value == 0 SET n.value == 1")
                    .unwrap();
            }
            "transaction" => {
                let mut tx = triviumdb::database::TxBuilder::new();
                tx.update_payload(1, json!({"value":1}));
                db.commit_tx(tx).unwrap();
            }
            "cas" => {
                db.compare_and_set_payload_field(1, "value", &json!(0), json!(1))
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let command = guarded(
            &old["revision"],
            "stale",
            json!([{ "op":"insert","id":"99","vector":[1,0],"payload":{"bad":true} }]),
        );
        assert!(
            matches!(
                db.console_request(&session, &wire(command)),
                Err(TriviumError::WriteConflict { .. })
            ),
            "{operation}"
        );
        assert!(!db.contains(99).unwrap());
        db.close().unwrap();
    }
}

#[test]
fn 回执淘汰及无事实变化的事务都不能使旧请求重新执行() {
    let root = TestDir::new("edit-eviction");
    let path = root.0.join("sample.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"value":"original"}))
        .unwrap();
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let old = snapshot(&mut db, &session)["revision"].clone();
    let operations = json!([{"op":"unlink","source":"1","target":"99"}]);
    let first = guarded(&old, "first", operations.clone());
    let ack = call(&mut db, &session, first.clone());
    assert_ne!(old, ack["revision"]);
    // 首条加后续 63 条恰好填满回执容量，关闭后仍应保留首条。
    for index in 0..63 {
        let revision = snapshot(&mut db, &session)["revision"].clone();
        call(
            &mut db,
            &session,
            guarded(&revision, &format!("later-{index}"), operations.clone()),
        );
    }
    let reserved = db.resource_stats().reserved_workspace_bytes;
    assert!(reserved > 0);
    db.close().unwrap();
    assert!(db.resource_stats().reserved_workspace_bytes < reserved);
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    assert_eq!(
        call(&mut db, &session, status(first.clone())),
        json!({"outcome":"committed","createdIds":[],"revision":ack["revision"]})
    );
    let revision = snapshot(&mut db, &session)["revision"].clone();
    assert_ne!(revision, ack["revision"]);
    let newest = guarded(&revision, "after-reopen", operations);
    let newest_ack = call(&mut db, &session, newest.clone());
    assert_eq!(
        call(&mut db, &session, status(first.clone())),
        json!({"outcome":"unknown"})
    );
    assert!(matches!(
        db.console_request(&session, &wire(first)),
        Err(TriviumError::WriteConflict { .. })
    ));
    assert_eq!(db.all_node_ids().unwrap(), vec![1]);
    assert_eq!(
        db.get_payload(1).unwrap().unwrap(),
        json!({"value":"original"})
    );
    // 保留该笔已确认事务的 WAL，另行验证恢复后的关闭也会保存纯回执变化。
    let recovery = TestDir::new("edit-eviction-recovery");
    for entry in std::fs::read_dir(&root.0).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name != "sample.tdb.lock" {
            std::fs::copy(entry.path(), recovery.0.join(name)).unwrap();
        }
    }
    db.close().unwrap();
    let recovered_path = recovery.0.join("sample.tdb");
    let mut recovered = Database::<f32>::open(recovered_path.to_str().unwrap(), 2).unwrap();
    recovered.close().unwrap();
    for path in [path, recovered_path] {
        let mut reopened = Database::<f32>::open_read_only(path.to_str().unwrap(), 2).unwrap();
        let readonly = ConsoleSession::new(Default::default()).unwrap();
        assert_eq!(
            call(&mut reopened, &readonly, status(newest.clone())),
            json!({"outcome":"committed","createdIds":[],"revision":newest_ack["revision"]})
        );
        assert_eq!(reopened.all_node_ids().unwrap(), vec![1]);
        let node = reopened.get(1).unwrap().unwrap();
        assert_eq!(node.vector, vec![1., 0.]);
        assert_eq!(node.payload, json!({"value":"original"}));
        assert!(node.edges.is_empty());
        reopened.close().unwrap();
    }
}

#[test]
fn 两个编辑者共享旧快照时只允许一个完整事务提交() {
    use std::sync::{Arc, Barrier, Mutex};
    let root = TestDir::new("edit-two-writers");
    let path = root.0.join("sample.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"owner":"original"}))
        .unwrap();
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let revision = snapshot(&mut db, &session)["revision"].clone();
    let db = Arc::new(Mutex::new(db));
    let barrier = Arc::new(Barrier::new(3));
    let workers = [("a", 2), ("b", 3)]
        .into_iter()
        .map(|(owner, id)| {
            let db = db.clone();
            let barrier = barrier.clone();
            let revision = revision.clone();
            std::thread::spawn(move || {
                let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
                let request = wire(guarded(
                    &revision,
                    owner,
                    json!([
                        {"op":"updatePayload","id":"1","payload":{"owner":owner}},
                        {"op":"insert","id":id.to_string(),"vector":[0,1],"payload":{"owner":owner}}
                    ]),
                ));
                barrier.wait();
                let result = db.lock().unwrap().console_request(&session, &request);
                (owner, id, result)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let outcomes = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(outcomes.iter().filter(|(_, _, r)| r.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|(_, _, r)| matches!(r, Err(TriviumError::WriteConflict { .. })))
            .count(),
        1
    );
    let (winner, id, _) = outcomes.iter().find(|(_, _, r)| r.is_ok()).unwrap();
    db.lock().unwrap().close().unwrap();
    let mut reopened = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    let mut ids = reopened.all_node_ids().unwrap();
    ids.sort_unstable();
    assert_eq!(ids, vec![1, *id]);
    assert_eq!(
        reopened.get_payload(1).unwrap().unwrap(),
        json!({"owner":winner})
    );
    assert_eq!(
        reopened.get_payload(*id).unwrap().unwrap(),
        json!({"owner":winner})
    );
    assert_eq!(reopened.get(*id).unwrap().unwrap().vector, vec![0., 1.]);
    reopened.close().unwrap();
}

#[test]
fn 预检拒绝不消耗版本也不产生回执且响应预算在WAL前生效() {
    let root = TestDir::new("edit-refused");
    let mut db = Database::<f32>::open(root.0.join("sample.tdb").to_str().unwrap(), 2).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"state":"original"}))
        .unwrap();
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let revision = snapshot(&mut db, &session)["revision"].clone();
    let bad = guarded(
        &revision,
        "retry-valid",
        json!([
            {"op":"insert","id":"2","vector":[0,1],"payload":{}},
            {"op":"insert","id":"1","vector":[0,1],"payload":{}}
        ]),
    );
    assert!(matches!(
        db.console_request(&session, &wire(bad.clone()))
            .unwrap_err()
            .underlying(),
        TriviumError::NodeAlreadyExists(1)
    ));
    assert_eq!(snapshot(&mut db, &session)["revision"], revision);
    assert_eq!(
        call(&mut db, &session, status(bad)),
        json!({"outcome":"unknown"})
    );
    let command = guarded(
        &revision,
        "retry-valid",
        json!([{"op":"insert","vector":[0,1],"payload":{"state":"created"}}]),
    );
    let small = ConsoleSession::from_json(r#"{"readOnly":false,"maxResponseBytes":1}"#).unwrap();
    assert!(matches!(
        db.console_request(&small, &wire(command.clone())),
        Err(TriviumError::ConsoleResponseTooLarge {
            committed: false,
            ..
        })
    ));
    assert_eq!(snapshot(&mut db, &session)["revision"], revision);
    assert_eq!(db.all_node_ids().unwrap(), vec![1]);
    let ack = call(&mut db, &session, command);
    assert_eq!(ack["createdIds"], json!(["2"]));
    assert_eq!(
        db.get_payload(1).unwrap().unwrap(),
        json!({"state":"original"})
    );
    assert_eq!(
        db.get_payload(2).unwrap().unwrap(),
        json!({"state":"created"})
    );
    db.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn WAL同步与应用故障不把未知事务当作可重放的新写入() {
    use triviumdb::test_hooks::{IoPoint, fail_io_at};
    for point in [IoPoint::WalSync, IoPoint::TransactionApply] {
        let root = TestDir::new(&format!("edit-uncertain-{point:?}"));
        let path = root.0.join("sample.tdb");
        let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
        db.insert_with_id(1, &[1., 0.], json!({"state":"original"}))
            .unwrap();
        db.flush().unwrap();
        db.set_sync_mode(triviumdb::storage::wal::SyncMode::Full)
            .unwrap();
        let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
        let revision = snapshot(&mut db, &session)["revision"].clone();
        let command = guarded(
            &revision,
            "uncertain",
            json!([
                {"op":"updatePayload","id":"1","payload":{"state":"committed"}},
                {"op":"insert","vector":[0,1],"payload":{"state":"created"}}
            ]),
        );
        let failure = fail_io_at(point);
        assert!(matches!(
            db.console_request(&session, &wire(command.clone()))
                .unwrap_err()
                .underlying(),
            TriviumError::Io(_)
        ));
        drop(failure);
        let retry = db
            .console_request(&session, &wire(command.clone()))
            .unwrap_err();
        assert_eq!(
            retry.code(),
            if point == IoPoint::TransactionApply {
                "TDB_WAL_CLOSED"
            } else {
                "TDB_WRITE_OUTCOME_UNKNOWN"
            }
        );
        drop(db);
        let mut reopened = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
        let fresh = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
        let confirmed = call(&mut reopened, &fresh, status(command.clone()));
        assert_eq!(confirmed["outcome"], "committed");
        assert_eq!(confirmed["createdIds"], json!(["2"]));
        assert_eq!(
            call(&mut reopened, &fresh, command),
            json!({"createdIds":["2"],"revision":confirmed["revision"],"replayed":true})
        );
        assert_eq!(reopened.node_count().unwrap(), 2);
        assert_eq!(
            reopened.get_payload(1).unwrap().unwrap(),
            json!({"state":"committed"})
        );
        assert_eq!(
            reopened.get_payload(2).unwrap().unwrap(),
            json!({"state":"created"})
        );
        assert_eq!(reopened.get(2).unwrap().unwrap().vector, vec![0., 1.]);
        reopened.close().unwrap();
    }
}

#[test]
fn 分组提交拒绝提前确认且退出后原请求仍可执行() {
    let root = TestDir::new("edit-group-commit");
    let path = root.0.join("sample.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"state":"original"}))
        .unwrap();
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let revision = snapshot(&mut db, &session)["revision"].clone();
    let command = guarded(
        &revision,
        "group-rejected",
        json!([
            {"op":"updatePayload","id":"1","payload":{"state":"committed"}},
            {"op":"insert","vector":[0,1],"payload":{"state":"created"}}
        ]),
    );
    let before = std::fs::read(path.with_extension("tdb.wal")).unwrap();
    let (result, _) = db
        .group_commit(|db| db.console_request(&session, &wire(command.clone())))
        .unwrap();
    assert!(matches!(result, Err(TriviumError::InvalidInput(_))));
    assert_eq!(
        std::fs::read(path.with_extension("tdb.wal")).unwrap(),
        before
    );
    assert_eq!(snapshot(&mut db, &session)["revision"], revision);
    assert_eq!(
        call(&mut db, &session, status(command.clone())),
        json!({"outcome":"unknown"})
    );
    assert_eq!(db.all_node_ids().unwrap(), vec![1]);
    assert_eq!(
        db.get_payload(1).unwrap().unwrap(),
        json!({"state":"original"})
    );
    assert_eq!(call(&mut db, &session, command)["createdIds"], json!(["2"]));
    db.close().unwrap();
    let mut reopened = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    assert_eq!(reopened.node_count().unwrap(), 2);
    assert_eq!(
        reopened.get_payload(1).unwrap().unwrap(),
        json!({"state":"committed"})
    );
    assert_eq!(
        reopened.get_payload(2).unwrap().unwrap(),
        json!({"state":"created"})
    );
    assert_eq!(reopened.get(2).unwrap().unwrap().vector, vec![0., 1.]);
    reopened.close().unwrap();
}

#[test]
fn 已提交事务的重放响应失败保留确认结果() {
    let root = TestDir::new("edit-response-outcome");
    let path = root.file("db.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"state":"original"}))
        .unwrap();
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let revision = snapshot(&mut db, &session)["revision"].clone();
    let command = guarded(
        &revision,
        "response-outcome",
        json!([
            {"op":"updatePayload","id":"1","payload":{"state":"committed"}}
        ]),
    );
    let ack = call(&mut db, &session, command.clone());
    let small = ConsoleSession::from_json(r#"{"readOnly":false,"maxResponseBytes":1}"#).unwrap();
    let error = db
        .console_request(&small, &wire(command.clone()))
        .unwrap_err();
    assert_eq!(error.code(), "TDB_CONSOLE_RESPONSE_TOO_LARGE");
    assert_eq!(error.details(), json!({"max_bytes":1,"committed":true}));
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        json!({"phase":"response","outcome":"committed"})
    );
    assert_eq!(
        call(&mut db, &session, status(command)),
        json!({"outcome":"committed","createdIds":[],"revision":ack["revision"]})
    );
    db.close().unwrap();
    let mut reopened = Database::<f32>::open(&path, 2).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), vec![1]);
    let node = reopened.get(1).unwrap().unwrap();
    assert_eq!(node.vector, vec![1., 0.]);
    assert_eq!(node.payload, json!({"state":"committed"}));
    assert!(node.edges.is_empty());
    reopened.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 提交进入WAL后卸载会话仍原子完成并可由新会话确认() {
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
    let root = TestDir::new("edit-detach-wal");
    let path = root.0.join("sample.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"state":"original"}))
        .unwrap();
    let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let revision = snapshot(&mut db, &session)["revision"].clone();
    let command = guarded(
        &revision,
        "cancelled-transport",
        json!([
            {"op":"updatePayload","id":"1","payload":{"state":"committed"}},
            {"op":"insert","vector":[0,1],"payload":{"state":"created"}}
        ]),
    );
    let gate = pause_at(ConcurrencyPoint::AfterWalAppend);
    let worker_session = session.clone();
    let raw = wire(command.clone());
    let worker = std::thread::spawn(move || {
        let result = db.console_request(&worker_session, &raw);
        (db, result)
    });
    gate.wait_until_arrived();
    session.detach();
    let locked = matches!(
        Database::<f32>::open(path.to_str().unwrap(), 2),
        Err(TriviumError::DatabaseLocked(_))
    );
    gate.release();
    let (mut db, result) = worker.join().unwrap();
    assert!(locked);
    let result: Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(result["data"]["createdIds"], json!(["2"]));
    let fresh = ConsoleSession::new(Default::default()).unwrap();
    assert_eq!(
        call(&mut db, &fresh, status(command)),
        json!({"outcome":"committed","createdIds":["2"],"revision":result["data"]["revision"]})
    );
    db.close().unwrap();
    let mut reopened = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    assert_eq!(reopened.node_count().unwrap(), 2);
    assert_eq!(
        reopened.get_payload(1).unwrap().unwrap(),
        json!({"state":"committed"})
    );
    assert_eq!(
        reopened.get_payload(2).unwrap().unwrap(),
        json!({"state":"created"})
    );
    reopened.close().unwrap();
}
