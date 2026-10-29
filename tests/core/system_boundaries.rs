//! 共享契约的入口、状态和组合操作矩阵。
use super::support::TestDir;
use serde_json::json;
use triviumdb::database::{Config, StorageMode};
use triviumdb::filter::Filter;
use triviumdb::graph::reachability::ReachabilityConfig;
use triviumdb::{Database, ResourceLimits, ResourceRuntime, TriviumError};

fn build(mode: StorageMode, path: &str, runtime: ResourceRuntime) -> Database<f32> {
    let mut db = Database::open_with_runtime(
        path,
        Config {
            dim: 2,
            storage_mode: mode,
            auto_build_quiver: false,
            memory_limit: 2 * 1024 * 1024,
            ..Default::default()
        },
        runtime,
    )
    .unwrap();
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/system_boundaries.json")).unwrap();
    for id in fixture["nodes"].as_array().unwrap() {
        db.insert_with_id(id.as_u64().unwrap(), &[1., 0.], json!({"id":id}))
            .unwrap();
    }
    for edge in fixture["edges"].as_array().unwrap() {
        db.link(
            edge[0].as_u64().unwrap(),
            edge[1].as_u64().unwrap(),
            edge[2].as_str().unwrap(),
            1.,
        )
        .unwrap();
    }
    db
}

fn limited() -> ResourceRuntime {
    ResourceRuntime::new(ResourceLimits {
        memory_limit: 16 * 1024 * 1024,
        maintenance_memory_limit: 2 * 1024 * 1024,
        max_threads: 1,
        ..Default::default()
    })
    .unwrap()
}
fn budget_error<T: std::fmt::Debug>(result: triviumdb::Result<T>) {
    assert!(
        matches!(result,Err(TriviumError::ResourceBudgetExceeded {resource:"memory_bytes",requested,used,limit}) if requested>0 && used==limit),
        "必须在共享工作区分配前拒绝: {result:?}"
    );
}

#[test]
fn graph_first在共享前台余额为零时拒绝且不读取payload() {
    let root = TestDir::new("graph_first_admission");
    let runtime = limited();
    let mut db = build(StorageMode::Mmap, &root.file("data.tdb"), runtime.clone());
    db.set_memory_limit(14 * 1024 * 1024).unwrap();
    assert_eq!(runtime.stats().available_foreground_bytes, Some(0));
    let before = db.payload_memory_stats();
    budget_error(db.search_exact(&[1., 0.], 1));
    budget_error(db.search_graph_first(&[1., 0.], &[1, 2, 3, 4], 1, 10));
    assert_eq!(
        db.payload_memory_stats().payload_lookups,
        before.payload_lookups
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    db.set_memory_limit(2 * 1024 * 1024).unwrap();
    let hits = db
        .search_graph_first(&[1., 0.], &[4, 3, 2, 1, 1], 1, 4)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        (hits[0].id, hits[0].score, hits[0].payload.clone()),
        (1, 1., json!({"id":1}))
    );
    db.close().unwrap();
}

