//! 同一独立样例覆盖公开查询入口、绑定共享协议及拒绝后的完整事实。

use serde_json::{Value, json};
use triviumdb::database::Config;
use triviumdb::{ConsoleOptions, ConsoleSession, Database, TriviumError};

fn check(error: TriviumError, case: &Value) {
    let info = serde_json::to_value(error.info()).unwrap();
    assert_eq!(info["schema_version"], 1, "{case}");
    assert_eq!(info["code"], "TDB_QUERY_PARSE", "{case}");
    assert_eq!(info["stage"], case["stage"], "{case}");
    assert_eq!(info["target"]["query_span"], case["span"], "{case}");
    if case["mutation"] == true {
        assert_eq!(
            info["write"],
            json!({"phase":"validation","outcome":"not_committed"})
        );
    }
    if case["span"].is_null() {
        assert!(info.get("target").is_none(), "{info}");
    }
    let details = info["details"].as_object().unwrap();
    assert_eq!(details.len(), 1);
    assert!(!details["reason"].as_str().unwrap().is_empty());
}

#[test]
fn query_diagnostic_contract_preserves_data_and_source_positions() {
    let cases: Value =
        serde_json::from_str(include_str!("../contracts/query_diagnostics.json")).unwrap();
    let root = crate::support::TestDir::new("query_diagnostics");
    let file = root.file("diagnostics.tdb");
    let config = Config {
        dim: 2,
        auto_build_quiver: false,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(&file, config).unwrap();
    db.insert_with_id(7, &[1., 0.], json!({"saved":"完整事实"}))
        .unwrap();
    let session = ConsoleSession::new(ConsoleOptions {
        read_only: false,
        ..Default::default()
    })
    .unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let query = case["query"].as_str().unwrap();
        let mutation = case["mutation"].as_bool().unwrap_or(false);
        if mutation {
            check(db.tql_mut(query).unwrap_err(), case);
        } else {
            check(db.tql(query).unwrap_err(), case);
            check(db.query(query).unwrap_err(), case);
            check(db.tql_values(query).unwrap_err(), case);
            check(db.tql_nodes(query).unwrap_err(), case);
            check(db.prepare_tql(query).unwrap_err(), case);
            check(
                db.tql_with_control(query, Default::default()).unwrap_err(),
                case,
            );
        }
        let request = json!({"version":1,"requestId":case["name"],"command":{
            "operation":if mutation {"mutate"} else {"query"},"query":query
        }});
        check(
            db.console_request(&session, &request.to_string())
                .unwrap_err(),
            case,
        );
    }
    db.close().unwrap();
    let mut reopened = Database::<f32>::open_with_config(&file, config).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), vec![7]);
    let node = reopened.get(7).unwrap().unwrap();
    assert_eq!(node.vector, vec![1., 0.]);
    assert_eq!(node.payload, json!({"saved":"完整事实"}));
    assert!(node.edges.is_empty());
    reopened.close().unwrap();
}

#[test]
fn query_span_rejects_invalid_utf8_boundaries_and_counts_newlines() {
    use triviumdb::error::QuerySpan;
    assert!(QuerySpan::at("中", 1).is_none());
    assert!(QuerySpan::at("中", 4).is_none());
    assert!(QuerySpan::range("中", 0, 1).is_none());
    assert!(QuerySpan::range("中", 3, 0).is_none());
    assert_eq!(QuerySpan::range("中", 0, 3).unwrap().end_byte, 3);
    assert_eq!(
        QuerySpan::at("a\rb\nc\r\n\t🙂", 12),
        Some(QuerySpan {
            start_byte: 12,
            end_byte: 12,
            line: 4,
            column: 3,
        })
    );
}

