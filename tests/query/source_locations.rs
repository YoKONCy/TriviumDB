//! AST 源码树与规划、执行诊断的独立契约。

use serde_json::{Value, json};
use triviumdb::database::{Config, RowOverflowPolicy};
use triviumdb::query::tql_parser::{parse_tql_located, parse_tql_statement_located};
use triviumdb::query::tql_source::{TqlSource, TqlSourceNode};
use triviumdb::{Database, TriviumError};

fn text<'a>(source: &'a TqlSource, node: &TqlSourceNode) -> &'a str {
    let range = node.range.clone().expect("该显式节点必须包含源码范围");
    &source.input()[range]
}

fn child<'a>(node: &'a TqlSourceNode, kind: &str, index: usize) -> &'a TqlSourceNode {
    node.child(kind, index)
        .unwrap_or_else(|| panic!("缺少 {kind}[{index}]，实际节点 {node:?}"))
}

#[test]
fn binary_predicate_and_projection_trees_retain_all_child_ranges() {
    let parsed = parse_tql_located("MATCH (n) WHERE (n.x == 1 OR n.x == 2) AND NOT n.y == 3 RETURN 1 + 2 * 3 AS total, sum(n.x) AS sum_value, count(*) AS count_value ORDER BY total DESC LIMIT 2 OFFSET 1").unwrap();
    let source = parsed.source();
    let predicate = child(&source.root, "predicate", 0);
    assert_eq!(
        text(source, child(predicate, "predicate", 0)),
        "(n.x == 1 OR n.x == 2)"
    );
    assert_eq!(
        text(source, child(predicate, "predicate", 1)),
        "NOT n.y == 3"
    );
    let item = child(child(&source.root, "return", 0), "return_item", 0);
    let expr = child(child(item, "return_kind", 0), "expression", 0);
    assert_eq!(text(source, expr), "1 + 2 * 3");
    assert_eq!(text(source, child(expr, "expression", 0)), "1");
    let product = child(expr, "expression", 1);
    assert_eq!(text(source, product), "2 * 3");
    assert_eq!(text(source, child(product, "expression", 0)), "2");
    assert_eq!(text(source, child(product, "expression", 1)), "3");
    let aggregate = child(
        child(child(&source.root, "return", 0), "return_item", 1),
        "return_kind",
        0,
    );
    assert_eq!(text(source, aggregate), "sum(n.x)");
    assert_eq!(text(source, child(aggregate, "return_kind", 0)), "n.x");
    let count = child(
        child(child(&source.root, "return", 0), "return_item", 2),
        "return_kind",
        0,
    );
    assert_eq!(text(source, child(count, "return_kind", 0)), "*");
    assert_eq!(text(source, child(&source.root, "order", 0)), "total DESC");
    assert_eq!(text(source, child(&source.root, "limit", 0)), "LIMIT 2");
    assert_eq!(text(source, child(&source.root, "offset", 0)), "OFFSET 1");
}

#[test]
fn every_pipeline_stage_keeps_its_scope_and_complete_range() {
    let cases = [
        ("EXPAND seed [:related*1..2] AS out", "out"),
        (
            "pagerank seed MODE EXPAND HOPS 2 BOTH LABELS [related] AS out",
            "out",
        ),
        (
            "sa_ppr_config seed depth 2 alpha 0.15 max_edges 8 min_weight 0 AS out",
            "out",
        ),
        ("diversify seed TOP 2 quality_weight 1 AS out", "out"),
        (
            "residual seed BY VECTOR [0, 1] TOP 2 lambda 0.1 threshold 0 iterations 8 AS out",
            "out",
        ),
        ("topics seed k 2 iterations 8 AS out", "out"),
        (
            "all_paths seed to [8] depth 3 paths 8 aggregate max_product AS out",
            "out",
        ),
        ("shortest_paths seed TO [8] LABEL related AS out", "out"),
        ("weighted_paths seed TO [8] LABEL related AS out", "out"),
        ("yen_paths seed TO [8] K 3 LABEL related AS out", "out"),
        (
            "node_similarity seed TOP 4 CUTOFF 0.3 LABEL related AS out",
            "out",
        ),
        ("union seed IDS [8] AS out", "out"),
        (
            "iterate seed EXPAND [:related*1..1] times 4 fixed AS out",
            "out",
        ),
        ("RANK seed BY VECTOR [1, 0] TOP 1", "seed"),
        ("WHERE seed.active == true", "seed"),
    ];
    for (stage, output) in cases {
        let query = format!(
            "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed {stage} WITH {output} RETURN {output}"
        );
        let parsed = parse_tql_located(&query).unwrap_or_else(|error| panic!("{query}: {error:?}"));
        let source = parsed.source();
        let pipeline = child(&source.root, "pipeline", 0);
        let actual = pipeline
            .children
            .iter()
            .map(|node| text(source, node))
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            vec!["AS seed", "WITH seed", stage, &format!("WITH {output}")],
            "{query}"
        );
        let synthetic = child(
            child(child(pipeline, "stage", 0), "with_item", 0),
            "expression",
            0,
        );
        assert!(synthetic.range.is_none());
    }
}

