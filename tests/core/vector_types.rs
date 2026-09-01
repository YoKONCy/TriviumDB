#![allow(non_snake_case)]
//! f16 / u64 向量类型集成测试
//!
//! 当前所有集成测试仅覆盖 Database<f32>，本文件补齐 f16（半精度压缩）和
//! u64（二进制哈希/SimHash）两种向量类型的核心业务流程验证：
//!
//! - CRUD 基础操作
//! - 持久化（flush + 重新加载）
//! - 向量搜索正确性
//! - 事务提交
//! - 崩溃恢复（WAL 回放）
//! - 精度边界与极端值

use half::f16;
use triviumdb::{BatchSearchConfig, Database, SearchConfig};

// ════════════════════════════════════════════════════════════════
//  公共基础设施
// ════════════════════════════════════════════════════════════════

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("vtype_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

/// 构造 f16 向量的辅助函数
fn f16_vec(vals: &[f32]) -> Vec<f16> {
    vals.iter().map(|&v| f16::from_f32(v)).collect()
}

// ════════════════════════════════════════════════════════════════
//  F16: 半精度向量类型测试
// ════════════════════════════════════════════════════════════════

const F16_DIM: usize = 4;

#[test]
fn F16_基础CRUD_插入查询删除() {
    let path = tmp_db("f16_crud");
    cleanup(&path);

    let mut db = Database::<f16>::open(&path, F16_DIM).unwrap();

    // 插入
    let id = db
        .insert(
            &f16_vec(&[1.0, 0.0, 0.0, 0.0]),
            serde_json::json!({"type": "f16", "label": "unit_x"}),
        )
        .unwrap();

    assert_eq!(db.node_count(), 1);
    assert!(db.contains(id));

    // 查询：通过 get() 获取 NodeView，其中包含 vector 字段
    let node = db.get(id).unwrap();
    assert_eq!(node.id, id);
    assert_eq!(node.payload["label"], "unit_x");
    assert_eq!(node.vector.len(), F16_DIM);
    assert!((node.vector[0].to_f32() - 1.0).abs() < 0.01);

    // 删除
    db.delete(id).unwrap();
    assert_eq!(db.node_count(), 0);
    assert!(!db.contains(id));

    cleanup(&path);
}

#[test]
fn F16_搜索_余弦相似度正确排序() {
    let path = tmp_db("f16_search");
    cleanup(&path);

    let mut db = Database::<f16>::open(&path, F16_DIM).unwrap();

    let id_target = db
        .insert(
            &f16_vec(&[1.0, 0.0, 0.0, 0.0]),
            serde_json::json!({"label": "target"}),
        )
        .unwrap();
    db.insert(
        &f16_vec(&[0.0, 1.0, 0.0, 0.0]),
        serde_json::json!({"label": "orthogonal"}),
    )
    .unwrap();
    db.insert(
        &f16_vec(&[0.9, 0.1, 0.0, 0.0]),
        serde_json::json!({"label": "near_target"}),
    )
    .unwrap();

    let query = f16_vec(&[1.0, 0.0, 0.0, 0.0]);
    let results = db.search(&query, 3, 0, 0.0).unwrap();

    assert!(!results.is_empty(), "f16 搜索结果不应为空");
    assert_eq!(
        results[0].id, id_target,
        "与 query 完全匹配的 f16 节点应排第一"
    );
    // 第二名应是 near_target（余弦相似度 ≈ 0.99），不是 orthogonal（余弦 = 0）
    assert_eq!(results[1].payload["label"], "near_target");

    cleanup(&path);
}

#[test]
fn F16_批量搜索_保持输入顺序() {
    let path = tmp_db("f16_batch_search");
    cleanup(&path);
    let mut db = Database::<f16>::open(&path, F16_DIM).unwrap();
    let first = db
        .insert(&f16_vec(&[1.0, 0.0, 0.0, 0.0]), serde_json::json!({}))
        .unwrap();
    let second = db
        .insert(&f16_vec(&[0.0, 1.0, 0.0, 0.0]), serde_json::json!({}))
        .unwrap();
    let queries = vec![
        f16_vec(&[0.0, 1.0, 0.0, 0.0]),
        f16_vec(&[1.0, 0.0, 0.0, 0.0]),
    ];
    let config = SearchConfig {
        top_k: 1,
        expand_depth: 0,
        min_score: -1.0,
        ..Default::default()
    };

    let results = db
        .search_batch(&queries, &config, &BatchSearchConfig { parallelism: 2 })
        .unwrap();
    assert_eq!(results[0][0].id, second);
    assert_eq!(results[1][0].id, first);
    cleanup(&path);
}

#[test]
fn F16_精确搜索_使用半精度存储值排序() {
    let path = tmp_db("f16_exact_search");
    cleanup(&path);
    let mut db = Database::<f16>::open(&path, F16_DIM).unwrap();
    let first = db
        .insert(
            &f16_vec(&[0.33331, 0.66663, 0.12503, 0.25007]),
            serde_json::json!({"rank": 1}),
        )
        .unwrap();
    let second = db
        .insert(
            &f16_vec(&[0.30001, 0.70003, 0.10001, 0.20003]),
            serde_json::json!({"rank": 2}),
        )
        .unwrap();
    let query = f16_vec(&[0.33331, 0.66663, 0.12503, 0.25007]);

    let hits = db.search_exact(&query, 10).unwrap();

    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].id, first);
    assert_eq!(hits[1].id, second);
    assert!((hits[0].score - 1.0).abs() < 1e-6);
    cleanup(&path);
}

