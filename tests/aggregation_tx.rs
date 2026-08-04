#![allow(non_snake_case)]
//! TQL 聚合函数、文档过滤操作符与事务系统集成测试
//!
//! 验证范围：
//! - `query/tql_executor.rs`: 聚合函数 (count/sum/avg/min/max/collect)、分组聚合、RETURN AS 别名
//! - `query/tql_parser.rs`: $in/$nin/$exists/$all/$type/$size/$and/$or 操作符、MATCHES 谓词、NOT 谓词
//! - `database/transaction.rs`: begin_tx 全操作组合 (insert/link/unlink/update_payload/update_vector/delete)、
//!   预检失败 (不存在节点、维度不匹配、重复 ID)
//! - `cognitive.rs`: NMF 语义分析 (聚焦/均匀/新颖/零向量)
//! - `database/mod.rs`: WAL 回放幂等性、多次 flush 幂等性

use triviumdb::database::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    let path = dir
        .join(format!("cov4_{}", name))
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

// ════════════════════════════════════════════════════════════════
//  tql_executor.rs — 聚合函数 (lines 1114-1267)
// ════════════════════════════════════════════════════════════════

fn seed_scored_graph(path: &str) -> Database<f32> {
    let mut db = Database::<f32>::open(path, DIM).unwrap();
    for i in 0..10u32 {
        db.insert(
            &[i as f32, 0.0, 0.0, 0.0],
            serde_json::json!({
                "type": "item",
                "name": format!("item_{}", i),
                "score": i as f64 * 10.0,
                "tags": ["a", "b", "c"],
                "group": if i < 5 { "alpha" } else { "beta" }
            }),
        )
        .unwrap();
    }

    let ids = db.all_node_ids();
    for i in 0..ids.len() - 1 {
        db.link(ids[i], ids[i + 1], "next", 1.0).unwrap();
    }
    db
}

