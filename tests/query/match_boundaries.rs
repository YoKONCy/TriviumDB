//! 按物理边计行、显式路径栈、预算与运行中取消的独立样例。
use super::support::TestDir;
use serde_json::json;
use triviumdb::database::{Config, StorageMode};
use triviumdb::query::tql_executor::{QueryControl, TqlLimits, TqlValue, execute_tql_with_limits};
use triviumdb::query::tql_parser::parse_tql;
use triviumdb::storage::memtable::MemTable;
use triviumdb::{Database, TriviumError};

fn number(value: &TqlValue<f32>) -> u64 {
    match value {
        TqlValue::Int(value) => (*value).try_into().unwrap(),
        TqlValue::UInt(value) => *value,
        _ => panic!("应返回精确整数列: {value:?}"),
    }
}

#[test]
fn 属性索引与反转计划保持平行边的完整行重数() {
    let contract: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/graph_query_boundaries.json")).unwrap();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("parallel_match");
        let path = root.file("data.tdb");
        let config = Config {
            dim: 1,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        for node in contract["nodes"].as_array().unwrap() {
            db.insert_with_id(
                node["id"].as_u64().unwrap(),
                &[1.],
                json!({"kind": node["kind"]}),
            )
            .unwrap();
        }
        for edge in contract["edges"].as_array().unwrap() {
            db.link(
                edge[0].as_u64().unwrap(),
                edge[1].as_u64().unwrap(),
                edge[2].as_str().unwrap(),
                1.,
            )
            .unwrap();
        }
        for phase in 0..3 {
            if phase == 1 {
                db.create_index("kind").unwrap();
            }
            if phase == 2 {
                db.close().unwrap();
                db = Database::open_read_only(&path, 1).unwrap();
            }
            for case in contract["counts"].as_array().unwrap() {
                let rows = db.tql(case["query"].as_str().unwrap()).unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(
                    number(&rows[0]["total"]),
                    case["expected"].as_u64().unwrap(),
                    "{mode:?}/{phase}/{case}"
                );
            }
        }
        db.close().unwrap();
    }
}

#[test]
fn 双向单跳自环只计一次且平行边分别计数() {
    let root = TestDir::new("self_loop_match");
    let mut db = Database::<f32>::open(&root.file("data.tdb"), 1).unwrap();
    db.insert_with_id(1, &[1.], json!({})).unwrap();
    for label in ["red", "blue"] {
        db.link(1, 1, label, 1.).unwrap();
    }
    for edge in ["-[]->", "<-[]-", "-[]-", "-[e]-", "-[:red|blue]-"] {
        let rows = db
            .tql(&format!(
                "MATCH (a {{id:1}}){edge}(b) RETURN COUNT(*) AS total"
            ))
            .unwrap();
        assert_eq!(number(&rows[0]["total"]), 2, "{edge}");
    }
    db.close().unwrap();
}

#[test]
fn 可变长路径恢复绑定并按每段简单路径处理环和零跳() {
    let mut mt = chain(3, false);
    mt.upsert_edge(3, 1, "next".into(), 1., json!({})).unwrap();
    let query = parse_tql("MATCH (a {id:1})-[:next*1..4]->(b) RETURN b").unwrap();
    let rows = execute_tql_with_limits(&query, &mt, TqlLimits::default()).unwrap();
    assert_eq!(
        rows.iter().map(|row| row["b"].id).collect::<Vec<_>>(),
        [2, 3, 4]
    );
    let query = parse_tql("MATCH (a {id:1})-[:next]->(b)-[:next*1..3]->(a) RETURN a, b").unwrap();
    let rows = execute_tql_with_limits(&query, &mt, TqlLimits::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0]["a"].id, rows[0]["b"].id), (1, 2));
    let query = parse_tql("MATCH (a {id:1})-[:next*0..0]->(b) RETURN b").unwrap();
    let rows = execute_tql_with_limits(&query, &mt, TqlLimits::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["b"].id, 1);
}

#[test]
fn 路径栈工作区在展开前受实例预算限制() {
    let mt = chain(54, false);
    let query = parse_tql("MATCH (a {id:1})-[:next*54..54]->(b) RETURN b").unwrap();
    let limits = TqlLimits {
        memory_limit: 4096,
        ..Default::default()
    };
    assert!(matches!(
        execute_tql_with_limits(&query, &mt, limits),
        Err(TriviumError::ResourceBudgetExceeded {
            resource: "query_memory_bytes",
            ..
        })
    ));
    let rows = execute_tql_with_limits(&query, &mt, TqlLimits::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["b"].id, 55);
}

