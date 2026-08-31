//! TSNG C1 matched-recall、多策略与工业搜索 Gate。
//!
//! ## 方法
//! 在同一固定 seed 数据集上比较 PostFilter、BQ prefilter、TSNG、GraphUnion 和
//! Industrial 五条执行路径。每个策略扫相同 ef_search/bonus/quota/graph-seed 参数，
//! 再在目标 Recall@10 下比较 P95、NDCG、重排候选、页读取、内存和写入增量。
//!
//! ## 复现约束
//! 随机数据由常量 `SEED` 唯一决定；所有选择性以 ppm 表达，避免浮点配置漂移。
//! warmup 不进入延迟样本。Gate 必须同时满足 matched-recall、内存预算和 SSD 零写，
//! 不能只依据最快单点。输出默认为 `target/bench-reports/tsng-c1-matched-recall.json`。

use serde::Serialize;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use triviumdb::graph::reachability::ReachabilityDirection;
use triviumdb::index::quiver::QuIVerConfig;
use triviumdb::{
    BeamAdaptation, Database, Filter, GraphSignalQuery, IndustrialSearchConfig, TsngBudget,
    TsngGroundTruth, TsngQuery, TsngSearchConfig, TsngSearchMetrics, TsngWeights, quality_metrics,
};

const SEED: u64 = 0xC1C1_4741_5445_0002;
const TOP_K: usize = 10;
const CLUSTERS: usize = 32;
const DEFAULT_PROPERTY_SELECTIVITY_PPM: usize = 50_000;
const DEFAULT_GRAPH_SELECTIVITY_PPM: usize = 50_000;

#[derive(Serialize)]
struct Config {
    nodes: usize,
    queries: usize,
    warmup: usize,
    dim: usize,
    seed: u64,
    clusters: usize,
    property_selectivity: f64,
    graph_selectivity: f64,
    target_recall: f64,
    ef_values: Vec<usize>,
    metadata_bonus_caps_ppm: Vec<u32>,
    signal_queue_quotas_ppm: Vec<u32>,
    graph_seed_limits: Vec<usize>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Method {
    PostFilter,
    BqPrefilter,
    Tsng,
    GraphUnion,
    IndustrialFixed,
    IndustrialSelectivity,
    IndustrialDensity,
}

#[derive(Clone, Copy)]
struct NavigationVariant {
    ef_search: usize,
    metadata_bonus_cap_ppm: u32,
    signal_queue_quota_ppm: u32,
    graph_seed_limit: usize,
}

struct SweepConfig<'a> {
    ef_values: &'a [usize],
    metadata_bonus_caps_ppm: &'a [u32],
    signal_queue_quotas_ppm: &'a [u32],
    graph_seed_limits: &'a [usize],
    warmup: usize,
    target_recall: f64,
}

#[derive(Serialize)]
struct CurvePoint {
    method: Method,
    ef_search: usize,
    metadata_bonus_cap_ppm: u32,
    signal_queue_quota_ppm: u32,
    graph_seed_limit: usize,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    mean_recall_at_10: f64,
    mean_ndcg_at_10: f64,
    mean_result_count: f64,
    mean_navigation_scores: f64,
    mean_candidates_reranked: f64,
    peak_candidates: usize,
    peak_temp_bytes: usize,
    estimated_vector_page_reads: usize,
    estimated_payload_page_reads: usize,
    estimated_graph_page_reads: usize,
    mean_bq_posting_lookup_ms: f64,
    mean_bq_node_mapping_ms: f64,
    mean_bq_heap_scan_ms: f64,
    mean_bq_output_sort_ms: f64,
    bq_slot_cache_hits: usize,
    bq_slot_cache_misses: usize,
    mean_bq_mapped_candidates: f64,
    mean_adaptive_ef_search: f64,
    mean_vector_density_skew_ppm: f64,
    selected_access_path: String,
    property_path_cost: u64,
    graph_path_cost: u64,
    filtered_ann_path_cost: u64,
    resident_heap_bytes: u64,
    mapped_bytes: u64,
    wal_bytes_delta: i64,
    sidecar_bytes_delta: i64,
    checkpoint_bytes_delta: i64,
    temporary_spill_bytes_delta: i64,
}

