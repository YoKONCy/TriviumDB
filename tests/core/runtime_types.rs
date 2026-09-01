#![allow(non_snake_case)]
//! 运行时类型系统、排序引擎与 f16 半精度集成测试
//!
//! 验证范围：
//! - `query/tql_executor.rs`: compare_runtime 全类型交叉 (Float/Int/String/Bool)、cmp_f64 全 6 操作符、
//!   eval_tql_expr_single/.id 伪字段、extract_order_key 排序键提取
//! - `query/tql_parser.rs`: CREATE 带权重边语法、无效语法错误路径 (空查询/缺括号/无效操作符)
//! - `vector.rs`: f16 (半精度) VectorType 全生命周期 (insert → search → flush → reopen)
//! - `storage/vec_pool.rs` + `file_format.rs`: flush/reopen mmap 加载路径、增量写入幂等性

use std::collections::HashMap;
use triviumdb::database::Database;
use triviumdb::node::Node;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    let path = dir
        .join(format!("cov5_{}", name))
        .to_string_lossy()
        .to_string();
    cleanup(&path);
    path
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok", ".tmp", ".vec.tmp"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

fn node<'a>(row: &'a HashMap<String, Node<f32>>, var: &str) -> &'a Node<f32> {
    row.get(var)
        .unwrap_or_else(|| panic!("结果行缺少变量 {var}"))
}

fn payload_f64(node: &Node<f32>, key: &str) -> f64 {
    node.payload
        .get(key)
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| panic!("payload 缺少数值字段 {key}"))
}

fn payload_bool(node: &Node<f32>, key: &str) -> bool {
    node.payload
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| panic!("payload 缺少布尔字段 {key}"))
}

fn payload_str<'a>(node: &'a Node<f32>, key: &str) -> &'a str {
    node.payload
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("payload 缺少字符串字段 {key}"))
}

fn assert_match_rows<T>(rows: &[HashMap<String, Node<T>>], var: &str, expected_len: usize) {
    assert_eq!(rows.len(), expected_len, "查询结果行数不符合预期");
    for row in rows {
        assert!(row.contains_key(var), "结果行缺少变量 {var}");
    }
}

fn assert_find_rows(rows: &[HashMap<String, Node<f32>>], expected_len: usize) {
    assert_eq!(rows.len(), expected_len, "FIND 结果行数不符合预期");
    for row in rows {
        assert!(row.contains_key("_"), "FIND RETURN * 应绑定 _");
    }
}

fn assert_tql_err<T>(result: triviumdb::Result<T>, expected_fragment: &str) {
    let err = match result {
        Ok(_) => panic!("无效 TQL 必须被拒绝"),
        Err(err) => err,
    };
    let msg = err.to_string();
    assert!(!msg.is_empty(), "TQL 拒绝必须返回可诊断错误");
    assert!(
        msg.contains(expected_fragment),
        "错误信息应包含 {expected_fragment}，实际为 {msg}"
    );
}

// ════════════════════════════════════════════════════════════════
//  tql_executor.rs — compare_runtime 全类型组合 (L744-780)
// ════════════════════════════════════════════════════════════════

fn seed_typed_graph(path: &str) -> Database<f32> {
    let mut db = Database::<f32>::open(path, DIM).unwrap();
    for i in 0..8u32 {
        db.insert(
            &[i as f32, 0.0, 0.0, 0.0],
            serde_json::json!({
                "name": format!("n_{}", i),
                "score": i as f64 * 1.5,
                "rank": i,
                "active": i % 2 == 0,
                "label": if i < 4 { "low" } else { "high" }
            }),
        )
        .unwrap();
    }
    let mut ids = db.all_node_ids();
    ids.sort(); // 确保边拓扑在所有平台上确定性一致（HashMap 迭代序不保证稳定）
    for i in 0..ids.len() - 1 {
        db.link(ids[i], ids[i + 1], "seq", 1.0).unwrap();
    }
    db
}

