use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;
use triviumdb::{Database, NodeId};

const DEFAULT_SEED: u64 = 0x5452_4956_4955_4DDB;
const FILE_SUFFIXES: &[&str] = &[
    "",
    ".vec",
    ".wal",
    ".lock",
    ".flush_ok",
    ".quiver",
    ".text",
    ".pidx",
    ".pidx.tmp",
];

#[derive(Debug, Clone, Serialize)]
struct BenchmarkConfig {
    nodes: usize,
    queries: usize,
    warmup_queries: usize,
    dim: usize,
    average_degree: usize,
    seed: u64,
    keep_database: bool,
}

#[derive(Debug, Serialize)]
struct ScenarioReport {
    track: &'static str,
    scenario: String,
    variant: String,
    iterations: usize,
    result_items: usize,
    elapsed_seconds: f64,
    qps: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    min_ms: f64,
    max_ms: f64,
    rss_bytes_before: u64,
    rss_bytes_after: u64,
    minor_faults_delta: u64,
    major_faults_delta: u64,
}

#[derive(Debug, Serialize)]
struct BaselineReport {
    schema_version: u32,
    package_version: &'static str,
    git_sha: String,
    operating_system: &'static str,
    architecture: &'static str,
    config: BenchmarkConfig,
    database_bytes: u64,
    build_seconds: f64,
    scenarios: Vec<ScenarioReport>,
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(default)
}

fn config_from_env() -> BenchmarkConfig {
    BenchmarkConfig {
        nodes: env_usize("TRIVIUM_BASELINE_NODES", 100_000).max(100),
        queries: env_usize("TRIVIUM_BASELINE_QUERIES", 100).max(1),
        warmup_queries: env_usize("TRIVIUM_BASELINE_WARMUP", 10),
        dim: env_usize("TRIVIUM_BASELINE_DIM", 8).clamp(1, 3072),
        average_degree: env_usize("TRIVIUM_BASELINE_DEGREE", 4).max(1),
        seed: env_u64("TRIVIUM_BASELINE_SEED", DEFAULT_SEED),
        keep_database: env_bool("TRIVIUM_BASELINE_KEEP_DB", false),
    }
}

fn cleanup_database(path: &Path) {
    let base = path.to_string_lossy();
    for suffix in FILE_SUFFIXES {
        fs::remove_file(format!("{base}{suffix}")).ok();
    }
}

fn database_bytes(path: &Path) -> u64 {
    let base = path.to_string_lossy();
    FILE_SUFFIXES
        .iter()
        .filter_map(|suffix| fs::metadata(format!("{base}{suffix}")).ok())
        .map(|metadata| metadata.len())
        .sum()
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    sorted
        .get(((sorted.len().saturating_sub(1)) as f64 * fraction) as usize)
        .copied()
        .unwrap_or_default()
}

fn measure<F>(
    track: &'static str,
    scenario: impl Into<String>,
    variant: impl Into<String>,
    iterations: usize,
    warmup_iterations: usize,
    mut operation: F,
) -> ScenarioReport
where
    F: FnMut(usize) -> usize,
{
    for iteration in 0..warmup_iterations {
        black_box(operation(iteration));
    }

    let before = triviumdb::observability::process_memory_snapshot().unwrap_or_default();
    let started = Instant::now();
    let mut latencies = Vec::with_capacity(iterations);
    let mut result_items = 0usize;
    for iteration in 0..iterations {
        let query_started = Instant::now();
        result_items = result_items.saturating_add(black_box(operation(iteration)));
        latencies.push(query_started.elapsed().as_secs_f64() * 1000.0);
    }
    let elapsed = started.elapsed();
    let after = triviumdb::observability::process_memory_snapshot().unwrap_or_default();
    latencies.sort_by(|left, right| left.total_cmp(right));

    ScenarioReport {
        track,
        scenario: scenario.into(),
        variant: variant.into(),
        iterations,
        result_items,
        elapsed_seconds: elapsed.as_secs_f64(),
        qps: iterations as f64 / elapsed.as_secs_f64().max(f64::EPSILON),
        p50_ms: percentile(&latencies, 0.50),
        p95_ms: percentile(&latencies, 0.95),
        p99_ms: percentile(&latencies, 0.99),
        min_ms: latencies.first().copied().unwrap_or_default(),
        max_ms: latencies.last().copied().unwrap_or_default(),
        rss_bytes_before: before.rss_bytes,
        rss_bytes_after: after.rss_bytes,
        minor_faults_delta: after.minor_faults.saturating_sub(before.minor_faults),
        major_faults_delta: after.major_faults.saturating_sub(before.major_faults),
    }
}