#[derive(Serialize)]
struct GateResult {
    target_recall: f64,
    fixed_ef: Option<usize>,
    selectivity_ef: Option<usize>,
    density_ef: Option<usize>,
    fixed_p95_ms: Option<f64>,
    selectivity_p95_ms: Option<f64>,
    density_p95_ms: Option<f64>,
    density_recall_delta: Option<f64>,
    density_p95_speedup: Option<f64>,
    density_page_read_reduction: Option<f64>,
    density_candidate_reduction: Option<f64>,
    quality_gate_passed: bool,
    density_work_gate_passed: bool,
    memory_gate_passed: bool,
    ssd_wear_gate_passed: bool,
    passed: bool,
    reason: String,
}

#[derive(Serialize)]
struct Scenario {
    name: &'static str,
    curves: Vec<CurvePoint>,
    gate: GateResult,
}

#[derive(Serialize)]
struct DatasetValidation {
    unique_query_clusters: usize,
    property_matches: usize,
    graph_edges: usize,
    exact_property_results_full: bool,
    exact_graph_results_full: bool,
    exact_three_signal_results_full: bool,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    package_version: &'static str,
    config: Config,
    build_seconds: f64,
    dataset_validation: DatasetValidation,
    scenarios: Vec<Scenario>,
}

struct QueryCase {
    vector: Vec<f32>,
    graph: GraphSignalQuery,
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn splitmix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn random_unit(seed: u64, dim: usize) -> Vec<f32> {
    let mut output = (0..dim)
        .map(|axis| {
            let bits = splitmix(seed ^ (axis as u64).wrapping_mul(0xD1B5_4A32_D192_ED03));
            (bits >> 40) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        })
        .collect::<Vec<_>>();
    normalize(&mut output);
    output
}

fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    for value in vector {
        *value /= norm.max(f32::EPSILON);
    }
}

fn selected_by_ppm(local: usize, ppm: usize, salt: u64) -> bool {
    let mixed = splitmix((local as u64) ^ salt);
    (mixed % 1_000_000) < ppm as u64
}

fn clustered_vector(
    index: usize,
    dim: usize,
    centroids: &[Vec<f32>],
    property_selectivity_ppm: usize,
) -> Vec<f32> {
    let cluster = index % centroids.len();
    let noise = random_unit(SEED ^ index as u64, dim);
    let relevant = selected_by_ppm(index / centroids.len(), property_selectivity_ppm, SEED);
    let signal = if relevant { 0.55 } else { 0.75 };
    let mut output = centroids[cluster]
        .iter()
        .zip(noise)
        .map(|(center, noise)| center + signal * noise)
        .collect::<Vec<_>>();
    normalize(&mut output);
    output
}

fn cleanup(path: &Path) {
    let base = path.to_string_lossy();
    for suffix in ["", ".wal", ".vec", ".lock", ".flush_ok", ".pidx", ".gidx"] {
        fs::remove_file(format!("{base}{suffix}")).ok();
    }
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    sorted
        .get(((sorted.len().saturating_sub(1)) as f64 * fraction) as usize)
        .copied()
        .unwrap_or_default()
}