#[test]
fn 可达性和子图在共享前台余额为零时拒绝并可恢复执行() {
    let root = TestDir::new("reachability_admission");
    let runtime = limited();
    let mut db = build(StorageMode::Mmap, &root.file("data.tdb"), runtime.clone());
    db.set_memory_limit(14 * 1024 * 1024).unwrap();
    let config = ReachabilityConfig::default();
    budget_error(db.reachable(1, &config));
    budget_error(db.reachable_detailed(1, &config));
    budget_error(db.query_subgraph(1, &config));
    budget_error(db.neighbors(1, 2));
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    db.set_memory_limit(2 * 1024 * 1024).unwrap();
    let hits = db.reachable(1, &config).unwrap();
    assert_eq!(
        hits.iter()
            .map(|hit| (hit.target_id, hit.depth))
            .collect::<Vec<_>>(),
        [(2, 1)]
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    db.close().unwrap();
}

#[test]
fn 少量可达节点的工作区随实际探索增长() {
    let root = TestDir::new("actual_frontier");
    let runtime = limited();
    let mut db = Database::<f32>::open_with_runtime(
        &root.file("data.tdb"),
        Config {
            dim: 2,
            memory_limit: 2 * 1024 * 1024,
            auto_build_quiver: false,
            ..Default::default()
        },
        runtime.clone(),
    )
    .unwrap();
    for id in 1..=2048 {
        db.insert_with_id(id, &[1., 0.], json!({})).unwrap();
    }
    db.link(1, 2, "rel", 1.).unwrap();
    db.set_memory_limit(14 * 1024 * 1024 - 16 * 1024).unwrap();
    assert_eq!(db.neighbors(1, 1).unwrap(), [2]);
    assert_eq!(
        db.reachable(1, &ReachabilityConfig::default())
            .unwrap()
            .iter()
            .map(|hit| hit.target_id)
            .collect::<Vec<_>>(),
        [2]
    );
    assert_eq!(
        db.query_subgraph(1, &ReachabilityConfig::default())
            .unwrap()
            .nodes
            .iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    db.set_memory_limit(2 * 1024 * 1024).unwrap();
    db.close().unwrap();
}

#[test]
fn rust范围入口与json范围保持完整整数比较() {
    let payload = json!({"n":9_007_199_254_740_993u64});
    assert!(Filter::gt("n", 9_007_199_254_740_992.).matches(&payload));
    assert!(!Filter::lte("n", 9_007_199_254_740_992.).matches(&payload));
}

#[test]
fn 范围入口在索引和重开前后保持独立数值真值表() {
    use triviumdb::query::tql_ast::QueryEntry;
    use triviumdb::query::tql_executor::TqlValue;
    use triviumdb::query::tql_parser::parse_tql;
    use triviumdb::query::tql_prepared::PreparedTql;
    let payloads = [
        json!(9_007_199_254_740_991u64),
        json!(9_007_199_254_740_992u64),
        json!(9_007_199_254_740_993u64),
        json!(9_007_199_254_740_994u64),
        json!(-9_007_199_254_740_993i64),
        json!(-9_007_199_254_740_992i64),
        json!(u64::MAX),
        json!(i64::MIN),
        json!(0.5),
        json!(0),
        json!(null),
        json!("text"),
    ];
    // 2^53 两侧的相邻整数、完整 i64/u64 边界及小数，预期直接列出节点集合。
    let cases = [
        (
            9_007_199_254_740_992.,
            [
                vec![3, 4, 7],
                vec![2, 3, 4, 7],
                vec![1, 5, 6, 8, 9, 10],
                vec![1, 2, 5, 6, 8, 9, 10],
            ],
        ),
        (
            -9_007_199_254_740_992.,
            [
                vec![1, 2, 3, 4, 7, 9, 10],
                vec![1, 2, 3, 4, 6, 7, 9, 10],
                vec![5, 8],
                vec![5, 6, 8],
            ],
        ),
        (
            0.5,
            [
                vec![1, 2, 3, 4, 7],
                vec![1, 2, 3, 4, 7, 9],
                vec![5, 6, 8, 10],
                vec![5, 6, 8, 9, 10],
            ],
        ),
        (
            18_446_744_073_709_551_616.,
            [vec![], vec![], (1..=10).collect(), (1..=10).collect()],
        ),
        (
            f64::INFINITY,
            [vec![], vec![], (1..=10).collect(), (1..=10).collect()],
        ),
        (
            f64::NEG_INFINITY,
            [(1..=10).collect(), (1..=10).collect(), vec![], vec![]],
        ),
        (f64::NAN, [vec![], vec![], vec![], vec![]]),
    ];
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("range_entry_matrix");
        let path = root.file("data.tdb");
        let config = Config {
            dim: 2,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        for (id, n) in payloads.iter().enumerate() {
            db.insert_with_id(id as u64 + 1, &[1., 0.], json!({"n":n}))
                .unwrap();
        }
        for phase in 0..3 {
            if phase == 1 {
                db.create_ordered_index("n").unwrap();
            }
            if phase == 2 {
                db.close().unwrap();
                db = Database::open_read_only(&path, 2).unwrap();
            }
            for (threshold, expected) in &cases {
                let filters = [
                    Filter::gt("n", *threshold),
                    Filter::gte("n", *threshold),
                    Filter::lt("n", *threshold),
                    Filter::lte("n", *threshold),
                ];
                for (index, filter) in filters.into_iter().enumerate() {
                    let mut query = parse_tql("FIND {n:0} RETURN _.id AS id").unwrap();
                    query.entry = QueryEntry::Find { filter };
                    let prepared = PreparedTql::from_query(query);
                    let rows = db
                        .execute_prepared_tql(&prepared, &Default::default())
                        .unwrap();
                    let mut ids = rows
                        .iter()
                        .map(|row| match row["id"] {
                            TqlValue::UInt(id) => id,
                            TqlValue::Int(id) => id.try_into().unwrap(),
                            _ => panic!("应返回整数ID"),
                        })
                        .collect::<Vec<_>>();
                    ids.sort_unstable();
                    assert_eq!(ids, expected[index], "{mode:?}/{phase}/{threshold}/{index}");
                    if threshold.is_finite() {
                        let op = ["$gt", "$gte", "$lt", "$lte"][index];
                        let filter = Filter::from_json(&json!({"n":{op:threshold}})).unwrap();
                        let ids = payloads
                            .iter()
                            .enumerate()
                            .filter(|(_, n)| filter.matches(&json!({"n":n})))
                            .map(|(id, _)| id as u64 + 1)
                            .collect::<Vec<_>>();
                        assert_eq!(ids, expected[index]);
                    }
                }
            }
        }
        db.close().unwrap();
    }
}

#[test]
fn graph_first只物化最终命中的payload并在中途拒绝后归还预算() {
    let root = TestDir::new("graph_first_hydration");
    let runtime = ResourceRuntime::new(ResourceLimits {
        memory_limit: 32 * 1024 * 1024,
        maintenance_memory_limit: 4 * 1024 * 1024,
        max_threads: 1,
        ..Default::default()
    })
    .unwrap();
    let mut db = Database::<f32>::open_with_runtime(
        &root.file("data.tdb"),
        Config {
            dim: 2,
            memory_limit: 8 * 1024 * 1024,
            payload_cache_bytes: 0,
            auto_build_quiver: false,
            ..Default::default()
        },
        runtime.clone(),
    )
    .unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"winner":true}))
        .unwrap();
    for id in 2..=4 {
        db.insert_with_id(id, &[-1., 0.], json!({"large":"x".repeat(512*1024)}))
            .unwrap();
    }
    db.set_memory_limit(28 * 1024 * 1024 - 512).unwrap();
    let before = db.payload_memory_stats();
    assert!(matches!(
        db.search_graph_first(&[1., 0.], &[1, 2, 3, 4], 1, 4),
        Err(TriviumError::ResourceBudgetExceeded { .. })
    ));
    assert_eq!(
        db.payload_memory_stats().payload_lookups,
        before.payload_lookups
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    db.set_memory_limit(28 * 1024 * 1024 - 4096).unwrap();
    let hits = db
        .search_graph_first(&[1., 0.], &[4, 3, 2, 1], 1, 4)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].payload, json!({"winner":true}));
    assert_eq!(
        db.payload_memory_stats().payload_lookups - before.payload_lookups,
        1
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    db.set_memory_limit(8 * 1024 * 1024).unwrap();
    db.close().unwrap();
}

#[test]
fn detach_delete原子删除相连集合并保留集合外事实() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../contracts/system_boundaries.json")).unwrap();
        for query in fixture["delete_queries"].as_array().unwrap() {
            let root = TestDir::new("detach_connected");
            let path = root.file("data.tdb");
            let mut db = build(
                mode,
                &path,
                ResourceRuntime::new(ResourceLimits::default()).unwrap(),
            );
            let result = db.tql_mut(query.as_str().unwrap()).unwrap();
            assert_eq!(result.affected, 2);
            assert_eq!(db.all_node_ids().unwrap(), [3, 4]);
            assert_eq!(db.get_payload(3).unwrap(), Some(json!({"id":3})));
            let edges = db.get_edges(3).unwrap();
            assert_eq!(edges.len(), 1);
            assert_eq!((edges[0].target_id, edges[0].label.as_str()), (4, "keep"));
            assert!(db.validate_graph().unwrap().valid);
            db.close().unwrap();
            let mut reopened = Database::<f32>::open(&path, 2).unwrap();
            assert_eq!(reopened.all_node_ids().unwrap(), [3, 4]);
            assert_eq!(reopened.get_edges(3).unwrap()[0].target_id, 4);
            reopened.close().unwrap();
        }
    }
}