/// count(a) 聚合
#[test]
fn COV4_01_tql_agg_count() {
    let path = tmp_db("agg_count");
    let db = seed_scored_graph(&path);

    let results = db.tql(r#"MATCH (a)-[:next]->(b) RETURN count(a)"#).unwrap();
    assert!(!results.is_empty(), "count 聚合应返回结果");

    cleanup(&path);
}

/// sum(a.score) 聚合
#[test]
fn COV4_02_tql_agg_sum() {
    let path = tmp_db("agg_sum");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"MATCH (a)-[:next]->(b) RETURN sum(a.score)"#)
        .unwrap();
    assert!(!results.is_empty());

    cleanup(&path);
}

/// avg(a.score) 聚合
#[test]
fn COV4_03_tql_agg_avg() {
    let path = tmp_db("agg_avg");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"MATCH (a)-[:next]->(b) RETURN avg(a.score)"#)
        .unwrap();
    assert!(!results.is_empty());

    cleanup(&path);
}

/// min(a.score) 聚合
#[test]
fn COV4_04_tql_agg_min() {
    let path = tmp_db("agg_min");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"MATCH (a)-[:next]->(b) RETURN min(a.score)"#)
        .unwrap();
    assert!(!results.is_empty());

    cleanup(&path);
}

/// max(a.score) 聚合
#[test]
fn COV4_05_tql_agg_max() {
    let path = tmp_db("agg_max");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"MATCH (a)-[:next]->(b) RETURN max(a.score)"#)
        .unwrap();
    assert!(!results.is_empty());

    cleanup(&path);
}

/// collect(a) 聚合
#[test]
fn COV4_06_tql_agg_collect() {
    let path = tmp_db("agg_collect");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"MATCH (a)-[:next]->(b) RETURN collect(a)"#)
        .unwrap();
    assert!(!results.is_empty());

    cleanup(&path);
}

/// 混合聚合: 分组列 + 聚合列
#[test]
fn COV4_07_tql_agg_with_group() {
    let path = tmp_db("agg_group");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"FIND {type: "item"} RETURN _.group, count(_)"#)
        .unwrap();
    assert!(!results.is_empty());

    cleanup(&path);
}

/// RETURN 带 alias (AS)
#[test]
fn COV4_08_tql_return_alias() {
    let path = tmp_db("agg_alias");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"MATCH (a)-[:next]->(b) RETURN count(a) AS total, max(a.score) AS top_score"#)
        .unwrap();
    assert!(!results.is_empty());

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  tql_parser.rs — 更多 Filter 操作符
// ════════════════════════════════════════════════════════════════

/// $in 操作符
#[test]
fn COV4_09_filter_in() {
    let path = tmp_db("filter_in");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"FIND {name: {$in: ["item_1", "item_3", "item_5"]}} RETURN *"#)
        .unwrap();
    assert_eq!(results.len(), 3, "$in 应匹配 3 个");

    cleanup(&path);
}

/// $nin 操作符
#[test]
fn COV4_10_filter_nin() {
    let path = tmp_db("filter_nin");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"FIND {name: {$nin: ["item_1", "item_3"]}} RETURN *"#)
        .unwrap();
    assert_eq!(results.len(), 8, "$nin 应排除 2 个");

    cleanup(&path);
}

/// $exists 操作符
#[test]
fn COV4_11_filter_exists() {
    let path = tmp_db("filter_exists");
    let db = seed_scored_graph(&path);

    let results = db.tql(r#"FIND {score: {$exists: true}} RETURN *"#).unwrap();
    assert_eq!(results.len(), 10);

    let results = db
        .tql(r#"FIND {nonexistent: {$exists: false}} RETURN *"#)
        .unwrap();
    assert_eq!(results.len(), 10);

    cleanup(&path);
}

/// $type 操作符
#[test]
fn COV4_12_filter_type() {
    let path = tmp_db("filter_type");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"FIND {name: {$type: "string"}} RETURN *"#)
        .unwrap();
    assert_eq!(results.len(), 10);

    cleanup(&path);
}

/// $all 操作符
#[test]
fn COV4_13_filter_all() {
    let path = tmp_db("filter_all");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"FIND {tags: {$all: ["a", "b"]}} RETURN *"#)
        .unwrap();
    assert_eq!(results.len(), 10);

    cleanup(&path);
}

/// $size 操作符
#[test]
fn COV4_14_filter_size() {
    let path = tmp_db("filter_size");
    let db = seed_scored_graph(&path);

    let results = db.tql(r#"FIND {tags: {$size: 3}} RETURN *"#).unwrap();
    assert_eq!(results.len(), 10);

    cleanup(&path);
}

/// $and / $or 组合过滤
#[test]
fn COV4_15_filter_and_or() {
    let path = tmp_db("filter_andor");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"FIND {$and: [{group: "alpha"}, {score: {$gte: 20}}]} RETURN *"#)
        .unwrap();
    assert_eq!(results.len(), 3);

    let results = db
        .tql(r#"FIND {$or: [{group: "alpha"}, {score: {$gte: 80}}]} RETURN *"#)
        .unwrap();
    assert_eq!(results.len(), 7);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  transaction.rs — 事务内多操作覆盖 (begin_tx API)
// ════════════════════════════════════════════════════════════════

/// 事务: 全操作组合 Link + UpdatePayload + UpdateVector + Unlink + Delete
#[test]
fn COV4_16_tx_full_ops() {
    let path = tmp_db("tx_full");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 基础节点
    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "A"}));
        tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "B"}));
        tx.insert(&[0.0, 0.0, 1.0, 0.0], serde_json::json!({"name": "C"}));
        tx.commit().unwrap()
    };

    // 事务内: Link + UpdatePayload + UpdateVector
    {
        let mut tx = db.begin_tx();
        tx.link(ids[0], ids[1], "knows", 0.8);
        tx.link(ids[1], ids[2], "knows", 0.7);
        tx.update_payload(
            ids[0],
            serde_json::json!({"name": "A_updated", "flag": true}),
        );
        tx.update_vector(ids[2], &[0.0, 0.0, 0.0, 1.0]);
        tx.commit().unwrap();
    }

    assert_eq!(db.get_payload(ids[0]).unwrap()["name"], "A_updated");
    assert_eq!(db.get_edges(ids[0]).len(), 1);

    // 事务内: Unlink + Delete
    {
        let mut tx = db.begin_tx();
        tx.unlink(ids[0], ids[1]);
        tx.delete(ids[2]);
        tx.commit().unwrap();
    }

    assert!(db.get_edges(ids[0]).is_empty());
    assert!(!db.contains(ids[2]));

    cleanup(&path);
}

/// 事务: Link 不存在的节点 - 预检失败
#[test]
fn COV4_17_tx_link_nonexistent() {
    let path = tmp_db("tx_link_err");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let result = {
        let mut tx = db.begin_tx();
        tx.link(999, 998, "x", 1.0);
        tx.commit()
    };
    assert!(result.is_err(), "Link 不存在节点应失败");

    cleanup(&path);
}