fn measure<'a>(
    method: Method,
    db: &Database<f32>,
    queries: &[TsngQuery<'a, f32>],
    exact: &[TsngGroundTruth],
    variant: NavigationVariant,
    warmup: usize,
) -> CurvePoint {
    let config = TsngSearchConfig {
        ef_search: variant.ef_search,
        candidate_pool: variant.ef_search,
        metadata_bonus_cap_ppm: variant.metadata_bonus_cap_ppm,
        signal_queue_quota_ppm: variant.signal_queue_quota_ppm,
        graph_seed_limit: variant.graph_seed_limit,
    };
    let search = |query: &TsngQuery<'_, f32>| match method {
        Method::PostFilter => db.search_tsng_post_filter(query, config),
        Method::BqPrefilter => db.search_tsng_bq_prefilter(
            query,
            IndustrialSearchConfig {
                ann: config,
                ..IndustrialSearchConfig::for_top_k(query.top_k)
            },
        ),
        Method::Tsng => db.search_tsng(query, config),
        Method::GraphUnion => db.search_tsng_graph_union(query, config),
        Method::IndustrialFixed | Method::IndustrialSelectivity | Method::IndustrialDensity => {
            let beam_adaptation = match method {
                Method::IndustrialFixed => BeamAdaptation::Fixed,
                Method::IndustrialSelectivity => BeamAdaptation::Selectivity,
                Method::IndustrialDensity => BeamAdaptation::SelectivityAndDensity,
                _ => unreachable!(),
            };
            db.search_tsng_industrial(
                query,
                IndustrialSearchConfig {
                    ann: config,
                    direct_rerank_bytes: query
                        .top_k
                        .saturating_mul(query.vector.len())
                        .saturating_mul(std::mem::size_of::<f32>()),
                    beam_adaptation,
                    ..IndustrialSearchConfig::for_top_k(query.top_k)
                },
            )
        }
    };
    for query in queries.iter().take(warmup.min(queries.len())) {
        std::hint::black_box(search(query).unwrap());
    }
    let mut latencies = Vec::with_capacity(queries.len());
    let mut recall = 0.0;
    let mut ndcg = 0.0;
    let mut result_count = 0usize;
    let mut metrics = TsngSearchMetrics::default();
    let writes_before = db.storage_write_stats();
    for (query, truth) in queries.iter().zip(exact) {
        let started = Instant::now();
        let output = std::hint::black_box(search(query).unwrap());
        latencies.push(started.elapsed().as_secs_f64() * 1000.0);
        let ids = output.hits.iter().map(|hit| hit.id).collect::<Vec<_>>();
        let quality = quality_metrics(&truth.hits, &ids, TOP_K);
        recall += quality.recall_at_k;
        ndcg += quality.ndcg_at_k;
        result_count += output.hits.len();
        metrics.navigation_scores += output.metrics.navigation_scores;
        metrics.candidates_reranked += output.metrics.candidates_reranked;
        metrics.candidate_peak = metrics.candidate_peak.max(output.metrics.candidate_peak);
        metrics.estimated_temp_bytes = metrics
            .estimated_temp_bytes
            .max(output.metrics.estimated_temp_bytes);
        metrics.estimated_vector_page_reads += output.metrics.estimated_vector_page_reads;
        metrics.estimated_payload_page_reads += output.metrics.estimated_payload_page_reads;
        metrics.estimated_graph_page_reads += output.metrics.estimated_graph_page_reads;
        metrics.bq_posting_lookup_ns += output.metrics.bq_posting_lookup_ns;
        metrics.bq_node_mapping_ns += output.metrics.bq_node_mapping_ns;
        metrics.bq_heap_scan_ns += output.metrics.bq_heap_scan_ns;
        metrics.bq_output_sort_ns += output.metrics.bq_output_sort_ns;
        metrics.bq_slot_cache_hits += output.metrics.bq_slot_cache_hits;
        metrics.bq_slot_cache_misses += output.metrics.bq_slot_cache_misses;
        metrics.bq_mapped_candidates += output.metrics.bq_mapped_candidates;
        metrics.adaptive_ef_search += output.metrics.adaptive_ef_search;
        metrics.vector_density_skew_ppm += output.metrics.vector_density_skew_ppm;
        metrics.access_path = output.metrics.access_path;
        metrics.property_path_cost = output.metrics.property_path_cost;
        metrics.graph_path_cost = output.metrics.graph_path_cost;
        metrics.filtered_ann_path_cost = output.metrics.filtered_ann_path_cost;
    }
    let writes_after = db.storage_write_stats();
    let memory = db.index_memory_stats();
    let delta = |after: u64, before: u64| after as i64 - before as i64;
    latencies.sort_by(|left, right| left.total_cmp(right));
    let count = queries.len() as f64;
    CurvePoint {
        method,
        ef_search: variant.ef_search,
        metadata_bonus_cap_ppm: variant.metadata_bonus_cap_ppm,
        signal_queue_quota_ppm: variant.signal_queue_quota_ppm,
        graph_seed_limit: variant.graph_seed_limit,
        p50_ms: percentile(&latencies, 0.50),
        p95_ms: percentile(&latencies, 0.95),
        p99_ms: percentile(&latencies, 0.99),
        mean_recall_at_10: recall / count,
        mean_ndcg_at_10: ndcg / count,
        mean_result_count: result_count as f64 / count,
        mean_navigation_scores: metrics.navigation_scores as f64 / count,
        mean_candidates_reranked: metrics.candidates_reranked as f64 / count,
        peak_candidates: metrics.candidate_peak,
        peak_temp_bytes: metrics.estimated_temp_bytes,
        estimated_vector_page_reads: metrics.estimated_vector_page_reads,
        estimated_payload_page_reads: metrics.estimated_payload_page_reads,
        estimated_graph_page_reads: metrics.estimated_graph_page_reads,
        mean_bq_posting_lookup_ms: metrics.bq_posting_lookup_ns as f64 / count / 1_000_000.0,
        mean_bq_node_mapping_ms: metrics.bq_node_mapping_ns as f64 / count / 1_000_000.0,
        mean_bq_heap_scan_ms: metrics.bq_heap_scan_ns as f64 / count / 1_000_000.0,
        mean_bq_output_sort_ms: metrics.bq_output_sort_ns as f64 / count / 1_000_000.0,
        bq_slot_cache_hits: metrics.bq_slot_cache_hits,
        bq_slot_cache_misses: metrics.bq_slot_cache_misses,
        mean_bq_mapped_candidates: metrics.bq_mapped_candidates as f64 / count,
        mean_adaptive_ef_search: metrics.adaptive_ef_search as f64 / count,
        mean_vector_density_skew_ppm: metrics.vector_density_skew_ppm as f64 / count,
        selected_access_path: format!("{:?}", metrics.access_path),
        property_path_cost: metrics.property_path_cost,
        graph_path_cost: metrics.graph_path_cost,
        filtered_ann_path_cost: metrics.filtered_ann_path_cost,
        resident_heap_bytes: memory.resident_heap_bytes,
        mapped_bytes: memory.mapped_bytes,
        wal_bytes_delta: delta(writes_after.wal_bytes, writes_before.wal_bytes),
        sidecar_bytes_delta: delta(writes_after.sidecar_bytes, writes_before.sidecar_bytes),
        checkpoint_bytes_delta: delta(
            writes_after.checkpoint_bytes,
            writes_before.checkpoint_bytes,
        ),
        temporary_spill_bytes_delta: delta(
            writes_after.temporary_spill_bytes,
            writes_before.temporary_spill_bytes,
        ),
    }
}