#[test]
fn F16_持久化_flush后重新加载数据完整() {
    let path = tmp_db("f16_persist");
    cleanup(&path);

    // 第一阶段：写入并 flush
    {
        let mut db = Database::<f16>::open(&path, F16_DIM).unwrap();
        for i in 0..10u32 {
            db.insert(
                &f16_vec(&[i as f32, (i as f32).sin(), (i as f32).cos(), 1.0]),
                serde_json::json!({"idx": i}),
            )
            .unwrap();
        }
        db.flush().unwrap();
    }

    // 第二阶段：重新打开并验证
    let db = Database::<f16>::open(&path, F16_DIM).unwrap();
    assert_eq!(db.node_count(), 10, "flush 后重新加载应恢复全部 10 个节点");

    // 验证每个节点的 payload 语义完整
    for &id in &db.all_node_ids() {
        let payload = db.get_payload(id).expect("节点 payload 不应丢失");
        assert!(
            payload.get("idx").is_some(),
            "节点 {} 的 payload 应包含 idx 字段",
            id
        );
    }

    // 搜索应正常工作
    let query = f16_vec(&[0.0, 0.0, 0.0, 1.0]);
    let results = db.search(&query, 5, 0, 0.0).unwrap();
    assert!(!results.is_empty(), "重新加载后搜索不应返回空结果");

    cleanup(&path);
}

#[test]
fn F16_事务_提交与维度校验() {
    let path = tmp_db("f16_tx");
    cleanup(&path);

    let mut db = Database::<f16>::open(&path, F16_DIM).unwrap();

    // 正常事务提交
    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(
            &f16_vec(&[1.0, 0.0, 0.0, 0.0]),
            serde_json::json!({"tx": "committed"}),
        );
        tx.insert(
            &f16_vec(&[0.0, 1.0, 0.0, 0.0]),
            serde_json::json!({"tx": "committed"}),
        );
        tx.commit().unwrap()
    };
    assert_eq!(ids.len(), 2, "事务应返回 2 个 ID");
    assert_eq!(db.node_count(), 2);

    // 维度不匹配的事务应被拒绝
    let result = {
        let mut tx = db.begin_tx();
        tx.insert(
            &f16_vec(&[1.0, 0.0]), // 维度 2 ≠ 4
            serde_json::json!({"tx": "bad_dim"}),
        );
        tx.commit()
    };
    assert!(result.is_err(), "维度不匹配的 f16 事务应被 commit 拒绝");
    // 已提交的 2 个节点不受影响
    assert_eq!(db.node_count(), 2);

    cleanup(&path);
}

#[test]
fn F16_WAL恢复_未flush数据通过WAL回放() {
    let path = tmp_db("f16_wal");
    cleanup(&path);

    // 第一阶段：写入但不 flush（数据仅在 WAL 中）
    {
        let mut db = Database::<f16>::open(&path, F16_DIM).unwrap();
        for i in 0..5u32 {
            db.insert(
                &f16_vec(&[i as f32, 0.0, 0.0, 0.0]),
                serde_json::json!({"wal": true, "idx": i}),
            )
            .unwrap();
        }
        // drop 时 WAL BufWriter flush 但不 flush DB
    }

    // 第二阶段：重新打开 → WAL 回放
    let db = Database::<f16>::open(&path, F16_DIM).unwrap();
    assert_eq!(
        db.node_count(),
        5,
        "f16 WAL 回放应恢复全部 5 个未 flush 的节点"
    );

    for &id in &db.all_node_ids() {
        let p = db.get_payload(id).unwrap();
        assert_eq!(p["wal"], true, "WAL 恢复的节点 payload 应正确");
    }

    cleanup(&path);
}

