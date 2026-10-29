//! 索引覆盖、数值精度和图写入准入使用固定数据与独立预期验收。
use crate::support::TestDir;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use triviumdb::database::{AccessMode, Config, StorageMode, TxBuilder};
use triviumdb::{Database, TriviumError, VectorType};

fn config(mode: StorageMode) -> Config {
    Config {
        dim: 2,
        storage_mode: mode,
        auto_build_quiver: false,
        payload_cache_bytes: 0,
        ..Default::default()
    }
}

fn ids(db: &Database<f32>, query: &str) -> Vec<u64> {
    let mut ids: Vec<_> = db
        .tql_nodes(query)
        .unwrap()
        .iter()
        .map(|row| row["_"].id)
        .collect();
    ids.sort_unstable();
    ids
}

fn files(path: &str) -> BTreeMap<String, Vec<u8>> {
    let path = std::path::Path::new(path);
    let base = path.file_name().unwrap().to_str().unwrap();
    std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(Result::unwrap)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().unwrap();
            (name == base || name.starts_with(&format!("{base}.")))
                .then(|| (name, std::fs::read(entry.path()).unwrap()))
        })
        .collect()
}

#[test]
fn 跨端wal样本保留完整id向量payload和边() {
    let root = TestDir::new("numeric_fixture_contract");
    let path = root.file("sample.tdb");
    Database::<f32>::open_with_config(&path, config(StorageMode::Mmap))
        .unwrap()
        .close()
        .unwrap();
    std::fs::write(
        format!("{path}.wal"),
        include_bytes!("../fixtures/numeric_ids/wal_v3.bin"),
    )
    .unwrap();
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    let expected = [
        1,
        9_007_199_254_740_991,
        9_007_199_254_740_992,
        9_007_199_254_740_993,
    ];
    let mut actual = db.all_node_ids().unwrap();
    actual.sort_unstable();
    assert_eq!(actual, expected);
    for id in expected {
        let node = db.get(id).unwrap().unwrap();
        assert_eq!(node.id, id);
        assert_eq!(node.vector, [1., 0.]);
        assert_eq!(
            node.payload,
            json!({"group":"fixture","exact_id":id.to_string()})
        );
    }
    let mut edges = db.get_edges(1).unwrap();
    edges.sort_by_key(|edge| edge.target_id);
    assert_eq!(edges.len(), 2);
    for (edge, dst) in edges.iter().zip(&expected[2..]) {
        assert_eq!(edge.target_id, *dst);
        assert_eq!(edge.label, "next");
        assert_eq!(edge.weight, 1.);
        assert_eq!(edge.metadata, Value::Null);
    }
    let incoming = db.get_incoming_edges(1, None).unwrap();
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].source_id, 9_007_199_254_740_993);
    assert_eq!(incoming[0].target_id, 1);
    assert_eq!(incoming[0].label, "back");
    assert_eq!(incoming[0].weight, 1.);
    assert_eq!(incoming[0].metadata, Value::Null);
    db.close().unwrap();
}

