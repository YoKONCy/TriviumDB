//! 写入结果由真实提交阶段与事务所有权提供，拒绝后核对事实和后续重开。

use super::support::TestDir;
use serde_json::{Value, json};
use std::sync::{Arc, Barrier};
use triviumdb::database::TxBuilder;
use triviumdb::{Database, SharedDatabase, TriviumError};

fn expected(name: &str) -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("../contracts/write_outcomes.json")).unwrap();
    fixture[name].clone()
}

#[test]
fn 共享事务拒绝保留操作而内核预检拒绝消费操作() {
    let root = TestDir::new("write_outcomes");
    let path = root.file("db.tdb");
    let database = SharedDatabase::<f32>::open(&path, 2).unwrap();
    database
        .try_write(|core| core.insert_with_id(1, &[1., 0.], json!({"state":"seed"})))
        .unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    let mut transaction = TxBuilder::new();
    transaction.insert_with_id(2, &[0., 1.], json!({"state":"saved"}));
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        let reader = database.clone();
        let entered_reader = entered.clone();
        let release_reader = release.clone();
        let task = scope.spawn(move || {
            reader.try_read(|core| {
                entered_reader.wait();
                release_reader.wait();
                core.get_payload(1)
            })
        });
        entered.wait();
        let error = database.commit_tx(&mut transaction).unwrap_err();
        release.wait();
        assert_eq!(task.join().unwrap().unwrap(), Some(json!({"state":"seed"})));
        assert_eq!(error.code(), "TDB_BUSY");
        assert_eq!(
            serde_json::to_value(error.info()).unwrap()["write"],
            expected("admission")
        );
    });
    assert_eq!(transaction.pending_count(), 1);
    let error = database
        .try_read(|_| database.commit_tx(&mut transaction))
        .unwrap_err();
    assert_eq!(error.code(), "TDB_REENTRANT_ACCESS");
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        expected("admission")
    );
    assert_eq!(transaction.pending_count(), 1);
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert_eq!(database.commit_tx(&mut transaction).unwrap(), vec![2]);
    assert_eq!(transaction.pending_count(), 0);
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    transaction.insert_with_id(3, &[1.], json!({"state":"rejected"}));
    let error = database.commit_tx(&mut transaction).unwrap_err();
    assert!(matches!(
        error.underlying(),
        TriviumError::DimensionMismatch {
            expected: 2,
            got: 1
        }
    ));
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        expected("validation")
    );
    assert_eq!(transaction.pending_count(), 0);
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    database.close().unwrap();
    transaction.insert_with_id(3, &[1., 0.], json!({"state":"closed"}));
    let error = database.commit_tx(&mut transaction).unwrap_err();
    assert_eq!(error.code(), "TDB_DATABASE_CLOSED");
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        expected("admission")
    );
    assert_eq!(transaction.pending_count(), 1);
    let mut reopened = Database::<f32>::open(&path, 2).unwrap();
    assert_eq!(reopened.all_node_ids().unwrap(), vec![1, 2]);
    for (id, vector, state) in [(1, vec![1., 0.], "seed"), (2, vec![0., 1.], "saved")] {
        let node = reopened.get(id).unwrap().unwrap();
        assert_eq!(node.vector, vector);
        assert_eq!(node.payload, json!({"state":state}));
        assert!(node.edges.is_empty());
    }
    reopened.close().unwrap();
    let error = reopened.commit_tx(transaction).unwrap_err();
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        expected("core_admission")
    );
}
