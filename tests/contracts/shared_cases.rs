//! 语言无关 JSON 公共契约的 Rust 权威适配器。

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use triviumdb::Database;
use triviumdb::query::tql_prepared::TqlParamValue;

#[derive(Deserialize)]
struct Contract {
    schema_version: u32,
    setup: Setup,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Setup {
    dimension: usize,
    nodes: Vec<NodeSetup>,
    edges: Vec<EdgeSetup>,
}

#[derive(Deserialize)]
struct NodeSetup {
    id: u64,
    vector: Vec<f32>,
    payload: Value,
}

#[derive(Deserialize)]
struct EdgeSetup {
    source: u64,
    target: u64,
    label: String,
    weight: f32,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    operation: String,
    #[serde(default)]
    node_id: Option<u64>,
    #[serde(default)]
    tql: Option<String>,
    #[serde(default)]
    parameters: HashMap<String, Value>,
    #[serde(default)]
    documents: Vec<triviumdb::index::text::TextIndexDocument>,
    expected: Value,
}

fn parameter(value: &Value) -> TqlParamValue {
    TqlParamValue::from_json(value).expect("共享契约仅允许公共 Prepared 标量参数")
}

#[test]
fn 共享json契约由rust适配器完整执行() {
    let contract: Contract = serde_json::from_str(include_str!("public_cases.json")).unwrap();
    assert_eq!(contract.schema_version, 1);
    let root = std::env::temp_dir().join("triviumdb_shared_contract_rust");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.tdb").to_string_lossy().to_string();
    for suffix in [
        "",
        ".wal",
        ".vec",
        ".lock",
        ".flush_ok",
        ".graph",
        ".propidx",
        ".text",
        ".text.meta",
        ".quiver",
        ".quiver.meta",
    ] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
    let mut database = Database::<f32>::open(&path, contract.setup.dimension).unwrap();
    for node in contract.setup.nodes {
        database
            .insert_with_id(node.id, &node.vector, node.payload)
            .unwrap();
    }
    for edge in contract.setup.edges {
        database
            .link(edge.source, edge.target, &edge.label, edge.weight)
            .unwrap();
    }

    for case in contract.cases {
        match case.operation.as_str() {
            "get_payload" => {
                let payload = database
                    .get_payload(case.node_id.unwrap())
                    .unwrap()
                    .unwrap();
                let field = case.expected["field"].as_str().unwrap();
                assert_eq!(payload[field], case.expected["value"], "{}", case.name);
            }
            "prepared_tql" => {
                let prepared = database.prepare_tql(case.tql.as_deref().unwrap()).unwrap();
                let parameters = case
                    .parameters
                    .iter()
                    .map(|(name, value)| (name.clone(), parameter(value)))
                    .collect();
                let rows = database
                    .execute_prepared_tql(&prepared, &parameters)
                    .unwrap();
                assert_eq!(
                    rows.len(),
                    case.expected["row_count"].as_u64().unwrap() as usize
                );
                let column = case.expected["column"].as_str().unwrap();
                let actual = match &rows[0][column] {
                    triviumdb::query::tql_executor::TqlValue::Int(value) => *value as f64,
                    triviumdb::query::tql_executor::TqlValue::Float(value) => *value,
                    other => panic!("共享契约期待数值，实际为 {other:?}"),
                };
                assert_eq!(actual, case.expected["value"].as_f64().unwrap());
            }
            "tql_path" => {
                let rows = database.tql_values(case.tql.as_deref().unwrap()).unwrap();
                let expected = case.expected["path"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_u64().unwrap())
                    .collect::<Vec<_>>();
                assert!(
                    matches!(&rows[0]["path"], triviumdb::query::tql_executor::TqlValue::Path(path) if path == &expected)
                );
            }
            "prepared_missing_parameter" => {
                let prepared = database.prepare_tql(case.tql.as_deref().unwrap()).unwrap();
                let error = database
                    .execute_prepared_tql(&prepared, &HashMap::new())
                    .unwrap_err();
                assert_eq!(error.code(), case.expected["error_code"].as_str().unwrap());
                assert_eq!(error.details(), case.expected["error_details"]);
            }
            "text_lifecycle" => {
                let keyword = case.parameters["keyword"].as_str().unwrap();
                database
                    .index_text(1, case.parameters["old_text"].as_str().unwrap())
                    .unwrap();
                database.index_keyword(1, keyword).unwrap();
                database.index_keyword(1, keyword).unwrap();
                database.index_keyword(2, keyword).unwrap();
                database.build_text_index().unwrap();
                database
                    .index_text(1, case.parameters["new_text"].as_str().unwrap())
                    .unwrap();
                for phase in ["before_delete", "after_delete"] {
                    if phase == "after_delete" {
                        database.delete(1).unwrap();
                    }
                    for check in case.expected[phase].as_array().unwrap() {
                        let rows = database
                            .tql_values(check["query"].as_str().unwrap())
                            .unwrap();
                        assert_eq!(
                            rows.len() as u64,
                            check["count"].as_u64().unwrap(),
                            "{}",
                            case.name
                        );
                    }
                }
            }
            "text_rebuild" => {
                database.rebuild_text_index(&case.documents).unwrap();
                for check in case.expected["checks"].as_array().unwrap() {
                    assert_eq!(
                        database
                            .tql_values(check["query"].as_str().unwrap())
                            .unwrap()
                            .len() as u64,
                        check["count"].as_u64().unwrap(),
                        "{}",
                        case.name
                    );
                }
            }
            other => panic!("未知共享契约操作：{other}"),
        }
    }
    database.close().unwrap();
    assert!(
        database.build_text_index().is_err(),
        "关闭后的文本构建错误必须透传"
    );
}