#[test]
fn mutations_keep_source_actions_assignments_payloads_and_created_entities() {
    let parsed =
        parse_tql_statement_located("MATCH (n) WHERE n.id == 7 SET n.x == [1, '中'], n.y == true")
            .unwrap();
    let source = parsed.source();
    let mutation = child(&source.root, "mutation", 0);
    assert_eq!(
        text(source, child(mutation, "mutation_source", 0)),
        "MATCH (n) WHERE n.id == 7"
    );
    let action = child(mutation, "action", 0);
    let assignments = child(action, "assignments", 0);
    assert_eq!(
        text(source, child(assignments, "assignment", 0)),
        "n.x == [1, '中']"
    );
    assert_eq!(
        text(source, child(assignments, "assignment", 1)),
        "n.y == true"
    );
    let parsed = parse_tql_statement_located("CREATE (a {x: 1})-[:related]->(b {x: 2})").unwrap();
    let source = parsed.source();
    let create = child(
        child(child(&source.root, "mutation", 0), "action", 0),
        "create",
        0,
    );
    assert_eq!(text(source, child(create, "create_node", 0)), "(a {x: 1})");
    let edge = child(create, "create_edge", 0);
    assert_eq!(text(source, edge), "-[:related]->(b {x: 2})");
    assert_eq!(text(source, child(edge, "create_node", 0)), "(b {x: 2})");
    let parsed = parse_tql_statement_located("CREATE (a)").unwrap();
    let create = child(
        child(child(&parsed.source().root, "mutation", 0), "action", 0),
        "create",
        0,
    );
    assert!(
        child(child(create, "create_node", 0), "payload", 0)
            .range
            .is_none()
    );
}

fn verify_error(error: TriviumError, case: &Value) {
    let info = serde_json::to_value(error.info()).unwrap();
    assert_eq!(info["code"], case["code"], "{case}: {info}");
    assert_eq!(info["stage"], case["stage"], "{case}: {info}");
    assert_eq!(
        info["target"],
        json!({"query_span":case["span"]}),
        "{case}: {info}"
    );
    assert_eq!(info["schema_version"], 1);
    if case["mutation"] == true {
        assert_eq!(info["write"], case["write"], "{case}: {info}");
    } else {
        assert!(info.get("write").is_none());
    }
}

#[test]
fn shared_plan_execute_locations_preserve_data_and_prepared_sources() {
    let cases: Value = serde_json::from_str(include_str!(
        "../contracts/query_execution_diagnostics.json"
    ))
    .unwrap();
    let root = crate::support::TestDir::new("query_execution_sources");
    let path = root.file("db.tdb");
    let config = Config {
        dim: 2,
        auto_build_quiver: false,
        max_query_rows: Some(1),
        row_overflow: RowOverflowPolicy::Throw,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    db.insert_with_id(7, &[1., 0.], json!({"kind":"alpha"}))
        .unwrap();
    db.insert_with_id(8, &[0., 1.], json!({"kind":"beta"}))
        .unwrap();
    db.create_unique_index("kind").unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let query = case["query"].as_str().unwrap();
        if case["mutation"] == true {
            verify_error(db.tql_mut(query).unwrap_err(), case);
        } else {
            verify_error(db.tql(query).unwrap_err(), case);
            verify_error(db.tql_nodes(query).unwrap_err(), case);
            if case["prepare_error"] == true {
                verify_error(db.prepare_tql(query).unwrap_err(), case);
            } else {
                let prepared = db.prepare_tql(query).unwrap();
                verify_error(
                    db.execute_prepared_tql(&prepared, &Default::default())
                        .unwrap_err(),
                    case,
                );
            }
        }
        assert_eq!(
            std::fs::read(format!("{path}.wal")).unwrap(),
            before,
            "{case}"
        );
    }
    db.close().unwrap();
    let mut reopened = Database::<f32>::open_with_config(&path, config).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), vec![7, 8]);
    for (id, vector, kind) in [(7, vec![1., 0.], "alpha"), (8, vec![0., 1.], "beta")] {
        let node = reopened.get(id).unwrap().unwrap();
        assert_eq!(node.vector, vector);
        assert_eq!(node.payload, json!({"kind":kind}));
        assert!(node.edges.is_empty());
    }
    reopened.close().unwrap();
}

