//! 写入边界覆盖直接操作、事务、补丁与 WAL 恢复，预期来自明确的数值和 JSON 契约。
use crate::support;

use serde_json::{Value, json};
use triviumdb::database::{Config, StorageMode, TxBuilder};
use triviumdb::{Database, TriviumError, VectorType};

fn nested(depth: usize) -> Value {
    (0..depth).fold(Value::Null, |v, _| Value::Array(vec![v]))
}

fn config(mode: StorageMode, cache: usize) -> Config {
    Config {
        dim: 2,
        storage_mode: mode,
        payload_cache_bytes: cache,
        auto_build_quiver: false,
        ..Default::default()
    }
}

fn json_boundaries<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>() {
    let root = support::TestDir::new("json_boundaries");
    let vector = [T::from_f32(1.0), T::from_f32(0.0)];
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for cache in [0, 1024 * 1024] {
            let path = root.file(&format!("{mode:?}_{cache}.tdb"));
            let cfg = config(mode, cache);
            let mut db = Database::<T>::open_with_config(&path, cfg).unwrap();
            db.insert_with_id(1, &vector, json!({"keep": 1})).unwrap();
            db.insert_with_id(2, &vector, Value::Null).unwrap();
            // 默认解码器允许 127 层容器，128 层必须在任何 WAL 变更前拒绝。
            let accepted = nested(127);
            assert_eq!(
                serde_json::from_str::<Value>(&accepted.to_string()).unwrap(),
                accepted
            );
            assert!(serde_json::from_str::<Value>(&nested(128).to_string()).is_err());
            for operation in [
                "insert",
                "id",
                "update",
                "upsert",
                "patch",
                "cas",
                "tx_insert",
                "tx_update",
                "edge",
            ] {
                let before = std::fs::read(format!("{path}.wal")).unwrap();
                let payload = nested(128);
                let result = match operation {
                    "insert" => db.insert(&vector, payload).map(|_| ()),
                    "id" => db.insert_with_id(3, &vector, payload),
                    "update" => db.update_payload(1, payload),
                    "upsert" => db.upsert_with_id(1, &vector, payload),
                    "patch" => db.patch_payload(1, json!({"$set": {"deep": payload}})),
                    "cas" => db.compare_and_set_payload_field(1, "keep", &json!(1), payload),
                    "edge" => db.upsert_edge(1, 2, "deep", 1.0, payload),
                    _ => {
                        let mut tx = TxBuilder::new();
                        if operation == "tx_insert" {
                            tx.insert(&vector, payload);
                        } else {
                            tx.update_payload(1, payload);
                        }
                        db.commit_tx(tx).map(|_| ())
                    }
                };
                assert!(
                    matches!(result, Err(TriviumError::InvalidInput(_))),
                    "{operation}: {result:?}"
                );
                assert_eq!(
                    std::fs::read(format!("{path}.wal")).unwrap(),
                    before,
                    "{operation}"
                );
                assert_eq!(db.get_payload(1).unwrap(), Some(json!({"keep":1})));
                assert_eq!(db.node_count().unwrap(), 2);
                assert!(db.get_edges(1).unwrap().is_empty());
            }
            db.insert_with_id(3, &vector, accepted.clone()).unwrap();
            assert_eq!(db.get_payload(3).unwrap(), Some(accepted.clone()));
            // 独立空库只靠 WAL 恢复同一合法值，避免 close 的检查点掩盖回放路径。
            let replay_path = root.file(&format!("replay_{mode:?}_{cache}.tdb"));
            std::fs::copy(&path, &replay_path).unwrap();
            std::fs::copy(format!("{path}.wal"), format!("{replay_path}.wal")).unwrap();
            let mut replay = Database::<T>::open_with_config(&replay_path, cfg).unwrap();
            assert_eq!(replay.get_payload(3).unwrap(), Some(accepted.clone()));
            replay.close().unwrap();
            db.close().unwrap();
            let mut reopened = Database::<T>::open_with_config(&path, cfg).unwrap();
            assert_eq!(reopened.get(3).unwrap().unwrap().payload, accepted);
            reopened.close().unwrap();
        }
    }
}

#[test]
fn 所有dtype的json写入边界与读取恢复一致() {
    json_boundaries::<f32>();
    json_boundaries::<half::f16>();
    json_boundaries::<u64>();
}

#[test]
fn 底层memtable同样在修改向量目录和索引前拒绝深层json() {
    let mut mt = triviumdb::storage::memtable::MemTable::<f32>::new(2);
    mt.insert_with_id(1, &[1., 0.], json!({"value":"original"}))
        .unwrap();
    mt.insert_with_id(2, &[0., 1.], Value::Null).unwrap();
    mt.register_property_index("value");
    assert!(matches!(
        mt.insert(&[0., 1.], nested(128)),
        Err(TriviumError::InvalidInput(_))
    ));
    assert!(matches!(
        mt.insert_with_id(9, &[0., 1.], nested(128)),
        Err(TriviumError::InvalidInput(_))
    ));
    assert!(matches!(
        mt.update_payload(1, nested(128)),
        Err(TriviumError::InvalidInput(_))
    ));
    assert!(matches!(
        mt.upsert_edge(1, 2, "edge".into(), 1., nested(128)),
        Err(TriviumError::InvalidInput(_))
    ));
    assert_eq!(mt.node_count(), 2);
    assert_eq!(mt.get_vector(1).unwrap(), [1., 0.]);
    assert_eq!(*mt.get_payload(1).unwrap(), json!({"value":"original"}));
    assert_eq!(
        mt.find_by_property_index("value", &json!("original"))
            .unwrap(),
        [1]
    );
    assert!(mt.get_edges(1).unwrap_or(&[]).is_empty());
    assert_eq!(mt.insert(&[1., 1.], Value::Null).unwrap(), 3);
}

