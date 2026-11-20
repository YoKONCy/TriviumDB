use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::json;
use triviumdb::Database;
use triviumdb::error::TriviumError;
use triviumdb::graph::budget::{BudgetDimension, BudgetExhaustionPolicy, TraversalBudget};
use triviumdb::graph::pathfinding::shortest_path;
use triviumdb::graph::reachability::{ReachabilityConfig, traverse_detailed};
use triviumdb::storage::memtable::MemTable;

const DIM: usize = 2;

fn graph() -> MemTable<f32> {
    let mut mt = MemTable::new(DIM);
    for id in 1..=12 {
        mt.insert_with_id(id, &[id as f32, 0.0], json!({})).unwrap();
    }
    for (source, target, label) in [
        (1, 2, "road"),
        (1, 3, "road"),
        (2, 4, "road"),
        (3, 4, "road"),
        (4, 5, "road"),
        (5, 6, "road"),
        (2, 7, "noise"),
        (7, 8, "noise"),
        (8, 9, "noise"),
        (9, 10, "noise"),
        (10, 11, "noise"),
        (11, 12, "noise"),
        (6, 6, "self"),
    ] {
        mt.link(source, target, label.into(), 1.0).unwrap();
    }
    mt
}

fn generous_budget() -> TraversalBudget {
    TraversalBudget {
        max_visited_nodes: 100,
        max_examined_edges: 100,
        max_frontier_size: 100,
        max_depth: 10,
        exhaustion_policy: BudgetExhaustionPolicy::Error,
    }
}

#[test]
fn 双向部分结果保留耗尽预算前找到的路径() {
    let mut mt = triviumdb::storage::memtable::MemTable::<f32>::new(2);
    for id in 1..=4 {
        mt.insert_with_id(id, &[1.0, 0.0], serde_json::Value::Null)
            .unwrap();
    }
    for dst in 2..=4 {
        mt.link(1, dst, "edge".into(), 1.0).unwrap();
    }
    for (nodes, edges) in [(3, 8), (8, 1)] {
        let budget = triviumdb::graph::budget::TraversalBudget {
            max_visited_nodes: nodes,
            max_examined_edges: edges,
            exhaustion_policy: triviumdb::graph::budget::BudgetExhaustionPolicy::Partial,
            ..Default::default()
        };
        let result =
            triviumdb::graph::pathfinding::shortest_path_bidirectional(&mt, 1, 2, None, &budget)
                .unwrap();
        assert!(result.truncated);
        assert_eq!(result.path, Some(vec![1, 2]));
    }
}

#[test]
fn 双向初始状态和扩展前沿均在预算内准入() {
    let mt = graph();
    for (dimension, budget) in [
        (
            BudgetDimension::VisitedNodes,
            TraversalBudget {
                max_visited_nodes: 1,
                ..generous_budget()
            },
        ),
        (
            BudgetDimension::FrontierSize,
            TraversalBudget {
                max_frontier_size: 1,
                ..generous_budget()
            },
        ),
        (
            BudgetDimension::FrontierSize,
            TraversalBudget {
                max_frontier_size: 2,
                ..generous_budget()
            },
        ),
    ] {
        let error =
            triviumdb::graph::pathfinding::shortest_path_bidirectional(&mt, 1, 6, None, &budget)
                .unwrap_err();
        assert!(
            matches!(error, TriviumError::TraversalBudgetExceeded { dimension: actual, .. } if actual == dimension),
            "{error:?}"
        );
        let partial = TraversalBudget {
            exhaustion_policy: BudgetExhaustionPolicy::Partial,
            ..budget
        };
        let result =
            triviumdb::graph::pathfinding::shortest_path_bidirectional(&mt, 1, 6, None, &partial)
                .unwrap();
        assert_eq!(result.path, None);
        assert!(result.truncated);
        assert!(result.bidirectional);
        assert!(result.metrics.visited_nodes <= budget.max_visited_nodes);
        assert!(result.metrics.peak_frontier_size <= budget.max_frontier_size);
        assert!(result.metrics.examined_edges <= budget.max_examined_edges);
    }
    let budget = TraversalBudget {
        max_visited_nodes: 1,
        max_frontier_size: 1,
        ..generous_budget()
    };
    let same = triviumdb::graph::pathfinding::shortest_path_bidirectional(&mt, 1, 1, None, &budget)
        .unwrap();
    assert_eq!(same.path, Some(vec![1]));
    assert_eq!(same.metrics.visited_nodes, 1);
    assert_eq!(same.metrics.peak_frontier_size, 1);
    assert!(!same.truncated);
}

#[test]
fn 最短路径只对实际存在的端点返回自身路径() {
    let mt = graph();
    for depth in [0, 1, 10] {
        assert_eq!(shortest_path(&mt, 999, 999, depth, None), None);
        assert_eq!(shortest_path(&mt, 1, 999, depth, None), None);
        assert_eq!(shortest_path(&mt, 999, 1, depth, None), None);
        assert_eq!(shortest_path(&mt, 1, 1, depth, None), Some(vec![1]));
    }
}