#[test]
fn cancellation_and_raw_ast_report_only_known_source_facts() {
    use triviumdb::query::tql_executor::{QueryControl, TqlLimits, execute_tql_values_with_limits};
    use triviumdb::query::tql_parser::parse_tql;
    use triviumdb::storage::memtable::MemTable;
    let root = crate::support::TestDir::new("query_source_cancel");
    let mut db = Database::<f32>::open(&root.file("db.tdb"), 2).unwrap();
    let control = QueryControl::default();
    control.cancel();
    let info = serde_json::to_value(
        db.tql_with_control("MATCH (n) RETURN n", control)
            .unwrap_err()
            .info(),
    )
    .unwrap();
    assert_eq!(info["code"], "TDB_QUERY_CANCELLED");
    assert_eq!(info["stage"], "execute");
    assert_eq!(
        info["target"],
        json!({"query_span":{"start_byte":0,"end_byte":18,"line":1,"column":1}})
    );
    let info =
        serde_json::to_value(db.tql("MATCH (n) RETURN $x AS value").unwrap_err().info()).unwrap();
    assert_eq!(info["code"], "TDB_QUERY_EXECUTION");
    assert_eq!(info["stage"], "bind");
    assert_eq!(
        info["target"],
        json!({"parameter":"x","query_span":{"start_byte":17,"end_byte":19,"line":1,"column":18}})
    );
    let query = parse_tql("SEARCH VECTOR [1] TOP 1 RETURN *").unwrap();
    let mt = MemTable::<f32>::new(2);
    let info = serde_json::to_value(
        execute_tql_values_with_limits(&query, &mt, TqlLimits::default())
            .unwrap_err()
            .info(),
    )
    .unwrap();
    assert_eq!(info["code"], "TDB_DIMENSION_MISMATCH");
    assert_eq!(info["stage"], "execute");
    assert!(info.get("target").is_none());
    db.close().unwrap();
}

#[test]
fn empty_matches_reject_unbound_rank_and_mutation_targets_before_writing() {
    let root = crate::support::TestDir::new("query_source_empty_bindings");
    let path = root.file("db.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    for (start, action) in [
        (14, "SET missing.x == 1"),
        (10, "SET VECTOR(missing) == [1, 0]"),
        (10, "DELETE missing"),
    ] {
        let query = format!("MATCH (n) {action}");
        let info = serde_json::to_value(db.tql_mut(&query).unwrap_err().info()).unwrap();
        assert_eq!(info["code"], "TDB_QUERY_EXECUTION");
        assert_eq!(info["stage"], "plan");
        assert_eq!(
            info["target"],
            json!({"query_span":{
            "start_byte":start,"end_byte":query.len(),"line":1,"column":start + 1
            }})
        );
        assert_eq!(
            info["write"],
            json!({"phase":"validation","outcome":"not_committed"})
        );
    }
    let query = "MATCH (n) RANK missing BY VECTOR [1, 0] TOP 1 RETURN n";
    let info = serde_json::to_value(db.tql(query).unwrap_err().info()).unwrap();
    assert_eq!(info["code"], "TDB_QUERY_PARSE");
    assert_eq!(info["stage"], "plan");
    assert_eq!(
        info["target"],
        json!({"query_span":{
        "start_byte":10,"end_byte":45,"line":1,"column":11
        }})
    );
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert!(db.all_node_ids().unwrap().is_empty());
    let head = "MATCH (n)-[e]->(m) ";
    let rank = "RANK e BY VECTOR [1, 0] TOP 1";
    let info = serde_json::to_value(
        db.tql(&format!("{head}{rank} RETURN n"))
            .unwrap_err()
            .info(),
    )
    .unwrap();
    assert_eq!(info["code"], "TDB_QUERY_PARSE");
    assert_eq!(info["stage"], "plan");
    assert_eq!(
        info["target"],
        json!({"query_span":{
            "start_byte":head.len(),"end_byte":head.len() + rank.len(),"line":1,"column":head.len() + 1
        }})
    );
    db.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn fused_and_split_operators_keep_their_actual_logical_origins() {
    use triviumdb::test_hooks::{QueryExecutionStrategy, force_query_strategy};
    let root = crate::support::TestDir::new("query_source_fusion");
    let path = root.file("db.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    db.insert_with_id(7, &[1., 0.], json!({"kind":"alpha"}))
        .unwrap();
    db.insert_with_id(8, &[0., 1.], json!({"kind":"beta"}))
        .unwrap();
    db.link(7, 8, "related", 1.).unwrap();
    let head = "FIND {kind: 'alpha'} AS seed WITH seed ";
    let expand = "EXPAND seed [:related*1..1] AS hop";
    let rank = "RANK hop BY VECTOR [1] TOP 1";
    let query = format!("{head}{expand} {rank} RETURN hop");
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    for (strategy, start) in [
        (QueryExecutionStrategy::Auto, head.len()),
        (
            QueryExecutionStrategy::DisableFusion,
            head.len() + expand.len() + 1,
        ),
    ] {
        let _guard = force_query_strategy(strategy);
        let error = db
            .tql(&query)
            .expect_err(&format!("策略 {strategy:?} 必须拒绝错误维度"));
        assert!(matches!(
            error.underlying(),
            TriviumError::DimensionMismatch {
                expected: 2,
                got: 1
            }
        ));
        let info = serde_json::to_value(error.info()).unwrap();
        assert_eq!(info["stage"], "execute");
        assert_eq!(
            info["target"],
            json!({"query_span":{
                "start_byte":start,"end_byte":head.len() + expand.len() + 1 + rank.len(),"line":1,"column":start + 1
            }})
        );
        assert!(info.get("write").is_none());
    }
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert_eq!(db.all_node_ids().unwrap(), vec![7, 8]);
    let edge = db.get_edge(7, 8, "related").unwrap().unwrap();
    assert_eq!(edge.weight, 1.);
    db.close().unwrap();
}