#[test]
fn F16_更新向量与Payload() {
    let path = tmp_db("f16_update");
    cleanup(&path);

    let mut db = Database::<f16>::open(&path, F16_DIM).unwrap();
    let id = db
        .insert(
            &f16_vec(&[1.0, 0.0, 0.0, 0.0]),
            serde_json::json!({"version": 1}),
        )
        .unwrap();

    // 更新向量
    db.update_vector(id, &f16_vec(&[0.0, 0.0, 0.0, 1.0]))
        .unwrap();
    let node = db.get(id).unwrap();
    assert!(
        (node.vector[3].to_f32() - 1.0).abs() < 0.01,
        "更新后向量第 4 分量应为 1.0"
    );

    // 更新 payload
    db.update_payload(id, serde_json::json!({"version": 2}))
        .unwrap();
    let p = db.get_payload(id).unwrap();
    assert_eq!(p["version"], 2);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  U64: 二进制哈希 / SimHash 向量类型测试
// ════════════════════════════════════════════════════════════════

const U64_DIM: usize = 2; // 2 × 64bit = 128-bit 哈希指纹

#[test]
fn U64_基础CRUD_插入查询删除() {
    let path = tmp_db("u64_crud");
    cleanup(&path);

    let mut db = Database::<u64>::open(&path, U64_DIM).unwrap();

    let id = db
        .insert(
            &[0xFFFFFFFF_00000000u64, 0x12345678_9ABCDEF0u64],
            serde_json::json!({"type": "u64", "label": "hash_a"}),
        )
        .unwrap();

    assert_eq!(db.node_count(), 1);
    let node = db.get(id).unwrap();
    assert_eq!(node.payload["label"], "hash_a");
    assert_eq!(node.vector[0], 0xFFFFFFFF_00000000u64);
    assert_eq!(node.vector[1], 0x12345678_9ABCDEF0u64);

    db.delete(id).unwrap();
    assert_eq!(db.node_count(), 0);

    cleanup(&path);
}

#[test]
fn U64_搜索_汉明相似度正确排序() {
    let path = tmp_db("u64_search");
    cleanup(&path);

    let mut db = Database::<u64>::open(&path, U64_DIM).unwrap();

    // 目标哈希
    let target_hash = [0xFFFFFFFF_FFFFFFFFu64, 0xFFFFFFFF_FFFFFFFFu64];
    let id_exact = db
        .insert(&target_hash, serde_json::json!({"label": "exact"}))
        .unwrap();

    // 1 位差异（汉明距离 = 1）
    let id_near = db
        .insert(
            &[0xFFFFFFFF_FFFFFFFEu64, 0xFFFFFFFF_FFFFFFFFu64],
            serde_json::json!({"label": "near"}),
        )
        .unwrap();

    // 大量差异（全 0 vs 全 1，汉明距离 = 128）
    db.insert(
        &[0x0000000000000000u64, 0x0000000000000000u64],
        serde_json::json!({"label": "far"}),
    )
    .unwrap();

    let results = db.search(&target_hash, 3, 0, 0.0).unwrap();
    assert!(!results.is_empty(), "u64 搜索结果不应为空");
    assert_eq!(
        results[0].id, id_exact,
        "完全匹配的哈希应排第一（汉明距离 = 0）"
    );
    assert_eq!(
        results[1].id, id_near,
        "1 位差异的哈希应排第二（汉明距离 = 1）"
    );

    cleanup(&path);
}

#[test]
fn U64_批量搜索_汉明结果正确() {
    let path = tmp_db("u64_batch_search");
    cleanup(&path);
    let mut db = Database::<u64>::open(&path, U64_DIM).unwrap();
    let zero = db.insert(&[0, 0], serde_json::json!({})).unwrap();
    let ones = db
        .insert(&[u64::MAX, u64::MAX], serde_json::json!({}))
        .unwrap();
    let config = SearchConfig {
        top_k: 1,
        expand_depth: 0,
        min_score: -1.0,
        ..Default::default()
    };

    let results = db
        .search_batch(
            &[vec![u64::MAX, u64::MAX], vec![0, 0]],
            &config,
            &BatchSearchConfig { parallelism: 2 },
        )
        .unwrap();
    assert_eq!(results[0][0].id, ones);
    assert_eq!(results[1][0].id, zero);
    cleanup(&path);
}

#[test]
fn U64_持久化_flush后重新加载() {
    let path = tmp_db("u64_persist");
    cleanup(&path);

    {
        let mut db = Database::<u64>::open(&path, U64_DIM).unwrap();
        for i in 0..10u64 {
            db.insert(
                &[i, i.wrapping_mul(0x9E3779B97F4A7C15)], // 伪随机哈希
                serde_json::json!({"idx": i}),
            )
            .unwrap();
        }
        db.flush().unwrap();
    }

    let db = Database::<u64>::open(&path, U64_DIM).unwrap();
    assert_eq!(db.node_count(), 10, "u64 flush 后应恢复全部 10 个节点");

    // 验证向量数据完整性
    for &id in &db.all_node_ids() {
        let node = db.get(id).unwrap();
        assert_eq!(
            node.vector.len(),
            U64_DIM,
            "每个节点应有 {} 维 u64 向量",
            U64_DIM
        );
    }

    cleanup(&path);
}

#[test]
fn U64_事务_多操作原子提交() {
    let path = tmp_db("u64_tx");
    cleanup(&path);

    let mut db = Database::<u64>::open(&path, U64_DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[0xAAAA, 0xBBBB], serde_json::json!({"tx": 1}));
        tx.insert(&[0xCCCC, 0xDDDD], serde_json::json!({"tx": 2}));
        tx.insert(&[0xEEEE, 0xFFFF], serde_json::json!({"tx": 3}));
        tx.commit().unwrap()
    };
    assert_eq!(ids.len(), 3);
    assert_eq!(db.node_count(), 3);

    for &id in &ids {
        assert!(db.contains(id), "事务提交的 ID {} 应可访问", id);
    }

    cleanup(&path);
}