#[test]
fn 删除唯一索引同时删除约束并允许重复值持久化() {
    let root = TestDir::new("drop_unique_contract");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        db.insert_with_id(1, &[1., 0.], json!({"email":"same"}))
            .unwrap();
        db.create_unique_index("email").unwrap();
        db.drop_index("email").unwrap();
        assert_eq!(db.list_unique_indexes().unwrap(), Vec::<Vec<String>>::new());
        db.insert_with_id(2, &[0., 1.], json!({"email":"same"}))
            .unwrap();
        assert!(matches!(
            db.create_unique_index("email"),
            Err(TriviumError::UniqueConstraintViolation { .. })
        ));
        assert!(db.list_unique_indexes().unwrap().is_empty());
        db.close().unwrap();
        for access in [AccessMode::ReadOnly, AccessMode::ReadWrite] {
            let mut db = Database::<f32>::open_with_config(
                &path,
                Config {
                    access_mode: access,
                    ..config(mode)
                },
            )
            .unwrap();
            assert_eq!(ids(&db, r#"FIND {email: "same"} RETURN *"#), [1, 2]);
            assert!(db.list_unique_indexes().unwrap().is_empty());
            db.close().unwrap();
        }
    }
}

#[test]
fn 部分覆盖索引在新增修改删除与只读重开后保持完整候选() {
    let root = TestDir::new("partial_index_contract");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for ordered in [false, true] {
            let path = root.file(&format!("{mode:?}_{ordered}.tdb"));
            let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
            // 数字、布尔、空值、缺失字段、容器、字符串均属于 tenant=a 的事实。
            let values = [
                json!(2),
                json!(1),
                json!(true),
                Value::Null,
                json!([1]),
                json!({"x":1}),
                json!("a"),
            ];
            for (i, v) in values.into_iter().enumerate() {
                db.insert_with_id(i as u64 + 1, &[1., 0.], json!({"tenant":"a","v":v}))
                    .unwrap();
            }
            db.insert_with_id(8, &[1., 0.], json!({"tenant":"a"}))
                .unwrap();
            db.insert_with_id(9, &[1., 0.], json!({"tenant":"other","v":0}))
                .unwrap();
            if ordered {
                db.create_ordered_index("v").unwrap();
            } else {
                db.create_composite_index(&["tenant".into(), "v".into()])
                    .unwrap();
            }
            let query = if ordered {
                r#"FIND {tenant: "a"} RETURN * ORDER BY _.v ASC"#
            } else {
                r#"FIND {tenant: "a"} RETURN *"#
            };
            assert_eq!(ids(&db, query), [1, 2, 3, 4, 5, 6, 7, 8]);
            db.update_payload(1, json!({"tenant":"a"})).unwrap();
            db.update_payload(8, json!({"tenant":"a","v":9})).unwrap();
            db.delete(5).unwrap();
            db.insert_with_id(10, &[0., 1.], json!({"tenant":"a"}))
                .unwrap();
            let expected = [1, 2, 3, 4, 6, 7, 8, 10];
            assert_eq!(ids(&db, query), expected);
            // 完整标量等值仍可安全使用复合索引。
            assert_eq!(ids(&db, r#"FIND {tenant: "a", v: 9} RETURN *"#), [8]);
            db.close().unwrap();
            let before = files(&path);
            let mut reader = Database::<f32>::open_with_config(
                &path,
                Config {
                    access_mode: AccessMode::ReadOnly,
                    ..config(mode)
                },
            )
            .unwrap();
            assert_eq!(ids(&reader, query), expected);
            assert_eq!(reader.get_payload(10).unwrap(), Some(json!({"tenant":"a"})));
            reader.close().unwrap();
            assert_eq!(files(&path), before);
            let mut reopened = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
            assert_eq!(ids(&reopened, query), expected);
            reopened.close().unwrap();
        }
    }
}

#[test]
fn 复合唯一约束保留完整标量精度并支持事务交换与重开() {
    let root = TestDir::new("composite_exact_keys");
    let values = [
        json!(9_007_199_254_740_992u64),
        json!(9_007_199_254_740_993u64),
        json!(u64::MAX),
        json!(i64::MIN),
        json!(1),
        json!(1.0),
        json!(true),
        json!("1"),
        Value::Null,
    ];
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        db.create_unique_composite_index(&["tenant".into(), "v".into()])
            .unwrap();
        for (i, v) in values.iter().enumerate() {
            db.insert_with_id(i as u64 + 1, &[1., 0.], json!({"tenant":"a","v":v}))
                .unwrap();
        }
        let wal = std::fs::read(format!("{path}.wal")).unwrap();
        for v in &values {
            assert!(matches!(
                db.insert_with_id(20, &[1., 0.], json!({"tenant":"a","v":v})),
                Err(TriviumError::UniqueConstraintViolation { .. })
            ));
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), wal);
        }
        let mut tx = TxBuilder::new();
        tx.update_payload(1, json!({"tenant":"a","v":values[1]}));
        tx.update_payload(2, json!({"tenant":"a","v":values[0]}));
        db.commit_tx(tx).unwrap();
        for v in &values[..2] {
            assert!(matches!(
                db.insert_with_id(20, &[1., 0.], json!({"tenant":"a","v":v})),
                Err(TriviumError::UniqueConstraintViolation { .. })
            ));
        }
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(
            db.get_payload(1).unwrap(),
            Some(json!({"tenant":"a","v":values[1]}))
        );
        assert!(matches!(
            db.insert_with_id(20, &[1., 0.], json!({"tenant":"a","v":values[0]})),
            Err(TriviumError::UniqueConstraintViolation { .. })
        ));
        db.delete(2).unwrap();
        db.insert_with_id(20, &[1., 0.], json!({"tenant":"a","v":values[0]}))
            .unwrap();
        db.close().unwrap();
    }
}

