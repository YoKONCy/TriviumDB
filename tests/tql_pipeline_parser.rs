//! TQL 可组合 Pipeline 的 Parser/AST 契约测试。
//! 覆盖 WITH、EXPAND、集合、路径、图算法及非法作用域，防止语法接受与执行脱节。

use serde_json::json;
use triviumdb::query::tql_ast::{PipelineStage, ReturnExprKind, TqlExpr};
use triviumdb::query::tql_executor::{TqlValue, execute_tql, execute_tql_values};
use triviumdb::query::tql_parser::parse_tql;
use triviumdb::query::tql_prepared::{PreparedTql, TqlParamValue};
use triviumdb::storage::memtable::MemTable;

fn graph() -> MemTable<f32> {
    let mut mt = MemTable::new(2);
    mt.insert_with_id(1, &[1.0, 0.0], json!({"active": true}))
        .unwrap();
    mt.insert_with_id(2, &[0.9, 0.1], json!({"active": true}))
        .unwrap();
    mt.insert_with_id(3, &[0.0, 1.0], json!({"active": false}))
        .unwrap();
    mt.link(1, 2, "related".into(), 1.0).unwrap();
    mt.link(2, 3, "related".into(), 1.0).unwrap();
    mt
}

#[test]
fn with_链和一等分数表达式解析正确() {
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed EXPAND seed [:related*1..2] AS related WITH related, similarity(related) AS sim WHERE sim > 0.5 RETURN related, similarity(related) AS score ORDER BY similarity(related) DESC LIMIT 2",
    )
    .unwrap();
    assert!(
        query
            .pipeline
            .iter()
            .any(|stage| matches!(stage, PipelineStage::Expand(_)))
    );
    let PipelineStage::Filter(_) = query.pipeline.last().unwrap() else {
        panic!("最后阶段应为管线 WHERE");
    };
    let with = query
        .pipeline
        .iter()
        .rev()
        .find_map(|stage| match stage {
            PipelineStage::With(with) => Some(with),
            _ => None,
        })
        .unwrap();
    assert_eq!(with.items.len(), 2);
    assert!(matches!(with.items[1].expr, TqlExpr::Similarity { .. }));
    let triviumdb::query::tql_ast::ReturnClause::Expressions(items) = query.returns else {
        panic!("RETURN 应包含标量表达式");
    };
    assert!(matches!(
        items[1].kind,
        ReturnExprKind::Scalar(TqlExpr::Similarity { .. })
    ));
    let alias_query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed, similarity(seed) AS sim RETURN seed ORDER BY sim DESC",
    )
    .unwrap();
    assert_eq!(alias_query.order_by.len(), 1);
}

#[test]
fn expand_rank_融合端到端保持精确_topk() {
    let mt = graph();
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed EXPAND seed [:related*1..2] AS related RANK related BY VECTOR [1, 0] TOP 1 RETURN related, similarity(related) AS score",
    )
    .unwrap();
    let rows = execute_tql_values(&query, &mt).unwrap();
    assert_eq!(rows.len(), 1);
    let TqlValue::Node(node) = &rows[0]["related"] else {
        panic!("related 应为节点值");
    };
    assert_eq!(node.id, 2);
    assert!(matches!(rows[0]["score"], TqlValue::Float(score) if score > 0.9));
}

#[test]
fn with_作用域遮蔽和重复别名明确拒绝() {
    let dropped =
        parse_tql("SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed AS kept RETURN seed").unwrap_err();
    assert!(dropped.contains("作用域外"));

    let duplicate =
        parse_tql("SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed AS x, seed AS x RETURN x")
            .unwrap_err();
    assert!(duplicate.contains("重复定义"));

    let unknown =
        parse_tql("SEARCH VECTOR [1, 0] TOP 2 AS seed WITH missing RETURN missing").unwrap_err();
    assert!(unknown.contains("未定义变量"));
}

#[test]
fn with_标量必须显式_as_且函数变量受作用域校验() {
    assert!(
        parse_tql("SEARCH VECTOR [1, 0] TOP 2 AS seed WITH similarity(seed) RETURN seed").is_err()
    );
    assert!(parse_tql(
        "SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed WHERE similarity(missing) > 0.1 RETURN seed"
    )
    .is_err());
}

#[test]
fn search_with_expand_similarity_filter_端到端执行() {
    let mt = graph();
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed EXPAND seed [:related*1..2] AS related WITH related WHERE similarity(related) > 0.6 RETURN related, similarity(related) AS score ORDER BY similarity(related) DESC LIMIT 2",
    )
    .unwrap();
    let rows = execute_tql_values(&query, &mt).unwrap();
    assert_eq!(rows.len(), 1);
    let TqlValue::Node(node) = &rows[0]["related"] else {
        panic!("related 应为节点值");
    };
    assert_eq!(node.id, 2);
    assert!(matches!(rows[0]["score"], TqlValue::Float(score) if score > 0.9));
    assert!(
        !node
            .payload
            .as_object()
            .unwrap()
            .contains_key("__tql_similarity")
    );
}

