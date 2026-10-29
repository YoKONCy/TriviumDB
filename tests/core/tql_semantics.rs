//! 用独立样例验证各查询入口的返回代数与 DML 最终事实。
use crate::support::TestDir;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use triviumdb::database::{AccessMode, Config, RowOverflowPolicy, StorageMode};
use triviumdb::query::tql_executor::{QueryControl, TqlValue};
use triviumdb::{Database, TriviumError};

fn config(mode: StorageMode) -> Config {
    Config {
        dim: 1,
        storage_mode: mode,
        auto_build_quiver: false,
        payload_cache_bytes: 0,
        payload_cache_entry_bytes: 0,
        ..Default::default()
    }
}
fn scalar(v: &TqlValue<f32>) -> Value {
    match v {
        TqlValue::Int(v) => json!(v),
        TqlValue::UInt(v) => json!(v),
        TqlValue::Float(v) => json!(v),
        TqlValue::String(v) => json!(v),
        TqlValue::Bool(v) => json!(v),
        TqlValue::Null => Value::Null,
        TqlValue::List(v) => json!(v),
        other => panic!("此样本要求标量，实际为 {other:?}"),
    }
}
fn rows(db: &Database<f32>, query: &str) -> Vec<BTreeMap<String, Value>> {
    db.tql(query)
        .unwrap_or_else(|e| panic!("{query}: {e:?}"))
        .iter()
        .map(|row| row.iter().map(|(k, v)| (k.clone(), scalar(v))).collect())
        .collect()
}
fn fixture(path: &str, mode: StorageMode) -> Database<f32> {
    let mut db = Database::<f32>::open_with_config(path, config(mode)).unwrap();
    for (id, x, g, k) in [
        (1, 3, "a", Some(json!(8))),
        (2, 1, "b", Some(Value::Null)),
        (3, 2, "b", None),
    ] {
        let mut p = json!({"kind":"sample","x":x,"g":g});
        if let Some(k) = k {
            p["k"] = k;
        }
        db.insert_with_id(id, &[1.], p).unwrap();
    }
    db.insert_with_id(99, &[0.], json!({"kind":"root"}))
        .unwrap();
    for id in 1..=3 {
        db.link(99, id, "item", id as f32).unwrap();
    }
    db
}
fn sources() -> [(&'static str, &'static str); 5] {
    [
        ("MATCH (n {kind: \"sample\"})", "n"),
        ("FIND {kind: \"sample\"}", "_"),
        ("MATCH (n {kind: \"sample\"}) AS n WITH n", "n"),
        ("FIND {kind: \"sample\"} AS n WITH n", "n"),
        ("MATCH (root)-[edge:item]->(n)", "n"),
    ]
}
fn expect(db: &Database<f32>, query: &str, expected: Value) {
    assert_eq!(
        serde_json::to_value(rows(db, query)).unwrap(),
        expected,
        "{query}"
    );
}

#[test]
fn 计算表达式与数值相等的distinct在各入口一致() {
    let root = TestDir::new("tql_distinct_values");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let mut db = fixture(&root.file(&format!("{mode:?}.tdb")), mode);
        for (source, n) in sources() {
            expect(
                &db,
                &format!("{source} RETURN DISTINCT {n}.x + 1 AS v ORDER BY v ASC"),
                json!([{"v":2},{"v":3},{"v":4}]),
            );
            expect(
                &db,
                &format!("{source} RETURN DISTINCT {n}.g AS v ORDER BY v ASC"),
                json!([{"v":"a"},{"v":"b"}]),
            );
        }
        db.update_payload(1, json!({"kind":"sample","x":1}))
            .unwrap();
        db.update_payload(2, json!({"kind":"sample","x":1.0}))
            .unwrap();
        db.update_payload(3, json!({"kind":"sample","x":2}))
            .unwrap();
        for (source, n) in sources() {
            expect(
                &db,
                &format!("{source} RETURN DISTINCT {n}.x AS v ORDER BY v ASC"),
                json!([{"v":1},{"v":2}]),
            );
        }
        db.close().unwrap();
    }
}

