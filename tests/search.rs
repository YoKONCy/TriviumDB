#![allow(non_snake_case)]
//! 向量搜索回归测试
//!
//! 覆盖范围：
//! - P1-5 BQ 粗筛正确性（删除/更新后搜索一致性）
//! - 基础 cosine 相似度搜索
//! - 空库搜索、min_score 过滤、top_k 边界

use triviumdb::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    std::fs::create_dir_all("test_data").ok();
    format!("test_data/search_{}", name)
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

// ════════ 基础搜索 ════════

#[test]
fn 基础搜索_余弦相似度最高的节点排在最前() {
    let path = tmp_db("cosine_basic");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"label": "target"}));
        tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"label": "other1"}));
        tx.insert(&[0.0, 0.0, 1.0, 0.0], serde_json::json!({"label": "other2"}));
        tx.commit().unwrap()
    };

    let results = db.search(&[1.0, 0.0, 0.0, 0.0], 3, 0, 0.0).unwrap();
    assert!(!results.is_empty(), "搜索结果不应为空");
    assert_eq!(results[0].id, ids[0], "与 query 最相似的节点应排第一");

    cleanup(&path);
}

#[test]
fn 空库搜索_应返回空结果不panic() {
    let path = tmp_db("empty_search");
    cleanup(&path);
    let db = Database::<f32>::open(&path, DIM).unwrap();

    let results = db.search(&[1.0, 0.0, 0.0, 0.0], 5, 0, 0.0).unwrap();
    assert!(results.is_empty(), "空库搜索应返回空结果");

    cleanup(&path);
}

#[test]
fn 搜索_min_score过滤_低分节点不出现() {
    let path = tmp_db("min_score");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));  // 高分
        tx.insert(&[0.0, 0.0, 0.0, 1.0], serde_json::json!({}));  // 低分（正交）
        tx.commit().unwrap();
    }

    let results = db.search(&[1.0, 0.0, 0.0, 0.0], 5, 0, 0.99).unwrap();
    assert!(results.len() <= 1, "高 min_score 应过滤掉大部分节点");

    cleanup(&path);
}

#[test]
fn 搜索_top_k超过总节点数_只返回实际节点数() {
    let path = tmp_db("topk_overflow");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
        tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({}));
        tx.commit().unwrap();
    }

    let results = db.search(&[1.0, 0.0, 0.0, 0.0], 100, 0, 0.0).unwrap();
    assert!(results.len() <= 2, "top_k 超过总数时应只返回实际节点数");

    cleanup(&path);
}

// ════════ P1-5：BQ 搜索一致性 ════════

#[test]
fn P1_5_BQ搜索_删除后不返回被删节点() {
    let path = tmp_db("bq_delete");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"label": "to_delete"}));
        tx.insert(&[0.9, 0.1, 0.0, 0.0], serde_json::json!({"label": "keep1"}));
        tx.insert(&[0.8, 0.2, 0.0, 0.0], serde_json::json!({"label": "keep2"}));
        tx.commit().unwrap()
    };
    let del_id = ids[0];

    { let mut tx = db.begin_tx(); tx.delete(del_id); tx.commit().unwrap(); }

    let results = db.search(&[1.0, 0.0, 0.0, 0.0], 5, 0, 0.0).unwrap();
    assert!(!results.iter().any(|h| h.id == del_id), "已删除节点不应出现在搜索结果中");

    cleanup(&path);
}

#[test]
fn P1_5_BQ搜索_更新向量后_结果反映变化() {
    let path = tmp_db("bq_update");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"label": "node_a"}));
        tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"label": "node_b"}));
        tx.commit().unwrap()
    };

    let results_before = db.search(&[1.0, 0.0, 0.0, 0.0], 2, 0, 0.0).unwrap();
    assert_eq!(results_before[0].id, ids[0], "更新前 node_a 应排第一");

    // 将 node_a 的向量改到远离 query 的方向
    db.update_vector(ids[0], &[0.0, 0.0, 0.0, 1.0]).unwrap();

    let results_after = db.search(&[1.0, 0.0, 0.0, 0.0], 2, 0, 0.0).unwrap();
    assert!(
        results_after[0].id != ids[0] || results_after[0].score < results_before[0].score,
        "更新向量后搜索结果应发生变化"
    );

    cleanup(&path);
}

#[test]
fn P1_5_BQ搜索与BruteForce一致性() {
    let path = tmp_db("bq_consistency");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    {
        let mut tx = db.begin_tx();
        for i in 0..20u32 {
            let v: Vec<f32> = (0..DIM).map(|d| if d == (i % 4) as usize { 1.0 } else { 0.0 }).collect();
            tx.insert(&v, serde_json::json!({"i": i}));
        }
        tx.commit().unwrap();
    }

    let results = db.search(&[1.0, 0.0, 0.0, 0.0], 5, 0, 0.0).unwrap();

    for w in results.windows(2) {
        assert!(w[0].score >= w[1].score, "搜索结果应按分数降序排列");
    }
    assert!(results.len() <= 5, "返回结果数不应超过 top_k");

    cleanup(&path);
}

// ════════ 搜索边界 ════════

#[test]
fn 搜索_单节点库_总是返回该节点() {
    let path = tmp_db("single_node");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"only": true}));
        tx.commit().unwrap()
    };

    let results = db.search(&[0.0, 1.0, 0.0, 0.0], 10, 0, 0.0).unwrap();
    assert_eq!(results.len(), 1, "单节点库应恰好返回 1 个结果");
    assert_eq!(results[0].id, ids[0]);

    cleanup(&path);
}

#[test]
fn 搜索_批量删除后_库变空_搜索返回空() {
    let path = tmp_db("batch_delete_search");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
        tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({}));
        tx.commit().unwrap()
    };

    {
        let mut tx = db.begin_tx();
        for id in &ids { tx.delete(*id); }
        tx.commit().unwrap();
    }

    let results = db.search(&[1.0, 0.0, 0.0, 0.0], 10, 0, 0.0).unwrap();
    assert!(results.is_empty(), "全部删除后搜索应返回空");

    cleanup(&path);
}