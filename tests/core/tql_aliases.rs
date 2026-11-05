//! WITH 快照、完整作用域与实体排序的独立样例。
use crate::support::TestDir;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use triviumdb::query::tql_executor::TqlValue;
use triviumdb::{Database, TriviumError};

fn rows(db: &Database<f32>, query: &str) -> Value {
    json!(
        db.tql(query)
            .unwrap_or_else(|e| panic!("{query}: {e:?}"))
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|(name, value)| {
                        let value = match value {
                            TqlValue::Node(node) => json!({"node":node.id}),
                            TqlValue::Int(value) => json!(value),
                            TqlValue::UInt(value) => json!(value),
                            TqlValue::Float(value) => json!(value),
                            TqlValue::String(value) => json!(value),
                            TqlValue::Bool(value) => json!(value),
                            TqlValue::List(value) => json!(value),
                            TqlValue::Object(value) => json!(value),
                            TqlValue::Null => Value::Null,
                            other => panic!("样例出现意外值：{other:?}"),
                        };
                        (name, value)
                    })
                    .collect::<BTreeMap<_, _>>()
            })
            .collect::<Vec<_>>()
    )
}

#[test]
fn with标量与实体别名在扩展后保留投影值() {
    let root = TestDir::new("tql_with_snapshot");
    let mut db = Database::<f32>::open(&root.file("sample.tdb"), 1).unwrap();
    db.insert_with_id(1, &[1.], json!({"seed":true,"x":11}))
        .unwrap();
    db.insert_with_id(2, &[1.], json!({"x":22})).unwrap();
    db.link(1, 2, "rel", 1.).unwrap();
    assert_eq!(
        rows(
            &db,
            "FIND {seed:true} AS seed WITH seed, seed.x AS before, COALESCE(seed,NULL) AS origin EXPAND seed [:rel*1..1] AS reached RETURN before, origin, reached.x AS after"
        ),
        json!([{"before":11,"origin":{"node":1},"after":22}])
    );
    let nodes=db.tql_nodes("FIND {seed:true} AS seed WITH seed, seed AS origin EXPAND seed [:rel*1..1] AS reached RETURN origin").unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["origin"].id, 1);
    assert_eq!(nodes[0]["origin"].payload, json!({"seed":true,"x":11}));
    db.close().unwrap();
}

#[test]
fn with递归检查每个表达式变量() {
    let root = TestDir::new("tql_with_scope");
    let mut db = Database::<f32>::open(&root.file("sample.tdb"), 1).unwrap();
    db.insert_with_id(1, &[1.], json!({"x":11})).unwrap();
    for expr in [
        "n.x + missing.x",
        "COALESCE(n.x,missing.x)",
        "COALESCE(n.x,missing.x + 1) IS NULL",
    ] {
        let query = format!("MATCH (n) AS n WITH n, {expr} AS total RETURN total");
        assert!(
            matches!(db.tql(&query), Err(TriviumError::QuerySyntax { .. })),
            "{query}"
        );
    }
    db.close().unwrap();
}

#[test]
fn 返回别名排序保留实体与标量的类型次序() {
    let root = TestDir::new("tql_alias_sort");
    let mut db = Database::<f32>::open(&root.file("sample.tdb"), 1).unwrap();
    db.insert_with_id(1, &[1.], json!({})).unwrap();
    db.insert_with_id(2, &[1.], json!({"v":3})).unwrap();
    for source in ["MATCH (n)", "MATCH (n) AS n WITH n"] {
        for order in ["COALESCE(n.v,n)", "chosen"] {
            let query = format!(
                "{source} RETURN n.id AS id, COALESCE(n.v,n) AS chosen ORDER BY {order} ASC"
            );
            assert_eq!(
                rows(&db, &query),
                json!([{"id":2,"chosen":3},{"id":1,"chosen":{"node":1}}]),
                "{query}"
            );
        }
    }
    db.close().unwrap();
}

