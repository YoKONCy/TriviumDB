//! 单条 WAL 应用失败进入统一恢复保护，重开核对独立完整事实。

use serde_json::{Value, json};
use triviumdb::database::{Config, StorageMode};
use triviumdb::storage::wal::SyncMode;
use triviumdb::test_hooks::{IoPoint, fail_io_at};
use triviumdb::{Database, VectorType};

fn vector<T: VectorType>(a: f32, b: f32) -> Vec<T> {
    vec![T::from_f32(a), T::from_f32(b)]
}

fn facts<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    database: &Database<T>,
) -> Value {
    let mut ids = database.all_node_ids().unwrap();
    ids.sort_unstable();
    let nodes: Vec<Value> = ids
        .into_iter()
        .map(|id| {
            let node = database.get(id).unwrap().unwrap();
            let mut edges = node.edges;
            edges.sort_by(|left, right| {
                (left.target_id, &left.label).cmp(&(right.target_id, &right.label))
            });
            let edges: Vec<Value> = edges
                .iter()
                .map(|edge| {
                    json!({
                        "target": edge.target_id,
                        "label": edge.label,
                        "weight": edge.weight,
                        "metadata": edge.metadata,
                    })
                })
                .collect();
            json!({
                "id": id,
                "vector": node.vector.iter().map(|value| value.to_f32()).collect::<Vec<_>>(),
                "payload": node.payload,
                "edges": edges,
            })
        })
        .collect();
    json!(nodes)
}

fn execute<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    database: &mut Database<T>,
    operation: &str,
) -> triviumdb::Result<()> {
    match operation {
        "insert" => database
            .insert(&vector(1., 1.), json!({"state":"inserted"}))
            .map(|_| ()),
        "insert_with_id" => {
            database.insert_with_id(3, &vector(1., 1.), json!({"state":"inserted"}))
        }
        "update_payload" => database.update_payload(1, json!({"state":"changed","count":2})),
        "patch_payload" => {
            database.patch_payload(1, json!({"$set":{"state":"changed"},"$inc":{"count":1}}))
        }
        "cas" => database.compare_and_set_payload_field(1, "count", &json!(1), json!(2)),
        "update_vector" => database.update_vector(1, &vector(0., 1.)),
        "link" => database.link(1, 2, "new", 0.5),
        "delete" => database.delete(1),
        "unlink" => database.unlink(1, 2),
        "unlink_label" => database.unlink_label(1, 2, "related"),
        other => panic!("未知直接写入契约 {other}"),
    }
}

fn matrix<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(dtype: &str) {
    let contract: Value =
        serde_json::from_str(include_str!("../contracts/direct_write_cases.json")).unwrap();
    let outcomes: Value =
        serde_json::from_str(include_str!("../contracts/write_outcomes.json")).unwrap();
    let root = super::support::TestDir::new(dtype);
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for point in [IoPoint::WalSync, IoPoint::DirectWriteApply] {
            for case in contract["cases"].as_array().unwrap() {
                let operation = case["operation"].as_str().unwrap();
                let directory = root.0.join(format!("{mode:?}-{point:?}-{operation}"));
                std::fs::create_dir(&directory).unwrap();
                let path = directory.join("db.tdb");
                let config = Config {
                    dim: 2,
                    storage_mode: mode,
                    sync_mode: SyncMode::Full,
                    auto_build_quiver: false,
                    ..Default::default()
                };
                let mut database =
                    Database::<T>::open_with_config(path.to_str().unwrap(), config).unwrap();
                database
                    .insert_with_id(1, &vector(1., 0.), json!({"state":"seed","count":1}))
                    .unwrap();
                database
                    .insert_with_id(2, &vector(0., 1.), json!({"state":"neighbor"}))
                    .unwrap();
                database.link(1, 2, "related", 1.).unwrap();
                database.flush().unwrap();
                assert_eq!(facts(&database), contract["before"]);
                let failure = fail_io_at(point);
                let error = execute(&mut database, operation).unwrap_err();
                drop(failure);
                assert_eq!(
                    error.code(),
                    "TDB_IO",
                    "{dtype}/{mode:?}/{point:?}/{operation}"
                );
                let expected = if point == IoPoint::WalSync {
                    &outcomes["direct_wal"]
                } else {
                    &outcomes["direct_apply"]
                };
                assert_eq!(
                    serde_json::to_value(error.info()).unwrap()["write"],
                    *expected
                );
                if point == IoPoint::WalSync {
                    assert_eq!(facts(&database), contract["before"]);
                } else {
                    assert_eq!(database.node_count().unwrap_err().code(), "TDB_WAL_CLOSED");
                }
                let before = std::fs::read(path.with_extension("tdb.wal")).unwrap();
                let rejected = database
                    .update_payload(2, json!({"state":"rejected"}))
                    .unwrap_err();
                assert_eq!(rejected.code(), "TDB_WAL_CLOSED");
                assert_eq!(
                    serde_json::to_value(rejected.info()).unwrap()["write"],
                    outcomes["direct_admission"]
                );
                assert_eq!(
                    std::fs::read(path.with_extension("tdb.wal")).unwrap(),
                    before
                );
                drop(database);
                let moved = directory.with_extension("moved");
                std::fs::rename(&directory, &moved).unwrap();
                let path = moved.join("db.tdb");
                let mut reopened =
                    Database::<T>::open_with_config(path.to_str().unwrap(), config).unwrap();
                // sync 故障在完整 flush 后注入，应用故障也已有完整 WAL，当前进程内重开必须恢复整条事实。
                assert_eq!(
                    facts(&reopened),
                    case["after"],
                    "{dtype}/{mode:?}/{point:?}/{operation}"
                );
                reopened
                    .insert_with_id(4, &vector(1., 1.), json!({"state":"after-reopen"}))
                    .unwrap();
                reopened.close().unwrap();
                let mut reopened =
                    Database::<T>::open_with_config(path.to_str().unwrap(), config).unwrap();
                let mut expected = case["after"].as_array().unwrap().clone();
                expected.push(json!({"id":4,"vector":[1.0,1.0],"payload":{"state":"after-reopen"},"edges":[]}));
                assert_eq!(facts(&reopened), json!(expected));
                reopened.close().unwrap();
            }
        }
    }
}

#[test]
fn direct_write_faults_f32() {
    matrix::<f32>("direct-f32");
}
#[test]
fn direct_write_faults_f16() {
    matrix::<half::f16>("direct-f16");
}
#[test]
fn direct_write_faults_u64() {
    matrix::<u64>("direct-u64");
}