#[test]
fn 节点id与计算表达式构成完整分组键() {
    let root = TestDir::new("tql_group_keys");
    let mut db = fixture(&root.file("sample.tdb"), StorageMode::Mmap);
    for (source, n) in sources() {
        expect(
            &db,
            &format!("{source} RETURN {n}.id AS ident, COUNT(*) AS total ORDER BY ident ASC"),
            json!([{"ident":1,"total":1},{"ident":2,"total":1},{"ident":3,"total":1}]),
        );
        expect(
            &db,
            &format!("{source} RETURN {n}.x + 1 AS v, COUNT(*) AS total ORDER BY v ASC"),
            json!([{"v":2,"total":1},{"v":3,"total":1},{"v":4,"total":1}]),
        );
    }
    db.close().unwrap();
}

#[test]
fn count求值字面量缺失空值与有效假值() {
    let root = TestDir::new("tql_count_expr");
    let mut db = fixture(&root.file("sample.tdb"), StorageMode::Rom);
    for (source, n) in sources() {
        expect(
            &db,
            &format!(
                "{source} RETURN COUNT(1) AS literal, COUNT({n}.k) AS field, COUNT(NULL) AS absent"
            ),
            json!([{"literal":3,"field":1,"absent":0}]),
        );
        expect(
            &db,
            &format!("{source} RETURN COUNT(COALESCE({n}.k, 1)) AS total"),
            json!([{"total":3}]),
        );
    }
    for (id, k) in [(1, json!(false)), (2, json!(0)), (3, json!(""))] {
        db.update_payload(id, json!({"kind":"sample","k":k}))
            .unwrap();
    }
    expect(
        &db,
        "MATCH (n {kind: \"sample\"}) RETURN COUNT(n.k) AS total",
        json!([{"total":3}]),
    );
    expect(
        &db,
        "MATCH (n {kind: \"missing\"}) RETURN COUNT(1) AS total",
        json!([{"total":0}]),
    );
    db.close().unwrap();
}

#[test]
fn 聚合默认列名及同名别名保留独立绑定() {
    let root = TestDir::new("tql_aggregate_names");
    let mut db = fixture(&root.file("sample.tdb"), StorageMode::Mmap);
    for (source, n) in sources() {
        expect(
            &db,
            &format!("{source} RETURN COUNT(*)"),
            json!([{"Count(*)":3}]),
        );
        expect(
            &db,
            &format!("{source} RETURN {n}.g AS g, COUNT(*) AS {n} ORDER BY {n}.g ASC"),
            json!([{ "g":"a",n:1},{"g":"b",n:2}]),
        );
    }
    db.close().unwrap();
}

#[test]
fn 聚合distinct与排序先于分页() {
    let root = TestDir::new("tql_aggregate_order");
    let mut db = fixture(&root.file("sample.tdb"), StorageMode::Rom);
    for (source, n) in sources() {
        expect(
            &db,
            &format!("{source} RETURN DISTINCT COUNT({n}.g) AS total"),
            json!([{"total":2}]),
        );
        expect(
            &db,
            &format!("{source} RETURN {n}.g AS g, COUNT(*) AS total ORDER BY {n}.g DESC LIMIT 1"),
            json!([{"g":"b","total":2}]),
        );
        expect(
            &db,
            &format!(
                "{source} RETURN {n}.g AS g, COUNT(*) AS total ORDER BY total DESC LIMIT 1 OFFSET 1"
            ),
            json!([{"g":"a","total":1}]),
        );
    }
    db.close().unwrap();
}

#[test]
fn return别名及嵌套别名算术参与排序() {
    let root = TestDir::new("tql_order_alias");
    let mut db = fixture(&root.file("sample.tdb"), StorageMode::Mmap);
    for (source, n) in sources() {
        expect(
            &db,
            &format!("{source} RETURN {n}.x AS v ORDER BY v DESC"),
            json!([{"v":3},{"v":2},{"v":1}]),
        );
        expect(
            &db,
            &format!("{source} RETURN {n}.x + 1 AS v ORDER BY v * -1 ASC LIMIT 2"),
            json!([{"v":4},{"v":3}]),
        );
    }
    db.close().unwrap();
}