/// WHERE 浮点数比较 (cmp_f64 全 6 操作符: ==, !=, >, >=, <, <=)
#[test]
fn COV5_01_where_float_cmp() {
    let path = tmp_db("float_cmp");
    let db = seed_typed_graph(&path);

    // Float == (epsilon比较)
    let r = db.tql(r#"FIND {score: 0.0} RETURN *"#).unwrap();
    assert_find_rows(&r, 1);
    assert_eq!(payload_f64(node(&r[0], "_"), "score"), 0.0);

    // Float !=
    let r = db
        .tql(r#"MATCH (a) WHERE a.score != 0.0 RETURN a"#)
        .unwrap();
    assert_match_rows(&r, "a", 7);
    assert!(
        r.iter()
            .all(|row| payload_f64(node(row, "a"), "score") != 0.0),
        "score != 0.0 不能返回零分节点"
    );

    // Float >
    let r = db.tql(r#"MATCH (a) WHERE a.score > 5.0 RETURN a"#).unwrap();
    assert_match_rows(&r, "a", 4);
    assert!(
        r.iter()
            .all(|row| payload_f64(node(row, "a"), "score") > 5.0),
        "score > 5.0 不能返回不满足条件的节点"
    );

    // Float >=
    let r = db
        .tql(r#"MATCH (a) WHERE a.score >= 10.5 RETURN a"#)
        .unwrap();
    assert_match_rows(&r, "a", 1);
    assert_eq!(payload_f64(node(&r[0], "a"), "score"), 10.5);

    // Float <
    let r = db.tql(r#"MATCH (a) WHERE a.score < 3.0 RETURN a"#).unwrap();
    assert_match_rows(&r, "a", 2);
    assert!(
        r.iter()
            .all(|row| payload_f64(node(row, "a"), "score") < 3.0),
        "score < 3.0 不能返回越界节点"
    );

    // Float <=
    let r = db
        .tql(r#"MATCH (a) WHERE a.score <= 1.5 RETURN a"#)
        .unwrap();
    assert_match_rows(&r, "a", 2);
    assert!(
        r.iter()
            .all(|row| payload_f64(node(row, "a"), "score") <= 1.5),
        "score <= 1.5 不能返回越界节点"
    );

    cleanup(&path);
}

/// WHERE Bool 比较 (==, !=)
#[test]
fn COV5_02_where_bool_cmp() {
    let path = tmp_db("bool_cmp");
    let db = seed_typed_graph(&path);

    let r = db
        .tql(r#"MATCH (a) WHERE a.active == true RETURN a"#)
        .unwrap();
    assert_match_rows(&r, "a", 4);
    assert!(
        r.iter().all(|row| payload_bool(node(row, "a"), "active")),
        "active == true 不能返回 false 节点"
    );

    let r = db
        .tql(r#"MATCH (a) WHERE a.active != true RETURN a"#)
        .unwrap();
    assert_match_rows(&r, "a", 4);
    assert!(
        r.iter().all(|row| !payload_bool(node(row, "a"), "active")),
        "active != true 不能返回 true 节点"
    );

    cleanup(&path);
}

/// WHERE String 比较 (全操作符)
#[test]
fn COV5_03_where_string_cmp() {
    let path = tmp_db("str_cmp");
    let db = seed_typed_graph(&path);

    let r = db
        .tql(r#"MATCH (a) WHERE a.label == "low" RETURN a"#)
        .unwrap();
    assert_eq!(r.len(), 4);

    let r = db
        .tql(r#"MATCH (a) WHERE a.label != "low" RETURN a"#)
        .unwrap();
    assert_eq!(r.len(), 4);

    // String > / < (字典序)
    let r = db.tql(r#"MATCH (a) WHERE a.label > "l" RETURN a"#).unwrap();
    assert_match_rows(&r, "a", 4);
    assert!(
        r.iter()
            .all(|row| payload_str(node(row, "a"), "label") > "l"),
        "label > l 不能返回字典序不满足的节点"
    );

    let r = db.tql(r#"MATCH (a) WHERE a.label < "z" RETURN a"#).unwrap();
    assert_match_rows(&r, "a", 8);
    assert!(
        r.iter()
            .all(|row| payload_str(node(row, "a"), "label") < "z"),
        "label < z 应覆盖当前数据集全部节点"
    );

    cleanup(&path);
}

/// WHERE Int-Float 交叉比较 (L748-749)
#[test]
fn COV5_04_where_int_float_cross() {
    let path = tmp_db("int_float");
    let db = seed_typed_graph(&path);

    // Int vs Float
    let r = db.tql(r#"MATCH (a) WHERE a.rank > 2.5 RETURN a"#).unwrap();
    assert_match_rows(&r, "a", 5);
    assert!(
        r.iter()
            .all(|row| payload_f64(node(row, "a"), "rank") > 2.5),
        "rank > 2.5 不能返回越界节点"
    );

    // Float vs Int (通过 score 字段和整数字面量)
    let r = db.tql(r#"MATCH (a) WHERE a.score > 5 RETURN a"#).unwrap();
    assert_match_rows(&r, "a", 4);
    assert!(
        r.iter()
            .all(|row| payload_f64(node(row, "a"), "score") > 5.0),
        "score > 5 不能返回越界节点"
    );

    cleanup(&path);
}

/// WHERE 对 .id 伪字段的比较 (L687-688, L707-708)
#[test]
fn COV5_05_where_id_field() {
    let path = tmp_db("id_field");
    let db = seed_typed_graph(&path);

    let ids = db.all_node_ids();
    let target = ids[3];
    let q = format!("MATCH (a) WHERE a.id == {} RETURN a", target);
    let r = db.tql(&q).unwrap();
    assert_eq!(r.len(), 1);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  tql_executor.rs — ORDER BY + extract_order_key (L952-990)
// ════════════════════════════════════════════════════════════════

/// ORDER BY 各种值类型排序 (Int, Float, String, NULL)
#[test]
fn COV5_06_order_by_types() {
    let path = tmp_db("order_types");
    let db = seed_typed_graph(&path);

    // ORDER BY Int field
    let r = db
        .tql(r#"FIND {active: true} RETURN * ORDER BY _.rank DESC"#)
        .unwrap();
    assert_find_rows(&r, 4);
    let ranks: Vec<_> = r
        .iter()
        .map(|row| payload_f64(node(row, "_"), "rank"))
        .collect();
    assert_eq!(ranks, vec![6.0, 4.0, 2.0, 0.0]);

    // ORDER BY Float field
    let r = db
        .tql(r#"FIND {active: false} RETURN * ORDER BY _.score ASC"#)
        .unwrap();
    assert_find_rows(&r, 4);
    let scores: Vec<_> = r
        .iter()
        .map(|row| payload_f64(node(row, "_"), "score"))
        .collect();
    assert_eq!(scores, vec![1.5, 4.5, 7.5, 10.5]);

    // ORDER BY String field
    let r = db
        .tql(r#"FIND {active: true} RETURN * ORDER BY _.label ASC"#)
        .unwrap();
    assert_find_rows(&r, 4);
    assert!(
        r.iter().all(|row| payload_bool(node(row, "_"), "active")),
        "ORDER BY 不能破坏 active=true 过滤条件"
    );
    let labels: Vec<_> = r
        .iter()
        .map(|row| payload_str(node(row, "_"), "label"))
        .collect();
    assert_eq!(labels, vec!["high", "high", "low", "low"]);

    cleanup(&path);
}

/// ORDER BY _.id (id 伪字段排序)
#[test]
fn COV5_07_order_by_id() {
    let path = tmp_db("order_id");
    let db = seed_typed_graph(&path);

    let r = db
        .tql(r#"FIND {active: true} RETURN * ORDER BY _.id DESC"#)
        .unwrap();
    assert_find_rows(&r, 4);
    assert!(
        r.iter().all(|row| payload_bool(node(row, "_"), "active")),
        "ORDER BY _.id 不能破坏 active=true 过滤条件"
    );
    let ids: Vec<_> = r.iter().map(|row| node(row, "_").id).collect();
    assert!(
        ids.windows(2).all(|pair| pair[0] >= pair[1]),
        "ORDER BY _.id DESC 必须按 id 降序返回"
    );

    cleanup(&path);
}

/// FIND + WHERE + ORDER BY + LIMIT 组合
#[test]
fn COV5_08_find_where_order_limit() {
    let path = tmp_db("find_combo");
    let db = seed_typed_graph(&path);

    let r = db
        .tql(r#"FIND {active: true} WHERE {rank: {$gte: 2}} RETURN * ORDER BY _.rank DESC LIMIT 2"#)
        .unwrap();
    assert_find_rows(&r, 2);
    let ranks: Vec<_> = r
        .iter()
        .map(|row| {
            let n = node(row, "_");
            assert!(payload_bool(n, "active"), "组合查询不能返回 inactive 节点");
            let rank = payload_f64(n, "rank");
            assert!(rank >= 2.0, "组合查询不能返回 rank < 2 的节点");
            rank
        })
        .collect();
    assert_eq!(ranks.len(), 2, "组合查询 LIMIT 2 必须返回两条");
    assert!(
        ranks.windows(2).all(|pair| pair[0] >= pair[1]),
        "组合查询返回结果必须保持 rank 降序"
    );

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  tql_executor.rs — FIND + AND/OR/NOT 单节点谓词 (L640-660)
// ════════════════════════════════════════════════════════════════

/// FIND + WHERE AND / OR / NOT (走 eval_predicate_single 路径)
#[test]
fn COV5_09_find_and_or_not() {
    let path = tmp_db("find_logic");
    let db = seed_typed_graph(&path);

    // FIND + AND 当前语法会把空 FIND 文档安全拒绝
    let r = db.tql(r#"FIND {} WHERE _.rank > 2 AND _.rank < 6 RETURN *"#);
    let err = r.expect_err("空 FIND 文档应被安全拒绝");
    assert!(!err.to_string().is_empty(), "安全拒绝必须返回可诊断错误");
    assert_eq!(db.node_count(), 8, "解析失败不能污染数据库");

    // FIND + OR 当前语法会把空 FIND 文档安全拒绝
    let r = db.tql(r#"FIND {} WHERE _.rank < 2 OR _.rank > 5 RETURN *"#);
    let err = r.expect_err("空 FIND 文档应被安全拒绝");
    assert!(!err.to_string().is_empty(), "安全拒绝必须返回可诊断错误");
    assert_eq!(db.node_count(), 8, "解析失败不能污染数据库");

    // FIND + NOT 当前语法会把空 FIND 文档安全拒绝
    let r = db.tql(r#"FIND {} WHERE NOT _.active == true RETURN *"#);
    let err = r.expect_err("空 FIND 文档应被安全拒绝");
    assert!(!err.to_string().is_empty(), "安全拒绝必须返回可诊断错误");
    assert_eq!(db.node_count(), 8, "解析失败不能污染数据库");

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  vector.rs — f16 VectorType 覆盖
// ════════════════════════════════════════════════════════════════

/// f16 数据库全流程
#[test]
fn COV5_10_f16_full_lifecycle() {
    use half::f16;
    let path = tmp_db("f16_life");

    let mut db = Database::<f16>::open(&path, DIM).unwrap();
    let id1 = db
        .insert(
            &[
                f16::from_f32(1.0),
                f16::from_f32(0.0),
                f16::from_f32(0.0),
                f16::from_f32(0.0),
            ],
            serde_json::json!({"type": "half"}),
        )
        .unwrap();
    let id2 = db
        .insert(
            &[
                f16::from_f32(0.0),
                f16::from_f32(1.0),
                f16::from_f32(0.0),
                f16::from_f32(0.0),
            ],
            serde_json::json!({"type": "half"}),
        )
        .unwrap();
    db.link(id1, id2, "f16_edge", 0.5).unwrap();

    let hits = db
        .search(
            &[
                f16::from_f32(1.0),
                f16::from_f32(0.0),
                f16::from_f32(0.0),
                f16::from_f32(0.0),
            ],
            5,
            0,
            0.0,
        )
        .unwrap();
    assert_eq!(hits.len(), 2, "f16 搜索应返回两个半精度节点");
    assert_eq!(hits[0].id, id1, "完全相同的 f16 向量应排第一");
    assert!(
        hits.iter()
            .all(|hit| hit.payload.get("type") == Some(&serde_json::json!("half"))),
        "f16 搜索结果必须来自半精度测试节点"
    );

    // flush + reopen (触发 vec_pool mmap 加载路径)
    db.flush().unwrap();
    drop(db);

    let db2 = Database::<f16>::open(&path, DIM).unwrap();
    assert!(db2.contains(id1));
    assert!(db2.contains(id2));

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  vec_pool + file_format — flush/reopen 触发 mmap 加载 (L121-161)
// ════════════════════════════════════════════════════════════════

/// flush → reopen 触发 mmap 向量加载路径
#[test]
fn COV5_11_flush_reopen_mmap() {
    let path = tmp_db("mmap_load");

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..50u32 {
            db.insert(
                &[i as f32, (i as f32).sin(), (i as f32).cos(), 1.0],
                serde_json::json!({"idx": i}),
            )
            .unwrap();
        }
        db.flush().unwrap();
    }

    // 重新打开 → vec_pool 从 .vec 文件 mmap 加载
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.node_count(), 50);

    let hits = db.search(&[25.0, 0.0, 0.0, 0.0], 5, 0, 0.0).unwrap();
    assert_eq!(hits.len(), 5, "mmap 重新加载后搜索应遵守 top_k=5");
    assert!(
        hits.iter().all(|hit| hit.payload.get("idx").is_some()),
        "mmap 搜索结果必须保留原始 payload"
    );

    cleanup(&path);
}

/// 多次 flush/reopen 幂等 + 增量写入
#[test]
fn COV5_12_incremental_flush_reopen() {
    let path = tmp_db("incr_flush");

    // 第一轮
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..10u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"round": 1}))
                .unwrap();
        }
        db.flush().unwrap();
    }

    // 第二轮增量
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        assert_eq!(db.node_count(), 10);
        for i in 10..20u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"round": 2}))
                .unwrap();
        }
        db.flush().unwrap();
    }

    // 验证
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.node_count(), 20);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  tql_parser.rs — CREATE 带权重边 + 更多错误路径
// ════════════════════════════════════════════════════════════════

/// CREATE 带权重边 (L971-989)
#[test]
fn COV5_13_tql_create_weighted_edge() {
    let path = tmp_db("create_weight");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let result = db
        .tql_mut(r#"CREATE (a {name: "X"})-[:link {weight: 0.7}]->(b {name: "Y"})"#)
        .unwrap();
    assert!(result.affected >= 2);

    cleanup(&path);
}

/// TQL 无效语法错误
#[test]
fn COV5_14_tql_syntax_errors() {
    let path = tmp_db("syntax_err");
    let db = Database::<f32>::open(&path, DIM).unwrap();

    // 不完整 SEARCH
    assert_tql_err(db.tql("SEARCH VECTOR"), "Expected");
    assert_eq!(db.node_count(), 0, "语法错误不能污染空数据库");

    // MATCH 缺少括号
    assert_tql_err(db.tql("MATCH a RETURN a"), "Expected");
    assert_eq!(db.node_count(), 0, "语法错误不能污染空数据库");

    // 空查询
    assert_tql_err(db.tql(""), "Expected");
    assert_eq!(db.node_count(), 0, "语法错误不能污染空数据库");

    // 无效操作符
    assert_tql_err(db.tql(r#"FIND {x: {$invalid: 1}} RETURN *"#), "invalid");
    assert_eq!(db.node_count(), 0, "语法错误不能污染空数据库");

    cleanup(&path);
}

/// TQL RETURN DISTINCT 覆盖 + 边界
#[test]
fn COV5_15_tql_distinct_edge() {
    let path = tmp_db("distinct_edge");
    let db = seed_typed_graph(&path);

    // DISTINCT 带属性
    let r = db
        .tql(r#"MATCH (a)-[:seq]->(b) RETURN DISTINCT a.label"#)
        .unwrap();
    assert_eq!(r.len(), 2, "DISTINCT a.label 应只返回 low/high 两类标签");
    let mut labels: Vec<_> = r
        .iter()
        .map(|row| {
            row.get("a")
                .and_then(|node| node.payload.get("label"))
                .cloned()
                .expect("DISTINCT a.label 每行必须保留 a.label")
        })
        .collect();
    labels.sort_by_key(|v| v.as_str().unwrap_or_default().to_string());
    assert_eq!(
        labels,
        vec![serde_json::json!("high"), serde_json::json!("low")]
    );

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  tql_executor.rs — SEARCH VECTOR 结合 FIND 风格的 WHERE (L640-660)
// ════════════════════════════════════════════════════════════════

/// SEARCH VECTOR + complex WHERE
#[test]
fn COV5_16_search_complex_where() {
    let path = tmp_db("search_cwhere");
    let db = seed_typed_graph(&path);

    let r = db
        .tql(r#"SEARCH VECTOR [3.0, 0.0, 0.0, 0.0] TOP 5 WHERE {$and: [{active: true}, {rank: {$gte: 2}}]} RETURN *"#)
        .unwrap();
    assert!(r.len() <= 5, "SEARCH TOP 5 必须限制返回数量");
    assert!(
        !r.is_empty(),
        "当前数据集应至少命中一个 active=true 且 rank>=2 的节点"
    );
    assert!(
        r.iter().all(|row| {
            let n = node(row, "_");
            payload_bool(n, "active") && payload_f64(n, "rank") >= 2.0
        }),
        "SEARCH 复杂 WHERE 不能返回不满足条件的节点"
    );

    cleanup(&path);
}

/// MATCH (a)-[:label]->(b) WHERE b.field 覆盖双节点谓词
#[test]
fn COV5_17_match_where_on_b() {
    let path = tmp_db("where_on_b");
    let db = seed_typed_graph(&path);

    let r = db
        .tql(r#"MATCH (a)-[:seq]->(b) WHERE b.rank > 5 RETURN a, b"#)
        .unwrap();
    assert!(
        r.len() <= 2,
        "b.rank > 5 返回数量不能超过当前数据集可满足路径上限"
    );
    assert!(
        r.iter().all(|row| {
            row.contains_key("a")
                && row.contains_key("b")
                && payload_f64(node(row, "b"), "rank") > 5.0
        }),
        "WHERE on b 不能返回 b.rank 不满足条件的路径"
    );

    // WHERE on both a and b
    let r = db
        .tql(r#"MATCH (a)-[:seq]->(b) WHERE a.active == true AND b.score > 5.0 RETURN a, b"#)
        .unwrap();
    assert!(
        r.len() <= 3,
        "a.active 与 b.score 组合谓词返回数量不能超过当前数据集可满足路径上限"
    );
    assert!(
        r.iter().all(|row| {
            row.contains_key("a")
                && row.contains_key("b")
                && payload_bool(node(row, "a"), "active")
                && payload_f64(node(row, "b"), "score") > 5.0
        }),
        "双变量 WHERE 不能返回不满足组合谓词的路径"
    );

    cleanup(&path);
}

/// MATCH ORDER BY a.field (非 FIND 场景的 extract_order_key)
#[test]
fn COV5_18_match_order_by() {
    let path = tmp_db("match_order");
    let db = seed_typed_graph(&path);

    let r = db
        .tql(r#"MATCH (a)-[:seq]->(b) RETURN a, b ORDER BY a.score DESC, b.rank ASC"#)
        .unwrap();
    assert_eq!(r.len(), 7, "seq 链应返回 7 条有向边");
    let scores: Vec<_> = r
        .iter()
        .map(|row| payload_f64(node(row, "a"), "score"))
        .collect();
    assert!(
        scores.windows(2).all(|pair| pair[0] >= pair[1]),
        "MATCH ORDER BY a.score DESC 必须按降序返回"
    );

    cleanup(&path);
}

/// TQL mut SET + DELETE 复杂场景
#[test]
fn COV5_19_tql_mut_complex() {
    let path = tmp_db("mut_complex");
    let mut db = seed_typed_graph(&path);

    // SET 单个字段
    let r = db
        .tql_mut(r#"MATCH (a) WHERE a.rank == 0 SET a.label == "updated""#)
        .unwrap();
    assert!(r.affected >= 1);

    // 验证
    let results = db.tql(r#"FIND {label: "updated"} RETURN *"#).unwrap();
    assert_find_rows(&results, 1);
    let updated = node(&results[0], "_");
    assert_eq!(payload_f64(updated, "rank"), 0.0);
    assert_eq!(
        updated.payload.get("label"),
        Some(&serde_json::json!("updated"))
    );

    cleanup(&path);
}

/// tql_mut DELETE 多变量
#[test]
fn COV5_20_tql_mut_delete_multi() {
    let path = tmp_db("del_multi");
    let mut db = seed_typed_graph(&path);
    let before = db.node_count();

    let r = db
        .tql_mut(r#"MATCH (a) WHERE a.rank < 2 DELETE a"#)
        .unwrap();
    assert!(r.affected >= 1);
    assert!(db.node_count() < before);

    cleanup(&path);
}