#[test]
fn 双向路径按实际扫描的平行边计量预算() {
    let mut mt = MemTable::<f32>::new(DIM);
    for id in 1..=3 {
        mt.insert_with_id(id, &[1., 0.], json!({})).unwrap();
    }
    for label in ["a", "b", "c"] {
        mt.link(1, 2, label.into(), 1.).unwrap();
    }
    let budget = TraversalBudget {
        max_examined_edges: 1,
        ..generous_budget()
    };
    let error =
        triviumdb::graph::pathfinding::shortest_path_bidirectional(&mt, 1, 3, Some("c"), &budget)
            .unwrap_err();
    assert!(matches!(
        error,
        TriviumError::TraversalBudgetExceeded {
            dimension: BudgetDimension::ExaminedEdges,
            examined_edges: 1,
            visited_nodes: 2,
            ..
        }
    ));
}

#[test]
fn 双向_bfs_与单向_bfs最短长度一致且路径确定() {
    let mt = graph();
    let expected = shortest_path(&mt, 1, 6, 10, Some("road")).unwrap();
    for _ in 0..20 {
        let output = triviumdb::graph::pathfinding::shortest_path_bidirectional(
            &mt,
            1,
            6,
            Some("road"),
            &generous_budget(),
        )
        .unwrap();
        assert_eq!(output.path, Some(expected.clone()));
        assert_eq!(output.path, Some(vec![1, 2, 4, 5, 6]));
        assert!(!output.truncated);
        assert!(output.bidirectional);
        assert!(output.metrics.examined_edges > 0);
    }
}

fn differential_budget() -> TraversalBudget {
    TraversalBudget {
        max_visited_nodes: 1_000,
        max_examined_edges: 10_000,
        max_frontier_size: 1_000,
        max_depth: 10,
        exhaustion_policy: BudgetExhaustionPolicy::Error,
    }
}

#[test]
fn 双向_bfs_随机图与单向_bfs最短长度差分一致() {
    let mut rng = StdRng::seed_from_u64(0xB2D1_FF3E);
    for graph_index in 0..20 {
        let mut mt = MemTable::new(DIM);
        for id in 1..=80 {
            mt.insert_with_id(id, &[id as f32, 0.0], json!({})).unwrap();
        }
        for _ in 0..400 {
            let source = rng.gen_range(1..=80);
            let target = rng.gen_range(1..=80);
            mt.link(source, target, "edge".into(), 1.0).unwrap();
        }
        for pair_index in 0..40 {
            let source = rng.gen_range(1..=80);
            let target = rng.gen_range(1..=80);
            let expected = shortest_path(&mt, source, target, 10, Some("edge"));
            let actual = triviumdb::graph::pathfinding::shortest_path_bidirectional(
                &mt,
                source,
                target,
                Some("edge"),
                &differential_budget(),
            )
            .unwrap()
            .path;
            assert_eq!(
                actual.as_ref().map(Vec::len),
                expected.as_ref().map(Vec::len),
                "graph={graph_index}, pair={pair_index}, {source}->{target}"
            );
            if let Some(path) = actual {
                assert_eq!(path.first(), Some(&source));
                assert_eq!(path.last(), Some(&target));
                assert!(
                    path.windows(2)
                        .all(|edge| mt.get_edge(edge[0], edge[1], "edge").is_some())
                );
            }
        }
    }
}

#[test]
fn 双向_bfs_处理不可达_零深度_自环和标签过滤() {
    let mt = graph();
    let same = triviumdb::graph::pathfinding::shortest_path_bidirectional(
        &mt,
        6,
        6,
        None,
        &generous_budget(),
    )
    .unwrap();
    assert_eq!(same.path, Some(vec![6]));

    let unreachable = triviumdb::graph::pathfinding::shortest_path_bidirectional(
        &mt,
        6,
        1,
        None,
        &generous_budget(),
    )
    .unwrap();
    assert_eq!(unreachable.path, None);

    let filtered = triviumdb::graph::pathfinding::shortest_path_bidirectional(
        &mt,
        1,
        12,
        Some("road"),
        &generous_budget(),
    )
    .unwrap();
    assert_eq!(filtered.path, None);
}

#[test]
fn 双向_bfs_预算默认报错且_partial显式截断() {
    let mt = graph();
    let error_budget = TraversalBudget {
        max_examined_edges: 1,
        ..generous_budget()
    };
    assert!(matches!(
        triviumdb::graph::pathfinding::shortest_path_bidirectional(&mt, 1, 12, None, &error_budget,),
        Err(TriviumError::TraversalBudgetExceeded {
            dimension: BudgetDimension::ExaminedEdges,
            ..
        })
    ));

    let partial_budget = TraversalBudget {
        exhaustion_policy: BudgetExhaustionPolicy::Partial,
        ..error_budget
    };
    let output = triviumdb::graph::pathfinding::shortest_path_bidirectional(
        &mt,
        1,
        12,
        None,
        &partial_budget,
    )
    .unwrap();
    assert!(output.truncated);
    assert_eq!(output.path, None);
    assert_eq!(output.metrics.examined_edges, 1);
}

