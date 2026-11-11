//! 迁移暂存失败、提交前恢复与提交后保留完整目标的行为边界。
use super::support::TestDir;
use serde_json::json;
use triviumdb::database::{Config, StorageMode};
use triviumdb::test_hooks::{IoPoint, fail_io_at};

#[test]
fn 发布持有目标锁且文件碰撞只回收本次移动的文件() {
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
    let source_root = TestDir::new("migration_collision_source");
    let target_root = TestDir::new("migration_collision_target");
    let mut source = Database::<f32>::open(&source_root.file("source.tdb"), 1).unwrap();
    source
        .insert_with_id(1, &[1.], json!({"source":true}))
        .unwrap();
    let target = target_root.file("target.tdb");
    let gate = pause_at(ConcurrencyPoint::MigrationFilePublished);
    let worker_target = target.clone();
    let worker = std::thread::spawn(move || {
        let result = source.migrate_to(&worker_target, 2);
        source.close().unwrap();
        result.err().expect("目标文件碰撞必须报错")
    });
    gate.wait_until_arrived();
    // 发布屏障后独立竞争真实锁文件，验证租约本身；路径校验错误不能代替锁证据。
    let contender = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!("{target}.lock"))
        .unwrap();
    let contention = fs2::FileExt::try_lock_exclusive(&contender).unwrap_err();
    assert_eq!(
        contention.raw_os_error(),
        fs2::lock_contended_error().raw_os_error()
    );
    let opening = Database::<f32>::open(&target, 2);
    assert!(matches!(opening, Err(TriviumError::DatabaseLocked(_))));
    std::fs::write(format!("{target}.vec"), b"outside publisher evidence").unwrap();
    gate.release();
    assert!(
        matches!(worker.join().unwrap().underlying(),TriviumError::Io(error) if error.kind()==std::io::ErrorKind::AlreadyExists)
    );
    assert_eq!(
        std::fs::read(format!("{target}.vec")).unwrap(),
        b"outside publisher evidence"
    );
    assert!(!std::path::Path::new(&target).exists());
    let mut names = std::fs::read_dir(&target_root.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["target.tdb.lock", "target.tdb.vec"]);
}
use triviumdb::{Database, TriviumError};

#[test]
fn 迁移在公开目标前预留最终实例配额() {
    use triviumdb::resources::{ResourceLimits, ResourceRuntime};
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
    const MB: usize = 1024 * 1024;
    let root = TestDir::new("migration-publish-quota");
    let runtime = ResourceRuntime::new(ResourceLimits {
        memory_limit: 8 * MB,
        maintenance_memory_limit: MB,
        ..Default::default()
    })
    .unwrap();
    let mut source = Database::<f32>::open_with_runtime(
        &root.file("source.tdb"),
        Config {
            dim: 1,
            memory_limit: MB,
            auto_build_quiver: false,
            ..Default::default()
        },
        runtime.clone(),
    )
    .unwrap();
    source
        .insert_with_id(7, &[1.], json!({"state":"source"}))
        .unwrap();
    let target = root.file("target.tdb");
    let worker_target = target.clone();
    let pause = pause_at(ConcurrencyPoint::MigrationBeforeCommit);
    let worker = std::thread::spawn(move || {
        let result = source.migrate_to(&worker_target, 2);
        source.close().unwrap();
        result.unwrap()
    });
    pause.wait_until_arrived();
    assert_eq!(runtime.stats().reserved_database_bytes, 2 * MB);
    let blocker = root.file("blocker.tdb");
    let error = Database::<f32>::open_with_runtime(
        &blocker,
        Config {
            dim: 1,
            memory_limit: 6 * MB,
            ..Default::default()
        },
        runtime.clone(),
    )
    .err()
    .unwrap();
    assert_eq!(error.code(), "QueueFull");
    assert_eq!(
        error.details(),
        json!({"resource":"memory_bytes","requested":6 * MB,"used":2 * MB,"limit":7 * MB})
    );
    assert!(!std::path::Path::new(&blocker).exists());
    pause.release();
    let (mut migrated, ids) = worker.join().unwrap();
    assert_eq!(ids, [7]);
    for _ in 0..2 {
        assert_eq!(migrated.all_node_ids().unwrap(), [7]);
        let node = migrated.get(7).unwrap().unwrap();
        assert_eq!(node.vector, [0., 0.]);
        assert_eq!(node.payload, json!({"state":"source"}));
        assert!(node.edges.is_empty());
        migrated.close().unwrap();
        migrated = Database::<f32>::open(&target, 2).unwrap();
    }
    migrated.close().unwrap();
    assert_eq!(runtime.stats().attached_databases, 0);
}

