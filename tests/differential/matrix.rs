use super::evaluator::evaluate;
use super::find_reference::{
    assert_aggregate_differential, assert_projection_distinct_differential,
};
use super::generator::{queries, query_tql};
use super::model::*;
use std::collections::BTreeMap;
use triviumdb::database::{Config, Database, StorageMode};
use triviumdb::query::tql_executor::TqlValue;

fn open(path: &str, mode: StorageMode) -> Database<f32> {
    Database::open_with_config(
        path,
        Config {
            dim: 3,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        },
    )
    .unwrap()
}

pub(crate) fn cleanup(path: &str) {
    for suffix in [
        "",
        ".wal",
        ".vec",
        ".lock",
        ".flush_ok",
        ".propidx",
        ".graph",
    ] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

pub(crate) fn seed(database: &mut Database<f32>, reference: &RefDatabase) {
    for node in reference.nodes.values() {
        database
            .insert_with_id(node.id, &[node.id as f32, 1.0, 0.0], node.payload.clone())
            .unwrap();
    }
    for edge in &reference.edges {
        database
            .link(edge.source, edge.target, &edge.label, 1.0)
            .unwrap();
    }
}

fn execute(database: &Database<f32>, query: &Query) -> Vec<CanonicalRow> {
    let tql = query_tql(query);
    match query {
        Query::Find { order, .. } => {
            let mut rows = database
                .tql_nodes(&tql)
                .unwrap_or_else(|error| panic!("TQL 失败: {tql}\n{error}"))
                .into_iter()
                .map(|row| {
                    let node = row.get("_").expect("FIND 必须返回隐式节点");
                    BTreeMap::from([("id".into(), RefScalar::Integer(node.id as i64))])
                })
                .collect::<Vec<_>>();
            if order.is_empty() {
                rows.sort_by_key(|row| row["id"].clone());
            }
            rows
        }
        Query::Match { .. } => {
            let mut rows = database
                .tql_nodes(&tql)
                .unwrap_or_else(|error| panic!("TQL 失败: {tql}\n{error}"))
                .into_iter()
                .map(|row| {
                    BTreeMap::from([
                        ("source".into(), RefScalar::Integer(row["a"].id as i64)),
                        ("target".into(), RefScalar::Integer(row["b"].id as i64)),
                    ])
                })
                .collect::<Vec<_>>();
            rows.sort_by_key(|row| (row["source"].clone(), row["target"].clone()));
            rows
        }
        Query::CountBy { .. } => database
            .tql_values(&tql)
            .unwrap_or_else(|error| panic!("TQL 失败: {tql}\n{error}"))
            .into_iter()
            .map(|row| {
                let group = match &row["bucket"] {
                    TqlValue::String(value) => RefScalar::String(value.clone()),
                    value => panic!("分组值类型错误: {value:?}"),
                };
                let count = match &row["total"] {
                    TqlValue::Int(value) => RefScalar::Integer(*value),
                    value => panic!("count 类型错误: {value:?}"),
                };
                BTreeMap::from([("group".into(), group), ("count".into(), count)])
            })
            .collect(),
    }
}

fn assert_query(database: &Database<f32>, reference: &RefDatabase, query: &Query, context: &str) {
    let expected = evaluate(reference, query);
    let actual = execute(database, query);
    assert_eq!(
        actual,
        expected,
        "{context}\nquery={query:?}\ntql={}",
        query_tql(query)
    );
}

fn run_matrix(mode: StorageMode, indexed: bool) {
    let path = std::env::temp_dir()
        .join("triviumdb_test")
        .join(format!("differential_{mode:?}_{indexed}.tdb"))
        .to_string_lossy()
        .to_string();
    std::fs::create_dir_all(std::env::temp_dir().join("triviumdb_test")).unwrap();
    cleanup(&path);
    let reference = RefDatabase::fixture(48);
    let mut database = open(&path, mode);
    seed(&mut database, &reference);
    if indexed {
        database.create_index("kind").unwrap();
        database.create_ordered_index("rank").unwrap();
        database.create_bitmap_index("active").unwrap();
        database
            .create_composite_index(&["kind".into(), "group".into()])
            .unwrap();
    }
    let cases = queries(0xD1FF_EA11, 40);
    for query in &cases {
        assert_query(&database, &reference, query, "写入后");
    }
    assert_aggregate_differential(&database, &reference);
    assert_projection_distinct_differential(&database, &reference);
    database.flush().unwrap();
    for query in &cases {
        assert_query(&database, &reference, query, "flush 后");
    }
    assert_aggregate_differential(&database, &reference);
    assert_projection_distinct_differential(&database, &reference);
    database.compact().unwrap();
    drop(database);
    let database = open(&path, mode);
    for query in &cases {
        assert_query(&database, &reference, query, "compact/reopen 后");
    }
    assert_aggregate_differential(&database, &reference);
    assert_projection_distinct_differential(&database, &reference);
    drop(database);
    cleanup(&path);
}

#[test]
fn FIND_MATCH_聚合在索引与持久化矩阵下和独立_reference一致() {
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        for indexed in [false, true] {
            run_matrix(mode, indexed);
        }
    }
}

#[test]
fn metamorphic_布尔恒等分页与索引不改变语义() {
    let reference = RefDatabase::fixture(32);
    let base = Predicate::Compare {
        field: "kind".into(),
        operation: CompareOp::Eq,
        value: RefScalar::String("alpha".into()),
    };
    let variants = [
        base.clone(),
        Predicate::And(Box::new(base.clone()), Box::new(Predicate::True)),
        Predicate::Or(Box::new(base.clone()), Box::new(Predicate::False)),
        Predicate::Not(Box::new(Predicate::Not(Box::new(base.clone())))),
    ];
    let expected = evaluate(
        &reference,
        &Query::Find {
            predicate: base,
            order: vec![Order {
                field: "rank".into(),
                direction: Direction::Ascending,
            }],
            offset: 0,
            limit: Some(100),
        },
    );
    for predicate in variants {
        let actual = evaluate(
            &reference,
            &Query::Find {
                predicate,
                order: vec![Order {
                    field: "rank".into(),
                    direction: Direction::Ascending,
                }],
                offset: 0,
                limit: Some(100),
            },
        );
        assert_eq!(actual, expected);
    }
}

#[test]
fn reference_缺失字段与异类型比较fail_closed() {
    let reference = RefDatabase::fixture(8);
    for predicate in [
        Predicate::Compare {
            field: "missing".into(),
            operation: CompareOp::Eq,
            value: RefScalar::String("x".into()),
        },
        Predicate::Compare {
            field: "rank".into(),
            operation: CompareOp::Greater,
            value: RefScalar::String("x".into()),
        },
    ] {
        assert!(
            evaluate(
                &reference,
                &Query::Find {
                    predicate,
                    order: Vec::new(),
                    offset: 0,
                    limit: None,
                }
            )
            .is_empty()
        );
    }
}
