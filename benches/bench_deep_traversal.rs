//! 单源大前沿 BFS 吞吐与确定性基准。
//!
//! ## 复现矩阵
//! 默认构造 1M 节点，分别测试平均出度 4/8/16、深度 3/5/8、线程数
//! 1/2/4/8/16。可用 `TDB_TRAVERSAL_NODES` 缩放节点数，但跨机器比较时必须保持
//! 图规模一致。
//!
//! ## 数据模型与正确性 oracle
//! 目标节点由固定乘法散列生成，因此图在整个 NodeId 空间扩展且完全可复现。
//! 每个出度/深度组合的五种线程配置必须产生相同结果哈希，否则立即失败。
//!
//! ## 指标
//! `million_teps` 是每秒实际检查边数（百万），不等同于返回节点吞吐。图构建、线程池
//! 创建和 JSON 写入不纳入遍历计时。

#[path = "support/mod.rs"]
mod support;

use serde::Serialize;
use serde_json::json;
use std::time::Instant;
use triviumdb::graph::budget::BudgetExhaustionPolicy;
use triviumdb::graph::reachability::{
    ReachabilityConfig, ReachabilityDirection, traverse_compact, traverse_compact_parallel,
};
use triviumdb::storage::memtable::MemTable;

#[derive(Serialize)]
struct Point {
    /// 图规模（节点数）
    nodes: usize,
    /// 平均出度
    avg_degree: usize,
    /// 遍历跳数
    depth: usize,
    /// 命中节点数
    reached_nodes: usize,
    /// 实际遍历边数
    traversed_edges: usize,
    /// 使用线程数
    threads: usize,
    /// 结果序列哈希
    result_hash: u64,
    elapsed_ms: f64,
    /// 每秒遍历边数（百万）
    million_teps: f64,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    points: Vec<Point>,
}

/// 构造平均出度为 avg_degree 的确定性图。
/// 使用固定步长跳转而非随机数，保证结果可复现且跨运行可比。
fn build(nodes: usize, avg_degree: usize) -> MemTable<f32> {
    let mut mt = MemTable::new(4);
    for id in 1..=nodes as u64 {
        mt.insert_with_id(id, &[1.0, 0.0, 0.0, 0.0], json!({"group": id % 32}))
            .unwrap();
    }
    let total = nodes as u64;
    for id in 1..=total {
        for k in 0..avg_degree as u64 {
            // 乘法散列打散目标，形成真正的扩展图：邻居分布在整个 id 空间而非局部相邻，
            // 既保证每个节点出度恒定，又让多跳前沿按出度指数增长（贴近真实社交/知识图谱）。
            let scattered = (id
                .wrapping_mul(2_654_435_761)
                .wrapping_add(k.wrapping_mul(40_503)))
                % total;
            let target = scattered + 1;
            if target != id {
                mt.link(id, target, "related".into(), 0.9).unwrap();
            }
        }
    }
    mt
}

fn measure(
    mt: &MemTable<f32>,
    nodes: usize,
    avg_degree: usize,
    depth: usize,
    threads: usize,
) -> Point {
    let config = ReachabilityConfig {
        min_depth: 1,
        max_depth: depth,
        labels: None,
        direction: ReachabilityDirection::Outgoing,
        max_visited_nodes: nodes,
        max_results: nodes,
        max_edges: nodes.saturating_mul(avg_degree).saturating_mul(2),
        max_frontier_size: nodes,
        exhaustion_policy: BudgetExhaustionPolicy::Error,
    };
    let started = Instant::now();
    let output = if threads == 1 {
        traverse_compact(mt, 1, &config).unwrap()
    } else {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| traverse_compact_parallel(mt, 1, &config))
            .unwrap()
    };
    let elapsed = started.elapsed().as_secs_f64();
    let result_hash = output
        .results
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, hit| {
            hash.wrapping_mul(0x100000001b3) ^ hit.target_id ^ (hit.depth as u64).rotate_left(17)
        });
    Point {
        nodes,
        avg_degree,
        depth,
        reached_nodes: output.results.len(),
        traversed_edges: output.traversed_edges,
        threads,
        result_hash,
        elapsed_ms: elapsed * 1000.0,
        million_teps: output.traversed_edges as f64 / elapsed / 1_000_000.0,
    }
}

fn main() {
    let nodes = support::env_usize("TDB_TRAVERSAL_NODES", 1_000_000);
    let mut points = Vec::new();
    for avg_degree in [4usize, 8, 16] {
        let mt = build(nodes, avg_degree);
        for depth in [3usize, 5, 8] {
            for threads in [1usize, 2, 4, 8, 16] {
                points.push(measure(&mt, nodes, avg_degree, depth, threads));
            }
        }
    }
    for group in points.as_chunks::<5>().0 {
        let hashes = group
            .iter()
            .map(|point| point.result_hash)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(hashes.len(), 1, "不同线程数的 BFS 结果不一致");
    }
    for point in &points {
        println!(
            "节点 {} 出度 {} 深度 {} 线程 {}：命中 {} 节点，遍历 {} 边，{:.3} ms，{:.2} M TEPS",
            point.nodes,
            point.avg_degree,
            point.depth,
            point.threads,
            point.reached_nodes,
            point.traversed_edges,
            point.elapsed_ms,
            point.million_teps
        );
    }
    let path = support::write_json_report(
        format!("deep-traversal-{nodes}.json"),
        &Report {
            schema_version: 1,
            points,
        },
    );
    println!("深遍历报告已写入 {}", path.display());
}