fn deterministic_vector(id: usize, dim: usize) -> Vec<f32> {
    (0..dim)
        .map(|axis| {
            let value = id.wrapping_mul(131).wrapping_add(axis.wrapping_mul(17)) % 1009;
            value as f32 / 1009.0 - 0.5
        })
        .collect()
}

fn build_database(path: &Path, config: &BenchmarkConfig) -> (Database<f32>, Vec<NodeId>, f64) {
    cleanup_database(path);
    let path_text = path.to_string_lossy();
    let started = Instant::now();
    let mut db = Database::<f32>::open(&path_text, config.dim).expect("创建基准数据库失败");
    db.disable_auto_compaction();
    let mut ids = Vec::with_capacity(config.nodes);

    for index in 0..config.nodes {
        let id = db
            .insert(
                &deterministic_vector(index, config.dim),
                json!({
                    "kind": format!("kind_{}", index % 5),
                    "tenant": format!("tenant_{}", index % 100),
                    "region": format!("region_{}", (index / 100) % 10),
                    "rare": format!("rare_{}", index % 10_000),
                    "sequence": index,
                }),
            )
            .expect("写入基准节点失败");
        ids.push(id);
    }

    let mut rng = StdRng::seed_from_u64(config.seed);
    for source_index in 0..config.nodes {
        for edge_index in 0..config.average_degree {
            let offset = 1 + edge_index * 97;
            let target_index = (source_index + offset) % config.nodes;
            db.link(
                ids[source_index],
                ids[target_index],
                if edge_index % 2 == 0 {
                    "uniform_a"
                } else {
                    "uniform_b"
                },
                1.0,
            )
            .expect("写入均匀图边失败");
        }

        let rank = source_index + 1;
        let hub_degree = if rank <= 4 {
            (config.average_degree * 16).min(config.nodes.saturating_sub(1))
        } else if rank <= 64 {
            (config.average_degree * 4).min(config.nodes.saturating_sub(1))
        } else {
            1
        };
        for _ in 0..hub_degree {
            let target_index = rng.gen_range(0..config.nodes);
            if target_index != source_index {
                db.link(ids[source_index], ids[target_index], "hub", 1.0)
                    .expect("写入高出度图边失败");
            }
        }
    }

    db.flush().expect("持久化基准数据库失败");
    (db, ids, started.elapsed().as_secs_f64())
}

fn run_track_a(
    db: &mut Database<f32>,
    config: &BenchmarkConfig,
    scenarios: &mut Vec<ScenarioReport>,
) {
    let query_cases = [
        (
            "eq_selectivity_20pct",
            "MATCH (a {kind: \"kind_0\"}) RETURN a LIMIT 100",
        ),
        (
            "eq_selectivity_1pct",
            "MATCH (a {tenant: \"tenant_0\"}) RETURN a LIMIT 100",
        ),
        (
            "eq_selectivity_0_01pct",
            "MATCH (a {rare: \"rare_0\"}) RETURN a LIMIT 100",
        ),
        (
            "eq_selectivity_zero",
            "MATCH (a {rare: \"missing\"}) RETURN a LIMIT 100",
        ),
        (
            "and_selectivity",
            "MATCH (a {tenant: \"tenant_0\", region: \"region_0\"}) RETURN a LIMIT 100",
        ),
    ];

    for (name, query) in query_cases {
        scenarios.push(measure(
            "A",
            name,
            "full_scan",
            config.queries,
            config.warmup_queries,
            |_| db.tql(black_box(query)).expect("执行无索引 TQL 失败").len(),
        ));
    }

    let index_started = Instant::now();
    for field in ["kind", "tenant", "region", "rare"] {
        db.create_index(field).expect("创建属性索引失败");
    }
    let index_elapsed = index_started.elapsed();
    scenarios.push(ScenarioReport {
        track: "A",
        scenario: "index_build_four_fields".to_owned(),
        variant: "current_hash_index".to_owned(),
        iterations: 1,
        result_items: config.nodes.saturating_mul(4),
        elapsed_seconds: index_elapsed.as_secs_f64(),
        qps: 1.0 / index_elapsed.as_secs_f64().max(f64::EPSILON),
        p50_ms: index_elapsed.as_secs_f64() * 1000.0,
        p95_ms: index_elapsed.as_secs_f64() * 1000.0,
        p99_ms: index_elapsed.as_secs_f64() * 1000.0,
        min_ms: index_elapsed.as_secs_f64() * 1000.0,
        max_ms: index_elapsed.as_secs_f64() * 1000.0,
        rss_bytes_before: 0,
        rss_bytes_after: 0,
        minor_faults_delta: 0,
        major_faults_delta: 0,
    });

    for (name, query) in query_cases {
        scenarios.push(measure(
            "A",
            name,
            "current_hash_index",
            config.queries,
            config.warmup_queries,
            |_| db.tql(black_box(query)).expect("执行索引 TQL 失败").len(),
        ));
    }
}

