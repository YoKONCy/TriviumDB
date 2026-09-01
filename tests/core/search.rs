#![allow(non_snake_case)]
//! 向量搜索回归测试
//!
//! 覆盖范围：
//! - P1-5 BQ 粗筛正确性（删除/更新后搜索一致性）
//! - 基础 cosine 相似度搜索
//! - 空库搜索、min_score 过滤、top_k 边界

use triviumdb::{BatchSearchConfig, Database, SearchConfig, TriviumError};

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("search_{}", name))
        .to_string_lossy()
        .to_string()
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
        tx.insert(
            &[1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"label": "target"}),
        );
        tx.insert(
            &[0.0, 1.0, 0.0, 0.0],
            serde_json::json!({"label": "other1"}),
        );
        tx.insert(
            &[0.0, 0.0, 1.0, 0.0],
            serde_json::json!({"label": "other2"}),
        );
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
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({})); // 高分
        tx.insert(&[0.0, 0.0, 0.0, 1.0], serde_json::json!({})); // 低分（正交）
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

#[test]
fn 精确搜索_返回全库真实TopK和完整Payload() {
    let path = tmp_db("exact_topk");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"rank": 1}));
        tx.insert(&[0.8, 0.6, 0.0, 0.0], serde_json::json!({"rank": 2}));
        tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"rank": 3}));
        tx.commit().unwrap()
    };

    let hits = db.search_exact(&[1.0, 0.0, 0.0, 0.0], 2).unwrap();

    assert_eq!(hits.iter().map(|hit| hit.id).collect::<Vec<_>>(), ids[..2]);
    assert_eq!(hits[0].payload["rank"], 1);
    assert_eq!(hits[1].payload["rank"], 2);
    assert!(hits[0].score >= hits[1].score);
    cleanup(&path);
}

#[test]
fn 精确搜索_同分按NodeId稳定排序() {
    let path = tmp_db("exact_tie");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"i": 0}));
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"i": 1}));
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"i": 2}));
        tx.commit().unwrap()
    };

    let hits = db.search_exact(&[1.0, 0.0, 0.0, 0.0], 2).unwrap();
    assert_eq!(hits.iter().map(|hit| hit.id).collect::<Vec<_>>(), ids[..2]);
    cleanup(&path);
}

#[test]
fn 精确搜索_跳过删除节点并反映向量更新() {
    let path = tmp_db("exact_mutation");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
        tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({}));
        tx.insert(&[0.5, 0.5, 0.0, 0.0], serde_json::json!({}));
        tx.commit().unwrap()
    };
    db.delete(ids[0]).unwrap();
    db.update_vector(ids[1], &[1.0, 0.0, 0.0, 0.0]).unwrap();

    let hits = db.search_exact(&[1.0, 0.0, 0.0, 0.0], 10).unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].id, ids[1]);
    assert!(!hits.iter().any(|hit| hit.id == ids[0]));
    cleanup(&path);
}

#[test]
fn 精确搜索_参数与空库边界() {
    let path = tmp_db("exact_boundaries");
    cleanup(&path);
    let db = Database::<f32>::open(&path, DIM).unwrap();

    assert!(
        db.search_exact(&[1.0, 0.0, 0.0, 0.0], 3)
            .unwrap()
            .is_empty()
    );
    assert!(db.search_exact(&[1.0, 0.0], 3).is_err());
    assert!(db.search_exact(&[1.0, 0.0, 0.0, 0.0], 0).is_err());
    assert!(
        db.search_exact(&[1.0, 0.0, 0.0, 0.0], usize::MAX)
            .unwrap()
            .is_empty()
    );
    cleanup(&path);
}

#[test]
fn 精确搜索_与朴素全量排序结果一致() {
    let path = tmp_db("exact_naive_comparison");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let query = [0.7_f32, -0.2, 0.5, 0.4];
    let query_norm = query.iter().map(|value| value * value).sum::<f32>().sqrt();
    let mut expected = Vec::new();

    for index in 0..257_u64 {
        let vector = [
            ((index * 17 + 3) % 101) as f32 - 50.0,
            ((index * 29 + 7) % 103) as f32 - 51.0,
            ((index * 43 + 11) % 107) as f32 - 53.0,
            ((index * 61 + 13) % 109) as f32 - 54.0,
        ];
        let id = db
            .insert(&vector, serde_json::json!({"index": index}))
            .unwrap();
        let dot = query
            .iter()
            .zip(vector.iter())
            .map(|(left, right)| left * right)
            .sum::<f32>();
        let vector_norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        expected.push((id, dot / (query_norm * vector_norm)));
    }

    expected.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    let hits = db.search_exact(&query, 37).unwrap();

    assert_eq!(hits.len(), 37);
    for (hit, (expected_id, expected_score)) in hits.iter().zip(expected.iter()) {
        assert_eq!(hit.id, *expected_id);
        assert!((hit.score - expected_score).abs() < 1e-6);
    }
    cleanup(&path);
}

