#![allow(non_snake_case)]
//! 图谱查询 DSL 回归测试
//!
//! 覆盖范围：
//! - P1-6 匿名节点语义校验（路径中匿名节点应报错）
//! - 节点精确查找（按属性）
//! - 单跳 / 两跳路径遍历
//! - WHERE 条件过滤
//! - 边标签过滤
//! - 无匹配 / 空库 / 语法错误

use triviumdb::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    std::fs::create_dir_all("test_data").ok();
    format!("test_data/query_{}", name)
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

/// 构建测试图谱：alice -[knows]-> bob -[likes]-> carol
fn build_graph(path: &str) -> (Database<f32>, u64, u64, u64) {
    let mut db = Database::<f32>::open(path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "alice", "age": 30}));
        tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "bob",   "age": 25}));
        tx.insert(&[0.0, 0.0, 1.0, 0.0], serde_json::json!({"name": "carol", "age": 28}));
        tx.commit().unwrap()
    };

    {
        let mut tx = db.begin_tx();
        tx.link(ids[0], ids[1], "knows", 1.0);
        tx.link(ids[1], ids[2], "likes", 0.8);
        tx.commit().unwrap();
    }

    (db, ids[0], ids[1], ids[2])
}

// ════════ P1-6：匿名节点语义校验 ════════

#[test]
fn P1_6_匿名中间节点_应解析报错() {
    let path = tmp_db("anon_mid");
    cleanup(&path);
    let (db, ..) = build_graph(&path);

    // 路径中间节点匿名 () → 根据 P1-6 修复应报错
    let result = db.query("MATCH (a)-[]->()-[]->(c) RETURN c");
    assert!(result.is_err(), "路径中匿名中间节点应返回解析错误");

    drop(db);
    cleanup(&path);
}

#[test]
fn P1_6_匿名起始节点带边_应解析报错() {
    let path = tmp_db("anon_start");
    cleanup(&path);
    let (db, ..) = build_graph(&path);

    let result = db.query("MATCH ()-[]->(b) RETURN b");
    assert!(result.is_err(), "有边的路径中起始节点匿名应报错");

    drop(db);
    cleanup(&path);
}

#[test]
fn P1_6_纯节点匹配允许匿名() {
    let path = tmp_db("anon_bare");
    cleanup(&path);
    let (db, ..) = build_graph(&path);

    // 纯节点匹配（无边），匿名或具名都不应 panic
    let result = db.query("MATCH (n) RETURN n");
    let _ = result;

    drop(db);
    cleanup(&path);
}

// ════════ 基础节点查询 ════════

#[test]
fn 查询_单节点匹配_按name属性() {
    let path = tmp_db("match_name");
    cleanup(&path);
    let (db, alice_id, ..) = build_graph(&path);

    let results = db.query(r#"MATCH (n {name: "alice"}) RETURN n"#).unwrap();
    assert_eq!(results.len(), 1, "按 name=alice 应匹配 1 个节点");
    let node = results[0].get("n").unwrap();
    assert_eq!(node.id, alice_id);

    drop(db);
    cleanup(&path);
}

#[test]
fn 查询_按ID精确查找() {
    let path = tmp_db("match_id");
    cleanup(&path);
    let (db, alice_id, ..) = build_graph(&path);

    let results = db.query(&format!("MATCH (n {{id: {}}}) RETURN n", alice_id)).unwrap();
    if !results.is_empty() {
        let node = results[0].get("n").unwrap();
        assert_eq!(node.id, alice_id, "按 id 查找应返回对应节点");
    }

    drop(db);
    cleanup(&path);
}

// ════════ 路径遍历 ════════

#[test]
fn 查询_单跳路径_alice_knows_bob() {
    let path = tmp_db("single_hop");
    cleanup(&path);
    let (db, _alice_id, bob_id, _carol_id) = build_graph(&path);

    let results = db.query(r#"MATCH (a {name: "alice"})-[:knows]->(b) RETURN b"#).unwrap();
    assert_eq!(results.len(), 1, "alice knows 应正好匹配 1 个节点");
    let b = results[0].get("b").unwrap();
    assert_eq!(b.id, bob_id);

    drop(db);
    cleanup(&path);
}

#[test]
fn 查询_两跳路径_alice_to_carol() {
    let path = tmp_db("two_hop");
    cleanup(&path);
    let (db, _alice_id, _bob_id, carol_id) = build_graph(&path);

    let results = db.query(
        r#"MATCH (a {name: "alice"})-[:knows]->(b)-[:likes]->(c) RETURN c"#
    ).unwrap();
    assert_eq!(results.len(), 1, "两跳路径应匹配 alice->bob->carol");
    let c = results[0].get("c").unwrap();
    assert_eq!(c.id, carol_id);

    drop(db);
    cleanup(&path);
}

#[test]
fn 查询_边标签不匹配_返回空() {
    let path = tmp_db("label_mismatch");
    cleanup(&path);
    let (db, ..) = build_graph(&path);

    let results = db.query(r#"MATCH (a {name: "alice"})-[:hates]->(b) RETURN b"#).unwrap();
    assert!(results.is_empty(), "不存在的边标签应返回空结果");

    drop(db);
    cleanup(&path);
}

// ════════ WHERE 条件过滤 ════════

#[test]
fn 查询_WHERE条件过滤() {
    let path = tmp_db("where_filter");
    cleanup(&path);
    let (db, _alice_id, bob_id, _carol_id) = build_graph(&path);

    let results = db.query(
        r#"MATCH (a {name: "alice"})-[:knows]->(b) WHERE b.age < 27 RETURN b"#
    ).unwrap();
    assert_eq!(results.len(), 1, "age < 27 应只匹配 bob");
    let b = results[0].get("b").unwrap();
    assert_eq!(b.id, bob_id);

    drop(db);
    cleanup(&path);
}

#[test]
fn 查询_WHERE无匹配_返回空() {
    let path = tmp_db("where_empty");
    cleanup(&path);
    let (db, ..) = build_graph(&path);

    let results = db.query(
        r#"MATCH (a {name: "alice"})-[:knows]->(b) WHERE b.age > 100 RETURN b"#
    ).unwrap();
    assert!(results.is_empty(), "WHERE 无匹配时应返回空");

    drop(db);
    cleanup(&path);
}

// ════════ 边界场景 ════════

#[test]
fn 查询_空库_返回空() {
    let path = tmp_db("empty_graph");
    cleanup(&path);
    let db = Database::<f32>::open(&path, DIM).unwrap();

    let results = db.query(r#"MATCH (n {name: "anyone"}) RETURN n"#).unwrap();
    assert!(results.is_empty(), "空库下查询应返回空");

    drop(db);
    cleanup(&path);
}

#[test]
fn 查询_语法错误_应返回Err() {
    let path = tmp_db("syntax_err");
    cleanup(&path);
    let db = Database::<f32>::open(&path, DIM).unwrap();

    let result = db.query("TOTALLY INVALID SYNTAX !!!@#$");
    assert!(result.is_err(), "无效语法应返回 Err");

    drop(db);
    cleanup(&path);
}