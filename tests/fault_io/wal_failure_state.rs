//! 同步失败后的隔离状态必须覆盖普通、批量及事务写入，并经历进程退出和恢复。
use super::support;

use serde_json::json;
use triviumdb::database::{Config, StorageMode, TxBuilder};
use triviumdb::storage::wal::{SyncMode, Wal, WalEntry};
use triviumdb::test_hooks::{IoPoint, fail_io_at};
use triviumdb::{Database, TriviumError};

fn config(mode: StorageMode) -> Config {
    Config {
        dim: 2,
        storage_mode: mode,
        sync_mode: SyncMode::Full,
        auto_build_quiver: false,
        ..Default::default()
    }
}

#[test]
fn 事务应用失败拒绝部分状态并从完整wal恢复() {
    let root = support::TestDir::new("transaction_apply_failure");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        db.insert_with_id(1, &[1., 0.], json!({"state":"confirmed"}))
            .unwrap();
        db.flush().unwrap();
        let retained = db.quiver_build_handle();
        let mut tx = TxBuilder::new();
        tx.update_payload(1, json!({"state":"updated"}));
        tx.insert_with_id(2, &[0., 1.], json!({"state":"inserted"}));
        let guard = fail_io_at(IoPoint::TransactionApply);
        assert!(matches!(db.commit_tx(tx), Err(TriviumError::Io(_))));
        drop(guard);
        let before = std::fs::read(format!("{path}.wal")).unwrap();
        assert!(matches!(db.get_payload(1), Err(TriviumError::WalClosed)));
        assert!(matches!(db.node_count(), Err(TriviumError::WalClosed)));
        assert!(matches!(
            db.search(&[1., 0.], 1, 0, 0.),
            Err(TriviumError::WalClosed)
        ));
        assert!(matches!(
            retained.prepare_auto_build(),
            Err(TriviumError::WalClosed)
        ));
        assert!(matches!(
            db.insert(&[1., 1.], json!({})),
            Err(TriviumError::WalClosed)
        ));
        assert!(matches!(db.flush(), Err(TriviumError::WalClosed)));
        assert!(matches!(db.close(), Err(TriviumError::WalClosed)));
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
        drop(retained);
        drop(db);
        let mut recovered = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(
            recovered.get_payload(1).unwrap(),
            Some(json!({"state":"updated"}))
        );
        assert_eq!(recovered.get(2).unwrap().unwrap().vector, [0., 1.]);
        assert_eq!(
            recovered.get_payload(2).unwrap(),
            Some(json!({"state":"inserted"}))
        );
        assert_eq!(recovered.node_count().unwrap(), 2);
        recovered
            .insert_with_id(3, &[1., 1.], json!({"state":"after-recovery"}))
            .unwrap();
        recovered.close().unwrap();
        let mut reopened = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(
            reopened.get_payload(1).unwrap(),
            Some(json!({"state":"updated"}))
        );
        assert_eq!(
            reopened.get_payload(2).unwrap(),
            Some(json!({"state":"inserted"}))
        );
        assert_eq!(
            reopened.get_payload(3).unwrap(),
            Some(json!({"state":"after-recovery"}))
        );
        reopened.close().unwrap();
    }
}

#[test]
fn wal失败隔离子进程入口() {
    // 测试驱动以环境变量进入子进程场景，常规枚举不执行进程退出。
    let Some(path) = std::env::var_os("TDB_WAL_FAILURE_CHILD") else {
        return;
    };
    let path = path.to_str().unwrap();
    let mode = if std::env::var("TDB_WAL_MODE").unwrap() == "Rom" {
        StorageMode::Rom
    } else {
        StorageMode::Mmap
    };
    let point = if std::env::var("TDB_WAL_POINT").unwrap() == "flush" {
        IoPoint::WalFlush
    } else {
        IoPoint::WalSync
    };
    let operation = std::env::var("TDB_WAL_OPERATION").unwrap();
    let mut db = Database::<f32>::open_with_config(path, config(mode)).unwrap();
    db.insert_with_id(10, &[1., 0.], json!({"state":"confirmed"}))
        .unwrap();
    let guard = fail_io_at(point);
    let failed = if operation == "tx" {
        let mut tx = TxBuilder::new();
        tx.insert(&[0., 1.], json!({"state":"uncertain"}));
        db.commit_tx(tx).map(|_| ())
    } else {
        db.insert(&[0., 1.], json!({"state":"uncertain"}))
            .map(|_| ())
    };
    assert!(matches!(failed, Err(TriviumError::Io(_))), "{failed:?}");
    drop(guard);
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    assert!(matches!(
        db.insert(&[1., 1.], json!({"state":"rejected"})),
        Err(TriviumError::WalClosed)
    ));
    assert!(matches!(
        db.update_payload(10, json!({"state":"rejected"})),
        Err(TriviumError::WalClosed)
    ));
    let mut tx = TxBuilder::new();
    tx.delete(10);
    assert!(matches!(db.commit_tx(tx), Err(TriviumError::WalClosed)));
    assert!(matches!(db.flush(), Err(TriviumError::WalClosed)));
    assert!(matches!(db.close(), Err(TriviumError::WalClosed)));
    assert_eq!(
        db.get_payload(10).unwrap(),
        Some(json!({"state":"confirmed"}))
    );
    assert_eq!(db.node_count().unwrap(), 1);
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    std::process::exit(0);
}

