//! ID、可选绑定、JSON 值、列名与检索向量的独立边界样例。
use crate::support::TestDir;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use triviumdb::database::{Config, StorageMode};
use triviumdb::query::tql_executor::TqlValue;
use triviumdb::query::tql_prepared::TqlParamValue;
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
fn json_value(value: &TqlValue<f32>) -> Value {
    match value {
        TqlValue::Int(v) => json!(v),
        TqlValue::UInt(v) => json!(v),
        TqlValue::Float(v) => json!(v),
        TqlValue::String(v) => json!(v),
        TqlValue::Bool(v) => json!(v),
        TqlValue::List(v) => json!(v),
        TqlValue::Null => Value::Null,
        TqlValue::Node(v) => json!({"node":v.id}),
        TqlValue::Edge(v) => {
            json!({"source":v.source_id,"target":v.target_id,"label":v.label,"weight":v.weight})
        }
        TqlValue::Path(v) => json!(v),
        TqlValue::Object(v) => json!(v),
    }
}
fn rows(db: &Database<f32>, query: &str) -> Value {
    let rows = db
        .tql(query)
        .unwrap_or_else(|error| panic!("{query}: {error:?}"));
    json!(
        rows.iter()
            .map(|row| row
                .iter()
                .map(|(key, value)| (key.clone(), json_value(value)))
                .collect::<BTreeMap<_, _>>())
            .collect::<Vec<_>>()
    )
}

#[test]
fn 内联id过滤保持完整无符号域且非法id不会写错节点() {
    let root = TestDir::new("tql_structural_ids");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        let large = 18_446_744_073_709_551_613u64;
        for id in [1, 9_007_199_254_740_993, large] {
            db.insert_with_id(id, &[1.], json!({"id":-3,"keep":true}))
                .unwrap();
        }
        for indexed in [false, true] {
            if indexed {
                db.create_index("id").unwrap();
            }
            for literal in ["-3", "-1", "0", "1.5", "true", "\"1\""] {
                assert_eq!(
                    rows(
                        &db,
                        &format!("MATCH (n {{id:{literal}}}) RETURN n.id AS id")
                    ),
                    json!([]),
                    "{indexed}/{literal}"
                );
            }
            for id in [1, 9_007_199_254_740_993, large] {
                assert_eq!(
                    rows(&db, &format!("MATCH (n {{id:{id}}}) RETURN n.id AS id")),
                    json!([{"id":id}])
                );
            }
            assert_eq!(
                rows(
                    &db,
                    "MATCH (n {id:{$in:[1,18446744073709551613]}}) RETURN n.id AS id ORDER BY id ASC"
                ),
                json!([{"id":1},{"id":large}])
            );
            assert_eq!(
                rows(
                    &db,
                    "MATCH (n {id:{$gte:9007199254740993}}) RETURN n.id AS id ORDER BY id ASC"
                ),
                json!([{"id":9_007_199_254_740_993u64},{"id":large}])
            );
        }
        let before = std::fs::read(format!("{path}.wal")).unwrap();
        assert_eq!(
            db.tql_mut("MATCH (n {id:-3}) SET n.keep == false")
                .unwrap()
                .affected,
            0
        );
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
        db.tql_mut("MATCH (n {id:18446744073709551613}) SET n.mark == true")
            .unwrap();
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(
            db.get_payload(large).unwrap(),
            Some(json!({"id":-3,"keep":true,"mark":true}))
        );
        assert_eq!(
            db.get_payload(1).unwrap(),
            Some(json!({"id":-3,"keep":true}))
        );
        db.close().unwrap();
    }
}

#[test]
fn optional仅为空缺后缀补空且索引只影响访问路径() {
    let root = TestDir::new("tql_optional_anchors");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Mmap))
            .unwrap();
    for (id, x) in [(1, 1), (2, 2), (3, 1), (4, 3)] {
        db.insert_with_id(id, &[1.], json!({"x":x})).unwrap();
    }
    db.link(1, 4, "rel", 1.).unwrap();
    for indexed in [false, true] {
        if indexed {
            db.create_index("x").unwrap();
        }
        assert_eq!(
            rows(
                &db,
                "OPTIONAL MATCH (a {x:1})-[:rel]->(b) RETURN a.id AS a, b.id AS b ORDER BY a.id ASC"
            ),
            json!([{"a":1,"b":4},{"a":3,"b":null}]),
            "{indexed}"
        );
        assert_eq!(
            rows(
                &db,
                "OPTIONAL MATCH (a {x:99})-[:rel]->(b) RETURN a.id AS a"
            ),
            json!([])
        );
    }
    db.close().unwrap();
}