#[test]
fn 多源汇合与连续扩展保留各自的投影环境() {
    let root = TestDir::new("tql_alias_merge");
    let mut db = Database::<f32>::open(&root.file("sample.tdb"), 1).unwrap();
    for (id, x, seed) in [(1, 11, true), (2, 22, true), (3, 33, false), (4, 44, false)] {
        db.insert_with_id(id, &[1.], json!({"x":x,"seed":seed}))
            .unwrap();
    }
    for (a, b) in [(1, 3), (2, 3), (3, 4)] {
        db.link(a, b, "rel", 1.).unwrap();
    }
    for indexed in [false, true] {
        if indexed {
            db.create_index("x").unwrap();
        }
        assert_eq!(
            rows(
                &db,
                "FIND {seed:true} AS seed WITH seed, seed.x AS before EXPAND seed [:rel*1..1] AS child WITH child, seed AS origin, before + 1 AS before EXPAND child [:rel*1..1] AS last RETURN origin.id AS origin, before, last.id AS last ORDER BY origin ASC"
            ),
            json!([{"origin":1,"before":12,"last":4},{"origin":2,"before":23,"last":4}])
        );
        assert_eq!(
            rows(
                &db,
                "FIND {seed:true} AS seed WITH seed, seed.x AS before EXPAND seed [:rel*1..1] AS child WITH child, seed, before WHERE seed.x == 11 RETURN seed.id AS origin, child.id AS child, before"
            ),
            json!([{"origin":1,"child":3,"before":11}])
        );
        assert_eq!(
            rows(
                &db,
                "FIND {seed:true} AS seed WITH seed, seed.x AS before EXPAND seed [:rel*1..1] AS child WITH child, seed, before WHERE seed MATCHES {x:11} RETURN seed.id AS origin, before"
            ),
            json!([{"origin":1,"before":11}])
        );
        assert_eq!(
            rows(
                &db,
                "FIND {seed:true} AS seed WITH seed, seed.x AS before EXPAND seed [:rel*1..1] AS child RETURN before, COUNT(*) AS total ORDER BY before ASC"
            ),
            json!([{"before":11,"total":1},{"before":22,"total":1}])
        );
    }
    db.close().unwrap();
}

#[test]
fn 连续with使用进入阶段时的值并区分节点与标量别名() {
    let root = TestDir::new("tql_alias_simultaneous");
    let mut db = Database::<f32>::open(&root.file("sample.tdb"), 1).unwrap();
    db.insert_with_id(1, &[1.], json!({"x":11,"object":{"k":4},"arr":[]}))
        .unwrap();
    assert_eq!(
        rows(
            &db,
            "MATCH (n) AS n WITH n.x AS n, n AS original WITH original, n AS before, n + 1 AS n RETURN before, n, original.id AS id"
        ),
        json!([{"before":11,"n":12,"id":1}])
    );
    assert_eq!(
        rows(
            &db,
            "MATCH (n) AS n WITH n.x AS x WITH x AS renamed RETURN *"
        ),
        json!([{"renamed":11}])
    );
    assert_eq!(
        rows(
            &db,
            "MATCH (n) AS n WITH n, n.object AS obj, n.arr AS list RETURN obj.k AS k, list IS NULL AS absent"
        ),
        json!([{"k":4,"absent":false}])
    );
    for query in [
        "MATCH (n) AS n WITH n.x AS scalar WITH scalar AS renamed EXPAND renamed [:rel*1..1] AS bad RETURN bad",
        "MATCH (n) AS n WITH n AS fresh, fresh.x AS value RETURN value",
        "MATCH (n) AS n WITH n.x AS scalar WITH n RETURN n",
    ] {
        assert!(
            matches!(db.tql(query), Err(TriviumError::QuerySyntax { .. })),
            "{query}"
        );
    }
    db.close().unwrap();
}

#[test]
fn 投影的实体与对象字段可通过返回别名排序() {
    let root = TestDir::new("tql_alias_properties");
    let mut db = Database::<f32>::open(&root.file("sample.tdb"), 1).unwrap();
    db.insert_with_id(1, &[1.], json!({"x":22,"obj":{"k":22}}))
        .unwrap();
    db.insert_with_id(2, &[1.], json!({"x":11,"obj":{"k":11}}))
        .unwrap();
    for source in ["MATCH (n)", "MATCH (n) AS n WITH n"] {
        for (value, field) in [("COALESCE(n,NULL)", "x"), ("COALESCE(n.obj,NULL)", "k")] {
            let query = format!(
                "{source} RETURN n.id AS id, {value} AS chosen ORDER BY chosen.{field} ASC"
            );
            let result = db.tql(&query).unwrap();
            let ids = result
                .iter()
                .map(|row| match row["id"] {
                    TqlValue::Int(id) => id,
                    _ => panic!("ID 类型错误"),
                })
                .collect::<Vec<_>>();
            assert_eq!(ids, vec![2, 1], "{query}");
        }
    }
    db.close().unwrap();
}

