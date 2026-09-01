#![allow(non_snake_case)]
//! Phase 2 新增特性集成测试
//! 覆盖：DISTINCT、聚合函数 (COUNT/SUM/AVG/MIN/MAX/COLLECT)、OPTIONAL MATCH、AS 别名
//! 以及标签索引下推优化

use triviumdb::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("p2_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

/// 构建测试图谱：
/// Alice(age=30) -[:knows]-> Bob(age=25) -[:knows]-> Carol(age=35)
/// Alice -[:works_at]-> Acme(type="company")
/// Dave(age=28) 无边，孤立节点
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
    let _dave_id = db
        .insert(
            &[0.5, 0.5, 0.0, 0.0],
            serde_json::json!({"name": "Dave", "age": 28}),
        )
        .unwrap();

    db.link(alice_id, bob_id, "knows", 1.0).unwrap();
    db.link(bob_id, carol_id, "knows", 1.0).unwrap();
    db.link(alice_id, acme_id, "works_at", 1.0).unwrap();

    db
}

// ═══════════════════════════════════════════════════════════════
//  聚合函数
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_聚合函数_COUNT() {
    let path = tmp_db("agg_count");
    let db = build_test_db(&path);

    // 统计 Alice 的 knows 好友数
    let results = db
        .tql("MATCH (a)-[:knows]->(b) RETURN count(b) AS friend_count")
        .unwrap();
    assert_eq!(results.len(), 1, "COUNT 应聚合为单行");

    let row = &results[0];
    let count_node = &row["friend_count"];
    let count_val = count_node.payload.get("friend_count").unwrap();
    assert_eq!(count_val.as_i64().unwrap(), 2, "图中有 2 条 knows 边");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_聚合函数_AVG() {
    let path = tmp_db("agg_avg");
    let db = build_test_db(&path);

    let results = db
        .tql("MATCH (a)-[:knows]->(b) RETURN avg(b.age) AS avg_age")
        .unwrap();
    assert_eq!(results.len(), 1);

    let avg_node = &results[0]["avg_age"];
    let avg_val = avg_node.payload.get("avg_age").unwrap().as_f64().unwrap();
    // Bob=25, Carol=35 → avg = 30.0
    assert!(
        (avg_val - 30.0).abs() < 0.01,
        "AVG(age) should be 30.0, got {}",
        avg_val
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_聚合函数_SUM_MIN_MAX() {
    let path = tmp_db("agg_sum_min_max");
    let db = build_test_db(&path);

    let results = db.tql("MATCH (a)-[:knows]->(b) RETURN sum(b.age) AS total, min(b.age) AS youngest, max(b.age) AS oldest").unwrap();
    assert_eq!(results.len(), 1);

    let row = &results[0];
    let total = row["total"].payload.get("total").unwrap().as_f64().unwrap();
    let youngest = row["youngest"]
        .payload
        .get("youngest")
        .unwrap()
        .as_f64()
        .unwrap();
    let oldest = row["oldest"]
        .payload
        .get("oldest")
        .unwrap()
        .as_f64()
        .unwrap();

    assert!((total - 60.0).abs() < 0.01, "SUM should be 60");
    assert!((youngest - 25.0).abs() < 0.01, "MIN should be 25");
    assert!((oldest - 35.0).abs() < 0.01, "MAX should be 35");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_聚合函数_COLLECT() {
    let path = tmp_db("agg_collect");
    let db = build_test_db(&path);

    let results = db
        .tql("MATCH (a)-[:knows]->(b) RETURN collect(b.name) AS names")
        .unwrap();
    assert_eq!(results.len(), 1);

    let names = results[0]["names"].payload.get("names").unwrap();
    let arr = names.as_array().unwrap();
    assert_eq!(arr.len(), 2);

    // 名字应包含 Bob 和 Carol（顺序不确定）
    let name_strs: Vec<&str> = arr.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(name_strs.contains(&"Bob"));
    assert!(name_strs.contains(&"Carol"));

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  DISTINCT
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_DISTINCT_去重() {
    let path = tmp_db("distinct");
    let db = build_test_db(&path);

    // 不带 DISTINCT，Alice 出现 2 次（Alice->Bob, Alice->Acme 各一条路径的 a 都是 Alice）
    let all_results = db.tql("MATCH (a)-[]->(b) RETURN a, b").unwrap();
    let distinct_results = db.tql("MATCH (a)-[]->(b) RETURN DISTINCT a").unwrap();

    // all_results 有 3 条（Alice->Bob, Bob->Carol, Alice->Acme）
    assert_eq!(all_results.len(), 3);
    // DISTINCT a 应去重 Alice，只剩 2 个唯一的 a（Alice 和 Bob）
    assert_eq!(distinct_results.len(), 2, "DISTINCT 应去除重复行");

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  AS 别名
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_AS别名_属性访问() {
    let path = tmp_db("as_alias");
    let db = build_test_db(&path);

    let results = db
        .tql("MATCH (a)-[:knows]->(b) RETURN a.name, b.name")
        .unwrap();
    // 应该能解析并返回结果
    assert!(!results.is_empty(), "属性投影应返回结果");

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  OPTIONAL MATCH
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_OPTIONAL_MATCH_语法解析() {
    let path = tmp_db("optional_match");
    let db = build_test_db(&path);

    // OPTIONAL MATCH 正常匹配
    let results = db
        .tql("OPTIONAL MATCH (a)-[:knows]->(b) RETURN a, b")
        .unwrap();
    assert!(!results.is_empty(), "OPTIONAL MATCH 有匹配时应返回结果");

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  标签索引下推优化
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_标签索引下推_减少扫描() {
    let path = tmp_db("label_pushdown");
    let db = build_test_db(&path);

    // 不带标签约束 → 全扫描
    let r1 = db.tql("MATCH (a)-[]->(b) RETURN a, b").unwrap();
    // 带标签约束 → 使用 label_index 缩小候选集
    let r2 = db.tql("MATCH (a)-[:works_at]->(b) RETURN a, b").unwrap();

    assert_eq!(
        r1.len(),
        3,
        "无标签约束：Alice->Bob, Bob->Carol, Alice->Acme"
    );
    assert_eq!(r2.len(), 1, "标签约束 works_at：仅 Alice->Acme");

    // 验证结果正确性
    let name = r2[0]["b"].payload.get("name").unwrap().as_str().unwrap();
    assert_eq!(name, "Acme");

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  EXPLAIN 查询计划
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_EXPLAIN_基础计划() {
    let path = tmp_db("explain_basic");
    let db = build_test_db(&path);

    let results = db
        .tql("EXPLAIN MATCH (a)-[:knows]->(b) RETURN a, b")
        .unwrap();
    assert_eq!(results.len(), 1, "EXPLAIN 应返回单行计划");

    let plan = &results[0]["plan"].payload;
    assert_eq!(plan.get("entry").unwrap().as_str().unwrap(), "MATCH");
    assert!(
        plan.get("detail")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("knows")
    );
    assert_eq!(plan.get("total_nodes").unwrap().as_i64().unwrap(), 5);

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_EXPLAIN_标签索引下推策略() {
    let path = tmp_db("explain_label");
    let db = build_test_db(&path);

    let results = db
        .tql("EXPLAIN MATCH (a)-[:works_at]->(b) RETURN a, b")
        .unwrap();
    let plan = &results[0]["plan"].payload;

    let strategy = plan.get("candidate_strategy").unwrap().as_str().unwrap();
    assert!(
        strategy.contains("label_index"),
        "策略应显示标签索引下推, got: {}",
        strategy
    );
    assert!(strategy.contains("works_at"), "策略应包含标签名");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_EXPLAIN_ID短路策略() {
    let path = tmp_db("explain_id");
    let db = build_test_db(&path);

    let results = db
        .tql("EXPLAIN MATCH (a {id: 1})-[:knows]->(b) RETURN b")
        .unwrap();
    let plan = &results[0]["plan"].payload;

    let strategy = plan.get("candidate_strategy").unwrap().as_str().unwrap();
    assert!(
        strategy.contains("id_shortcut"),
        "策略应显示 ID 短路, got: {}",
        strategy
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_EXPLAIN_全扫描策略() {
    let path = tmp_db("explain_fullscan");
    let db = build_test_db(&path);

    let results = db.tql("EXPLAIN MATCH (a)-[]->(b) RETURN a, b").unwrap();
    let plan = &results[0]["plan"].payload;

    let strategy = plan.get("candidate_strategy").unwrap().as_str().unwrap();
    assert!(
        strategy.contains("full_scan"),
        "无约束应显示全扫描, got: {}",
        strategy
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_EXPLAIN_优化提示() {
    let path = tmp_db("explain_opts");
    let db = build_test_db(&path);

    // 聚合 + LIMIT → 应显示 aggregation + LIMIT early termination
    let results = db
        .tql("EXPLAIN MATCH (a)-[:knows]->(b) RETURN count(b) AS cnt LIMIT 10")
        .unwrap();
    let plan = &results[0]["plan"].payload;

    let opts = plan.get("optimizations").unwrap().as_array().unwrap();
    let opt_strs: Vec<&str> = opts.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(opt_strs.contains(&"aggregation"), "应包含 aggregation 优化");
    assert!(
        opt_strs.contains(&"LIMIT early termination"),
        "应包含 LIMIT 优化"
    );
    assert!(
        opt_strs.contains(&"label index pushdown"),
        "应包含标签索引下推"
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_EXPLAIN_FIND入口() {
    let path = tmp_db("explain_find");
    let db = build_test_db(&path);

    let results = db.tql(r#"EXPLAIN FIND {name: "Alice"} RETURN *"#).unwrap();
    let plan = &results[0]["plan"].payload;

    assert_eq!(plan.get("entry").unwrap().as_str().unwrap(), "FIND");
    assert_eq!(
        plan.get("candidate_strategy").unwrap().as_str().unwrap(),
        "full_scan"
    );

    drop(db);
    cleanup(&path);
}

// ═══════════════════════════════════════════════════════════════
//  投影裁剪
// ═══════════════════════════════════════════════════════════════

#[test]
fn 测试_投影裁剪_属性引用剥离向量() {
    let path = tmp_db("proj_pruning");
    let db = build_test_db(&path);

    // RETURN a.name, b.age → a 和 b 都是仅属性引用，vector + edges 应被清空
    let results = db
        .tql("MATCH (a)-[:knows]->(b) RETURN a.name, b.age")
        .unwrap();
    assert!(!results.is_empty());

    for row in &results {
        for node in row.values() {
            // 投影裁剪后，仅属性引用的变量应无 vector 和 edges
            assert!(node.vector.is_empty(), "属性引用节点的 vector 应被裁剪");
            assert!(node.edges.is_empty(), "属性引用节点的 edges 应被裁剪");
            // payload 保留
            assert!(!node.payload.is_null(), "payload 应保留");
        }
    }

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_投影裁剪_完整引用保留向量() {
    let path = tmp_db("proj_no_prune");
    let db = build_test_db(&path);

    // RETURN a, b.name → a 是完整引用（保留 vector），b 是属性引用（裁剪 vector）
    let results = db.tql("MATCH (a)-[:knows]->(b) RETURN a, b.name").unwrap();
    assert!(!results.is_empty());

    for row in &results {
        // a 是完整引用，应保留 vector
        let a = &row["a"];
        assert!(!a.vector.is_empty(), "完整引用 a 的 vector 应保留");

        // b 是仅属性引用，应被裁剪
        let b = &row["b"];
        assert!(b.vector.is_empty(), "属性引用 b 的 vector 应被裁剪");
    }

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_投影裁剪_EXPLAIN显示() {
    let path = tmp_db("proj_explain");
    let db = build_test_db(&path);

    let results = db
        .tql("EXPLAIN MATCH (a)-[:knows]->(b) RETURN a.name, b.age")
        .unwrap();
    let plan = &results[0]["plan"].payload;

    let opts = plan.get("optimizations").unwrap().as_array().unwrap();
    let opt_strs: Vec<&str> = opts.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(
        opt_strs.contains(&"projection pruning"),
        "EXPLAIN 应显示投影裁剪优化"
    );

    drop(db);
    cleanup(&path);
}
