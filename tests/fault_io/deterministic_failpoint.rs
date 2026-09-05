use triviumdb::Database;
use triviumdb::storage::wal::{Wal, WalEntry};
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
    for suffix in [
        "",
        ".wal",
        ".vec",
        ".lock",
        ".flush_ok",
        ".tmp",
        ".vec.tmp",
        ".flush_ok.tmp",
    ] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
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

fn v2_wal(path: &str) -> Vec<u8> {
    #[allow(dead_code)]
    #[derive(serde::Serialize)]
    enum V2<T> {
        TxBegin {
            tx_id: u64,
        },
        TxCommit {
            tx_id: u64,
        },
        Insert {
            id: u64,
            vector: Vec<T>,
            payload: String,
        },
    }
    let data = bincode::serialize(&V2::Insert::<f32> {
        id: 9,
        vector: vec![1.0; DIM],
        payload: serde_json::json!({"legacy": true}).to_string(),
    })
    .unwrap();
    let mut bytes = vec![b'T', b'V', b'W', b'L', 2, 0];
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(&crc32fast::hash(&data).to_le_bytes());
    std::fs::write(format!("{path}.wal"), &bytes).unwrap();
    bytes
}

#[test]
fn wal_v2迁移各发布阶段故障均保持原文件且可重试() {
    for point in [
        IoPoint::WalMigrationCreate,
        IoPoint::WalMigrationWrite,
        IoPoint::WalMigrationSync,
        IoPoint::WalMigrationRename,
    ] {
        let path = path(&format!("migration_{point:?}"));
        cleanup(&path);
        let original = v2_wal(&path);
        let guard = fail_io_at(point);
        assert!(Wal::upgrade_legacy_wal::<f32>(&path).is_err());
        drop(guard);
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), original);
        assert!(!std::path::Path::new(&format!("{path}.wal.upgrade.tmp")).exists());
        assert!(Wal::upgrade_legacy_wal::<f32>(&path).unwrap());
        let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
        assert!(matches!(&entries[0], WalEntry::Insert { id: 9, .. }));
        cleanup(&path);
    }
}

#[test]
fn marker_io各操作定点失败均返回错误且不发布伪成功generation() {
    for point in [
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
        cleanup(&path);
    }
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
    }
    database.flush().unwrap();
    drop(database);
    let reader = Database::<f32>::open_read_only(&path, DIM).unwrap();
    assert_eq!(reader.node_count(), 5);
    cleanup(&path);
}