#[test]
fn 迁移故障按提交点保留完整目标并支持重试() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for point in [
            IoPoint::GraphSidecarWrite,
            IoPoint::MigrationPublishFile,
            IoPoint::MigrationAfterFirstFile,
            IoPoint::MigrationPublishMarker,
            IoPoint::MigrationAfterCommit,
            IoPoint::MigrationCleanup,
        ] {
            let source_root = TestDir::new("migration_fault_source");
            let target_root = TestDir::new("migration_fault_target");
            let path = source_root.file("source.tdb");
            let mut source = Database::<f32>::open_with_config(
                &path,
                Config {
                    dim: 1,
                    storage_mode: mode,
                    auto_build_quiver: false,
                    ..Default::default()
                },
            )
            .unwrap();
            for id in [1, 2] {
                source.insert_with_id(id, &[1.], json!({"id":id})).unwrap();
            }
            source
                .upsert_edge(1, 2, "rel", 2., json!({"value":"完整"}))
                .unwrap();
            let target = target_root.file("target.tdb");
            {
                let _failure = fail_io_at(point);
                let error = source.migrate_to(&target, 2).err().unwrap();
                assert!(matches!(error.underlying(), TriviumError::Io(_)));
                let expected = match point {
                    IoPoint::GraphSidecarWrite => {
                        json!({"phase":"validation","outcome":"not_committed"})
                    }
                    IoPoint::MigrationAfterCommit => json!({"phase":"publish","outcome":"unknown"}),
                    IoPoint::MigrationCleanup => json!({"phase":"publish","outcome":"committed"}),
                    _ => json!({"phase":"publish","outcome":"not_committed"}),
                };
                assert_eq!(
                    serde_json::to_value(error.info()).unwrap()["write"],
                    expected,
                    "{mode:?}/{point:?}"
                );
            }
            assert_eq!(source.get_payload(1).unwrap(), Some(json!({"id":1})));
            assert_eq!(
                source.get_edge(1, 2, "rel").unwrap().unwrap().metadata,
                json!({"value":"完整"})
            );
            let mut target_db = if matches!(
                point,
                IoPoint::MigrationAfterCommit | IoPoint::MigrationCleanup
            ) {
                Database::<f32>::open(&target, 2).unwrap()
            } else {
                assert!(
                    std::fs::read_dir(&target_root.0)
                        .unwrap()
                        .all(|entry| entry.unwrap().file_name() == "target.tdb.lock")
                );
                let (db, ids) = source.migrate_to(&target, 2).unwrap();
                assert_eq!(ids, [1, 2]);
                db
            };
            assert_eq!(target_db.all_node_ids().unwrap(), [1, 2]);
            for id in [1, 2] {
                let node = target_db.get(id).unwrap().unwrap();
                assert_eq!(node.vector, [0., 0.]);
                assert_eq!(node.payload, json!({"id":id}));
            }
            assert_eq!(
                target_db.get_edge(1, 2, "rel").unwrap().unwrap().metadata,
                json!({"value":"完整"})
            );
            target_db.close().unwrap();
            drop(target_db);
            let mut again = Database::<f32>::open(&target, 2).unwrap();
            assert_eq!(again.all_node_ids().unwrap(), [1, 2]);
            for id in [1, 2] {
                let node = again.get(id).unwrap().unwrap();
                assert_eq!(node.vector, [0., 0.]);
                assert_eq!(node.payload, json!({"id":id}));
                assert_eq!(node.edges.len(), usize::from(id == 1));
            }
            let edge = again.get_edge(1, 2, "rel").unwrap().unwrap();
            assert_eq!(edge.target_id, 2);
            assert_eq!(edge.label, "rel");
            assert_eq!(edge.weight, 2.);
            assert_eq!(edge.metadata, json!({"value":"完整"}));
            again.close().unwrap();
            source.close().unwrap();
        }
    }
}
