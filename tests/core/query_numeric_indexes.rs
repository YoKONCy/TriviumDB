//! 独立样例验证事务最终键、范围分页、完整整数和索引构建准入。
use crate::support::TestDir;
use serde_json::{Value, json};
use triviumdb::database::{AccessMode, Config, StorageMode, TxBuilder};
use triviumdb::query::tql_executor::TqlValue;
use triviumdb::query::tql_prepared::TqlParamValue;
use triviumdb::{Database, TriviumError};

fn config(mode: StorageMode) -> Config {
    Config {
        dim: 2,
        storage_mode: mode,
        auto_build_quiver: false,
        payload_cache_bytes: 0,
        payload_cache_entry_bytes: 0,
        ..Default::default()
    }
}
fn scalar(value: &TqlValue<f32>) -> Value {
    match value {
        TqlValue::Int(value) => json!(value),
        TqlValue::UInt(value) => json!(value),
        TqlValue::Float(value) => json!(value),
        TqlValue::Null => Value::Null,
        TqlValue::List(values) => Value::Array(values.clone()),
        _ => panic!("样本应返回数值标量"),
    }
}
fn ids(db: &Database<f32>, query: &str) -> Vec<u64> {
    db.tql_nodes(query)
        .unwrap()
        .iter()
        .map(|row| row["_"].id)
        .collect()
}

#[test]
fn 复合唯一键仅反映事务最终状态且wal恢复保留其他所有者() {
    let root = TestDir::new("unique_final_projection");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for ending in ["new", "original", "delete", "chain"] {
            let path = root.file(&format!("{mode:?}_{ending}.tdb"));
            let replay = root.file(&format!("replay_{mode:?}_{ending}.tdb"));
            // 两个独立检查点具有相同事实，回放样本只复制本轮事务 WAL。
            for base in [&path, &replay] {
                let mut db = Database::<f32>::open_with_config(base, config(mode)).unwrap();
                for (id, key) in [(1, "a"), (2, "b"), (3, "c")] {
                    db.insert_with_id(id, &[1., 0.], json!({"tenant":"t","key":key}))
                        .unwrap();
                }
                db.create_unique_composite_index(&["tenant".into(), "key".into()])
                    .unwrap();
                db.close().unwrap();
            }
            let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
            let mut tx = TxBuilder::new();
            tx.update_payload(1, json!({"tenant":"t","key":"b"}));
            if ending == "chain" {
                tx.update_payload(1, json!({"tenant":"t","key":"c"}));
            }
            let final_key = if ending == "original" || ending == "chain" {
                "a"
            } else {
                "d"
            };
            if ending == "delete" {
                tx.delete(1);
            } else {
                tx.update_payload(1, json!({"tenant":"t","key":final_key}));
            }
            db.commit_tx(tx).unwrap();
            std::fs::copy(format!("{path}.wal"), format!("{replay}.wal")).unwrap();
            let mut recovered = Database::<f32>::open_with_config(&replay, config(mode)).unwrap();
            for (instance, base) in [(&mut db, &path), (&mut recovered, &replay)] {
                assert_eq!(
                    instance.get_payload(2).unwrap(),
                    Some(json!({"tenant":"t","key":"b"}))
                );
                assert_eq!(
                    instance.get_payload(3).unwrap(),
                    Some(json!({"tenant":"t","key":"c"}))
                );
                assert_eq!(
                    instance.get_payload(1).unwrap(),
                    if ending == "delete" {
                        None
                    } else {
                        Some(json!({"tenant":"t","key":final_key}))
                    }
                );
                for key in ["b", "c"] {
                    let before = std::fs::read(format!("{base}.wal")).unwrap();
                    let result =
                        instance.insert_with_id(9, &[1., 0.], json!({"tenant":"t","key":key}));
                    assert!(
                        matches!(result, Err(TriviumError::UniqueConstraintViolation { .. })),
                        "{ending}/{key}: {result:?}"
                    );
                    assert_eq!(std::fs::read(format!("{base}.wal")).unwrap(), before);
                }
                instance
                    .insert_with_id(9, &[0., 1.], json!({"tenant":"t","key":"free"}))
                    .unwrap();
                instance.close().unwrap();
            }
            for base in [&path, &replay] {
                let mut reopened = Database::<f32>::open_with_config(base, config(mode)).unwrap();
                assert_eq!(
                    reopened.get_payload(9).unwrap(),
                    Some(json!({"tenant":"t","key":"free"}))
                );
                assert!(matches!(
                    reopened.insert_with_id(10, &[1., 0.], json!({"tenant":"t","key":"b"})),
                    Err(TriviumError::UniqueConstraintViolation { .. })
                ));
                reopened.close().unwrap();
            }
        }
    }
}

