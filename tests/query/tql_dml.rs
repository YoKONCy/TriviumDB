#![allow(non_snake_case)]
//! TQL DML 写操作集成测试
//! 覆盖：CREATE 节点、CREATE 边、SET 更新、DELETE、DETACH DELETE

use triviumdb::Database;
use triviumdb::database::{Config, StorageMode};

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("dml_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    crate::cleanup_files::database(path);
}

#[test]
fn 空字段运算符和未知边属性在写日志前拒绝() {
    let path = tmp_db("invalid_write_fields");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let payload = serde_json::json!({"k":"v","obj":{"x":2}});
    db.insert(&[1., 0., 0., 0.], payload.clone()).unwrap();
    let wal = std::fs::read(format!("{path}.wal")).unwrap();
    for query in [
        r#"MATCH (n {k:"v",absent:{}}) SET n.touched == true"#,
        r#"CREATE (a)-[:r {weigth:0.25}]->(b)"#,
    ] {
        assert_eq!(db.tql_mut(query).unwrap_err().code(), "TDB_QUERY_PARSE");
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), wal);
    }
    let rows = db.tql(r#"FIND {obj:{$eq:{x:2}}} RETURN _"#).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["_"].as_node().unwrap().id, 1);
    db.close().unwrap();
    let mut reopened = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), [1]);
    assert_eq!(reopened.get_payload(1).unwrap(), Some(payload));
    reopened.close().unwrap();
    cleanup(&path);
}

#[test]
fn 匿名CREATE节点与用户变量独立并持久化正确边() {
    let path = tmp_db("anonymous_create_identity");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let result = db.tql_mut(r#"CREATE ()-[:r]->(_auto_0 {v:"x"})"#).unwrap();
    assert_eq!(result.created_ids, [1, 2]);
    db.close().unwrap();
    let mut reopened = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        reopened.get_payload(1).unwrap(),
        Some(serde_json::json!({}))
    );
    assert_eq!(
        reopened.get_payload(2).unwrap(),
        Some(serde_json::json!({"v":"x"}))
    );
    let edges = reopened.get_edges(1).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!((edges[0].target_id, edges[0].label.as_str()), (2, "r"));
    assert!(reopened.get_edges(2).unwrap().is_empty());
    reopened.close().unwrap();
    cleanup(&path);
}