#[test]
fn U64_WAL恢复() {
    let path = tmp_db("u64_wal");
    cleanup(&path);

    {
        let mut db = Database::<u64>::open(&path, U64_DIM).unwrap();
        for i in 0..8u64 {
            db.insert(&[i, !i], serde_json::json!({"idx": i})).unwrap();
        }
    }

    let db = Database::<u64>::open(&path, U64_DIM).unwrap();
    assert_eq!(db.node_count(), 8, "u64 WAL 回放应恢复全部 8 个节点");

    let results = db.search(&[0u64, !0u64], 3, 0, 0.0).unwrap();
    assert!(!results.is_empty(), "WAL 恢复后搜索应正常");

    cleanup(&path);
}

#[test]
fn U64_更新向量与Payload() {
    let path = tmp_db("u64_update");
    cleanup(&path);

    let mut db = Database::<u64>::open(&path, U64_DIM).unwrap();
    let id = db
        .insert(&[0xAAAA, 0xBBBB], serde_json::json!({"v": 1}))
        .unwrap();

    db.update_vector(id, &[0xCCCC, 0xDDDD]).unwrap();
    let node = db.get(id).unwrap();
    assert_eq!(node.vector[0], 0xCCCC);
    assert_eq!(node.vector[1], 0xDDDD);

    db.update_payload(id, serde_json::json!({"v": 2})).unwrap();
    assert_eq!(db.get_payload(id).unwrap()["v"], 2);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  混合边界测试
// ════════════════════════════════════════════════════════════════

#[test]
fn F16_精度边界_极小值不丢失() {
    let path = tmp_db("f16_precision");
    cleanup(&path);

    let mut db = Database::<f16>::open(&path, F16_DIM).unwrap();

    // f16 最小正规数 ≈ 6.1e-5，最大值 ≈ 65504
    let id = db
        .insert(
            &f16_vec(&[65504.0, 0.0001, -65504.0, 0.0]),
            serde_json::json!({"edge_case": true}),
        )
        .unwrap();

    let node = db.get(id).unwrap();
    // f16 有精度损失，但不应变成 0 或 NaN
    assert!(node.vector[0].to_f32() > 60000.0, "f16 最大值应保留");
    assert!(node.vector[1].to_f32() > 0.0, "f16 极小值不应变为 0");
    assert!(node.vector[2].to_f32() < -60000.0, "f16 最小值应保留");
    assert!(!node.vector[0].to_f32().is_nan(), "f16 值不应变为 NaN");

    cleanup(&path);
}

#[test]
fn U64_全零与全一_哈希极端值() {
    let path = tmp_db("u64_extreme");
    cleanup(&path);

    let mut db = Database::<u64>::open(&path, U64_DIM).unwrap();

    let id_zero = db
        .insert(&[0u64, 0u64], serde_json::json!({"hash": "all_zero"}))
        .unwrap();
    let id_ones = db
        .insert(
            &[u64::MAX, u64::MAX],
            serde_json::json!({"hash": "all_ones"}),
        )
        .unwrap();

    // 全 0 搜全 0 应完全匹配
    let results = db.search(&[0u64, 0u64], 2, 0, 0.0).unwrap();
    assert_eq!(results[0].id, id_zero, "全零搜全零应精确匹配");

    // 全 1 搜全 1 应完全匹配
    let results = db.search(&[u64::MAX, u64::MAX], 2, 0, 0.0).unwrap();
    assert_eq!(results[0].id, id_ones, "全一搜全一应精确匹配");

    cleanup(&path);
}

#[test]
fn F16_大批量插入搜索_50节点() {
    let path = tmp_db("f16_bulk");
    cleanup(&path);

    let mut db = Database::<f16>::open(&path, F16_DIM).unwrap();

    for i in 0..50u32 {
        let angle = i as f32 * 0.1;
        db.insert(
            &f16_vec(&[angle.cos(), angle.sin(), 0.0, 1.0]),
            serde_json::json!({"idx": i}),
        )
        .unwrap();
    }

    assert_eq!(db.node_count(), 50);

    // flush + 重新加载
    db.flush().unwrap();
    drop(db);
    let db = Database::<f16>::open(&path, F16_DIM).unwrap();
    assert_eq!(
        db.node_count(),
        50,
        "f16 大批量 flush 后应保留全部 50 个节点"
    );

    // 搜索应返回正确数量
    let query = f16_vec(&[1.0, 0.0, 0.0, 1.0]);
    let results = db.search(&query, 10, 0, 0.0).unwrap();
    assert_eq!(results.len(), 10, "top_k=10 应返回 10 条结果");

    cleanup(&path);
}

#[test]
fn U64_大批量插入搜索_50节点() {
    let path = tmp_db("u64_bulk");
    cleanup(&path);

    let mut db = Database::<u64>::open(&path, U64_DIM).unwrap();

    for i in 0..50u64 {
        db.insert(
            &[i, i.wrapping_mul(0x517CC1B727220A95)],
            serde_json::json!({"idx": i}),
        )
        .unwrap();
    }
    assert_eq!(db.node_count(), 50);

    db.flush().unwrap();
    drop(db);
    let db = Database::<u64>::open(&path, U64_DIM).unwrap();
    assert_eq!(
        db.node_count(),
        50,
        "u64 大批量 flush 后应保留全部 50 个节点"
    );

    let results = db
        .search(&[25u64, 25u64.wrapping_mul(0x517CC1B727220A95)], 10, 0, 0.0)
        .unwrap();
    assert_eq!(results.len(), 10, "u64 top_k=10 应返回 10 条结果");

    cleanup(&path);
}

#[test]
fn F16_图谱边_插入与查询() {
    let path = tmp_db("f16_edges");
    cleanup(&path);

    let mut db = Database::<f16>::open(&path, F16_DIM).unwrap();

    let id_a = db
        .insert(
            &f16_vec(&[1.0, 0.0, 0.0, 0.0]),
            serde_json::json!({"name": "A"}),
        )
        .unwrap();
    let id_b = db
        .insert(
            &f16_vec(&[0.0, 1.0, 0.0, 0.0]),
            serde_json::json!({"name": "B"}),
        )
        .unwrap();

    db.link(id_a, id_b, "knows", 1.0).unwrap();
    let edges = db.get_edges(id_a);
    assert_eq!(edges.len(), 1, "A 应有 1 条出边");
    assert_eq!(edges[0].target_id, id_b, "出边应指向 B");
    assert_eq!(edges[0].label, "knows");

    cleanup(&path);
}