fn run_track_a2(
    db: &mut Database<f32>,
    config: &BenchmarkConfig,
    scenarios: &mut Vec<ScenarioReport>,
) {
    let queries = [
        (
            "range_gt_99pct_limit_100",
            "FIND {sequence: {$gt: 1000}} RETURN * LIMIT 100",
        ),
        (
            "range_gt_1pct",
            &format!(
                "FIND {{sequence: {{$gt: {}}}}} RETURN *",
                config.nodes.saturating_sub(config.nodes / 100)
            ),
        ),
        (
            "range_order_desc_limit_100",
            "FIND {sequence: {$gte: 0}} RETURN * ORDER BY _.sequence DESC LIMIT 100",
        ),
    ];
    for (name, query) in &queries {
        scenarios.push(measure(
            "A2",
            *name,
            "full_scan",
            config.queries,
            config.warmup_queries,
            |_| db.tql(black_box(query)).expect("执行范围全扫描失败").len(),
        ));
    }

    let started = Instant::now();
    db.create_ordered_index("sequence")
        .expect("创建有序属性索引失败");
    let elapsed = started.elapsed().as_secs_f64();
    scenarios.push(ScenarioReport {
        track: "A2",
        scenario: "ordered_index_build_sequence".to_owned(),
        variant: "art_ordered_index".to_owned(),
        iterations: 1,
        result_items: config.nodes,
        elapsed_seconds: elapsed,
        qps: 1.0 / elapsed.max(f64::EPSILON),
        p50_ms: elapsed * 1000.0,
        p95_ms: elapsed * 1000.0,
        p99_ms: elapsed * 1000.0,
        min_ms: elapsed * 1000.0,
        max_ms: elapsed * 1000.0,
        rss_bytes_before: 0,
        rss_bytes_after: 0,
        minor_faults_delta: 0,
        major_faults_delta: 0,
    });
    for (name, query) in &queries {
        scenarios.push(measure(
            "A2",
            *name,
            "art_ordered_index",
            config.queries,
            config.warmup_queries,
            |_| {
                db.tql(black_box(query))
                    .expect("执行有序索引查询失败")
                    .len()
            },
        ));
    }
}

