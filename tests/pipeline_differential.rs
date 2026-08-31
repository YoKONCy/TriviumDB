use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use triviumdb::VectorType;
use triviumdb::graph::budget::{BudgetExhaustionPolicy, TraversalBudget};
use triviumdb::graph::pathfinding::{BoundedPathConfig, bounded_all_paths};
use triviumdb::graph::reachability::{ReachabilityConfig, ReachabilityDirection, traverse_compact};
use triviumdb::graph::subset::{
    LabelPropagationConfig, SubsetPageRankConfig, deterministic_label_propagation,
    subset_degree_centrality, subset_pagerank, subset_wcc,
};
use triviumdb::query::pipeline::{
    BoundedAllPaths, BoundedIterate, DegreeCentralityOperator, ExactRerank, Expand,
    GraphSubsetMode, LabelPropagationOperator, NodeSet, PageRankOperator, PathStrengthAggregation,
    PipelineBudget, PipelineContext, PipelineOperator, PropertyLookup, SaPprOperator, SetOperation,
    WccOperator, combine_sets,
};
use triviumdb::storage::memtable::MemTable;

fn graph() -> MemTable<f32> {
    let mut mt = MemTable::new(8);
    for id in 1..=40u64 {
        let mut vector = vec![0.0; 8];
        vector[id as usize % 8] = 1.0;
        mt.insert_with_id(id, &vector, json!({"active": id % 2 == 0, "group": id % 4}))
            .unwrap();
    }
    mt.register_property_index("active");
    for id in 1..40u64 {
        mt.link(id, id + 1, "next".into(), 0.9).unwrap();
        if id + 4 <= 40 {
            mt.link(id, id + 4, "related".into(), 0.7).unwrap();
        }
    }
    mt
}

fn budget() -> PipelineBudget {
    PipelineBudget {
        max_stages: 64,
        max_nodes: 10_000,
        max_node_set_bytes: 32 * 1024 * 1024,
        max_vector_read_bytes: 32 * 1024 * 1024,
        traversal: TraversalBudget {
            max_visited_nodes: 10_000,
            max_examined_edges: 100_000,
            max_frontier_size: 10_000,
            max_depth: 16,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        },
        parallelism: Default::default(),
    }
}

fn apply<O: PipelineOperator<f32>>(mt: &MemTable<f32>, input: NodeSet, operator: O) -> NodeSet {
    operator
        .apply(input, &mut PipelineContext::new(mt, budget()))
        .unwrap()
}

fn materialize(set: NodeSet) -> NodeSet {
    NodeSet::from_rows(set.into_rows())
}

fn expand(mt: &MemTable<f32>, input: NodeSet) -> NodeSet {
    apply(
        mt,
        input,
        Expand {
            min_depth: 1,
            max_depth: 2,
            labels: None,
            direction: ReachabilityDirection::Outgoing,
            include_input: true,
        },
    )
}

fn rerank(mt: &MemTable<f32>, input: NodeSet, top_k: usize) -> NodeSet {
    apply(
        mt,
        input,
        ExactRerank {
            query: vec![1.0; 8],
            top_k: Some(top_k),
        },
    )
}

fn ids(set: &NodeSet) -> Vec<u64> {
    set.rows().iter().map(|row| row.id).collect()
}

fn reference_expand(mt: &MemTable<f32>, sources: &[u64]) -> BTreeSet<u64> {
    let config = ReachabilityConfig {
        min_depth: 1,
        max_depth: 2,
        labels: None,
        direction: ReachabilityDirection::Outgoing,
        max_visited_nodes: 10_000,
        max_results: 10_000,
        max_edges: 100_000,
        max_frontier_size: 10_000,
        exhaustion_policy: BudgetExhaustionPolicy::Error,
    };
    let mut output = BTreeSet::from_iter(sources.iter().copied());
    for &source in sources {
        output.extend(
            traverse_compact(mt, source, &config)
                .unwrap()
                .results
                .into_iter()
                .map(|hit| hit.target_id),
        );
    }
    output
}

