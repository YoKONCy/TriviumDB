#![allow(non_snake_case)]
//! 属性二级索引集成测试
//!
//! 覆盖：
//! - 索引注册 + 回填
//! - 索引加速 MATCH / FIND 查询
//! - insert/update/delete 后索引自动维护
//! - 索引删除

use triviumdb::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("idx_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

// ═══════════════════════════════════════════════════════════════
//  索引创建 + 查询加速
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_创建索引后_FIND加速() {
    let path = tmp_db("find_accel");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 先插入数据
    db.insert(
        &[1.0, 0.0, 0.0, 0.0],
        serde_json::json!({"name": "Alice", "type": "person"}),
    )
    .unwrap();
    db.insert(
        &[0.0, 1.0, 0.0, 0.0],
        serde_json::json!({"name": "Bob", "type": "person"}),
    )
    .unwrap();
    db.insert(
        &[0.0, 0.0, 1.0, 0.0],
        serde_json::json!({"name": "Summit", "type": "event"}),
    )
    .unwrap();

    // 创建索引（会回填已有数据）
    db.create_index("type").unwrap();

    // 使用索引加速的 FIND
    let results = db.tql_nodes(r#"FIND {type: "person"} RETURN *"#).unwrap();
    assert_eq!(results.len(), 2, "应找到 Alice 和 Bob");

    let results = db.tql_nodes(r#"FIND {type: "event"} RETURN *"#).unwrap();
    assert_eq!(results.len(), 1, "应找到 Summit");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_创建索引后_MATCH加速() {
    let path = tmp_db("match_accel");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let alice = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice"}))
        .unwrap();
    let bob = db
        .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "Bob"}))
        .unwrap();
    db.link(alice, bob, "knows", 1.0).unwrap();

    // 创建 name 索引
    db.create_index("name").unwrap();

    // MATCH 使用索引定位起点
    let results = db
        .tql_nodes(r#"MATCH (a {name: "Alice"})-[:knows]->(b) RETURN b"#)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0]["b"]
            .payload
            .get("name")
            .unwrap()
            .as_str()
            .unwrap(),
        "Bob"
    );

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  索引自动维护
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_新插入节点自动进入索引() {
    let path = tmp_db("auto_insert");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 先创建索引
    db.create_index("name").unwrap();

    // 之后插入的节点应自动进入索引
    db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice"}))
        .unwrap();
    db.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "Bob"}))
        .unwrap();

    let results = db.tql_nodes(r#"FIND {name: "Alice"} RETURN *"#).unwrap();
    assert_eq!(results.len(), 1);

    let results = db.tql_nodes(r#"FIND {name: "Bob"} RETURN *"#).unwrap();
    assert_eq!(results.len(), 1);

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_更新后索引同步() {
    let path = tmp_db("auto_update");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    db.create_index("status").unwrap();

    db.insert(
        &[1.0, 0.0, 0.0, 0.0],
        serde_json::json!({"name": "Alice", "status": "active"}),
    )
    .unwrap();

    // 更新 status
    db.tql_mut(r#"MATCH (a {name: "Alice"}) SET a.status == "archived""#)
        .unwrap();

    // 旧值查不到
    let results = db.tql_nodes(r#"FIND {status: "active"} RETURN *"#).unwrap();
    assert_eq!(results.len(), 0, "active 应已无结果");

    // 新值能查到
    let results = db
        .tql_nodes(r#"FIND {status: "archived"} RETURN *"#)
        .unwrap();
    assert_eq!(results.len(), 1, "archived 应有 Alice");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_删除后索引清理() {
    let path = tmp_db("auto_delete");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    db.create_index("name").unwrap();

    db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice"}))
        .unwrap();
    db.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "Bob"}))
        .unwrap();

    // 删除 Alice
    db.tql_mut(r#"MATCH (a {name: "Alice"}) DELETE a"#).unwrap();

    // Alice 查不到
    let results = db.tql_nodes(r#"FIND {name: "Alice"} RETURN *"#).unwrap();
    assert_eq!(results.len(), 0, "Alice 应已被删除");

    // Bob 仍在
    let results = db.tql_nodes(r#"FIND {name: "Bob"} RETURN *"#).unwrap();
    assert_eq!(results.len(), 1, "Bob 应仍存在");

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  索引删除
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_删除索引后仍可查询() {
    let path = tmp_db("drop_index");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    db.create_index("name").unwrap();
    db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice"}))
        .unwrap();

    // 有索引时可以查
    let results = db.tql_nodes(r#"FIND {name: "Alice"} RETURN *"#).unwrap();
    assert_eq!(results.len(), 1);

    // 删除索引
    db.drop_index("name").unwrap();

    // 仍可查（退化为全扫描）
    let results = db.tql_nodes(r#"FIND {name: "Alice"} RETURN *"#).unwrap();
    assert_eq!(results.len(), 1, "删除索引后仍应能通过全扫描找到");

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  多字段索引
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_多字段索引() {
    let path = tmp_db("multi_field");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    db.create_index("name").unwrap();
    db.create_index("type").unwrap();

    db.insert(
        &[1.0, 0.0, 0.0, 0.0],
        serde_json::json!({"name": "Alice", "type": "person"}),
    )
    .unwrap();
    db.insert(
        &[0.0, 1.0, 0.0, 0.0],
        serde_json::json!({"name": "Summit", "type": "event"}),
    )
    .unwrap();
    db.insert(
        &[0.0, 0.0, 1.0, 0.0],
        serde_json::json!({"name": "Report", "type": "event"}),
    )
    .unwrap();

    let results = db.tql_nodes(r#"FIND {type: "event"} RETURN *"#).unwrap();
    assert_eq!(results.len(), 2, "应找到 2 个 event");

    let results = db.tql_nodes(r#"FIND {name: "Alice"} RETURN *"#).unwrap();
    assert_eq!(results.len(), 1, "应找到 1 个 Alice");

    drop(db);
    cleanup(&path);
}
