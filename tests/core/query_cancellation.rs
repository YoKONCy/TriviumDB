//! 取消与期限在真实读取及 Console 准入中保留原因、事实和资源账本。

use super::index_maintenance::maintenance;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use triviumdb::database::{Config, StorageMode};
use triviumdb::query::tql_executor::QueryControl;
use triviumdb::{ConsoleHost, ConsoleOptions, Database, TriviumError, VectorType};

fn controls() -> [(QueryControl, &'static str); 3] {
    let cancelled = QueryControl::default();
    cancelled.clone().cancel();
    let expired = QueryControl::with_deadline(std::time::Instant::now());
    let both = expired.clone();
    both.cancel();
    // 两个期限对象共享取消标志；两种条件同时成立仍返回期限类别。
    [
        (cancelled, "TDB_QUERY_CANCELLED"),
        (
            QueryControl::with_deadline(std::time::Instant::now()),
            "TDB_QUERY_DEADLINE_EXCEEDED",
        ),
        (both, "TDB_QUERY_DEADLINE_EXCEEDED"),
    ]
}

fn check_error(error: TriviumError, code: &str, write: Option<Value>) {
    let info = error.info();
    assert_eq!(info.code, code);
    assert_eq!(info.schema_version, 1);
    assert_eq!(info.details, json!({}));
    assert!(info.stage.is_none());
    assert!(info.target.is_none());
    assert_eq!(
        info.write.map(|write| serde_json::to_value(write).unwrap()),
        write
    );
}

fn matrix<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(dtype: &str) {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = super::support::TestDir::new(dtype);
        let path = root.file(&format!("cancel-{mode:?}.tdb"));
        let config = Config {
            dim: 2,
            storage_mode: mode,
            load_text_index: true,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<T>::open_with_config(&path, config).unwrap();
        maintenance::seed(&mut db);
        let before = std::fs::read(format!("{path}.wal")).unwrap();
        let prepared = db.prepare_tql("MATCH (n) RETURN n").unwrap();
        for (control, code) in controls() {
            check_error(
                db.tql_with_control("MATCH (n) RETURN n", control.clone())
                    .unwrap_err(),
                code,
                None,
            );
            check_error(
                db.execute_prepared_tql_with_control(&prepared, &HashMap::new(), control)
                    .unwrap_err(),
                code,
                None,
            );
            assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
            assert_eq!(db.resource_stats().active_tasks, 0);
            assert_eq!(
                maintenance::facts(&db),
                maintenance::expected(false, json!([]))
            );
        }
        let database = Arc::new(Mutex::new(db));
        let host = ConsoleHost::attach(
            &database,
            ConsoleOptions {
                read_only: false,
                ..Default::default()
            },
        )
        .unwrap();
        for (control, code) in controls() {
            for (command, write) in [
                (
                    json!({"operation":"query","query":"MATCH (n) RETURN n"}),
                    None,
                ),
                (
                    json!({"operation":"transaction","operations":[{"op":"delete","id":"1"}]}),
                    Some(json!({"phase":"admission","outcome":"not_committed"})),
                ),
            ] {
                let raw = json!({"version":1,"requestId":"相同请求","command":command}).to_string();
                check_error(
                    host.request_with_control(&raw, control.clone())
                        .unwrap_err(),
                    code,
                    write,
                );
                assert!(!host.session().cancel("相同请求"));
                let retry = json!({"version":1,"requestId":"相同请求","command":{
                    "operation":"query","query":"MATCH (n) RETURN 42 AS value"}})
                .to_string();
                let body: Value = serde_json::from_str(&host.request(&retry).unwrap()).unwrap();
                assert_eq!(
                    body["data"],
                    json!({"rows":[{"value":42},{"value":42}],
                    "rowCount":2,"columns":[{"name":"value","kind":"scalar"}]})
                );
                let core = database.lock().unwrap();
                assert_eq!(core.resource_stats().reserved_workspace_bytes, 0);
                assert_eq!(core.resource_stats().active_tasks, 0);
                assert_eq!(
                    maintenance::facts(&core),
                    maintenance::expected(false, json!([]))
                );
            }
        }
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
        database.lock().unwrap().close().unwrap();
        let mut reopened = Database::<T>::open_with_config(&path, config).unwrap();
        assert_eq!(
            maintenance::facts(&reopened),
            maintenance::expected(false, json!([]))
        );
        reopened.flush().unwrap();
        reopened.close().unwrap();
    }
}

#[test]
fn 取消和期限保留独立类别并归还请求资源() {
    matrix::<f32>("cancel-f32");
    matrix::<half::f16>("cancel-f16");
    matrix::<u64>("cancel-u64");
}
