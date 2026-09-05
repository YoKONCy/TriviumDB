use serde_json::json;
use std::collections::BTreeSet;
use triviumdb::graph::analytics::{
    GraphWorkspace, articulation_points, build_workspace, harmonic_centrality, hits, k_core,
    node_similarity, strongly_connected_components, triangle_metrics, weighted_dijkstra,
    yen_k_shortest_paths,
};
use triviumdb::graph::budget::{BudgetExhaustionPolicy, TraversalBudget};
use triviumdb::graph::reachability::{
    ReachabilityConfig, ReachabilityDirection, traverse_compact, traverse_compact_parallel,
};
use triviumdb::graph::subset::{
    LabelPropagationConfig, SubsetPageRankConfig, deterministic_label_propagation,
    deterministic_label_propagation_parallel, subset_betweenness, subset_betweenness_parallel,
    subset_degree_centrality, subset_pagerank, subset_pagerank_parallel, subset_wcc,
    subset_wcc_parallel,
};
use triviumdb::query::pipeline::{
    BetweennessOperator, DegreeCentralityOperator, GraphSubsetMode, LabelPropagationOperator,
    LeidenOperator, NodeSet, PageRankOperator, PipelineBudget, PipelineContext, PipelineOperator,
    ScoreKind, StructuralAlgorithm, StructuralGraphOperator, WccOperator,
};
use triviumdb::storage::memtable::MemTable;

fn graph() -> MemTable<f32> {
    let mut mt = MemTable::new(2);
    for id in 1..=7 {
        mt.insert_with_id(id, &[id as f32, 0.0], json!({})).unwrap();
    }
    for (source, target, label) in [
        (1, 2, "x"),
        (2, 3, "x"),
        (3, 1, "x"),
        (3, 4, "bridge"),
        (4, 5, "x"),
        (5, 6, "x"),
        (6, 4, "x"),
    ] {
        mt.link(source, target, label.into(), 1.0).unwrap();
    }
    mt
}

fn budget(edges: usize) -> PipelineBudget {
    PipelineBudget {
        max_stages: 16,
        max_nodes: 100,
        max_node_set_bytes: 1024 * 1024,
        max_vector_read_bytes: 1024 * 1024,
        max_payload_lookups: 100,
        max_payload_parsed_bytes: 1024 * 1024,
        traversal: TraversalBudget {
            max_visited_nodes: 100,
            max_examined_edges: edges,
            max_frontier_size: 100,
            max_depth: 4,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        },
        parallelism: Default::default(),
    }
}

#[test]
fn g1_scc_kcore_割点与手工_reference_一致() {
    let mt = graph();
    let nodes = BTreeSet::from([1, 2, 3, 4, 5, 6, 7]);
    let workspace = build_workspace(&mt, &nodes, None, 100, 1024 * 1024).unwrap();
    let scc = strongly_connected_components(&workspace);
    assert_eq!(scc[&1], 1);
    assert_eq!(scc[&2], 1);
    assert_eq!(scc[&3], 1);
    assert_eq!(scc[&4], 4);
    assert_eq!(scc[&5], 4);
    assert_eq!(scc[&6], 4);
    assert_eq!(scc[&7], 7);

    let core = k_core(&workspace);
    assert_eq!(core[&1], 2);
    assert_eq!(core[&2], 2);
    assert_eq!(core[&3], 2);
    assert_eq!(core[&7], 0);
    assert_eq!(articulation_points(&workspace), BTreeSet::from([3, 4]));
}

