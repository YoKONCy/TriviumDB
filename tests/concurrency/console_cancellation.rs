//! 真实 WAL 前屏障验证取消结果、回执工作区归还和持有宿主时的文件释放。

use super::support;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use triviumdb::database::{Config, StorageMode};
use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
use triviumdb::{ConsoleHost, ConsoleOptions, Database};

fn wire(command: Value) -> String {
    json!({"version":1,"requestId":"同一写请求","command":command}).to_string()
}

#[test]
fn 提交前取消归还工作区且请求可重用并在持有宿主时释放文件() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let mut root = support::TestDir::new("console-cancel-before-commit");
        let path = root.file("db.tdb");
        let config = Config {
            dim: 2,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut core = Database::<f32>::open_with_config(&path, config).unwrap();
        core.insert_with_id(1, &[1., 0.], json!({"saved":"原始事实"}))
            .unwrap();
        let database = Arc::new(Mutex::new(core));
        let host = ConsoleHost::attach(
            &database,
            ConsoleOptions {
                read_only: false,
                ..Default::default()
            },
        )
        .unwrap();
        let baseline: Value = serde_json::from_str(
            &host
                .request(&wire(json!({
                    "operation":"getNodes","ids":["1"],"withRevision":true
                })))
                .unwrap(),
        )
        .unwrap();
        let revision = baseline["data"]["revision"].as_str().unwrap();
        let before = std::fs::read(format!("{path}.wal")).unwrap();
        for command in [
            json!({"operation":"transaction","operations":[{"op":"updateVector","id":"1","vector":[0,1]}]}),
            json!({"operation":"guardedTransaction","expectedRevision":revision,"idempotencyKey":"取消回执",
                "operations":[{"op":"updateVector","id":"1","vector":[0,1]}]}),
            json!({"operation":"importNodes","records":[{"id":"2","vector":[0,1],"payload":{"added":true}}]}),
            json!({"operation":"importEdges","records":[{"source":"1","target":"1","label":"self","weight":2}]}),
        ] {
            let gate = pause_at(ConcurrencyPoint::ConsoleBeforeCommit);
            let worker_host = host.clone();
            let worker = std::thread::spawn(move || worker_host.request(&wire(command)));
            gate.wait_until_arrived();
            assert!(host.session().cancel("同一写请求"));
            gate.release();
            let error = worker.join().unwrap().unwrap_err();
            let info = serde_json::to_value(error.info()).unwrap();
            assert_eq!(info["code"], "TDB_QUERY_CANCELLED");
            assert_eq!(info["details"], json!({}));
            assert_eq!(
                info["write"],
                json!({"phase":"validation","outcome":"not_committed"})
            );
            assert!(!host.session().cancel("同一写请求"));
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
            let result: Value = serde_json::from_str(
                &host
                    .request(&wire(json!({
                        "operation":"getNodes","ids":["1","2"],"withRevision":true
                    })))
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                result["data"],
                json!({"nodes":[{"id":"1","vector":[1.,0.],
                "payload":{"saved":"原始事实"},"edges":[]}],"missingIds":["2"],"revision":revision})
            );
            let core = database.lock().unwrap();
            assert_eq!(core.resource_stats().reserved_workspace_bytes, 0);
            assert_eq!(core.resource_stats().active_tasks, 0);
        }
        let retry = wire(
            json!({"operation":"guardedTransaction","expectedRevision":revision,
            "idempotencyKey":"取消回执","operations":[{"op":"updateVector","id":"1","vector":[0,1]}]}),
        );
        let result: Value = serde_json::from_str(&host.request(&retry).unwrap()).unwrap();
        let committed = result["data"]["revision"].as_str().unwrap();
        assert!(!committed.is_empty());
        assert_ne!(committed, revision);
        assert_eq!(
            result["data"],
            json!({"createdIds":[],"revision":committed,"replayed":false})
        );
        let replay: Value = serde_json::from_str(&host.request(&retry).unwrap()).unwrap();
        assert_eq!(
            replay["data"],
            json!({"createdIds":[],"revision":committed,"replayed":true})
        );
        database.lock().unwrap().close().unwrap();
        let moved = root.0.with_extension("moved");
        assert!(root.0.is_absolute());
        assert_eq!(root.0.parent(), moved.parent());
        std::fs::rename(&root.0, &moved).unwrap();
        root.0 = moved;
        assert_eq!(
            host.request(&wire(json!({"operation":"info"})))
                .unwrap_err()
                .code(),
            "TDB_DATABASE_CLOSED"
        );
        let mut reopened = Database::<f32>::open_with_config(&root.file("db.tdb"), config).unwrap();
        assert_eq!(reopened.all_node_ids().unwrap(), vec![1]);
        let node = reopened.get(1).unwrap().unwrap();
        assert_eq!(node.vector, vec![0., 1.]);
        assert_eq!(node.payload, json!({"saved":"原始事实"}));
        assert!(node.edges.is_empty());
        reopened.flush().unwrap();
        reopened.close().unwrap();
    }
}
