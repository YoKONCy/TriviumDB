#![allow(non_snake_case)]
//! TQL 执行器端到端集成测试
//!
//! 通过 db.tql_nodes() 公开入口验证完整管线：
//! 解析 → 规划 → 执行 → 投影 → 排序 → 分页

use triviumdb::database::{Config, Database, RowOverflowPolicy, StorageMode};

const DIM: usize = 2;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("tql_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

fn build_test_db(name: &str) -> (Database<f32>, String) {
    let path = tmp_db(name);
    cleanup(&path);
    let config = Config {
        dim: DIM,
        storage_mode: StorageMode::Rom,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();

    // 插入测试数据 (ID 从 1 自增)
    // ID=1: Alice
    db.insert(
        &[1.0, 0.0],
        serde_json::json!({"type": "person", "name": "Alice", "age": 30, "region": "cn"}),
    )
    .unwrap();
    // ID=2: Bob
    db.insert(
        &[0.0, 1.0],
        serde_json::json!({"type": "person", "name": "Bob", "age": 25, "region": "kr"}),
    )
    .unwrap();
    // ID=3: Summit
    db.insert(
        &[0.5, 0.5],
        serde_json::json!({"type": "event", "name": "Summit", "heat": 0.9, "region": "cn"}),
    )
    .unwrap();
    // ID=4: Report
    db.insert(
        &[0.3, 0.7],
        serde_json::json!({"type": "event", "name": "Report", "heat": 0.3, "region": "jp"}),
    )
    .unwrap();
    // ID=5: Carol
    db.insert(
        &[0.8, 0.2],
        serde_json::json!({"type": "person", "name": "Carol", "age": 35, "region": "cn"}),
    )
    .unwrap();

    // 建立图谱关系
    db.link(1, 2, "knows", 1.0).unwrap(); // Alice knows Bob
    db.link(1, 5, "knows", 1.0).unwrap(); // Alice knows Carol
    db.link(2, 5, "reports_to", 1.0).unwrap(); // Bob reports_to Carol
    db.link(1, 3, "authored", 1.0).unwrap(); // Alice authored Summit
    db.link(5, 4, "authored", 1.0).unwrap(); // Carol authored Report

    (db, path)
}

#[test]
fn GraphFirst在图匹配集合内精确排序并去重Anchor() {
    let path = tmp_db("graph_first_rank");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let near_without_edge = db
        .insert(&[1.0, 0.0], serde_json::json!({"name": "near"}))
        .unwrap();
    let anchor_a = db
        .insert(&[0.8, 0.2], serde_json::json!({"name": "a"}))
        .unwrap();
    let anchor_b = db
        .insert(&[0.5, 0.5], serde_json::json!({"name": "b"}))
        .unwrap();
    let target = db
        .insert(&[-1.0, 0.0], serde_json::json!({"name": "target"}))
        .unwrap();
    db.link(anchor_a, target, "CITES", 1.0).unwrap();
    db.link(anchor_a, target, "MENTIONS", 1.0).unwrap();
    db.link(anchor_b, target, "CITES", 1.0).unwrap();

    let rows = db
        .tql_nodes(
            "MATCH (doc)-[:CITES|MENTIONS]->(ref) RANK doc BY VECTOR [1.0, 0.0] TOP 2 RETURN doc",
        )
        .unwrap();
    let ids: Vec<u64> = rows.iter().map(|row| row["doc"].id).collect();
    assert_eq!(ids, vec![anchor_a, anchor_b]);
    assert!(!ids.contains(&near_without_edge));
    cleanup(&path);
}

#[test]
fn GraphFirst未绑定Rank变量明确报错() {
    let (db, path) = build_test_db("graph_first_unbound");
    let result =
        db.tql_nodes("MATCH (a)-[:knows]->(b) RANK missing BY VECTOR [1.0, 0.0] TOP 1 RETURN a");
    assert!(result.is_err());
    cleanup(&path);
}

#[test]
fn GraphFirst分页在排名后执行且显式排序可覆盖排名() {
    let (db, path) = build_test_db("graph_first_paging");
    let ranked = db
        .tql_nodes(
            "MATCH (a)-[:knows]->(b) RANK b BY VECTOR [1.0, 0.0] TOP 2 RETURN b LIMIT 1 OFFSET 1",
        )
        .unwrap();
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0]["b"].payload["name"], "Bob");

    let ordered = db
        .tql_nodes("MATCH (a)-[:knows]->(b) RANK b BY VECTOR [1.0, 0.0] TOP 2 RETURN b ORDER BY b.name ASC")
        .unwrap();
    assert_eq!(ordered[0]["b"].payload["name"], "Bob");
    assert_eq!(ordered[1]["b"].payload["name"], "Carol");
    drop(db);
    cleanup(&path);
}

