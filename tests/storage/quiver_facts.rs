//! 相同长度的主数据及向量变更也必须使旧 QuIVer 失效。
use super::support::TestDir;
use serde_json::json;
use triviumdb::database::{AccessMode, Config, MissingIndexPolicy, StorageMode};
use triviumdb::{Database, TriviumError};

#[test]
fn 旧quiver不能覆盖新的节点身份或同长度向量事实() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for replace_id in [false, true] {
            let root = TestDir::new("quiver_fact_binding");
            let path = root.file("data.tdb");
            let config = Config {
                dim: 2,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            };
            let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
            db.insert_with_id(1, &[0., 1.], json!({"v":1})).unwrap();
            db.insert_with_id(2, &[0., 1.], json!({"v":2})).unwrap();
            db.build_quiver_index(None).unwrap();
            db.flush().unwrap();
            let old_index = std::fs::read(format!("{path}.quiver")).unwrap();
            let old_meta = std::fs::read(format!("{path}.quiver.meta")).unwrap();
            let old_size = std::fs::metadata(&path).unwrap().len();
            db.close().unwrap();
            let mut valid_reader = Database::<f32>::open_with_config(
                &path,
                Config {
                    access_mode: AccessMode::ReadOnly,
                    missing_index_policy: MissingIndexPolicy::Error,
                    ..config
                },
            )
            .unwrap();
            assert_eq!(
                valid_reader
                    .search(&[0., 1.], 2, 0, 0.)
                    .unwrap()
                    .iter()
                    .map(|hit| hit.id)
                    .collect::<Vec<_>>(),
                [1, 2]
            );
            valid_reader.close().unwrap();
            let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
            let winner = if replace_id {
                db.delete(1).unwrap();
                db.insert_with_id(3, &[1., 0.], json!({"v":3})).unwrap();
                3
            } else {
                db.update_vector(1, &[1., 0.]).unwrap();
                1
            };
            db.close().unwrap();
            assert_eq!(std::fs::metadata(&path).unwrap().len(), old_size);
            std::fs::write(format!("{path}.quiver"), &old_index).unwrap();
            std::fs::write(format!("{path}.quiver.meta"), &old_meta).unwrap();
            let strict = Config {
                access_mode: AccessMode::ReadOnly,
                missing_index_policy: MissingIndexPolicy::Error,
                ..config
            };
            assert!(matches!(
                Database::<f32>::open_with_config(&path, strict),
                Err(TriviumError::ImmutableArtifactInvalid { .. })
            ));
            let mut reader = Database::<f32>::open_with_config(
                &path,
                Config {
                    access_mode: AccessMode::ReadOnly,
                    ..config
                },
            )
            .unwrap();
            let hits = reader.search(&[1., 0.], 2, 0, 0.).unwrap();
            assert_eq!(
                hits.iter()
                    .map(|hit| (hit.id, hit.score))
                    .collect::<Vec<_>>(),
                [(winner, 1.), (2, 0.)]
            );
            reader.close().unwrap();
            assert_eq!(std::fs::read(format!("{path}.quiver")).unwrap(), old_index);
            assert_eq!(
                std::fs::read(format!("{path}.quiver.meta")).unwrap(),
                old_meta
            );
        }
    }
}

#[test]
fn 历史v1元数据缺少事实绑定时严格拒绝且只读回退保留原件() {
    let root = TestDir::new("quiver_legacy_meta");
    let path = root.file("data.tdb");
    let config = Config {
        dim: 2,
        auto_build_quiver: false,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"v":1})).unwrap();
    db.build_quiver_index(None).unwrap();
    db.close().unwrap();
    // v1 独立布局为 QMET、版本 1、三个长度、节点数、维度及索引 CRC，共 48 字节。
    let index = std::fs::read(format!("{path}.quiver")).unwrap();
    let mut legacy = b"QMET".to_vec();
    legacy.extend(1u32.to_le_bytes());
    legacy.extend(std::fs::metadata(&path).unwrap().len().to_le_bytes());
    legacy.extend(
        std::fs::metadata(format!("{path}.vec"))
            .map(|m| m.len())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    legacy.extend((index.len() as u64).to_le_bytes());
    legacy.extend(1u64.to_le_bytes());
    legacy.extend(2u32.to_le_bytes());
    legacy.extend(crc32fast::hash(&index).to_le_bytes());
    std::fs::write(format!("{path}.quiver.meta"), &legacy).unwrap();
    assert!(matches!(
        Database::<f32>::open_with_config(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                missing_index_policy: MissingIndexPolicy::Error,
                ..config
            }
        ),
        Err(TriviumError::ImmutableArtifactInvalid { .. })
    ));
    let mut reader = Database::<f32>::open_with_config(
        &path,
        Config {
            access_mode: AccessMode::ReadOnly,
            ..config
        },
    )
    .unwrap();
    let hit = reader.search(&[1., 0.], 1, 0, 0.).unwrap().remove(0);
    assert_eq!((hit.id, hit.score, hit.payload), (1, 1., json!({"v":1})));
    reader.close().unwrap();
    assert_eq!(
        std::fs::read(format!("{path}.quiver.meta")).unwrap(),
        legacy
    );
    assert_eq!(std::fs::read(format!("{path}.quiver")).unwrap(), index);
}