#[test]
fn g2_三角聚类系数与_hits_保持精确确定() {
    let mut mt = MemTable::new(2);
    for id in 1..=4 {
        mt.insert_with_id(id, &[id as f32, 0.0], json!({})).unwrap();
    }
    for (source, target) in [(1, 2), (2, 3), (3, 1), (1, 4), (4, 2)] {
        mt.link(source, target, "x".into(), 1.0).unwrap();
    }
    let nodes = BTreeSet::from([1, 2, 3, 4]);
    let workspace = build_workspace(&mt, &nodes, Some("x"), 100, 1024 * 1024).unwrap();
    let metrics = triangle_metrics(&workspace, 100).unwrap();
    assert_eq!(metrics[&3].0, 1);
    assert_eq!(metrics[&4].0, 1);
    assert_eq!(metrics[&1].0, 2);
    assert!((metrics[&1].1 - 2.0 / 3.0).abs() < 1e-12);
    let first = hits(&workspace, 100, 1e-10, 10_000).unwrap();
    let second = hits(&workspace, 100, 1e-10, 10_000).unwrap();
    assert_eq!(first, second);
    assert!(triangle_metrics(&workspace, 0).is_err());
    assert!(hits(&workspace, 100, 1e-10, 1).is_err());
    assert!(
        first
            .values()
            .all(|(authority, hub)| authority.is_finite() && hub.is_finite())
    );
}

#[test]
fn g1_链星团与自环边界图结果正确() {
    let chain = GraphWorkspace {
        directed: [(1, vec![2]), (2, vec![3]), (3, vec![4]), (4, vec![])]
            .into_iter()
            .collect(),
        reverse: [(1, vec![]), (2, vec![1]), (3, vec![2]), (4, vec![3])]
            .into_iter()
            .collect(),
        undirected: [(1, vec![2]), (2, vec![1, 3]), (3, vec![2, 4]), (4, vec![3])]
            .into_iter()
            .collect(),
        weighted: [
            (1, vec![(2, 1.0)]),
            (2, vec![(3, 1.0)]),
            (3, vec![(4, 1.0)]),
            (4, vec![]),
        ]
        .into_iter()
        .collect(),
        examined_edges: 3,
    };
    assert_eq!(
        strongly_connected_components(&chain),
        [(1, 1), (2, 2), (3, 3), (4, 4)].into_iter().collect()
    );
    assert_eq!(
        k_core(&chain),
        [(1, 1), (2, 1), (3, 1), (4, 1)].into_iter().collect()
    );
    assert_eq!(articulation_points(&chain), BTreeSet::from([2, 3]));

    let mut mt = MemTable::new(2);
    for id in 1..=5 {
        mt.insert_with_id(id, &[0.0, 0.0], json!({})).unwrap();
    }
    for (source, target) in [(1, 1), (1, 2), (1, 3), (1, 4), (2, 3), (3, 4), (4, 2)] {
        mt.link(source, target, "x".into(), 1.0).unwrap();
    }
    let workspace = build_workspace(
        &mt,
        &BTreeSet::from([1, 2, 3, 4, 5]),
        Some("x"),
        100,
        1024 * 1024,
    )
    .unwrap();
    let core = k_core(&workspace);
    assert_eq!(core[&1], 3);
    assert_eq!(core[&2], 3);
    assert_eq!(core[&3], 3);
    assert_eq!(core[&4], 3);
    assert_eq!(core[&5], 0);
    assert!(articulation_points(&workspace).is_empty());
}

#[test]
fn g2_无三角低度节点与_hits_参数预算边界正确() {
    let mut mt = MemTable::new(2);
    for id in 1..=3 {
        mt.insert_with_id(id, &[0.0, 0.0], json!({})).unwrap();
    }
    mt.link(1, 2, "x".into(), 1.0).unwrap();
    let workspace =
        build_workspace(&mt, &BTreeSet::from([1, 2, 3]), Some("x"), 10, 1024 * 1024).unwrap();
    let metrics = triangle_metrics(&workspace, 10).unwrap();
    assert_eq!(metrics[&1], (0, 0.0));
    assert_eq!(metrics[&2], (0, 0.0));
    assert_eq!(metrics[&3], (0, 0.0));
    assert!(hits(&workspace, 0, 1e-6, 10).is_err());
    assert!(hits(&workspace, 10, f64::NAN, 100).is_err());
    let scores = hits(&workspace, 10, 1e-6, 100).unwrap();
    let authority_norm = scores
        .values()
        .map(|(authority, _)| authority * authority)
        .sum::<f64>()
        .sqrt();
    let hub_norm = scores
        .values()
        .map(|(_, hub)| hub * hub)
        .sum::<f64>()
        .sqrt();
    assert!((authority_norm - 1.0).abs() < 1e-12);
    assert!((hub_norm - 1.0).abs() < 1e-12);
}