#[test]
fn 边绑定在过滤表达式和可选返回中一致() {
    let root = TestDir::new("tql_edge_bindings");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Mmap))
            .unwrap();
    for (id, kind) in [(1, "start"), (2, "target"), (3, "start")] {
        db.insert_with_id(id, &[1.], json!({"kind":kind})).unwrap();
    }
    db.link(1, 2, "rel", 2.).unwrap();
    assert_eq!(
        rows(
            &db,
            "MATCH (a)-[e:rel]->(b) WHERE e.weight > 1 RETURN b.id AS id, e IS NULL AS absent, e IS NOT NULL AS present"
        ),
        json!([{"id":2,"absent":false,"present":true}])
    );
    assert_eq!(
        rows(
            &db,
            "OPTIONAL MATCH (a {kind:\"start\"})-[e:rel]->(b) RETURN a.id AS id, e AS edge ORDER BY id ASC"
        ),
        json!([{"id":1,"edge":{"source":1,"target":2,"label":"rel","weight":2.0}},{"id":3,"edge":null}])
    );
    assert_eq!(
        rows(
            &db,
            "OPTIONAL MATCH (a {kind:\"start\"})-[e:rel]->(b) WHERE e.weight > 3 RETURN a.id AS id, e IS NULL AS absent ORDER BY id ASC"
        ),
        json!([{"id":1,"absent":true},{"id":3,"absent":true}])
    );
    db.close().unwrap();
}

#[test]
fn json容器与空值在所有表达式入口中保持类型() {
    let root = TestDir::new("tql_json_runtime");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Rom))
            .unwrap();
    for (id, arr, obj) in [
        (1, json!([]), json!({})),
        (2, json!([1, 2]), json!({"x":1})),
    ] {
        db.insert_with_id(id, &[1.], json!({"kind":"sample","arr":arr,"obj":obj}))
            .unwrap();
    }
    for (source, n) in [
        ("MATCH (n)", "n"),
        ("FIND {kind:\"sample\"}", "_"),
        ("MATCH (n) AS n WITH n", "n"),
    ] {
        assert_eq!(
            rows(
                &db,
                &format!(
                    "{source} RETURN {n}.arr IS NULL AS a, {n}.obj IS NULL AS o ORDER BY {n}.id ASC"
                )
            ),
            json!([{"a":false,"o":false},{"a":false,"o":false}])
        );
        assert_eq!(
            rows(
                &db,
                &format!(
                    "{source} RETURN COUNT({n}.arr) AS direct, COUNT(COALESCE({n}.arr,NULL)) AS indirect"
                )
            ),
            json!([{"direct":2,"indirect":2}])
        );
        assert_eq!(
            rows(
                &db,
                &format!("{source} RETURN COALESCE({n}.arr,NULL) AS arr ORDER BY {n}.id ASC")
            ),
            json!([{"arr":[]},{"arr":[1,2]}])
        );
    }
    db.close().unwrap();
}

#[test]
fn 默认列名区分字面量且重复列显式拒绝() {
    let root = TestDir::new("tql_column_names");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Mmap))
            .unwrap();
    db.insert_with_id(1, &[1.], json!({})).unwrap();
    assert_eq!(rows(&db, "MATCH (n) RETURN 1, 2"), json!([{"1":1,"2":2}]));
    for query in [
        "MATCH (n) RETURN 1 AS x, 2 AS x",
        "MATCH (n) RETURN 1, 1",
        "MATCH (n) AS n WITH n RETURN n, n",
    ] {
        assert!(
            matches!(db.tql(query), Err(TriviumError::QuerySyntax { .. })),
            "{query}"
        );
    }
    db.close().unwrap();
}

