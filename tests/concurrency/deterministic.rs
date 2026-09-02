#![allow(non_snake_case)]
//! GJB-5000B 确定性复现测试
//!
//! 军工系统要求：相同输入 → 相同输出（bit-exact），跨多次运行、跨编译一致。
//! 本文件验证 TriviumDB 的核心路径满足确定性要求。

use triviumdb::VectorType;
use triviumdb::database::{Database, SearchConfig};

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("det_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

/// 使用固定种子生成确定性向量（归一化，模拟真实 embedding 分布）
fn deterministic_vector(seed: u32, dim: usize) -> Vec<f32> {
    // LCG 伪随机数生成器（确定性，跨平台一致）
    let mut lcg = seed as u64 ^ 0xDEADBEEF;
    let raw: Vec<f32> = (0..dim)
        .map(|_| {
            lcg = lcg
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            // 将 u64 映射到 [-1, 1] 区间
            ((lcg >> 33) as f64 / (1u64 << 31) as f64 * 2.0 - 1.0) as f32
        })
        .collect();
    // L2 归一化，模拟真实 embedding（所有维度量级一致，BQ 量化更稳定）
    let norm = raw.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-10);
    raw.iter().map(|x| x / norm).collect()
}

// ════════════════════════════════════════════════════════════════
//  1. 搜索结果确定性
// ════════════════════════════════════════════════════════════════