#[test]
fn 同步失败后无新增确认结果且重启保留先前确认数据() {
    let root = support::TestDir::new("wal_failure");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for point in ["flush", "sync"] {
            for operation in ["insert", "tx"] {
                let path = root.file(&format!("{mode:?}_{point}_{operation}.tdb"));
                let exe = std::env::current_exe().unwrap();
                let mut command = if std::env::var_os("TRIVIUM_TEST_QEMU_AARCH64").is_some() {
                    let mut command = std::process::Command::new("qemu-aarch64");
                    command.args(["-L", "/usr/aarch64-linux-gnu"]).arg(exe);
                    command
                } else {
                    std::process::Command::new(exe)
                };
                let output = command
                    .args([
                        "--exact",
                        "wal_failure_state::wal失败隔离子进程入口",
                        "--nocapture",
                    ])
                    .env("TDB_WAL_FAILURE_CHILD", &path)
                    .env("TDB_WAL_MODE", format!("{mode:?}"))
                    .env("TDB_WAL_POINT", point)
                    .env("TDB_WAL_OPERATION", operation)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{mode:?}/{point}/{operation}: {output:?}"
                );
                let mut recovered = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
                let mut ids = recovered.all_node_ids().unwrap();
                ids.sort_unstable();
                // 未确认的完整日志允许被恢复；任何已确认数据必须保留且无后续拒绝写入的内容。
                assert!(ids == [10] || ids == [10, 11], "{ids:?}");
                assert_eq!(recovered.get(10).unwrap().unwrap().vector, [1., 0.]);
                assert_eq!(
                    recovered.get_payload(10).unwrap(),
                    Some(json!({"state":"confirmed"}))
                );
                let uncertain = recovered.get_payload(11).unwrap();
                assert!(uncertain.is_none() || uncertain == Some(json!({"state":"uncertain"})));
                recovered
                    .insert_with_id(20, &[1., 1.], json!({"state":"after-reopen"}))
                    .unwrap();
                recovered.close().unwrap();
                let mut reopened = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
                assert_eq!(
                    reopened.get_payload(10).unwrap(),
                    Some(json!({"state":"confirmed"}))
                );
                assert_eq!(
                    reopened.get_payload(20).unwrap(),
                    Some(json!({"state":"after-reopen"}))
                );
                reopened.close().unwrap();
            }
        }
    }
}

#[test]
fn 底层wal隔离禁止clear追加和group重启() {
    let root = support::TestDir::new("wal_quarantine");
    for point in [IoPoint::WalFlush, IoPoint::WalSync] {
        for batch in [false, true] {
            let path = root.file(&format!("{point:?}_{batch}.tdb"));
            let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
            let entry = WalEntry::Delete::<f32> { id: 1 };
            let guard = fail_io_at(point);
            let result = if batch {
                wal.append_batch(1, std::slice::from_ref(&entry))
            } else {
                wal.append(&entry)
            };
            assert!(matches!(result, Err(TriviumError::Io(_))));
            drop(guard);
            let before = std::fs::read(format!("{path}.wal")).unwrap();
            assert!(matches!(wal.append(&entry), Err(TriviumError::WalClosed)));
            assert!(matches!(
                wal.append_batch(2, &[entry]),
                Err(TriviumError::WalClosed)
            ));
            assert!(matches!(wal.clear(), Err(TriviumError::WalClosed)));
            assert!(matches!(
                wal.begin_group_commit(),
                Err(TriviumError::WalClosed)
            ));
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
        }
    }
}

#[test]
fn group同步和事务中途序列化失败同样隔离writer() {
    let root = support::TestDir::new("wal_batch_partial");
    for point in [IoPoint::WalFlush, IoPoint::WalSync] {
        let path = root.file(&format!("group_{point:?}.tdb"));
        let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
        wal.begin_group_commit().unwrap();
        wal.append_batch(1, &[WalEntry::Delete::<f32> { id: 1 }])
            .unwrap();
        let guard = fail_io_at(point);
        assert!(matches!(
            wal.finish_group_commit(),
            Err(TriviumError::Io(_))
        ));
        drop(guard);
        assert!(matches!(
            wal.append(&WalEntry::Delete::<f32> { id: 2 }),
            Err(TriviumError::WalClosed)
        ));
        assert!(matches!(wal.clear(), Err(TriviumError::WalClosed)));
    }
    struct InvalidVector;
    impl serde::Serialize for InvalidVector {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(
                "测试序列化失败 (Test serialization failure)",
            ))
        }
    }
    let path = root.file("partial.tdb");
    let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
    wal.append(&WalEntry::Delete::<f32> { id: 10 }).unwrap();
    let result = wal.append_batch(
        2,
        &[WalEntry::Insert {
            id: 11,
            vector: vec![InvalidVector],
            payload: "null".into(),
        }],
    );
    assert!(matches!(result, Err(TriviumError::Serialization(_))));
    assert!(matches!(
        wal.append(&WalEntry::Delete::<f32> { id: 12 }),
        Err(TriviumError::WalClosed)
    ));
    assert!(matches!(wal.clear(), Err(TriviumError::WalClosed)));
    drop(wal);
    let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(matches!(entries[0], WalEntry::Delete { id: 10 }));
}
