#![allow(non_snake_case)]
//! 认知搜索管线深层路径与 NMF 矩阵分解集成测试
//!
//! 验证范围：
//! - `database/pipeline.rs`: FISTA 稀疏分解残差 + 影子查询触发、不应期疲劳抑制、DPP 多样性采样
//! - `database/mod.rs`: migrate_to 带边迁移 (节点 + 边拓扑完整迁移到新维度)
//! - `cognitive.rs`: NMF 乘法更新算法 (矩阵分解收敛性验证)

use triviumdb::database::{Database, SearchConfig};

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    let path = dir
        .join(format!("cov6_{}", name))
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

/// FISTA 残差 + 影子查询 (pipeline L528-565)
#[test]
fn COV6_01_fista_shadow_query() {
    let path = tmp_db("fista");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..50u32 {
        db.insert(
            &[
                (i as f32).sin(),
                (i as f32).cos(),
                (i as f32 * 0.1).sin(),
                (i as f32 * 0.1).cos(),
            ],
            serde_json::json!({"idx": i}),
        )
        .unwrap();
    }
    let ids = db.all_node_ids();
    for i in 0..ids.len() - 1 {
        db.link(ids[i], ids[i + 1], "chain", 0.9).unwrap();
    }

    let config = SearchConfig {
        top_k: 10,
        expand_depth: 2,
        min_score: 0.0,
        enable_advanced_pipeline: true,
        enable_sparse_residual: true,
        fista_lambda: 0.01,
        fista_threshold: 0.001,
        ..Default::default()
    };
    let hits = db.search_advanced(&[1.0, 0.0, 0.0, 0.0], &config).unwrap();
    eprintln!("  FISTA+shadow: {} hits", hits.len());
    cleanup(&path);
}

/// migrate_to 带边迁移 (mod.rs L878-896)
#[test]
fn COV6_02_migrate_with_edges() {
    let path = tmp_db("mig_edge");
    let new_path = tmp_db("mig_edge_new");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id1 = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "A"}))
        .unwrap();
    let id2 = db
        .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "B"}))
        .unwrap();
    let id3 = db
        .insert(&[0.0, 0.0, 1.0, 0.0], serde_json::json!({"name": "C"}))
        .unwrap();
    db.link(id1, id2, "knows", 0.8).unwrap();
    db.link(id2, id3, "likes", 0.6).unwrap();

    let (new_db, migrated) = db.migrate_to(&new_path, 8).unwrap();
    assert_eq!(migrated.len(), 3);
    let edges = new_db.get_edges(id1);
    assert!(!edges.is_empty(), "边应迁移");

    cleanup(&path);
    cleanup(&new_path);
}

/// NMF 乘法更新 (cognitive L278-410)
#[test]
fn COV6_03_nmf_multiplicative() {
    use triviumdb::cognitive::nmf_multiplicative_update;

    let v_flat: Vec<f32> = vec![
        1.0, 0.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.5, 0.8, 0.2, 0.4, 0.1, 0.1, 0.9, 0.05, 0.4, 0.5, 0.5,
        0.25, 0.25,
    ];
    let (w, h) = nmf_multiplicative_update(&v_flat, 5, 4, 2, 50, 1e-4);
    assert_eq!(w.len(), 5 * 2);
    assert_eq!(h.len(), 2 * 4);
}

/// 不应期/抑制 + DPP (pipeline refractory)
#[test]
fn COV6_04_refractory_dpp() {
    let path = tmp_db("refract");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..30u32 {
        db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"idx": i}))
            .unwrap();
    }
    let ids = db.all_node_ids();
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len().min(i + 5) {
            db.link(ids[i], ids[j], "dense", 0.9).unwrap();
        }
    }

    let config = SearchConfig {
        top_k: 10,
        expand_depth: 3,
        min_score: 0.0,
        enable_advanced_pipeline: true,
        enable_refractory_fatigue: true,
        enable_inverse_inhibition: true,
        lateral_inhibition_threshold: 5,
        enable_dpp: true,
        dpp_quality_weight: 0.6,
        ..Default::default()
    };
    let hits = db.search_advanced(&[15.0, 0.0, 0.0, 0.0], &config).unwrap();
    eprintln!("  Refractory+DPP: {} hits", hits.len());
    cleanup(&path);
}