#[test]
fn u64检索字面量参数及rank保持原始位模式() {
    let root = TestDir::new("tql_u64_search");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let mut db =
            Database::<u64>::open_with_config(&root.file(&format!("{mode:?}.tdb")), config(mode))
                .unwrap();
        db.insert_with_id(1, &[16_777_216], json!({})).unwrap();
        db.insert_with_id(2, &[16_777_217], json!({})).unwrap();
        db.insert_with_id(3, &[9_007_199_254_740_993], json!({}))
            .unwrap();
        db.insert_with_id(4, &[u64::MAX], json!({})).unwrap();
        for (literal, id) in [
            ("16777217", 2),
            ("9007199254740993", 3),
            ("18446744073709551615", 4),
        ] {
            for query in [
                format!("SEARCH VECTOR [{literal}] TOP 1 RETURN *"),
                format!("SEARCH VECTOR [{literal}] TOP 1 AS n WITH n RETURN n"),
                format!("MATCH (n) RANK n BY VECTOR [{literal}] TOP 1 RETURN n"),
            ] {
                let rows = db.tql(&query).unwrap();
                assert_eq!(rows.len(), 1, "{query}");
                let node = rows[0].values().next().unwrap().as_node().unwrap();
                assert_eq!(node.id, id, "{query}");
            }
            let prepared = db
                .prepare_tql("SEARCH VECTOR [$q] TOP 1 AS n WITH n RETURN n")
                .unwrap();
            let params = std::collections::HashMap::from([(
                "q".to_owned(),
                TqlParamValue::UInt(literal.parse().unwrap()),
            )]);
            let rows = db.execute_prepared_tql(&prepared, &params).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["n"].as_node().unwrap().id, id);
        }
        for literal in ["-1", "1.5", "18446744073709551616"] {
            assert!(
                db.tql(&format!("SEARCH VECTOR [{literal}] TOP 1 RETURN *"))
                    .is_err()
            );
        }
        db.close().unwrap();
    }
}

#[test]
fn payload的id索引不裁剪结构id谓词和分页顺序() {
    let root = TestDir::new("tql_id_index_domains");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Mmap))
            .unwrap();
    for id in [1, 2, 3] {
        db.insert_with_id(id, &[1.], json!({"kind":"sample","id":4-id}))
            .unwrap();
    }
    for indexed in [false, true] {
        if indexed {
            db.create_index("id").unwrap();
            db.create_ordered_index("id").unwrap();
        }
        assert_eq!(
            rows(
                &db,
                "FIND {kind:\"sample\"} RETURN _.id AS ident ORDER BY _.id ASC LIMIT 1"
            ),
            json!([{"ident":1}]),
            "{indexed}"
        );
        for source in [
            "FIND {kind:\"sample\"} AS n WITH n",
            "SEARCH VECTOR [1] TOP 3 AS n WITH n",
        ] {
            assert_eq!(
                rows(
                    &db,
                    &format!("{source} WHERE n.id == 1 RETURN n.id AS ident")
                ),
                json!([{"ident":1}]),
                "{indexed}/{source}"
            );
        }
    }
    db.close().unwrap();
}

#[test]
fn 三端共享容器与边界样例使用固定事实验收() {
    let case: Value =
        serde_json::from_str(include_str!("../contracts/tql_boundary_cases.json")).unwrap();
    let root = TestDir::new("tql_shared_boundaries");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Mmap))
            .unwrap();
    for record in case["records"].as_array().unwrap() {
        db.insert_with_id(
            record["id"].as_u64().unwrap(),
            &[1.],
            record["payload"].clone(),
        )
        .unwrap();
    }
    db.upsert_edge(1, 2, "rel", 2., case["edge_metadata"].clone())
        .unwrap();
    for source in case["sources"].as_array().unwrap() {
        for test in case["cases"].as_array().unwrap() {
            let query = format!(
                "{} {}",
                source["query"].as_str().unwrap(),
                test["suffix"]
                    .as_str()
                    .unwrap()
                    .replace("{var}", source["variable"].as_str().unwrap())
            );
            assert_eq!(rows(&db, &query), test["rows"], "{query}");
        }
    }
    for indexed in [false, true] {
        if indexed {
            db.create_index("x").unwrap();
        }
        for test in case["edge_cases"].as_array().unwrap() {
            assert_eq!(
                rows(&db, test["query"].as_str().unwrap()),
                test["rows"],
                "{}",
                test["query"]
            );
        }
    }
    db.close().unwrap();
}