#[test]
fn g3_加权路径调和中心性与_yen_reference_一致() {
    let mut mt = MemTable::new(2);
    for id in 1..=5 {
        mt.insert_with_id(id, &[0.0, 0.0], json!({})).unwrap();
    }
    for (source, target, weight) in [
        (1, 2, 1.0),
        (2, 4, 1.0),
        (1, 3, 1.0),
        (3, 4, 1.0),
        (1, 4, 3.0),
        (2, 3, 0.5),
    ] {
        mt.link(source, target, "road".into(), weight).unwrap();
    }
    let workspace = build_workspace(
        &mt,
        &BTreeSet::from([1, 2, 3, 4, 5]),
        Some("road"),
        100,
        1024 * 1024,
    )
    .unwrap();
    let shortest = weighted_dijkstra(&workspace, 1, 4, 100).unwrap().unwrap();
    assert_eq!(shortest.nodes, vec![1, 2, 4]);
    assert_eq!(shortest.cost, 2.0);
    let paths = yen_k_shortest_paths(&workspace, 1, 4, 3, 1_000, 100).unwrap();
    assert_eq!(paths.len(), 3);
    assert_eq!(paths[0].nodes, vec![1, 2, 4]);
    assert_eq!(paths[1].nodes, vec![1, 3, 4]);
    assert_eq!(paths[2].nodes, vec![1, 2, 3, 4]);
    assert_eq!(paths[2].cost, 2.5);
    let harmonic = harmonic_centrality(&workspace, 1_000).unwrap();
    assert!((harmonic[&1] - 2.5).abs() < 1e-12);
    assert_eq!(harmonic[&5], 0.0);
    let mut invalid = workspace.clone();
    invalid.weighted.entry(1).or_default().push((4, -1.0));
    assert!(weighted_dijkstra(&invalid, 1, 4, 100).is_err());
    assert!(weighted_dijkstra(&workspace, 1, 4, 0).is_err());
    assert!(yen_k_shortest_paths(&workspace, 1, 4, 0, 100, 10).is_err());
}

#[test]
fn g4_pairset_jaccard_结果有界确定且预算拒绝() {
    let mut mt = MemTable::new(2);
    for id in 1..=6 {
        mt.insert_with_id(id, &[0.0, 0.0], json!({})).unwrap();
    }
    for (source, target) in [(1, 4), (1, 5), (2, 4), (2, 5), (3, 4), (3, 6)] {
        mt.link(source, target, "likes".into(), 1.0).unwrap();
    }
    let workspace = build_workspace(
        &mt,
        &BTreeSet::from([1, 2, 3, 4, 5, 6]),
        Some("likes"),
        100,
        1024 * 1024,
    )
    .unwrap();
    let first = node_similarity(&workspace, 3, 0.3, 100, 1024).unwrap();
    let second = node_similarity(&workspace, 3, 0.3, 100, 1024).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.pairs()[0].left, 1);
    assert_eq!(first.pairs()[0].right, 2);
    assert_eq!(first.pairs()[0].similarity, 1.0);
    assert!(first.pairs().len() <= 3);
    assert!(node_similarity(&workspace, 3, 0.0, 1, 1024).is_err());
    assert!(node_similarity(&workspace, 3, 0.0, 100, 1).is_err());
}

