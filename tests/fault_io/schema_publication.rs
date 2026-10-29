//! 唯一约束发布失败后的隔离状态与提交点恢复。
use super::support;

use serde_json::json;
use triviumdb::database::{Config, StorageMode};
use triviumdb::test_hooks::{IoPoint, fail_io_at};
use triviumdb::{Database, Result, TriviumError};

fn change(db: &mut Database<f32>, composite: bool, create: bool) -> Result<()> {
    let fields = ["tenant".into(), "email".into()];
    match (composite, create) {
        (false, true) => db.create_unique_index("email"),
        (false, false) => db.drop_index("email"),
        (true, true) => db.create_unique_composite_index(&fields),
        (true, false) => db.drop_composite_index(&fields),
    }
}

#[test]
fn 约束发布失败隔离实例并按真实提交点恢复() {
    let root = support::TestDir::new("schema_publication");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for composite in [false, true] {
            for create in [false, true] {
                for point in [
                    IoPoint::PublicationBackupCopy,
                    IoPoint::PropertySidecarWrite,
                    IoPoint::GraphSidecarWrite,
                    IoPoint::CheckpointWalClear,
                ] {
                    let path = root.file(&format!("{mode:?}-{composite}-{create}-{point:?}.tdb"));
                    let cfg = Config {
                        dim: 2,
                        storage_mode: mode,
                        sync_mode: triviumdb::storage::wal::SyncMode::Full,
                        auto_build_quiver: false,
                        ..Default::default()
                    };
                    let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
                    db.insert_with_id(1, &[1., 0.], json!({"tenant":"a","email":"same"}))
                        .unwrap();
                    if !create {
                        change(&mut db, composite, true).unwrap();
                    }
                    db.close().unwrap();
                    let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
                    // 此记录尚未检查点化，所有发布失败阶段均必须保留这次已确认写入。
                    db.insert_with_id(2, &[0., 1.], json!({"tenant":"a","email":"other"}))
                        .unwrap();
                    let error = {
                        let _failure = fail_io_at(point);
                        change(&mut db, composite, create).unwrap_err()
                    };
                    assert!(matches!(error, TriviumError::Io(_)), "{mode:?}/{point:?}");
                    let wal_before = std::fs::read(format!("{path}.wal")).unwrap();
                    assert!(matches!(
                        db.insert_with_id(3, &[1., 1.], json!({"tenant":"a","email":"same"})),
                        Err(TriviumError::WalClosed)
                    ));
                    assert!(matches!(db.get(1), Err(TriviumError::WalClosed)));
                    assert!(matches!(
                        db.search_exact(&[1., 0.], 1),
                        Err(TriviumError::WalClosed)
                    ));
                    assert!(matches!(
                        db.list_unique_indexes(),
                        Err(TriviumError::WalClosed)
                    ));
                    assert!(matches!(db.flush(), Err(TriviumError::WalClosed)));
                    assert!(matches!(db.close(), Err(TriviumError::WalClosed)));
                    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), wal_before);
                    drop(db);

                    let mut recovered = Database::<f32>::open_with_config(&path, cfg).unwrap();
                    assert_eq!(recovered.all_node_ids().unwrap(), [1, 2]);
                    assert_eq!(
                        recovered.get(2).unwrap().unwrap().payload,
                        json!({"tenant":"a","email":"other"})
                    );
                    // WAL 清理点位于发布提交之后；其余故障点均位于发布提交之前。
                    let has_constraint = if point == IoPoint::CheckpointWalClear {
                        create
                    } else {
                        !create
                    };
                    let expected = if !has_constraint {
                        Vec::new()
                    } else if composite {
                        vec![vec!["tenant".to_owned(), "email".to_owned()]]
                    } else {
                        vec![vec!["email".to_owned()]]
                    };
                    assert_eq!(recovered.list_unique_indexes().unwrap(), expected);
                    let duplicate = recovered.insert_with_id(
                        3,
                        &[1., 1.],
                        json!({"tenant":"a","email":"same"}),
                    );
                    if has_constraint {
                        assert!(matches!(
                            duplicate,
                            Err(TriviumError::UniqueConstraintViolation { .. })
                        ));
                    } else {
                        duplicate.unwrap();
                    }
                    recovered.close().unwrap();
                    let mut reopened = Database::<f32>::open_with_config(&path, cfg).unwrap();
                    assert_eq!(reopened.list_unique_indexes().unwrap(), expected);
                    assert_eq!(
                        reopened.node_count().unwrap(),
                        if has_constraint { 2 } else { 3 }
                    );
                    reopened.close().unwrap();
                }
            }
        }
    }
}

#[test]
fn 约束构建预检拒绝不隔离可用实例() {
    let root = support::TestDir::new("schema_preflight");
    let path = root.file("db.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    for id in [1, 2] {
        db.insert_with_id(id, &[1., 0.], json!({"email":"same"}))
            .unwrap();
    }
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    assert!(matches!(
        db.create_unique_index("email"),
        Err(TriviumError::UniqueConstraintViolation { .. })
    ));
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert!(db.list_unique_indexes().unwrap().is_empty());
    db.insert_with_id(3, &[0., 1.], json!({"email":"different"}))
        .unwrap();
    db.close().unwrap();
}
