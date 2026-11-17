use triviumdb::Database;
use triviumdb::test_hooks::{IoPoint, fail_io_at};

const DIM: usize = 4;

fn path(name: &str) -> String {
    let directory = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&directory).unwrap();
    directory
        .join(format!("io_failpoint_{name}"))
        .to_string_lossy()
        .into_owned()
}

fn cleanup(path: &str) {
    crate::cleanup_files::database(path);
}

fn seed(path: &str) {
    cleanup(path);
    let mut database = Database::<f32>::open(path, DIM).unwrap();
    for id in 1..=4u64 {
        database
            .insert_with_id(
                id,
                &[id as f32, 1.0, 2.0, 3.0],
                serde_json::json!({"id": id}),
            )
            .unwrap();
    }
    database.flush().unwrap();
}

#[test]
fn marker_io各操作定点失败均返回错误且不发布伪成功generation() {
    for point in [
        IoPoint::PublicationBackupCopy,
        IoPoint::PublicationJournalSync,
        IoPoint::PublicationJournalRename,
        IoPoint::MarkerMetadata,
        IoPoint::MarkerCreate,
        IoPoint::MarkerWrite,
        IoPoint::MarkerSync,
        IoPoint::MarkerRename,
    ] {
        let path = path(&format!("{point:?}"));
        seed(&path);
        let old_marker = std::fs::read(format!("{path}.flush_ok")).unwrap();
        let mut database = Database::<f32>::open(&path, DIM).unwrap();
        database
            .insert_with_id(5, &[5.0, 1.0, 2.0, 3.0], serde_json::json!({"id": 5}))
            .unwrap();
        let guard = fail_io_at(point);
        let result = database.flush();
        drop(guard);
        assert!(result.is_err(), "{point:?} 故障必须传播到调用方");
        drop(database);

        let marker = std::fs::read(format!("{path}.flush_ok")).unwrap();
        assert_eq!(marker, old_marker, "{point:?} 故障不得发布新的提交标记");
        assert!(
            Database::<f32>::open_read_only(&path, DIM).is_err(),
            "跨文件撕裂必须 fail-closed"
        );
        let mut recovered = Database::<f32>::open(&path, DIM).unwrap();
        assert_eq!(
            recovered.node_count().unwrap(),
            5,
            "{point:?} 必须恢复全部已写入事务"
        );
        for id in 1..=5 {
            assert_eq!(
                recovered.get(id).unwrap().unwrap().vector,
                vec![id as f32, 1.0, 2.0, 3.0]
            );
            assert_eq!(
                recovered.get_payload(id).unwrap().unwrap(),
                serde_json::json!({"id": id})
            );
        }
        recovered.flush().unwrap();
        drop(recovered);
        cleanup(&path);
    }
}

#[test]
fn 恢复复制失败必须保留日志与备份并允许再次打开() {
    let path = path("restore_retry");
    seed(&path);
    let mut database = Database::<f32>::open(&path, DIM).unwrap();
    database
        .insert_with_id(5, &[5.0, 1.0, 2.0, 3.0], serde_json::json!({"id": 5}))
        .unwrap();
    {
        let _guard = fail_io_at(IoPoint::MarkerRename);
        assert!(database.flush().is_err());
    }
    drop(database);
    {
        let _guard = fail_io_at(IoPoint::PublicationRestoreCopy);
        assert!(matches!(
            Database::<f32>::open(&path, DIM),
            Err(triviumdb::TriviumError::Io(_))
        ));
    }
    let mut recovered = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(recovered.node_count().unwrap(), 5);
    assert_eq!(
        recovered.get(5).unwrap().unwrap().vector,
        vec![5.0, 1.0, 2.0, 3.0]
    );
    recovered.flush().unwrap();
    drop(recovered);
    cleanup(&path);
}

#[test]
fn wal_group_commit_flush与sync故障均传播且不报告成功() {
    for point in [IoPoint::WalFlush, IoPoint::WalSync] {
        let path = path(&format!("group_commit_{point:?}"));
        cleanup(&path);
        let mut database = Database::<f32>::open_with_config(
            &path,
            triviumdb::database::Config {
                dim: DIM,
                storage_mode: triviumdb::database::StorageMode::Mmap,
                sync_mode: triviumdb::storage::wal::SyncMode::Full,
                ..Default::default()
            },
        )
        .unwrap();
        let before = database.wal_stats().sync_count;
        let guard = fail_io_at(point);
        let result = database.group_commit(|database| {
            database.insert_with_id(1, &[1.0, 0.0, 0.0, 0.0], serde_json::json!({"id": 1}))
        });
        drop(guard);
        assert!(result.is_err(), "{point:?} 必须传播到 Group Commit 调用方");
        assert_eq!(database.wal_stats().sync_count, before);
        cleanup(&path);
    }
}

#[test]
fn io_failpoint_guard作用域结束后恢复正常发布() {
    let path = path("guard_scope");
    seed(&path);
    let mut database = Database::<f32>::open(&path, DIM).unwrap();
    database
        .insert_with_id(5, &[5.0, 1.0, 2.0, 3.0], serde_json::json!({"id": 5}))
        .unwrap();
    {
        let _guard = fail_io_at(IoPoint::MarkerCreate);
        assert!(database.flush().is_err());
        database
            .insert_with_id(6, &[6.0, 1.0, 2.0, 3.0], serde_json::json!({"id": 6}))
            .unwrap();
        assert!(
            database.flush().is_err(),
            "第二次失败也必须保留最初的恢复基础"
        );
    }
    database.flush().unwrap();
    drop(database);
    let reader = Database::<f32>::open_read_only(&path, DIM).unwrap();
    assert_eq!(reader.node_count().unwrap(), 6);
    for id in 1..=6 {
        assert_eq!(
            reader.get(id).unwrap().unwrap().vector,
            vec![id as f32, 1.0, 2.0, 3.0]
        );
    }
    cleanup(&path);
}