#[test]
fn g1_g2_pipeline_保留命名指标并受工作区预算约束() {
    let mt = graph();
    let input = NodeSet::from_ids([1, 2, 3, 4, 5, 6]);
    for algorithm in [
        StructuralAlgorithm::Scc,
        StructuralAlgorithm::KCore,
        StructuralAlgorithm::ArticulationPoints,
        StructuralAlgorithm::TriangleCount,
        StructuralAlgorithm::Hits,
    ] {
        let mut context = PipelineContext::new(&mt, budget(1_000));
        let output = StructuralGraphOperator {
            mode: GraphSubsetMode::Induced,
            algorithm,
            label_filter: None,
            max_iterations: 20,
            tolerance: 1e-6,
        }
        .apply(input.clone(), &mut context)
        .unwrap();
        assert!(!output.is_empty());
    }

    let mut tiny = budget(100);
    tiny.max_node_set_bytes = 1;
    let mut context = PipelineContext::new(&mt, tiny);
    let error = StructuralGraphOperator {
        mode: GraphSubsetMode::Induced,
        algorithm: StructuralAlgorithm::KCore,
        label_filter: None,
        max_iterations: 20,
        tolerance: 1e-6,
    }
    .apply(input, &mut context)
    .unwrap_err();
    assert!(error.to_string().contains("工作区"));
}

#[test]
fn 位图_bfs_跨线程方向标签空洞和预算完全确定() {
    let mut mt = MemTable::new(2);
    for id in [10u64, 20, 30, 40, 50, 60] {
        mt.insert_with_id(id, &[1.0, 0.0], json!({})).unwrap();
    }
    mt.link(10, 20, "a".into(), 1.0).unwrap();
    mt.link(10, 30, "b".into(), 1.0).unwrap();
    mt.link(20, 40, "a".into(), 1.0).unwrap();
    mt.link(30, 40, "b".into(), 1.0).unwrap();
    mt.link(40, 50, "a".into(), 1.0).unwrap();
    mt.delete(30).unwrap();
    mt.insert_with_id(70, &[1.0, 0.0], json!({})).unwrap();
    mt.link(20, 70, "a".into(), 1.0).unwrap();

    for direction in [
        ReachabilityDirection::Outgoing,
        ReachabilityDirection::Incoming,
        ReachabilityDirection::Both,
    ] {
        for labels in [None, Some(vec!["a".to_string()])] {
            let config = ReachabilityConfig {
                min_depth: 0,
                max_depth: 4,
                labels,
                direction,
                max_visited_nodes: 100,
                max_results: 100,
                max_edges: 100,
                max_frontier_size: 100,
                exhaustion_policy: BudgetExhaustionPolicy::Error,
            };
            let source = if direction == ReachabilityDirection::Incoming {
                50
            } else {
                10
            };
            let reference = traverse_compact(&mt, source, &config).unwrap();
            for threads in [1usize, 2, 4, 8] {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap();
                for _ in 0..5 {
                    let actual = pool
                        .install(|| traverse_compact_parallel(&mt, source, &config))
                        .unwrap();
                    assert_eq!(actual.results, reference.results);
                    assert_eq!(actual.visited_nodes, reference.visited_nodes);
                    assert_eq!(actual.traversed_edges, reference.traversed_edges);
                }
            }
        }
    }

    for (visited, edges, frontier, results) in [
        (2, 100, 100, 100),
        (100, 1, 100, 100),
        (100, 100, 1, 100),
        (100, 100, 100, 1),
    ] {
        let config = ReachabilityConfig {
            min_depth: 1,
            max_depth: 4,
            labels: None,
            direction: ReachabilityDirection::Outgoing,
            max_visited_nodes: visited,
            max_results: results,
            max_edges: edges,
            max_frontier_size: frontier,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        };
        let reference = traverse_compact(&mt, 10, &config).unwrap_err().to_string();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let actual = pool
            .install(|| traverse_compact_parallel(&mt, 10, &config))
            .unwrap_err()
            .to_string();
        assert_eq!(actual, reference);
    }
}

