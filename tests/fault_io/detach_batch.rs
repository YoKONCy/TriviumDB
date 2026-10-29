//! 删除集合内部有关联时，故障恢复仍只接受完整的旧状态或提交后的完整新状态。
use super::support::TestDir;
use serde_json::{Value, json};
use triviumdb::Database;
use triviumdb::database::{Config, StorageMode};
use triviumdb::storage::wal::SyncMode;
use triviumdb::test_hooks::{IoPoint, fail_io_at};

type EdgeState = (u64, String, f32, Value);
type NodeState = (u64, Vec<f32>, Value, Vec<EdgeState>);

fn snapshot(db: &Database<f32>) -> Vec<NodeState> {
    let mut ids = db.all_node_ids().unwrap();
    ids.sort_unstable();
    ids.into_iter()
        .map(|id| {
            let node = db.get(id).unwrap().unwrap();
            let mut edges = db
                .get_edges(id)
                .unwrap()
                .into_iter()
                .map(|edge| (edge.target_id, edge.label, edge.weight, edge.metadata))
                .collect::<Vec<_>>();
            edges.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
            (id, node.vector, node.payload, edges)
        })
        .collect()
}

#[test]
fn 关联集合删除在WAL和应用失败后恢复完整事务状态() {
    let fixture: Value =
        serde_json::from_str(include_str!("../contracts/system_boundaries.json")).unwrap();
    let old = fixture["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| {
            let id = id.as_u64().unwrap();
            let mut edges = fixture["edges"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|edge| edge[0].as_u64() == Some(id))
                .map(|edge| {
                    (
                        edge[1].as_u64().unwrap(),
                        edge[2].as_str().unwrap().to_owned(),
                        1.,
                        Value::Null,
                    )
                })
                .collect::<Vec<_>>();
            edges.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
            (id, vec![1., 0.], json!({"id":id}), edges)
        })
        .collect::<Vec<NodeState>>();
    let new = vec![
        (
            3,
            vec![1., 0.],
            json!({"id":3}),
            vec![(4, "keep".into(), 1., Value::Null)],
        ),
        (4, vec![1., 0.], json!({"id":4}), vec![]),
    ];
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for point in [
            IoPoint::WalFlush,
            IoPoint::WalSync,
            IoPoint::TransactionApply,
        ] {
            let root = TestDir::new("detach_fault");
            let path = root.file("data.tdb");
            let config = Config {
                dim: 2,
                storage_mode: mode,
                sync_mode: SyncMode::Full,
                auto_build_quiver: false,
                ..Default::default()
            };
            let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
            for node in &old {
                db.insert_with_id(node.0, &node.1, node.2.clone()).unwrap();
            }
            for edge in fixture["edges"].as_array().unwrap() {
                db.link(
                    edge[0].as_u64().unwrap(),
                    edge[1].as_u64().unwrap(),
                    edge[2].as_str().unwrap(),
                    1.,
                )
                .unwrap();
            }
            db.flush().unwrap();
            {
                let _failure = fail_io_at(point);
                assert!(
                    db.tql_mut(fixture["delete_queries"][0].as_str().unwrap())
                        .is_err()
                );
            }
            drop(db);
            let mut recovered = Database::<f32>::open_with_config(&path, config).unwrap();
            let actual = snapshot(&recovered);
            if point == IoPoint::TransactionApply {
                assert_eq!(actual, new);
            } else {
                assert!(
                    actual == old || actual == new,
                    "{mode:?}/{point:?}: {actual:?}"
                );
            }
            assert!(recovered.validate_graph().unwrap().valid);
            recovered.close().unwrap();
            drop(recovered);
            let mut again = Database::<f32>::open_with_config(&path, config).unwrap();
            assert_eq!(snapshot(&again), actual);
            again.close().unwrap();
        }
    }
}
