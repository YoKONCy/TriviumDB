#![allow(non_snake_case)]
//! TQL (Trivium Query Language) 解析器集成测试
//!
//! 覆盖三种查询入口的解析正确性：
//! - MATCH: 图模式匹配（含可变长路径、多标签边）
//! - FIND: MongoDB 风格文档过滤
//! - SEARCH: 向量检索 + EXPAND
//! - WHERE: 统一谓词（Cypher 比较 + Mongo 文档过滤 + 混合）

use triviumdb::filter::Filter;
use triviumdb::query::tql_ast::*;
use triviumdb::query::tql_parser::parse_tql;

// ═══════════════════════════════════════════════════════════════════════
//  FIND 入口测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn FIND_简单等值匹配() {
    let q = parse_tql(r#"FIND {type: "event"} RETURN *"#).unwrap();
    assert!(matches!(q.entry, QueryEntry::Find { .. }));
    assert!(matches!(q.returns, ReturnClause::All));
}

#[test]
fn FIND_多字段隐式AND() {
    let q = parse_tql(r#"FIND {type: "event", region: "cn"} RETURN *"#).unwrap();
    if let QueryEntry::Find { filter } = &q.entry {
        assert!(matches!(filter, Filter::And(_)));
    } else {
        panic!("Expected Find entry");
    }
}

#[test]
fn FIND_操作符语法() {
    let q = parse_tql(r#"FIND {age: {$gt: 18}, heat: {$lte: 0.9}} RETURN *"#).unwrap();
    if let QueryEntry::Find { filter } = &q.entry {
        assert!(matches!(filter, Filter::And(_)));
    } else {
        panic!("Expected Find entry");
    }
}

#[test]
fn FIND_In操作符() {
    let q = parse_tql(r#"FIND {type: {$in: ["event", "incident"]}} RETURN *"#).unwrap();
    if let QueryEntry::Find { filter } = &q.entry {
        assert!(matches!(filter, Filter::In(_, _)));
    } else {
        panic!("Expected Find entry");
    }
}

#[test]
fn FIND_Exists操作符() {
    let q = parse_tql(r#"FIND {heat: {$exists: true}} RETURN *"#).unwrap();
    if let QueryEntry::Find { filter } = &q.entry {
        assert!(matches!(filter, Filter::Exists(_, true)));
    } else {
        panic!("Expected Find entry");
    }
}

#[test]
fn FIND_All操作符() {
    let q = parse_tql(r#"FIND {tags: {$all: ["AI", "security"]}} RETURN *"#).unwrap();
    if let QueryEntry::Find { filter } = &q.entry {
        assert!(matches!(filter, Filter::All(_, _)));
    } else {
        panic!("Expected Find entry");
    }
}

#[test]
fn FIND_逻辑组合_Or() {
    let q = parse_tql(r#"FIND {$or: [{type: "event"}, {type: "person"}]} RETURN *"#).unwrap();
    if let QueryEntry::Find { filter } = &q.entry {
        assert!(matches!(filter, Filter::Or(_)));
    } else {
        panic!("Expected Find entry with Or");
    }
}

#[test]
fn FIND_带LIMIT_OFFSET() {
    let q = parse_tql(r#"FIND {type: "event"} RETURN * LIMIT 10 OFFSET 20"#).unwrap();
    assert_eq!(q.limit, Some(10));
    assert_eq!(q.offset, Some(20));
}

#[test]
fn FIND_带ORDER_BY() {
    let q = parse_tql(r#"FIND {type: "event"} RETURN * ORDER BY a.score DESC LIMIT 5"#).unwrap();
    assert_eq!(q.order_by.len(), 1);
    assert!(q.order_by[0].descending);
    assert_eq!(q.limit, Some(5));
}

// ═══════════════════════════════════════════════════════════════════════
//  MATCH 入口测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn MATCH_单节点() {
    let q = parse_tql(r#"MATCH (a {name: "Alice"}) RETURN a"#).unwrap();
    if let QueryEntry::Match { pattern } = &q.entry {
        assert_eq!(pattern.nodes.len(), 1);
        assert_eq!(pattern.edges.len(), 0);
        assert_eq!(pattern.nodes[0].var, Some("a".into()));
        assert!(pattern.nodes[0].filter.is_some());
    } else {
        panic!("Expected Match entry");
    }
}

#[test]
fn MATCH_单跳路径() {
    let q = parse_tql(r#"MATCH (a)-[:knows]->(b) RETURN b"#).unwrap();
    if let QueryEntry::Match { pattern } = &q.entry {
        assert_eq!(pattern.nodes.len(), 2);
        assert_eq!(pattern.edges.len(), 1);
        assert_eq!(pattern.edges[0].labels, vec!["knows"]);
        assert!(pattern.edges[0].hop_range.is_none());
    } else {
        panic!("Expected Match entry");
    }
}

#[test]
fn MATCH_多跳路径() {
    let q = parse_tql(r#"MATCH (a)-[:knows]->(b)-[:works_at]->(c) RETURN c"#).unwrap();
    if let QueryEntry::Match { pattern } = &q.entry {
        assert_eq!(pattern.nodes.len(), 3);
        assert_eq!(pattern.edges.len(), 2);
        assert_eq!(pattern.edges[0].labels, vec!["knows"]);
        assert_eq!(pattern.edges[1].labels, vec!["works_at"]);
    } else {
        panic!("Expected Match entry");
    }
}

#[test]
fn MATCH_可变长路径() {
    let q = parse_tql(r#"MATCH (a)-[:knows*1..3]->(b) RETURN b"#).unwrap();
    if let QueryEntry::Match { pattern } = &q.entry {
        let hop = pattern.edges[0].hop_range.unwrap();
        assert_eq!(hop.min, 1);
        assert_eq!(hop.max, 3);
    } else {
        panic!("Expected Match entry");
    }
}

#[test]
fn MATCH_多标签边_管道分隔() {
    let q = parse_tql(r#"MATCH (a)-[:knows|works_with]->(b) RETURN b"#).unwrap();
    if let QueryEntry::Match { pattern } = &q.entry {
        assert_eq!(pattern.edges[0].labels, vec!["knows", "works_with"]);
    } else {
        panic!("Expected Match entry");
    }
}

#[test]
fn MATCH_多标签_可变长_组合() {
    let q = parse_tql(r#"MATCH (a)-[:knows|derived_from*1..5]->(b) RETURN b"#).unwrap();
    if let QueryEntry::Match { pattern } = &q.entry {
        assert_eq!(pattern.edges[0].labels, vec!["knows", "derived_from"]);
        let hop = pattern.edges[0].hop_range.unwrap();
        assert_eq!(hop.min, 1);
        assert_eq!(hop.max, 5);
    } else {
        panic!("Expected Match entry");
    }
}

#[test]
fn MATCH_任意边() {
    let q = parse_tql(r#"MATCH (a)-[]->(b) RETURN a, b"#).unwrap();
    if let QueryEntry::Match { pattern } = &q.entry {
        assert!(pattern.edges[0].labels.is_empty());
    } else {
        panic!("Expected Match entry");
    }
}

#[test]
fn FIND_RFC3339操作符() {
    let query = parse_tql(
        r#"FIND {createdAt: {$afterEq: "2026-08-21T08:00:00Z", $before: "2026-08-22T00:00:00Z"}} RETURN *"#,
    )
    .unwrap();
    if let QueryEntry::Find { filter } = query.entry {
        assert!(filter.matches(&serde_json::json!({
            "createdAt": "2026-08-21T16:30:00+08:00"
        })));
        assert!(!filter.matches(&serde_json::json!({
            "createdAt": "2026-08-22T00:00:00Z"
        })));
    } else {
        panic!("应解析为 FIND");
    }
}

#[test]
fn MATCH_内联Mongo操作符() {
    // Q1 决策 B: 内联属性支持 Mongo 操作符
    let q = parse_tql(r#"MATCH (a {age: {$gt: 18}}) RETURN a"#).unwrap();
    if let QueryEntry::Match { pattern } = &q.entry {
        assert!(pattern.nodes[0].filter.is_some());
        let f = pattern.nodes[0].filter.as_ref().unwrap();
        assert!(matches!(f, Filter::Range(_, _, _)));
    } else {
        panic!("Expected Match entry");
    }
}

#[test]
fn MATCH_RANK解析() {
    let query =
        parse_tql(r#"MATCH (doc)-[:CITES]->(ref) RANK doc BY VECTOR [1.0, 0.0] TOP 3 RETURN doc"#)
            .unwrap();
    let rank = query.rank.unwrap();
    assert_eq!(rank.var, "doc");
    assert_eq!(rank.vector, vec![1.0, 0.0]);
    assert_eq!(rank.top_k, 3);
}

#[test]
fn RANK关键字仍可作为Payload字段名() {
    let query =
        parse_tql("FIND {rank: {$gte: 2}} WHERE _.rank > 2 RETURN _.rank ORDER BY _.rank DESC")
            .unwrap();
    assert!(query.rank.is_none());
    assert_eq!(query.order_by.len(), 1);
}

#[test]
fn SEARCH_EXPAND方向解析() {
    let query =
        parse_tql(r#"SEARCH VECTOR [1.0, 0.0] TOP 3 EXPAND INCOMING [:CITES*1..2] RETURN *"#)
            .unwrap();
    if let QueryEntry::Search { expand, .. } = query.entry {
        let expand = expand.unwrap();
        assert_eq!(expand.direction, EdgeDirection::Backward);
        assert_eq!(expand.labels, vec!["CITES"]);
    } else {
        panic!("应解析为 SEARCH");
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  WHERE 统一谓词测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn WHERE_Cypher比较() {
    let q = parse_tql(r#"MATCH (a)-[:knows]->(b) WHERE b.age > 20 RETURN b"#).unwrap();
    assert!(q.predicate.is_some());
    if let Some(Predicate::Compare { op, .. }) = &q.predicate {
        assert!(matches!(op, TqlCompOp::Gt));
    } else {
        panic!("Expected Compare predicate");
    }
}

#[test]
fn WHERE_AND组合() {
    let q = parse_tql(r#"MATCH (a)-[:knows]->(b) WHERE a.name == "Alice" AND b.age > 20 RETURN b"#)
        .unwrap();
    assert!(matches!(q.predicate, Some(Predicate::And(_, _))));
}

#[test]
fn WHERE_OR组合() {
    let q = parse_tql(r#"MATCH (a)-[:knows]->(b) WHERE a.score > 0.5 OR b.type == "vip" RETURN b"#)
        .unwrap();
    assert!(matches!(q.predicate, Some(Predicate::Or(_, _))));
}

#[test]
fn WHERE_括号优先级() {
    let q =
        parse_tql(r#"MATCH (a)-[]->(b) WHERE (a.x > 1 OR a.y > 2) AND b.z == 3 RETURN b"#).unwrap();
    // 结构应为 And(Or(x>1, y>2), z==3)
    assert!(matches!(q.predicate, Some(Predicate::And(_, _))));
}

#[test]
fn WHERE_NOT() {
    let q = parse_tql(r#"MATCH (a) WHERE NOT a.deleted == true RETURN a"#).unwrap();
    assert!(matches!(q.predicate, Some(Predicate::Not(_))));
}

#[test]
fn WHERE_文档过滤() {
    let q = parse_tql(r#"FIND {type: "event"} WHERE {heat: {$gte: 0.7}} RETURN *"#).unwrap();
    if let Some(Predicate::DocFilter { var, .. }) = &q.predicate {
        assert!(var.is_none()); // FIND 场景下 var 为 None
    } else {
        panic!("Expected DocFilter predicate");
    }
}

#[test]
fn WHERE_变量绑定MATCHES() {
    let q = parse_tql(
        r#"MATCH (a)-[:reports_to]->(boss) WHERE boss MATCHES {level: {$in: ["director", "vp"]}} RETURN boss"#
    ).unwrap();
    if let Some(Predicate::DocFilter { var, filter }) = &q.predicate {
        assert_eq!(var.as_deref(), Some("boss"));
        assert!(matches!(filter, Filter::In(_, _)));
    } else {
        panic!("Expected DocFilter with var binding");
    }
}

#[test]
fn WHERE_混合Cypher和MATCHES() {
    let q = parse_tql(
        r#"MATCH (a)-[:knows]->(b) WHERE a.region == "cn" AND b MATCHES {role: {$in: ["admin"]}} RETURN b"#
    ).unwrap();
    assert!(matches!(q.predicate, Some(Predicate::And(_, _))));
}

// ═══════════════════════════════════════════════════════════════════════
//  SEARCH 入口测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn SEARCH_基础向量检索() {
    let q = parse_tql(r#"SEARCH VECTOR [0.1, 0.2, 0.3] TOP 10 RETURN *"#).unwrap();
    if let QueryEntry::Search {
        vector,
        top_k,
        expand,
    } = &q.entry
    {
        assert_eq!(vector.len(), 3);
        assert!((vector[0] - 0.1).abs() < 1e-10);
        assert_eq!(*top_k, 10);
        assert!(expand.is_none());
    } else {
        panic!("Expected Search entry");
    }
}

#[test]
fn SEARCH_带EXPAND() {
    let q =
        parse_tql(r#"SEARCH VECTOR [0.1, -0.2] TOP 20 EXPAND [:related*1..2] RETURN *"#).unwrap();
    if let QueryEntry::Search { expand, .. } = &q.entry {
        let ex = expand.as_ref().unwrap();
        assert_eq!(ex.labels, vec!["related"]);
        assert_eq!(ex.min_depth, 1);
        assert_eq!(ex.max_depth, 2);
    } else {
        panic!("Expected Search entry");
    }
}

#[test]
fn SEARCH_负数向量() {
    let q = parse_tql(r#"SEARCH VECTOR [-0.5, 0.3, -1] TOP 5 RETURN *"#).unwrap();
    if let QueryEntry::Search { vector, .. } = &q.entry {
        assert!((vector[0] - (-0.5)).abs() < 1e-10);
        assert!((vector[2] - (-1.0)).abs() < 1e-10);
    } else {
        panic!("Expected Search entry");
    }
}

#[test]
fn SEARCH_带WHERE过滤() {
    let q = parse_tql(r#"SEARCH VECTOR [0.1, 0.2] TOP 50 WHERE {type: "event"} RETURN *"#).unwrap();
    assert!(q.predicate.is_some());
}

#[test]
fn SEARCH_完整三融合() {
    let q = parse_tql(
        r#"SEARCH VECTOR [0.1, 0.2] TOP 20 EXPAND [:related*1..2] WHERE {heat: {$gte: 0.7}} RETURN * ORDER BY a.score DESC LIMIT 10"#
    ).unwrap();
    assert!(matches!(q.entry, QueryEntry::Search { .. }));
    assert!(q.predicate.is_some());
    assert_eq!(q.order_by.len(), 1);
    assert!(q.order_by[0].descending);
    assert_eq!(q.limit, Some(10));
}

// ═══════════════════════════════════════════════════════════════════════
//  RETURN 子句测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn RETURN_星号() {
    let q = parse_tql(r#"FIND {type: "event"} RETURN *"#).unwrap();
    assert!(matches!(q.returns, ReturnClause::All));
}

#[test]
fn RETURN_多变量() {
    let q = parse_tql(r#"MATCH (a)-[]->(b) RETURN a, b"#).unwrap();
    if let ReturnClause::Variables(vars) = &q.returns {
        assert_eq!(vars, &["a", "b"]);
    } else {
        panic!("Expected Variables return");
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  注释测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn 行注释被跳过() {
    let q = parse_tql("FIND {type: \"event\"} -- 查找所有事件\nRETURN *").unwrap();
    assert!(matches!(q.entry, QueryEntry::Find { .. }));
}

// ═══════════════════════════════════════════════════════════════════════
//  错误处理测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn 错误_缺少RETURN() {
    let result = parse_tql(r#"FIND {type: "event"}"#);
    assert!(result.is_err());
}

#[test]
fn 错误_空文档过滤() {
    let result = parse_tql(r#"FIND {} RETURN *"#);
    assert!(result.is_err());
}

#[test]
fn 错误_未知操作符() {
    let result = parse_tql(r#"FIND {age: {$unknown: 5}} RETURN *"#);
    assert!(result.is_err());
}

#[test]
fn 错误_跳数范围反转() {
    let result = parse_tql(r#"MATCH (a)-[:knows*5..1]->(b) RETURN b"#);
    assert!(result.is_err());
}

#[test]
fn 错误_匿名中间节点() {
    let result = parse_tql(r#"MATCH (a)-[:knows]->()-[:works_at]->(c) RETURN c"#);
    assert!(result.is_err());
}

#[test]
fn 错误_未知查询入口() {
    let result = parse_tql(r#"SELECT * FROM nodes"#);
    assert!(result.is_err());
}