fn evaluate_gate(curves: &[CurvePoint], target_recall: f64) -> GateResult {
    let best = |method: Method| {
        curves
            .iter()
            .filter(|point| {
                std::mem::discriminant(&point.method) == std::mem::discriminant(&method)
                    && point.mean_recall_at_10 >= target_recall
                    && point.mean_result_count >= TOP_K as f64
            })
            .min_by(|left, right| {
                left.ef_search
                    .cmp(&right.ef_search)
                    .then_with(|| left.p95_ms.total_cmp(&right.p95_ms))
            })
    };
    let fixed = best(Method::IndustrialFixed);
    let selectivity = best(Method::IndustrialSelectivity);
    let density = best(Method::IndustrialDensity);
    let density_recall_delta = selectivity
        .zip(density)
        .map(|(selectivity, density)| density.mean_recall_at_10 - selectivity.mean_recall_at_10);
    let density_p95_speedup = selectivity
        .zip(density)
        .map(|(selectivity, density)| selectivity.p95_ms / density.p95_ms.max(f64::EPSILON));
    let density_page_read_reduction = selectivity.zip(density).map(|(selectivity, density)| {
        1.0 - density.estimated_vector_page_reads as f64
            / selectivity.estimated_vector_page_reads.max(1) as f64
    });
    let density_candidate_reduction = selectivity.zip(density).map(|(selectivity, density)| {
        1.0 - density.mean_candidates_reranked / selectivity.mean_candidates_reranked.max(1.0)
    });
    let quality_gate_passed = fixed.is_some()
        && selectivity.is_some()
        && density.is_some()
        && density_recall_delta.is_some_and(|delta| delta >= -f64::EPSILON);
    let density_work_gate_passed = density_p95_speedup.is_some_and(|speedup| speedup >= 1.05)
        || density_page_read_reduction.is_some_and(|reduction| reduction >= 0.05)
        || density_candidate_reduction.is_some_and(|reduction| reduction >= 0.05);
    let memory_gate_passed = density.is_some_and(|point| {
        point.peak_candidates > 0
            && point.peak_temp_bytes > 0
            && point.temporary_spill_bytes_delta == 0
    });
    let ssd_wear_gate_passed = density.is_some_and(|point| {
        point.wal_bytes_delta == 0
            && point.sidecar_bytes_delta == 0
            && point.checkpoint_bytes_delta == 0
            && point.temporary_spill_bytes_delta == 0
    });
    let passed = quality_gate_passed
        && density_work_gate_passed
        && memory_gate_passed
        && ssd_wear_gate_passed;
    let reason = if fixed.is_none() || selectivity.is_none() || density.is_none() {
        "至少一种 beam 模式未达到目标 Recall".to_owned()
    } else if !quality_gate_passed {
        "密度感知模式相对选择性感知模式发生 Recall 回退".to_owned()
    } else if !memory_gate_passed {
        "密度感知模式未通过有界内存或零 spill Gate".to_owned()
    } else if !ssd_wear_gate_passed {
        "密度感知模式未通过只读零写入 Gate".to_owned()
    } else if !density_work_gate_passed {
        "密度感知模式未降低 P95、向量页读取或候选精排工作量".to_owned()
    } else {
        "三方达到目标 Recall，密度感知无质量回退并降低至少一项工作量".to_owned()
    };
    GateResult {
        target_recall,
        fixed_ef: fixed.map(|point| point.ef_search),
        selectivity_ef: selectivity.map(|point| point.ef_search),
        density_ef: density.map(|point| point.ef_search),
        fixed_p95_ms: fixed.map(|point| point.p95_ms),
        selectivity_p95_ms: selectivity.map(|point| point.p95_ms),
        density_p95_ms: density.map(|point| point.p95_ms),
        density_recall_delta,
        density_p95_speedup,
        density_page_read_reduction,
        density_candidate_reduction,
        quality_gate_passed,
        density_work_gate_passed,
        memory_gate_passed,
        ssd_wear_gate_passed,
        passed,
        reason,
    }
}