fn reference_rank(mt: &MemTable<f32>, candidates: &BTreeSet<u64>, top_k: usize) -> Vec<u64> {
    let query = vec![1.0; 8];
    let mut scored = candidates
        .iter()
        .filter_map(|&id| {
            mt.get_vector(id)
                .map(|vector| (id, <f32 as VectorType>::similarity(&query, vector)))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    scored.truncate(top_k);
    scored.into_iter().map(|(id, _)| id).collect()
}

/// 返回直接流水计划与每阶段显式物化计划；两者均执行真实产品算子。
fn execute_family(mt: &MemTable<f32>, family: usize, staged: bool) -> NodeSet {
    let barrier = |set| if staged { materialize(set) } else { set };
    let seeds = NodeSet::from_ids([1, 4]);
    match family {
        1 => expand(mt, seeds),
        2 => rerank(mt, barrier(expand(mt, seeds)), 12),
        3 => rerank(mt, barrier(expand(mt, seeds)), 6),
        4 => {
            let ranked = rerank(mt, NodeSet::from_ids(mt.all_node_ids()), 40);
            let active = apply(
                mt,
                NodeSet::empty(),
                PropertyLookup {
                    field: "active".into(),
                    value: json!(true),
                },
            );
            combine_sets(barrier(ranked), active, SetOperation::Intersect)
        }
        5 => {
            let active = apply(
                mt,
                NodeSet::empty(),
                PropertyLookup {
                    field: "active".into(),
                    value: json!(true),
                },
            );
            rerank(mt, barrier(expand(mt, barrier(active))), 10)
        }
        6 => {
            let expanded = expand(mt, seeds);
            let ranked = apply(
                mt,
                barrier(expanded),
                PageRankOperator {
                    mode: GraphSubsetMode::Induced,
                    config: Default::default(),
                    label_filter: None,
                },
            );
            rerank(mt, barrier(ranked), 8)
        }
        7 => {
            let communities = apply(
                mt,
                barrier(expand(mt, seeds)),
                LabelPropagationOperator {
                    mode: GraphSubsetMode::Induced,
                    config: LabelPropagationConfig {
                        max_iterations: 16,
                        min_community_size: 1,
                    },
                    label_filter: None,
                },
            );
            apply(
                mt,
                barrier(communities),
                DegreeCentralityOperator {
                    mode: GraphSubsetMode::Induced,
                    label_filter: None,
                },
            )
        }
        8 => {
            let left = expand(mt, NodeSet::from_ids([1]));
            let right = expand(mt, NodeSet::from_ids([4]));
            rerank(
                mt,
                combine_sets(barrier(left), barrier(right), SetOperation::Intersect),
                8,
            )
        }
        9 => apply(
            mt,
            NodeSet::from_ids([1]),
            BoundedAllPaths {
                targets: vec![8],
                config: BoundedPathConfig {
                    max_depth: 8,
                    max_paths: 32,
                    label_sequence: None,
                    forbidden_nodes: Default::default(),
                },
                aggregation: PathStrengthAggregation::MaxProduct,
            },
        ),
        10 => {
            let expanded = expand(mt, seeds);
            let active = apply(
                mt,
                NodeSet::empty(),
                PropertyLookup {
                    field: "active".into(),
                    value: json!(true),
                },
            );
            let selected = combine_sets(barrier(expanded), active, SetOperation::Intersect);
            apply(
                mt,
                barrier(selected),
                PageRankOperator {
                    mode: GraphSubsetMode::Induced,
                    config: Default::default(),
                    label_filter: None,
                },
            )
        }
        11 => {
            let connected = apply(
                mt,
                barrier(expand(mt, seeds)),
                WccOperator {
                    mode: GraphSubsetMode::Induced,
                    label_filter: None,
                },
            );
            rerank(mt, barrier(connected), 8)
        }
        12 => {
            let propagated = apply(
                mt,
                seeds,
                SaPprOperator {
                    max_depth: 3,
                    restart_alpha: 0.15,
                    labels: None,
                    max_edges_per_node: 16,
                    min_edge_weight: 0.0,
                },
            );
            let active = apply(
                mt,
                NodeSet::empty(),
                PropertyLookup {
                    field: "active".into(),
                    value: json!(true),
                },
            );
            combine_sets(barrier(propagated), active, SetOperation::Intersect)
        }
        13 => {
            let active = apply(
                mt,
                NodeSet::empty(),
                PropertyLookup {
                    field: "active".into(),
                    value: json!(true),
                },
            );
            let communities = apply(
                mt,
                barrier(active),
                LabelPropagationOperator {
                    mode: GraphSubsetMode::Induced,
                    config: LabelPropagationConfig {
                        max_iterations: 8,
                        min_community_size: 1,
                    },
                    label_filter: None,
                },
            );
            expand(mt, barrier(communities))
        }
        14 => apply(
            mt,
            NodeSet::from_ids([1]),
            BoundedIterate {
                operators: vec![Box::new(Expand {
                    min_depth: 1,
                    max_depth: 1,
                    labels: None,
                    direction: ReachabilityDirection::Outgoing,
                    include_input: true,
                })],
                max_iterations: 4,
                stop_on_fixed_point: true,
            },
        ),
        _ => unreachable!(),
    }
}

fn assert_reference(mt: &MemTable<f32>, family: usize, actual: &NodeSet) {
    match family {
        1 => assert_eq!(
            ids(actual),
            reference_expand(mt, &[1, 4])
                .into_iter()
                .collect::<Vec<_>>()
        ),
        2 => assert_eq!(
            ids(actual),
            reference_rank(mt, &reference_expand(mt, &[1, 4]), 12)
        ),
        3 => assert_eq!(
            ids(actual),
            reference_rank(mt, &reference_expand(mt, &[1, 4]), 6)
        ),
        4 => {
            let expected = mt
                .all_node_ids()
                .into_iter()
                .filter(|id| {
                    mt.get_payload(*id)
                        .is_some_and(|payload| payload["active"] == true)
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            assert_eq!(ids(actual), expected);
        }
        5 => {
            let active = mt
                .all_node_ids()
                .into_iter()
                .filter(|id| {
                    mt.get_payload(*id)
                        .is_some_and(|payload| payload["active"] == true)
                })
                .collect::<Vec<_>>();
            assert_eq!(
                ids(actual),
                reference_rank(mt, &reference_expand(mt, &active), 10)
            );
        }
        6 => {
            let universe = reference_expand(mt, &[1, 4]);
            let reference = subset_pagerank(
                mt,
                &universe,
                SubsetPageRankConfig::default(),
                None,
                100_000,
            )
            .unwrap();
            let scores = reference.scores.into_iter().collect::<BTreeMap<_, _>>();
            for row in actual.rows() {
                assert!(scores.contains_key(&row.id));
                assert!(row.similarity.is_some());
            }
        }
        7 => {
            let universe = reference_expand(mt, &[1, 4]);
            let labels = deterministic_label_propagation(
                mt,
                &universe,
                LabelPropagationConfig {
                    max_iterations: 16,
                    min_community_size: 1,
                },
                None,
                100_000,
            )
            .unwrap();
            let (degree, _) = subset_degree_centrality(mt, &universe, None, 100_000).unwrap();
            let degree = degree
                .into_iter()
                .map(|item| (item.id, item.normalized as f32))
                .collect::<BTreeMap<_, _>>();
            for row in actual.rows() {
                assert_eq!(
                    row.community_id,
                    labels.node_to_community.get(&row.id).copied()
                );
                assert_eq!(row.graph_score.unwrap().value, degree[&row.id]);
            }
        }
        8 => {
            let left = reference_expand(mt, &[1]);
            let right = reference_expand(mt, &[4]);
            let intersection = left.intersection(&right).copied().collect::<BTreeSet<_>>();
            assert_eq!(ids(actual), reference_rank(mt, &intersection, 8));
        }
        9 => {
            let reference = bounded_all_paths(
                mt,
                1,
                8,
                &BoundedPathConfig {
                    max_depth: 8,
                    max_paths: 32,
                    label_sequence: None,
                    forbidden_nodes: Default::default(),
                },
                &budget().traversal,
            )
            .unwrap();
            assert_eq!(actual.len(), usize::from(!reference.paths.is_empty()));
            if let Some(row) = actual.rows().first() {
                assert_eq!(row.path_count, Some(reference.paths.len()));
            }
        }
        10 => {
            let universe = reference_expand(mt, &[1, 4])
                .into_iter()
                .filter(|id| id % 2 == 0)
                .collect::<BTreeSet<_>>();
            let reference =
                subset_pagerank(mt, &universe, Default::default(), None, 100_000).unwrap();
            assert_eq!(
                BTreeSet::from_iter(ids(actual)),
                BTreeSet::from_iter(reference.scores.into_iter().map(|(id, _)| id))
            );
        }
        11 => {
            let universe = reference_expand(mt, &[1, 4]);
            let (components, _) = subset_wcc(mt, &universe, None, 100_000).unwrap();
            let expected = components.into_iter().flatten().collect::<BTreeSet<_>>();
            assert!(ids(actual).into_iter().all(|id| expected.contains(&id)));
        }
        12 => assert!(
            actual
                .rows()
                .iter()
                .all(|row| row.id % 2 == 0 && row.graph_score.is_some())
        ),
        13 => {
            let active = mt
                .all_node_ids()
                .into_iter()
                .filter(|id| id % 2 == 0)
                .collect::<Vec<_>>();
            assert_eq!(
                BTreeSet::from_iter(ids(actual)),
                reference_expand(mt, &active)
            );
        }
        14 => assert_eq!(
            BTreeSet::from_iter(ids(actual)),
            reference_expand_depth(mt, 1, 4)
        ),
        _ => unreachable!(),
    }
}

fn reference_expand_depth(mt: &MemTable<f32>, source: u64, depth: usize) -> BTreeSet<u64> {
    let config = ReachabilityConfig {
        min_depth: 0,
        max_depth: depth,
        labels: None,
        direction: ReachabilityDirection::Outgoing,
        max_visited_nodes: 10_000,
        max_results: 10_000,
        max_edges: 100_000,
        max_frontier_size: 10_000,
        exhaustion_policy: BudgetExhaustionPolicy::Error,
    };
    traverse_compact(mt, source, &config)
        .unwrap()
        .results
        .into_iter()
        .map(|hit| hit.target_id)
        .collect()
}

#[test]
fn q1_到_q14_独立_reference_与双物理计划全部一致() {
    let mt = graph();
    for family in 1..=14 {
        let pipelined = execute_family(&mt, family, false);
        let materialized = execute_family(&mt, family, true);
        assert_eq!(pipelined, materialized, "Q{family} 两个物理计划不一致");
        assert_reference(&mt, family, &pipelined);
    }
}
