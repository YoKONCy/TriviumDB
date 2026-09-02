use serde_json::json;
use triviumdb::Database;

const DIM: usize = 4;

fn path(name: &str) -> String {
    let directory = std::env::temp_dir().join("triviumdb_planner_tests");
    std::fs::create_dir_all(&directory).unwrap();
    directory
        .join(format!("{name}_{}", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

fn cleanup(path: &str) {
    for suffix in ["", ".vec", ".wal", ".lock", ".flush_ok", ".pidx"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

fn build(name: &str) -> (String, Database<f32>) {
    let path = path(name);
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let mut ids = Vec::with_capacity(200);
    for sequence in 0..200usize {
        let id = db
            .insert(
                &[sequence as f32, 0.0, 0.0, 0.0],
                json!({
                    "group": format!("group_{}", sequence % 10),
                    "rare": format!("rare_{sequence}"),
                    "side": if sequence == 199 { "target" } else { "source" },
                }),
            )
            .unwrap();
        ids.push(id);
    }
    for source in ids.iter().take(199) {
        db.link(*source, ids[199], "points_to", 1.0).unwrap();
    }
    (path, db)
}

fn plan(db: &Database<f32>, query: &str) -> serde_json::Value {
    db.tql_nodes(query).unwrap()[0]["plan"].payload.clone()
}

#[test]
fn planner_稳定选择主键属性交集和标签索引() {
    let (path, mut db) = build("access_paths");
    db.create_index("group").unwrap();
    db.create_index("rare").unwrap();

    let primary = plan(&db, "EXPLAIN MATCH (a {id: 1}) RETURN a");
    assert_eq!(primary["access_path"]["kind"], "primary_key");
    assert_eq!(primary["estimated_rows"], 1);

    let property = plan(&db, r#"EXPLAIN FIND {rare: "rare_7"} RETURN *"#);
    assert_eq!(property["access_path"]["kind"], "property_index");
    assert_eq!(property["estimated_rows"], 1);

    let intersection = plan(
        &db,
        r#"EXPLAIN FIND {group: "group_7", rare: "rare_7"} RETURN *"#,
    );
    assert_eq!(
        intersection["access_path"]["kind"],
        "property_index_intersection"
    );
    assert_eq!(intersection["estimated_rows"], 1);

    let label = plan(
        &db,
        "EXPLAIN MATCH (a)-[:points_to]->(b) RETURN a, b LIMIT 10",
    );
    assert_eq!(label["access_path"]["kind"], "edge_label_index");

    drop(db);
    cleanup(&path);
}

#[test]
fn planner_按选择性反转单链_match_且结果不变() {
    let (path, mut db) = build("reverse");
    db.create_index("side").unwrap();
    let query = r#"MATCH (a)-[:points_to]->(b {side: "target"}) RETURN a, b"#;
    let expected = db.tql_nodes(query).unwrap();
    assert_eq!(expected.len(), 199);

    let explain = plan(
        &db,
        r#"EXPLAIN MATCH (a)-[:points_to]->(b {side: "target"}) RETURN a, b"#,
    );
    assert_eq!(explain["reversed"], true);
    assert_eq!(explain["estimated_rows"], 1);
    assert_eq!(explain["access_path"]["kind"], "property_index");

    let mut pairs: Vec<_> = expected
        .iter()
        .map(|row| (row["a"].id, row["b"].id))
        .collect();
    pairs.sort_unstable();
    assert_eq!(pairs.first().unwrap().1, pairs.last().unwrap().1);

    drop(db);
    cleanup(&path);
}

#[test]
fn planner_不反转可变长和_optional_match() {
    let (path, mut db) = build("safe_reverse_gate");
    db.create_index("side").unwrap();

    let variable = plan(
        &db,
        r#"EXPLAIN MATCH (a)-[:points_to*1..2]->(b {side: "target"}) RETURN a, b"#,
    );
    assert_eq!(variable["reversed"], false);

    let optional = plan(
        &db,
        r#"EXPLAIN OPTIONAL MATCH (a)-[:points_to]->(b {side: "target"}) RETURN a, b"#,
    );
    assert_eq!(optional["reversed"], false);

    drop(db);
    cleanup(&path);
}

#[test]
fn explain_analyze_返回估算实际行数和耗时() {
    let (path, mut db) = build("analyze");
    db.create_index("rare").unwrap();
    let explain = plan(&db, r#"EXPLAIN ANALYZE FIND {rare: "rare_7"} RETURN *"#);
    assert_eq!(explain["analyze"], true);
    assert_eq!(explain["estimated_rows"], 1);
    assert_eq!(explain["actual_rows"], 1);
    assert!(explain["elapsed_ms"].as_f64().unwrap() >= 0.0);
    assert_eq!(explain["property_index_stats"][0]["distinct_count"], 200);

    drop(db);
    cleanup(&path);
}

#[test]
fn planner_复合与_bitmap_访问路径和扫描结果一致() {
    let (path, mut db) = build("composite_bitmap");
    let composite_query = r#"FIND {group: "group_7", rare: "rare_7"} RETURN *"#;
    let bitmap_query = r#"FIND {$or: [{group: "group_1"}, {group: "group_3"}]} RETURN *"#;
    let composite_reference = canonical(&db.tql_nodes(composite_query).unwrap());
    let bitmap_reference = canonical(&db.tql_nodes(bitmap_query).unwrap());

    db.create_composite_index(&["group".into(), "rare".into()])
        .unwrap();
    db.create_bitmap_index("group").unwrap();
    assert_eq!(
        canonical(&db.tql_nodes(composite_query).unwrap()),
        composite_reference
    );
    assert_eq!(
        canonical(&db.tql_nodes(bitmap_query).unwrap()),
        bitmap_reference
    );

    let composite = plan(
        &db,
        r#"EXPLAIN FIND {group: "group_7", rare: "rare_7"} RETURN *"#,
    );
    assert_eq!(composite["access_path"]["kind"], "composite_property_index");
    let bitmap = plan(
        &db,
        r#"EXPLAIN FIND {$or: [{group: "group_1"}, {group: "group_3"}]} RETURN *"#,
    );
    assert_eq!(bitmap["access_path"]["kind"], "bitmap_property_index");

    drop(db);
    cleanup(&path);
}

#[test]
fn planner_有无索引执行结果差分一致() {
    let (path, mut db) = build("differential");
    let queries = [
        r#"FIND {group: "group_3"} RETURN *"#,
        r#"FIND {rare: "missing"} RETURN *"#,
        r#"MATCH (a {group: "group_3"})-[:points_to]->(b) RETURN a, b"#,
        r#"MATCH (a)-[:points_to]->(b {side: "target"}) RETURN a, b"#,
    ];
    let without: Vec<Vec<Vec<u64>>> = queries
        .iter()
        .map(|query| canonical(&db.tql_nodes(query).unwrap()))
        .collect();

    for field in ["group", "rare", "side"] {
        db.create_index(field).unwrap();
    }
    for (query, expected) in queries.iter().zip(without) {
        assert_eq!(
            canonical(&db.tql_nodes(query).unwrap()),
            expected,
            "{query}"
        );
    }

    drop(db);
    cleanup(&path);
}

fn canonical(
    rows: &[std::collections::HashMap<String, triviumdb::node::Node<f32>>],
) -> Vec<Vec<u64>> {
    let mut result: Vec<Vec<u64>> = rows
        .iter()
        .map(|row| {
            let mut bindings: Vec<_> = row.iter().map(|(name, node)| (name, node.id)).collect();
            bindings.sort_by_key(|(name, _)| *name);
            bindings.into_iter().map(|(_, id)| id).collect()
        })
        .collect();
    result.sort();
    result
}