#[test]
fn 容器内容相等与分组保持键序和精确数字语义() {
    let root = TestDir::new("tql_container_equality");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Mmap))
            .unwrap();
    let a: Value = serde_json::from_str(r#"{"a":1,"b":[true,null]}"#).unwrap();
    let b: Value = serde_json::from_str(r#"{"b":[true,null],"a":1.0}"#).unwrap();
    for (id, left, right) in [
        (1, a.clone(), b.clone()),
        (
            2,
            json!({"n":9_007_199_254_740_993u64}),
            json!({"n":9_007_199_254_740_992.0}),
        ),
        (3, b.clone(), a.clone()),
    ] {
        db.insert_with_id(id, &[1.], json!({"left":left,"right":right}))
            .unwrap();
    }
    for (source, n) in [
        ("MATCH (n)", "n"),
        ("FIND {left: {$exists:true}} AS n WITH n", "n"),
    ] {
        assert_eq!(
            rows(
                &db,
                &format!(
                    "{source} WHERE {n}.left == {n}.right RETURN {n}.id AS id ORDER BY id ASC"
                )
            ),
            json!([{"id":1},{"id":3}])
        );
        assert_eq!(
            rows(
                &db,
                &format!("{source} WHERE {n}.left != {n}.right RETURN {n}.id AS id")
            ),
            json!([{"id":2}])
        );
        assert_eq!(
            rows(
                &db,
                &format!(
                    "{source} RETURN COALESCE({n}.left,NULL) AS object, COUNT(*) AS total ORDER BY total DESC"
                )
            ),
            json!([{"object":a,"total":2},{"object":{"n":9_007_199_254_740_993u64},"total":1}])
        );
    }
    let result = db
        .tql("MATCH (n {id:1}) RETURN COALESCE(n.left,NULL) AS value")
        .unwrap();
    assert!(matches!(result[0]["value"], TqlValue::Object(_)));
    db.close().unwrap();
}

#[test]
fn 混合类型排序为完整次序且有无索引保持一致() {
    let root = TestDir::new("tql_total_order");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Rom))
            .unwrap();
    for (id, value) in [
        (1, Value::Null),
        (2, json!({"a":1})),
        (3, json!([2])),
        (4, json!([1])),
        (5, json!(true)),
        (6, json!(false)),
        (7, json!("z")),
        (8, json!(2)),
        (9, json!(1)),
    ] {
        db.insert_with_id(id, &[1.], json!({"v":value})).unwrap();
    }
    let expected =
        json!([{"id":9},{"id":8},{"id":7},{"id":6},{"id":5},{"id":4},{"id":3},{"id":2},{"id":1}]);
    for indexed in [false, true] {
        if indexed {
            db.create_ordered_index("v").unwrap();
        }
        for (source, n) in [
            ("MATCH (n)", "n"),
            ("FIND {v:{$exists:true}}", "_"),
            ("FIND {v:{$exists:true}} AS n WITH n", "n"),
        ] {
            assert_eq!(
                rows(
                    &db,
                    &format!("{source} RETURN {n}.id AS id ORDER BY {n}.v ASC")
                ),
                expected,
                "{indexed}/{source}"
            );
        }
    }
    db.close().unwrap();
}

