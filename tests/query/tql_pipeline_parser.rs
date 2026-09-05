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

fn cognitive_graph() -> MemTable<f32> {
    let mut mt = graph();
    mt.index_text(1, "rust memory safety database");
    mt.index_text(2, "rust graph vector search");
    mt.index_text(3, "python scripting language");
    mt.index_keyword(1, "memory safety");
    mt.index_keyword(2, "vector search");
    mt.build_text_index();
    mt
}

#[test]
fn text_bm25_ac_hybrid_入口与分数稳定执行() {
    let mt = cognitive_graph();
    for kind in ["bm25", "ac", "hybrid"] {
        let query = parse_tql(&format!(
            "TEXT {kind} \"rust memory safety\" TOP 3 AS hit WITH hit RETURN hit, text_score(hit) AS score ORDER BY text_score(hit) DESC"
        ))
        .unwrap();
        let first = execute_tql_values(&query, &mt).unwrap();
        let second = execute_tql_values(&query, &mt).unwrap();
        assert!(!first.is_empty());
        assert_eq!(first.len(), second.len());
        assert!(
            first
                .iter()
                .all(|row| matches!(row["score"], TqlValue::Float(_)))
        );
        let ids = |rows: &[std::collections::HashMap<String, TqlValue<f32>>]| {
            rows.iter()
                .map(|row| match &row["hit"] {
                    TqlValue::Node(node) => node.id,
                    _ => 0,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&first), ids(&second));
    }
}

#[test]
fn dpp_sa_ppr_fista_nmf_作为有界_tql_算子执行() {
    let mt = cognitive_graph();
    let dpp = parse_tql("SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed diversify seed TOP 2 quality_weight 1 AS diverse WITH diverse RETURN diverse, diversity_score(diverse) AS score").unwrap();
    let rows = execute_tql_values(&dpp, &mt).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|row| matches!(row["score"], TqlValue::Float(_)))
    );

    let ppr = parse_tql("SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed sa_ppr_config seed depth 2 alpha 0.15 max_edges 8 min_weight 0 labels [related] AS expanded WITH expanded RETURN expanded, graph_score(expanded) AS score").unwrap();
    assert!(!execute_tql_values(&ppr, &mt).unwrap().is_empty());

    let residual = parse_tql("SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed residual seed BY VECTOR [0, 1] TOP 2 lambda 0.1 threshold 0 iterations 8 AS shadow WITH shadow RETURN shadow, residual_score(shadow) AS score").unwrap();
    assert!(!execute_tql_values(&residual, &mt).unwrap().is_empty());

    let topics = parse_tql("SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed topics seed k 2 iterations 8 AS clustered WITH clustered RETURN clustered, topic(clustered) AS topic_id, topic_score(clustered) AS score").unwrap();
    let rows = execute_tql_values(&topics, &mt).unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .all(|row| matches!(row["topic_id"], TqlValue::Int(_)))
    );
}

#[test]
fn 认知算子预算和参数_fail_closed() {
    let mt = cognitive_graph();
    let oversized_dpp = parse_tql("SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed diversify seed TOP 4 AS diverse WITH diverse RETURN diverse").unwrap();
    assert!(execute_tql_values(&oversized_dpp, &mt).is_err());
    assert!(parse_tql("TEXT bm25 \"rust\" TOP 2 b 1.5 AS hit WITH hit RETURN hit").is_ok());
    let invalid_text =
        parse_tql("TEXT bm25 \"rust\" TOP 2 b 1.5 AS hit WITH hit RETURN hit").unwrap();
    assert!(execute_tql_values(&invalid_text, &mt).is_err());
    assert!(parse_tql("SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed topics seed k 33 iterations 8 AS clustered WITH clustered RETURN clustered").is_ok());
    let oversized_nmf = parse_tql("SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed topics seed k 33 iterations 8 AS clustered WITH clustered RETURN clustered").unwrap();
    assert!(execute_tql_values(&oversized_nmf, &mt).is_err());
    let oversized_fista = parse_tql("SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed residual seed BY VECTOR [0, 1] TOP 2 lambda 0.1 threshold 0 iterations 257 AS shadow WITH shadow RETURN shadow").unwrap();
    assert!(execute_tql_values(&oversized_fista, &mt).is_err());
}

