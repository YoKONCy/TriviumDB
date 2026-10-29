//! 以单节点抽象状态验证事务的有序组合、失败原子性与持久化结果。

use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use triviumdb::database::{Config, StorageMode, TxBuilder};
use triviumdb::{Database, TriviumError, VectorType};

struct TestDir(std::path::PathBuf);
impl TestDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "tdb_transaction_state_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ExpectedNode {
    vector: [f32; 4],
    payload: Value,
}

fn vector<T: VectorType>(value: [f32; 4]) -> Vec<T> {
    value.into_iter().map(T::from_f32).collect()
}

fn assert_node<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    db: &Database<T>,
    expected: &Option<ExpectedNode>,
) {
    match expected {
        Some(expected) => {
            let node = db.get(10).unwrap().unwrap();
            assert_eq!(node.id, 10);
            assert_eq!(node.payload, expected.payload);
            assert_eq!(
                node.vector.iter().map(|x| x.to_f32()).collect::<Vec<_>>(),
                expected.vector
            );
            assert_eq!(db.node_count().unwrap(), 2);
        }
        None => {
            assert!(db.get(10).unwrap().is_none());
            assert_eq!(db.node_count().unwrap(), 1);
        }
    }
    let untouched = db.get(20).unwrap().unwrap();
    assert_eq!(untouched.payload, json!({"state":"untouched"}));
    assert_eq!(
        untouched
            .vector
            .iter()
            .map(|x| x.to_f32())
            .collect::<Vec<_>>(),
        [1., 0., 0., 0.]
    );
}

fn ordered_model<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>() {
    let dir = TestDir::new();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = dir.file(&format!("{mode:?}.tdb"));
        let config = Config {
            dim: 4,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<T>::open_with_config(&path, config).unwrap();
        db.insert_with_id(20, &vector([1., 0., 0., 0.]), json!({"state":"untouched"}))
            .unwrap();
        for initially_present in [false, true] {
            // 四种操作的全部一至四步组合，覆盖“删除→重建→重复插入”等状态反转。
            for length in 1..=4 {
                for encoded in 0..4usize.pow(length) {
                    if db.contains(10).unwrap() {
                        db.delete(10).unwrap();
                    }
                    let initial = initially_present.then(|| ExpectedNode {
                        vector: [1., 0., 0., 0.],
                        payload: json!({"state":"original"}),
                    });
                    if let Some(node) = &initial {
                        db.insert_with_id(10, &vector(node.vector), node.payload.clone())
                            .unwrap();
                    }
                    let before_wal = std::fs::read(format!("{path}.wal")).unwrap();
                    let mut projected = initial.clone();
                    let mut expected_ids = Vec::new();
                    let mut expected_error = None;
                    let mut tx = TxBuilder::new();
                    let mut digits = encoded;
                    for _ in 0..length {
                        let operation = digits % 4;
                        digits /= 4;
                        match operation {
                            0 => tx.insert_with_id(
                                10,
                                &vector([0., 1., 0., 0.]),
                                json!({"state":"inserted"}),
                            ),
                            1 => tx.delete(10),
                            2 => tx.update_payload(10, json!({"state":"updated"})),
                            3 => tx.update_vector(10, &vector([0., 0., 1., 0.])),
                            _ => unreachable!(),
                        }
                        if expected_error.is_none() {
                            match (operation, projected.as_mut()) {
                                (0, Some(_)) => expected_error = Some(true),
                                (0, None) => {
                                    projected = Some(ExpectedNode {
                                        vector: [0., 1., 0., 0.],
                                        payload: json!({"state":"inserted"}),
                                    });
                                    expected_ids.push(10);
                                }
                                (_, None) => expected_error = Some(false),
                                (1, Some(_)) => projected = None,
                                (2, Some(node)) => node.payload = json!({"state":"updated"}),
                                (3, Some(node)) => node.vector = [0., 0., 1., 0.],
                                _ => unreachable!(),
                            }
                        }
                    }
                    let result = db.commit_tx(tx);
                    let context = format!(
                        "{mode:?}, 初始存在={initially_present}, 长度={length}, 编码={encoded}"
                    );
                    let expected = match expected_error {
                        Some(duplicate) => {
                            if duplicate {
                                assert!(
                                    matches!(result, Err(TriviumError::NodeAlreadyExists(10))),
                                    "{context}: {result:?}"
                                );
                            } else {
                                assert!(
                                    matches!(result, Err(TriviumError::NodeNotFound(10))),
                                    "{context}: {result:?}"
                                );
                            }
                            assert_eq!(
                                std::fs::read(format!("{path}.wal")).unwrap(),
                                before_wal,
                                "{context}"
                            );
                            initial
                        }
                        None => {
                            assert_eq!(result.unwrap(), expected_ids, "{context}");
                            projected
                        }
                    };
                    assert_node(&db, &expected);
                }
            }
        }
        let final_node = Some(ExpectedNode {
            vector: [0., 0., 1., 0.],
            payload: json!({"state":"original"}),
        });
        assert_node(&db, &final_node);
        db.close().unwrap();
        let mut reopened = Database::<T>::open_with_config(&path, config).unwrap();
        assert_node(&reopened, &final_node);
        reopened.close().unwrap();
    }
}