#[test]
fn 重复变量约束节点身份且边引用支持空值表达式() {
    let root = TestDir::new("tql_reused_variables");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Mmap))
            .unwrap();
    for id in [1, 2] {
        db.insert_with_id(id, &[1.], json!({})).unwrap();
    }
    db.link(1, 2, "rel", 2.).unwrap();
    db.link(1, 1, "loop", 3.).unwrap();
    assert_eq!(
        rows(&db, "MATCH (a)-[]->(a) RETURN COUNT(*) AS total"),
        json!([{"total":1}])
    );
    assert_eq!(
        rows(&db, "MATCH (a)-[e]->(a) RETURN COUNT(*) AS total"),
        json!([{"total":1}])
    );
    assert_eq!(
        rows(
            &db,
            "MATCH (a)-[e:rel]-(b) RETURN DISTINCT COUNT(COALESCE(e,NULL)) AS total"
        ),
        json!([{"total":1}])
    );
    assert_eq!(
        rows(
            &db,
            "OPTIONAL MATCH (a {id:2})-[e:rel]->(b) RETURN a.id AS id, b IS NULL AS node_absent, e IS NULL AS edge_absent"
        ),
        json!([{"id":2,"node_absent":true,"edge_absent":true}])
    );
    assert_eq!(
        rows(
            &db,
            "OPTIONAL MATCH (_ {id:2})-[e:rel]->(b) RETURN b.id AS direct, COALESCE(b.id,0) AS fallback"
        ),
        json!([{"direct":null,"fallback":0}])
    );
    let result = db
        .tql("OPTIONAL MATCH (a {id:2})-[e:rel]->(b) RETURN *")
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].len(), 3);
    assert!(matches!(result[0]["b"], TqlValue::Null));
    assert!(matches!(result[0]["e"], TqlValue::Null));
    assert_eq!(result[0]["a"].as_node().unwrap().id, 2);
    db.close().unwrap();
}

#[test]
fn prepared默认列名稳定且非法槽位和未绑定参数明确拒绝() {
    use triviumdb::query::{tql_ast::QueryEntry, tql_parser::parse_tql, tql_prepared::PreparedTql};
    let root = TestDir::new("tql_prepared_slots");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Mmap))
            .unwrap();
    db.insert_with_id(1, &[1.], json!({})).unwrap();
    let prepared = db.prepare_tql("MATCH (n) RETURN $a, $b").unwrap();
    let params = std::collections::HashMap::from([
        ("a".into(), TqlParamValue::Int(1)),
        ("b".into(), TqlParamValue::Int(1)),
    ]);
    let result = db.execute_prepared_tql(&prepared, &params).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].len(), 2);
    assert_eq!(json_value(&result[0]["$a"]), json!(1));
    assert_eq!(json_value(&result[0]["$b"]), json!(1));
    assert!(matches!(
        db.tql("MATCH (n) RETURN $a"),
        Err(TriviumError::QueryExecution(_))
    ));
    for slots in [
        vec![(usize::MAX, "a".into())],
        vec![(0, "a".into()), (0, "b".into())],
    ] {
        let mut query = parse_tql("SEARCH VECTOR [1] TOP 1 RETURN *").unwrap();
        let QueryEntry::Search {
            vector_parameters, ..
        } = &mut query.entry
        else {
            panic!("样本要求 SEARCH")
        };
        *vector_parameters = slots;
        let prepared = PreparedTql::from_query(query);
        let values = prepared
            .parameter_names()
            .into_iter()
            .map(|name| (name.to_owned(), TqlParamValue::Int(1)))
            .collect();
        assert!(matches!(
            prepared.bind(&values),
            Err(TriviumError::QueryExecution(_))
        ));
    }
    for query in [
        "MATCH (a)-[a]->(b) RETURN a",
        "MATCH (a) RETURN missing IS NULL",
    ] {
        assert!(
            matches!(db.tql(query), Err(TriviumError::QuerySyntax { .. })),
            "{query}"
        );
    }
    db.close().unwrap();
}

#[test]
fn 函数同名别名可排序且函数调用保留原语义() {
    let root = TestDir::new("tql_function_aliases");
    let mut db =
        Database::<f32>::open_with_config(&root.file("sample.tdb"), config(StorageMode::Mmap))
            .unwrap();
    for id in [1, 2] {
        db.insert_with_id(id, &[1.], json!({"x":id})).unwrap();
    }
    for source in ["MATCH (n)", "MATCH (n) AS n WITH n"] {
        for alias in ["id", "path", "coalesce", "community"] {
            assert_eq!(
                rows(
                    &db,
                    &format!("{source} RETURN COALESCE(n.x,0) AS {alias} ORDER BY {alias} DESC")
                ),
                json!([{alias:2},{alias:1}])
            );
        }
    }
    assert_eq!(
        rows(&db, "MATCH (n) RETURN id(n) AS id ORDER BY id DESC"),
        json!([{"id":2},{"id":1}])
    );
    db.close().unwrap();
}

