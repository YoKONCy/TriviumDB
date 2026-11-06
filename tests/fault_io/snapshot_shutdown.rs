//! 快照发布失败与关闭超时的独立故障检查。

use std::time::Duration;
use triviumdb::storage::snapshot::{restore_snapshot, verify_snapshot};
use triviumdb::test_hooks::{ConcurrencyPoint, IoPoint, fail_io_at, pause_at_path};
use triviumdb::{Database, ShutdownPhase, TriviumError};

fn root() -> std::path::PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("tdb_snapshot_fault_{}_{n}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    root
}

#[test]
fn 快照各阶段失败不暴露半份目标且可重试() {
    let root = root();
    let mut db = Database::<f32>::open(&root.join("live.tdb").to_string_lossy(), 3).unwrap();
    db.insert_with_id(
        1,
        &[1.0, 2.0, 3.0],
        serde_json::json!({"state": "original"}),
    )
    .unwrap();
    for (index, point) in [
        IoPoint::SnapshotCopy,
        IoPoint::SnapshotSync,
        IoPoint::SnapshotPublish,
    ]
    .into_iter()
    .enumerate()
    {
        let target = root.join(format!("snapshot-{index}"));
        let injection = fail_io_at(point);
        let error = db
            .create_snapshot(&target.to_string_lossy(), "snapshot")
            .unwrap_err();
        assert_eq!(error.code(), "TDB_IO");
        assert_eq!(
            serde_json::to_value(error.info()).unwrap()["write"],
            serde_json::json!({"phase":"validation","outcome":"not_committed"})
        );
        drop(injection);
        assert!(!target.exists());
        assert!(!std::fs::read_dir(&root).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".snapshot.tmp")
        }));
        db.create_snapshot(&target.to_string_lossy(), "snapshot")
            .unwrap();
        verify_snapshot(&target.to_string_lossy()).unwrap();
        let restored = root.join(format!("restored-{index}"));
        let injection = fail_io_at(point);
        let error =
            restore_snapshot(&target.to_string_lossy(), &restored.to_string_lossy()).unwrap_err();
        assert_eq!(error.code(), "TDB_IO");
        assert_eq!(
            serde_json::to_value(error.info()).unwrap()["write"],
            serde_json::json!({"phase":"validation","outcome":"not_committed"})
        );
        drop(injection);
        assert!(!restored.exists());
        restore_snapshot(&target.to_string_lossy(), &restored.to_string_lossy()).unwrap();
        // 复制与恢复都验证真实记录，并再次持久化，防止仅校验 manifest 掩盖数据缺失。
        for directory in [&target, &restored] {
            let path = directory.join("database.tdb");
            let mut recovered = Database::<f32>::open(path.to_str().unwrap(), 3).unwrap();
            for _ in 0..2 {
                assert_eq!(recovered.all_node_ids().unwrap(), [1]);
                let node = recovered.get(1).unwrap().unwrap();
                assert_eq!(node.vector, [1., 2., 3.]);
                assert_eq!(node.payload, serde_json::json!({"state":"original"}));
                assert!(node.edges.is_empty());
                recovered.close().unwrap();
                recovered = Database::<f32>::open(path.to_str().unwrap(), 3).unwrap();
            }
            recovered.close().unwrap();
        }
    }
    db.close().unwrap();
}

#[test]
fn 快照重命名后目录同步失败保留完整目标并报告不确定状态() {
    let root = root();
    let mut db = Database::<f32>::open(&root.join("live.tdb").to_string_lossy(), 3).unwrap();
    db.insert_with_id(
        1,
        &[1.0, 0.0, 0.0],
        serde_json::json!({"value": "committed"}),
    )
    .unwrap();
    let target = root.join("snapshot");
    let failure = fail_io_at(IoPoint::SnapshotDirectorySync);
    let error = db
        .create_snapshot(&target.to_string_lossy(), "generation")
        .unwrap_err();
    assert!(matches!(
        error.underlying(),
        TriviumError::SnapshotPublicationUncertain { .. }
    ));
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        serde_json::json!({"phase":"publish","outcome":"unknown"})
    );
    drop(failure);
    assert_eq!(
        verify_snapshot(&target.to_string_lossy())
            .unwrap()
            .node_count,
        1
    );
    let restored = root.join("restored-uncertain");
    let failure = fail_io_at(IoPoint::SnapshotDirectorySync);
    let error = restore_snapshot(target.to_str().unwrap(), restored.to_str().unwrap()).unwrap_err();
    drop(failure);
    assert!(matches!(
        error.underlying(),
        TriviumError::SnapshotPublicationUncertain { .. }
    ));
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        serde_json::json!({"phase":"publish","outcome":"unknown"})
    );
    for directory in [&target, &restored] {
        let path = directory.join("database.tdb");
        for _ in 0..2 {
            let mut recovered = Database::<f32>::open(path.to_str().unwrap(), 3).unwrap();
            assert_eq!(recovered.all_node_ids().unwrap(), [1]);
            let node = recovered.get(1).unwrap().unwrap();
            assert_eq!(node.vector, [1., 0., 0.]);
            assert_eq!(node.payload, serde_json::json!({"value":"committed"}));
            assert!(node.edges.is_empty());
            recovered.close().unwrap();
        }
    }
    assert!(matches!(
        db.create_snapshot(&target.to_string_lossy(), "replacement")
            .unwrap_err()
            .underlying(),
        TriviumError::SnapshotDestinationExists { .. }
    ));
    db.close().unwrap();
}