#[test]
fn ordered_state_f32() {
    ordered_model::<f32>();
}
#[test]
fn ordered_state_f16() {
    ordered_model::<half::f16>();
}
#[test]
fn ordered_state_u64() {
    ordered_model::<u64>();
}

#[test]
fn unrecoverable_json_is_rejected_before_wal() {
    let dir = TestDir::new();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for operation in 0..4 {
            let path = dir.file(&format!("{mode:?}-{operation}.tdb"));
            let mut db = Database::<f32>::open_with_config(
                &path,
                Config {
                    dim: 4,
                    storage_mode: mode,
                    auto_build_quiver: false,
                    ..Default::default()
                },
            )
            .unwrap();
            db.insert_with_id(10, &[1., 0., 0., 0.], json!({"state":"original"}))
                .unwrap();
            db.insert_with_id(20, &[1., 0., 0., 0.], json!({"state":"untouched"}))
                .unwrap();
            // 超过恢复端默认 128 层的 JSON 深度，Value 本身仍可由调用方手工构造。
            let deep = (0..129).fold(Value::Null, |value, _| Value::Array(vec![value]));
            let before_wal = std::fs::read(format!("{path}.wal")).unwrap();
            let mut tx = db.begin_tx();
            tx.update_payload(20, json!({"must_not":"commit"}));
            match operation {
                0 => tx.insert(&[1., 0., 0., 0.], deep),
                1 => tx.insert_with_id(30, &[1., 0., 0., 0.], deep),
                2 => tx.update_payload(10, deep),
                3 => tx.upsert_edge(10, 20, "deep", 1., deep),
                _ => unreachable!(),
            }
            assert!(matches!(tx.commit(), Err(TriviumError::InvalidInput(_))));
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before_wal);
            assert_node(
                &db,
                &Some(ExpectedNode {
                    vector: [1., 0., 0., 0.],
                    payload: json!({"state":"original"}),
                }),
            );
            assert!(db.get_edges(10).unwrap().is_empty());
            db.close().unwrap();
        }
    }
}

