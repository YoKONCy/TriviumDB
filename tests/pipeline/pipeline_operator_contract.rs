use serde_json::json;
use triviumdb::Filter;
use triviumdb::graph::budget::{BudgetExhaustionPolicy, TraversalBudget};
use triviumdb::graph::reachability::{
    ReachabilityConfig, ReachabilityDirection, traverse_compact, traverse_detailed,
};
use triviumdb::query::parallel::QueryParallelismBudget;
use triviumdb::query::pipeline::{
    DegreeCentralityOperator, ExactRerank, ExactVectorSearch, Expand, ExpandExactRerank,
    GraphMetric, GraphSubsetMode, Limit, NodeIdsSource, NodeRow, NodeSet, PayloadFilter,
    PipelineBudget, PipelineContext, PipelineOperator, PropertyLookup, ScoreKind, ScoreValue,
    SetOperation, combine_sets, execute_pipeline,
};
use triviumdb::storage::memtable::MemTable;

const DIM: usize = 4;

#[test]
fn compact_遍历与完整参考实现逐项一致() {
    let mt = graph();
    for direction in [
        ReachabilityDirection::Outgoing,
        ReachabilityDirection::Incoming,
        ReachabilityDirection::Both,
    ] {
        for max_depth in 1..=3 {
            let config = ReachabilityConfig {
                min_depth: 0,
                max_depth,
                labels: Some(vec!["related".into()]),
                direction,
                max_visited_nodes: 100,
                max_results: 100,
                max_edges: 100,
                max_frontier_size: 100,
                exhaustion_policy: BudgetExhaustionPolicy::Error,
            };
            let compact = traverse_compact(&mt, 1, &config).unwrap();
            let detailed = traverse_detailed(&mt, 1, &config).unwrap();
            let compact_rows = compact
                .results
                .iter()
                .map(|hit| (hit.target_id, hit.depth))
                .collect::<Vec<_>>();
            let detailed_rows = detailed
                .results
                .iter()
                .map(|hit| (hit.target_id, hit.depth))
                .collect::<Vec<_>>();
            assert_eq!(compact_rows, detailed_rows);
            assert_eq!(compact.visited_nodes, detailed.visited_nodes);
            assert_eq!(compact.traversed_edges, detailed.traversed_edges);
        }
    }
}

fn graph() -> MemTable<f32> {
    let mut mt = MemTable::new(DIM);
    for (id, vector, active) in [
        (1, [1.0, 0.0, 0.0, 0.0], true),
        (2, [0.9, 0.1, 0.0, 0.0], true),
        (3, [0.8, 0.2, 0.0, 0.0], false),
        (4, [0.0, 1.0, 0.0, 0.0], true),
        (5, [0.0, 0.9, 0.1, 0.0], false),
    ] {
        mt.insert_with_id(id, &vector, json!({"active": active, "group": id % 2}))
            .unwrap();
    }
    mt.register_property_index("active");
    mt.link(1, 2, "related".into(), 1.0).unwrap();
    mt.link(2, 3, "related".into(), 0.8).unwrap();
    mt.link(1, 4, "other".into(), 1.0).unwrap();
    mt
}

fn budget() -> PipelineBudget {
    PipelineBudget {
        max_stages: 16,
        max_nodes: 100,
        max_node_set_bytes: 1024 * 1024,
        max_vector_read_bytes: 1024 * 1024,
        max_payload_lookups: 100,
        max_payload_parsed_bytes: 1024 * 1024,
        traversal: TraversalBudget {
            max_visited_nodes: 100,
            max_examined_edges: 100,
            max_frontier_size: 100,
            max_depth: 4,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        },
        parallelism: Default::default(),
    }
}

fn parallel_budget(threads: usize) -> PipelineBudget {
    PipelineBudget {
        parallelism: QueryParallelismBudget {
            max_threads: threads,
            min_parallel_rows: 0,
        },
        ..budget()
    }
}

#[test]
fn 并行预算小任务串行且线程上限生效() {
    let budget = QueryParallelismBudget {
        max_threads: 64,
        min_parallel_rows: 10,
    };
    assert_eq!(budget.threads(9), 1);
    assert!((1..=64).contains(&budget.threads(10)));
}

