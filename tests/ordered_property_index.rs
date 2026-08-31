use serde_json::json;
use triviumdb::Database;
use triviumdb::database::{AccessMode, Config};

const DIM: usize = 4;

fn path(name: &str) -> String {
    let directory = std::env::temp_dir().join("triviumdb_ordered_index_tests");
    std::fs::create_dir_all(&directory).unwrap();
    directory
        .join(format!("{name}_{}", std::process::id()))
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
        ".pidx",
        ".manifest.json",
    ] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

fn build(name: &str, nodes: usize) -> (String, Database<f32>) {
    let path = path(name);
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for sequence in 0..nodes {
        db.insert(
            &[sequence as f32, 0.0, 0.0, 0.0],
            json!({
                "score": sequence as i64 - (nodes as i64 / 2),
                "name": format!("name_{sequence:05}"),
                "bucket": sequence % 17,
            }),
        )
        .unwrap();
    }
    (path, db)
}

fn ids(db: &Database<f32>, query: &str) -> Vec<u64> {
    let mut ids: Vec<_> = db
        .tql(query)
        .unwrap()
        .into_iter()
        .flat_map(|row| row.into_values().map(|node| node.id))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[test]
fn ordered_index_数字和字符串范围与全扫描差分一致() {
    let (path, mut db) = build("range_differential", 2_000);
    let queries = [
        r#"FIND {score: {$gt: 100}} RETURN *"#,
        r#"FIND {score: {$gte: 100}} RETURN *"#,
        r#"FIND {score: {$lt: -100}} RETURN *"#,
        r#"FIND {score: {$lte: -100}} RETURN *"#,
        r#"FIND {name: {$gt: "name_01900"}} RETURN *"#,
        r#"FIND {name: {$lte: "name_00100"}} RETURN *"#,
    ];
    let expected: Vec<_> = queries.iter().map(|query| ids(&db, query)).collect();
    db.create_ordered_index("score").unwrap();
    db.create_ordered_index("name").unwrap();
    for (query, expected) in queries.iter().zip(expected) {
        assert_eq!(ids(&db, query), expected, "{query}");
    }
    drop(db);
    cleanup(&path);
}

#[test]
fn ordered_index_重启只读不可变和统计保持一致() {
    let (path, mut db) = build("persistence", 200);
    db.create_ordered_index("score").unwrap();
    db.flush().unwrap();
    db.publish_generation_manifest("ordered-index-generation")
        .unwrap();
    drop(db);

    let read_only = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            access_mode: AccessMode::ReadOnly,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        ids(&read_only, r#"FIND {score: {$gte: 90}} RETURN *"#).len(),
        10
    );
    let explain = read_only
        .tql(r#"EXPLAIN FIND {score: {$gte: 90}} RETURN *"#)
        .unwrap();
    assert_eq!(
        explain[0]["plan"].payload["access_path"]["kind"],
        "ordered_property_index"
    );
    assert_eq!(
        explain[0]["plan"].payload["property_index_stats"][0]["kind"],
        "ordered"
    );
    drop(read_only);

    let immutable = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            access_mode: AccessMode::Immutable,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        ids(&immutable, r#"FIND {score: {$lt: -90}} RETURN *"#).len(),
        10
    );
    drop(immutable);
    cleanup(&path);
}

#[test]
fn ordered_index_crud_同步且删除后无幽灵命中() {
    let (path, mut db) = build("crud", 10);
    db.create_ordered_index("score").unwrap();
    let id = db
        .insert(&[0.0; DIM], json!({"score": 1_000, "name": "special"}))
        .unwrap();
    assert!(ids(&db, r#"FIND {score: {$gt: 900}} RETURN *"#).contains(&id));
    db.update_payload(id, json!({"score": -1_000, "name": "special"}))
        .unwrap();
    assert!(!ids(&db, r#"FIND {score: {$gt: 900}} RETURN *"#).contains(&id));
    assert!(ids(&db, r#"FIND {score: {$lt: -900}} RETURN *"#).contains(&id));
    db.delete(id).unwrap();
    assert!(!ids(&db, r#"FIND {score: {$lt: -900}} RETURN *"#).contains(&id));
    drop(db);
    cleanup(&path);
}

#[test]
fn ordered_index_不同数字表示共享数值顺序() {
    let path = path("numeric_order");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for score in [
        json!(-2),
        json!(-1.5),
        json!(0),
        json!(1),
        json!(1.5),
        json!(2),
    ] {
        db.insert(&[0.0; DIM], json!({"score": score})).unwrap();
    }
    let expected = ids(&db, r#"FIND {score: {$gt: 0.0}} RETURN *"#);
    db.create_ordered_index("score").unwrap();
    assert_eq!(ids(&db, r#"FIND {score: {$gt: 0.0}} RETURN *"#), expected);
    drop(db);
    cleanup(&path);
}

#[test]
fn ordered_index_order_by_limit_保持排序并安全早停() {
    let (path, mut db) = build("order_limit", 2_000);
    let query = r#"FIND {score: {$gte: -1000}} RETURN * ORDER BY _.score DESC LIMIT 10"#;
    let expected = db.tql(query).unwrap();
    let expected_scores: Vec<_> = expected
        .iter()
        .map(|row| row["_"].payload["score"].as_i64().unwrap())
        .collect();
    db.create_ordered_index("score").unwrap();
    let actual = db.tql(query).unwrap();
    let actual_scores: Vec<_> = actual
        .iter()
        .map(|row| row["_"].payload["score"].as_i64().unwrap())
        .collect();
    assert_eq!(actual_scores, expected_scores);
    assert!(actual_scores.windows(2).all(|pair| pair[0] >= pair[1]));
    drop(db);
    cleanup(&path);
}

#[test]
fn ordered_index_sidecar_逐字节截断全部拒绝且不_panic() {
    let (path, mut db) = build("truncation", 50);
    db.create_ordered_index("score").unwrap();
    db.close().unwrap();
    let sidecar = format!("{path}.pidx");
    let original = std::fs::read(&sidecar).unwrap();
    for length in 0..original.len() {
        std::fs::write(&sidecar, &original[..length]).unwrap();
        let result = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: DIM,
                missing_index_policy: triviumdb::database::MissingIndexPolicy::Error,
                ..Default::default()
            },
        );
        assert!(result.is_err(), "截断到 {length} 字节时必须拒绝");
    }
    cleanup(&path);
}

#[test]
fn ordered_index_复杂类型安全回退全扫描() {
    let path = path("complex_fallback");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.insert(&[0.0; DIM], json!({"value": [1, 2]})).unwrap();
    db.create_ordered_index("value").unwrap();
    assert!(
        db.tql(r#"FIND {value: {$type: "array"}} RETURN *"#)
            .unwrap()
            .len()
            == 1
    );
    drop(db);
    cleanup(&path);
}
