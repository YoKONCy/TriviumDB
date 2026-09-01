use serde_json::json;
use triviumdb::graph::budget::{BudgetExhaustionPolicy, TraversalBudget};
use triviumdb::graph::reachability::ReachabilityDirection;
use triviumdb::query::pipeline::{
    BoundedIterate, Expand, NodeSet, PipelineBudget, PipelineContext, PipelineOperator,
    SaPprOperator, ScoreKind,
};
use triviumdb::storage::memtable::MemTable;

fn graph() -> MemTable<f32> {
    let mut mt = MemTable::new(2);
    for id in 1..=5 {
        mt.insert_with_id(id, &[id as f32, 1.0], json!({"id": id}))
            .unwrap();
    }
    mt.link(1, 2, "next".into(), 1.0).unwrap();
    mt.link(2, 3, "next".into(), 1.0).unwrap();
    mt.link(3, 4, "next".into(), 1.0).unwrap();
    mt.link(4, 5, "next".into(), 1.0).unwrap();
    mt
}

fn budget() -> PipelineBudget {
    PipelineBudget {
        max_stages: 16,
        max_nodes: 100,
        max_node_set_bytes: 1024 * 1024,
        max_vector_read_bytes: 1024 * 1024,
        traversal: TraversalBudget {
            max_visited_nodes: 100,
            max_examined_edges: 100,
            max_frontier_size: 100,
            max_depth: 8,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        },
        parallelism: Default::default(),
    }
}

#[test]
fn sa_ppr_明确标记_depth_bounded_且确定() {
    let mt = graph();
    let operator = SaPprOperator {
        max_depth: 2,
        restart_alpha: 0.15,
        labels: Some(vec!["next".into()]),
        max_edges_per_node: 8,
        min_edge_weight: 0.0,
    };
    let mut first_context = PipelineContext::new(&mt, budget());
    let first = operator
        .apply(NodeSet::from_ids([1]), &mut first_context)
        .unwrap();
    let mut second_context = PipelineContext::new(&mt, budget());
    let second = operator
        .apply(NodeSet::from_ids([1]), &mut second_context)
        .unwrap();
    assert_eq!(first, second);
    assert!(first.rows().iter().all(|row| {
        row.graph_score
            .is_some_and(|score| score.kind == ScoreKind::DepthBounded)
    }));
}

#[test]
fn iterate_去重并在固定点终止() {
    let mt = graph();
    let iterate = BoundedIterate {
        operators: vec![Box::new(Expand {
            min_depth: 1,
            max_depth: 1,
            labels: Some(vec!["next".into()]),
            direction: ReachabilityDirection::Outgoing,
            include_input: false,
        })],
        max_iterations: 10,
        stop_on_fixed_point: true,
    };
    let mut context = PipelineContext::new(&mt, budget());
    let output = iterate.apply(NodeSet::from_ids([1]), &mut context).unwrap();
    assert_eq!(
        output.rows().iter().map(|row| row.id).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
}

#[test]
fn iterate_最大轮数严格限制传播深度() {
    let mt = graph();
    let iterate = BoundedIterate {
        operators: vec![Box::new(Expand {
            min_depth: 1,
            max_depth: 1,
            labels: Some(vec!["next".into()]),
            direction: ReachabilityDirection::Outgoing,
            include_input: false,
        })],
        max_iterations: 2,
        stop_on_fixed_point: false,
    };
    let mut context = PipelineContext::new(&mt, budget());
    let output = iterate.apply(NodeSet::from_ids([1]), &mut context).unwrap();
    assert_eq!(
        output.rows().iter().map(|row| row.id).collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
fn iterate_零轮数明确拒绝() {
    let mt = graph();
    let iterate = BoundedIterate::<f32> {
        operators: Vec::new(),
        max_iterations: 0,
        stop_on_fixed_point: true,
    };
    let mut context = PipelineContext::new(&mt, budget());
    assert!(iterate.apply(NodeSet::from_ids([1]), &mut context).is_err());
}