#[test]
fn 检索转换在空库中同样拒绝超出dtype范围的分量() {
    fn check<T: triviumdb::VectorType + serde::Serialize + serde::de::DeserializeOwned>(
        file: &str,
        literal: &str,
    ) {
        let mut db = Database::<T>::open_with_config(file, config(StorageMode::Mmap)).unwrap();
        for query in [
            format!("SEARCH VECTOR [{literal}] TOP 1 RETURN *"),
            format!("SEARCH VECTOR [{literal}] TOP 1 AS n WITH n RETURN n"),
            format!("MATCH (n) RANK n BY VECTOR [{literal}] TOP 1 RETURN n"),
        ] {
            assert!(
                matches!(db.tql(&query), Err(TriviumError::InvalidVector { .. })),
                "{query}"
            );
        }
        db.close().unwrap();
    }
    let root = TestDir::new("tql_empty_vector_validation");
    check::<f32>(&root.file("f32.tdb"), "1e100");
    check::<half::f16>(&root.file("f16.tdb"), "65520");
    check::<u64>(&root.file("u64.tdb"), "1.5");
}

#[test]
fn 数值相等的索引候选保留整数与浮点表示() {
    let root = TestDir::new("tql_numeric_candidate_domain");
    for index_kind in ["none", "hash", "ordered", "bitmap", "composite"] {
        let mut db = Database::<f32>::open_with_config(
            &root.file(&format!("{index_kind}.tdb")),
            config(StorageMode::Mmap),
        )
        .unwrap();
        // 80 个候选提供索引选择性；前 15 个节点覆盖相邻大整数与整数/浮点等价表示。
        for id in 1..=80 {
            let x = match id {
                1 => json!(1),
                2 => json!(1.0),
                3 => json!(9_007_199_254_740_992u64),
                4 => json!(9_007_199_254_740_993u64),
                5 => json!(9_007_199_254_740_992.0),
                6 => json!(-0.0),
                7 => json!(0),
                8 => json!(i64::MIN),
                9 => json!(i64::MIN as f64),
                10 => json!(9_223_372_036_854_775_808u64),
                11 => json!(9_223_372_036_854_775_808.0),
                12 => json!(u64::MAX),
                13 => json!(18_446_744_073_709_551_616.0),
                14 => json!(-1),
                15 => json!(-1.0),
                _ => json!(id),
            };
            db.insert_with_id(id, &[1.], json!({"x":x,"kind":"sample"}))
                .unwrap();
        }
        match index_kind {
            "hash" => {
                db.create_index("x").unwrap();
                db.create_index("kind").unwrap();
            }
            "ordered" => {
                db.create_ordered_index("x").unwrap();
            }
            "bitmap" => {
                db.create_bitmap_index("x").unwrap();
            }
            "composite" => {
                db.create_composite_index(&["kind".into(), "x".into()])
                    .unwrap();
            }
            _ => {}
        }
        for (literal, ids) in [
            ("1", vec![1, 2]),
            ("1.0", vec![1, 2]),
            ("9007199254740992", vec![3, 5]),
            ("9007199254740992.0", vec![3, 5]),
            ("9007199254740993", vec![4]),
            ("0", vec![6, 7]),
            ("-0.0", vec![6, 7]),
            ("-9223372036854775808", vec![8, 9]),
            ("9223372036854775808", vec![10, 11]),
            ("18446744073709551615", vec![12]),
            ("18446744073709551616.0", vec![13]),
            ("-1", vec![14, 15]),
        ] {
            for source in [
                "MATCH (n)",
                "MATCH (n) AS n WITH n",
                "FIND {kind:\"sample\"} AS n WITH n",
            ] {
                let query =
                    format!("{source} WHERE n.x == {literal} RETURN n.id AS id ORDER BY id ASC");
                assert_eq!(
                    rows(&db, &query),
                    json!(ids.iter().map(|id| json!({"id":id})).collect::<Vec<_>>()),
                    "{index_kind}/{query}"
                );
            }
        }
        db.close().unwrap();
    }
}