#[test]
fn GraphFirst_EXPLAIN报告精确Anchor排序() {
    let (db, path) = build_test_db("graph_first_explain");
    let rows = db
        .tql_nodes("EXPLAIN MATCH (a)-[:knows]->(b) RANK b BY VECTOR [1.0, 0.0] TOP 2 RETURN b")
        .unwrap();
    let optimizations = rows[0]["plan"].payload["optimizations"].as_array().unwrap();
    assert!(
        optimizations
            .iter()
            .any(|item| item == "GraphFirst exact anchor ranking")
    );
    drop(db);
    cleanup(&path);
}

#[test]
fn SEARCH_EXPAND多标签与空标签语义正确() {
    let path = tmp_db("search_expand_labels");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let seed = db.insert(&[1.0, 0.0], serde_json::json!({})).unwrap();
    let knows = db.insert(&[0.0, 1.0], serde_json::json!({})).unwrap();
    let works = db.insert(&[-1.0, 0.0], serde_json::json!({})).unwrap();
    db.link(seed, knows, "KNOWS", 1.0).unwrap();
    db.link(seed, works, "WORKS", 1.0).unwrap();

    let rows = db
        .tql_nodes("SEARCH VECTOR [1.0, 0.0] TOP 1 EXPAND [:KNOWS|WORKS*1..1] RETURN *")
        .unwrap();
    let ids: std::collections::HashSet<u64> = rows.iter().map(|row| row["_"].id).collect();
    assert_eq!(ids, [seed, knows, works].into_iter().collect());

    let rows = db
        .tql_nodes("SEARCH VECTOR [1.0, 0.0] TOP 1 EXPAND [*1..1] RETURN *")
        .unwrap();
    assert_eq!(rows.len(), 3);
    cleanup(&path);
}

#[test]
fn SEARCH_EXPAND支持Incoming和Both方向() {
    let path = tmp_db("search_expand_direction");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let source = db.insert(&[0.0, 1.0], serde_json::json!({})).unwrap();
    let seed = db.insert(&[1.0, 0.0], serde_json::json!({})).unwrap();
    let target = db.insert(&[-1.0, 0.0], serde_json::json!({})).unwrap();
    db.link(source, seed, "REL", 1.0).unwrap();
    db.link(seed, target, "REL", 1.0).unwrap();

    let incoming = db
        .tql_nodes("SEARCH VECTOR [1.0, 0.0] TOP 1 EXPAND INCOMING [:REL*1..1] RETURN *")
        .unwrap();
    assert_eq!(
        incoming
            .iter()
            .map(|row| row["_"].id)
            .collect::<std::collections::HashSet<_>>(),
        [seed, source].into_iter().collect()
    );

    let both = db
        .tql_nodes("SEARCH VECTOR [1.0, 0.0] TOP 1 EXPAND BOTH [:REL*1..1] RETURN *")
        .unwrap();
    assert_eq!(
        both.iter()
            .map(|row| row["_"].id)
            .collect::<std::collections::HashSet<_>>(),
        [source, seed, target].into_iter().collect()
    );
    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════════════
//  FIND 端到端
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn cooperative_cancellation在全表扫描前和扫描中fail_closed() {
    use triviumdb::query::tql_executor::QueryControl;

    let path = tmp_db("cooperative_cancel");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            storage_mode: StorageMode::Rom,
            ..Default::default()
        },
    )
    .unwrap();
    for id in 1..=1024u64 {
        db.insert_with_id(id, &[1.0, 0.0], serde_json::json!({"id": id}))
            .unwrap();
    }
    let control = QueryControl::default();
    control.cancel();
    let error = db
        .tql_with_control("MATCH (n) RETURN n", control)
        .unwrap_err();
    assert!(matches!(error, triviumdb::TriviumError::QueryCancelled));

    let expired = QueryControl::with_deadline(std::time::Instant::now());
    let error = db
        .tql_with_control("FIND {missing: true} RETURN *", expired)
        .unwrap_err();
    assert!(matches!(error, triviumdb::TriviumError::QueryCancelled));
    cleanup(&path);
}

