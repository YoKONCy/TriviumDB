use serde_json::json;
use triviumdb::graph::reachability::ReachabilityDirection;
use triviumdb::index::quiver::QuIVerConfig;
use triviumdb::{
    BeamAdaptation, Database, Filter, GraphSignalQuery, IndustrialAccessPath,
    IndustrialSearchConfig, QueryMemoryBudget, TsngBudget, TsngQuery, TsngSearchConfig,
    TsngWeights, quality_metrics,
};

const DIM: usize = 16;
const NODES: usize = 256;

fn cleanup(path: &str) {
    for suffix in ["", ".wal", ".vec", ".lock", ".flush_ok", ".pidx"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

fn vector(index: usize) -> Vec<f32> {
    let mut output = (0..DIM)
        .map(|axis| {
            let mixed = index
                .wrapping_mul(0x9E37)
                .wrapping_add(axis.wrapping_mul(0x85EB));
            (mixed % 2003) as f32 / 1001.5 - 1.0
        })
        .collect::<Vec<_>>();
    let norm = output.iter().map(|value| value * value).sum::<f32>().sqrt();
    for value in &mut output {
        *value /= norm.max(f32::EPSILON);
    }
    output
}

fn database(name: &str, build_quiver: bool) -> (String, Database<f32>) {
    let directory = std::env::temp_dir().join("triviumdb_tsng_c1");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{}_{}.tdb", name, std::process::id()));
    let path = path.to_string_lossy().to_string();
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for index in 0..NODES {
        db.insert_with_id(
            index as u64 + 1,
            &vector(index),
            json!({
                "tenant": index % 4,
                "active": index % 3 != 0,
            }),
        )
        .unwrap();
    }
    for index in 1..NODES {
        db.link(index as u64, index as u64 + 1, "related", 1.0)
            .unwrap();
        if index + 7 <= NODES {
            db.link(index as u64, (index + 7) as u64, "related", 0.8)
                .unwrap();
        }
    }
    if build_quiver {
        db.build_quiver_index(Some(QuIVerConfig {
            m: 16,
            ef_construction: 64,
            alpha: 1.2,
        }))
        .unwrap();
    }
    (path, db)
}

fn pure_query<'a>(vector: &'a [f32], top_k: usize) -> TsngQuery<'a, f32> {
    TsngQuery {
        vector,
        payload_filter: None,
        graph: None,
        top_k,
        weights: TsngWeights::default(),
        budget: TsngBudget::default(),
    }
}