fn scenario<'a>(
    name: &'static str,
    db: &Database<f32>,
    queries: Vec<TsngQuery<'a, f32>>,
    sweep: SweepConfig<'_>,
) -> Scenario {
    let exact = queries
        .iter()
        .map(|query| db.tsng_ground_truth(query).unwrap())
        .collect::<Vec<_>>();
    let combinations = sweep
        .metadata_bonus_caps_ppm
        .len()
        .saturating_mul(sweep.signal_queue_quotas_ppm.len())
        .saturating_mul(sweep.graph_seed_limits.len());
    let mut curves = Vec::with_capacity(sweep.ef_values.len() * (combinations + 1));
    for &ef in sweep.ef_values {
        let base_variant = NavigationVariant {
            ef_search: ef,
            metadata_bonus_cap_ppm: 0,
            signal_queue_quota_ppm: 0,
            graph_seed_limit: 0,
        };
        curves.push(measure(
            Method::IndustrialFixed,
            db,
            &queries,
            &exact,
            base_variant,
            sweep.warmup,
        ));
        curves.push(measure(
            Method::IndustrialSelectivity,
            db,
            &queries,
            &exact,
            base_variant,
            sweep.warmup,
        ));
        curves.push(measure(
            Method::IndustrialDensity,
            db,
            &queries,
            &exact,
            base_variant,
            sweep.warmup,
        ));
        curves.push(measure(
            Method::PostFilter,
            db,
            &queries,
            &exact,
            base_variant,
            sweep.warmup,
        ));
        if name == "vector_property" {
            curves.push(measure(
                Method::BqPrefilter,
                db,
                &queries,
                &exact,
                base_variant,
                sweep.warmup,
            ));
        }
        for &cap in sweep.metadata_bonus_caps_ppm {
            for &quota in sweep.signal_queue_quotas_ppm {
                for &seed_limit in sweep.graph_seed_limits {
                    let variant = NavigationVariant {
                        ef_search: ef,
                        metadata_bonus_cap_ppm: cap,
                        signal_queue_quota_ppm: quota,
                        graph_seed_limit: seed_limit,
                    };
                    curves.push(measure(
                        Method::Tsng,
                        db,
                        &queries,
                        &exact,
                        variant,
                        sweep.warmup,
                    ));
                    if seed_limit > 0 && quota == 0 {
                        curves.push(measure(
                            Method::GraphUnion,
                            db,
                            &queries,
                            &exact,
                            variant,
                            sweep.warmup,
                        ));
                    }
                }
            }
        }
    }
    let gate = evaluate_gate(&curves, sweep.target_recall);
    Scenario { name, curves, gate }
}

