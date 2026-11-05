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
fn oversized_query_reports_lex_stage_without_a_fabricated_anchor() {
    use triviumdb::query::{tql_lexer::MAX_TQL_INPUT_BYTES, tql_parser::parse_tql_diagnostic};
    let query = " ".repeat(MAX_TQL_INPUT_BYTES + 1);
    let info = serde_json::to_value(parse_tql_diagnostic(&query).unwrap_err().info()).unwrap();
    assert_eq!(info["code"], "TDB_QUERY_PARSE");
    assert_eq!(info["stage"], "lex");
    assert!(info.get("target").is_none());
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
                TqlParamValue::from_named_json(name, value).map(|value| (name.clone(), value))
            })
            .collect::<triviumdb::Result<HashMap<_, _>>>();
        let error = parameters
            .and_then(|values| db.execute_prepared_tql(&prepared, &values))
            .unwrap_err();
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
            assert_eq!(info["target"], json!({"parameter":case["parameter"]}));
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
        assert_eq!(info["target"], json!({"parameter":"x"}));
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