/// 事务: UpdatePayload/UpdateVector/Delete/Unlink 不存在节点
#[test]
fn COV4_18_tx_ops_nonexistent() {
    let path = tmp_db("tx_ops_err");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // UpdatePayload 不存在
    let r = {
        let mut tx = db.begin_tx();
        tx.update_payload(999, serde_json::json!({"x": 1}));
        tx.commit()
    };
    assert!(r.is_err());

    // UpdateVector 不存在
    let r = {
        let mut tx = db.begin_tx();
        tx.update_vector(999, &[1.0, 0.0, 0.0, 0.0]);
        tx.commit()
    };
    assert!(r.is_err());

    // Delete 不存在
    let r = {
        let mut tx = db.begin_tx();
        tx.delete(999);
        tx.commit()
    };
    assert!(r.is_err());

    // Unlink 不存在
    let r = {
        let mut tx = db.begin_tx();
        tx.unlink(999, 998);
        tx.commit()
    };
    assert!(r.is_err());

    cleanup(&path);
}

/// 事务: UpdateVector 维度不匹配
#[test]
fn COV4_19_tx_dim_mismatch() {
    let path = tmp_db("tx_dim");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
        tx.commit().unwrap()
    };

    let result = {
        let mut tx = db.begin_tx();
        tx.update_vector(ids[0], &[1.0, 0.0]); // 维度不匹配
        tx.commit()
    };
    assert!(result.is_err(), "维度不匹配应失败");

    cleanup(&path);
}

/// 事务: InsertWithId 重复 ID
#[test]
fn COV4_20_tx_duplicate_id() {
    let path = tmp_db("tx_dup_id");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
        tx.commit().unwrap()
    };

    let result = {
        let mut tx = db.begin_tx();
        tx.insert_with_id(ids[0], &[0.0, 1.0, 0.0, 0.0], serde_json::json!({}));
        tx.commit()
    };
    assert!(result.is_err(), "重复 ID 应失败");

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  WAL 回放 + cognitive NMF
// ════════════════════════════════════════════════════════════════

/// WAL 回放幂等性: 含 link/update/unlink/delete 的完整回放
#[test]
fn COV4_21_wal_replay_full() {
    let path = tmp_db("wal_full");
    let id1;
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        id1 = db
            .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"v": 1}))
            .unwrap();
        let id2 = db
            .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"v": 2}))
            .unwrap();
        db.link(id1, id2, "rel", 0.5).unwrap();
        db.update_payload(id1, serde_json::json!({"v": 10}))
            .unwrap();
        db.update_vector(id2, &[0.0, 0.0, 1.0, 0.0]).unwrap();
        db.unlink(id1, id2).unwrap();
        db.delete(id2).unwrap();
        // 不 flush — WAL 仍在
    }

    // 重新打开触发 WAL 回放
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.node_count(), 1);
    assert_eq!(db.get_payload(id1).unwrap()["v"], 10);

    cleanup(&path);
}

/// NMF analyze_query 覆盖
#[test]
fn COV4_22_nmf_analyze_query() {
    use triviumdb::cognitive::nmf_analyze_query;

    let k = 3;
    let d = 4;
    let h_flat = vec![1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];

    // 聚焦查询
    let (depth, coverage, novelty, topics) =
        nmf_analyze_query(&[1.0, 0.0, 0.0, 0.0], &h_flat, k, d);
    assert!(depth > 0.0, "聚焦应有正深度: {}", depth);
    assert!(coverage >= 1);
    eprintln!(
        "  focused: d={:.2}, c={}, n={:.2}, t={:?}",
        depth, coverage, novelty, topics
    );

    // 均匀查询
    let (d2, c2, _, _) = nmf_analyze_query(&[1.0, 1.0, 1.0, 0.0], &h_flat, k, d);
    assert!(d2 < depth);
    assert!(c2 >= coverage);

    // 新颖查询
    let (_, _, n3, _) = nmf_analyze_query(&[0.0, 0.0, 0.0, 1.0], &h_flat, k, d);
    assert!(n3 > 0.0, "新颖查询应有正新颖度: {}", n3);

    // 零向量
    let _ = nmf_analyze_query(&[0.0, 0.0, 0.0, 0.0], &h_flat, k, d);
}

// ════════════════════════════════════════════════════════════════
//  tql_parser.rs — 更多语法分支
// ════════════════════════════════════════════════════════════════