#[test]
fn 具名边保留平行边计数及实际返回列() {
    let root = TestDir::new("tql_named_edges");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Mmap))
            .unwrap();
    for id in [1, 2] {
        db.insert_with_id(id, &[1.], json!({})).unwrap();
    }
    db.link(1, 2, "a", 1.).unwrap();
    db.link(1, 2, "b", 2.).unwrap();
    db.link(1, 1, "loop", 3.).unwrap();
    expect(
        &db,
        "MATCH (a)-[e]->(b) RETURN COUNT(*) AS total",
        json!([{"total":3}]),
    );
    expect(
        &db,
        "MATCH (a)-[e]->(b) RETURN COUNT(*) AS total LIMIT 1",
        json!([{"total":3}]),
    );
    expect(
        &db,
        "MATCH (a)-[e]->(b) RETURN e.label AS label, COUNT(e.weight) AS total ORDER BY label ASC",
        json!([{"label":"a","total":1},{"label":"b","total":1},{"label":"loop","total":1}]),
    );
    let nodes = db
        .tql_nodes("MATCH (a)-[e]->(b) RETURN COUNT(*) AS total LIMIT 1")
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["total"].payload, json!({"total":3}));
    expect(
        &db,
        "MATCH (a)-[e]->(b) RETURN b.id AS target, e.label AS label ORDER BY e.weight DESC",
        json!([{"target":1,"label":"loop"},{"target":2,"label":"b"},{"target":2,"label":"a"}]),
    );
    expect(
        &db,
        "MATCH (a)-[e:loop]-(b) RETURN COUNT(*) AS total",
        json!([{"total":1}]),
    );
    let rows = db.tql("MATCH (a)-[e:b]->(b) RETURN e AS edge").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 1);
    let TqlValue::Edge(edge) = &rows[0]["edge"] else {
        panic!("应返回实际边值")
    };
    assert_eq!(
        (
            edge.source_id,
            edge.target_id,
            edge.label.as_str(),
            edge.weight
        ),
        (1, 2, "b", 2.)
    );
    db.close().unwrap();
}

#[test]
fn with与普通入口遵守行预算偏移预算和取消() {
    let root = TestDir::new("tql_pipeline_limits");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let mut cfg = config(mode);
        cfg.max_query_rows = Some(2);
        cfg.memory_limit = 64 * 1024;
        let mut db =
            Database::<f32>::open_with_config(&root.file(&format!("{mode:?}.tdb")), cfg).unwrap();
        for id in 1..=3 {
            db.insert_with_id(id, &[1.], json!({"x":id})).unwrap();
        }
        for source in [
            "MATCH (n)",
            "MATCH (n) AS n WITH n",
            "FIND {x: {$exists: true}} AS n WITH n",
        ] {
            assert!(
                matches!(
                    db.tql(&format!("{source} RETURN n")),
                    Err(TriviumError::QueryRowBudgetExceeded { budget: 2 })
                ),
                "{source}"
            );
            expect(
                &db,
                &format!("{source} RETURN n.id AS id LIMIT 3"),
                json!([{"id":1},{"id":2},{"id":3}]),
            );
            assert!(
                matches!(
                    db.tql(&format!("{source} RETURN n LIMIT 2 OFFSET 10000")),
                    Err(TriviumError::QueryRowBudgetExceeded { .. })
                ),
                "{source}"
            );
            let cancelled = QueryControl::default();
            cancelled.cancel();
            assert!(matches!(
                db.tql_with_control(&format!("{source} RETURN n LIMIT 3"), cancelled),
                Err(TriviumError::QueryCancelled)
            ));
        }
        db.close().unwrap();
    }
    let mut cfg = config(StorageMode::Rom);
    cfg.max_query_rows = Some(2);
    cfg.row_overflow = RowOverflowPolicy::Break;
    let mut db = Database::<f32>::open_with_config(&root.file("partial.tdb"), cfg).unwrap();
    for id in 1..=3 {
        db.insert_with_id(id, &[1.], json!({})).unwrap();
    }
    expect(
        &db,
        "MATCH (n) AS n WITH n RETURN n.id AS id",
        json!([{"id":1},{"id":2}]),
    );
    assert!(
        db.tql("MATCH (n) AS n WITH n RETURN COUNT(*) AS total")
            .is_err()
    );
    db.close().unwrap();
}

