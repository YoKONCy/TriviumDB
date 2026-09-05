use serde_json::json;
use std::collections::HashMap;
use triviumdb::database::{Config, Database};
use triviumdb::index::property::PropertyIndexKind;
use triviumdb::query::tql_executor::TqlValue;
use triviumdb::query::tql_prepared::TqlParamValue;

fn path(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("triviumdb_api_alignment_{name}.tdb"))
        .to_string_lossy()
        .into_owned()
}

fn cleanup(path: &str) {
    for suffix in [
        "",
        ".vec",
        ".wal",
        ".lock",
        ".flush_ok",
        ".quiver",
        ".quiver.meta",
        ".text",
        ".text.meta",
        ".pidx",
        ".gidx",
        ".manifest.json",
    ] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

#[test]
fn 四类属性索引和存储格式可结构化观测() {
    let path = path("index_info");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: 2,
            ..Default::default()
        },
    )
    .unwrap();
    db.insert(
        &[1.0, 0.0],
        json!({"kind": "note", "score": 1, "tenant": "a", "region": "x"}),
    )
    .unwrap();
    db.create_index("kind").unwrap();
    db.create_ordered_index("score").unwrap();
    db.create_composite_index(&["tenant".into(), "region".into()])
        .unwrap();
    db.create_bitmap_index("region").unwrap();

    let info = db.index_info();
    assert_eq!(info.len(), 4);
    assert!(info.iter().any(|item| item.kind == PropertyIndexKind::Hash));
    assert!(
        info.iter()
            .any(|item| item.kind == PropertyIndexKind::Ordered)
    );
    assert!(info.iter().any(
        |item| item.kind == PropertyIndexKind::Composite && item.fields == ["tenant", "region"]
    ));
    assert!(
        info.iter()
            .any(|item| item.kind == PropertyIndexKind::Bitmap)
    );

    let storage = db.storage_info();
    assert_eq!(storage.database_format_current, 9);
    assert_eq!(storage.property_index_format, 6);
    assert_eq!(storage.graph_index_format, 2);
    assert_eq!(storage.wal_format, 3);
    assert_eq!(storage.dim, 2);
    assert_eq!(storage.node_count, 1);
    cleanup(&path);
}

#[test]
fn prepared_tql和一等路径列表值可通过公共_api执行() {
    let path = path("prepared");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    db.insert_with_id(1, &[1.0, 0.0], json!({"kind": "a"}))
        .unwrap();
    db.insert_with_id(2, &[0.9, 0.1], json!({"kind": "b"}))
        .unwrap();
    db.link(1, 2, "next", 1.0).unwrap();

    let prepared = db
        .prepare_tql(
            "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed shortest_paths seed TO [2] LABEL next AS route WITH route RETURN path(route) AS path, collect(route.id) AS ids, $bonus + path_length(route) AS score",
        )
        .unwrap();
    assert_eq!(prepared.parameter_names(), vec!["bonus"]);
    let rows = db
        .execute_prepared_tql(
            &prepared,
            &HashMap::from([("bonus".into(), TqlParamValue::Int(10))]),
        )
        .unwrap();
    assert!(matches!(&rows[0]["path"], TqlValue::Path(value) if value == &vec![1, 2]));
    assert!(matches!(&rows[0]["ids"], TqlValue::List(value) if value == &vec![json!(2)]));
    assert!(matches!(rows[0]["score"], TqlValue::Float(value) if value == 11.0));
    cleanup(&path);
}

#[test]
fn 历史静默_api全部明确拒绝() {
    let path = path("removed");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    let id = db.insert(&[1.0, 0.0], json!({"value": 1})).unwrap();

    assert!(matches!(
        db.patch_payload(id, json!({"value": 2})),
        Err(triviumdb::TriviumError::ApiMigrationRequired { .. })
    ));
    assert!(matches!(
        db.tql_mut("FIND {value: 1} RETURN *"),
        Err(triviumdb::TriviumError::ApiMigrationRequired { .. })
    ));
    cleanup(&path);
}