#[test]
fn 并行_pagerank安全忽略子集外边并与串行一致() {
    let mut mt = MemTable::new(2);
    for id in 1..=4 {
        mt.insert_with_id(id, &[id as f32, 0.0], json!({})).unwrap();
    }
    mt.link(1, 2, "inside".into(), 1.0).unwrap();
    mt.link(1, 3, "outside".into(), 1.0).unwrap();
    mt.link(2, 4, "outside".into(), 1.0).unwrap();
    let nodes = BTreeSet::from([1, 2]);
    let config = SubsetPageRankConfig::default();

    let sequential = subset_pagerank(&mt, &nodes, config, None, 100).unwrap();
    let parallel = subset_pagerank_parallel(&mt, &nodes, config, None, 100).unwrap();
    assert_eq!(parallel, sequential);
    assert!(parallel.scores.iter().all(|(id, _)| nodes.contains(id)));

    let filtered_sequential = subset_pagerank(&mt, &nodes, config, Some("inside"), 100).unwrap();
    let filtered_parallel =
        subset_pagerank_parallel(&mt, &nodes, config, Some("inside"), 100).unwrap();
    assert_eq!(filtered_parallel, filtered_sequential);
}

#[test]
fn 第二批并行算法与串行参考完全一致() {
    let mt = graph();
    let nodes = BTreeSet::from([1, 2, 3, 4, 5, 6]);
    let reachability = ReachabilityConfig {
        min_depth: 0,
        max_depth: 4,
        labels: None,
        direction: ReachabilityDirection::Outgoing,
        max_visited_nodes: 100,
        max_results: 100,
        max_edges: 100,
        max_frontier_size: 100,
        exhaustion_policy: BudgetExhaustionPolicy::Error,
    };
    let sequential = traverse_compact(&mt, 1, &reachability).unwrap();
    let parallel = traverse_compact_parallel(&mt, 1, &reachability).unwrap();
    assert_eq!(parallel.results, sequential.results);
    assert_eq!(parallel.visited_nodes, sequential.visited_nodes);
    assert_eq!(parallel.traversed_edges, sequential.traversed_edges);

    let config = SubsetPageRankConfig::default();
    assert_eq!(
        subset_pagerank_parallel(&mt, &nodes, config, None, 10_000).unwrap(),
        subset_pagerank(&mt, &nodes, config, None, 10_000).unwrap()
    );
    assert_eq!(
        subset_wcc_parallel(&mt, &nodes, None, 10_000).unwrap(),
        subset_wcc(&mt, &nodes, None, 10_000).unwrap()
    );
    assert_eq!(
        subset_betweenness_parallel(&mt, &nodes, None, Some(3), 100_000).unwrap(),
        subset_betweenness(&mt, &nodes, None, Some(3), 100_000).unwrap()
    );

    // 并行标签传播使用同步轮次语义；验证不同并行运行完全确定，而非与旧异步更新强求相同分区。
    let label_config = LabelPropagationConfig {
        max_iterations: 16,
        min_community_size: 1,
    };
    let first =
        deterministic_label_propagation_parallel(&mt, &nodes, label_config, None, 10_000).unwrap();
    let sequential =
        deterministic_label_propagation(&mt, &nodes, label_config, None, 10_000).unwrap();
    let second =
        deterministic_label_propagation_parallel(&mt, &nodes, label_config, None, 10_000).unwrap();
    assert_eq!(first, sequential);
    assert_eq!(first, second);
}

#[test]
fn 第二批并行算法预算仍然_fail_closed() {
    let mt = graph();
    let nodes = BTreeSet::from([1, 2, 3, 4, 5, 6]);
    assert!(subset_pagerank_parallel(&mt, &nodes, Default::default(), None, 1).is_err());
    assert!(subset_wcc_parallel(&mt, &nodes, None, 1).is_err());
    assert!(subset_betweenness_parallel(&mt, &nodes, None, Some(2), 1).is_err());
    assert!(
        deterministic_label_propagation_parallel(
            &mt,
            &nodes,
            LabelPropagationConfig {
                max_iterations: 4,
                min_community_size: 1
            },
            None,
            1,
        )
        .is_err()
    );
}