#[test]
fn nodeset_dedup_将后续行合并进保留行且加权距离与路径同源() {
    let mut rows = vec![NodeRow::new(9), NodeRow::new(9)];
    rows[0].path = Some(vec![1, 9]);
    rows[0].provenance.source_ids = vec![1];
    rows[0]
        .set_graph_metric(
            GraphMetric::WeightedDistance,
            ScoreValue {
                value: 1.0,
                kind: ScoreKind::Exact,
            },
        )
        .unwrap();
    rows[1].path = Some(vec![9]);
    rows[1].provenance.source_ids = vec![9];
    rows[1]
        .set_graph_metric(
            GraphMetric::WeightedDistance,
            ScoreValue {
                value: 0.0,
                kind: ScoreKind::Exact,
            },
        )
        .unwrap();

    let output = NodeSet::from_rows(rows);
    assert_eq!(output.len(), 1);
    assert_eq!(output.rows()[0].path, Some(vec![9]));
    assert_eq!(
        output.rows()[0]
            .graph_metric(GraphMetric::WeightedDistance)
            .unwrap()
            .value,
        0.0
    );
    assert_eq!(output.rows()[0].provenance.source_ids, vec![1, 9]);
}

#[test]
fn nodeset_去重排序与集合运算合并来源() {
    let left = NodeSet::from_ids([3, 1, 1, 2]);
    let right = NodeSet::from_ids([2, 4]);
    assert_eq!(
        left.rows().iter().map(|row| row.id).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(
        combine_sets(left.clone(), right.clone(), SetOperation::Union)
            .rows()
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    assert_eq!(
        combine_sets(left.clone(), right.clone(), SetOperation::Intersect)
            .rows()
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        [2]
    );
    assert_eq!(
        combine_sets(left, right, SetOperation::Difference)
            .rows()
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        [1, 3]
    );
}

#[test]
fn vector_expand_rerank_filter_limit_四阶段语义正确() {
    let mt = graph();
    let operators: Vec<Box<dyn PipelineOperator<f32>>> = vec![
        Box::new(ExactVectorSearch {
            query: vec![1.0, 0.0, 0.0, 0.0],
            top_k: 1,
        }),
        Box::new(Expand {
            min_depth: 1,
            max_depth: 2,
            labels: Some(vec!["related".into()]),
            direction: ReachabilityDirection::Outgoing,
            include_input: true,
        }),
        Box::new(ExactRerank {
            query: vec![1.0, 0.0, 0.0, 0.0],
            top_k: None,
        }),
        Box::new(PayloadFilter {
            filter: Filter::eq("active", json!(true)),
        }),
        Box::new(Limit { limit: 2 }),
    ];
    let mut context = PipelineContext::with_profile(&mt, budget(), true);
    let output = execute_pipeline(&mut context, &operators).unwrap();
    assert_eq!(
        output.rows().iter().map(|row| row.id).collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(output.rows().iter().all(|row| {
        row.similarity
            .is_some_and(|score| score.kind == ScoreKind::Exact)
    }));
    assert_eq!(context.metrics.len(), operators.len());
    assert_eq!(context.metrics[1].operator, "expand");
}

#[test]
fn expand_紧凑物化保留多来源与最小深度() {
    let mut mt = graph();
    mt.link(4, 3, "related".into(), 1.0).unwrap();
    let output = Expand {
        min_depth: 1,
        max_depth: 2,
        labels: Some(vec!["related".into()]),
        direction: ReachabilityDirection::Outgoing,
        include_input: false,
    }
    .apply(
        NodeSet::from_ids([1, 4]),
        &mut PipelineContext::new(&mt, budget()),
    )
    .unwrap();
    let row = output.rows().iter().find(|row| row.id == 3).unwrap();
    assert_eq!(row.provenance.source_ids, [1, 4]);
    assert_eq!(row.provenance.min_depth, Some(1));
    assert_eq!(row.graph_score.unwrap().value, 1.0);
}

#[test]
fn exact_vector_search在pipeline归一化后仍保持相似度排名() {
    let mt = graph();
    let operators: Vec<Box<dyn PipelineOperator<f32>>> = vec![Box::new(ExactVectorSearch {
        query: vec![1.0, 0.0, 0.0, 0.0],
        top_k: 3,
    })];
    let mut context = PipelineContext::new(&mt, budget());
    let output = execute_pipeline(&mut context, &operators).unwrap();
    assert_eq!(
        output.rows().iter().map(|row| row.id).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert!(
        output
            .rows()
            .windows(2)
            .all(|pair| { pair[0].similarity.unwrap().value >= pair[1].similarity.unwrap().value })
    );
}

#[test]
fn exact_rerank_topk_选择算法与全排序完全一致() {
    let mt = graph();
    let input = NodeSet::from_ids([5, 2, 4, 1, 3]);
    let query = vec![1.0, 0.0, 0.0, 0.0];
    let mut full_context = PipelineContext::new(&mt, budget());
    let full = ExactRerank {
        query: query.clone(),
        top_k: None,
    }
    .apply(input.clone(), &mut full_context)
    .unwrap();
    let mut topk_context = PipelineContext::new(&mt, budget());
    let topk = ExactRerank {
        query,
        top_k: Some(3),
    }
    .apply(input, &mut topk_context)
    .unwrap();
    assert_eq!(topk.rows(), &full.rows()[..3]);
}

#[test]
fn expand_rank_融合算子与分离执行结果完全一致() {
    let mt = graph();
    let source = NodeSet::from_ids([1]);
    let query = vec![1.0, 0.0, 0.0, 0.0];
    let expand = || Expand {
        min_depth: 1,
        max_depth: 2,
        labels: None,
        direction: ReachabilityDirection::Outgoing,
        include_input: true,
    };

    let mut separated_context = PipelineContext::new(&mt, budget());
    let expanded = expand()
        .apply(source.clone(), &mut separated_context)
        .unwrap();
    let separated = ExactRerank {
        query: query.clone(),
        top_k: Some(3),
    }
    .apply(expanded, &mut separated_context)
    .unwrap();

    let mut fused_context = PipelineContext::new(&mt, budget());
    let fused = ExpandExactRerank {
        expand: expand(),
        query,
        top_k: 3,
    }
    .apply(source, &mut fused_context)
    .unwrap();
    assert_eq!(fused, separated);
}

#[test]
fn 属性入口走索引并可与图入口求交() {
    let mt = graph();
    let mut context = PipelineContext::new(&mt, budget());
    let property = PropertyLookup {
        field: "active".into(),
        value: json!(true),
    }
    .apply(NodeSet::empty(), &mut context)
    .unwrap();
    let graph = Expand {
        min_depth: 1,
        max_depth: 2,
        labels: Some(vec!["related".into()]),
        direction: ReachabilityDirection::Outgoing,
        include_input: true,
    }
    .apply(NodeSet::from_ids([1]), &mut context)
    .unwrap();
    let ids = combine_sets(property, graph, SetOperation::Intersect)
        .rows()
        .iter()
        .map(|row| row.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, [1, 2]);
}

#[test]
fn 第一批并行算子与单线程逐字段完全一致() {
    let mut mt = graph();
    mt.link(4, 3, "related".into(), 1.0).unwrap();
    let source = NodeSet::from_ids([1, 2, 4]);

    let run_expand = |threads| {
        Expand {
            min_depth: 1,
            max_depth: 2,
            labels: None,
            direction: ReachabilityDirection::Outgoing,
            include_input: true,
        }
        .apply(
            source.clone(),
            &mut PipelineContext::new(&mt, parallel_budget(threads)),
        )
        .unwrap()
    };
    assert_eq!(run_expand(1), run_expand(4));

    let run_rerank = |threads| {
        ExactRerank {
            query: vec![1.0, 0.0, 0.0, 0.0],
            top_k: Some(3),
        }
        .apply(
            NodeSet::from_ids([1, 2, 3, 4, 5]),
            &mut PipelineContext::new(&mt, parallel_budget(threads)),
        )
        .unwrap()
    };
    assert_eq!(run_rerank(1), run_rerank(4));

    let run_property = |threads| {
        let result = PropertyLookup {
            field: "group".into(),
            value: json!(1),
        }
        .apply(
            NodeSet::empty(),
            &mut PipelineContext::new(&mt, parallel_budget(threads)),
        )
        .unwrap();
        assert!(result.rows().iter().all(|row| {
            row.provenance
                .property_origin
                .as_ref()
                .is_some_and(|origin| origin.field == "group" && origin.value == json!(1))
        }));
        result
    };
    assert_eq!(run_property(1), run_property(4));

    let run_degree = |threads| {
        DegreeCentralityOperator {
            mode: GraphSubsetMode::Induced,
            label_filter: None,
        }
        .apply(
            NodeSet::from_ids([1, 2, 3, 4, 5]),
            &mut PipelineContext::new(&mt, parallel_budget(threads)),
        )
        .unwrap()
    };
    assert_eq!(run_degree(1), run_degree(4));
}

#[test]
fn 并行_expand_全局预算仍然_fail_closed() {
    let mt = graph();
    let mut tiny = parallel_budget(4);
    tiny.traversal.max_examined_edges = 1;
    let result = Expand {
        min_depth: 1,
        max_depth: 2,
        labels: None,
        direction: ReachabilityDirection::Outgoing,
        include_input: false,
    }
    .apply(
        NodeSet::from_ids([1, 2]),
        &mut PipelineContext::new(&mt, tiny),
    );
    assert!(result.is_err());
}

#[test]
fn 管线阶段节点向量图与_payload_预算均提前拒绝() {
    let mut mt = graph();
    mt.configure_payload_cache(0, 0);
    let mut tiny = budget();
    tiny.max_stages = 1;
    let mut context = PipelineContext::new(&mt, tiny);
    let operators: Vec<Box<dyn PipelineOperator<f32>>> = vec![
        Box::new(NodeIdsSource { ids: vec![1] }),
        Box::new(Limit { limit: 1 }),
    ];
    assert!(execute_pipeline(&mut context, &operators).is_err());

    let mut tiny = budget();
    tiny.max_node_set_bytes = 1;
    let mut context = PipelineContext::new(&mt, tiny);
    let operators: Vec<Box<dyn PipelineOperator<f32>>> =
        vec![Box::new(NodeIdsSource { ids: vec![1] })];
    assert!(execute_pipeline(&mut context, &operators).is_err());

    let mut tiny = budget();
    tiny.max_vector_read_bytes = DIM * std::mem::size_of::<f32>();
    let mut context = PipelineContext::new(&mt, tiny);
    let operators: Vec<Box<dyn PipelineOperator<f32>>> = vec![Box::new(ExactVectorSearch {
        query: vec![1.0, 0.0, 0.0, 0.0],
        top_k: 1,
    })];
    assert!(execute_pipeline(&mut context, &operators).is_err());

    let mut tiny = budget();
    tiny.traversal.max_examined_edges = 1;
    let mut context = PipelineContext::new(&mt, tiny);
    let operators: Vec<Box<dyn PipelineOperator<f32>>> = vec![
        Box::new(NodeIdsSource { ids: vec![1] }),
        Box::new(Expand {
            min_depth: 1,
            max_depth: 2,
            labels: None,
            direction: ReachabilityDirection::Outgoing,
            include_input: false,
        }),
    ];
    assert!(execute_pipeline(&mut context, &operators).is_err());

    let mut tiny = budget();
    tiny.max_payload_lookups = 0;
    let mut context = PipelineContext::new(&mt, tiny);
    let operators: Vec<Box<dyn PipelineOperator<f32>>> = vec![
        Box::new(NodeIdsSource { ids: vec![1] }),
        Box::new(PayloadFilter {
            filter: Filter::eq("kind", json!("root")),
        }),
    ];
    assert!(matches!(
        execute_pipeline(&mut context, &operators),
        Err(triviumdb::TriviumError::PayloadQueryBudgetExceeded {
            dimension: "lookups",
            ..
        })
    ));
}