#[test]
fn shared_transaction_contract() {
    fn check<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>() {
        let contract: Value =
            serde_json::from_str(include_str!("../contracts/transaction_cases.json")).unwrap();
        let dir = TestDir::new();
        for mode in [StorageMode::Rom, StorageMode::Mmap] {
            for case in contract["cases"].as_array().unwrap() {
                let path = dir.file(&format!("{mode:?}-{}.tdb", case["name"].as_str().unwrap()));
                let config = Config {
                    dim: contract["dimension"].as_u64().unwrap() as usize,
                    storage_mode: mode,
                    auto_build_quiver: false,
                    ..Default::default()
                };
                let mut db = Database::<T>::open_with_config(&path, config).unwrap();
                let decode_vector = |value: &Value| {
                    value
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| T::from_f32(x.as_f64().unwrap() as f32))
                        .collect::<Vec<_>>()
                };
                for node in contract["setup"].as_array().unwrap() {
                    db.insert_with_id(
                        node["id"].as_u64().unwrap(),
                        &decode_vector(&node["vector"]),
                        node["payload"].clone(),
                    )
                    .unwrap();
                }
                let before_wal = std::fs::read(format!("{path}.wal")).unwrap();
                let mut tx = TxBuilder::new();
                for op in case["operations"].as_array().unwrap() {
                    match op["type"].as_str().unwrap() {
                        "insert" => tx.insert(&decode_vector(&op["vector"]), op["payload"].clone()),
                        "insertWithId" => tx.insert_with_id(
                            op["id"].as_u64().unwrap(),
                            &decode_vector(&op["vector"]),
                            op["payload"].clone(),
                        ),
                        "delete" => tx.delete(op["id"].as_u64().unwrap()),
                        "updatePayload" => {
                            tx.update_payload(op["id"].as_u64().unwrap(), op["payload"].clone())
                        }
                        "updateVector" => tx.update_vector(
                            op["id"].as_u64().unwrap(),
                            &decode_vector(&op["vector"]),
                        ),
                        other => panic!("未知事务契约操作：{other}"),
                    }
                }
                let result = db.commit_tx(tx);
                let expected = &case["expected"];
                match expected.get("error").and_then(Value::as_str) {
                    Some("NodeAlreadyExists") => {
                        assert!(matches!(result, Err(TriviumError::NodeAlreadyExists(10))))
                    }
                    Some("NodeNotFound") => {
                        assert!(matches!(result, Err(TriviumError::NodeNotFound(10))))
                    }
                    Some(other) => panic!("未知事务契约错误：{other}"),
                    None => assert_eq!(
                        result.unwrap(),
                        expected["ids"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|id| id.as_u64().unwrap())
                            .collect::<Vec<_>>()
                    ),
                }
                if expected.get("error").is_some() {
                    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before_wal);
                }
                let assert_contents = |db: &Database<T>| {
                    let expected_nodes = expected["nodes"].as_array().unwrap();
                    let mut ids = db.get_all_ids().unwrap();
                    ids.sort_unstable();
                    assert_eq!(
                        ids,
                        expected_nodes
                            .iter()
                            .map(|node| node["id"].as_u64().unwrap())
                            .collect::<Vec<_>>()
                    );
                    for expected_node in expected_nodes {
                        let node = db
                            .get(expected_node["id"].as_u64().unwrap())
                            .unwrap()
                            .unwrap();
                        assert_eq!(node.payload, expected_node["payload"]);
                        assert_eq!(
                            node.vector
                                .iter()
                                .map(|x| x.to_f32() as f64)
                                .collect::<Vec<_>>(),
                            expected_node["vector"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|x| x.as_f64().unwrap())
                                .collect::<Vec<_>>()
                        );
                    }
                };
                assert_contents(&db);
                let replay_path = format!("{path}.replay");
                std::fs::copy(&path, &replay_path).unwrap();
                std::fs::copy(format!("{path}.wal"), format!("{replay_path}.wal")).unwrap();
                let mut replay = Database::<T>::open_with_config(&replay_path, config).unwrap();
                assert_contents(&replay);
                replay.close().unwrap();
                db.close().unwrap();
                let mut reopened = Database::<T>::open_with_config(&path, config).unwrap();
                assert_contents(&reopened);
                reopened.close().unwrap();
            }
        }
    }
    check::<f32>();
    check::<half::f16>();
    check::<u64>();
}

