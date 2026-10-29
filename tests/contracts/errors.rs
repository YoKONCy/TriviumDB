//! 三端共用的真实 API 错误用例；预期字段来自独立 JSON 样例。

use serde_json::{Value, json};
use std::collections::HashMap;
use triviumdb::database::{AccessMode, Config};
use triviumdb::query::tql_prepared::TqlParamValue;
use triviumdb::{Database, TriviumError};

#[path = "../support/mod.rs"]
mod support;

fn error<T>(result: triviumdb::Result<T>) -> TriviumError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("契约要求该操作返回错误"),
    }
}

#[test]
fn 共享错误用例保留类别与完整字段() {
    let contract: Value = serde_json::from_str(include_str!("error_cases.json")).unwrap();
    for case in contract["cases"].as_array().unwrap() {
        let operation = case["operation"].as_str().unwrap();
        let root = support::TestDir::new(operation);
        let path = root.file("database.tdb");
        let mut db = Database::<f32>::open(&path, 2).unwrap();
        db.insert_with_id(1, &[1.0, 0.0], json!({"kind":"alpha"}))
            .unwrap();
        db.insert_with_id(2, &[0.0, 1.0], json!({"kind":"beta"}))
            .unwrap();
        let actual = match operation {
            "missing_parameter" | "unexpected_parameter" => {
                let prepared = db
                    .prepare_tql("MATCH (n) RETURN $required AS value")
                    .unwrap();
                let parameters = if operation == "missing_parameter" {
                    HashMap::new()
                } else {
                    HashMap::from([
                        ("required".into(), TqlParamValue::Int(1)),
                        ("extra".into(), TqlParamValue::Int(2)),
                    ])
                };
                error(db.execute_prepared_tql(&prepared, &parameters))
            }
            "dimension" => error(db.insert(&[1.0], json!({}))),
            "missing_node" => error(db.update_payload(999, json!({}))),
            "duplicate_node" => error(db.insert_with_id(1, &[1.0, 0.0], json!({}))),
            "unique" => {
                db.create_unique_index("kind").unwrap();
                error(db.insert_with_id(3, &[1.0, 0.0], json!({"kind":"alpha"})))
            }
            "readonly" | "rows" => {
                db.close().unwrap();
                db = Database::open_with_config(
                    &path,
                    Config {
                        dim: 2,
                        access_mode: AccessMode::ReadOnly,
                        max_query_rows: Some(1),
                        ..Default::default()
                    },
                )
                .unwrap();
                if operation == "readonly" {
                    error(db.insert(&[1.0, 0.0], json!({})))
                } else {
                    error(db.tql_values("MATCH (n) RETURN n"))
                }
            }
            "migration" => error(db.tql_mut("MATCH (n) RETURN n")),
            "closed" => {
                db.close().unwrap();
                error(db.get_payload(1))
            }
            "locked" => error(Database::<f32>::open(&path, 2)),
            "io" => {
                std::fs::write(root.file("blocked"), "文件占据父路径").unwrap();
                error(Database::<f32>::open(&root.file("blocked/database.tdb"), 2))
            }
            "version" => {
                // 58 字节为主文件头长度，4..6 是 little-endian 版本号。
                let mut header = [0u8; 58];
                header[..4].copy_from_slice(b"TVDB");
                header[4..6].copy_from_slice(&65535u16.to_le_bytes());
                let future = root.file("future.tdb");
                std::fs::write(&future, header).unwrap();
                error(Database::<f32>::open(&future, 2))
            }
            other => panic!("未知错误操作：{other}"),
        };
        assert_eq!(
            actual.code(),
            case["code"].as_str().unwrap(),
            "{operation}: {actual:?}"
        );
        let mut details = actual.details();
        for field in case["string_fields"].as_array().into_iter().flatten() {
            assert!(
                !details
                    .as_object_mut()
                    .unwrap()
                    .remove(field.as_str().unwrap())
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .is_empty()
            );
        }
        for field in case["integer_fields"].as_array().into_iter().flatten() {
            assert!(
                details
                    .as_object_mut()
                    .unwrap()
                    .remove(field.as_str().unwrap())
                    .unwrap()
                    .is_i64()
            );
        }
        assert_eq!(details, case["details"], "{operation}");
        if operation != "closed" {
            db.close().unwrap();
        }
    }
}
