//! 查询管线算子的单查询内线程扩展基准。
//!
//! ## 覆盖范围
//! 同一数据集分别测量多源 EXPAND、exact rerank、Degree centrality 和无索引属性扫描。
//! 每个算子依次请求 1/2/4/8/16 线程，并记录运行时实际采用的线程数。
//!
//! ## 正确性与计时边界
//! 每个线程配置必须产生相同稳定结果哈希；数据构建和输入 NodeSet 构造不计时，
//! `elapsed_ms` 包含单次 `PipelineOperator::apply` 的完整成本。默认数据规模为
//! 200K × 768，可通过 `TDB_PARALLEL_NODES` 和 `TDB_PARALLEL_DIM` 调整。

#[path = "support/mod.rs"]
mod support;

use serde::Serialize;
use serde_json::json;
use std::time::Instant;
use triviumdb::graph::budget::{BudgetExhaustionPolicy, TraversalBudget};
use triviumdb::graph::reachability::ReachabilityDirection;
use triviumdb::query::parallel::QueryParallelismBudget;
use triviumdb::query::pipeline::{
    DegreeCentralityOperator, ExactRerank, Expand, GraphSubsetMode, NodeSet, PipelineBudget,
    PipelineContext, PipelineOperator, PropertyLookup,
};
use triviumdb::storage::memtable::MemTable;

#[derive(Debug, Serialize)]
struct Point {
    operator: &'static str,
    requested_threads: usize,
    actual_threads: usize,
    elapsed_ms: f64,
    rows: usize,
    result_hash: u64,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    nodes: usize,
    dim: usize,
    available_threads: usize,
    points: Vec<Point>,
}

fn build(nodes: usize, dim: usize) -> MemTable<f32> {
    let mut mt = MemTable::new(dim);
    for id in 1..=nodes as u64 {
        let mut vector = vec![0.0; dim];
        vector[id as usize % dim] = 1.0;
        mt.insert_with_id(
            id,
            &vector,
            json!({"active": id % 10 == 0, "group": id % 32}),
        )
        .unwrap();
    }
    for id in 1..=nodes as u64 {
        for offset in [1u64, 17, 257, 4099] {
            let target = (id + offset - 1) % nodes as u64 + 1;
            if target != id {
                mt.link(id, target, "related".into(), 0.9).unwrap();
            }
        }
    }
    mt
}

fn budget(nodes: usize, dim: usize, threads: usize) -> PipelineBudget {
    PipelineBudget {
        max_stages: 16,
        max_nodes: nodes,
        max_node_set_bytes: nodes.saturating_mul(256),
        max_vector_read_bytes: nodes.saturating_mul(dim).saturating_mul(8),
        traversal: TraversalBudget {
            max_visited_nodes: nodes.saturating_mul(8),
            max_examined_edges: nodes.saturating_mul(32),
            max_frontier_size: nodes,
            max_depth: 8,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        },
        parallelism: QueryParallelismBudget {
            max_threads: threads,
            min_parallel_rows: 0,
        },
    }
}

fn hash(set: &NodeSet) -> u64 {
    set.rows().iter().fold(0xcbf29ce484222325u64, |hash, row| {
        let score = row
            .similarity
            .or(row.graph_score)
            .or(row.property_score)
            .map_or(0, |score| score.value.to_bits() as u64);
        hash.wrapping_mul(0x100000001b3)
            .wrapping_add(row.id)
            .wrapping_add(score.rotate_left(17))
    })
}

fn run<T: PipelineOperator<f32>>(
    operator_name: &'static str,
    operator: T,
    input: NodeSet,
    mt: &MemTable<f32>,
    nodes: usize,
    dim: usize,
    requested_threads: usize,
) -> Point {
    let parallelism = QueryParallelismBudget {
        max_threads: requested_threads,
        min_parallel_rows: 0,
    };
    let started = Instant::now();
    let output = operator
        .apply(
            input,
            &mut PipelineContext::new(mt, budget(nodes, dim, requested_threads)),
        )
        .unwrap();
    Point {
        operator: operator_name,
        requested_threads,
        actual_threads: parallelism.threads(nodes),
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        rows: output.len(),
        result_hash: hash(&output),
    }
}

fn main() {
    let nodes = support::env_usize("TDB_PARALLEL_NODES", 200_000);
    let dim = support::env_usize("TDB_PARALLEL_DIM", 768);
    let available_threads = support::available_threads();
    let mt = build(nodes, dim);
    let all = NodeSet::from_ids(1..=nodes as u64);
    let sources = NodeSet::from_ids((10..=nodes as u64).step_by(10));
    let mut points = Vec::new();
    for requested_threads in [1usize, 2, 4, 8, 16] {
        points.push(run(
            "multi_source_expand",
            Expand {
                min_depth: 1,
                max_depth: 2,
                labels: None,
                direction: ReachabilityDirection::Outgoing,
                include_input: false,
            },
            sources.clone(),
            &mt,
            nodes,
            dim,
            requested_threads,
        ));
        points.push(run(
            "exact_rerank",
            ExactRerank {
                query: vec![1.0; dim],
                top_k: Some(100),
            },
            all.clone(),
            &mt,
            nodes,
            dim,
            requested_threads,
        ));
        points.push(run(
            "degree",
            DegreeCentralityOperator {
                mode: GraphSubsetMode::Induced,
                label_filter: None,
            },
            all.clone(),
            &mt,
            nodes,
            dim,
            requested_threads,
        ));
        points.push(run(
            "property_scan",
            PropertyLookup {
                field: "active".into(),
                value: json!(true),
            },
            NodeSet::empty(),
            &mt,
            nodes,
            dim,
            requested_threads,
        ));
    }
    for operator in [
        "multi_source_expand",
        "exact_rerank",
        "degree",
        "property_scan",
    ] {
        let hashes = points
            .iter()
            .filter(|point| point.operator == operator)
            .map(|point| point.result_hash)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(hashes.len(), 1, "{operator} 不同线程数结果不一致");
    }
    for point in &points {
        println!(
            "{} 请求/实际线程 {}/{}：{:.3} ms，{} 行",
            point.operator,
            point.requested_threads,
            point.actual_threads,
            point.elapsed_ms,
            point.rows
        );
    }
    let path = support::write_json_report(
        format!("query-parallel-{nodes}-{dim}.json"),
        &Report {
            schema_version: 1,
            nodes,
            dim,
            available_threads,
            points,
        },
    );
    println!("查询并行报告已写入 {}", path.display());
}