#[test]
fn 多字段set按节点和赋值顺序合并并持久化() {
    let root = TestDir::new("tql_set_fields");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        db.insert_with_id(1, &[1.], json!({"a":0,"b":0,"keep":true}))
            .unwrap();
        db.link(1, 1, "self", 1.).unwrap();
        db.tql_mut("MATCH (a)-[:self]->(b) SET a.a == 1, b.b == 2, a.a == 3")
            .unwrap();
        assert_eq!(
            db.get_payload(1).unwrap(),
            Some(json!({"a":3,"b":2,"keep":true}))
        );
        db.close().unwrap();
        let mut reader = Database::<f32>::open_with_config(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                ..config(mode)
            },
        )
        .unwrap();
        assert_eq!(
            reader.get_payload(1).unwrap(),
            Some(json!({"a":3,"b":2,"keep":true}))
        );
        reader.close().unwrap();
    }
}

#[test]
fn set_u64向量保持完整整数并在非法分量时保留wal和所有目标() {
    let root = TestDir::new("tql_set_u64");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let mut db = Database::<u64>::open_with_config(&path, config(mode)).unwrap();
        for id in [1, 2] {
            db.insert_with_id(id, &[7], json!({})).unwrap();
        }
        for expected in [
            16_777_217,
            9_007_199_254_740_993,
            1u64 << 63,
            u64::MAX - 2,
            u64::MAX,
        ] {
            db.tql_mut(&format!("MATCH (n) SET VECTOR(n) == [{expected}]"))
                .unwrap();
            for id in [1, 2] {
                assert_eq!(db.get(id).unwrap().unwrap().vector, [expected]);
            }
        }
        for literal in ["-1", "1.5", "18446744073709551616", "1e20"] {
            let before = std::fs::read(format!("{path}.wal")).unwrap();
            assert!(
                db.tql_mut(&format!("MATCH (n) SET VECTOR(n) == [{literal}]"))
                    .is_err(),
                "{literal}"
            );
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
            for id in [1, 2] {
                assert_eq!(db.get(id).unwrap().unwrap().vector, [u64::MAX]);
            }
        }
        db.close().unwrap();
        let mut db = Database::<u64>::open_with_config(&path, config(mode)).unwrap();
        for id in [1, 2] {
            assert_eq!(db.get(id).unwrap().unwrap().vector, [u64::MAX]);
        }
        db.close().unwrap();
    }
}