#[test]
fn payload增长在各更新入口遵守预算且失败保持原值和wal() {
    let root = support::TestDir::new("payload_admission");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for operation in [
            "update",
            "patch",
            "cas",
            "upsert",
            "tx",
            "tx_insert_update",
            "metadata",
        ] {
            let path = root.file(&format!("{mode:?}_{operation}.tdb"));
            // 256 KiB 准入账本，2 MiB 字符串显著超过预算且仍低于单 Payload 8 MiB 上限。
            let cfg = Config {
                memory_limit: 256 * 1024,
                ..config(mode, 0)
            };
            let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
            db.insert_with_id(1, &[1., 0.], json!({"data":"small"}))
                .unwrap();
            db.insert_with_id(2, &[0., 1.], Value::Null).unwrap();
            let before = std::fs::read(format!("{path}.wal")).unwrap();
            let payload = json!({"data":"x".repeat(2 * 1024 * 1024)});
            let result = match operation {
                "update" => db.update_payload(1, payload),
                "patch" => db.patch_payload(1, json!({"$set":payload})),
                "cas" => db.compare_and_set_payload_field(
                    1,
                    "data",
                    &json!("small"),
                    payload["data"].clone(),
                ),
                "upsert" => db.upsert_with_id(1, &[0., 1.], payload),
                "metadata" => db.upsert_edge(1, 2, "edge", 1., payload),
                _ => {
                    let mut tx = TxBuilder::new();
                    if operation == "tx_insert_update" {
                        tx.insert_with_id(3, &[1., 0.], Value::Null);
                    }
                    tx.update_payload(1, payload);
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
            assert_eq!(db.get(1).unwrap().unwrap().vector, [1., 0.]);
            assert_eq!(db.get_payload(1).unwrap(), Some(json!({"data":"small"})));
            assert_eq!(db.node_count().unwrap(), 2);
            assert!(db.estimated_memory() <= cfg.memory_limit);
            db.patch_payload(1, json!({"$set":{"data":"fits"}}))
                .unwrap();
            db.close().unwrap();
            let mut reopened = Database::<f32>::open_with_config(&path, cfg).unwrap();
            assert_eq!(
                reopened.get_payload(1).unwrap(),
                Some(json!({"data":"fits"}))
            );
            reopened.close().unwrap();
        }
    }
}

#[test]
fn inc精确整数跨符号边界和小数持久化() {
    let root = support::TestDir::new("inc_exact");
    let path = root.file("numbers.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    for (i, (initial, delta, expected)) in [
        (
            json!(9007199254740992u64),
            json!(1),
            json!(9007199254740993u64),
        ),
        (json!(i64::MAX), json!(1), json!(9223372036854775808u64)),
        (json!(u64::MAX), json!(-1), json!(u64::MAX - 1)),
        (json!(-1), json!(1), json!(0)),
        (json!(0), json!(-1), json!(-1)),
        (json!(1.25), json!(0.5), json!(1.75)),
        (
            json!(9007199254740992u64),
            json!(1.0),
            json!(9007199254740993u64),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let id = i as u64 + 1;
        db.insert_with_id(id, &[1., 0.], json!({"value":initial}))
            .unwrap();
        db.patch_payload(id, json!({"$inc":{"value":delta}}))
            .unwrap();
        assert_eq!(db.get_payload(id).unwrap(), Some(json!({"value":expected})));
    }
    db.close().unwrap();
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    assert_eq!(
        db.get_payload(1).unwrap().unwrap()["value"],
        json!(9007199254740993u64)
    );
    assert_eq!(
        db.get_payload(3).unwrap().unwrap()["value"],
        json!(u64::MAX - 1)
    );
    db.close().unwrap();
}

#[test]
fn inc溢出和类型错误原子拒绝包括同补丁其他字段() {
    let root = support::TestDir::new("inc_rejection");
    let path = root.file("numbers.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    for (initial, delta) in [
        (json!(u64::MAX), json!(1)),
        (json!(i64::MIN), json!(-1)),
        (json!(1e308), json!(1e308)),
        (json!(1), json!("1")),
        (Value::Null, json!(1)),
        (json!("1"), json!(1)),
        (json!(9007199254740993u64), json!(0.5)),
    ] {
        let original = json!({"value":initial, "keep":"original"});
        db.upsert_with_id(1, &[1., 0.], original.clone()).unwrap();
        let before = std::fs::read(format!("{path}.wal")).unwrap();
        let result = db.patch_payload(
            1,
            json!({"$set":{"keep":"changed"}, "$inc":{"value":delta}}),
        );
        assert!(matches!(result, Err(TriviumError::InvalidInput(_))));
        assert_eq!(db.get_payload(1).unwrap(), Some(original));
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    }
    db.patch_payload(1, json!({"$inc":{"new":2}})).unwrap();
    assert_eq!(db.get_payload(1).unwrap().unwrap()["new"], json!(2));
    db.close().unwrap();
}