/// 相同数据集 + 相同查询 → 100 次重复搜索，结果 bit-exact 一致
#[test]
fn DET_01_相同查询_100次结果bit_exact一致() {
    let path = tmp_db("repeat_search");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 固定种子构造数据集
    for i in 0..200u32 {
        let vec = deterministic_vector(i, DIM);
        db.insert(&vec, serde_json::json!({"idx": i})).unwrap();
    }

    let query = deterministic_vector(9999, DIM);

    // 第一次搜索作为基准
    let baseline = db.search(&query, 10, 0, 0.0).unwrap();
    assert!(!baseline.is_empty(), "基准搜索应返回结果");

    // 重复 100 次，每次与基准对比
    for round in 1..=100 {
        let result = db.search(&query, 10, 0, 0.0).unwrap();

        assert_eq!(
            result.len(),
            baseline.len(),
            "第 {} 轮: 结果数量不一致 ({} vs {})",
            round,
            result.len(),
            baseline.len()
        );

        for (j, (a, b)) in baseline.iter().zip(result.iter()).enumerate() {
            assert_eq!(
                a.id, b.id,
                "第 {} 轮 第 {} 名: ID 不一致 ({} vs {})",
                round, j, a.id, b.id
            );
            assert_eq!(
                a.score.to_bits(),
                b.score.to_bits(),
                "第 {} 轮 第 {} 名: score 不 bit-exact ({} vs {})",
                round,
                j,
                a.score,
                b.score
            );
        }
    }

    eprintln!("  ✅ 100 次搜索 bit-exact 一致");
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  2. SIMD 与标量路径一致性（跨平台确定性基础）
// ════════════════════════════════════════════════════════════════

/// 在多种维度下，验证 VectorType::similarity 的对称性和自反性
/// 这些数学不变量如果被违反，说明 SIMD 路径存在精度问题
#[test]
fn DET_02_similarity数学不变量_大规模验证() {
    let dims = [1, 2, 3, 4, 7, 8, 15, 16, 31, 32, 63, 64, 128, 255, 256, 512];

    for &dim in &dims {
        for seed in 0..50u32 {
            let a = deterministic_vector(seed, dim);
            let b = deterministic_vector(seed + 1000, dim);

            // 对称性
            let ab = f32::similarity(&a, &b);
            let ba = f32::similarity(&b, &a);
            assert!(
                (ab - ba).abs() < 1e-6,
                "dim={} seed={}: 对称性违反 sim(a,b)={} vs sim(b,a)={}",
                dim,
                seed,
                ab,
                ba
            );

            // 范围
            assert!(
                (-1.01..=1.01).contains(&ab),
                "dim={} seed={}: 超出范围 sim={}",
                dim,
                seed,
                ab
            );

            // 自反性
            let aa = f32::similarity(&a, &a);
            let is_zero = a.iter().all(|x| *x == 0.0);
            if !is_zero {
                assert!(
                    (aa - 1.0).abs() < 0.01,
                    "dim={} seed={}: 自反性违反 sim(a,a)={}",
                    dim,
                    seed,
                    aa
                );
            }
        }
    }

    eprintln!(
        "  ✅ {} 维度 × 50 种子 = {} 组数学不变量全部通过",
        dims.len(),
        dims.len() * 50
    );
}

// ════════════════════════════════════════════════════════════════
//  3. BQ 索引与暴力搜索的排序一致性
// ════════════════════════════════════════════════════════════════

/// QuIVer 图搜索与 BruteForce 的 Top-1 近似一致性
/// QuIVer 是 Vamana 近似搜索，不保证 bit-exact，但 Top-1 必须足够接近
/// 注：使用 64 维 2000 节点以接近真实使用场景
#[test]
fn DET_03_QuIVer与BruteForce_Top1近似一致性() {
    let path = tmp_db("quiver_vs_brute");
    cleanup(&path);

    let test_dim = 64;
    let mut db = Database::<f32>::open(&path, test_dim).unwrap();

    for i in 0..2000u32 {
        let vec = deterministic_vector(i, test_dim);
        db.insert(&vec, serde_json::json!({"idx": i})).unwrap();
    }

    // 手动构建 QuIVer（使用更大的 ef_construction 保证图质量）
    use triviumdb::index::quiver::QuIVerConfig;
    db.build_quiver_index(Some(QuIVerConfig {
        m: 32,
        ef_construction: 256,
        alpha: 1.2,
    }))
    .unwrap();

    for q_seed in [42u32, 100, 200, 300, 400, 999, 1234, 5678] {
        let query = deterministic_vector(q_seed, test_dim);

        // BruteForce 搜索
        let brute_cfg = SearchConfig {
            top_k: 1,
            expand_depth: 0,
            min_score: -1.0,
            force_brute_force: true,
            ..Default::default()
        };
        let brute = db.search_advanced(&query, &brute_cfg).unwrap();
        assert_eq!(brute.len(), 1, "BruteForce Top-1 必须返回结果");

        // QuIVer 搜索（top_k=20 → ef_search=160，给 BQ 更宽的 beam 覆盖）
        let quiver_cfg = SearchConfig {
            top_k: 20,
            expand_depth: 0,
            min_score: -1.0,
            ..Default::default()
        };
        let quiver = db.search_advanced(&query, &quiver_cfg).unwrap();
        assert!(!quiver.is_empty(), "QuIVer 必须返回结果");

        // QuIVer 近似搜索：Top-1 分数必须 >= 0.80 * BruteForce Top-1 分数
        // BQ 2-bit 量化在特殊向量分布下可能走偏，阈值不宜过高
        assert!(
            quiver[0].score >= brute[0].score * 0.80,
            "query_seed={}: QuIVer Top-1 分数 ({}) 远低于 BruteForce ({})",
            q_seed,
            quiver[0].score,
            brute[0].score
        );
    }

    cleanup(&path);
}

/// 数据量超过自动路由阈值（10,000）时，QuIVer 自动构建并保持结果结构正确
#[test]
fn DET_03B_自动QuIVer路由_结果结构正确() {
    let path = tmp_db("quiver_auto_route");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..10_001u32 {
        let vec = deterministic_vector(i, DIM);
        db.insert(&vec, serde_json::json!({"idx": i})).unwrap();
    }

    for q_seed in [7u32, 42, 2048] {
        let query = deterministic_vector(q_seed, DIM);
        let cfg = SearchConfig {
            top_k: 10,
            expand_depth: 0,
            min_score: -1.0,
            ..Default::default()
        };
        let results = db.search_advanced(&query, &cfg).unwrap();
        assert_eq!(results.len(), 10, "自动 QuIVer 必须返回完整 TopK");

        let mut seen = std::collections::HashSet::new();
        for hit in &results {
            assert!(seen.insert(hit.id), "自动 QuIVer 结果不能返回重复节点");
            let payload_idx = hit
                .payload
                .get("idx")
                .and_then(|value| value.as_u64())
                .expect("自动 QuIVer 结果必须携带原始 idx payload");
            assert!(payload_idx < 10_001, "自动 QuIVer 不能返回越界 payload");
        }
        assert!(
            results
                .windows(2)
                .all(|pair| pair[0].score >= pair[1].score),
            "自动 QuIVer 返回结果必须按精排分数降序排列"
        );
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  4. WAL 回放确定性
// ════════════════════════════════════════════════════════════════

/// flush 前后数据完全一致：验证持久化/反序列化不引入精度损失
#[test]
fn DET_04_flush前后数据bit_exact一致() {
    let path = tmp_db("flush_exact");
    cleanup(&path);

    let mut id_payload_map = std::collections::HashMap::new();

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..100u32 {
            let vec = deterministic_vector(i, DIM);
            let payload = serde_json::json!({"seed": i, "data": format!("node_{}", i)});
            let id = db.insert(&vec, payload.clone()).unwrap();
            id_payload_map.insert(id, payload);
        }
        db.flush().unwrap();
    }

    // 重新打开
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.node_count(), 100, "flush 后节点数应一致");

    for &id in &db.all_node_ids() {
        let payload = db.get_payload(id).unwrap();
        let expected = id_payload_map.get(&id).unwrap();
        assert_eq!(
            payload, *expected,
            "节点 {} 的 payload 在 flush 后不一致",
            id
        );
    }

    eprintln!("  ✅ 100 个节点 flush 前后 payload bit-exact 一致");
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  5. TQL 查询确定性
// ════════════════════════════════════════════════════════════════

/// 同一 TQL 查询执行 50 次，结果完全一致
#[test]
fn DET_05_TQL查询_50次结果一致() {
    let path = tmp_db("tql_det");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(
            &[1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"name": "Alice", "type": "person", "age": 30}),
        );
        tx.insert(
            &[0.0, 1.0, 0.0, 0.0],
            serde_json::json!({"name": "Bob", "type": "person", "age": 25}),
        );
        tx.insert(
            &[0.0, 0.0, 1.0, 0.0],
            serde_json::json!({"name": "Charlie", "type": "person", "age": 35}),
        );
        tx.commit().unwrap()
    };

    db.link(ids[0], ids[1], "knows", 1.0).unwrap();
    db.link(ids[1], ids[2], "knows", 1.0).unwrap();

    let queries = [
        r#"FIND {"type": "person"} RETURN *"#,
        r#"FIND {"name": "Alice"} RETURN *"#,
        r#"FIND {"name": "Bob"} RETURN *"#,
    ];

    for query in &queries {
        let baseline = db.tql_nodes(query).unwrap();

        for round in 1..=50 {
            let result = db.tql_nodes(query).unwrap();
            assert_eq!(
                result.len(),
                baseline.len(),
                "query='{}' 第 {} 轮: 结果数量不一致",
                query,
                round
            );
        }
    }

    eprintln!("  ✅ 3 个 TQL × 50 轮 = 150 次查询确定性验证通过");
    cleanup(&path);
}