#[test]
fn match_create按完整匹配集建立独立节点及边作用域() {
    let root = TestDir::new("tql_create_rows");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let mut cfg = config(mode);
        cfg.max_query_rows = Some(1);
        let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
        let result = db
            .tql_mut("MATCH (n {kind: \"seed\"}) CREATE (n)-[:copy]->(a {kind: \"a\"})")
            .unwrap();
        assert_eq!((result.affected, result.created_ids), (0, vec![]));
        for id in 1..=3 {
            db.insert_with_id(id, &[1.], json!({"kind":"seed"}))
                .unwrap();
        }
        let result=db.tql_mut("MATCH (n {kind: \"seed\"}) CREATE (n)-[:copy]->(a {kind: \"a\"})-[:next]->(b {kind: \"b\"})").unwrap();
        assert_eq!(result.created_ids, [4, 5, 6, 7, 8, 9]);
        assert_eq!(result.affected, 12);
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        let actual=db.tql("MATCH (s)-[:copy]->(a)-[:next]->(b) RETURN s.id AS s, a.id AS a, b.id AS b ORDER BY s.id ASC").unwrap();
        let values = actual
            .iter()
            .map(|r| (scalar(&r["s"]), scalar(&r["a"]), scalar(&r["b"])))
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            [
                (json!(1), json!(4), json!(5)),
                (json!(2), json!(6), json!(7)),
                (json!(3), json!(8), json!(9))
            ]
        );
        let kinds = db
            .all_node_ids()
            .unwrap()
            .into_iter()
            .map(|id| (id, db.get_payload(id).unwrap().unwrap()["kind"].clone()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            kinds,
            BTreeMap::from([
                (1, json!("seed")),
                (2, json!("seed")),
                (3, json!("seed")),
                (4, json!("a")),
                (5, json!("b")),
                (6, json!("a")),
                (7, json!("b")),
                (8, json!("a")),
                (9, json!("b"))
            ])
        );
        db.close().unwrap();
    }
}

#[test]
fn 多匹配create约束失败时整批拒绝() {
    let root = TestDir::new("tql_create_atomic");
    let path = root.file("sample.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    for id in [1, 2] {
        db.insert_with_id(id, &[1.], json!({"kind":"seed"}))
            .unwrap();
    }
    db.create_unique_index("key").unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    assert!(matches!(
        db.tql_mut("MATCH (n {kind: \"seed\"}) CREATE (m {key: \"duplicate\"})"),
        Err(TriviumError::UniqueConstraintViolation { .. })
    ));
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert_eq!(db.all_node_ids().unwrap(), [1, 2]);
    db.close().unwrap();
}

#[test]
fn 三端共享返回样例逐项验证完整结果() {
    let contract: Value =
        serde_json::from_str(include_str!("../contracts/tql_return_cases.json")).unwrap();
    let root = TestDir::new("tql_shared_returns");
    let mut db = fixture(&root.file("sample.tdb"), StorageMode::Mmap);
    for source in contract["sources"].as_array().unwrap() {
        for case in contract["cases"].as_array().unwrap() {
            let query = format!(
                "{} {}",
                source["query"].as_str().unwrap(),
                case["suffix"]
                    .as_str()
                    .unwrap()
                    .replace("{var}", source["variable"].as_str().unwrap())
            );
            expect(&db, &query, case["rows"].clone());
        }
    }
    db.close().unwrap();
}

#[test]
fn 浮点向量转换在目标dtype溢出前拒绝且保留原值() {
    fn check<T: triviumdb::VectorType + serde::Serialize + serde::de::DeserializeOwned>(
        path: &str,
        bad: &str,
    ) {
        let mut db = Database::<T>::open_with_config(path, config(StorageMode::Mmap)).unwrap();
        db.insert_with_id(1, &[T::from_f32(7.)], json!({})).unwrap();
        db.tql_mut("MATCH (n) SET VECTOR(n) == [2.5]").unwrap();
        assert_eq!(db.get(1).unwrap().unwrap().vector[0].to_f32(), 2.5);
        let before = std::fs::read(format!("{path}.wal")).unwrap();
        assert!(matches!(
            db.tql_mut(&format!("MATCH (n) SET VECTOR(n) == [{bad}]")),
            Err(TriviumError::InvalidVector { .. })
        ));
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
        assert_eq!(db.get(1).unwrap().unwrap().vector[0].to_f32(), 2.5);
        db.close().unwrap();
    }
    let root = TestDir::new("tql_float_conversion");
    check::<f32>(&root.file("f32.tdb"), "1e100");
    check::<half::f16>(&root.file("f16.tdb"), "65520");
}