#[test]
fn 批量搜索_结果顺序与逐条搜索严格一致() {
    let path = tmp_db("batch_order");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for index in 0..64_u64 {
        let vector = [
            ((index * 7 + 1) % 31) as f32,
            ((index * 11 + 2) % 37) as f32,
            ((index * 13 + 3) % 41) as f32,
            ((index * 17 + 4) % 43) as f32,
        ];
        db.insert(&vector, serde_json::json!({"index": index}))
            .unwrap();
    }
    let queries = vec![
        vec![1.0, 0.0, 0.0, 0.0],
        vec![0.0, 1.0, 0.0, 0.0],
        vec![0.0, 0.0, 1.0, 0.0],
        vec![0.0, 0.0, 0.0, 1.0],
        vec![1.0, 1.0, 1.0, 1.0],
    ];
    let config = SearchConfig {
        top_k: 7,
        expand_depth: 0,
        min_score: -1.0,
        ..Default::default()
    };
    let expected = queries
        .iter()
        .map(|query| db.search_hybrid(None, Some(query), &config).unwrap())
        .collect::<Vec<_>>();

    let actual = db
        .search_batch(&queries, &config, &BatchSearchConfig { parallelism: 3 })
        .unwrap();

    assert_eq!(actual.len(), queries.len());
    for (actual_hits, expected_hits) in actual.iter().zip(expected.iter()) {
        assert_eq!(actual_hits.len(), expected_hits.len());
        for (actual_hit, expected_hit) in actual_hits.iter().zip(expected_hits.iter()) {
            assert_eq!(actual_hit.id, expected_hit.id);
            assert_eq!(actual_hit.score.to_bits(), expected_hit.score.to_bits());
            assert_eq!(actual_hit.payload, expected_hit.payload);
        }
    }
    cleanup(&path);
}

#[test]
fn 批量搜索_空批次与单查询边界() {
    let path = tmp_db("batch_empty_single");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"single": true}))
        .unwrap();
    let config = SearchConfig {
        top_k: 10,
        expand_depth: 0,
        min_score: -1.0,
        ..Default::default()
    };

    assert!(
        db.search_batch(&[], &config, &BatchSearchConfig::default())
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        db.search_batch(
            &[],
            &SearchConfig {
                top_k: 0,
                ..Default::default()
            },
            &BatchSearchConfig::default()
        ),
        Err(TriviumError::InvalidInput(_))
    ));
    let hits = db
        .search_batch(
            &[vec![1.0, 0.0, 0.0, 0.0]],
            &config,
            &BatchSearchConfig { parallelism: 64 },
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0][0].id, id);
    cleanup(&path);
}

#[test]
fn 批量搜索_任一非法查询整批失败() {
    let path = tmp_db("batch_atomic_validation");
    cleanup(&path);
    let db = Database::<f32>::open(&path, DIM).unwrap();
    let config = SearchConfig::default();

    let dimension_error = db.search_batch(
        &[vec![1.0, 0.0, 0.0, 0.0], vec![1.0, 0.0]],
        &config,
        &BatchSearchConfig::default(),
    );
    assert!(matches!(
        dimension_error,
        Err(TriviumError::DimensionMismatch {
            expected: DIM,
            got: 2
        })
    ));
    let invalid_vector = db.search_batch(
        &[vec![1.0, 0.0, 0.0, 0.0], vec![f32::NAN, 0.0, 0.0, 0.0]],
        &config,
        &BatchSearchConfig::default(),
    );
    assert!(matches!(
        invalid_vector,
        Err(TriviumError::InvalidVector { .. })
    ));
    cleanup(&path);
}

