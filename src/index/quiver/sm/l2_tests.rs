//! 无训练L2的精算时序、完整预算、线程一致性和实际数据库入口验证。
use super::*;

fn fixture(n: usize) -> Vec<f32> {
    let mut x = (0..n * DIM)
        .map(|j| ((j.wrapping_mul(7919) % 997) as f32 - 498.) / 16000.)
        .collect::<Vec<_>>();
    for row in x.as_chunks_mut::<DIM>().0.iter_mut() {
        let norm = row.iter().map(|&v| (v as f64).powi(2)).sum::<f64>().sqrt();
        for v in row {
            *v = (*v as f64 / norm) as f32;
        }
    }
    x
}
fn index(base: &[f32]) -> SmIndex {
    let mut model = index_with_coefficients(base, &[]);
    model.use_reconstructed_l2();
    model
}
fn index_with_coefficients(base: &[f32], coefficients: &[f32]) -> SmIndex {
    let n = base.len() / DIM;
    let graph = (0..n)
        .map(|u| (1..=16).map(|d| ((u + d) % n) as u32).collect())
        .collect();
    SmIndex::from_parts(base, coefficients, graph, (0..n as u64).collect(), 0, 0).unwrap()
}
#[test]
fn l2_exact_access_is_only_final_rerank_and_budget_is_complete() {
    let base = fixture(128);
    let model = index(&base);
    for budget in [16, 32, 64, 96] {
        let cfg = QuIVerSearchConfig {
            top_k: 10,
            ef_search: 112,
            rerank_limit: Some(budget),
        };
        let expected = model
            .search_traced_with_execution(
                &base[19 * DIM..20 * DIM],
                &base,
                &cfg,
                KernelMode::Optimized,
            )
            .unwrap();
        assert_eq!(expected.trace.exact.len(), budget);
        let mut accesses = Vec::new();
        let actual = model
            .search_with_vectors_with_execution(
                &base[19 * DIM..20 * DIM],
                &cfg,
                KernelMode::Optimized,
                |id, buffer| {
                    accesses.push(id as u32);
                    buffer.extend_from_slice(&base[id * DIM..(id + 1) * DIM]);
                    true
                },
            )
            .unwrap();
        assert_eq!(
            accesses, expected.trace.exact,
            "无锚点模式不得在精排集合之外访问FP32"
        );
        assert_eq!(actual.neighbors, expected.neighbors);
        assert_eq!(
            accesses
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            budget
        );
        let reference = model
            .search_traced_with_execution(
                &base[19 * DIM..20 * DIM],
                &base,
                &cfg,
                KernelMode::Reference,
            )
            .unwrap();
        assert_eq!(reference.neighbors, expected.neighbors);
        assert_eq!(reference.trace, expected.trace);
    }
}
#[test]
fn l2_threads_reproduce_single_thread_outputs() {
    use rayon::prelude::*;
    let base = fixture(128);
    let model = index(&base);
    let cfg = QuIVerSearchConfig {
        top_k: 10,
        ef_search: 64,
        rerank_limit: Some(32),
    };
    let expected = base
        .as_chunks::<DIM>()
        .0
        .iter()
        .take(32)
        .map(|q| {
            model
                .search_with_execution(q, &base, &cfg, KernelMode::Optimized)
                .unwrap()
                .neighbors
        })
        .collect::<Vec<_>>();
    for threads in [1, 8, 16, 32] {
        let actual = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                base.par_chunks_exact(DIM)
                    .take(32)
                    .map(|q| {
                        model
                            .search_with_execution(q, &base, &cfg, KernelMode::Optimized)
                            .unwrap()
                            .neighbors
                    })
                    .collect::<Vec<_>>()
            });
        assert_eq!(actual, expected);
    }
}
#[test]
fn l2_memtable_slot_mapping_normalization_and_invalidation() {
    let base = fixture(128);
    let mut mt = crate::storage::memtable::MemTable::<f32>::new(DIM);
    mt.set_auto_build_quiver(false);
    for row in base.as_chunks::<DIM>().0.iter() {
        mt.insert(
            &row.iter().map(|v| v * 3.).collect::<Vec<_>>(),
            serde_json::json!({}),
        )
        .unwrap();
    }
    mt.build_quiver(&super::super::QuIVerConfig {
        m: 8,
        ef_construction: 32,
        alpha: 1.2,
    });
    mt.enable_quiver_sm_l2().unwrap();
    assert!(mt.quiver_sm_l2_enabled());
    assert!(!mt.quiver_corrected_sm_enabled());
    let cfg = QuIVerSearchConfig {
        top_k: 10,
        ef_search: 64,
        rerank_limit: Some(64),
    };
    let q = base[47 * DIM..48 * DIM]
        .iter()
        .map(|v| v * 7.)
        .collect::<Vec<_>>();
    let hits = mt.quiver().unwrap().search(
        &q,
        |slot, buffer| {
            if let Some(row) = mt.vec_pool().get(slot) {
                buffer.extend_from_slice(row);
                true
            } else {
                false
            }
        },
        &cfg,
    );
    assert_eq!(hits[0].0, 48);
    assert!((hits[0].1 - 1.).abs() < 1e-5);
    mt.insert(&base[..DIM], serde_json::json!({})).unwrap();
    assert!(mt.quiver_sm_l2_enabled(), "写入后继续使用默认SM-L2");
}
#[test]
fn l2_database_public_api_is_runtime_only_and_reload_does_not_guess_mode() {
    let directory = std::env::temp_dir().join(format!(
        "tdb-l2-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let base = fixture(128);
    let mut db = crate::Database::<f32>::open_with_config(
        directory.join("l2.tdb").to_str().unwrap(),
        crate::database::Config {
            dim: DIM,
            auto_build_quiver: false,
            ..Default::default()
        },
    )
    .unwrap();
    for row in base.as_chunks::<DIM>().0.iter() {
        db.insert(row, serde_json::json!({})).unwrap();
    }
    db.build_quiver_index(Some(super::super::QuIVerConfig {
        m: 8,
        ef_construction: 32,
        alpha: 1.2,
    }))
    .unwrap();
    db.enable_quiver_sm_l2().unwrap();
    assert!(db.quiver_sm_l2_enabled());
    let hits = db.search(&base[23 * DIM..24 * DIM], 10, 0, 0.).unwrap();
    assert!(!hits.is_empty());
    let model = index(&base);
    let path = directory.join("l2-sm.bin");
    model.save(&path).unwrap();
    let loaded = SmIndex::load(&path, &base).unwrap();
    assert_eq!(loaded.scoring, SmScoring::CorrectedInnerProduct);
    assert!(loaded.rows.iter().all(|row| row.reconstructed_norm == 0.));
    drop(db);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn l2_inline_norm_preserves_legacy_scores_and_owned_memory() {
    let base = fixture(128);
    let coefficients = (0..128).map(|i| (i % 7) as f32 / 100.).collect::<Vec<_>>();
    let mut model = index_with_coefficients(&base, &coefficients);
    let owned_before = model.routing_owned_bytes();
    // 保留旧独立数组的计算次序，检查迁移后的逐位评分兼容性。
    let legacy_norms = model
        .rows
        .iter()
        .map(|row| {
            let high = row.weak + row.delta;
            (DIM - row.strong_count as usize) as f32 * row.weak * row.weak
                + row.strong_count as f32 * high * high
        })
        .collect::<Vec<_>>();
    model.use_reconstructed_l2();
    assert_eq!(owned_before, model.routing_owned_bytes());
    assert_eq!(std::mem::size_of::<SmRow>(), 256);
    assert_eq!(std::mem::offset_of!(SmRow, reconstructed_norm), 204);
    let memory = model.memory_components();
    assert_eq!(memory["reconstructed_norms_bytes"], 0);
    assert_eq!(memory["inline_reconstructed_norms_bytes"], 128 * 4);
    assert_eq!(
        memory["inline_reconstructed_norms_included_in_sm_rows"],
        true
    );
    for (i, row) in model.rows.iter().enumerate() {
        assert_eq!(row.reconstructed_norm.to_bits(), legacy_norms[i].to_bits());
        assert_eq!(row.coefficient.to_bits(), coefficients[i].to_bits());
    }
    for query in base.as_chunks::<DIM>().0.iter().step_by(13) {
        let q = query_scalar(query);
        let high = q.weak + q.delta;
        let norm = (DIM as f32 - q.count) * q.weak * q.weak + q.count * high * high;
        for (id, row) in model.rows.iter().enumerate() {
            let expected = 1. - (norm + legacy_norms[id] - 2. * scalar_score(row, &q));
            assert_eq!(
                model.navigation_score(&q, id, 0., norm, false).to_bits(),
                expected.to_bits()
            );
        }
    }
}

#[test]
fn l2_inline_norm_keeps_snapshot_bytes_and_reload_behavior() {
    let directory = std::env::temp_dir().join(format!(
        "tdb-inline-norm-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let base = fixture(128);
    let coefficients = vec![0.125; 128];
    let mut model = index_with_coefficients(&base, &coefficients);
    let before = directory.join("before.bin");
    let after = directory.join("after.bin");
    model.save(&before).unwrap();
    model.use_reconstructed_l2();
    model.save(&after).unwrap();
    assert_eq!(
        std::fs::read(&before).unwrap(),
        std::fs::read(&after).unwrap()
    );
    let cfg = QuIVerSearchConfig {
        top_k: 10,
        ef_search: 64,
        rerank_limit: Some(32),
    };
    let expected = model
        .search_traced_with_execution(
            &base[19 * DIM..20 * DIM],
            &base,
            &cfg,
            KernelMode::Optimized,
        )
        .unwrap();
    let mut loaded = SmIndex::load(&after, &base).unwrap();
    assert_eq!(loaded.scoring, SmScoring::CorrectedInnerProduct);
    assert!(loaded.rows.iter().all(|row| row.reconstructed_norm == 0.));
    loaded.use_reconstructed_l2();
    let actual = loaded
        .search_traced_with_execution(
            &base[19 * DIM..20 * DIM],
            &base,
            &cfg,
            KernelMode::Optimized,
        )
        .unwrap();
    assert_eq!(actual.neighbors, expected.neighbors);
    assert_eq!(actual.trace, expected.trace);
    assert_eq!(actual.counts, expected.counts);
    std::fs::remove_dir_all(directory).unwrap();
}