#[test]
fn return别名仅进入排序作用域且节点别名可排序() {
    let root = TestDir::new("tql_return_scope");
    let mut db = fixture(&root.file("sample.tdb"), StorageMode::Mmap);
    for query in [
        "MATCH (n) AS n WITH n WHERE v > 0 RETURN n.x AS v",
        "MATCH (n) AS n WITH n RETURN v AS v",
        "MATCH (n) AS n WITH n RETURN n.x AS v ORDER BY absent",
    ] {
        assert!(
            matches!(db.tql(query), Err(TriviumError::QueryParse(_))),
            "{query}"
        );
    }
    for (source, n) in sources() {
        let query = format!("{source} RETURN {n} AS object ORDER BY object.x DESC");
        let rows = db.tql(&query).unwrap();
        let ids = rows
            .iter()
            .map(|row| row["object"].as_node().unwrap().id)
            .collect::<Vec<_>>();
        assert_eq!(ids, [1, 3, 2], "{query}");
    }
    db.close().unwrap();
}

#[test]
fn 纯向量管线在最终分页后物化所需节点() {
    let root = TestDir::new("tql_late_return");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Mmap))
            .unwrap();
    for id in 1..=64 {
        db.insert_with_id(id, &[1.], json!({"value":id})).unwrap();
    }
    let query = "SEARCH VECTOR [1] TOP 8 AS n WITH n RETURN n LIMIT 1";
    let before = db.payload_memory_stats();
    let rows = db.tql(query).unwrap();
    let after = db.payload_memory_stats();
    assert_eq!(rows.len(), 1);
    let node = rows[0]["n"].as_node().unwrap();
    assert_eq!((node.id, node.payload.clone()), (1, json!({"value":1})));
    assert_eq!(after.payload_lookups - before.payload_lookups, 1);
    let before = db.payload_memory_stats();
    let nodes = db.tql_nodes(query).unwrap();
    let after = db.payload_memory_stats();
    assert_eq!((nodes.len(), nodes[0]["n"].id), (1, 1));
    assert_eq!(after.payload_lookups - before.payload_lookups, 1);
    db.close().unwrap();
}

#[test]
fn set字段和完整整数向量在未检查点wal回放后保持事实() {
    let root = TestDir::new("tql_set_replay");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let source = root.file(&format!("{mode:?}_source.tdb"));
        let replay = root.file(&format!("{mode:?}_replay.tdb"));
        for path in [&source, &replay] {
            let mut db = Database::<u64>::open_with_config(path, config(mode)).unwrap();
            for id in [1, 2] {
                db.insert_with_id(id, &[7], json!({"a":0,"b":0,"kind":"persist"}))
                    .unwrap();
            }
            db.close().unwrap();
        }
        let mut db = Database::<u64>::open_with_config(&source, config(mode)).unwrap();
        db.tql_mut("MATCH (n) SET n.a == 1, n.b == 2").unwrap();
        db.tql_mut("MATCH (n) SET VECTOR(n) == [9007199254740993]")
            .unwrap();
        std::fs::copy(format!("{source}.wal"), format!("{replay}.wal")).unwrap();
        db.close().unwrap();
        let mut recovered = Database::<u64>::open_with_config(&replay, config(mode)).unwrap();
        for id in [1, 2] {
            let node = recovered.get(id).unwrap().unwrap();
            assert_eq!(node.vector, [9_007_199_254_740_993]);
            assert_eq!(node.payload, json!({"a":1,"b":2,"kind":"persist"}));
        }
        recovered.close().unwrap();
    }
}

#[test]
fn set遇到非对象目标时整批拒绝并保留wal() {
    let root = TestDir::new("tql_set_invalid_target");
    let path = root.file("sample.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    db.insert_with_id(1, &[1.], json!({"a":0})).unwrap();
    db.insert_with_id(2, &[1.], json!([])).unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    assert!(matches!(
        db.tql_mut("MATCH (n) SET n.a == 1"),
        Err(TriviumError::InvalidInput(_))
    ));
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert_eq!(db.get_payload(1).unwrap(), Some(json!({"a":0})));
    assert_eq!(db.get_payload(2).unwrap(), Some(json!([])));
    db.close().unwrap();
}
