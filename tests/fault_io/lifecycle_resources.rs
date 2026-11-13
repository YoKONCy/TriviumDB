use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use triviumdb::test_hooks::{IoPoint, fail_io_at};
use triviumdb::{Database, NoopHook, SearchHook, ShutdownPhase, TriviumError};

struct OwnedHook(Arc<AtomicUsize>);
impl SearchHook for OwnedHook {}
impl Drop for OwnedHook {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn open(name: &str) -> (Database<f32>, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("tdb-close-io-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut db = Database::open(&root.join("sample.tdb").to_string_lossy(), 3).unwrap();
    db.insert_with_id(1, &[1.0; 3], serde_json::json!({}))
        .unwrap();
    (db, root)
}

#[test]
fn 最后同步失败保留数据文件锁与Hook且可重试关闭() {
    for asynchronous in [false, true] {
        let (mut db, root) = open(if asynchronous { "async" } else { "sync" });
        db.flush().unwrap();
        db.enable_auto_compaction(std::time::Duration::from_secs(3600))
            .unwrap();
        let dropped = Arc::new(AtomicUsize::new(0));
        db.set_hook(OwnedHook(dropped.clone())).unwrap();
        let failure = fail_io_at(IoPoint::WalSync);
        let result = if asynchronous {
            db.close_timeout(std::time::Duration::from_secs(5))
                .map(|_| ())
        } else {
            db.close()
        };
        assert!(matches!(result, Err(TriviumError::Io(_))));
        assert_ne!(db.shutdown_status().phase, ShutdownPhase::Closed);
        if asynchronous {
            assert!(db.instance_stats().compaction.is_none());
        } else {
            assert_eq!(
                db.instance_stats().compaction.unwrap().phase,
                triviumdb::MaintenancePhase::Scheduled
            );
        }
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        assert!(matches!(
            Database::<f32>::open(&root.join("sample.tdb").to_string_lossy(), 3),
            Err(TriviumError::DatabaseLocked(_))
        ));
        drop(failure);
        db.close().unwrap();
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        let mut recovered =
            Database::<f32>::open(&root.join("sample.tdb").to_string_lossy(), 3).unwrap();
        assert_eq!(recovered.all_node_ids().unwrap(), vec![1]);
        assert_eq!(recovered.get(1).unwrap().unwrap().vector, vec![1.0; 3]);
        assert_eq!(
            recovered.get_payload(1).unwrap().unwrap(),
            serde_json::json!({})
        );
        recovered.close().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn 分组正常返回路径同步失败传播原错误并拒绝后续写入() {
    let (mut db, root) = open("group-return");
    db.set_sync_mode(triviumdb::storage::wal::SyncMode::Full)
        .unwrap();
    let failure = fail_io_at(IoPoint::WalSync);
    let result = db.group_commit(|db| db.insert_with_id(2, &[2.0; 3], serde_json::json!({})));
    let error = result.unwrap_err();
    assert!(matches!(error.underlying(), TriviumError::Io(_)));
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        serde_json::json!({"phase":"group_sync","outcome":"unknown"})
    );
    drop(failure);
    assert!(matches!(
        db.insert(&[3.0; 3], serde_json::json!({}))
            .unwrap_err()
            .underlying(),
        TriviumError::WalClosed
    ));
    drop(db);
    let mut recovered =
        Database::<f32>::open(&root.join("sample.tdb").to_string_lossy(), 3).unwrap();
    assert!(recovered.contains(1).unwrap());
    recovered.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn 分组异常展开中同步失败关闭写入能力() {
    let (mut db, root) = open("group-unwind");
    db.set_sync_mode(triviumdb::storage::wal::SyncMode::Full)
        .unwrap();
    let failure = fail_io_at(IoPoint::WalSync);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = db.group_commit(|db| {
            db.insert_with_id(2, &[2.0; 3], serde_json::json!({}))
                .unwrap();
            panic!("模拟分组调用方异常");
        });
    }));
    assert!(result.is_err());
    drop(failure);
    assert!(matches!(
        db.insert(&[3.0; 3], serde_json::json!({}))
            .unwrap_err()
            .underlying(),
        TriviumError::WalClosed
    ));
    assert!(matches!(
        db.group_commit(|_| ()).unwrap_err().underlying(),
        TriviumError::WalClosed
    ));
    db.set_hook(NoopHook).unwrap();
    drop(db);
    let mut recovered =
        Database::<f32>::open(&root.join("sample.tdb").to_string_lossy(), 3).unwrap();
    assert!(recovered.contains(1).unwrap());
    recovered.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
