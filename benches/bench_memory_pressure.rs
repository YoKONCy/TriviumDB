//! 内存压力场景下的检索吞吐、尾延迟与常驻内存报告。
//!
//! 该入口刻意使用可配置规模而非 Criterion 微基准，报告 mmap/heap 观测并验证预算不会
//! 在高压力下退化为 OOM；结果写入统一 bench-reports 目录供 CI 上传。

use serde::Serialize;
use std::time::{Duration, Instant};
use triviumdb::database::{Config, Database, SearchConfig, StorageMode};

#[derive(Serialize)]
struct Report {
    nodes: usize,
    dim: usize,
    queries: usize,
    elapsed_seconds: f64,
    qps: f64,
    p50_ms: f64,
    p99_ms: f64,
    rss_bytes_before: u64,
    rss_bytes_after: u64,
    major_faults_delta: u64,
    minor_faults_delta: u64,
    database_bytes: u64,
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let nodes = env_usize("TRIVIUM_PRESSURE_NODES", 20_000);
    let dim = env_usize("TRIVIUM_PRESSURE_DIM", 128).min(3072);
    let queries = env_usize("TRIVIUM_PRESSURE_QUERIES", 500);
    let duration_seconds = env_usize("TRIVIUM_PRESSURE_SECONDS", 15);
    let pressure_bytes = env_usize("TRIVIUM_PRESSURE_BYTES", 0);
    let pressure_pages = if pressure_bytes > 0 {
        let mut pages = vec![0u8; pressure_bytes];
        for offset in (0..pages.len()).step_by(4096) {
            pages[offset] = (offset / 4096) as u8;
        }
        Some(pages)
    } else {
        None
    };
    let path = std::env::temp_dir().join("triviumdb_memory_pressure_ci.tdb");
    let path_text = path.to_string_lossy().to_string();
    for suffix in ["", ".vec", ".wal", ".lock", ".flush_ok", ".quiver"] {
        std::fs::remove_file(format!("{path_text}{suffix}")).ok();
    }

    let mut db = Database::<f32>::open_with_config(
        &path_text,
        Config {
            dim,
            storage_mode: StorageMode::Mmap,
            auto_build_quiver: true,
            expected_nodes: Some(nodes),
            ..Default::default()
        },
    )
    .expect("创建观测数据库失败");
    for id in 0..nodes {
        let vector: Vec<f32> = (0..dim)
            .map(|axis| (((id.wrapping_mul(131) + axis * 17) % 1009) as f32 / 1009.0) - 0.5)
            .collect();
        db.insert(&vector, serde_json::Value::Null)
            .expect("写入观测向量失败");
    }
    db.flush().expect("持久化观测数据库失败");
    db.build_quiver_index(None).expect("构建观测索引失败");

    let config = SearchConfig {
        top_k: 10,
        recall_k: 20,
        rerank_k: 20,
        expand_depth: 0,
        min_score: -1.0,
        ..Default::default()
    };
    let before = triviumdb::observability::process_memory_snapshot().unwrap_or_default();
    std::hint::black_box(&pressure_pages);
    let started = Instant::now();
    let deadline = started + Duration::from_secs(duration_seconds as u64);
    let mut completed = 0usize;
    let mut latencies = Vec::with_capacity(queries);
    while completed < queries && Instant::now() < deadline {
        let query: Vec<f32> = (0..dim)
            .map(|axis| (((completed.wrapping_mul(313) + axis * 29) % 1013) as f32 / 1013.0) - 0.5)
            .collect();
        let query_started = Instant::now();
        let _ = db.search_advanced(&query, &config).expect("观测查询失败");
        latencies.push(query_started.elapsed().as_secs_f64() * 1000.0);
        completed += 1;
    }
    let elapsed = started.elapsed();
    let after = triviumdb::observability::process_memory_snapshot().unwrap_or_default();
    latencies.sort_by(|a, b| a.total_cmp(b));
    let percentile = |fraction: f64| {
        latencies
            .get(((latencies.len().saturating_sub(1)) as f64 * fraction) as usize)
            .copied()
            .unwrap_or_default()
    };
    let database_bytes = ["", ".vec", ".quiver"]
        .iter()
        .filter_map(|suffix| std::fs::metadata(format!("{path_text}{suffix}")).ok())
        .map(|metadata| metadata.len())
        .sum();
    let report = Report {
        nodes,
        dim,
        queries: completed,
        elapsed_seconds: elapsed.as_secs_f64(),
        qps: completed as f64 / elapsed.as_secs_f64().max(f64::EPSILON),
        p50_ms: percentile(0.50),
        p99_ms: percentile(0.99),
        rss_bytes_before: before.rss_bytes,
        rss_bytes_after: after.rss_bytes,
        major_faults_delta: after.major_faults.saturating_sub(before.major_faults),
        minor_faults_delta: after.minor_faults.saturating_sub(before.minor_faults),
        database_bytes,
    };
    let json = serde_json::to_string_pretty(&report).expect("序列化观测报告失败");
    println!("{json}");
    std::fs::create_dir_all("target/bench-reports").expect("创建观测报告目录失败");
    std::fs::write("target/bench-reports/memory-pressure-report.json", &json)
        .expect("写入观测报告失败");
}
