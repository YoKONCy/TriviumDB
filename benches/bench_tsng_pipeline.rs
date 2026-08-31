//! 三模管线 Q1–Q14 能力覆盖与分层诊断 runner。
//!
//! ## 定位
//! 本文件不是统一延迟排行榜，而是将路线图 Q1–Q14 映射到真实 PipelineOperator，
//! 用于确认复杂查询族均可执行、预算指标可观测，并定位跨阶段物化成本。各 Q 点输入
//! 集合和算子不同，因此只能比较同名 Q 点在相同参数下的基线结果。
//!
//! ## 参数与数据
//! - `TDB_PIPELINE_NODES`：节点数，默认 200K。
//! - `TDB_PIPELINE_DIM`：向量维度，默认 768。
//! - `TDB_PIPELINE_REPORT`：可选自定义报告路径。
//! - 向量、Payload、next/related 边均由 NodeId 确定性生成，不使用随机数。
//!
//! ## Q5 特殊诊断点
//! 当节点数不超过 200K 时，额外执行 detailed reference、compact traversal、
//! `Expand::apply`、NodeSet clone 和 normalize 五个分层点。这些点用于 Amdahl 分析，
//! 不应与 Q1–Q14 端到端结果横向排名。1M 运行会跳过昂贵 reference 以控制资源。
//!
//! ## 计时边界
//! 数据库构建不计时。每个 `Point` 通常包含一个 source operator 与一个目标 operator；
//! Q8 只计集合交集，Q5 分层点按其名称明确计时范围。报告中的 `vector_read_bytes`
//! 来自管线观测，不是操作系统物理读取字节。

use serde::Serialize;
use serde_json::json;
use std::fs;
use std::time::Instant;
use triviumdb::graph::budget::{BudgetExhaustionPolicy, TraversalBudget};
use triviumdb::graph::pathfinding::BoundedPathConfig;
use triviumdb::graph::reachability::{
    ReachabilityConfig, ReachabilityDirection, traverse_compact, traverse_detailed,
};
use triviumdb::query::cascades::{OptimizerBudget, optimize_pipeline};
use triviumdb::query::pipeline::{
    BoundedAllPaths, BoundedIterate, DegreeCentralityOperator, ExactRerank, Expand,
    GraphSubsetMode, LabelPropagationOperator, NodeSet, PageRankOperator, PathStrengthAggregation,
    PipelineBudget, PipelineContext, PipelineOperator, PropertyLookup, SaPprOperator, SetOperation,
    WccOperator, combine_sets, execute_pipeline,
};
use triviumdb::query::tql_parser::parse_tql;
use triviumdb::storage::memtable::MemTable;

#[derive(Serialize)]
struct Point {
    query: &'static str,
    elapsed_ms: f64,
    rows: usize,
    stages: usize,
    vector_read_bytes: usize,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    nodes: usize,
    dim: usize,
    points: Vec<Point>,
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn build(nodes: usize, dim: usize) -> MemTable<f32> {
    let mut mt = MemTable::new(dim);
    for id in 1..=nodes as u64 {
        let mut vector = vec![0.0; dim];
        vector[(id as usize) % dim] = 1.0;
        mt.insert_with_id(
            id,
            &vector,
            json!({"active": id % 10 == 0, "group": id % 32}),
        )
        .unwrap();
    }
    mt.register_property_index("active");
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
        max_node_set_bytes: 256 * 1024 * 1024,
        max_vector_read_bytes: nodes.saturating_mul(dim).saturating_mul(16),
        traversal: TraversalBudget {
            max_visited_nodes: nodes,
            max_examined_edges: nodes.saturating_mul(8),
            max_frontier_size: nodes,
            max_depth: 16,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        },
        parallelism: Default::default(),
    }
}