#[test]
fn 边标签字节上限在所有入口的wal之前检查() {
    let root = TestDir::new("edge_label_contract");
    // u16 长度字段约束 UTF-8 字节；中文标签也需要遵守同一边界。
    let accepted = format!("{}aa", "中".repeat(21_844));
    assert_eq!(accepted.len(), 65_534);
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for operation in ["link", "upsert", "tx"] {
            let path = root.file(&format!("{mode:?}_{operation}.tdb"));
            let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
            for id in [1, 2] {
                db.insert_with_id(id, &[1., 0.], Value::Null).unwrap();
            }
            db.link(1, 2, &accepted, 1.).unwrap();
            db.link(1, 2, &"x".repeat(65_535), 2.).unwrap();
            let label = "中".repeat(21_846);
            let before = std::fs::read(format!("{path}.wal")).unwrap();
            let result = match operation {
                "link" => db.link(1, 2, &label, 1.),
                "upsert" => db.upsert_edge(1, 2, &label, 1., json!({"x":1})),
                _ => {
                    let mut tx = TxBuilder::new();
                    tx.link(1, 2, &label, 1.);
                    db.commit_tx(tx).map(|_| ())
                }
            };
            assert!(
                matches!(result, Err(TriviumError::InvalidInput(_))),
                "{operation}: {result:?}"
            );
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
            assert_eq!(db.get_edges(1).unwrap().len(), 2);
            db.close().unwrap();
            let mut reopened = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
            assert_eq!(
                reopened.get_edge(1, 2, &accepted).unwrap().unwrap().weight,
                1.
            );
            assert_eq!(
                reopened
                    .get_edge(1, 2, &"x".repeat(65_535))
                    .unwrap()
                    .unwrap()
                    .weight,
                2.
            );
            reopened.close().unwrap();
        }
    }
}

#[test]
fn 图标签和metadata增长在直接及事务入口遵守预算() {
    let root = TestDir::new("graph_heap_budget");
    const LIMIT: usize = 256 * 1024;
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for operation in ["label", "metadata", "tx"] {
            let path = root.file(&format!("{mode:?}_{operation}.tdb"));
            let mut db = Database::<f32>::open_with_config(
                &path,
                Config {
                    memory_limit: LIMIT,
                    ..config(mode)
                },
            )
            .unwrap();
            for id in [1, 2] {
                db.insert_with_id(id, &[1., 0.], Value::Null).unwrap();
            }
            let before = std::fs::read(format!("{path}.wal")).unwrap();
            let label = "x".repeat(60_000);
            let result = match operation {
                "label" => db.link(1, 2, &label, 1.),
                "metadata" => {
                    db.upsert_edge(1, 2, "metadata", 1., json!({"text":"x".repeat(100_000)}))
                }
                _ => {
                    let mut tx = TxBuilder::new();
                    for i in 0..5 {
                        tx.link(1, 2, &format!("{i}{label}"), 1.);
                    }
                    db.commit_tx(tx).map(|_| ())
                }
            };
            assert!(
                matches!(
                    result,
                    Err(TriviumError::CapacityReservationRejected { .. })
                ),
                "{operation}: {result:?}"
            );
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
            assert!(db.get_edges(1).unwrap().is_empty());
            db.link(1, 2, "small", 1.).unwrap();
            let before = std::fs::read(format!("{path}.wal")).unwrap();
            assert!(
                db.update_edge(1, 2, "small", None, Some(json!({"text":"x".repeat(LIMIT)})))
                    .is_err()
            );
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
            assert_eq!(
                db.get_edge(1, 2, "small").unwrap().unwrap().metadata,
                Value::Null
            );
            assert!(db.estimated_memory() <= LIMIT);
            db.close().unwrap();
        }
    }
}