#[test]
fn input_budgets_report_lex_stage_with_only_confirmed_positions() {
    use triviumdb::query::{
        tql_lexer::{MAX_TQL_INPUT_BYTES, MAX_TQL_TOKENS},
        tql_parser::parse_tql_diagnostic,
    };
    let query = " ".repeat(MAX_TQL_INPUT_BYTES + 1);
    let info = serde_json::to_value(parse_tql_diagnostic(&query).unwrap_err().info()).unwrap();
    assert_eq!(info["code"], "TDB_QUERY_PARSE");
    assert_eq!(info["stage"], "lex");
    assert!(info.get("target").is_none());
    let query = "+ ".repeat(MAX_TQL_TOKENS);
    let info = serde_json::to_value(parse_tql_diagnostic(&query).unwrap_err().info()).unwrap();
    assert_eq!(info["code"], "TDB_QUERY_PARSE");
    assert_eq!(info["stage"], "lex");
    // 每个 token 占一个字节，最后一个 token 后的空格是预算拒绝边界。
    let boundary = MAX_TQL_TOKENS * 2 - 1;
    assert_eq!(
        info["target"],
        json!({"query_span":{
            "start_byte":boundary,"end_byte":boundary,"line":1,"column":boundary + 1
        }})
    );
}

#[test]
fn token_ranges_preserve_escaping_and_exclude_comments() {
    use triviumdb::query::tql_lexer::{TqlLexer, TqlToken};
    let input = "'a\\'中' -- $ignored\r\n$x >= 12";
    let tokens = TqlLexer::new(input).tokenize_with_spans().unwrap();
    let actual = tokens
        .into_iter()
        .map(|token| (token.token, token.byte_start, token.byte_end))
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        vec![
            (TqlToken::StringLit("a'中".into()), 0, 8),
            (TqlToken::DollarOp("$x".into()), 22, 24),
            (TqlToken::Gte, 25, 27),
            (TqlToken::IntLit(12), 28, 30),
            (TqlToken::Eof, 30, 30),
        ]
    );
}

#[test]
fn prepared_source_outlives_input_and_database_without_holding_files() {
    use std::collections::HashMap;
    use triviumdb::query::tql_prepared::{PreparedTql, TqlParamValue};
    let root = crate::support::TestDir::new("prepared_source_lifecycle");
    let file = root.file("db.tdb");
    let mut db = Database::<f32>::open(&file, 2).unwrap();
    db.insert_with_id(7, &[1., 0.], json!({"saved":"完整事实"}))
        .unwrap();
    let prepared = {
        let input = String::from("MATCH (n) RETURN $x AS value");
        db.prepare_tql(&input).unwrap()
    };
    let clone = prepared.clone();
    let error = db
        .execute_prepared_tql(&clone, &HashMap::new())
        .unwrap_err();
    let expected = json!({"parameter":"x","query_span":{
        "start_byte":17,"end_byte":19,"line":1,"column":18
    }});
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["target"],
        expected
    );
    db.close().unwrap();
    let moved = root.file("moved.tdb");
    std::fs::rename(&file, &moved).unwrap();
    std::fs::rename(&moved, &file).unwrap();
    let mut reopened = Database::<f32>::open(&file, 2).unwrap();
    let node = reopened.get(7).unwrap().unwrap();
    assert_eq!(node.vector, vec![1., 0.]);
    assert_eq!(node.payload, json!({"saved":"完整事实"}));
    assert!(node.edges.is_empty());
    let rows = reopened
        .execute_prepared_tql(
            &clone,
            &HashMap::from([("x".into(), TqlParamValue::Int(42))]),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 1);
    assert!(matches!(
        rows[0]["value"],
        triviumdb::query::tql_executor::TqlValue::Int(42)
    ));
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["target"],
        expected
    );
    let query = triviumdb::query::tql_parser::parse_tql("MATCH (n) RETURN $x AS value").unwrap();
    let unlocated = PreparedTql::from_query(query)
        .bind(&HashMap::new())
        .unwrap_err();
    assert!(matches!(
        unlocated,
        TriviumError::MissingQueryParameter { .. }
    ));
    assert_eq!(
        serde_json::to_value(unlocated.info()).unwrap()["target"],
        json!({"parameter":"x"})
    );
    let standalone = TqlParamValue::from_named_json("x", &json!([1])).unwrap_err();
    assert_eq!(standalone.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        serde_json::to_value(standalone.info()).unwrap()["target"],
        json!({"parameter":"x"})
    );
    reopened.close().unwrap();
}