#[test]
fn 混合类型范围在limit和offset前排除其他类型候选() {
    let root = TestDir::new("typed_ordered_ranges");
    let cases = [
        (r#"FIND {v: {$lt: "z"}} RETURN * LIMIT 1"#, vec![11]),
        (
            r#"FIND {v: {$lt: "z"}} RETURN * LIMIT 1 OFFSET 1"#,
            vec![12],
        ),
        (r#"FIND {v: {$lte: "a"}} RETURN * LIMIT 1"#, vec![11]),
        (r#"FIND {v: {$gt: "a"}} RETURN * LIMIT 1"#, vec![12]),
        (r#"FIND {v: {$lt: 3}} RETURN * LIMIT 2"#, vec![1, 2]),
        (
            r#"FIND {v: {$gt: 5}} RETURN * ORDER BY _.v DESC LIMIT 2"#,
            vec![10, 9],
        ),
    ];
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        for n in 0..10 {
            db.insert_with_id(n + 1, &[1., 0.], json!({"v":n})).unwrap();
        }
        for (id, value) in [
            (11, json!("a")),
            (12, json!("m")),
            (13, json!("z")),
            (14, Value::Null),
            (15, json!(true)),
            (16, json!([1])),
        ] {
            db.insert_with_id(id, &[1., 0.], json!({"v":value}))
                .unwrap();
        }
        for indexed in [false, true] {
            if indexed {
                db.create_ordered_index("v").unwrap();
            }
            for (query, expected) in &cases {
                assert_eq!(ids(&db, query), *expected, "{mode:?}/{indexed}/{query}");
            }
        }
        db.close().unwrap();
        let mut reader = Database::<f32>::open_with_config(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                ..config(mode)
            },
        )
        .unwrap();
        for (query, expected) in &cases {
            assert_eq!(ids(&reader, query), *expected, "reopen/{query}");
        }
        reader.close().unwrap();
    }
}

#[test]
fn tql投影过滤排序参数与聚合保留完整整数() {
    let root = TestDir::new("tql_unsigned_domain");
    let expected = [
        9_007_199_254_740_993u64,
        i64::MAX as u64,
        1u64 << 63,
        u64::MAX - 2,
    ];
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        for id in expected {
            db.insert_with_id(id, &[1., 0.], json!({"kind":"number","v":id,"w":id+1}))
                .unwrap();
        }
        for query in [
            "MATCH (n) RETURN n.id AS id, n.v AS v ORDER BY n.id ASC",
            r#"FIND {kind:"number"} AS n WITH n RETURN n.id AS id, n.v AS v ORDER BY n.id ASC"#,
        ] {
            let rows = db.tql(query).unwrap();
            assert_eq!(rows.len(), 4);
            for (row, id) in rows.iter().zip(expected) {
                assert_eq!(scalar(&row["id"]), json!(id));
                assert_eq!(scalar(&row["v"]), json!(id));
            }
        }
        assert_eq!(
            db.tql("MATCH (n) WHERE n.id > 0 RETURN n").unwrap().len(),
            4
        );
        assert_eq!(
            db.tql("MATCH (n) WHERE n.v < n.w RETURN n").unwrap().len(),
            4
        );
        assert!(
            db.tql("MATCH (n) WHERE n.v == n.w RETURN n")
                .unwrap()
                .is_empty()
        );
        for id in expected {
            let prepared = db
                .prepare_tql("MATCH (n) WHERE n.id == $id RETURN n.id AS id")
                .unwrap();
            let params = std::collections::HashMap::from([(
                "id".to_owned(),
                TqlParamValue::from_json(&json!(id)).unwrap(),
            )]);
            let rows = db.execute_prepared_tql(&prepared, &params).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(scalar(&rows[0]["id"]), json!(id));
            let rows = db
                .tql(&format!(
                    "MATCH (n) WHERE n.id == {id} RETURN n.id AS id, n.v + 0 AS unchanged"
                ))
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(scalar(&rows[0]["unchanged"]), json!(id));
        }
        let rows = db
            .tql("MATCH (n) RETURN min(n.id) AS lo, max(n.id) AS hi, collect(n.id) AS all")
            .unwrap();
        assert_eq!(scalar(&rows[0]["lo"]), json!(expected[0]));
        assert_eq!(scalar(&rows[0]["hi"]), json!(expected[3]));
        assert_eq!(scalar(&rows[0]["all"]), json!(expected));
        let rows = db
            .tql("MATCH (n) WHERE n.id < 9223372036854775808 RETURN sum(n.id) AS total")
            .unwrap();
        assert_eq!(
            scalar(&rows[0]["total"]),
            json!(9_232_379_236_109_516_800u64)
        );
        for query in [
            "MATCH (n) WHERE n.id < 0 RETURN count(*) AS total_count, sum(n.id) AS total",
            r#"FIND {kind:"absent"} AS n WITH n RETURN count(*) AS total_count, sum(n.id) AS total"#,
        ] {
            let rows = db.tql(query).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(scalar(&rows[0]["total_count"]), json!(0));
            assert_eq!(scalar(&rows[0]["total"]), Value::Null);
        }
        db.close().unwrap();
    }
}

#[test]
fn tql浮点比较与小分母遵守精确比较和有限结果契约() {
    let root = TestDir::new("tql_float_contract");
    let path = root.file("sample.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    // 最小正规数的倒数为 2^1022；最小次正规数的倒数超出有限 f64 域。
    let samples = [
        (1, 1e-20, Some(1e20)),
        (2, -1e-20, Some(-1e20)),
        (3, 0., None),
        (4, -0., None),
        (
            5,
            f64::MIN_POSITIVE,
            Some(f64::from_bits(0x7fd0_0000_0000_0000)),
        ),
        (6, f64::from_bits(1), None),
    ];
    for (id, tiny, _) in samples {
        db.insert_with_id(
            id,
            &[1., 0.],
            json!({"tiny":tiny,"integer":9_007_199_254_740_993i64,"float":9_007_199_254_740_992.0}),
        )
        .unwrap();
    }
    for query in [
        "MATCH (n) WHERE n.tiny > 0 AND n.tiny == 0.0 RETURN n",
        "MATCH (n) WHERE n.tiny < 0 AND n.tiny == 0.0 RETURN n",
        "MATCH (n) WHERE n.integer == n.float RETURN n",
    ] {
        assert!(db.tql(query).unwrap().is_empty(), "{query}");
    }
    assert_eq!(
        db.tql("MATCH (n) WHERE n.integer > n.float RETURN n")
            .unwrap()
            .len(),
        6
    );
    for (id, _, expected) in samples {
        let row = db
            .tql(&format!(
                "MATCH (n) WHERE n.id == {id} RETURN 1.0 / n.tiny AS result"
            ))
            .unwrap();
        assert_eq!(
            scalar(&row[0]["result"]),
            expected.map_or(Value::Null, |value| json!(value))
        );
    }
    let row=db.tql("MATCH (n) RETURN 18446744073709551615 + 1 AS overflow, -9223372036854775808 AS minimum LIMIT 1").unwrap();
    assert_eq!(scalar(&row[0]["overflow"]), Value::Null);
    assert_eq!(scalar(&row[0]["minimum"]), json!(i64::MIN));
    db.insert_with_id(7, &[1., 0.], json!({"integer":1,"float":1.0}))
        .unwrap();
    for indexed in [false, true] {
        if indexed {
            db.create_index("integer").unwrap();
        }
        let rows = db
            .tql("MATCH (n) WHERE n.integer == 1.0 RETURN n.id AS id")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(scalar(&rows[0]["id"]), json!(7));
    }
    db.close().unwrap();
}

#[test]
fn 独立编码的无符号wal样本保留原始事实与嵌套整数() {
    let root = TestDir::new("unsigned_wal_fixture");
    let path = root.file("sample.tdb");
    Database::<f32>::open_with_config(&path, config(StorageMode::Mmap))
        .unwrap()
        .close()
        .unwrap();
    std::fs::write(
        format!("{path}.wal"),
        include_bytes!("../fixtures/numeric_ids/full_u64_v3.bin"),
    )
    .unwrap();
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    let id = u64::MAX - 2;
    assert_eq!(db.all_node_ids().unwrap(), [id]);
    let node = db.get(id).unwrap().unwrap();
    assert_eq!(node.id, id);
    assert_eq!(node.vector, [1., 0.]);
    assert_eq!(
        node.payload,
        json!({"kind":"native_uint","unsigned":u64::MAX,"nested":{"large":id}})
    );
    let rows = db
        .tql("MATCH (n) RETURN n.id AS id, n.unsigned AS value, collect(n.nested) AS nested")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(scalar(&rows[0]["id"]), json!(id));
    assert_eq!(scalar(&rows[0]["value"]), json!(u64::MAX));
    assert_eq!(scalar(&rows[0]["nested"]), json!([{"large":id}]));
    db.close().unwrap();
}

fn create_index(db: &mut Database<f32>, kind: &str) -> triviumdb::Result<()> {
    match kind {
        "hash" => db.create_index("key"),
        "ordered" => db.create_ordered_index("key"),
        "bitmap" => db.create_bitmap_index("key"),
        "composite" => db.create_composite_index(&["tenant".into(), "key".into()]),
        "unique" => db.create_unique_index("key"),
        "unique_composite" => db.create_unique_composite_index(&["tenant".into(), "key".into()]),
        "ngram" => db.create_ngram_index("key"),
        _ => unreachable!(),
    }
}

#[test]
fn 各类索引构建在预算不足时保持事实定义缓存与wal并可重试() {
    let root = TestDir::new("index_ddl_admission");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for kind in [
            "hash",
            "ordered",
            "bitmap",
            "composite",
            "unique",
            "unique_composite",
            "ngram",
        ] {
            let path = root.file(&format!("{mode:?}_{kind}.tdb"));
            let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
            for id in 1..=128 {
                db.insert_with_id(
                    id,
                    &[1., 0.],
                    json!({"tenant":"t","key":format!("value-{id:04}")}),
                )
                .unwrap();
            }
            db.create_index("tenant").unwrap();
            let before_info = serde_json::to_value(db.index_info().unwrap()).unwrap();
            let before = db.estimated_memory();
            // 8 KiB 足以解析单条小记录，整批索引的构建预留会超过该余量。
            db.set_memory_limit(before + 8 * 1024).unwrap();
            let wal = std::fs::read(format!("{path}.wal")).unwrap();
            assert!(
                matches!(
                    create_index(&mut db, kind),
                    Err(TriviumError::CapacityReservationRejected { .. })
                ),
                "{kind}"
            );
            assert_eq!(db.estimated_memory(), before);
            assert_eq!(
                serde_json::to_value(db.index_info().unwrap()).unwrap(),
                before_info
            );
            assert!(db.list_unique_indexes().unwrap().is_empty());
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), wal);
            db.set_memory_limit(16 * 1024 * 1024).unwrap();
            create_index(&mut db, kind).unwrap();
            assert_eq!(db.index_info().unwrap().len(), 2);
            assert!(db.estimated_memory() < 16 * 1024 * 1024);
            assert_eq!(ids(&db, r#"FIND {key:"value-0042"} RETURN *"#), [42]);
            db.update_payload(42, json!({"tenant":"t","key":"replacement"}))
                .unwrap();
            assert!(ids(&db, r#"FIND {key:"value-0042"} RETURN *"#).is_empty());
            assert_eq!(ids(&db, r#"FIND {key:"replacement"} RETURN *"#), [42]);
            db.close().unwrap();
            let mut reader = Database::<f32>::open_with_config(
                &path,
                Config {
                    access_mode: AccessMode::ReadOnly,
                    ..config(mode)
                },
            )
            .unwrap();
            assert_eq!(ids(&reader, r#"FIND {key:"replacement"} RETURN *"#), [42]);
            reader.close().unwrap();
        }
    }
}

#[test]
fn 唯一索引构建发现重复时保留原有非唯一索引定义() {
    let root = TestDir::new("failed_unique_build");
    for composite in [false, true] {
        let path = root.file(&format!("{composite}.tdb"));
        let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
        for id in [1, 2] {
            db.insert_with_id(id, &[1., 0.], json!({"tenant":"t","key":"same"}))
                .unwrap();
        }
        let fields = vec!["tenant".into(), "key".into()];
        if composite {
            db.create_composite_index(&fields).unwrap();
        } else {
            db.create_index("key").unwrap();
        }
        db.flush().unwrap();
        let before_info = serde_json::to_value(db.index_info().unwrap()).unwrap();
        let before_wal = std::fs::read(format!("{path}.wal")).unwrap();
        let failed = if composite {
            db.create_unique_composite_index(&fields)
        } else {
            db.create_unique_index("key")
        };
        assert!(matches!(
            failed,
            Err(TriviumError::UniqueConstraintViolation { .. })
        ));
        assert_eq!(
            serde_json::to_value(db.index_info().unwrap()).unwrap(),
            before_info
        );
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before_wal);
        assert!(db.list_unique_indexes().unwrap().is_empty());
        assert_eq!(ids(&db, r#"FIND {key:"same"} RETURN *"#), [1, 2]);
        db.close().unwrap();
    }
}