#[test]
fn 子集_pagerank_与手工矩阵迭代一致且不扫描集合外节点() {
    let mt = graph();
    let nodes = BTreeSet::from([1, 2, 3, 4]);
    let result = subset_pagerank(
        &mt,
        &nodes,
        SubsetPageRankConfig {
            tolerance: 1e-12,
            ..Default::default()
        },
        Some("x"),
        100,
    )
    .unwrap();
    let sum = result.scores.iter().map(|(_, score)| score).sum::<f64>();
    assert!((sum - 1.0).abs() < 1e-10);
    assert!(result.converged);
    assert_eq!(result.scores.last().unwrap().0, 4);
    assert!(result.scores.iter().all(|(id, _)| nodes.contains(id)));

    let invalid = SubsetPageRankConfig {
        damping: 1.0,
        ..Default::default()
    };
    assert!(subset_pagerank(&mt, &nodes, invalid, None, 100).is_err());
}

#[test]
fn wcc_度中心性和_brandes_与小图_reference_一致() {
    let mt = graph();
    let nodes = BTreeSet::from([1, 2, 3, 4, 5, 6, 7]);
    let (components, _) = subset_wcc(&mt, &nodes, None, 100).unwrap();
    assert_eq!(components, vec![vec![1, 2, 3, 4, 5, 6], vec![7]]);

    let (degree, _) = subset_degree_centrality(&mt, &nodes, None, 100).unwrap();
    let three = degree.iter().find(|entry| entry.id == 3).unwrap();
    assert_eq!(
        (three.out_degree, three.in_degree, three.total_degree),
        (2, 1, 3)
    );
    assert!((three.normalized - 0.25).abs() < 1e-12);

    let chain = BTreeSet::from([1, 2, 3]);
    let centrality = subset_betweenness(&mt, &chain, Some("x"), None, 100).unwrap();
    let scores = centrality
        .scores
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
    assert!(centrality.exact);
    assert!((scores[&2] - 0.5).abs() < 1e-12);
    assert!((scores[&1] - 0.5).abs() < 1e-12);
    assert!((scores[&3] - 0.5).abs() < 1e-12);
}

#[test]
fn 采样中介中心性显式标记近似且零采样拒绝() {
    let mt = graph();
    let nodes = BTreeSet::from([1, 2, 3, 4]);
    let sampled = subset_betweenness(&mt, &nodes, None, Some(2), 100).unwrap();
    assert!(!sampled.exact);
    assert_eq!(sampled.sampled_sources, 2);
    assert!(subset_betweenness(&mt, &nodes, None, Some(0), 100).is_err());
}

#[test]
fn 标签传播确定性且明确不是_leiden() {
    let mt = graph();
    let nodes = BTreeSet::from([1, 2, 3, 4, 5, 6]);
    let config = LabelPropagationConfig {
        max_iterations: 20,
        min_community_size: 2,
    };
    let first = deterministic_label_propagation(&mt, &nodes, config, Some("x"), 100).unwrap();
    let second = deterministic_label_propagation(&mt, &nodes, config, Some("x"), 100).unwrap();
    assert_eq!(first.node_to_community, second.node_to_community);
    assert_eq!(first.node_to_community[&1], first.node_to_community[&2]);
    assert_eq!(first.node_to_community[&4], first.node_to_community[&5]);
    assert_ne!(first.node_to_community[&1], first.node_to_community[&4]);
}