fn chain(length: u64, branches: bool) -> MemTable<f32> {
    let mut mt = MemTable::new(1);
    mt.set_auto_build_quiver(false);
    for id in 1..=length + 1 {
        mt.insert_with_id(id, &[1.], json!({})).unwrap();
    }
    for id in 1..=length {
        mt.upsert_edge(id, id + 1, "next".into(), 1., json!({}))
            .unwrap();
        if branches && id < length {
            mt.upsert_edge(id, id + 2, "next".into(), 1., json!({}))
                .unwrap();
        }
    }
    mt
}

#[test]
fn 小栈子进程完成深路径和超时回归() {
    let root = TestDir::new("match_worker");
    let receipt = root.file("completed");
    let mut child = super::query_child_command()
        .args([
            "--ignored",
            "--exact",
            "match_boundaries::small_stack_worker",
            "--nocapture",
        ])
        .env("TDB_MATCH_RECEIPT", &receipt)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("路径子进程超过等待上限");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(status.success(), "路径子进程必须正常完成: {status}");
    assert_eq!(
        std::fs::read(receipt).unwrap(),
        b"match-boundaries-completed"
    );
}

#[test]
#[ignore = "由父用例在独立进程执行"]
fn small_stack_worker() {
    let receipt = std::env::var("TDB_MATCH_RECEIPT").unwrap();
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            // 12000 跳远高于原递归实现可在 2 MiB 栈上安全处理的深度。
            let mt = chain(12_000, false);
            let query = parse_tql("MATCH (a {id:1})-[:next*12000..12000]->(b) RETURN b").unwrap();
            let rows = execute_tql_with_limits(&query, &mt, TqlLimits::default()).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["b"].id, 12_001);
            // 普通多段模式也用显式栈，覆盖可变长路径之外的递归边界。
            // AST 公共入口也保持有界调用栈；字符串入口另有运算复杂度上限。
            use triviumdb::query::tql_ast::{
                EdgeDirection, QueryEntry, ReturnClause, TqlEdgePattern, TqlNodePattern,
            };
            let mut query = parse_tql("MATCH (n0 {id:1}) RETURN n0").unwrap();
            let QueryEntry::Match { pattern } = &mut query.entry else {
                panic!("应为 MATCH")
            };
            for id in 1..=12_000 {
                pattern.nodes.push(TqlNodePattern {
                    var: Some(format!("n{id}")),
                    filter: None,
                });
                pattern.edges.push(TqlEdgePattern {
                    var: None,
                    labels: vec!["next".into()],
                    hop_range: None,
                    direction: EdgeDirection::Forward,
                });
            }
            query.returns = ReturnClause::Variables(vec!["n12000".into()]);
            let rows = execute_tql_with_limits(&query, &mt, TqlLimits::default()).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["n12000"].id, 12_001);

            // 55 节点的跳一/跳二 DAG 有指数级路径；最小跳数之前也必须消耗预算。
            let mt = chain(54, true);
            let query = parse_tql("MATCH (a {id:1})-[:next*54..54]->(b) RETURN b").unwrap();
            let started = std::time::Instant::now();
            let limits = TqlLimits {
                query_control: Some(QueryControl::with_deadline(
                    started + std::time::Duration::from_millis(100),
                )),
                ..Default::default()
            };
            assert!(matches!(
                execute_tql_with_limits(&query, &mt, limits),
                Err(TriviumError::QueryCancelled | TriviumError::QueryExecution(_))
            ));
            assert!(
                started.elapsed() < std::time::Duration::from_secs(2),
                "路径期限或步数预算必须及时终止展开"
            );
            let started = std::time::Instant::now();
            assert!(matches!(
                execute_tql_with_limits(&query, &mt, TqlLimits::default()),
                Err(TriviumError::QueryExecution(_))
            ));
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
        })
        .unwrap()
        .join()
        .unwrap();
    std::fs::write(receipt, b"match-boundaries-completed").unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 路径已经开始展开后取消和期限耗尽仍会终止执行() {
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
    for deadline in [false, true] {
        let mt = chain(54, true);
        let query = parse_tql("MATCH (a {id:1})-[:next*54..54]->(b) RETURN b").unwrap();
        let expiry = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let control = if deadline {
            QueryControl::with_deadline(expiry)
        } else {
            QueryControl::default()
        };
        let worker_control = control.clone();
        let gate = pause_at(ConcurrencyPoint::TqlPathStep);
        let worker = std::thread::spawn(move || {
            execute_tql_with_limits(
                &query,
                &mt,
                TqlLimits {
                    query_control: Some(worker_control),
                    ..Default::default()
                },
            )
        });
        gate.wait_until_arrived();
        if deadline {
            // 屏障证明执行已进入路径展开；时钟等待只用于到达真实期限。
            while std::time::Instant::now() < expiry {
                std::thread::sleep(expiry.saturating_duration_since(std::time::Instant::now()));
            }
        } else {
            control.cancel();
        }
        gate.release();
        assert!(matches!(
            worker.join().unwrap(),
            Err(TriviumError::QueryCancelled)
        ));
    }
}