fn run<T: PipelineOperator<f32> + 'static>(
    name: &'static str,
    mt: &MemTable<f32>,
    nodes: usize,
    dim: usize,
    source: NodeSet,
    operator: T,
) -> Point {
    let started = Instant::now();
    let mut context = PipelineContext::new(mt, budget(nodes, dim));
    let source_operator = NodeSetSource { set: source };
    let operators: Vec<Box<dyn PipelineOperator<f32>>> =
        vec![Box::new(source_operator), Box::new(operator)];
    let output = execute_pipeline(&mut context, &operators).unwrap();
    Point {
        query: name,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        rows: output.len(),
        stages: context.metrics.len(),
        vector_read_bytes: context
            .metrics
            .last()
            .map_or(0, |metrics| metrics.vector_read_bytes),
    }
}

struct NodeSetSource {
    set: NodeSet,
}

struct DetailedExpandReference {
    min_depth: usize,
    max_depth: usize,
    labels: Option<Vec<String>>,
}

impl PipelineOperator<f32> for DetailedExpandReference {
    fn name(&self) -> &'static str {
        "expand_detailed_reference"
    }

    fn apply(
        &self,
        input: NodeSet,
        context: &mut PipelineContext<'_, f32>,
    ) -> triviumdb::Result<NodeSet> {
        let mut ids = input
            .rows()
            .iter()
            .map(|row| row.id)
            .collect::<std::collections::BTreeSet<_>>();
        for source in input.rows() {
            let reached = traverse_detailed(
                context.memtable,
                source.id,
                &ReachabilityConfig {
                    min_depth: self.min_depth,
                    max_depth: self.max_depth,
                    labels: self.labels.clone(),
                    direction: ReachabilityDirection::Outgoing,
                    max_visited_nodes: context.budget.traversal.max_visited_nodes,
                    max_results: context.budget.max_nodes,
                    max_edges: context.budget.traversal.max_examined_edges,
                    max_frontier_size: context.budget.traversal.max_frontier_size,
                    exhaustion_policy: BudgetExhaustionPolicy::Error,
                },
            )?;
            ids.extend(reached.results.into_iter().map(|hit| hit.target_id));
        }
        Ok(NodeSet::from_ids(ids))
    }
}

/// Q5 成本分层：分别测量「纯遍历」「遍历+建行」「完整管线」三层，定位剩余瓶颈所在层。
fn q5_layers(mt: &MemTable<f32>, nodes: usize, dim: usize, source: &NodeSet) -> Vec<Point> {
    let cfg = ReachabilityConfig {
        min_depth: 1,
        max_depth: 3,
        labels: None,
        direction: ReachabilityDirection::Outgoing,
        max_visited_nodes: nodes,
        max_results: nodes,
        max_edges: nodes.saturating_mul(8),
        max_frontier_size: nodes,
        exhaustion_policy: BudgetExhaustionPolicy::Error,
    };

    // 第 1 层：只做 compact BFS，命中结果立即丢弃，不构造任何 NodeRow。
    let started = Instant::now();
    let mut compact_hits = 0usize;
    for row in source.rows() {
        let reached = traverse_compact(mt, row.id, &cfg).unwrap();
        compact_hits += reached.results.len();
    }
    let layer_compact = Point {
        query: "Q5_layer1_traverse_compact_only",
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        rows: compact_hits,
        stages: 0,
        vector_read_bytes: 0,
    };

    // 第 1 层对照：只做 detailed BFS（复制路径与边元数据），同样立即丢弃。
    let started = Instant::now();
    let mut detailed_hits = 0usize;
    for row in source.rows() {
        let reached = traverse_detailed(mt, row.id, &cfg).unwrap();
        detailed_hits += reached.results.len();
    }
    let layer_detailed = Point {
        query: "Q5_layer1_traverse_detailed_only",
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        rows: detailed_hits,
        stages: 0,
        vector_read_bytes: 0,
    };

    // 第 2 层：直接调用 Expand::apply，包含 BTreeMap 去重与 NodeRow / provenance 构造，但不含 normalize。
    let started = Instant::now();
    let mut context = PipelineContext::new(mt, budget(nodes, dim));
    let expanded = Expand {
        min_depth: 1,
        max_depth: 3,
        labels: None,
        direction: ReachabilityDirection::Outgoing,
        include_input: true,
    }
    .apply(source.clone(), &mut context)
    .unwrap();
    let layer_expand = Point {
        query: "Q5_layer2_expand_apply",
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        rows: expanded.len(),
        stages: 0,
        vector_read_bytes: 0,
    };

    // 第 3 层前置：单独测量一次 NodeSet 深拷贝的成本，便于把克隆开销从 normalize 中剥离。
    let started = Instant::now();
    let cloned_rows = expanded.clone().into_rows();
    let layer_clone = Point {
        query: "Q5_layer3a_nodeset_clone_only",
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        rows: cloned_rows.len(),
        stages: 0,
        vector_read_bytes: 0,
    };

    // 第 3 层：只测 normalize（排序 + 去重合并）。
    let started = Instant::now();
    let normalized = NodeSet::from_rows(cloned_rows);
    let layer_normalize = Point {
        query: "Q5_layer3b_normalize_only",
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        rows: normalized.len(),
        stages: 0,
        vector_read_bytes: 0,
    };

    vec![
        layer_compact,
        layer_detailed,
        layer_expand,
        layer_clone,
        layer_normalize,
    ]
}