#[test]
fn 批量搜索_拒绝危险配置() {
    let path = tmp_db("batch_rejected_config");
    cleanup(&path);
    let db = Database::<f32>::open(&path, DIM).unwrap();
    let queries = vec![vec![1.0, 0.0, 0.0, 0.0]];
    let fatigue_config = SearchConfig {
        enable_refractory_fatigue: true,
        ..Default::default()
    };
    assert!(matches!(
        db.search_batch(&queries, &fatigue_config, &BatchSearchConfig::default()),
        Err(TriviumError::InvalidInput(_))
    ));
    assert!(matches!(
        db.search_batch(
            &queries,
            &SearchConfig::default(),
            &BatchSearchConfig { parallelism: 65 }
        ),
        Err(TriviumError::InvalidInput(_))
    ));
    cleanup(&path);
}

#[test]
fn 批量搜索_关闭后拒绝旧句柄() {
    let path = tmp_db("batch_closed");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.close().unwrap();

    assert!(matches!(
        db.search_batch(
            &[vec![1.0, 0.0, 0.0, 0.0]],
            &SearchConfig::default(),
            &BatchSearchConfig::default()
        ),
        Err(TriviumError::DatabaseClosed)
    ));
    cleanup(&path);
}

#[test]
fn 批量搜索_同一实例多调用方并发稳定() {
    let path = tmp_db("batch_concurrent_callers");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for index in 0..128_u64 {
        db.insert(
            &[
                (index % 17) as f32 + 1.0,
                (index % 19) as f32 + 1.0,
                (index % 23) as f32 + 1.0,
                (index % 29) as f32 + 1.0,
            ],
            serde_json::json!({"index": index}),
        )
        .unwrap();
    }
    let queries = vec![
        vec![1.0, 0.0, 0.0, 0.0],
        vec![0.0, 1.0, 0.0, 0.0],
        vec![0.0, 0.0, 1.0, 0.0],
        vec![0.0, 0.0, 0.0, 1.0],
    ];
    let config = SearchConfig {
        top_k: 8,
        expand_depth: 0,
        min_score: -1.0,
        ..Default::default()
    };
    let expected = db
        .search_batch(&queries, &config, &BatchSearchConfig { parallelism: 4 })
        .unwrap()
        .into_iter()
        .map(|hits| hits.into_iter().map(|hit| hit.id).collect::<Vec<_>>())
        .collect::<Vec<_>>();

    std::thread::scope(|scope| {
        let handles = (0..12)
            .map(|_| {
                scope.spawn(|| {
                    for _ in 0..20 {
                        let actual = db
                            .search_batch(&queries, &config, &BatchSearchConfig { parallelism: 4 })
                            .unwrap()
                            .into_iter()
                            .map(|hits| hits.into_iter().map(|hit| hit.id).collect::<Vec<_>>())
                            .collect::<Vec<_>>();
                        assert_eq!(actual, expected);
                    }
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }
    });
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
        tx.insert(
            &[1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"label": "to_delete"}),
        );
        tx.insert(&[0.9, 0.1, 0.0, 0.0], serde_json::json!({"label": "keep1"}));
        tx.insert(&[0.8, 0.2, 0.0, 0.0], serde_json::json!({"label": "keep2"}));
        tx.commit().unwrap()
    };
    let del_id = ids[0];

    {
        let mut tx = db.begin_tx();
        tx.delete(del_id);
        tx.commit().unwrap();
    }

    let results = db.search(&[1.0, 0.0, 0.0, 0.0], 5, 0, 0.0).unwrap();
    assert!(
        !results.iter().any(|h| h.id == del_id),
        "已删除节点不应出现在搜索结果中"
    );

    cleanup(&path);
}

#[test]
fn P1_5_BQ搜索_更新向量后_结果反映变化() {
    let path = tmp_db("bq_update");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(
            &[1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"label": "node_a"}),
        );
        tx.insert(
            &[0.0, 1.0, 0.0, 0.0],
            serde_json::json!({"label": "node_b"}),
        );
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
            let v: Vec<f32> = (0..DIM)
                .map(|d| if d == (i % 4) as usize { 1.0 } else { 0.0 })
                .collect();
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
        for id in &ids {
            tx.delete(*id);
        }
        tx.commit().unwrap();
    }

    let results = db.search(&[1.0, 0.0, 0.0, 0.0], 10, 0, 0.0).unwrap();
    assert!(results.is_empty(), "全部删除后搜索应返回空");

    cleanup(&path);
}