#[test]
fn 内存预算占满时无新增容量的删除事务继续完成() {
    let root = TestDir::new("delete_at_budget");
    let path = root.file("sample.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    for id in [1, 2] {
        db.insert_with_id(id, &[1., 0.], json!({"keep":id}))
            .unwrap();
    }
    db.set_memory_limit(db.estimated_memory()).unwrap();
    let mut tx = TxBuilder::new();
    tx.delete(1);
    assert_eq!(db.commit_tx(tx).unwrap(), Vec::<u64>::new());
    assert_eq!(db.all_node_ids().unwrap(), [2]);
    assert_eq!(db.get_payload(2).unwrap(), Some(json!({"keep":2})));
    db.close().unwrap();
    let mut reopened = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), [2]);
    reopened.close().unwrap();
}

#[test]
fn 删除入边后图完整性覆盖平行边自环和节点删除() {
    let root = TestDir::new("graph_degree_contract");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for operation in ["unlink", "label", "delete_source", "delete_target"] {
            let path = root.file(&format!("{mode:?}_{operation}.tdb"));
            let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
            for id in [1, 2] {
                db.insert_with_id(id, &[1., 0.], Value::Null).unwrap();
            }
            db.link(1, 2, "a", 1.).unwrap();
            db.link(1, 2, "b", 2.).unwrap();
            db.link(1, 1, "self", 3.).unwrap();
            match operation {
                "unlink" => {
                    db.unlink(1, 2).unwrap();
                    db.unlink(1, 1).unwrap();
                }
                "label" => {
                    for label in ["a", "b", "self"] {
                        db.unlink_label(1, if label == "self" { 1 } else { 2 }, label)
                            .unwrap();
                    }
                }
                "delete_source" => db.delete(1).unwrap(),
                _ => {
                    db.delete(2).unwrap();
                    db.unlink(1, 1).unwrap();
                }
            }
            let report = db.validate_graph().unwrap();
            assert!(report.valid, "{mode:?}/{operation}: {report:?}");
            assert_eq!(db.graph_stats().unwrap().edge_count, 0);
            db.close().unwrap();
            for access in [AccessMode::ReadOnly, AccessMode::ReadWrite] {
                let mut reopened = Database::<f32>::open_with_config(
                    &path,
                    Config {
                        access_mode: access,
                        ..config(mode)
                    },
                )
                .unwrap();
                assert!(reopened.validate_graph().unwrap().valid);
                assert_eq!(reopened.graph_stats().unwrap().edge_count, 0);
                reopened.close().unwrap();
            }
        }
    }
}

fn id_endpoints<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>() {
    let root = TestDir::new("id_endpoint_contract");
    let vector = [T::from_f32(1.), T::from_f32(0.)];
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let mut db = Database::<T>::open_with_config(&path, config(mode)).unwrap();
        let before = std::fs::read(format!("{path}.wal")).unwrap();
        for id in [0, u64::MAX] {
            assert!(db.insert_with_id(id, &vector, Value::Null).is_err());
            assert!(db.upsert_with_id(id, &vector, Value::Null).is_err());
            let mut tx = TxBuilder::new();
            tx.insert_with_id(id, &vector, Value::Null);
            assert!(db.commit_tx(tx).is_err());
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
        }
        db.insert_with_id(u64::MAX - 1, &vector, json!({"max":u64::MAX}))
            .unwrap();
        let before = std::fs::read(format!("{path}.wal")).unwrap();
        assert!(db.insert(&vector, Value::Null).is_err());
        assert!(db.tql_mut("CREATE (n)").is_err());
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
        let replay = root.file(&format!("replay_{mode:?}.tdb"));
        std::fs::copy(&path, &replay).unwrap();
        std::fs::copy(format!("{path}.wal"), format!("{replay}.wal")).unwrap();
        let mut recovered = Database::<T>::open_with_config(&replay, config(mode)).unwrap();
        assert_eq!(
            recovered.get_payload(u64::MAX - 1).unwrap(),
            Some(json!({"max":u64::MAX}))
        );
        assert_eq!(recovered.all_node_ids().unwrap(), [u64::MAX - 1]);
        recovered.close().unwrap();
        db.close().unwrap();
        let mut reopened = Database::<T>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(
            reopened.get_payload(u64::MAX - 1).unwrap(),
            Some(json!({"max":u64::MAX}))
        );
        reopened.close().unwrap();
    }
}

#[test]
fn 所有dtype在wal前拒绝保留id并保留合法完整u64范围() {
    id_endpoints::<f32>();
    id_endpoints::<half::f16>();
    id_endpoints::<u64>();
}