#[test]
fn explain_暴露四类认知物理算子且估算受预算约束() {
    let mt = cognitive_graph();
    let cases = [
        (
            "EXPLAIN TEXT bm25 \"rust\" TOP 2 AS hit WITH hit RETURN hit",
            "text_first_source",
        ),
        (
            "EXPLAIN SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed diversify seed TOP 2 AS diverse WITH diverse RETURN diverse",
            "dpp_diversify",
        ),
        (
            "EXPLAIN SEARCH VECTOR [1, 0] TOP 2 AS seed WITH seed residual seed BY VECTOR [0, 1] TOP 2 lambda 0.1 threshold 0 iterations 8 AS shadow WITH shadow RETURN shadow",
            "fista_residual_recall",
        ),
        (
            "EXPLAIN SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed topics seed k 2 iterations 8 AS clustered WITH clustered RETURN clustered",
            "nmf_topics",
        ),
        (
            "EXPLAIN SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed sa_ppr_config seed depth 2 alpha 0.15 max_edges 8 min_weight 0 AS expanded WITH expanded RETURN expanded",
            "sa_ppr_depth_bounded",
        ),
    ];
    for (source, operator) in cases {
        let result = execute_tql(&parse_tql(source).unwrap(), &mt).unwrap();
        let rendered = format!("{result:?}");
        assert!(
            rendered.contains(operator),
            "EXPLAIN 应包含 {operator}: {rendered}"
        );
    }
}