fn run_track_b(
    db: &Database<f32>,
    ids: &[NodeId],
    config: &BenchmarkConfig,
    scenarios: &mut Vec<ScenarioReport>,
) {
    let anchor_count = ids.len().min(64);
    let labels = vec!["uniform_a".to_owned()];
    for depth in [1usize, 2, 5, 10] {
        scenarios.push(measure(
            "B",
            format!("neighbors_depth_{depth}"),
            "uniform_and_hub_edges",
            config.queries,
            config.warmup_queries,
            |iteration| db.neighbors(ids[iteration % anchor_count], depth).len(),
        ));
    }

    for depth in [1usize, 2, 5] {
        scenarios.push(measure(
            "B",
            format!("neighbors_labeled_depth_{depth}"),
            "uniform_a_only",
            config.queries,
            config.warmup_queries,
            |iteration| {
                db.neighbors_with_labels(ids[iteration % anchor_count], depth, Some(&labels))
                    .len()
            },
        ));
    }

    let shortest_pairs: Vec<(NodeId, NodeId)> = (0..anchor_count)
        .map(|index| {
            (
                ids[index],
                ids[(index + config.nodes / 2).min(ids.len() - 1)],
            )
        })
        .collect();
    scenarios.push(measure(
        "B2",
        "shortest_path_single_direction",
        "legacy_bfs_path_copy",
        config.queries,
        config.warmup_queries,
        |iteration| {
            let (source, target) = shortest_pairs[iteration % shortest_pairs.len()];
            db.shortest_path(source, target, 10, None)
                .map_or(0, |path| path.len())
        },
    ));
    let traversal_budget = triviumdb::graph::budget::TraversalBudget {
        max_visited_nodes: config.nodes.saturating_mul(2),
        max_examined_edges: config.nodes.saturating_mul(config.average_degree + 4),
        max_frontier_size: config.nodes,
        max_depth: 10,
        exhaustion_policy: triviumdb::graph::budget::BudgetExhaustionPolicy::Partial,
    };
    scenarios.push(measure(
        "B2",
        "shortest_path_bidirectional",
        "smaller_frontier_with_parent_map",
        config.queries,
        config.warmup_queries,
        |iteration| {
            let (source, target) = shortest_pairs[iteration % shortest_pairs.len()];
            db.shortest_path_bidirectional(source, target, None, &traversal_budget)
                .expect("执行双向 BFS 失败")
                .path
                .map_or(0, |path| path.len())
        },
    ));
    scenarios.push(measure(
        "B2",
        "graph_stats",
        "degree_and_label_histograms",
        config.queries,
        config.warmup_queries,
        |_| db.graph_stats().edge_count,
    ));

    let id_queries: Vec<String> = ids
        .iter()
        .take(anchor_count)
        .map(|id| format!("MATCH (a {{id: {id}}})-[]->(b) RETURN b"))
        .collect();
    scenarios.push(measure(
        "B",
        "tql_match_id_one_hop",
        "primary_key_anchor",
        config.queries,
        config.warmup_queries,
        |iteration| {
            db.tql(black_box(&id_queries[iteration % id_queries.len()]))
                .expect("执行主键起点 MATCH 失败")
                .len()
        },
    ));

    scenarios.push(measure(
        "B",
        "tql_match_property_one_hop",
        "indexed_rare_anchor",
        config.queries,
        config.warmup_queries,
        |_| {
            db.tql(black_box(
                "MATCH (a {rare: \"rare_0\"})-[]->(b) RETURN b LIMIT 100",
            ))
            .expect("执行属性起点 MATCH 失败")
            .len()
        },
    ));

    if config.nodes <= 10_000 {
        scenarios.push(measure(
            "B",
            "tql_match_selective_target_full_scan",
            "unindexed_target",
            config.queries,
            config.warmup_queries,
            |_| {
                db.tql(black_box(
                    "MATCH (a)-[]->(b {sequence: 0}) RETURN a, b LIMIT 100",
                ))
                .expect("执行未索引选择性终点 MATCH 失败")
                .len()
            },
        ));
    }

    scenarios.push(measure(
        "B",
        "tql_match_selective_target_one_hop",
        "planner_reversed_target",
        config.queries,
        config.warmup_queries,
        |_| {
            db.tql(black_box(
                "MATCH (a)-[]->(b {rare: \"rare_0\"}) RETURN a, b LIMIT 100",
            ))
            .expect("执行选择性终点 MATCH 失败")
            .len()
        },
    ));

    scenarios.push(measure(
        "B",
        "explain_analyze_selective_target",
        "planner_observability",
        config.queries,
        config.warmup_queries,
        |_| {
            db.tql(black_box(
                "EXPLAIN ANALYZE MATCH (a)-[]->(b {rare: \"rare_0\"}) RETURN a, b LIMIT 100",
            ))
            .expect("执行 EXPLAIN ANALYZE 失败")
            .len()
        },
    ));

    let multi_hop_queries: Vec<String> = ids
        .iter()
        .take(anchor_count)
        .map(|id| format!("MATCH (a {{id: {id}}})-[]->(b)-[]->(c) RETURN c LIMIT 100"))
        .collect();
    scenarios.push(measure(
        "B",
        "tql_match_id_two_hop",
        "primary_key_anchor",
        config.queries,
        config.warmup_queries,
        |iteration| {
            db.tql(black_box(
                &multi_hop_queries[iteration % multi_hop_queries.len()],
            ))
            .expect("执行两跳 MATCH 失败")
            .len()
        },
    ));
}

fn output_path() -> PathBuf {
    std::env::var_os("TRIVIUM_BASELINE_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/bench-reports/index-graph-baseline.json"))
}

fn main() {
    let config = config_from_env();
    let database_path = std::env::var_os("TRIVIUM_BASELINE_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("triviumdb_index_graph_baseline.tdb"));
    println!(
        "开始 Rust 索引与图基线：节点={}，查询={}，维度={}，平均度数={} / Starting Rust index and graph baseline: nodes={}, queries={}, dim={}, average_degree={}",
        config.nodes,
        config.queries,
        config.dim,
        config.average_degree,
        config.nodes,
        config.queries,
        config.dim,
        config.average_degree
    );

    let (mut db, ids, build_seconds) = build_database(&database_path, &config);
    let bytes = database_bytes(&database_path);
    let mut scenarios = Vec::new();
    run_track_a(&mut db, &config, &mut scenarios);
    run_track_a2(&mut db, &config, &mut scenarios);
    run_track_b(&db, &ids, &config, &mut scenarios);

    let report = BaselineReport {
        schema_version: 1,
        package_version: env!("CARGO_PKG_VERSION"),
        git_sha: std::env::var("TRIVIUM_BASELINE_GIT_SHA").unwrap_or_else(|_| "unknown".to_owned()),
        operating_system: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        config: config.clone(),
        database_bytes: bytes,
        build_seconds,
        scenarios,
    };
    let json = serde_json::to_string_pretty(&report).expect("序列化基准报告失败");
    let output = output_path();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).expect("创建基准报告目录失败");
    }
    fs::write(&output, &json).expect("写入基准报告失败");
    println!("{json}");
    println!(
        "基准报告已写入 {} / Benchmark report written to {}",
        output.display(),
        output.display()
    );

    drop(db);
    if !config.keep_database {
        cleanup_database(&database_path);
    }
}
