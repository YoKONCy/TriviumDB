//! 三模管线最终 matched-result 性能 Gate。
//!
//! ## 目的
//! 在 384/768/1536/3072 四个产品维度上，对四类三阶段查询比较“先全量评分”与
//! “先通过属性/图约束缩小候选再评分”两个合法物理计划。Gate 要求结果逐字段完全
//! 相同，并且 P95 至少提升 1.5×，或实际评分候选至少减少 50%。
//!
//! ## 数据与计时协议
//! - 默认每个维度 20K 节点，可用 `TDB_PIPELINE_GATE_NODES` 调整。
//! - 向量、Payload 和图均由 NodeId 确定性生成，不使用随机数。
//! - 每个计划独立执行 7 次，报告离散 P95；数据构建和公共中间集合不计时。
//! - 报告统一写入 `target/bench-reports/pipeline-gate.json`。
//!
//! 运行入口：`cargo bench --bench bench_pipeline_gate`。

#[path = "support/mod.rs"]
mod support;

use serde::Serialize;
use serde_json::json;
use std::time::Instant;
use triviumdb::graph::budget::{BudgetExhaustionPolicy, TraversalBudget};
use triviumdb::graph::reachability::ReachabilityDirection;
use triviumdb::query::pipeline::{
    ExactRerank, Expand, NodeSet, PipelineBudget, PipelineContext, PipelineOperator,
    PropertyLookup, SetOperation, combine_sets,
};
use triviumdb::storage::memtable::MemTable;

#[derive(Serialize)]
struct GatePoint {
    family: &'static str,
    dim: usize,
    baseline_p95_ms: f64,
    optimized_p95_ms: f64,
    speedup: f64,
    baseline_scored: usize,
    optimized_scored: usize,
    candidate_reduction: f64,
    result_hash: u64,
    passed: bool,
}

fn build(nodes: usize, dim: usize) -> MemTable<f32> {
    let mut mt = MemTable::new(dim);
    for id in 1..=nodes as u64 {
        let mut vector = vec![0.0; dim];
        vector[id as usize % dim] = 1.0;
        vector[(id as usize * 17 + 3) % dim] = 0.5;
        mt.insert_with_id(
            id,
            &vector,
            json!({"active": id % 10 == 0, "group": id % 32}),
        )
        .unwrap();
    }
    mt.register_property_index("active");
    mt.register_property_index("group");
    for id in 1..nodes as u64 {
        mt.link(id, id + 1, "next".into(), 0.9).unwrap();
        if id + 32 <= nodes as u64 && id % 4 == 0 {
            mt.link(id, id + 32, "related".into(), 0.7).unwrap();
        }
    }
    mt
}

fn budget(nodes: usize, dim: usize) -> PipelineBudget {
    PipelineBudget {
        max_stages: 64,
        max_nodes: nodes,
        max_node_set_bytes: nodes.saturating_mul(256),
        max_vector_read_bytes: nodes.saturating_mul(dim).saturating_mul(16),
        max_payload_lookups: nodes as u64,
        max_payload_parsed_bytes: nodes.saturating_mul(1024) as u64,
        traversal: TraversalBudget {
            max_visited_nodes: nodes.saturating_mul(8),
            max_examined_edges: nodes.saturating_mul(32),
            max_frontier_size: nodes,
            max_depth: 8,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        },
        parallelism: Default::default(),
    }
}

fn apply<O: PipelineOperator<f32>>(
    mt: &MemTable<f32>,
    nodes: usize,
    dim: usize,
    input: NodeSet,
    operator: O,
) -> NodeSet {
    operator
        .apply(input, &mut PipelineContext::new(mt, budget(nodes, dim)))
        .unwrap()
}

fn rank(
    mt: &MemTable<f32>,
    nodes: usize,
    dim: usize,
    input: NodeSet,
    top: Option<usize>,
) -> NodeSet {
    apply(
        mt,
        nodes,
        dim,
        input,
        ExactRerank {
            query: vec![1.0; dim],
            top_k: top,
        },
    )
}

fn lookup(
    mt: &MemTable<f32>,
    nodes: usize,
    dim: usize,
    field: &str,
    value: serde_json::Value,
) -> NodeSet {
    apply(
        mt,
        nodes,
        dim,
        NodeSet::empty(),
        PropertyLookup {
            field: field.into(),
            value,
        },
    )
}

fn expand(mt: &MemTable<f32>, nodes: usize, dim: usize, input: NodeSet) -> NodeSet {
    apply(
        mt,
        nodes,
        dim,
        input,
        Expand {
            min_depth: 1,
            max_depth: 3,
            labels: None,
            direction: ReachabilityDirection::Outgoing,
            include_input: true,
        },
    )
}

fn hash(set: &NodeSet) -> u64 {
    set.rows().iter().fold(0xcbf29ce484222325u64, |hash, row| {
        hash.wrapping_mul(0x100000001b3)
            ^ row.id
            ^ row
                .similarity
                .map_or(0, |score| score.value.to_bits() as u64)
    })
}

