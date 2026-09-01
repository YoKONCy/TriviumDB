use std::collections::HashMap;
use triviumdb::database::Database;
use triviumdb::query::tql_executor::TqlValue;
use triviumdb::query::tql_prepared::TqlParamValue;

fn canonical(rows: Vec<HashMap<String, TqlValue<f32>>>) -> Vec<u64> {
    let mut output = rows
        .into_iter()
        .map(|row| {
            row.values()
                .find_map(|value| match value {
                    TqlValue::Node(node) => Some(node.id),
                    _ => None,
                })
                .expect("查询行必须包含节点")
        })
        .collect::<Vec<_>>();
    output.sort_unstable();
    output
}

#[test]
fn Prepared与Direct在多组参数下逐行一致() {
    let root = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&root).unwrap();
    let path = root
        .join("prepared_differential.tdb")
        .to_string_lossy()
        .to_string();
    for suffix in ["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
    let mut database = Database::<f32>::open(&path, 2).unwrap();
    for id in 1..=24 {
        database
            .insert_with_id(
                id,
                &[id as f32, 1.0],
                serde_json::json!({"rank": id as i64}),
            )
            .unwrap();
    }
    let prepared = database
        .prepare_tql("MATCH (node) WHERE node.rank >= $min AND node.rank <= $max RETURN node, node.rank AS rank")
        .unwrap();
    assert_eq!(prepared.parameter_names(), vec!["max", "min"]);
    for (min, max) in [(1, 1), (3, 8), (10, 24), (30, 40)] {
        let direct = database
            .tql_values(&format!(
                "MATCH (node) WHERE node.rank >= {min} AND node.rank <= {max} RETURN node, node.rank AS rank"
            ))
            .unwrap();
        let parameters = HashMap::from([
            ("min".into(), TqlParamValue::Int(min)),
            ("max".into(), TqlParamValue::Int(max)),
        ]);
        let actual = database
            .execute_prepared_tql(&prepared, &parameters)
            .unwrap();
        assert_eq!(canonical(actual), canonical(direct));
    }
    assert!(
        database
            .execute_prepared_tql(&prepared, &HashMap::new())
            .is_err()
    );
}