#[test]
fn 三端共享别名样例覆盖索引与存储模式() {
    use triviumdb::database::{Config, StorageMode};
    let cases: Value =
        serde_json::from_str(include_str!("../contracts/tql_alias_cases.json")).unwrap();
    let root = TestDir::new("tql_shared_aliases");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 1,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        for record in cases["records"].as_array().unwrap() {
            db.insert_with_id(
                record["id"].as_u64().unwrap(),
                &[1.],
                record["payload"].clone(),
            )
            .unwrap();
        }
        for edge in cases["edges"].as_array().unwrap() {
            db.link(
                edge[0].as_u64().unwrap(),
                edge[1].as_u64().unwrap(),
                "rel",
                1.,
            )
            .unwrap();
        }
        for indexed in [false, true] {
            if indexed {
                db.create_index("x").unwrap();
            }
            for case in cases["cases"].as_array().unwrap() {
                let query = case["query"].as_str().unwrap();
                let result = rows(&db, query);
                if let Some(ids) = case.get("ids") {
                    assert_eq!(
                        json!(
                            result
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|row| row["id"].clone())
                                .collect::<Vec<_>>()
                        ),
                        *ids,
                        "{query}"
                    );
                } else {
                    assert_eq!(result, case["rows"], "{query}");
                }
            }
        }
        for query in cases["invalid"].as_array().unwrap() {
            assert!(matches!(
                db.tql(query.as_str().unwrap()),
                Err(TriviumError::QuerySyntax { .. })
            ));
        }
        db.close().unwrap();
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn 别名快照在串行并行及融合精排中保持一致() {
    use triviumdb::test_hooks::{QueryExecutionStrategy as Strategy, force_query_strategy};
    let root = TestDir::new("tql_alias_strategies");
    let mut db = Database::<f32>::open(&root.file("sample.tdb"), 1).unwrap();
    for (id, x, seed) in [(1, 11, true), (2, 22, false), (3, 33, true)] {
        db.insert_with_id(id, &[1.], json!({"x":x,"seed":seed}))
            .unwrap();
    }
    for source in [1, 3] {
        db.link(source, 2, "rel", 1.).unwrap();
    }
    for strategy in [
        Strategy::Auto,
        Strategy::ForceSerial,
        Strategy::ForceParallel,
        Strategy::DisableFusion,
    ] {
        let _guard = force_query_strategy(strategy);
        assert_eq!(
            rows(
                &db,
                "FIND {seed:true} AS seed WITH seed, seed.x AS before EXPAND seed [:rel*1..1] AS child RANK child BY VECTOR [1] TOP 2 RETURN seed.id AS origin, before, child.id AS child ORDER BY origin ASC"
            ),
            json!([{"origin":1,"before":11,"child":2},{"origin":3,"before":33,"child":2}]),
            "{strategy:?}"
        );
    }
    db.close().unwrap();
}

#[test]
fn with保存的大值计入查询工作区预算() {
    use triviumdb::query::tql_executor::{TqlLimits, execute_tql_values_with_limits};
    use triviumdb::query::tql_parser::parse_tql;
    use triviumdb::storage::memtable::MemTable;
    let mut mt = MemTable::<f32>::new(1);
    // 单列 64 KiB，独立于仅够容纳节点行的 16 KiB 工作区。
    mt.insert_with_id(1, &[1.], json!({"x":11,"text":"x".repeat(64*1024)}))
        .unwrap();
    let query = parse_tql("FIND {x:11} AS n WITH n, n.text AS captured RETURN captured").unwrap();
    assert!(matches!(
        execute_tql_values_with_limits(
            &query,
            &mt,
            TqlLimits {
                memory_limit: 16 * 1024,
                ..Default::default()
            }
        ),
        Err(TriviumError::QueryExecution(_))
    ));
}

#[test]
fn with投影遵守共享配额并在拒绝后释放工作区() {
    use triviumdb::database::Config;
    use triviumdb::{ResourceLimits, ResourceRuntime};
    let root = TestDir::new("tql_alias_shared_budget");
    // 总额 4 MiB，数据库预留 2 MiB，维护区保护 512 KiB；前台剩余 1536 KiB。
    let runtime = ResourceRuntime::new(ResourceLimits {
        memory_limit: 4 * 1024 * 1024,
        ..Default::default()
    })
    .unwrap();
    let mut db = Database::<f32>::open_with_runtime(
        &root.file("sample.tdb"),
        Config {
            dim: 1,
            memory_limit: 2 * 1024 * 1024,
            payload_cache_bytes: 0,
            payload_cache_entry_bytes: 0,
            auto_build_quiver: false,
            ..Default::default()
        },
        runtime.clone(),
    )
    .unwrap();
    db.insert_with_id(1, &[1.], json!({"text":"x".repeat(128*1024)}))
        .unwrap();
    // 13 列各保存 128 KiB，共 1664 KiB，超过共享前台余量且低于实例 2 MiB 上限。
    let columns = (0..13)
        .map(|index| format!("n.text AS v{index}"))
        .collect::<Vec<_>>()
        .join(",");
    let result = db.tql(&format!("MATCH (n) AS n WITH n, {columns} RETURN v0"));
    assert!(
        matches!(result, Err(TriviumError::ResourceBudgetExceeded { .. })),
        "{result:?}"
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert_eq!(
        rows(&db, "MATCH (n) AS n WITH n, 12 AS a RETURN a"),
        json!([{"a":12}])
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    db.close().unwrap();
}