#[test]
fn 损坏或多余制品拒绝恢复且已有目标保持原样() {
    let root = root();
    let mut db = Database::<f32>::open(&root.join("live.tdb").to_string_lossy(), 3).unwrap();
    db.insert_with_id(1, &[1.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    let source = root.join("source");
    db.create_snapshot(&source.to_string_lossy(), "original")
        .unwrap();
    let existing = root.join("existing");
    std::fs::create_dir(&existing).unwrap();
    std::fs::write(existing.join("keep"), b"unchanged").unwrap();
    assert!(matches!(
        restore_snapshot(&source.to_string_lossy(), &existing.to_string_lossy())
            .unwrap_err()
            .underlying(),
        TriviumError::SnapshotDestinationExists { .. }
    ));
    assert_eq!(std::fs::read(existing.join("keep")).unwrap(), b"unchanged");
    std::fs::write(source.join("unexpected.wal"), b"dirty").unwrap();
    assert!(verify_snapshot(&source.to_string_lossy()).is_err());
    std::fs::remove_file(source.join("unexpected.wal")).unwrap();
    let file = source.join("database.tdb");
    let mut bytes = std::fs::read(&file).unwrap();
    bytes[0] ^= 1;
    std::fs::write(file, bytes).unwrap();
    let rejected = root.join("rejected");
    assert!(restore_snapshot(&source.to_string_lossy(), &rejected.to_string_lossy()).is_err());
    assert!(!rejected.exists());
    db.close().unwrap();
}

#[test]
fn 关闭等待超时不释放正在刷盘的锁与配额() {
    let root = root();
    let path = root.join("live.tdb").to_string_lossy().into_owned();
    let mut db = Database::<f32>::open(&path, 3).unwrap();
    db.insert_with_id(1, &[1.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    let scope = root.canonicalize().unwrap().join("live.tdb");
    let pause = pause_at_path(
        ConcurrencyPoint::BeforeFlushMarkerRename,
        scope.to_str().unwrap(),
    );
    assert!(!db.close_timeout(Duration::ZERO).unwrap());
    pause.wait_until_arrived();
    let status = db.shutdown_status();
    assert_eq!(status.phase, ShutdownPhase::Flushing);
    assert_eq!(
        status.maintenance.unwrap().phase,
        triviumdb::MaintenancePhase::Running
    );
    assert!(db.instance_stats().memory.is_none());
    assert!(matches!(
        Database::<f32>::open(&path, 3),
        Err(TriviumError::DatabaseLocked(_))
    ));
    pause.release();
    assert!(db.close_timeout(Duration::from_secs(5)).unwrap());
    assert_eq!(db.shutdown_status().phase, ShutdownPhase::Closed);
    let mut reopened = Database::<f32>::open(&path, 3).unwrap();
    assert!(reopened.contains(1).unwrap());
    reopened.close().unwrap();
}

#[test]
fn 异步关闭继承故障注入并保留可重试状态() {
    let root = root();
    let path = root.join("live.tdb").to_string_lossy().into_owned();
    let mut db = Database::<f32>::open(&path, 3).unwrap();
    db.insert_with_id(1, &[1.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    let failure = fail_io_at(IoPoint::MarkerRename);
    assert!(db.close_timeout(Duration::from_secs(5)).is_err());
    drop(failure);
    assert_eq!(db.shutdown_status().phase, ShutdownPhase::Failed);
    assert!(db.shutdown_status().last_error.is_some());
    assert_eq!(db.shutdown_status().last_failure.unwrap().kind, "io");
    assert!(matches!(
        Database::<f32>::open(&path, 3),
        Err(TriviumError::DatabaseLocked(_))
    ));
    assert!(db.close_timeout(Duration::from_secs(5)).unwrap());
    assert_eq!(db.shutdown_status().phase, ShutdownPhase::Closed);
    assert!(db.shutdown_status().last_failure.is_none());
}