#[test]
fn c1_纯向量_tsng_显式走原始_bq_路径且结果确定() {
    let (path, mut db) = database("pure_vector", true);
    let query_vector = vector(73);
    let config = TsngSearchConfig {
        ef_search: 128,
        candidate_pool: 128,
        metadata_bonus_cap_ppm: 200_000,
        signal_queue_quota_ppm: 0,
        graph_seed_limit: 0,
    };
    let first = db
        .search_tsng(&pure_query(&query_vector, 10), config)
        .unwrap();
    assert_eq!(first.metrics.navigation_scores, 0);
    assert_eq!(first.metrics.navigation_property_checks, 0);
    assert_eq!(first.metrics.navigation_graph_checks, 0);
    assert!(first.metrics.candidates_reranked > 0);
    for _ in 0..20 {
        assert_eq!(
            db.search_tsng(&pure_query(&query_vector, 10), config)
                .unwrap(),
            first
        );
    }
    let exact = db
        .tsng_ground_truth(&pure_query(&query_vector, 10))
        .unwrap();
    let ids = first.hits.iter().map(|hit| hit.id).collect::<Vec<_>>();
    let quality = quality_metrics(&exact.hits, &ids, 10);
    assert!(quality.recall_at_k >= 0.8, "{quality:?}");
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_属性硬过滤不允许未命中节点进入最终结果() {
    let (path, mut db) = database("property", true);
    let query_vector = vector(41);
    let filter = Filter::from_json(&json!({"tenant": 1, "active": true})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.7,
            property: 0.3,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let output = db
        .search_tsng(
            &query,
            TsngSearchConfig {
                ef_search: 192,
                candidate_pool: 192,
                metadata_bonus_cap_ppm: 200_000,
                signal_queue_quota_ppm: 0,
                graph_seed_limit: 0,
            },
        )
        .unwrap();
    assert!(!output.hits.is_empty());
    assert!(output.hits.iter().all(|hit| hit.property_signal == 1.0));
    assert!(output.metrics.navigation_scores > 0);
    assert_eq!(
        output.metrics.navigation_property_checks,
        output.metrics.navigation_scores
    );
    assert_eq!(output.metrics.navigation_graph_checks, 0);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_三信号导航返回精确重排分量与质量指标() {
    let (path, mut db) = database("three_signal", true);
    let query_vector = vector(9);
    let filter = Filter::from_json(&json!({"active": true})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: Some(GraphSignalQuery {
            anchor_id: 1,
            direction: ReachabilityDirection::Outgoing,
            labels: Some(vec!["related".into()]),
            min_edge_weight: 0.5,
            max_hops: 4,
        }),
        top_k: 10,
        weights: TsngWeights {
            vector: 0.6,
            property: 0.2,
            graph: 0.2,
        },
        budget: TsngBudget::default(),
    };
    let exact = db.tsng_ground_truth(&query).unwrap();
    let output = db
        .search_tsng(
            &query,
            TsngSearchConfig {
                ef_search: 256,
                candidate_pool: 256,
                metadata_bonus_cap_ppm: 200_000,
                signal_queue_quota_ppm: 0,
                graph_seed_limit: 0,
            },
        )
        .unwrap();
    assert_eq!(output.metrics.navigation_scores, 256);
    assert_eq!(
        output.metrics.navigation_property_checks,
        output.metrics.navigation_scores
    );
    assert_eq!(
        output.metrics.navigation_graph_checks,
        output.metrics.navigation_scores
    );
    assert!(output.hits.iter().all(|hit| hit.property_signal == 1.0));
    let ids = output.hits.iter().map(|hit| hit.id).collect::<Vec<_>>();
    let quality = quality_metrics(&exact.hits, &ids, 10);
    assert_eq!(quality.recall_at_k, 1.0);
    assert_eq!(quality.ndcg_at_k, 1.0);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_没有_quiver_时明确回退精确_ground_truth() {
    let (path, mut db) = database("exact_fallback", false);
    let query_vector = vector(101);
    let query = pure_query(&query_vector, 12);
    let exact = db.tsng_ground_truth(&query).unwrap();
    let output = db
        .search_tsng(&query, TsngSearchConfig::for_top_k(12))
        .unwrap();
    assert_eq!(output.hits, exact.hits);
    assert_eq!(output.metrics.navigation_scores, 0);
    assert_eq!(output.metrics.candidates_reranked, NODES);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_串行后过滤基线使用原始_bq_导航并执行相同精确重排() {
    let (path, mut db) = database("post_filter_baseline", true);
    let query_vector = vector(41);
    let filter = Filter::from_json(&json!({"active": true})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.7,
            property: 0.3,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let exact = db.tsng_ground_truth(&query).unwrap();
    let output = db
        .search_tsng_post_filter(
            &query,
            TsngSearchConfig {
                ef_search: NODES,
                candidate_pool: NODES,
                metadata_bonus_cap_ppm: 0,
                signal_queue_quota_ppm: 0,
                graph_seed_limit: 0,
            },
        )
        .unwrap();
    assert_eq!(output.hits, exact.hits);
    assert!(output.metrics.navigation_scores > 0);
    assert_eq!(output.metrics.navigation_property_checks, 0);
    assert!(output.hits.iter().all(|hit| hit.property_signal == 1.0));
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_bounded_bonus_零上限与后过滤基线逐项一致() {
    let (path, mut db) = database("bounded_zero", true);
    let query_vector = vector(41);
    let filter = Filter::from_json(&json!({"active": true})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.7,
            property: 0.3,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let config = TsngSearchConfig {
        ef_search: 128,
        candidate_pool: 128,
        metadata_bonus_cap_ppm: 0,
        signal_queue_quota_ppm: 0,
        graph_seed_limit: 0,
    };
    let baseline = db.search_tsng_post_filter(&query, config).unwrap();
    let bounded = db.search_tsng(&query, config).unwrap();
    assert_eq!(bounded.hits, baseline.hits);
    assert_eq!(
        bounded.metrics.navigation_scores,
        baseline.metrics.navigation_scores
    );
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_bounded_bonus_非法上限明确拒绝() {
    let (path, mut db) = database("bounded_invalid", true);
    let query_vector = vector(3);
    let query = pure_query(&query_vector, 10);
    assert!(
        db.search_tsng(
            &query,
            TsngSearchConfig {
                ef_search: 64,
                candidate_pool: 64,
                metadata_bonus_cap_ppm: 1_000_001,
                signal_queue_quota_ppm: 0,
                graph_seed_limit: 0,
            },
        )
        .is_err()
    );
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_双队列确定性且保持_payload_硬过滤() {
    let (path, mut db) = database("dual_queue", true);
    let query_vector = vector(41);
    let filter = Filter::from_json(&json!({"active": true})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.7,
            property: 0.3,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let config = TsngSearchConfig {
        ef_search: 128,
        candidate_pool: 128,
        metadata_bonus_cap_ppm: 100_000,
        signal_queue_quota_ppm: 250_000,
        graph_seed_limit: 0,
    };
    let first = db.search_tsng(&query, config).unwrap();
    assert!(!first.hits.is_empty());
    assert!(first.hits.iter().all(|hit| hit.property_signal == 1.0));
    for _ in 0..20 {
        assert_eq!(db.search_tsng(&query, config).unwrap(), first);
    }
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_图通道按上限注入业务节点且最终语义精确() {
    let (path, mut db) = database("graph_channel", true);
    let query_vector = vector(200);
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: None,
        graph: Some(GraphSignalQuery {
            anchor_id: 1,
            direction: ReachabilityDirection::Outgoing,
            labels: Some(vec!["related".into()]),
            min_edge_weight: 0.5,
            max_hops: 3,
        }),
        top_k: 10,
        weights: TsngWeights {
            vector: 0.6,
            property: 0.0,
            graph: 0.4,
        },
        budget: TsngBudget::default(),
    };
    let output = db
        .search_tsng(
            &query,
            TsngSearchConfig {
                ef_search: 32,
                candidate_pool: 32,
                metadata_bonus_cap_ppm: 100_000,
                signal_queue_quota_ppm: 250_000,
                graph_seed_limit: 8,
            },
        )
        .unwrap();
    assert_eq!(output.metrics.graph_seeds_injected, 8);
    assert!(output.hits.iter().any(|hit| hit.graph_signal > 0.0));
    assert!(
        output
            .hits
            .iter()
            .all(|hit| hit.graph_depth.is_some() == (hit.graph_signal > 0.0))
    );
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_图候选并集覆盖可达集合时与精确答案一致() {
    let (path, mut db) = database("graph_union_exact", true);
    let query_vector = vector(200);
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&Filter::from_json(&json!({"active": true})).unwrap()),
        graph: Some(GraphSignalQuery {
            anchor_id: 1,
            direction: ReachabilityDirection::Outgoing,
            labels: Some(vec!["related".into()]),
            min_edge_weight: 0.5,
            max_hops: 4,
        }),
        top_k: 10,
        weights: TsngWeights {
            vector: 0.6,
            property: 0.2,
            graph: 0.2,
        },
        budget: TsngBudget::default(),
    };
    let exact = db.tsng_ground_truth(&query).unwrap();
    let union = db
        .search_tsng_graph_union(
            &query,
            TsngSearchConfig {
                ef_search: 128,
                candidate_pool: 128,
                metadata_bonus_cap_ppm: 0,
                signal_queue_quota_ppm: 0,
                graph_seed_limit: NODES,
            },
        )
        .unwrap();
    assert_eq!(union.hits, exact.hits);
    assert!(union.metrics.graph_seeds_injected > 0);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_纯向量忽略双队列与图通道配置() {
    let (path, mut db) = database("pure_vector_dual_guard", true);
    let query_vector = vector(73);
    let query = pure_query(&query_vector, 10);
    let baseline = db
        .search_tsng(&query, TsngSearchConfig::for_top_k(10))
        .unwrap();
    let experimental = db
        .search_tsng(
            &query,
            TsngSearchConfig {
                ef_search: 160,
                candidate_pool: 160,
                metadata_bonus_cap_ppm: 500_000,
                signal_queue_quota_ppm: 500_000,
                graph_seed_limit: 64,
            },
        )
        .unwrap();
    let same_config_baseline = db
        .search_tsng(
            &query,
            TsngSearchConfig {
                ef_search: 160,
                candidate_pool: 160,
                metadata_bonus_cap_ppm: 0,
                signal_queue_quota_ppm: 0,
                graph_seed_limit: 0,
            },
        )
        .unwrap();
    assert_eq!(experimental, same_config_baseline);
    assert_eq!(experimental.metrics.navigation_scores, 0);
    assert_eq!(experimental.metrics.graph_seeds_injected, 0);
    assert!(!baseline.hits.is_empty());
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_非法导航配置明确拒绝() {
    let (path, mut db) = database("invalid_config", true);
    let query_vector = vector(3);
    let query = pure_query(&query_vector, 10);
    assert!(
        db.search_tsng(
            &query,
            TsngSearchConfig {
                ef_search: 0,
                candidate_pool: 10,
                metadata_bonus_cap_ppm: 200_000,
                signal_queue_quota_ppm: 0,
                graph_seed_limit: 0,
            },
        )
        .is_err()
    );
    assert!(
        db.search_tsng(
            &query,
            TsngSearchConfig {
                ef_search: 10,
                candidate_pool: 0,
                metadata_bonus_cap_ppm: 200_000,
                signal_queue_quota_ppm: 0,
                graph_seed_limit: 0,
            },
        )
        .is_err()
    );
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_属性候选少时自动_property_first_且与精确答案一致() {
    let (path, mut db) = database("industrial_property_first", true);
    db.create_index("tenant").unwrap();
    let query_vector = vector(41);
    let filter = Filter::from_json(&json!({"tenant": 1})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.8,
            property: 0.2,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let exact = db.tsng_ground_truth(&query).unwrap();
    let mut config = IndustrialSearchConfig::for_top_k(10);
    config.direct_rerank_bytes = 100 * DIM * std::mem::size_of::<f32>();
    let output = db.search_tsng_industrial(&query, config).unwrap();
    assert_eq!(
        output.metrics.access_path,
        IndustrialAccessPath::PropertyFirst
    );
    assert_eq!(output.hits, exact.hits);
    assert!(output.metrics.candidate_peak <= 1_000);
    assert!(output.metrics.estimated_temp_bytes > 0);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_中等选择性属性自动_bq_prefilter_且保持精确语义() {
    let (path, mut db) = database("industrial_property_filtered_ann", true);
    db.create_index("active").unwrap();
    let query_vector = vector(41);
    let filter = Filter::from_json(&json!({"active": true})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.8,
            property: 0.2,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let exact = db.tsng_ground_truth(&query).unwrap();
    let mut config = IndustrialSearchConfig::for_top_k(10);
    config.direct_rerank_bytes = 100 * DIM * std::mem::size_of::<f32>();
    let output = db.search_tsng_industrial(&query, config).unwrap();
    assert_eq!(
        output.metrics.access_path,
        IndustrialAccessPath::PropertyVectorUnion
    );
    assert_eq!(output.hits, exact.hits);
    assert!(output.metrics.candidate_peak < NODES);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_密度感知_beam_暴露三方模式且不改变硬过滤语义() {
    let (path, mut db) = database("density_adaptive_beam", true);
    db.create_index("active").unwrap();
    let query_vector = vector(41);
    let filter = Filter::from_json(&json!({"active": true})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.8,
            property: 0.2,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let run = |mode| {
        let mut config = IndustrialSearchConfig::for_top_k(10);
        config.direct_rerank_bytes = 10 * DIM * std::mem::size_of::<f32>();
        config.beam_adaptation = mode;
        db.search_tsng_industrial(&query, config).unwrap()
    };
    let fixed = run(BeamAdaptation::Fixed);
    let selective = run(BeamAdaptation::Selectivity);
    let density = run(BeamAdaptation::SelectivityAndDensity);
    let density_repeat = run(BeamAdaptation::SelectivityAndDensity);
    assert_eq!(fixed.metrics.adaptive_ef_search, 160);
    assert_eq!(density.hits, density_repeat.hits);
    assert_eq!(
        density.metrics.access_path,
        density_repeat.metrics.access_path
    );
    assert_eq!(
        density.metrics.adaptive_ef_search,
        density_repeat.metrics.adaptive_ef_search
    );
    assert_eq!(
        density.metrics.vector_density_skew_ppm,
        density_repeat.metrics.vector_density_skew_ppm
    );
    assert_eq!(
        density.metrics.candidates_reranked,
        density_repeat.metrics.candidates_reranked
    );
    assert_eq!(
        IndustrialSearchConfig::for_top_k(10).beam_adaptation,
        BeamAdaptation::Selectivity
    );
    assert!(selective.metrics.adaptive_ef_search >= fixed.metrics.adaptive_ef_search);
    assert!(density.metrics.adaptive_ef_search >= fixed.metrics.adaptive_ef_search);
    assert!(density.metrics.vector_density_skew_ppm > 0);
    assert!(fixed.hits.iter().all(|hit| hit.property_signal == 1.0));
    assert!(selective.hits.iter().all(|hit| hit.property_signal == 1.0));
    assert!(density.hits.iter().all(|hit| hit.property_signal == 1.0));
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_图信号自动_graph_vector_union_并保持候选有界() {
    let (path, mut db) = database("industrial_graph_union", true);
    let query_vector = vector(200);
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: None,
        graph: Some(GraphSignalQuery {
            anchor_id: 1,
            direction: ReachabilityDirection::Outgoing,
            labels: Some(vec!["related".into()]),
            min_edge_weight: 0.5,
            max_hops: 4,
        }),
        top_k: 10,
        weights: TsngWeights {
            vector: 0.7,
            property: 0.0,
            graph: 0.3,
        },
        budget: TsngBudget::default(),
    };
    let output = db
        .search_tsng_industrial(&query, IndustrialSearchConfig::for_top_k(10))
        .unwrap();
    assert_eq!(
        output.metrics.access_path,
        IndustrialAccessPath::GraphVectorUnion
    );
    assert!(output.metrics.candidate_peak <= QueryMemoryBudget::default().max_union_ids());
    assert!(output.metrics.estimated_graph_page_reads > 0);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_候选与页读取预算在分配前拒绝且不产生_spill() {
    let (path, mut db) = database("industrial_budget", true);
    db.create_index("tenant").unwrap();
    let query_vector = vector(8);
    let filter = Filter::from_json(&json!({"tenant": 0})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.8,
            property: 0.2,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let mut config = IndustrialSearchConfig::for_top_k(10);
    config.memory.max_candidate_id_bytes = 8 * std::mem::size_of::<u64>();
    config.memory.max_rerank_vector_bytes = 8 * DIM * std::mem::size_of::<f32>();
    assert!(db.search_tsng_industrial(&query, config).is_err());
    assert_eq!(db.storage_write_stats().temporary_spill_bytes, 0);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_无索引属性安全回退_ann_post_filter_且结果只含硬过滤命中() {
    let (path, mut db) = database("industrial_unindexed_fallback", true);
    let query_vector = vector(57);
    let filter = Filter::from_json(&json!({"active": true})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.7,
            property: 0.3,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let output = db
        .search_tsng_industrial(&query, IndustrialSearchConfig::for_top_k(10))
        .unwrap();
    assert_eq!(
        output.metrics.access_path,
        IndustrialAccessPath::AnnPostFilter
    );
    assert!(output.hits.iter().all(|hit| hit.property_signal == 1.0));
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_索引_and_交集自动_property_first_且与精确答案一致() {
    let (path, mut db) = database("industrial_property_intersection", true);
    db.create_index("tenant").unwrap();
    db.create_index("active").unwrap();
    let query_vector = vector(99);
    let filter = Filter::from_json(&json!({"tenant": 2, "active": true})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.7,
            property: 0.3,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let exact = db.tsng_ground_truth(&query).unwrap();
    let output = db
        .search_tsng_industrial(&query, IndustrialSearchConfig::for_top_k(10))
        .unwrap();
    assert_eq!(
        output.metrics.access_path,
        IndustrialAccessPath::PropertyFirst
    );
    assert_eq!(output.hits, exact.hits);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_非法工业阈值明确拒绝且不写盘() {
    let (path, mut db) = database("industrial_invalid_threshold", true);
    let query_vector = vector(1);
    let query = pure_query(&query_vector, 10);
    let before = db.storage_write_stats();
    let mut config = IndustrialSearchConfig::for_top_k(10);
    config.direct_rerank_bytes = 100 * DIM * std::mem::size_of::<f32>();
    config.union_rerank_bytes = 10 * DIM * std::mem::size_of::<f32>();
    assert!(db.search_tsng_industrial(&query, config).is_err());
    assert_eq!(db.storage_write_stats(), before);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_1_bq_预筛只读属性候选并保持结果硬过滤() {
    let (path, mut db) = database("bq_prefilter", true);
    db.create_index("active").unwrap();
    let query_vector = vector(41);
    let filter = Filter::from_json(&json!({"active": true})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.8,
            property: 0.2,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let before = db.storage_write_stats();
    let first = db
        .search_tsng_bq_prefilter(&query, IndustrialSearchConfig::for_top_k(10))
        .unwrap();
    let output = db
        .search_tsng_bq_prefilter(&query, IndustrialSearchConfig::for_top_k(10))
        .unwrap();
    assert_eq!(first.hits, output.hits);
    assert_eq!(first.metrics.bq_slot_cache_misses, 1);
    assert_eq!(output.metrics.bq_slot_cache_hits, 1);
    assert_eq!(output.metrics.bq_slot_cache_misses, 0);
    assert!(output.metrics.bq_mapped_candidates >= output.metrics.candidates_reranked);
    assert_eq!(
        output.metrics.access_path,
        IndustrialAccessPath::PropertyVectorUnion
    );
    assert!(output.hits.iter().all(|hit| hit.property_signal == 1.0));
    assert!(output.metrics.navigation_scores >= output.metrics.candidates_reranked);
    assert_eq!(db.storage_write_stats(), before);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_1_属性更新推进身份并令_internal_slot_缓存失效() {
    let (path, mut db) = database("property_epoch", true);
    db.create_index("active").unwrap();
    let query_vector = vector(41);
    let filter = Filter::from_json(&json!({"active": true})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.8,
            property: 0.2,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let config = IndustrialSearchConfig::for_top_k(10);
    let first = db.search_tsng_bq_prefilter(&query, config).unwrap();
    let warm = db.search_tsng_bq_prefilter(&query, config).unwrap();
    assert_eq!(first.metrics.bq_slot_cache_misses, 1);
    assert_eq!(warm.metrics.bq_slot_cache_hits, 1);

    db.update_payload(2, json!({"tenant": 1, "active": false}))
        .unwrap();
    let changed = db.search_tsng_bq_prefilter(&query, config).unwrap();
    assert_eq!(changed.metrics.bq_slot_cache_misses, 1);
    assert!(changed.hits.iter().all(|hit| hit.id != 2));
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_1_bq_预筛在无索引时安全回退工业路径() {
    let (path, mut db) = database("bq_prefilter_fallback", true);
    let query_vector = vector(41);
    let filter = Filter::from_json(&json!({"active": true})).unwrap();
    let query = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.8,
            property: 0.2,
            graph: 0.0,
        },
        budget: TsngBudget::default(),
    };
    let output = db
        .search_tsng_bq_prefilter(&query, IndustrialSearchConfig::for_top_k(10))
        .unwrap();
    assert_eq!(
        output.metrics.access_path,
        IndustrialAccessPath::AnnPostFilter
    );
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_累计_wal_观测区分逻辑负载且查询零写入() {
    let (path, mut db) = database("industrial_cumulative_wal", true);
    let initial = db.storage_write_stats();
    let id = db
        .insert(&vector(99), json!({"tenant": "累计观测"}))
        .unwrap();
    let after_insert = db.storage_write_stats();
    assert!(after_insert.wal_bytes > initial.wal_bytes);
    assert!(after_insert.logical_bytes > initial.logical_bytes);
    assert!(after_insert.total_written_bytes() >= after_insert.wal_bytes);
    assert!(after_insert.write_amplification().is_finite());

    let query_vector = vector(99);
    let query = pure_query(&query_vector, 1);
    db.search_tsng_industrial(&query, IndustrialSearchConfig::for_top_k(1))
        .unwrap();
    assert_eq!(db.storage_write_stats(), after_insert);
    assert!(db.contains(id));
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c1_5_只读工业查询数据库文件零写且暴露内存统计() {
    let (path, mut db) = database("industrial_read_zero_write", true);
    db.create_index("tenant").unwrap();
    db.flush().unwrap();
    let before = db.storage_write_stats();
    let query_vector = vector(11);
    let query = pure_query(&query_vector, 10);
    for _ in 0..20 {
        db.search_tsng_industrial(&query, IndustrialSearchConfig::for_top_k(10))
            .unwrap();
    }
    let after = db.storage_write_stats();
    assert_eq!(before, after);
    assert_eq!(after.temporary_spill_bytes, 0);
    let memory = db.index_memory_stats();
    assert!(memory.hot_bytes > 0);
    assert!(memory.posting_entries >= NODES as u64);
    db.close().unwrap();
    cleanup(&path);
}
