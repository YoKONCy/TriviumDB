//! TSNG C0 精确 ground-truth 与统一成本模型基线。
//!
//! 该 runner 使用固定 seed 构造向量、属性和图信号，测量 `tsng_ground_truth` 的
//! P50/P95/P99、QPS 与逻辑成本。它是后续 C1/工业策略的正确性和成本参考，不代表
//! ANN 产品延迟。数据构建时间单独记录，查询 warmup 不进入延迟样本。
//!
//! 输出默认为 `target/bench-reports/tsng-c0.json`；具体节点数、查询数、warmup 和
//! 维度由文件下方的 `TDB_C0_*` 环境变量读取并写回报告。

use serde::Serialize;
use serde_json::json;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;
use triviumdb::graph::reachability::ReachabilityDirection;
use triviumdb::{Database, Filter, GraphSignalQuery, TsngBudget, TsngCost, TsngQuery, TsngWeights};

const SEED: u64 = 0xC0C1_5453_4E47_0001;

#[derive(Debug, Serialize)]
struct Config {
    nodes: usize,
    queries: usize,
    warmup: usize,
    dim: usize,
    seed: u64,
}

#[derive(Debug, Serialize)]
struct Scenario {
    name: &'static str,
    iterations: usize,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    qps: f64,
    result_items: usize,
    cost: TsngCost,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    package_version: &'static str,
    config: Config,
    build_seconds: f64,
    scenarios: Vec<Scenario>,
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn vector(id: usize, dim: usize) -> Vec<f32> {
    let mut output = (0..dim)
        .map(|axis| {
            let mixed = id
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

fn cleanup(path: &Path) {
    let base = path.to_string_lossy();
    for suffix in ["", ".wal", ".vec", ".lock", ".flush_ok", ".pidx"] {
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
    name: &'static str,
    db: &Database<f32>,
    query: &TsngQuery<'a, f32>,
    iterations: usize,
    warmup: usize,
) -> Scenario {
    for _ in 0..warmup {
        black_box(db.tsng_ground_truth(query).unwrap());
    }
    let mut latencies = Vec::with_capacity(iterations);
    let started = Instant::now();
    let mut result_items = 0;
    let mut cost = TsngCost::default();
    for _ in 0..iterations {
        let query_started = Instant::now();
        let output = black_box(db.tsng_ground_truth(query).unwrap());
        latencies.push(query_started.elapsed().as_secs_f64() * 1000.0);
        result_items += output.hits.len();
        cost = output.cost;
    }
    let elapsed = started.elapsed().as_secs_f64();
    latencies.sort_by(|left, right| left.total_cmp(right));
    Scenario {
        name,
        iterations,
        p50_ms: percentile(&latencies, 0.50),
        p95_ms: percentile(&latencies, 0.95),
        p99_ms: percentile(&latencies, 0.99),
        qps: iterations as f64 / elapsed.max(f64::EPSILON),
        result_items,
        cost,
    }
}

fn main() {
    let config = Config {
        nodes: env_usize("TRIVIUM_TSNG_NODES", 100_000).max(100),
        queries: env_usize("TRIVIUM_TSNG_QUERIES", 50).max(1),
        warmup: env_usize("TRIVIUM_TSNG_WARMUP", 5),
        dim: env_usize("TRIVIUM_TSNG_DIM", 64).clamp(1, 3072),
        seed: SEED,
    };
    let path = std::env::temp_dir().join(format!("triviumdb_tsng_c0_{}.tdb", std::process::id()));
    cleanup(&path);
    let started = Instant::now();
    let mut db = Database::<f32>::open(&path.to_string_lossy(), config.dim).unwrap();
    db.disable_auto_compaction();
    for index in 0..config.nodes {
        let id = index as u64 + 1;
        db.insert_with_id(
            id,
            &vector(index, config.dim),
            json!({
                "tenant": index % 100,
                "active": index % 5 != 0,
            }),
        )
        .unwrap();
    }
    for index in 1..config.nodes {
        let source = index as u64;
        let target = index as u64 + 1;
        db.link(source, target, "related", 1.0).unwrap();
        if index + 17 <= config.nodes {
            db.link(source, (index + 17) as u64, "related", 0.8)
                .unwrap();
        }
    }
    let build_seconds = started.elapsed().as_secs_f64();
    let query_vector = vector(config.nodes / 3, config.dim);
    let filter = Filter::from_json(&json!({"tenant": 7, "active": true})).unwrap();
    let graph = GraphSignalQuery {
        anchor_id: 1,
        direction: ReachabilityDirection::Outgoing,
        labels: Some(vec!["related".into()]),
        min_edge_weight: 0.5,
        max_hops: 3,
    };
    let budget = TsngBudget {
        max_candidates: config.nodes + 1,
        max_visited_nodes: config.nodes + 1,
        max_examined_edges: config.nodes.saturating_mul(4),
        max_frontier_size: config.nodes + 1,
    };
    let pure = TsngQuery {
        vector: &query_vector,
        payload_filter: None,
        graph: None,
        top_k: 10,
        weights: TsngWeights::default(),
        budget,
    };
    let property = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: None,
        top_k: 10,
        weights: TsngWeights {
            vector: 0.7,
            property: 0.3,
            graph: 0.0,
        },
        budget,
    };
    let graph_only = TsngQuery {
        vector: &query_vector,
        payload_filter: None,
        graph: Some(graph.clone()),
        top_k: 10,
        weights: TsngWeights {
            vector: 0.5,
            property: 0.0,
            graph: 0.5,
        },
        budget,
    };
    let tri = TsngQuery {
        vector: &query_vector,
        payload_filter: Some(&filter),
        graph: Some(graph),
        top_k: 10,
        weights: TsngWeights {
            vector: 0.5,
            property: 0.25,
            graph: 0.25,
        },
        budget,
    };
    let scenarios = vec![
        measure("exact_vector", &db, &pure, config.queries, config.warmup),
        measure(
            "exact_vector_property",
            &db,
            &property,
            config.queries,
            config.warmup,
        ),
        measure(
            "exact_vector_graph",
            &db,
            &graph_only,
            config.queries,
            config.warmup,
        ),
        measure(
            "exact_three_signal",
            &db,
            &tri,
            config.queries,
            config.warmup,
        ),
    ];
    let report = Report {
        schema_version: 1,
        package_version: env!("CARGO_PKG_VERSION"),
        config,
        build_seconds,
        scenarios,
    };
    let json = serde_json::to_string_pretty(&report).unwrap();
    println!("{json}");
    let output = std::env::var_os("TRIVIUM_TSNG_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/bench-reports/tsng-c0.json"));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&output, json).unwrap();
    println!(
        "C0 基准报告已写入 {} / C0 benchmark report written to {}",
        output.display(),
        output.display()
    );
    db.close().unwrap();
    cleanup(&path);
}