#[test]
fn g1_g2_图算法通过_tql_输出命名指标() {
    let mt = cognitive_graph();
    for (algorithm, scalar) in [
        ("scc", "community(scored)"),
        ("k_core", "core_number(scored)"),
        ("triangle_count", "triangle_count(scored)"),
        ("hits", "authority_score(scored)"),
    ] {
        let query = parse_tql(&format!(
            "SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed {algorithm} seed AS scored WITH scored RETURN scored, {scalar} AS value"
        ))
        .unwrap();
        let rows = execute_tql_values(&query, &mt).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .all(|row| !matches!(row["value"], TqlValue::Null))
        );
    }
    let articulation = parse_tql("SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed articulation_points seed AS critical WITH critical RETURN critical").unwrap();
    assert!(!execute_tql_values(&articulation, &mt).unwrap().is_empty());

    let expanded = parse_tql("SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed scc seed MODE EXPAND HOPS 2 BOTH LABELS [related] LABEL related AS component WITH component RETURN component, community(component) AS id").unwrap();
    assert!(matches!(
        expanded.pipeline.iter().find_map(|stage| match stage {
            PipelineStage::GraphAlgorithm(stage) => Some(&stage.subset),
            _ => None,
        }),
        Some(triviumdb::query::tql_ast::GraphSubsetSpec::Expand { hops: 2, .. })
    ));
    assert!(!execute_tql_values(&expanded, &mt).unwrap().is_empty());

    let both = parse_tql("SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed triangle_count seed AS triangles WITH triangles hits triangles AS ranked WITH ranked RETURN ranked, triangle_count(ranked) AS triangles, authority_score(ranked) AS authority, hub_score(ranked) AS hub, clustering_coefficient(ranked) AS clustering").unwrap();
    let rows = execute_tql_values(&both, &mt).unwrap();
    assert!(rows.iter().all(|row| {
        ["triangles", "authority", "hub", "clustering"]
            .iter()
            .all(|key| !matches!(row[*key], TqlValue::Null))
    }));
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
fn g3_g4_加权路径_yen_调和中心性与_pairset_端到端执行() {
    let mut mt = MemTable::new(2);
    for id in 1..=6 {
        mt.insert_with_id(id, &[0.0, 0.0], json!({})).unwrap();
    }
    for (source, target, weight) in [
        (1, 2, 1.0),
        (2, 4, 1.0),
        (1, 3, 1.0),
        (3, 4, 1.0),
        (1, 4, 3.0),
        (5, 2, 1.0),
        (5, 3, 1.0),
        (6, 2, 1.0),
        (6, 3, 1.0),
    ] {
        mt.link(source, target, "road".into(), weight).unwrap();
    }

    let weighted = parse_tql(
        "SEARCH VECTOR [0, 0] TOP 1 AS seed WITH seed weighted_paths seed TO [4] LABEL road AS route WITH route RETURN path(route) AS path, weighted_distance(route) AS distance",
    )
    .unwrap();
    let rows = execute_tql_values(&weighted, &mt).unwrap();
    assert!(matches!(&rows[0]["path"], TqlValue::Path(path) if path == &vec![1, 2, 4]));
    assert!(matches!(rows[0]["distance"], TqlValue::Float(value) if value == 2.0));

    let yen = parse_tql(
        "SEARCH VECTOR [0, 0] TOP 1 AS seed WITH seed yen_paths seed TO [4] K 3 LABEL road AS route WITH route RETURN path(route) AS path, path_rank(route) AS rank, weighted_distance(route) AS distance",
    )
    .unwrap();
    let rows = execute_tql_values(&yen, &mt).unwrap();
    assert_eq!(rows.len(), 3);
    assert!(matches!(rows[0]["rank"], TqlValue::Int(1)));
    assert!(matches!(rows[1]["rank"], TqlValue::Int(2)));
    assert!(matches!(rows[2]["rank"], TqlValue::Int(3)));

    let harmonic = parse_tql(
        "SEARCH VECTOR [0, 0] TOP 6 AS seed WITH seed harmonic_centrality seed LABEL road AS scored WITH scored RETURN scored, harmonic_centrality(scored) AS score",
    )
    .unwrap();
    assert_eq!(execute_tql_values(&harmonic, &mt).unwrap().len(), 6);

    let similarity = parse_tql(
        "SEARCH VECTOR [0, 0] TOP 6 AS seed WITH seed node_similarity seed TOP 4 CUTOFF 0.3 LABEL road AS pairs WITH pairs RETURN pair_left(pairs) AS left, pair_right(pairs) AS right, node_similarity(pairs) AS score",
    )
    .unwrap();
    let first = execute_tql_values(&similarity, &mt).unwrap();
    let second = execute_tql_values(&similarity, &mt).unwrap();
    assert_eq!(first.len(), 4);
    let project = |rows: &[std::collections::HashMap<String, TqlValue<f32>>]| {
        rows.iter()
            .map(|row| match (&row["left"], &row["right"], &row["score"]) {
                (TqlValue::Int(left), TqlValue::Int(right), TqlValue::Float(score)) => {
                    (*left, *right, score.to_bits())
                }
                _ => (0, 0, 0),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(project(&first), project(&second));
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
fn prepared_search_vector逐维参数绑定后与字面量热路径一致() {
    let prepared =
        PreparedTql::from_query(parse_tql("SEARCH VECTOR [$x, $y] TOP 2 RETURN *").unwrap());
    assert_eq!(prepared.parameter_names(), vec!["x", "y"]);
    let parameters = std::collections::HashMap::from([
        ("x".to_owned(), TqlParamValue::Float(1.0)),
        ("y".to_owned(), TqlParamValue::Int(0)),
    ]);
    let bound = prepared.bind(&parameters).unwrap();
    let literal = parse_tql("SEARCH VECTOR [1, 0] TOP 2 RETURN *").unwrap();
    let bound_ids = execute_tql(&bound, &graph())
        .unwrap()
        .into_iter()
        .map(|row| row["_"].id)
        .collect::<Vec<_>>();
    let literal_ids = execute_tql(&literal, &graph())
        .unwrap()
        .into_iter()
        .map(|row| row["_"].id)
        .collect::<Vec<_>>();
    assert_eq!(bound_ids, literal_ids);

    let mut invalid = parameters;
    invalid.insert("x".to_owned(), TqlParamValue::String("1".into()));
    assert!(prepared.bind(&invalid).is_err());
    assert!(
        execute_tql(
            &parse_tql("SEARCH VECTOR [$x, 0] TOP 1 RETURN *").unwrap(),
            &graph()
        )
        .is_err()
    );
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
