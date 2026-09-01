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
    let directory = std::env::temp_dir().join("triviumdb_b2_explain");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("explain_{}", std::process::id()));
    let path = path.to_string_lossy().into_owned();
    for suffix in ["", ".vec", ".wal", ".lock", ".flush_ok", ".pidx"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let first = db.insert(&[0.0; DIM], json!({"type": "first"})).unwrap();
    let second = db.insert(&[0.0; DIM], json!({"type": "second"})).unwrap();
    db.link(first, second, "road", 1.0).unwrap();
    let rows = db
        .tql(&format!(
            "EXPLAIN ANALYZE MATCH (a {{id: {first}}})-[:road]->(b) RETURN b"
        ))
        .unwrap();
    let plan = &rows[0]["plan"].payload;
    assert_eq!(plan["graph_stats"]["edge_count"], 1);
    assert_eq!(plan["graph_stats"]["label_stats"]["road"]["edge_count"], 1);
    assert_eq!(plan["traversal_budget"]["exhaustion_policy"], "error");
    assert_eq!(plan["analyze"], true);
    drop(db);
    for suffix in ["", ".vec", ".wal", ".lock", ".flush_ok", ".pidx"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

#[test]
fn 图统计在_link_unlink_delete_和重启后保持一致() {
    let directory = std::env::temp_dir().join("triviumdb_b2_stats");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("stats_{}", std::process::id()));
    let path = path.to_string_lossy().into_owned();
    for suffix in ["", ".vec", ".wal", ".lock", ".flush_ok", ".pidx"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let ids: Vec<_> = (0..4)
        .map(|value| db.insert(&[value as f32, 0.0], json!({})).unwrap())
        .collect();
    db.link(ids[0], ids[1], "a", 1.0).unwrap();
    db.link(ids[0], ids[2], "a", 1.0).unwrap();
    db.link(ids[2], ids[3], "b", 1.0).unwrap();
    let stats = db.graph_stats();
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
    assert_eq!(db.graph_stats().edge_count, 2);
    db.delete(ids[2]).unwrap();
    assert_eq!(db.graph_stats().edge_count, 0);
    db.flush().unwrap();
    drop(db);

    let reopened = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(reopened.graph_stats().edge_count, 0);
    assert_eq!(reopened.graph_stats().node_count, 3);
    drop(reopened);
    for suffix in ["", ".vec", ".wal", ".lock", ".flush_ok", ".pidx"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}
