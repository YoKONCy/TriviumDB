//! TQL AST 数据结构完整性测试
//!
//! 覆盖: 所有枚举变体的构造 + Clone + Debug

use triviumdb::Filter;
use triviumdb::query::tql_ast::*;

#[test]
fn query_entry_所有变体() {
    let _match = QueryEntry::Match {
        pattern: TqlPattern {
            nodes: vec![],
            edges: vec![],
        },
    };
    let _opt = QueryEntry::OptionalMatch {
        pattern: TqlPattern {
            nodes: vec![],
            edges: vec![],
        },
    };
    let _find = QueryEntry::Find {
        filter: Filter::eq("a", serde_json::json!(1)),
    };
    let _search = QueryEntry::Search {
        vector: vec![1.0, 2.0],
        vector_parameters: vec![],
        top_k: 5,
        expand: None,
    };
}

#[test]
fn edge_direction_eq() {
    assert_eq!(EdgeDirection::Forward, EdgeDirection::Forward);
    assert_ne!(EdgeDirection::Forward, EdgeDirection::Backward);
    assert_ne!(EdgeDirection::Backward, EdgeDirection::Both);
}

#[test]
fn agg_func_eq() {
    assert_eq!(AggFunc::Count, AggFunc::Count);
    assert_ne!(AggFunc::Sum, AggFunc::Avg);
    let _all = [
        AggFunc::Count,
        AggFunc::Sum,
        AggFunc::Avg,
        AggFunc::Min,
        AggFunc::Max,
        AggFunc::Collect,
    ];
}

#[test]
fn predicate_构造() {
    let cmp = Predicate::Compare {
        left: TqlExpr::Property {
            var: "a".into(),
            field: "name".into(),
        },
        op: TqlCompOp::Eq,
        right: TqlExpr::Literal(TqlLiteral::Str("Alice".into())),
    };
    let doc = Predicate::DocFilter {
        var: Some("b".into()),
        filter: Filter::gt("age", 18.0),
    };
    let _and = Predicate::And(Box::new(cmp.clone()), Box::new(doc));
    let _or = Predicate::Or(
        Box::new(cmp.clone()),
        Box::new(Predicate::Not(Box::new(cmp))),
    );
}

#[test]
fn return_clause_变体() {
    let _all = ReturnClause::All;
    let _vars = ReturnClause::Variables(vec!["a".into(), "b".into()]);
    let _exprs = ReturnClause::Expressions(vec![
        ReturnExpr {
            kind: ReturnExprKind::Var("a".into()),
            alias: None,
            distinct: false,
        },
        ReturnExpr {
            kind: ReturnExprKind::Property("a".into(), "name".into()),
            alias: Some("name".into()),
            distinct: true,
        },
        ReturnExpr {
            kind: ReturnExprKind::Aggregate(
                AggFunc::Count,
                Box::new(ReturnExprKind::Var("b".into())),
            ),
            alias: Some("cnt".into()),
            distinct: false,
        },
    ]);
}

#[test]
fn literal_所有变体() {
    let _int = TqlLiteral::Int(42);
    let _float = TqlLiteral::Float(1.25);
    let _str = TqlLiteral::Str("hello".into());
    let _bool = TqlLiteral::Bool(true);
    let _null = TqlLiteral::Null;
}

#[test]
fn comp_op_所有变体() {
    let _ops = [
        TqlCompOp::Eq,
        TqlCompOp::Ne,
        TqlCompOp::Gt,
        TqlCompOp::Gte,
        TqlCompOp::Lt,
        TqlCompOp::Lte,
    ];
}

#[test]
fn mutation_action_变体() {
    let _create = MutationAction::Create(CreateAction {
        nodes: vec![CreateNode {
            var: Some("a".into()),
            payload: serde_json::json!({"name": "x"}),
        }],
        edges: vec![CreateEdge {
            src_var: "a".into(),
            dst_var: "b".into(),
            label: "knows".into(),
            weight: 1.0,
        }],
    });
    let _set = MutationAction::Set(vec![SetAssignment {
        var: "a".into(),
        field: "age".into(),
        value: serde_json::json!(30),
    }]);
    let _del = MutationAction::Delete {
        vars: vec!["a".into()],
        detach: true,
    };
}

#[test]
fn tql_statement_变体() {
    let query = TqlQuery {
        explain: false,
        analyze: false,
        entry: QueryEntry::Find {
            filter: Filter::eq("x", serde_json::json!(1)),
        },
        pipeline: vec![],
        predicate: None,
        rank: None,
        returns: ReturnClause::All,
        order_by: vec![],
        limit: Some(10),
        offset: None,
    };
    let _stmt = TqlStatement::Query(query);

    let mutation = TqlMutation {
        source: None,
        action: MutationAction::Delete {
            vars: vec!["a".into()],
            detach: false,
        },
    };
    let _stmt2 = TqlStatement::Mutation(mutation);
}

#[test]
fn expand_clause() {
    let _e = ExpandClause {
        labels: vec!["knows".into()],
        min_depth: 1,
        max_depth: 3,
        direction: EdgeDirection::Forward,
    };
}

#[test]
fn hop_range() {
    let h = HopRange { min: 1, max: 5 };
    assert_eq!(h.min, 1);
    assert_eq!(h.max, 5);
}

#[test]
fn node_pattern_和_edge_pattern() {
    let _np = TqlNodePattern {
        var: Some("a".into()),
        filter: None,
    };
    let _ep = TqlEdgePattern {
        labels: vec!["knows".into()],
        hop_range: Some(HopRange { min: 1, max: 3 }),
        direction: EdgeDirection::Forward,
    };
}