impl PipelineOperator<f32> for NodeSetSource {
    fn name(&self) -> &'static str {
        "benchmark_nodeset_source"
    }

    fn apply(
        &self,
        _input: NodeSet,
        _context: &mut PipelineContext<'_, f32>,
    ) -> triviumdb::Result<NodeSet> {
        Ok(self.set.clone())
    }
}

#[allow(clippy::vec_init_then_push)]
fn main() {
    let nodes = env_usize("TDB_PIPELINE_NODES", 200_000);
    let dim = env_usize("TDB_PIPELINE_DIM", 768);
    let mt = build(nodes, dim);
    let seeds = NodeSet::from_ids([1, 4, 10, 32]);
    let expand = || Expand {
        min_depth: 1,
        max_depth: 3,
        labels: None,
        direction: ReachabilityDirection::Outgoing,
        include_input: true,
    };
    let subset = GraphSubsetMode::Expand {
        hops: 3,
        labels: None,
        direction: ReachabilityDirection::Outgoing,
    };
    let mut points = Vec::new();
    points.push(run(
        "Q1_search_expand",
        &mt,
        nodes,
        dim,
        seeds.clone(),
        expand(),
    ));
    points.push(run(
        "Q2_expand_similarity_filter",
        &mt,
        nodes,
        dim,
        seeds.clone(),
        ExactRerank {
            query: vec![1.0; dim],
            top_k: Some(32),
        },
    ));
    points.push(run(
        "Q3_expand_rank",
        &mt,
        nodes,
        dim,
        expand()
            .apply(
                seeds.clone(),
                &mut PipelineContext::new(&mt, budget(nodes, dim)),
            )
            .unwrap(),
        ExactRerank {
            query: vec![1.0; dim],
            top_k: Some(10),
        },
    ));
    points.push(run(
        "Q4_graph_rank_property",
        &mt,
        nodes,
        dim,
        seeds.clone(),
        PropertyLookup {
            field: "active".into(),
            value: json!(true),
        },
    ));
    let q5_source = PropertyLookup {
        field: "active".into(),
        value: json!(true),
    }
    .apply(
        NodeSet::empty(),
        &mut PipelineContext::new(&mt, budget(nodes, dim)),
    )
    .unwrap();
    if nodes <= 200_000 {
        points.push(run(
            "Q5_find_expand_detailed_reference",
            &mt,
            nodes,
            dim,
            q5_source.clone(),
            DetailedExpandReference {
                min_depth: 1,
                max_depth: 3,
                labels: None,
            },
        ));
    }
    if nodes <= 200_000 {
        points.extend(q5_layers(&mt, nodes, dim, &q5_source));
    }
    points.push(run(
        "Q5_find_expand_rank",
        &mt,
        nodes,
        dim,
        q5_source,
        expand(),
    ));
    points.push(run(
        "Q6_expand_pagerank",
        &mt,
        nodes,
        dim,
        seeds.clone(),
        PageRankOperator {
            mode: subset.clone(),
            config: Default::default(),
            label_filter: None,
        },
    ));
    points.push(run(
        "Q7_community_centrality",
        &mt,
        nodes,
        dim,
        seeds.clone(),
        LabelPropagationOperator {
            mode: subset.clone(),
            config: triviumdb::graph::subset::LabelPropagationConfig {
                max_iterations: 16,
                min_community_size: 1,
            },
            label_filter: None,
        },
    ));
    let left = expand()
        .apply(
            NodeSet::from_ids([1]),
            &mut PipelineContext::new(&mt, budget(nodes, dim)),
        )
        .unwrap();
    let right = expand()
        .apply(
            NodeSet::from_ids([4]),
            &mut PipelineContext::new(&mt, budget(nodes, dim)),
        )
        .unwrap();
    let started = Instant::now();
    let intersection = combine_sets(left, right, SetOperation::Intersect);
    points.push(Point {
        query: "Q8_multi_anchor_intersect",
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        rows: intersection.len(),
        stages: 1,
        vector_read_bytes: 0,
    });
    points.push(run(
        "Q9_path_constraints",
        &mt,
        nodes,
        dim,
        NodeSet::from_ids([1]),
        BoundedAllPaths {
            targets: vec![nodes.min(16) as u64],
            config: BoundedPathConfig {
                max_depth: 16,
                max_paths: 64,
                label_sequence: None,
                forbidden_nodes: Default::default(),
            },
            aggregation: PathStrengthAggregation::MaxProduct,
        },
    ));
    points.push(run(
        "Q10_expand_or_pagerank",
        &mt,
        nodes,
        dim,
        seeds.clone(),
        PageRankOperator {
            mode: subset.clone(),
            config: Default::default(),
            label_filter: None,
        },
    ));
    points.push(run(
        "Q11_wcc_rank",
        &mt,
        nodes,
        dim,
        seeds.clone(),
        WccOperator {
            mode: subset.clone(),
            label_filter: None,
        },
    ));
    points.push(run(
        "Q12_sa_ppr_property",
        &mt,
        nodes,
        dim,
        seeds.clone(),
        SaPprOperator {
            max_depth: 4,
            restart_alpha: 0.15,
            labels: None,
            max_edges_per_node: 32,
            min_edge_weight: 0.0,
        },
    ));
    points.push(run(
        "Q13_find_community_expand",
        &mt,
        nodes,
        dim,
        seeds.clone(),
        DegreeCentralityOperator {
            mode: subset,
            label_filter: None,
        },
    ));
    points.push(run(
        "Q14_bounded_iterate",
        &mt,
        nodes,
        dim,
        NodeSet::from_ids([1]),
        BoundedIterate {
            operators: vec![Box::new(expand())],
            max_iterations: 4,
            stop_on_fixed_point: true,
        },
    ));

    let vector = vec!["0"; dim].join(",");
    let query = parse_tql(&format!("SEARCH VECTOR [{vector}] TOP 10 AS seed WITH seed EXPAND seed [:next*1..2] AS related WITH related WHERE similarity(related) > 0 RETURN related")).unwrap();
    let _plan = optimize_pipeline(&query, &mt, OptimizerBudget::default());

    let report = Report {
        schema_version: 1,
        nodes,
        dim,
        points,
    };
    let output = std::env::var("TDB_PIPELINE_REPORT")
        .unwrap_or_else(|_| format!("target/bench-reports/tsng-c1-6-pipeline-{nodes}-{dim}.json"));
    if let Some(parent) = std::path::Path::new(&output).parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&output, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("三模管线报告已写入 {output}");
}