#[test]
fn c1_6_d_三种子集模式和分数质量正确() {
    let mt = graph();
    let input = NodeSet::from_ids([1, 2, 3]);

    let mut context = PipelineContext::new(&mt, budget(100));
    let pagerank = PageRankOperator {
        mode: GraphSubsetMode::Boundary {
            hops: 1,
            labels: None,
            direction: ReachabilityDirection::Outgoing,
        },
        config: SubsetPageRankConfig::default(),
        label_filter: None,
    }
    .apply(input.clone(), &mut context)
    .unwrap();
    assert_eq!(
        pagerank.rows().iter().map(|row| row.id).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert!(
        pagerank
            .rows()
            .iter()
            .all(|row| row.graph_score.unwrap().kind == ScoreKind::Exact)
    );

    let mut context = PipelineContext::new(&mt, budget(100));
    let wcc = WccOperator {
        mode: GraphSubsetMode::Expand {
            hops: 1,
            labels: None,
            direction: ReachabilityDirection::Outgoing,
        },
        label_filter: None,
    }
    .apply(input.clone(), &mut context)
    .unwrap();
    assert!(wcc.rows().iter().any(|row| row.id == 4));
    assert!(wcc.rows().iter().all(|row| row.community_id.is_some()));

    let mut context = PipelineContext::new(&mt, budget(100));
    let degree = DegreeCentralityOperator {
        mode: GraphSubsetMode::Induced,
        label_filter: None,
    }
    .apply(input.clone(), &mut context)
    .unwrap();
    assert!(
        degree
            .rows()
            .iter()
            .all(|row| row.graph_score.unwrap().kind == ScoreKind::Exact)
    );

    let mut context = PipelineContext::new(&mt, budget(100));
    let sampled = BetweennessOperator {
        mode: GraphSubsetMode::Induced,
        label_filter: None,
        sample_size: Some(1),
    }
    .apply(input.clone(), &mut context)
    .unwrap();
    assert!(
        sampled
            .rows()
            .iter()
            .all(|row| row.graph_score.unwrap().kind == ScoreKind::Approximate)
    );

    let mut context = PipelineContext::new(&mt, budget(100));
    let labels = LabelPropagationOperator {
        mode: GraphSubsetMode::Induced,
        config: LabelPropagationConfig {
            max_iterations: 10,
            min_community_size: 1,
        },
        label_filter: None,
    }
    .apply(input, &mut context)
    .unwrap();
    assert!(labels.rows().iter().all(|row| row.community_id.is_some()));

    let input = NodeSet::from_ids([1, 2, 3, 4, 5, 6]);
    let mut context = PipelineContext::new(&mt, budget(100));
    let leiden = LeidenOperator {
        mode: GraphSubsetMode::Induced,
        config: triviumdb::graph::leiden::LeidenConfig {
            min_community_size: 1,
            max_iterations: 16,
            compute_centroids: false,
        },
    }
    .apply(input, &mut context)
    .unwrap();
    assert_eq!(leiden.len(), 6);
    assert!(leiden.rows().iter().all(|row| row.community_id.is_some()));
}

#[test]
fn leiden_算子预算超限_fail_closed() {
    let mt = graph();
    let input = NodeSet::from_ids([1, 2, 3, 4, 5, 6]);
    let mut context = PipelineContext::new(&mt, budget(1));
    let error = LeidenOperator {
        mode: GraphSubsetMode::Induced,
        config: triviumdb::graph::leiden::LeidenConfig {
            min_community_size: 1,
            max_iterations: 16,
            compute_centroids: false,
        },
    }
    .apply(input, &mut context)
    .unwrap_err();
    assert!(error.to_string().contains("Leiden"));
}

#[test]
fn 图算法预算超限_fail_closed() {
    let mt = graph();
    let nodes = BTreeSet::from([1, 2, 3, 4, 5, 6]);
    assert!(subset_pagerank(&mt, &nodes, Default::default(), None, 1).is_err());
    assert!(subset_wcc(&mt, &nodes, None, 1).is_err());
    assert!(subset_degree_centrality(&mt, &nodes, None, 1).is_err());
    assert!(subset_betweenness(&mt, &nodes, None, None, 1).is_err());
    assert!(
        deterministic_label_propagation(
            &mt,
            &nodes,
            LabelPropagationConfig {
                max_iterations: 10,
                min_community_size: 1,
            },
            None,
            1,
        )
        .is_err()
    );
}