fn main() {
    let property_selectivity_ppm = env_usize(
        "TRIVIUM_TSNG_PROPERTY_SELECTIVITY_PPM",
        DEFAULT_PROPERTY_SELECTIVITY_PPM,
    )
    .clamp(1, 1_000_000);
    let graph_selectivity_ppm = env_usize(
        "TRIVIUM_TSNG_GRAPH_SELECTIVITY_PPM",
        DEFAULT_GRAPH_SELECTIVITY_PPM,
    )
    .clamp(1, 1_000_000);
    let nodes = env_usize("TRIVIUM_TSNG_NODES", 100_000).max(CLUSTERS * 20);
    let query_count = env_usize("TRIVIUM_TSNG_QUERIES", 50).clamp(1, CLUSTERS);
    let warmup = env_usize("TRIVIUM_TSNG_WARMUP", 3);
    let dim = env_usize("TRIVIUM_TSNG_DIM", 64).clamp(8, 3072);
    let target_recall = env_f64("TRIVIUM_TSNG_TARGET_RECALL", 0.80).clamp(0.0, 1.0);
    let ef_values = std::env::var("TRIVIUM_TSNG_EF_VALUES")
        .ok()
        .map(|values| {
            values
                .split(',')
                .filter_map(|value| value.trim().parse::<usize>().ok())
                .filter(|value| *value >= TOP_K)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| vec![64, 128, 256, 512, 1024, 2048, 4096]);
    let metadata_bonus_caps_ppm = std::env::var("TRIVIUM_TSNG_BONUS_CAPS_PPM")
        .ok()
        .map(|values| {
            values
                .split(',')
                .filter_map(|value| value.trim().parse::<u32>().ok())
                .filter(|value| *value <= 1_000_000)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| vec![100_000]);
    let signal_queue_quotas_ppm = std::env::var("TRIVIUM_TSNG_SIGNAL_QUOTAS_PPM")
        .ok()
        .map(|values| {
            values
                .split(',')
                .filter_map(|value| value.trim().parse::<u32>().ok())
                .filter(|value| *value <= 1_000_000)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| vec![0, 100_000, 200_000, 250_000, 330_000, 500_000]);
    let graph_seed_limits = std::env::var("TRIVIUM_TSNG_GRAPH_SEED_LIMITS")
        .ok()
        .map(|values| {
            values
                .split(',')
                .filter_map(|value| value.trim().parse::<usize>().ok())
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| vec![0, 16, 64, 256]);
    let centroids = (0..CLUSTERS)
        .map(|cluster| random_unit(SEED ^ (cluster as u64).rotate_left(17), dim))
        .collect::<Vec<_>>();
    let path = std::env::temp_dir().join(format!("triviumdb_tsng_gate_{}.tdb", std::process::id()));
    cleanup(&path);
    let started = Instant::now();
    let mut db = Database::<f32>::open(&path.to_string_lossy(), dim).unwrap();
    db.disable_auto_compaction();
    let mut property_matches = 0usize;
    let mut graph_edges = 0usize;
    let mut anchors = vec![0u64; CLUSTERS];
    let mut relevant_by_cluster = vec![Vec::new(); CLUSTERS];
    for index in 0..nodes {
        let cluster = index % CLUSTERS;
        let local = index / CLUSTERS;
        let property_relevant = selected_by_ppm(local, property_selectivity_ppm, SEED);
        let graph_relevant =
            local < 2 || selected_by_ppm(local, graph_selectivity_ppm, SEED ^ 0x4747_5241_5048);
        let id = index as u64 + 1;
        if local == 0 {
            anchors[cluster] = id;
        }
        if property_relevant {
            property_matches += 1;
        }
        if graph_relevant {
            relevant_by_cluster[cluster].push(id);
        }
        db.insert_with_id(
            id,
            &clustered_vector(index, dim, &centroids, property_selectivity_ppm),
            json!({"eligible": property_relevant, "cluster": cluster}),
        )
        .unwrap();
    }
    for cluster in 0..CLUSTERS {
        let anchor = anchors[cluster];
        for &target in &relevant_by_cluster[cluster] {
            if target != anchor {
                db.link(anchor, target, "relevant", 1.0).unwrap();
                graph_edges += 1;
            }
        }
    }
    db.create_index("eligible").unwrap();
    db.build_quiver_index(Some(QuIVerConfig {
        m: 32,
        ef_construction: 128,
        alpha: 1.2,
    }))
    .unwrap();
    db.flush().unwrap();
    db.close().unwrap();
    let mut db = Database::<f32>::open_read_only(&path.to_string_lossy(), dim).unwrap();
    assert!(db.index_memory_stats().mapped_bytes > 0);
    let build_seconds = started.elapsed().as_secs_f64();
    let cases = (0..query_count)
        .map(|cluster| {
            let source =
                relevant_by_cluster[cluster][1.min(relevant_by_cluster[cluster].len() - 1)];
            let index = source as usize - 1;
            QueryCase {
                vector: clustered_vector(index, dim, &centroids, property_selectivity_ppm),
                graph: GraphSignalQuery {
                    anchor_id: anchors[cluster],
                    direction: ReachabilityDirection::Outgoing,
                    labels: Some(vec!["relevant".into()]),
                    min_edge_weight: 0.5,
                    max_hops: 1,
                },
            }
        })
        .collect::<Vec<_>>();
    let filter = Filter::from_json(&json!({"eligible": true})).unwrap();
    let budget = TsngBudget {
        max_candidates: nodes + 1,
        max_visited_nodes: nodes + 1,
        max_examined_edges: nodes.saturating_mul(2),
        max_frontier_size: nodes + 1,
    };
    let make_queries = |weights: TsngWeights, property: bool, graph: bool| {
        cases
            .iter()
            .map(|case| TsngQuery {
                vector: &case.vector,
                payload_filter: property.then_some(&filter),
                graph: graph.then_some(case.graph.clone()),
                top_k: TOP_K,
                weights,
                budget,
            })
            .collect::<Vec<_>>()
    };
    let property_queries = make_queries(
        TsngWeights {
            vector: 0.7,
            property: 0.3,
            graph: 0.0,
        },
        true,
        false,
    );
    let graph_queries = make_queries(
        TsngWeights {
            vector: 0.65,
            property: 0.0,
            graph: 0.35,
        },
        false,
        true,
    );
    let tri_queries = make_queries(
        TsngWeights {
            vector: 0.6,
            property: 0.2,
            graph: 0.2,
        },
        true,
        true,
    );
    let validate_full = |queries: &[TsngQuery<'_, f32>]| {
        queries.iter().all(|query| {
            let truth = db.tsng_ground_truth(query).unwrap();
            truth.hits.len() == TOP_K
        })
    };
    let dataset_validation = DatasetValidation {
        unique_query_clusters: query_count,
        property_matches,
        graph_edges,
        exact_property_results_full: validate_full(&property_queries),
        exact_graph_results_full: validate_full(&graph_queries),
        exact_three_signal_results_full: validate_full(&tri_queries),
    };
    assert!(dataset_validation.exact_property_results_full);
    assert!(dataset_validation.exact_graph_results_full);
    assert!(dataset_validation.exact_three_signal_results_full);
    let scenarios = vec![
        scenario(
            "vector_property",
            &db,
            property_queries,
            SweepConfig {
                ef_values: &ef_values,
                metadata_bonus_caps_ppm: &metadata_bonus_caps_ppm,
                signal_queue_quotas_ppm: &signal_queue_quotas_ppm,
                graph_seed_limits: &[0],
                warmup,
                target_recall,
            },
        ),
        scenario(
            "vector_graph",
            &db,
            graph_queries,
            SweepConfig {
                ef_values: &ef_values,
                metadata_bonus_caps_ppm: &metadata_bonus_caps_ppm,
                signal_queue_quotas_ppm: &signal_queue_quotas_ppm,
                graph_seed_limits: &graph_seed_limits,
                warmup,
                target_recall,
            },
        ),
        scenario(
            "three_signal",
            &db,
            tri_queries,
            SweepConfig {
                ef_values: &ef_values,
                metadata_bonus_caps_ppm: &metadata_bonus_caps_ppm,
                signal_queue_quotas_ppm: &signal_queue_quotas_ppm,
                graph_seed_limits: &graph_seed_limits,
                warmup,
                target_recall,
            },
        ),
    ];
    let report = Report {
        schema_version: 2,
        package_version: env!("CARGO_PKG_VERSION"),
        config: Config {
            nodes,
            queries: query_count,
            warmup,
            dim,
            seed: SEED,
            clusters: CLUSTERS,
            property_selectivity: property_matches as f64 / nodes as f64,
            graph_selectivity: graph_edges as f64 / nodes as f64,
            target_recall,
            ef_values,
            metadata_bonus_caps_ppm,
            signal_queue_quotas_ppm,
            graph_seed_limits,
        },
        build_seconds,
        dataset_validation,
        scenarios,
    };
    let output = std::env::var_os("TRIVIUM_TSNG_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/bench-reports/tsng-c1-matched-recall.json"));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let json = serde_json::to_string_pretty(&report).unwrap();
    fs::write(&output, &json).unwrap();
    println!("{json}");
    println!(
        "C1 同 Recall Gate 报告已写入 {} / C1 matched-recall Gate report written to {}",
        output.display(),
        output.display()
    );
    db.close().unwrap();
    cleanup(&path);
}