#[test]
fn 标量返回和别名排序不污染_payload() {
    let mt = graph();
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 3 AS node WITH node, similarity(node) AS sim RETURN node, sim, similarity(node) AS score ORDER BY sim ASC LIMIT 2",
    )
    .unwrap();
    let rows = execute_tql_values(&query, &mt).unwrap();
    assert_eq!(rows.len(), 2);
    let scores = rows
        .iter()
        .map(|row| match row["score"] {
            TqlValue::Float(value) => value,
            _ => panic!("score 应为浮点标量"),
        })
        .collect::<Vec<_>>();
    assert!(scores[0] <= scores[1]);
    assert!(
        rows.iter()
            .all(|row| matches!(row["sim"], TqlValue::Float(_)))
    );
    assert!(rows.iter().all(|row| {
        match &row["node"] {
            TqlValue::Node(node) => !node
                .payload
                .as_object()
                .unwrap()
                .keys()
                .any(|key| key.starts_with("__tql_")),
            _ => false,
        }
    }));
    assert!(execute_tql(&query, &mt).is_err());
}

#[test]
fn find_和_match_可作为统一_pipeline_source() {
    let mt = graph();
    let find = parse_tql(
        "FIND {active: true} AS seed WITH seed EXPAND seed [:related*1..1] AS related WITH related RETURN related",
    )
    .unwrap();
    let find_rows = execute_tql_values(&find, &mt).unwrap();
    assert_eq!(find_rows.len(), 2);
    assert!(
        find_rows
            .iter()
            .any(|row| matches!(&row["related"], TqlValue::Node(node) if node.id == 2))
    );

    let matched = parse_tql(
        "MATCH (seed {active: true}) AS source WITH source EXPAND source [:related*1..1] AS related WITH related RETURN related",
    )
    .unwrap();
    let match_rows = execute_tql_values(&matched, &mt).unwrap();
    assert_eq!(match_rows.len(), 2);
}

#[test]
fn path_标量表达式可解析且无查询向量的_similarity_明确拒绝() {
    let parsed = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 2 AS node WITH node RETURN node, path_strength(node) AS strength, path_count(node) AS path_total",
    )
    .unwrap();
    assert!(matches!(
        parsed.returns,
        triviumdb::query::tql_ast::ReturnClause::Expressions(_)
    ));

    let invalid = parse_tql(
        "FIND {active: true} AS seed WITH seed EXPAND seed [:related*1..1] AS related WITH related WHERE similarity(related) > 0.1 RETURN related",
    )
    .unwrap();
    assert!(execute_tql_values(&invalid, &graph()).is_err());
}

#[test]
fn 图算法阶段通过统一_tql_pipeline_执行() {
    let mt = graph();
    for (algorithm, scalar) in [
        ("pagerank", "graph_score(scored)"),
        ("degree", "graph_score(scored)"),
        ("wcc", "community(scored)"),
        ("label_propagation", "community(scored)"),
        ("leiden", "community(scored)"),
        ("sa_ppr", "graph_score(scored)"),
    ] {
        let query = parse_tql(&format!(
            "SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed {algorithm} seed AS scored WITH scored RETURN scored, {scalar} AS value"
        ))
        .unwrap();
        let rows = execute_tql_values(&query, &mt).unwrap();
        assert!(!rows.is_empty(), "{algorithm} 应返回结果");
        assert!(rows.iter().all(|row| row.contains_key("value")));
    }
}

#[test]
fn all_paths_参数化语法端到端执行() {
    let mt = graph();
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed all_paths seed to [3] depth 3 paths 8 aggregate max_product labels [related, related] avoid [99] AS paths WITH paths RETURN paths, path_strength(paths) AS strength, path_count(paths) AS total",
    )
    .unwrap();
    assert!(
        query
            .pipeline
            .iter()
            .any(|stage| matches!(stage, PipelineStage::AllPaths(_)))
    );
    let rows = execute_tql_values(&query, &mt).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0]["strength"], TqlValue::Float(value) if value > 0.0));
    assert!(matches!(rows[0]["total"], TqlValue::Int(1)));
}

#[test]
fn iterate_参数化语法端到端执行并校验边界() {
    let mt = graph();
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed iterate seed EXPAND [:related*1..1] times 4 fixed AS reached WITH reached RETURN reached",
    )
    .unwrap();
    assert!(
        query
            .pipeline
            .iter()
            .any(|stage| matches!(stage, PipelineStage::Iterate(_)))
    );
    let rows = execute_tql_values(&query, &mt).unwrap();
    let mut ids = rows
        .iter()
        .map(|row| match &row["reached"] {
            TqlValue::Node(node) => node.id,
            _ => 0,
        })
        .collect::<Vec<_>>();
    ids.sort_unstable();
    assert_eq!(ids, vec![1, 2, 3]);
    assert!(parse_tql("SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed all_paths seed to [] depth 3 paths 8 aggregate max_product AS paths WITH paths RETURN paths").is_err());
}