#[test]
fn reachability_四维预算和指标全部生效() {
    let mt = graph();
    let error = traverse_detailed(
        &mt,
        1,
        &ReachabilityConfig {
            max_depth: 10,
            max_visited_nodes: 100,
            max_results: 100,
            max_edges: 1,
            max_frontier_size: 100,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
            ..Default::default()
        },
    );
    assert!(matches!(
        error,
        Err(TriviumError::TraversalBudgetExceeded {
            dimension: BudgetDimension::ExaminedEdges,
            ..
        })
    ));

    let output = traverse_detailed(
        &mt,
        1,
        &ReachabilityConfig {
            max_depth: 10,
            max_visited_nodes: 100,
            max_results: 100,
            max_edges: 100,
            max_frontier_size: 1,
            exhaustion_policy: BudgetExhaustionPolicy::Partial,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(output.truncated);
    assert!(output.peak_frontier_size >= 1);
    assert!(output.depth_reached >= 1);
    assert!(output.visited_nodes >= 2);
    assert!(output.traversed_edges >= 1);
}

#[test]
fn explain_analyze_暴露图统计和遍历预算() {
    let directory = crate::support::TestDir::new("graph_explain");
    let path = directory.file("database.tdb");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let first = db.insert(&[0.0; DIM], json!({"type": "first"})).unwrap();
    let second = db.insert(&[0.0; DIM], json!({"type": "second"})).unwrap();
    db.link(first, second, "road", 1.0).unwrap();
    let rows = db
        .tql_nodes(&format!(
            "EXPLAIN ANALYZE MATCH (a {{id: {first}}})-[:road]->(b) RETURN b"
        ))
        .unwrap();
    let plan = &rows[0]["plan"].payload;
    assert_eq!(plan["graph_stats"]["edge_count"], 1);
    assert_eq!(plan["graph_stats"]["label_stats"]["road"]["edge_count"], 1);
    assert_eq!(plan["traversal_budget"]["exhaustion_policy"], "error");
    assert_eq!(plan["analyze"], true);
    drop(db);
}

#[test]
fn 图统计在_link_unlink_delete_和重启后保持一致() {
    let directory = crate::support::TestDir::new("graph_stats");
    let path = directory.file("database.tdb");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let ids: Vec<_> = (0..4)
        .map(|value| db.insert(&[value as f32, 0.0], json!({})).unwrap())
        .collect();
    db.link(ids[0], ids[1], "a", 1.0).unwrap();
    db.link(ids[0], ids[2], "a", 1.0).unwrap();
    db.link(ids[2], ids[3], "b", 1.0).unwrap();
    let stats = db.graph_stats().unwrap();
    assert_eq!(stats.node_count, 4);
    assert_eq!(stats.edge_count, 3);
    assert_eq!(stats.avg_out_degree, 0.75);
    assert_eq!(stats.max_out_degree, 2);
    assert_eq!(stats.max_in_degree, 1);
    assert_eq!(stats.label_stats["a"].edge_count, 2);
    assert_eq!(stats.label_stats["a"].distinct_source_count, 1);
    assert_eq!(
        stats
            .out_degree_histogram
            .iter()
            .map(|b| b.node_count)
            .sum::<usize>(),
        4
    );

    db.unlink(ids[0], ids[1]).unwrap();
    assert_eq!(db.graph_stats().unwrap().edge_count, 2);
    db.delete(ids[2]).unwrap();
    assert_eq!(db.graph_stats().unwrap().edge_count, 0);
    db.flush().unwrap();
    drop(db);

    let reopened = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(reopened.graph_stats().unwrap().edge_count, 0);
    assert_eq!(reopened.graph_stats().unwrap().node_count, 3);
    drop(reopened);
}

#[test]
fn 公共图辅助函数拒绝虚构起点并尊重零结果上限() {
    use triviumdb::graph::pathfinding::{k_hop_neighbors, variable_length_paths};
    let mut mt = triviumdb::storage::memtable::MemTable::<f32>::new(1);
    assert!(k_hop_neighbors(&mt, 7, 0, None).is_empty());
    assert!(variable_length_paths(&mt, 7, 0, 0, None, 1).is_empty());
    mt.insert_with_id(7, &[1.], serde_json::json!({})).unwrap();
    assert!(variable_length_paths(&mt, 7, 0, 0, None, 0).is_empty());
    assert_eq!(variable_length_paths(&mt, 7, 0, 0, None, 1), [(7, vec![7])]);
    assert_eq!(
        k_hop_neighbors(&mt, 7, 0, None),
        std::collections::HashMap::from([(7, 0)])
    );
}