#[test]
fn recreation_updates_graph_text_and_unique_indexes() {
    fn check<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>() {
        let dir = TestDir::new();
        for mode in [StorageMode::Rom, StorageMode::Mmap] {
            let path = dir.file(&format!("graph-{mode:?}.tdb"));
            let config = Config {
                dim: 4,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            };
            let mut db = Database::<T>::open_with_config(&path, config).unwrap();
            db.insert_with_id(10, &vector([1., 0., 0., 0.]), json!({"key":"old"}))
                .unwrap();
            db.insert_with_id(20, &vector([1., 0., 0., 0.]), json!({"key":"peer"}))
                .unwrap();
            db.create_unique_index("key").unwrap();
            db.link(10, 20, "old-out", 1.).unwrap();
            db.link(20, 10, "old-in", 1.).unwrap();
            db.index_text(10, "oldtoken").unwrap();
            db.index_keyword(10, "oldanchor").unwrap();
            db.build_text_index().unwrap();
            // 让文本进入延期加载路径，验证预留的删除记录在事务里正确归属旧节点。
            db.close().unwrap();
            let mut db = Database::<T>::open_with_config(&path, config).unwrap();
            let mut tx = db.begin_tx();
            tx.delete(10);
            tx.insert_with_id(10, &vector([0., 1., 0., 0.]), json!({"key":"new"}));
            tx.update_payload(10, json!({"key":"final"}));
            tx.update_vector(10, &vector([0., 0., 1., 0.]));
            tx.upsert_edge(10, 20, "new-out", 3., json!({"edge":"new"}));
            tx.link(10, 10, "self", 2.);
            tx.unlink_label(10, 20, "absent");
            assert_eq!(tx.commit().unwrap(), [10]);
            for persisted in [false, true] {
                if persisted {
                    db.close().unwrap();
                    db = Database::<T>::open_with_config(&path, config).unwrap();
                }
                assert_eq!(db.node_count().unwrap(), 2);
                let node = db.get(10).unwrap().unwrap();
                assert_eq!(node.payload, json!({"key":"final"}));
                assert_eq!(
                    node.vector.iter().map(|x| x.to_f32()).collect::<Vec<_>>(),
                    [0., 0., 1., 0.]
                );
                assert_eq!(node.edges.len(), 2);
                let edge = db.get_edge(10, 20, "new-out").unwrap().unwrap();
                assert_eq!(edge.weight, 3.);
                assert_eq!(edge.metadata, json!({"edge":"new"}));
                assert_eq!(db.get_edge(10, 10, "self").unwrap().unwrap().weight, 2.);
                assert!(db.get_edges(20).unwrap().is_empty());
                assert!(db.validate_graph().unwrap().valid);
                db.build_text_index().unwrap();
                assert!(
                    db.tql_values("TEXT BM25 \"oldtoken\" TOP 10 AS hits WITH hits RETURN hits")
                        .unwrap()
                        .is_empty()
                );
                assert!(
                    db.tql_values("TEXT AC \"oldanchor\" TOP 10 AS hits WITH hits RETURN hits")
                        .unwrap()
                        .is_empty()
                );
                assert!(matches!(
                    db.insert(&vector([1., 0., 0., 0.]), json!({"key":"final"})),
                    Err(TriviumError::UniqueConstraintViolation { .. })
                ));
            }
            db.close().unwrap();
        }
    }
    check::<f32>();
    check::<half::f16>();
    check::<u64>();
}

#[test]
fn rejected_batch_does_not_consume_automatic_ids() {
    let dir = TestDir::new();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = dir.file(&format!("allocator-{mode:?}.tdb"));
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 4,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        db.insert_with_id(10, &[1., 0., 0., 0.], json!({"state":"original"}))
            .unwrap();
        let before = std::fs::read(format!("{path}.wal")).unwrap();
        let mut tx = db.begin_tx();
        tx.insert(&[0., 1., 0., 0.], json!({"state":"uncommitted"}));
        tx.delete(10);
        tx.insert_with_id(10, &[0., 1., 0., 0.], json!({"state":"A"}));
        tx.insert_with_id(10, &[0., 0., 1., 0.], json!({"state":"B"}));
        assert!(matches!(
            tx.commit(),
            Err(TriviumError::NodeAlreadyExists(10))
        ));
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
        assert_eq!(db.node_count().unwrap(), 1);
        assert_eq!(
            db.get_payload(10).unwrap(),
            Some(json!({"state":"original"}))
        );
        assert_eq!(db.insert(&[0., 1., 0., 0.], Value::Null).unwrap(), 11);
        db.close().unwrap();
    }
}
