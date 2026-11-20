//! Mmap/Rom、文本/属性/图与冷载荷的快照往返检查。

use triviumdb::Database;
use triviumdb::database::{Config, StorageMode};
use triviumdb::storage::snapshot::{restore_snapshot, verify_snapshot};

#[test]
fn manifest标识仅保存元数据且目录接口拒绝路径组件越界() {
    use triviumdb::TriviumError;
    use triviumdb::storage::{
        generation::GenerationStore,
        snapshot::{manifest_path, write_manifest},
    };
    let root = std::env::temp_dir().join(format!("tdb-manifest-identifier-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("sample.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(1, &[1., 0.], serde_json::json!({"value":1}))
        .unwrap();
    db.close().unwrap();
    let id = "../escaped/nested";
    let manifest = write_manifest(path.to_str().unwrap(), id, "f32", 2, 1).unwrap();
    assert_eq!(manifest.generation_id, id);
    assert_eq!(
        manifest_path(path.to_str().unwrap()),
        root.join("sample.tdb.manifest.json")
    );
    let store = GenerationStore::new(root.join("store"));
    assert!(matches!(
        store.generation_dir(id),
        Err(TriviumError::InvalidInput(_))
    ));
    assert!(matches!(
        store.database_path("valid", "../outside.tdb"),
        Err(TriviumError::InvalidInput(_))
    ));
    assert!(!root.join("store").exists());
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest_path(path.to_str().unwrap())).unwrap())
            .unwrap();
    assert_eq!(stored["generation_id"], id);
    assert_eq!(stored["node_count"], 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn 两种存储模式的完整索引快照恢复后事实一致() {
    for (index, mode) in [StorageMode::Mmap, StorageMode::Rom]
        .into_iter()
        .enumerate()
    {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tdb_snapshot_modes_{}_{nonce}_{index}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut db = Database::<f32>::open_with_config(
            &root.join("live.tdb").to_string_lossy(),
            Config {
                dim: 3,
                storage_mode: mode,
                load_text_index: true,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        for id in 1..=32u64 {
            db.insert_with_id(
                id,
                &[id as f32, 2.0, 3.0],
                serde_json::json!({"id": id, "name": format!("entity-{id}")}),
            )
            .unwrap();
            db.index_text(id, "快照全文检索测试").unwrap();
            db.index_keyword(id, "快照").unwrap();
            if id > 1 {
                db.link(id - 1, id, "next", 1.0).unwrap();
            }
        }
        db.build_text_index().unwrap();
        db.create_unique_index("name").unwrap();
        db.build_quiver_index(None).unwrap();
        let snapshot = root.join("snapshot");
        db.create_snapshot(&snapshot.to_string_lossy(), "all-indexes")
            .unwrap();
        db.delete(2).unwrap();
        let restored = root.join("restored");
        restore_snapshot(&snapshot.to_string_lossy(), &restored.to_string_lossy()).unwrap();
        let mut reader = Database::<f32>::open_with_config(
            &restored.join("database.tdb").to_string_lossy(),
            Config {
                dim: 3,
                access_mode: triviumdb::database::AccessMode::Immutable,
                load_text_index: true,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(reader.node_count().unwrap(), 32);
        for id in 1..=32u64 {
            let node = reader.get(id).unwrap().unwrap();
            assert_eq!(node.vector, vec![id as f32, 2.0, 3.0]);
            assert_eq!(
                node.payload,
                serde_json::json!({"id": id, "name": format!("entity-{id}")})
            );
            if id < 32 {
                assert_eq!(reader.get_edges(id).unwrap()[0].target_id, id + 1);
            }
        }
        assert_eq!(
            reader
                .indexed_lookup(&[("name".into(), serde_json::json!("entity-2"))], 10)
                .unwrap(),
            vec![2]
        );
        assert_eq!(
            reader
                .tql(r#"TEXT AC "快照" TOP 32 AS hits WITH hits RETURN hits"#)
                .unwrap()
                .len(),
            32
        );
        reader.close().unwrap();
        verify_snapshot(&restored.to_string_lossy()).unwrap();
        db.close().unwrap();
    }
}

#[test]
fn 清单dtype与主文件不一致时快照不能校验或发布恢复目录() {
    let root = super::support::TestDir::new("snapshot_dtype");
    let path = root.file("source.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    db.insert_with_id(1, &[1., 0.], serde_json::json!({"kind":"sample"}))
        .unwrap();
    let snapshot = root.file("snapshot");
    let restored = root.file("restored");
    db.create_snapshot(&snapshot, "snapshot-dtype").unwrap();
    db.close().unwrap();
    assert_eq!(verify_snapshot(&snapshot).unwrap().dtype, "f32");

    let manifest_path = std::path::Path::new(&snapshot).join("database.tdb.manifest.json");
    let original = std::fs::read(&manifest_path).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(&original).unwrap();
    manifest["dtype"] = "u64".into();
    let incorrect = serde_json::to_vec(&manifest).unwrap();
    std::fs::write(&manifest_path, &incorrect).unwrap();
    assert_eq!(
        verify_snapshot(&snapshot).unwrap_err().code(),
        "TDB_IMMUTABLE_ARTIFACT"
    );
    let error = restore_snapshot(&snapshot, &restored).unwrap_err();
    assert_eq!(error.code(), "TDB_IMMUTABLE_ARTIFACT");
    assert!(!std::path::Path::new(&restored).exists());
    assert_eq!(std::fs::read(&manifest_path).unwrap(), incorrect);

    std::fs::write(&manifest_path, original).unwrap();
    assert_eq!(restore_snapshot(&snapshot, &restored).unwrap().dtype, "f32");
    let reader = Database::<f32>::open_immutable(&format!("{restored}/database.tdb"), 2).unwrap();
    let node = reader.get(1).unwrap().unwrap();
    assert_eq!(node.vector, [1., 0.]);
    assert_eq!(node.payload, serde_json::json!({"kind":"sample"}));
}

#[test]
fn 快照与不可变打开共用存活节点数并跳过墓碑() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let dir = super::support::TestDir::new("live_manifest");
        let path = dir.file("source.tdb");
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 1,
                storage_mode: mode,
                ..Default::default()
            },
        )
        .unwrap();
        for id in [1, 2] {
            db.insert_with_id(id, &[1.], serde_json::json!(id)).unwrap();
        }
        db.delete(1).unwrap();
        db.create_snapshot(&dir.file("copy"), "gen").unwrap();
        assert_eq!(verify_snapshot(&dir.file("copy")).unwrap().node_count, 1);
        db.publish_generation_manifest("gen").unwrap();
        db.close().unwrap();
        let mut reader = Database::<f32>::open_immutable(&path, 1).unwrap();
        assert_eq!(reader.all_node_ids().unwrap(), [2]);
        reader.close().unwrap();
        triviumdb::storage::snapshot::write_manifest(&path, "gen", "f32", 1, 2).unwrap();
        assert_eq!(
            Database::<f32>::open_immutable(&path, 1)
                .err()
                .unwrap()
                .code(),
            "TDB_IMMUTABLE_ARTIFACT"
        );
    }
}