#[test]
fn TQL_FIND_简单等值() {
    let (db, path) = build_test_db("find_eq");
    let results = db.tql_nodes(r#"FIND {type: "person"} RETURN *"#).unwrap();
    assert_eq!(results.len(), 3, "应找到 Alice, Bob, Carol");
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_FIND_操作符Gt() {
    let (db, path) = build_test_db("find_gt");
    let results = db.tql_nodes(r#"FIND {age: {$gt: 28}} RETURN *"#).unwrap();
    assert_eq!(results.len(), 2, "age>28: Alice(30), Carol(35)");
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_FIND_操作符In() {
    let (db, path) = build_test_db("find_in");
    let results = db
        .tql_nodes(r#"FIND {region: {$in: ["cn", "kr"]}} RETURN *"#)
        .unwrap();
    assert_eq!(results.len(), 4, "cn: Alice,Summit,Carol; kr: Bob");
    drop(db);
    cleanup(&path);
}

#[test]
fn 全表分页按节点ID稳定且差集不会遗漏() {
    let path = tmp_db("stable_paging");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            storage_mode: StorageMode::Rom,
            ..Default::default()
        },
    )
    .unwrap();
    for id in (1..=30u64).rev() {
        db.insert_with_id(id, &[1.0, 0.0], serde_json::json!({"skip": id % 3 == 0}))
            .unwrap();
    }
    let first = db.tql_nodes("MATCH (n) RETURN n LIMIT 10").unwrap();
    let ids = first.iter().map(|row| row["n"].id).collect::<Vec<_>>();
    assert_eq!(ids, (1..=10).collect::<Vec<_>>());

    db.delete(5).unwrap();
    db.insert_with_id(31, &[1.0, 0.0], serde_json::json!({"skip": false}))
        .unwrap();
    let after_reuse = db.tql_nodes("MATCH (n) RETURN n LIMIT 10").unwrap();
    assert_eq!(
        after_reuse
            .iter()
            .map(|row| row["n"].id)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 6, 7, 8, 9, 10, 11]
    );

    db.create_index("skip").unwrap();
    let kept = db
        .tql_nodes(r#"FIND {skip: {$ne: true}} RETURN *"#)
        .unwrap()
        .into_iter()
        .map(|row| row["_"].id)
        .collect::<Vec<_>>();
    let expected = (1..=31)
        .filter(|id| *id != 5 && id % 3 != 0)
        .collect::<Vec<_>>();
    assert_eq!(kept, expected);
    drop(db);
    cleanup(&path);
}

#[test]
fn FIND小LIMIT在扫描与Hash索引路径保持相同稳定语义() {
    let path = tmp_db("find_limit_pushdown");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            storage_mode: StorageMode::Rom,
            ..Default::default()
        },
    )
    .unwrap();
    for id in (1..=1_000u64).rev() {
        db.insert_with_id(
            id,
            &[1.0, 0.0],
            serde_json::json!({"type": if id % 2 == 0 { "even" } else { "odd" }}),
        )
        .unwrap();
    }

    let query = r#"FIND {type: "even"} RETURN * LIMIT 3 OFFSET 2"#;
    let scan_ids = db
        .tql_nodes(query)
        .unwrap()
        .into_iter()
        .map(|row| row["_"].id)
        .collect::<Vec<_>>();
    assert_eq!(scan_ids, vec![6, 8, 10]);

    db.create_index("type").unwrap();
    let indexed_ids = db
        .tql_nodes(query)
        .unwrap()
        .into_iter()
        .map(|row| row["_"].id)
        .collect::<Vec<_>>();
    assert_eq!(indexed_ids, scan_ids);

    drop(db);
    cleanup(&path);
}

#[test]
fn FIND范围与复合过滤小LIMIT保持稳定提前终止语义() {
    let path = tmp_db("find_range_compound_limit");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            storage_mode: StorageMode::Rom,
            ..Default::default()
        },
    )
    .unwrap();
    for id in 1..=1_000u64 {
        db.insert_with_id(
            id,
            &[1.0, 0.0],
            serde_json::json!({"type": if id % 2 == 0 { "even" } else { "odd" }, "score": id}),
        )
        .unwrap();
    }
    let range = db
        .tql_nodes(r#"FIND {score: {$gte: 500}} RETURN * LIMIT 3"#)
        .unwrap()
        .into_iter()
        .map(|row| row["_"].id)
        .collect::<Vec<_>>();
    assert_eq!(range, vec![500, 501, 502]);
    let compound = db
        .tql_nodes(r#"FIND {type: "even", score: {$gte: 500}} RETURN * LIMIT 3"#)
        .unwrap()
        .into_iter()
        .map(|row| row["_"].id)
        .collect::<Vec<_>>();
    assert_eq!(compound, vec![500, 502, 504]);
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_FIND和MATCH超过五千行仍完整分页() {
    let path = tmp_db("pagination_over_5000");
    cleanup(&path);
    let config = Config {
        dim: DIM,
        storage_mode: StorageMode::Rom,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    let mut tx = db.begin_tx();
    for index in 0..6_001u64 {
        tx.insert(
            &[1.0, 0.0],
            serde_json::json!({"type": "page", "index": index}),
        );
    }
    tx.commit().unwrap();
    db.create_index("type").unwrap();

    assert_eq!(
        db.tql_nodes(r#"FIND {type: "page"} RETURN *"#)
            .unwrap()
            .len(),
        6_001
    );
    assert_eq!(
        db.tql_nodes(r#"FIND {type: "page"} RETURN * LIMIT 6001"#)
            .unwrap()
            .len(),
        6_001
    );
    assert_eq!(
        db.tql_nodes(r#"FIND {type: "page"} RETURN * LIMIT 1001 OFFSET 5000"#)
            .unwrap()
            .len(),
        1_001
    );
    assert_eq!(db.tql_nodes("MATCH (n) RETURN n").unwrap().len(), 6_001);
    assert_eq!(
        db.tql_nodes("MATCH (n) RETURN n LIMIT 1001 OFFSET 5000")
            .unwrap()
            .len(),
        1_001
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 显式LIMIT可覆盖配置行上限但OFFSET仍受内存硬上限() {
    let path = tmp_db("configured_row_limit");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            storage_mode: StorageMode::Rom,
            max_query_rows: Some(2),
            row_overflow: RowOverflowPolicy::Throw,
            memory_limit: 3 * (DIM * std::mem::size_of::<f32>() + 256),
            payload_cache_bytes: 0,
            payload_cache_entry_bytes: 0,
            ..Default::default()
        },
    )
    .unwrap();
    for id in 1..=5 {
        db.insert_with_id(id, &[1.0, 0.0], serde_json::json!({"kind": "row"}))
            .unwrap();
    }

    let overflow = db.tql_nodes("MATCH (n) RETURN n").unwrap_err();
    assert!(overflow.to_string().contains("超过 2 行上限"));
    assert_eq!(db.tql_nodes("MATCH (n) RETURN n LIMIT 3").unwrap().len(), 3);
    let memory_error = db
        .tql_nodes("MATCH (n) RETURN n LIMIT 2 OFFSET 2")
        .unwrap_err();
    assert!(
        memory_error
            .to_string()
            .contains("Query row budget exceeded")
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn Break仅允许普通查询返回部分结果() {
    let path = tmp_db("break_row_limit");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            storage_mode: StorageMode::Rom,
            max_query_rows: Some(2),
            row_overflow: RowOverflowPolicy::Break,
            ..Default::default()
        },
    )
    .unwrap();
    for id in 1..=5 {
        db.insert_with_id(id, &[1.0, 0.0], serde_json::json!({"kind": "row"}))
            .unwrap();
    }

    assert_eq!(db.tql_nodes("MATCH (n) RETURN n").unwrap().len(), 2);
    let aggregate_error = db
        .tql_values("MATCH (n) RETURN count(*) AS total")
        .unwrap_err();
    assert!(aggregate_error.to_string().contains("超过 2 行上限"));
    let ordered_error = db
        .tql_nodes("MATCH (n) RETURN n ORDER BY n.kind")
        .unwrap_err();
    assert!(ordered_error.to_string().contains("超过 2 行上限"));

    drop(db);
    cleanup(&path);
}

#[test]
fn TQL显式分页超过内存硬预算必须失败而非静默截断() {
    let path = tmp_db("row_budget");
    cleanup(&path);
    let db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            storage_mode: StorageMode::Rom,
            memory_limit: 10 * (DIM * std::mem::size_of::<f32>() + 256),
            ..Default::default()
        },
    )
    .unwrap();
    let error = db
        .tql_nodes("MATCH (n) RETURN n LIMIT 10 OFFSET 1")
        .unwrap_err();
    assert!(error.to_string().contains("Query row budget exceeded"));
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_FIND_带LIMIT() {
    let (db, path) = build_test_db("find_limit");
    let results = db
        .tql_nodes(r#"FIND {type: "person"} RETURN * LIMIT 2"#)
        .unwrap();
    assert_eq!(results.len(), 2);
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_FIND_带ORDER_BY() {
    let (db, path) = build_test_db("find_order");
    let results = db
        .tql_nodes(r#"FIND {type: "event"} RETURN * ORDER BY _.heat DESC"#)
        .unwrap();
    assert_eq!(results.len(), 2);
    // Summit(0.9) 应排在 Report(0.3) 前面
    let first = &results[0]["_"];
    assert_eq!(first.payload["name"], "Summit");
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_FIND_Or逻辑() {
    let (db, path) = build_test_db("find_or");
    let results = db
        .tql_nodes(r#"FIND {$or: [{name: "Alice"}, {name: "Bob"}]} RETURN *"#)
        .unwrap();
    assert_eq!(results.len(), 2);
    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════════════
//  MATCH 端到端
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn TQL_MATCH_单跳() {
    let (db, path) = build_test_db("match_single");
    let results = db
        .tql_nodes(r#"MATCH (a {name: "Alice"})-[:knows]->(b) RETURN b"#)
        .unwrap();
    assert_eq!(results.len(), 2, "Alice knows Bob and Carol");
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_MATCH_带WHERE() {
    let (db, path) = build_test_db("match_where");
    let results = db
        .tql_nodes(r#"MATCH (a {name: "Alice"})-[:knows]->(b) WHERE b.age > 28 RETURN b"#)
        .unwrap();
    assert_eq!(results.len(), 1, "Only Carol has age > 28");
    assert_eq!(results[0]["b"].payload["name"], "Carol");
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_MATCH_多跳() {
    let (db, path) = build_test_db("match_multi");
    let results = db
        .tql_nodes(r#"MATCH (a {name: "Alice"})-[:knows]->(b)-[:reports_to]->(c) RETURN c"#)
        .unwrap();
    assert_eq!(results.len(), 1, "Alice->Bob->Carol via reports_to");
    assert_eq!(results[0]["c"].payload["name"], "Carol");
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_MATCH_任意边() {
    let (db, path) = build_test_db("match_any_edge");
    let results = db
        .tql_nodes(r#"MATCH (a {name: "Alice"})-[]->(b) RETURN b"#)
        .unwrap();
    // Alice has 3 outgoing edges: knows->Bob, knows->Carol, authored->Summit
    assert_eq!(results.len(), 3);
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_MATCH_多标签() {
    let (db, path) = build_test_db("match_multi_label");
    let results = db
        .tql_nodes(r#"MATCH (a {name: "Alice"})-[:knows|authored]->(b) RETURN b"#)
        .unwrap();
    assert_eq!(results.len(), 3, "2 knows + 1 authored");
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_MATCH_可变长路径() {
    let (db, path) = build_test_db("match_varlen");
    let results = db
        .tql_nodes(r#"MATCH (a {name: "Alice"})-[:knows*1..2]->(b) RETURN b"#)
        .unwrap();
    // 1-hop: Bob, Carol
    // 2-hop through knows edges from Bob/Carol: none (Bob has reports_to not knows)
    assert!(results.len() >= 2, "At least Bob and Carol via 1-hop knows");
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_MATCH_WHERE_MATCHES() {
    let (db, path) = build_test_db("match_matches");
    let results = db
        .tql_nodes(r#"MATCH (a)-[:authored]->(e) WHERE e MATCHES {heat: {$gte: 0.5}} RETURN a, e"#)
        .unwrap();
    assert_eq!(results.len(), 1, "Only Summit has heat >= 0.5");
    assert_eq!(results[0]["e"].payload["name"], "Summit");
    assert_eq!(results[0]["a"].payload["name"], "Alice");
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_MATCH_内联Mongo操作符() {
    let (db, path) = build_test_db("match_inline_mongo");
    let results = db
        .tql_nodes(r#"MATCH (a {age: {$gte: 30}})-[:knows]->(b) RETURN a, b"#)
        .unwrap();
    // Alice(30) and Carol(35) have age >= 30
    // Alice knows Bob and Carol, Carol has no knows edges
    assert_eq!(results.len(), 2, "Alice(age=30) knows Bob and Carol");
    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════════════
//  SEARCH 端到端
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn TQL_SEARCH_基础() {
    let (db, path) = build_test_db("search_basic");
    let results = db
        .tql_nodes(r#"SEARCH VECTOR [1.0, 0.0] TOP 3 RETURN *"#)
        .unwrap();
    assert!(results.len() <= 3);
    // [1.0, 0.0] 最相似 Alice [1.0, 0.0]
    let first = &results[0]["_"];
    assert_eq!(first.payload["name"], "Alice");
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_SEARCH_带WHERE过滤() {
    let (db, path) = build_test_db("search_where");
    let results = db
        .tql_nodes(r#"SEARCH VECTOR [0.5, 0.5] TOP 5 WHERE {type: "event"} RETURN *"#)
        .unwrap();
    for row in &results {
        assert_eq!(row["_"].payload["type"], "event");
    }
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_SEARCH_带索引WHERE返回过滤集合内TopK() {
    let (mut db, path) = build_test_db("search_where_indexed_topk");
    db.create_index("type").unwrap();
    let results = db
        .tql_nodes(r#"SEARCH VECTOR [1.0, 0.0] TOP 2 WHERE {type: "event"} RETURN *"#)
        .unwrap();
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .all(|row| row["_"].payload["type"] == "event")
    );
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_SEARCH_带EXPAND() {
    let (db, path) = build_test_db("search_expand");
    let results = db
        .tql_nodes(r#"SEARCH VECTOR [1.0, 0.0] TOP 1 EXPAND [:knows*1..1] RETURN *"#)
        .unwrap();
    // TOP 1 = Alice, EXPAND knows 1-hop = Bob, Carol
    assert!(
        results.len() >= 2,
        "Should include Alice and her knows neighbors"
    );
    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════════════
//  错误处理
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn TQL_错误_语法解析() {
    let (db, path) = build_test_db("err_parse");
    let result = db.tql_nodes("INVALID QUERY");
    assert!(result.is_err());
    drop(db);
    cleanup(&path);
}

#[test]
fn TQL_空结果_不报错() {
    let (db, path) = build_test_db("empty_result");
    let results = db
        .tql_nodes(r#"FIND {type: "nonexistent"} RETURN *"#)
        .unwrap();
    assert!(results.is_empty());
    drop(db);
    cleanup(&path);
}