#[test]
fn prepared参数错误定位保留原始类别及数据库事实() {
    use std::collections::HashMap;
    use triviumdb::query::tql_prepared::TqlParamValue;
    let cases: Value =
        serde_json::from_str(include_str!("../contracts/prepared_diagnostics.json")).unwrap();
    let root = crate::support::TestDir::new("prepared_diagnostics");
    let file = root.file("db.tdb");
    let mut db = Database::<f32>::open(&file, 2).unwrap();
    db.insert_with_id(7, &[1., 0.], json!({"saved":"完整事实"}))
        .unwrap();
    let session = ConsoleSession::new(ConsoleOptions::default()).unwrap();
    let before = std::fs::read(format!("{file}.wal")).unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let query = case["query"].as_str().unwrap();
        let prepared = db.prepare_tql(query).unwrap();
        let parameters = case["parameters"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| {
                prepared
                    .parameter_from_json(name, value)
                    .map(|value| (name.clone(), value))
            })
            .collect::<triviumdb::Result<HashMap<_, _>>>();
        let conversion_rejected = case["conversion"] == true;
        let error = if conversion_rejected {
            parameters.unwrap_err()
        } else {
            db.execute_prepared_tql(&prepared, &parameters.unwrap())
                .unwrap_err()
        };
        for error in [
            error,
            db.console_request(
                &session,
                &json!({"version":1,"requestId":case["name"],"command":{
                    "operation":"query","query":query,"parameters":case["parameters"]
                }})
                .to_string(),
            )
            .unwrap_err(),
        ] {
            let info = serde_json::to_value(error.info()).unwrap();
            assert_eq!(info["code"], case["code"], "{case}");
            assert_eq!(info["stage"], "bind");
            let mut target = json!({"parameter":case["parameter"]});
            if let Some(span) = case.get("span") {
                target["query_span"] = span.clone();
            }
            assert_eq!(info["target"], target, "{case}");
            if let Some(details) = case.get("details") {
                assert_eq!(&info["details"], details);
            } else {
                assert_eq!(info["details"].as_object().unwrap().len(), 1);
                assert!(!info["details"]["reason"].as_str().unwrap().is_empty());
            }
        }
    }
    let prepared = db.prepare_tql("MATCH (n) RETURN $x AS value").unwrap();
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let error = db
            .execute_prepared_tql(
                &prepared,
                &HashMap::from([("x".into(), TqlParamValue::Float(value))]),
            )
            .unwrap_err();
        assert!(matches!(
            error.underlying(),
            TriviumError::QueryExecution(_)
        ));
        let info = serde_json::to_value(error.info()).unwrap();
        assert_eq!(info["stage"], "bind");
        assert_eq!(
            info["target"],
            json!({"parameter":"x","query_span":{
                "start_byte":17,"end_byte":19,"line":1,"column":18
            }})
        );
    }
    let rows = db
        .execute_prepared_tql(
            &prepared,
            &HashMap::from([("x".into(), TqlParamValue::Int(42))]),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(matches!(
        rows[0]["value"],
        triviumdb::query::tql_executor::TqlValue::Int(42)
    ));
    assert_eq!(std::fs::read(format!("{file}.wal")).unwrap(), before);
    db.close().unwrap();
    let mut reopened = Database::<f32>::open(&file, 2).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), vec![7]);
    let node = reopened.get(7).unwrap().unwrap();
    assert_eq!(node.vector, vec![1., 0.]);
    assert_eq!(node.payload, json!({"saved":"完整事实"}));
    assert!(node.edges.is_empty());
    reopened.close().unwrap();
}
