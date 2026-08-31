use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use triviumdb::graph::budget::{BudgetExhaustionPolicy, TraversalBudget};
use triviumdb::graph::pathfinding::{BoundedPathConfig, bounded_all_paths};
use triviumdb::graph::reachability::ReachabilityDirection;
use triviumdb::query::pipeline::{
    AnchorAggregation, BatchShortestPaths, BoundedAllPaths, MultiAnchorExpand, NodeSet,
    PathStrengthAggregation, PipelineBudget, PipelineContext, PipelineOperator, ScoreKind,
};
use triviumdb::storage::memtable::MemTable;

fn graph() -> MemTable<f32> {
    let mut mt = MemTable::new(2);
    for id in 1..=6 {
        mt.insert_with_id(id, &[id as f32, 1.0], json!({"id": id}))
            .unwrap();
    }
    mt.link(1, 2, "a".into(), 0.5).unwrap();
    mt.link(2, 4, "b".into(), 0.8).unwrap();
    mt.link(1, 3, "a".into(), 0.9).unwrap();
    mt.link(3, 4, "b".into(), 0.7).unwrap();
    mt.link(5, 3, "a".into(), 1.0).unwrap();
    mt.link(4, 6, "c".into(), 0.6).unwrap();
    mt.link(3, 1, "cycle".into(), 1.0).unwrap();
    mt
}

fn budget() -> TraversalBudget {
    TraversalBudget {
        max_visited_nodes: 100,
        max_examined_edges: 100,
        max_frontier_size: 100,
        max_depth: 8,
        exhaustion_policy: BudgetExhaustionPolicy::Error,
    }
}

fn pipeline_budget() -> PipelineBudget {
    PipelineBudget {
        max_stages: 16,
        max_nodes: 100,
        max_node_set_bytes: 1024 * 1024,
        max_vector_read_bytes: 1024 * 1024,
        traversal: budget(),
        parallelism: Default::default(),
    }
}

#[test]
fn 有界路径确定性且强度与标签序列正确() {
    let mt = graph();
    let config = BoundedPathConfig {
        max_depth: 3,
        max_paths: 10,
        label_sequence: Some(vec!["a".into(), "b".into()]),
        forbidden_nodes: BTreeSet::new(),
    };
    let first = bounded_all_paths(&mt, 1, 4, &config, &budget()).unwrap();
    let second = bounded_all_paths(&mt, 1, 4, &config, &budget()).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.paths.len(), 2);
    assert_eq!(first.paths[0].nodes, [1, 2, 4]);
    assert_eq!(first.paths[1].nodes, [1, 3, 4]);
    assert!((first.paths[0].strength_product - 0.4).abs() < 1e-6);
    assert!((first.paths[1].strength_product - 0.63).abs() < 1e-6);
    assert!(!first.truncated);
}

#[test]
fn 禁经节点与最大路径数严格生效() {
    let mt = graph();
    let restricted = bounded_all_paths(
        &mt,
        1,
        4,
        &BoundedPathConfig {
            max_depth: 3,
            max_paths: 10,
            label_sequence: None,
            forbidden_nodes: BTreeSet::from([3]),
        },
        &budget(),
    )
    .unwrap();
    assert_eq!(restricted.paths.len(), 1);
    assert_eq!(restricted.paths[0].nodes, [1, 2, 4]);

    let limited = bounded_all_paths(
        &mt,
        1,
        4,
        &BoundedPathConfig {
            max_depth: 3,
            max_paths: 1,
            label_sequence: None,
            forbidden_nodes: BTreeSet::new(),
        },
        &budget(),
    )
    .unwrap();
    assert_eq!(limited.paths.len(), 1);
    assert!(limited.truncated);
}

#[test]
fn 路径预算耗尽默认_fail_closed() {
    let mt = graph();
    let mut tiny = budget();
    tiny.max_examined_edges = 1;
    let result = bounded_all_paths(
        &mt,
        1,
        4,
        &BoundedPathConfig {
            max_depth: 4,
            max_paths: 10,
            label_sequence: None,
            forbidden_nodes: BTreeSet::new(),
        },
        &tiny,
    );
    assert!(result.is_err());
}

#[test]
fn 多锚点加权聚合保留全部来源() {
    let mt = graph();
    let mut context = PipelineContext::new(&mt, pipeline_budget());
    let output = MultiAnchorExpand {
        max_depth: 1,
        labels: Some(vec!["a".into()]),
        direction: ReachabilityDirection::Outgoing,
        anchor_weights: BTreeMap::from([(1, 2.0), (5, 3.0)]),
        aggregation: AnchorAggregation::WeightedSum,
    }
    .apply(NodeSet::from_ids([1, 5]), &mut context)
    .unwrap();
    let row3 = output.rows().iter().find(|row| row.id == 3).unwrap();
    assert_eq!(row3.provenance.source_ids, [1, 5]);
    assert_eq!(row3.graph_score.unwrap().kind, ScoreKind::Exact);
    assert!((row3.graph_score.unwrap().value - 5.0).abs() < 1e-6);
}

#[test]
fn 批量最短路径选取确定性最短链() {
    let mt = graph();
    let mut context = PipelineContext::new(&mt, pipeline_budget());
    let output = BatchShortestPaths {
        targets: vec![4, 6],
        label_filter: None,
    }
    .apply(NodeSet::from_ids([1]), &mut context)
    .unwrap();
    let row4 = output.rows().iter().find(|row| row.id == 4).unwrap();
    assert_eq!(row4.path.as_deref(), Some(&[1, 2, 4][..]));
    assert_eq!(row4.path_count, Some(1));
    let row6 = output.rows().iter().find(|row| row.id == 6).unwrap();
    assert_eq!(row6.provenance.min_depth, Some(3));
}

#[test]
fn 多路径算子聚合强度和路径数() {
    let mt = graph();
    let mut context = PipelineContext::new(&mt, pipeline_budget());
    let output = BoundedAllPaths {
        targets: vec![4],
        config: BoundedPathConfig {
            max_depth: 3,
            max_paths: 10,
            label_sequence: Some(vec!["a".into(), "b".into()]),
            forbidden_nodes: BTreeSet::new(),
        },
        aggregation: PathStrengthAggregation::SumProduct,
    }
    .apply(NodeSet::from_ids([1]), &mut context)
    .unwrap();
    let row = &output.rows()[0];
    assert_eq!(row.path_count, Some(2));
    assert!((row.path_strength.unwrap().value - 1.03).abs() < 1e-6);
    assert_eq!(row.path.as_deref(), Some(&[1, 3, 4][..]));
    assert_eq!(row.path_strength.unwrap().kind, ScoreKind::Exact);
}