#[test]
fn 算术_coalesce_空值与括号表达式端到端执行() {
    let query = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 1 AS node WITH node RETURN 1 + 2 * 3 AS precedence, (1 + 2) * 3 AS grouped, coalesce(1 / 0, 7) AS fallback, node.missing IS NULL AS missing",
    )
    .unwrap();
    let rows = execute_tql_values(&query, &graph()).unwrap();
    assert!(matches!(rows[0]["precedence"], TqlValue::Float(value) if value == 7.0));
    assert!(matches!(rows[0]["grouped"], TqlValue::Float(value) if value == 9.0));
    assert!(matches!(rows[0]["fallback"], TqlValue::Int(7)));
    assert!(matches!(rows[0]["missing"], TqlValue::Bool(true)));
}

#[test]
fn 管线聚合支持分组_count_数值聚合_collect_和空输入() {
    let grouped = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 3 AS node WITH node RETURN node.active AS active, count(*) AS total, sum(node.id) AS sum_id, avg(node.id) AS avg_id, min(node.id) AS min_id, max(node.id) AS max_id, collect(node.id) AS ids",
    )
    .unwrap();
    let rows = execute_tql_values(&grouped, &graph()).unwrap();
    assert_eq!(rows.len(), 2);
    let total = rows
        .iter()
        .map(|row| match row["total"] {
            TqlValue::Int(value) => value,
            _ => 0,
        })
        .sum::<i64>();
    assert_eq!(total, 3);
    assert!(
        rows.iter()
            .all(|row| matches!(row["sum_id"], TqlValue::Float(_)))
    );
    assert!(
        rows.iter()
            .all(|row| matches!(row["ids"], TqlValue::List(_)))
    );

    let empty = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 3 AS node WITH node WHERE node.id > 99 RETURN count(*) AS total, avg(node.id) AS avg_id",
    )
    .unwrap();
    let rows = execute_tql_values(&empty, &graph()).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0]["total"], TqlValue::Int(0)));
    assert!(matches!(rows[0]["avg_id"], TqlValue::Null));
}

#[test]
fn shortest_path_和集合代数端到端稳定执行() {
    let shortest = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed shortest_paths seed TO [3] LABEL related AS route WITH route RETURN route, path(route) AS path, path_length(route) AS length",
    )
    .unwrap();
    let rows = execute_tql_values(&shortest, &graph()).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(matches!(&rows[0]["path"], TqlValue::Path(path) if path == &vec![1, 2, 3]));
    assert!(matches!(rows[0]["length"], TqlValue::Int(2)));

    for (operation, expected) in [
        ("union", vec![1, 2, 3]),
        ("intersect", vec![2]),
        ("except", vec![1]),
    ] {
        let query = parse_tql(&format!(
            "SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed {operation} seed IDS [2, 3] AS combined WITH combined RETURN combined"
        ))
        .unwrap();
        let first = execute_tql_values(&query, &graph()).unwrap();
        let second = execute_tql_values(&query, &graph()).unwrap();
        let ids = |rows: &[std::collections::HashMap<String, TqlValue<f32>>]| {
            rows.iter()
                .map(|row| match &row["combined"] {
                    TqlValue::Node(node) => node.id,
                    _ => 0,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&first), expected);
        assert_eq!(ids(&first), ids(&second));
    }
    let invalid_set = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed union seed IDS [99] AS combined WITH combined RETURN combined",
    )
    .unwrap();
    assert!(execute_tql_values(&invalid_set, &graph()).is_err());
}

#[test]
fn prepared_tql_严格绑定并可重复执行() {
    let parsed = parse_tql(
        "SEARCH VECTOR [1, 0] TOP 3 AS node WITH node WHERE node.id > $minimum RETURN node, coalesce($bonus, 0) + node.id AS score",
    )
    .unwrap();
    let prepared = PreparedTql::from_query(parsed);
    assert_eq!(prepared.parameter_names(), vec!["bonus", "minimum"]);

    let mut parameters = std::collections::HashMap::from([
        ("minimum".to_owned(), TqlParamValue::Int(1)),
        ("bonus".to_owned(), TqlParamValue::Float(10.0)),
    ]);
    let first = execute_tql_values(&prepared.bind(&parameters).unwrap(), &graph()).unwrap();
    let second = execute_tql_values(&prepared.bind(&parameters).unwrap(), &graph()).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(second.len(), 2);
    assert!(matches!(first[0]["score"], TqlValue::Float(value) if value >= 12.0));

    parameters.remove("bonus");
    assert!(prepared.bind(&parameters).is_err());
    parameters.insert("bonus".to_owned(), TqlParamValue::Float(f64::NAN));
    assert!(prepared.bind(&parameters).is_err());
    parameters.insert("bonus".to_owned(), TqlParamValue::Null);
    parameters.insert("extra".to_owned(), TqlParamValue::Bool(true));
    assert!(prepared.bind(&parameters).is_err());
}

#[test]
fn 旧_tql_语法保持无_pipeline() {
    let query = parse_tql("SEARCH VECTOR [1, 0] TOP 2 RETURN *").unwrap();
    assert!(query.pipeline.is_empty());
    let rows = execute_tql(&query, &graph()).unwrap();
    assert_eq!(rows.len(), 2);
}
