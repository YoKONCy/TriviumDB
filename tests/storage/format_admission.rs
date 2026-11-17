//! 当前主文件布局及不受支持格式的零写拒绝。
use super::support::TestDir;
use serde_json::json;
use triviumdb::database::{AccessMode, Config, StorageMode};
use triviumdb::{Database, TriviumError};

#[test]
fn 当前格式使用自描述空BQ缓存块并保留完整事实() {
    let root = TestDir::new("current_format");
    let path = root.file("data.tdb");
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: 2,
            storage_mode: StorageMode::Rom,
            ..Default::default()
        },
    )
    .unwrap();
    db.insert_with_id(7, &[1., -1.], json!({"name":"记录"}))
        .unwrap();
    db.close().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 15);
    let offset = u64::from_le_bytes(bytes[50..58].try_into().unwrap()) as usize;
    assert_eq!(&bytes[offset..offset + 4], b"TBQF");
    assert_eq!(&bytes[offset + 4..offset + 8], &[1, 0, 48, 0]);
    assert_eq!(&bytes[offset + 8..offset + 16], &[0; 8]);
    let mut reopened = Database::<f32>::open_read_only(&path, 2).unwrap();
    let node = reopened.get(7).unwrap().unwrap();
    assert_eq!(node.vector, [1., -1.]);
    assert_eq!(node.payload, json!({"name":"记录"}));
    reopened.close().unwrap();
}

#[test]
fn 未知格式明确拒绝且文件组保持不变() {
    let root = TestDir::new("unknown_format");
    let path = root.file("data.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    db.insert_with_id(7, &[1., 0.], json!({"name":"记录"}))
        .unwrap();
    db.close().unwrap();
    let mut main = std::fs::read(&path).unwrap();
    main[4..6].copy_from_slice(&u16::MAX.to_le_bytes());
    std::fs::write(&path, main).unwrap();
    let files = || {
        std::fs::read_dir(&root.0)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), std::fs::read(entry.path()).unwrap())
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let before = files();
    let error = Database::<f32>::open_read_only(&path, 2)
        .err()
        .expect("未知版本必须拒绝");
    assert!(matches!(
        error.underlying(),
        TriviumError::UnsupportedDatabaseVersion {
            found: u16::MAX,
            ..
        }
    ));
    assert_eq!(files(), before);
}

#[test]
fn 当前主文件中的历史组件在创建锁前给出迁移诊断() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("mixed_format_admission");
        let path = root.file("data.tdb");
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 2,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        db.insert_with_id(7, &[1., 0.], json!({"rank": 7})).unwrap();
        db.create_index("rank").unwrap();
        db.build_quiver_index(None).unwrap();
        db.close().unwrap();
        std::fs::remove_file(format!("{path}.lock")).unwrap();
        let snapshot = || {
            std::fs::read_dir(&root.0)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    (entry.file_name(), std::fs::read(entry.path()).unwrap())
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        for (suffix, offset, replacement, component, found, current) in [
            (".quiver", 4, 1u32.to_le_bytes().to_vec(), "quiver", 1, 2),
            (
                ".pidx",
                4,
                7u16.to_le_bytes().to_vec(),
                "property_index",
                7,
                8,
            ),
            (
                ".pidx",
                6,
                1u16.to_le_bytes().to_vec(),
                "property_key",
                1,
                2,
            ),
        ] {
            let component_path = format!("{path}{suffix}");
            let original = super::artifact::read(&component_path).unwrap();
            let mut bytes = original.clone();
            bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
            super::artifact::write(&component_path, bytes).unwrap();
            let before = snapshot();
            for access_mode in [
                AccessMode::ReadWrite,
                AccessMode::ReadOnly,
                AccessMode::Immutable,
            ] {
                let error = Database::<f32>::open_with_config(
                    &path,
                    Config {
                        dim: 2,
                        access_mode,
                        ..Default::default()
                    },
                )
                .err()
                .expect("历史组件必须在准入阶段拒绝");
                assert_eq!(error.code(), "TDB_MIGRATION_REQUIRED");
                assert_eq!(
                    error.details(),
                    json!({
                        "component": component, "found": found, "current": current,
                        "migration_entrypoint": "scripts/migrate.py"
                    })
                );
                assert_eq!(snapshot(), before);
            }
            super::artifact::write(&component_path, original).unwrap();
        }
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 2,
                storage_mode: mode,
                ..Default::default()
            },
        )
        .unwrap();
        let node = db.get(7).unwrap().unwrap();
        assert_eq!(node.vector, [1., 0.]);
        assert_eq!(node.payload, json!({"rank": 7}));
        assert_eq!(db.search(&[1., 0.], 1, 0, -1.).unwrap()[0].id, 7);
        db.close().unwrap();
    }
}