fn p95(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    support::percentile_sorted(&samples, 95)
}

fn measure<F: FnMut() -> NodeSet>(mut run: F) -> (f64, NodeSet) {
    let mut samples = Vec::new();
    let mut output = NodeSet::empty();
    for _ in 0..7 {
        let started = Instant::now();
        output = run();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    (p95(samples), output)
}

fn gate_family(
    family: &'static str,
    dim: usize,
    baseline_scored: usize,
    optimized_scored: usize,
    baseline: impl FnMut() -> NodeSet,
    optimized: impl FnMut() -> NodeSet,
) -> GatePoint {
    let (baseline_p95_ms, baseline_output) = measure(baseline);
    let (optimized_p95_ms, optimized_output) = measure(optimized);
    assert_eq!(
        baseline_output, optimized_output,
        "{family}/{dim} 结果不一致"
    );
    let speedup = baseline_p95_ms / optimized_p95_ms.max(f64::EPSILON);
    let candidate_reduction = 1.0 - optimized_scored as f64 / baseline_scored.max(1) as f64;
    GatePoint {
        family,
        dim,
        baseline_p95_ms,
        optimized_p95_ms,
        speedup,
        baseline_scored,
        optimized_scored,
        candidate_reduction,
        result_hash: hash(&optimized_output),
        passed: speedup >= 1.5 || candidate_reduction >= 0.5,
    }
}

fn main() {
    let nodes = support::env_usize("TDB_PIPELINE_GATE_NODES", 20_000usize);
    let mut points = Vec::new();
    for dim in [384usize, 768, 1536, 3072] {
        let mt = build(nodes, dim);
        let all = NodeSet::from_ids(1..=nodes as u64);
        let active = lookup(&mt, nodes, dim, "active", json!(true));
        let group = lookup(&mt, nodes, dim, "group", json!(3));
        let expanded_a = expand(
            &mt,
            nodes,
            dim,
            NodeSet::from_ids((8..=nodes as u64).step_by(100)),
        );
        let expanded_b = expand(
            &mt,
            nodes,
            dim,
            NodeSet::from_ids((11..=nodes as u64).step_by(100)),
        );
        let intersection = combine_sets(
            expanded_a.clone(),
            expanded_b.clone(),
            SetOperation::Intersect,
        );
        let active_expanded =
            combine_sets(expanded_a.clone(), active.clone(), SetOperation::Intersect);
        let group_expanded =
            combine_sets(expanded_a.clone(), group.clone(), SetOperation::Intersect);

        let baseline_all = all.len();
        let optimized_active = active.len();
        points.push(gate_family(
            "property_filter_rank",
            dim,
            baseline_all,
            optimized_active,
            || {
                let scored = rank(&mt, nodes, dim, all.clone(), None);
                let selected = combine_sets(scored, active.clone(), SetOperation::Intersect);
                rank(&mt, nodes, dim, selected, Some(100))
            },
            || rank(&mt, nodes, dim, active.clone(), Some(100)),
        ));

        let union = combine_sets(expanded_a.clone(), expanded_b.clone(), SetOperation::Union);
        points.push(gate_family(
            "multi_anchor_intersect_rank",
            dim,
            union.len(),
            intersection.len(),
            || {
                let scored = rank(&mt, nodes, dim, union.clone(), None);
                let selected = combine_sets(scored, intersection.clone(), SetOperation::Intersect);
                rank(&mt, nodes, dim, selected, Some(100))
            },
            || rank(&mt, nodes, dim, intersection.clone(), Some(100)),
        ));

        points.push(gate_family(
            "expand_property_rank",
            dim,
            expanded_a.len(),
            active_expanded.len(),
            || {
                let scored = rank(&mt, nodes, dim, expanded_a.clone(), None);
                let selected = combine_sets(scored, active.clone(), SetOperation::Intersect);
                rank(&mt, nodes, dim, selected, Some(100))
            },
            || rank(&mt, nodes, dim, active_expanded.clone(), Some(100)),
        ));

        points.push(gate_family(
            "expand_group_rank",
            dim,
            expanded_a.len(),
            group_expanded.len(),
            || {
                let scored = rank(&mt, nodes, dim, expanded_a.clone(), None);
                let selected = combine_sets(scored, group.clone(), SetOperation::Intersect);
                rank(&mt, nodes, dim, selected, Some(100))
            },
            || rank(&mt, nodes, dim, group_expanded.clone(), Some(100)),
        ));
    }
    assert!(points.iter().all(|point| point.passed));
    for point in &points {
        println!(
            "{} / {} 维：P95 {:.3} → {:.3} ms，{:.2}×，候选减少 {:.1}%",
            point.family,
            point.dim,
            point.baseline_p95_ms,
            point.optimized_p95_ms,
            point.speedup,
            point.candidate_reduction * 100.0
        );
    }
    let path = support::write_json_report("pipeline-gate.json", &points);
    println!("三模管线 Gate 报告已写入 {}", path.display());
}