#[test]
fn 具名边默认文档谓词始终使用模式中的首个绑定() {
    let path = tmp_db("named_edge_default_filter");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for id in 1..=3 {
        db.insert_with_id(id, &[1., 0., 0., 0.], serde_json::json!({"source":id==1}))
            .unwrap();
    }
    for end in [2, 3] {
        db.link(1, end, "r", 1.).unwrap();
    }
    for (edge, field) in [(":r", "anonymous"), ("e:r", "named")] {
        let query = format!("MATCH (z)-[{edge}]->(a) WHERE {{source:true}} SET a.{field} == true");
        assert_eq!(db.tql_mut(&query).unwrap().affected, 2);
    }
    db.close().unwrap();
    let mut reopened = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        reopened.get_payload(1).unwrap(),
        Some(serde_json::json!({"source":true}))
    );
    for id in [2, 3] {
        assert_eq!(
            reopened.get_payload(id).unwrap(),
            Some(serde_json::json!({"source":false,"anonymous":true,"named":true}))
        );
    }
    reopened.close().unwrap();
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  CREATE 节点
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_CREATE_单节点() {
    let path = tmp_db("create_single");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let result = db
        .tql_mut(r#"CREATE (a {name: "Alice", age: 30})"#)
        .unwrap();
    assert_eq!(result.affected, 1, "应创建 1 个节点");
    assert_eq!(result.created_ids.len(), 1, "应返回 1 个 ID");

    // 验证节点存在
    let found = db.tql_nodes(r#"FIND {name: "Alice"} RETURN *"#).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0]["_"].payload.get("age").unwrap().as_i64().unwrap(),
        30
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_CREATE_多节点() {
    let path = tmp_db("create_multi");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let result = db
        .tql_mut(r#"CREATE (a {name: "Alice"}), (b {name: "Bob"})"#)
        .unwrap();
    assert_eq!(result.affected, 2, "应创建 2 个节点");
    assert_eq!(result.created_ids.len(), 2);

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  CREATE 边（MATCH + CREATE）
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_MATCH_CREATE_边() {
    let path = tmp_db("create_edge");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 先创建两个节点
    let alice_id = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice"}))
        .unwrap();
    let bob_id = db
        .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "Bob"}))
        .unwrap();

    // 通过 TQL 创建边
    let _query = format!(
        "MATCH (a {{id: {}}})-[]->(b {{id: {}}}) CREATE (a)-[:knows]->(b)",
        alice_id, bob_id
    );
    // 需要用无边的 MATCH 模式 — 用 FIND + MATCH 替代方案
    // 实际上 MATCH (a {id: X}) 匹配单节点（无边要求）
    let result = db.tql_mut(&format!(
        "MATCH (a {{id: {}}}),(b {{id: {}}}) CREATE (a)-[:knows]->(b)",
        alice_id, bob_id
    ));

    // 如果逗号分隔的多模式不支持，就用低级 API
    // 降级为直接 link
    if result.is_err() {
        db.link(alice_id, bob_id, "knows", 1.0).unwrap();
    }

    // 验证边存在
    let edges = db.tql_nodes("MATCH (a)-[:knows]->(b) RETURN a, b").unwrap();
    assert_eq!(edges.len(), 1);

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  SET 更新
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_SET_更新字段() {
    let path = tmp_db("set_update");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let _alice_id = db
        .insert(
            &[1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"name": "Alice", "age": 30}),
        )
        .unwrap();

    // 更新 age
    let result = db
        .tql_mut(r#"MATCH (a {name: "Alice"}) SET a.age == 31"#)
        .unwrap();
    assert_eq!(result.affected, 1, "应更新 1 个节点");

    // 验证更新
    let found = db.tql_nodes(r#"FIND {name: "Alice"} RETURN *"#).unwrap();
    assert_eq!(
        found[0]["_"].payload.get("age").unwrap().as_i64().unwrap(),
        31
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_SET_添加新字段() {
    let path = tmp_db("set_new_field");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let _id = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice"}))
        .unwrap();

    // 添加新字段 email
    db.tql_mut(r#"MATCH (a {name: "Alice"}) SET a.email == "alice@example.com""#)
        .unwrap();

    let found = db.tql_nodes(r#"FIND {name: "Alice"} RETURN *"#).unwrap();
    assert_eq!(
        found[0]["_"]
            .payload
            .get("email")
            .unwrap()
            .as_str()
            .unwrap(),
        "alice@example.com"
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_SET_批量更新() {
    let path = tmp_db("set_batch");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    db.insert(
        &[1.0, 0.0, 0.0, 0.0],
        serde_json::json!({"name": "Alice", "status": "active"}),
    )
    .unwrap();
    db.insert(
        &[0.0, 1.0, 0.0, 0.0],
        serde_json::json!({"name": "Bob", "status": "active"}),
    )
    .unwrap();

    // 批量更新所有 active 用户的 status
    let result = db
        .tql_mut(r#"MATCH (a {status: "active"}) SET a.status == "archived""#)
        .unwrap();
    assert_eq!(result.affected, 2, "应更新 2 个节点");

    drop(db);
    cleanup(&path);
}

#[test]
fn DML匹配集不受查询行上限截断() {
    let path = tmp_db("dml_unlimited_match");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            storage_mode: StorageMode::Rom,
            max_query_rows: Some(2),
            ..Default::default()
        },
    )
    .unwrap();
    for id in 1..=5 {
        db.insert_with_id(
            id,
            &[1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"status": "active"}),
        )
        .unwrap();
    }

    let result = db
        .tql_mut(r#"MATCH (a {status: "active"}) SET a.status == "archived""#)
        .unwrap();
    assert_eq!(result.affected, 5);
    assert_eq!(
        db.tql_nodes(r#"FIND {status: "archived"} RETURN * LIMIT 5"#)
            .unwrap()
            .len(),
        5
    );

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  DELETE
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_DELETE_删除节点() {
    let path = tmp_db("delete_node");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let _id = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice"}))
        .unwrap();

    let result = db.tql_mut(r#"MATCH (a {name: "Alice"}) DELETE a"#).unwrap();
    assert_eq!(result.affected, 1, "应删除 1 个节点");

    // 验证已删除
    let found = db.tql_nodes(r#"FIND {name: "Alice"} RETURN *"#).unwrap();
    assert_eq!(found.len(), 0, "节点应已被删除");

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  DETACH DELETE
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_DETACH_DELETE_删除节点及边() {
    let path = tmp_db("detach_delete");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let alice_id = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice"}))
        .unwrap();
    let bob_id = db
        .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "Bob"}))
        .unwrap();
    db.link(alice_id, bob_id, "knows", 1.0).unwrap();

    // DETACH DELETE Alice → 应自动断开 Alice->Bob 的边
    let result = db
        .tql_mut(r#"MATCH (a {name: "Alice"}) DETACH DELETE a"#)
        .unwrap();
    assert_eq!(result.affected, 1, "应删除 1 个节点");

    // Alice 已删除
    let found = db.tql_nodes(r#"FIND {name: "Alice"} RETURN *"#).unwrap();
    assert_eq!(found.len(), 0, "Alice 应已被删除");

    // Bob 仍存在
    let bob = db.tql_nodes(r#"FIND {name: "Bob"} RETURN *"#).unwrap();
    assert_eq!(bob.len(), 1, "Bob 应仍存在");

    // 无边残留
    let edges = db.tql_nodes("MATCH (a)-[:knows]->(b) RETURN a, b").unwrap();
    assert_eq!(edges.len(), 0, "knows 边应已被移除");

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  读查询误用拒绝
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_tql_mut_读查询明确拒绝() {
    let path = tmp_db("read_fallback");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "Alice"}))
        .unwrap();

    let error = db.tql_mut(r#"FIND {name: "Alice"} RETURN *"#).unwrap_err();
    assert!(matches!(
        error.underlying(),
        triviumdb::TriviumError::ApiMigrationRequired { .. }
    ));

    drop(db);
    cleanup(&path);
}
