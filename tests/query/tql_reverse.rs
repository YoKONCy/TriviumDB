#![allow(non_snake_case)]
//! TQL 反向边 & 双向边遍历集成测试
//!
//! 覆盖：
//! - (a)<-[:label]-(b)  反向单跳
//! - (a)<-[:label*1..K]-(b)  反向可变长路径
//! - (a)-[:label]-(b)  双向遍历
//! - 入度统计（反向 COUNT 聚合）

use triviumdb::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("rev_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

/// 构建测试图谱：
/// Alice -[:knows]-> Bob -[:knows]-> Carol
/// Alice -[:works_at]-> Acme
/// Dave -[:knows]-> Bob
fn build_test_db(path: &str) -> Database<f32> {
    cleanup(path);
    let mut db = Database::<f32>::open(path, DIM).unwrap();

    let alice_id = db
        .insert(
            &[1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"name": "Alice", "age": 30}),
        )
        .unwrap();
    let bob_id = db
        .insert(
            &[0.0, 1.0, 0.0, 0.0],
            serde_json::json!({"name": "Bob", "age": 25}),
        )
        .unwrap();
    let carol_id = db
        .insert(
            &[0.0, 0.0, 1.0, 0.0],
            serde_json::json!({"name": "Carol", "age": 35}),
        )
        .unwrap();
    let acme_id = db
        .insert(
            &[0.0, 0.0, 0.0, 1.0],
            serde_json::json!({"name": "Acme", "type": "company"}),
        )
        .unwrap();
    let dave_id = db
        .insert(
            &[0.5, 0.5, 0.0, 0.0],
            serde_json::json!({"name": "Dave", "age": 28}),
        )
        .unwrap();

    db.link(alice_id, bob_id, "knows", 1.0).unwrap(); // Alice -> Bob
    db.link(bob_id, carol_id, "knows", 1.0).unwrap(); // Bob -> Carol
    db.link(alice_id, acme_id, "works_at", 1.0).unwrap(); // Alice -> Acme
    db.link(dave_id, bob_id, "knows", 1.0).unwrap(); // Dave -> Bob

    db
}

// ═══════════════════════════════════════════════════════════════
//  反向单跳
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_反向单跳_谁认识Bob() {
    let path = tmp_db("rev_single");
    let db = build_test_db(&path);

    // Bob <-[:knows]- (b) → 谁 knows Bob？→ Alice 和 Dave
    let results = db
        .tql(r#"MATCH (a {name: "Bob"})<-[:knows]-(b) RETURN b"#)
        .unwrap();
    assert_eq!(results.len(), 2, "Alice 和 Dave 都 knows Bob");

    let names: Vec<&str> = results
        .iter()
        .map(|r| r["b"].payload.get("name").unwrap().as_str().unwrap())
        .collect();
    assert!(names.contains(&"Alice"));
    assert!(names.contains(&"Dave"));

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_反向单跳_无标签() {
    let path = tmp_db("rev_any");
    let db = build_test_db(&path);

    // Bob <-[]- (b) → 所有指向 Bob 的节点
    let results = db
        .tql(r#"MATCH (a {name: "Bob"})<-[]-(b) RETURN b"#)
        .unwrap();
    assert_eq!(results.len(), 2, "Alice 和 Dave 都指向 Bob");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_反向单跳_Carol() {
    let path = tmp_db("rev_carol");
    let db = build_test_db(&path);

    // Carol <-[:knows]- (b) → 谁 knows Carol？→ 只有 Bob
    let results = db
        .tql(r#"MATCH (a {name: "Carol"})<-[:knows]-(b) RETURN b"#)
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
//  反向可变长路径
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_反向可变长_溯源() {
    let path = tmp_db("rev_varlen");
    let db = build_test_db(&path);

    // Carol <-[:knows*1..2]- (b) → 1跳: Bob, 2跳: Alice, Dave
    let results = db
        .tql(r#"MATCH (a {name: "Carol"})<-[:knows*1..2]-(b) RETURN b"#)
        .unwrap();
    // 1-hop: Bob
    // 2-hop from Bob: Alice, Dave
    assert!(
        results.len() >= 3,
        "应至少找到 Bob, Alice, Dave; got {}",
        results.len()
    );

    let names: Vec<&str> = results
        .iter()
        .map(|r| r["b"].payload.get("name").unwrap().as_str().unwrap())
        .collect();
    assert!(names.contains(&"Bob"), "1跳: Bob");
    assert!(names.contains(&"Alice"), "2跳: Alice");
    assert!(names.contains(&"Dave"), "2跳: Dave");

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  双向遍历
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_双向遍历_Bob的所有关联() {
    let path = tmp_db("bidir");
    let db = build_test_db(&path);

    // Bob -[:knows]- (b) → 双向：Bob->Carol (正向) + Alice->Bob,Dave->Bob (反向)
    let results = db
        .tql(r#"MATCH (a {name: "Bob"})-[:knows]-(b) RETURN b"#)
        .unwrap();
    assert_eq!(results.len(), 3, "正向Carol + 反向Alice,Dave");

    let names: Vec<&str> = results
        .iter()
        .map(|r| r["b"].payload.get("name").unwrap().as_str().unwrap())
        .collect();
    assert!(names.contains(&"Carol"), "正向: Bob->Carol");
    assert!(names.contains(&"Alice"), "反向: Alice->Bob");
    assert!(names.contains(&"Dave"), "反向: Dave->Bob");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_双向遍历_无标签() {
    let path = tmp_db("bidir_any");
    let db = build_test_db(&path);

    // Alice -[]- (b) → 正向: Bob,Carol,Acme + 反向: (无人指向Alice)
    // Wait... Alice has no incoming edges in our test graph
    // So only forward: Bob, Acme (knows edges) + no backward
    // Actually Alice -> Bob (knows), Alice -> Acme (works_at)
    // Forward all: Bob, Acme; Backward: nobody
    let results = db
        .tql(r#"MATCH (a {name: "Alice"})-[]-(b) RETURN b"#)
        .unwrap();
    // Alice outgoing: Bob(knows), Carol? no Alice->Carol is not in graph
    // Alice outgoing: Bob(knows), Acme(works_at)  — that's 2 forward
    // Alice incoming: nobody
    assert_eq!(results.len(), 2, "Alice 正向: Bob, Acme; 无反向入边");

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  反向 + 聚合 → 入度统计
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_入度统计_反向COUNT() {
    let path = tmp_db("in_degree");
    let db = build_test_db(&path);

    // 统计 Bob 的 knows 入度
    let results = db
        .tql(r#"MATCH (a {name: "Bob"})<-[:knows]-(b) RETURN count(b) AS in_degree"#)
        .unwrap();
    assert_eq!(results.len(), 1);

    let count = results[0]["in_degree"]
        .payload
        .get("in_degree")
        .unwrap()
        .as_i64()
        .unwrap();
    assert_eq!(count, 2, "Bob 的 knows 入度应为 2 (Alice + Dave)");

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  EXPLAIN 反向策略
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_EXPLAIN_反向模式() {
    let path = tmp_db("explain_rev");
    let db = build_test_db(&path);

    let results = db
        .tql(r#"EXPLAIN MATCH (a {name: "Bob"})<-[:knows]-(b) RETURN b"#)
        .unwrap();
    let plan = &results[0]["plan"].payload;
    let detail = plan.get("detail").unwrap().as_str().unwrap();
    assert!(detail.contains("knows"), "EXPLAIN 应展示边标签");

    drop(db);
    cleanup(&path);
}
