use serde_json::json;
use triviumdb::Database;
use triviumdb::filter::{ComparableValue, Filter, RangeOp};
use triviumdb::query::planner::{AccessPath, plan_filter_with_limit};
use triviumdb::storage::memtable::MemTable;

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
                    "sequence": sequence,
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
fn planner_投影排序和其他字段排序保留完整输入且解释一致() {
    use triviumdb::query::tql_executor::execute_tql;
    use triviumdb::query::tql_parser::parse_tql;
    let mut mt = MemTable::<f32>::new(DIM);
    for (id, rank, nested) in [(1, 1, 30), (2, 2, 20), (3, 3, 10)] {
        mt.insert_with_id(
            id,
            &[1., 0., 0., 0.],
            json!({"rank":rank, "nested":{"rank":nested}, "same":0}),
        )
        .unwrap();
    }
    mt.register_ordered_property_index("rank");
    for order in ["picked.rank", "_.same"] {
        let query = format!(
            "EXPLAIN FIND {{rank: {{$gt:0}}}} RETURN _.nested AS picked ORDER BY {order} LIMIT 1"
        );
        let result = execute_tql(&parse_tql(&query).unwrap(), &mt).unwrap();
        let plan = &result[0]["plan"].payload;
        assert_eq!(plan["access_path"]["kind"], "ordered_property_index");
        assert_eq!(plan["estimated_rows"], 3);
        assert!(
            !plan["optimizations"]
                .as_array()
                .unwrap()
                .contains(&json!("LIMIT early termination")),
            "{query}: {plan}"
        );
    }
    let query = "EXPLAIN FIND {rank: {$gt:0}} RETURN _.id AS id ORDER BY _.rank LIMIT 1";
    let result = execute_tql(&parse_tql(query).unwrap(), &mt).unwrap();
    assert!(
        result[0]["plan"].payload["optimizations"]
            .as_array()
            .unwrap()
            .contains(&json!("LIMIT early termination"))
    );
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
    let before = db.payload_memory_stats();
    assert!(
        db.tql_nodes(r#"FIND {group: "group_7", rare: "missing"} RETURN * LIMIT 10"#)
            .unwrap()
            .is_empty()
    );
    let after = db.payload_memory_stats();
    assert_eq!(after.payload_lookups, before.payload_lookups);
    assert_eq!(after.payload_parsed_bytes, before.payload_parsed_bytes);
    let empty = plan(
        &db,
        r#"EXPLAIN FIND {group: "group_7", rare: "missing"} RETURN * LIMIT 10"#,
    );
    assert_eq!(empty["access_path"]["kind"], "composite_property_index");
    assert_eq!(empty["estimated_rows"], 0);
    let bitmap = plan(
        &db,
        r#"EXPLAIN FIND {$or: [{group: "group_1"}, {group: "group_3"}]} RETURN *"#,
    );
    assert_eq!(bitmap["access_path"]["kind"], "bitmap_property_index");

    drop(db);
    cleanup(&path);
}

#[test]
fn planner_find_limit_保留安全提前停止且不截断残余过滤() {
    let mut mt = MemTable::<f32>::new(DIM);
    for sequence in 0..200usize {
        mt.insert(
            &[sequence as f32, 0.0, 0.0, 0.0],
            json!({
                "sequence": sequence,
                "group": format!("group_{}", sequence % 10),
                "side": if sequence == 199 { "target" } else { "source" },
            }),
        )
        .unwrap();
    }

    let unindexed =
        plan_filter_with_limit(&Filter::Eq("group".into(), json!("group_1")), Some(10), &mt);
    assert!(matches!(unindexed.access_path, AccessPath::FullNodeScan));
    assert!(unindexed.candidates.is_empty());

    mt.register_ordered_property_index("sequence");
    let range = Filter::Range(
        "sequence".into(),
        RangeOp::Gte,
        ComparableValue::Number(0.into()),
    );
    let ordered = plan_filter_with_limit(&range, Some(10), &mt);
    assert!(matches!(
        ordered.access_path,
        AccessPath::OrderedPropertyIndex { .. }
    ));
    assert_eq!(ordered.candidates.len(), 10);

    let residual_filter = Filter::And(vec![range, Filter::Eq("side".into(), json!("target"))]);
    let residual = plan_filter_with_limit(&residual_filter, Some(1), &mt);
    assert!(matches!(
        residual.access_path,
        AccessPath::OrderedPropertyIndex { .. }
    ));
    assert_eq!(residual.candidates.len(), 200);
    assert_eq!(
        residual
            .candidates
            .iter()
            .filter_map(|id| mt.get_payload(*id))
            .filter(|payload| residual_filter.matches(payload))
            .count(),
        1
    );

    let (path, mut db) = build("find_limit_runtime");
    let before = db.payload_memory_stats().payload_lookups;
    let rows = db
        .tql_nodes(r#"FIND {group: "group_1"} RETURN * LIMIT 10"#)
        .unwrap();
    let unindexed_lookups = db.payload_memory_stats().payload_lookups - before;
    assert_eq!(rows.len(), 10);
    assert!(unindexed_lookups < 200);

    db.create_ordered_index("sequence").unwrap();
    let before = db.payload_memory_stats().payload_lookups;
    let rows = db
        .tql_nodes("FIND {sequence: {$gte: 0}} RETURN * LIMIT 10")
        .unwrap();
    let ordered_lookups = db.payload_memory_stats().payload_lookups - before;
    assert_eq!(rows.len(), 10);
    assert!(ordered_lookups <= 11);

    let rows = db
        .tql_nodes(r#"FIND {sequence: {$gte: 0}, side: "target"} RETURN * LIMIT 1"#)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["_"].payload["side"], "target");

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

#[cfg(feature = "test-hooks")]
#[test]
fn planner_限流与_explain_不物化无索引全集() {
    use triviumdb::query::tql_executor::execute_tql;
    use triviumdb::query::tql_parser::parse_tql;

    let mut mt = MemTable::<f32>::new(DIM);
    for sequence in 0..200usize {
        mt.insert(
            &[1.0, 0.0, 0.0, 0.0],
            json!({"sequence": sequence, "group": sequence % 10}),
        )
        .unwrap();
    }
    let queries = [
        "FIND {group: 1} RETURN * LIMIT 10",
        "FIND {sequence: {$gte: 10}} RETURN * LIMIT 10",
        "FIND {group: 1, sequence: {$gte: 10}} RETURN * LIMIT 10",
        "FIND {$or: [{group: 1}, {group: 3}]} RETURN * LIMIT 10",
    ];
    for query in queries {
        let before = mt.id_materializations();
        assert_eq!(
            execute_tql(&parse_tql(query).unwrap(), &mt).unwrap().len(),
            10
        );
        let explained = execute_tql(&parse_tql(&format!("EXPLAIN {query}")).unwrap(), &mt).unwrap();
        assert_eq!(
            explained[0]["plan"].payload["access_path"]["kind"],
            "full_node_scan"
        );
        assert_eq!(explained[0]["plan"].payload["total_nodes"], 200);
        assert_eq!(
            mt.id_materializations(),
            before,
            "限流与解释计划不得物化全集：{query}"
        );
    }

    // 对照组保留公开规划器的完整候选契约，证明计数能够观测到物化。
    let before = mt.id_materializations();
    let full = plan_filter_with_limit(&Filter::Eq("group".into(), json!(1)), None, &mt);
    assert_eq!(full.candidates.len(), 200);
    assert_eq!(mt.id_materializations(), before + 1);

    mt.register_bitmap_property_index("group");
    let before = mt.id_materializations();
    let indexed = plan_filter_with_limit(&Filter::Eq("group".into(), json!(1)), Some(10), &mt);
    assert!(matches!(
        indexed.access_path,
        AccessPath::BitmapPropertyIndex { .. }
    ));
    assert_eq!(
        mt.id_materializations(),
        before,
        "bitmap 等值查询不需要全集"
    );
    let complement = plan_filter_with_limit(&Filter::Ne("group".into(), json!(1)), Some(10), &mt);
    assert_eq!(complement.candidates.len(), 180);
    assert_eq!(
        mt.id_materializations(),
        before + 1,
        "bitmap 补集只按需物化一次"
    );
}

#[test]
fn planner_limit_保留复合_bitmap_和_ngram_索引() {
    let mut mt = MemTable::<f32>::new(DIM);
    for sequence in 0..20usize {
        mt.insert(
            &[1.0, 0.0, 0.0, 0.0],
            json!({"sequence": sequence, "group": sequence % 2, "text": "向量数据库测试"}),
        )
        .unwrap();
    }
    mt.register_composite_property_index(&["group".into(), "sequence".into()]);
    let composite = plan_filter_with_limit(
        &Filter::And(vec![
            Filter::Eq("group".into(), json!(1)),
            Filter::Eq("sequence".into(), json!(19)),
        ]),
        Some(1),
        &mt,
    );
    assert!(matches!(
        composite.access_path,
        AccessPath::CompositePropertyIndex { .. }
    ));
    assert_eq!(composite.candidates.len(), 1);

    mt.register_bitmap_property_index("group");
    let bitmap = plan_filter_with_limit(&Filter::In("group".into(), vec![json!(1)]), Some(1), &mt);
    assert!(matches!(
        bitmap.access_path,
        AccessPath::BitmapPropertyIndex { .. }
    ));
    assert_eq!(bitmap.candidates.len(), 10);

    mt.register_ngram_property_index("text").unwrap();
    let ngram = plan_filter_with_limit(
        &Filter::Contains("text".into(), "数据库".into()),
        Some(1),
        &mt,
    );
    assert!(matches!(
        ngram.access_path,
        AccessPath::NgramPropertyIndex { .. }
    ));
    assert_eq!(ngram.candidates.len(), 20);
}

#[test]
fn explain_limit_只对允许提前停止的查询标注优化() {
    let (path, mut db) = build("explain_limit");
    let scan = plan(&db, "EXPLAIN FIND {sequence: {$gte: 0}} RETURN * LIMIT 10");
    assert_eq!(scan["access_path"]["kind"], "full_node_scan");
    assert!(
        scan["optimizations"]
            .as_array()
            .unwrap()
            .contains(&json!("LIMIT early termination"))
    );
    let sorted = plan(
        &db,
        "EXPLAIN FIND {sequence: {$gte: 0}} RETURN * ORDER BY _.sequence DESC LIMIT 10",
    );
    assert!(
        !sorted["optimizations"]
            .as_array()
            .unwrap()
            .contains(&json!("LIMIT early termination"))
    );
    db.create_ordered_index("sequence").unwrap();
    let ordered = plan(&db, "EXPLAIN FIND {sequence: {$gte: 0}} RETURN * LIMIT 10");
    assert_eq!(ordered["access_path"]["kind"], "ordered_property_index");
    assert_eq!(ordered["estimated_rows"], 11);
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
