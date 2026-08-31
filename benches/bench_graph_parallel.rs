//! 图算法线程扩展与确定性基准。
//!
//! ## 覆盖范围
//! 在同一确定性 8 出度图上测量单源 BFS、PageRank、WCC、64 源采样
//! Betweenness 和同步标签传播，线程矩阵为 1/2/4/8/16。
//!
//! ## 正确性 oracle
//! 每个算法的所有线程配置必须产生相同位级结果哈希。Betweenness 固定采样前 64 个
//! source；标签传播使用固定轮次和稳定节点顺序，因此不存在随机种子漂移。
//!
//! ## 计时边界
//! 图、节点集合和 Rayon 线程池在各算法计时前构建；`elapsed_ms` 只包含算法调用。

#[path = "support/mod.rs"]
mod support;

use serde::Serialize;
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Instant;
use triviumdb::graph::budget::BudgetExhaustionPolicy;
use triviumdb::graph::reachability::{
    ReachabilityConfig, ReachabilityDirection, traverse_compact, traverse_compact_parallel,
};
use triviumdb::graph::subset::{
    LabelPropagationConfig, SubsetPageRankConfig, deterministic_label_propagation,
    deterministic_label_propagation_parallel, subset_betweenness, subset_betweenness_parallel,
    subset_pagerank, subset_pagerank_parallel, subset_wcc, subset_wcc_parallel,
};
use triviumdb::storage::memtable::MemTable;

#[derive(Serialize)]
struct Point {
    algorithm: &'static str,
    threads: usize,
    elapsed_ms: f64,
    result_hash: u64,
}

fn build(nodes: usize) -> MemTable<f32> {
    let mut mt = MemTable::new(4);
    for id in 1..=nodes as u64 {
        mt.insert_with_id(id, &[1.0, 0.0, 0.0, 0.0], json!({}))
            .unwrap();
    }
    for id in 1..=nodes as u64 {
        for offset in [1u64, 17, 257, 4099, 16381, 32749, 65521, 99991] {
            let target = (id + offset - 1) % nodes as u64 + 1;
            if target != id {
                mt.link(id, target, "edge".into(), 1.0).unwrap();
            }
        }
    }
    mt
}

fn mix(mut hash: u64, value: u64) -> u64 {
    hash = hash.wrapping_mul(0x100000001b3);
    hash ^ value
}

fn main() {
    let nodes = support::env_usize("TDB_GRAPH_PARALLEL_NODES", 100_000usize);
    let mt = build(nodes);
    let all = BTreeSet::from_iter(1..=nodes as u64);
    let bc_nodes = BTreeSet::from_iter(1..=5_000u64);
    let mut points = Vec::new();
    for threads in [1usize, 2, 4, 8, 16] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();

        let config = ReachabilityConfig {
            min_depth: 1,
            max_depth: 8,
            labels: None,
            direction: ReachabilityDirection::Outgoing,
            max_visited_nodes: nodes,
            max_results: nodes,
            max_edges: nodes * 16,
            max_frontier_size: nodes,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        };
        let started = Instant::now();
        let bfs = if threads == 1 {
            traverse_compact(&mt, 1, &config).unwrap()
        } else {
            pool.install(|| traverse_compact_parallel(&mt, 1, &config))
                .unwrap()
        };
        let hash = bfs
            .results
            .iter()
            .fold(0u64, |h, x| mix(mix(h, x.target_id), x.depth as u64));
        points.push(Point {
            algorithm: "single_source_bfs",
            threads,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            result_hash: hash,
        });

        let started = Instant::now();
        let pr = if threads == 1 {
            subset_pagerank(&mt, &all, SubsetPageRankConfig::default(), None, nodes * 16).unwrap()
        } else {
            pool.install(|| {
                subset_pagerank_parallel(
                    &mt,
                    &all,
                    SubsetPageRankConfig::default(),
                    None,
                    nodes * 16,
                )
            })
            .unwrap()
        };
        let hash = pr
            .scores
            .iter()
            .fold(0u64, |h, (id, score)| mix(mix(h, *id), score.to_bits()));
        points.push(Point {
            algorithm: "pagerank",
            threads,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            result_hash: hash,
        });

        let started = Instant::now();
        let wcc = if threads == 1 {
            subset_wcc(&mt, &all, None, nodes * 16).unwrap()
        } else {
            pool.install(|| subset_wcc_parallel(&mt, &all, None, nodes * 16))
                .unwrap()
        };
        let hash = wcc.0.iter().flatten().fold(0u64, |h, id| mix(h, *id));
        points.push(Point {
            algorithm: "wcc",
            threads,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            result_hash: hash,
        });

        let started = Instant::now();
        let bc = if threads == 1 {
            subset_betweenness(&mt, &bc_nodes, None, Some(64), 10_000_000).unwrap()
        } else {
            pool.install(|| subset_betweenness_parallel(&mt, &bc_nodes, None, Some(64), 10_000_000))
                .unwrap()
        };
        let hash = bc
            .scores
            .iter()
            .fold(0u64, |h, (id, score)| mix(mix(h, *id), score.to_bits()));
        points.push(Point {
            algorithm: "betweenness_sample64",
            threads,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            result_hash: hash,
        });

        let lp_config = LabelPropagationConfig {
            max_iterations: 16,
            min_community_size: 1,
        };
        let started = Instant::now();
        let lp = if threads == 1 {
            deterministic_label_propagation(&mt, &all, lp_config, None, nodes * 16).unwrap()
        } else {
            pool.install(|| {
                deterministic_label_propagation_parallel(&mt, &all, lp_config, None, nodes * 16)
            })
            .unwrap()
        };
        let hash = lp
            .node_to_community
            .iter()
            .fold(0u64, |h, (id, c)| mix(mix(h, *id), *c));
        points.push(Point {
            algorithm: "label_propagation",
            threads,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            result_hash: hash,
        });
    }
    for algorithm in [
        "single_source_bfs",
        "pagerank",
        "wcc",
        "betweenness_sample64",
        "label_propagation",
    ] {
        let hashes = points
            .iter()
            .filter(|p| p.algorithm == algorithm)
            .map(|p| p.result_hash)
            .collect::<BTreeSet<_>>();
        assert_eq!(hashes.len(), 1, "{algorithm} 不同线程数结果不一致");
    }
    for p in &points {
        println!("{} {} 线程：{:.3} ms", p.algorithm, p.threads, p.elapsed_ms);
    }
    let path = support::write_json_report("graph-parallel.json", &points);
    println!("图算法并行报告已写入 {}", path.display());
}