/// 解析错误路径
#[test]
fn COV4_23_parser_errors() {
    let path = tmp_db("parse_err");
    let db = Database::<f32>::open(&path, DIM).unwrap();

    assert!(db.tql("FIND").is_err());
    assert!(db.tql("MATCH").is_err());
    assert!(db.tql(r#"FIND {type: "x"} RETURN"#).is_err());

    cleanup(&path);
}

/// TQL MATCHES 谓词
#[test]
fn COV4_24_tql_matches_predicate() {
    let path = tmp_db("tql_matches");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"MATCH (a)-[:next]->(b) WHERE a MATCHES {group: "alpha"} RETURN a, b"#)
        .unwrap();
    eprintln!("  MATCHES: {} 条", results.len());

    cleanup(&path);
}

/// NOT 谓词
#[test]
fn COV4_25_tql_not_predicate() {
    let path = tmp_db("tql_not");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"MATCH (a) WHERE NOT a.score > 50 RETURN a"#)
        .unwrap();
    eprintln!("  NOT: {} 条", results.len());

    cleanup(&path);
}

/// 多列 ORDER BY
#[test]
fn COV4_26_tql_multi_order() {
    let path = tmp_db("tql_multi_ord");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"FIND {type: "item"} RETURN * ORDER BY _.group ASC, _.score DESC"#)
        .unwrap();
    assert!(!results.is_empty());

    cleanup(&path);
}

#[test]
fn COV4_26A_tql_order_before_offset_limit() {
    let path = tmp_db("tql_order_page");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"FIND {type: "item"} RETURN * ORDER BY _.score DESC LIMIT 3 OFFSET 2"#)
        .unwrap();
    let scores: Vec<i64> = results
        .iter()
        .map(|row| row["_"].payload["score"].as_f64().unwrap() as i64)
        .collect();
    assert_eq!(scores, vec![70, 60, 50], "必须先全量排序，再执行分页");

    cleanup(&path);
}

#[test]
fn COV4_26B_tql_aggregate_before_order_and_limit() {
    let path = tmp_db("tql_agg_page");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"FIND {type: "item"} RETURN _.group, count(_) AS total ORDER BY _.group DESC LIMIT 1"#)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["_"].payload["group"], "beta");
    assert_eq!(results[0]["total"].payload["total"], 5);

    cleanup(&path);
}

#[test]
fn COV4_26C_tql_create_continuous_edges_atomically() {
    let path = tmp_db("tql_create_chain");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let result = db
        .tql_mut(r#"CREATE (a {name: "a"})-[:first]->(b {name: "b"})-[:second]->(c {name: "c"})"#)
        .unwrap();
    assert_eq!(result.created_ids.len(), 3);
    assert_eq!(result.affected, 5);
    assert_eq!(
        db.tql(r#"MATCH (a)-[:first]->(b)-[:second]->(c) RETURN a, b, c"#)
            .unwrap()
            .len(),
        1
    );

    cleanup(&path);
}

#[test]
fn COV4_26D_tql_create_failure_is_atomic() {
    let path = tmp_db("tql_create_atomic");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let oversized = "x".repeat(8 * 1024 * 1024 + 1);
    let query = format!("CREATE (a {{name: \"kept_out\"}})-[:next]->(b {{data: \"{oversized}\"}})");

    assert!(db.tql_mut(&query).is_err());
    assert_eq!(db.node_count(), 0, "任一操作预检失败时整条 CREATE 不得落库");

    cleanup(&path);
}

/// RETURN a.field (属性投影)
#[test]
fn COV4_27_tql_return_property() {
    let path = tmp_db("tql_ret_prop");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"MATCH (a)-[:next]->(b) RETURN a.name, b.score"#)
        .unwrap();
    assert!(!results.is_empty());

    cleanup(&path);
}

/// EXPLAIN MATCH / SEARCH
#[test]
fn COV4_28_tql_explain_variants() {
    let path = tmp_db("explain_var");
    let db = seed_scored_graph(&path);

    let r1 = db
        .tql(r#"EXPLAIN MATCH (a)-[:next]->(b) RETURN a, b"#)
        .unwrap();
    assert!(!r1.is_empty());

    let r2 = db
        .tql("EXPLAIN SEARCH VECTOR [1.0, 0.0, 0.0, 0.0] TOP 3 RETURN *")
        .unwrap();
    assert!(!r2.is_empty());

    cleanup(&path);
}

/// SEARCH + EXPAND + WHERE 组合
#[test]
fn COV4_29_search_expand_where() {
    let path = tmp_db("search_combo");
    let db = seed_scored_graph(&path);

    let results = db
        .tql(r#"SEARCH VECTOR [5.0, 0.0, 0.0, 0.0] TOP 3 EXPAND [:next*1..2] WHERE {score: {$gte: 30}} RETURN *"#)
        .unwrap();
    eprintln!("  SEARCH+EXPAND+WHERE: {} 条", results.len());

    cleanup(&path);
}

/// flush 后 flush（幂等性）
#[test]
fn COV4_30_double_flush() {
    let path = tmp_db("dbl_flush");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    db.flush().unwrap();
    db.flush().unwrap();
    assert_eq!(db.node_count(), 1);

    cleanup(&path);
}
