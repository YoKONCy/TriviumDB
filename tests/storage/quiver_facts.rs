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
            let old_index = crate::artifact::read(format!("{path}.quiver")).unwrap();
            let old_meta = crate::artifact::read(format!("{path}.quiver.meta")).unwrap();
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
            crate::artifact::write(format!("{path}.quiver"), &old_index).unwrap();
            crate::artifact::write(format!("{path}.quiver.meta"), &old_meta).unwrap();
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
            assert_eq!(
                crate::artifact::read(format!("{path}.quiver")).unwrap(),
                old_index
            );
            assert_eq!(
                crate::artifact::read(format!("{path}.quiver.meta")).unwrap(),
                old_meta
            );
        }
    }
}
