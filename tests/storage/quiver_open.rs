//! 默认SM快照的完整重开、只读零写、恢复与文件资源契约。
use super::support::TestDir;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use triviumdb::database::{AccessMode, Config, MissingIndexPolicy, StorageMode};
use triviumdb::node::Edge;
use triviumdb::resources::{ResourceLimits, ResourceRuntime};
use triviumdb::storage::wal::{Wal, WalEntry};
use triviumdb::{Database, TriviumError, VectorType};

fn records(dim: usize) -> Vec<(u64, Vec<f32>)> {
    [(7, 0, 3.), (11, 1, 4.), (13, 0, -2.), (17, 0, 0.)]
        .into_iter()
        .map(|(id, axis, value)| {
            let mut vector = vec![0.; dim];
            vector[axis] = value;
            (id, vector)
        })
        .collect()
}

fn facts<T: VectorType + Serialize + DeserializeOwned>(db: &Database<T>, dim: usize) {
    assert_eq!(db.node_count().unwrap(), 4);
    for (id, expected) in records(dim) {
        let node = db.get(id).unwrap().unwrap();
        assert_eq!(node.id, id);
        assert_eq!(
            node.vector
                .iter()
                .map(|value| value.to_f32())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(node.payload, json!({"id":id,"body":"文档证据"}));
        let edges = if id == 7 {
            vec![Edge {
                target_id: 11,
                label: "引用".into(),
                weight: 0.5,
                metadata: json!({"source":7}),
            }]
        } else {
            vec![]
        };
        assert_eq!(node.edges, edges);
    }
    let mut query = vec![T::from_f32(0.); dim];
    query[0] = T::from_f32(1.);
    assert_eq!(
        db.search(&query, 4, 0, -1.)
            .unwrap()
            .iter()
            .map(|hit| (hit.id, hit.score))
            .collect::<Vec<_>>(),
        [(7, 1.), (11, 0.), (17, 0.), (13, -1.)]
    );
}

fn files(root: &std::path::Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
    let mut result = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect::<Vec<_>>();
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result
}

fn reopen<T: VectorType + Serialize + DeserializeOwned>() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for dim in [65, 768] {
            let root = TestDir::new("quiver_cached_open");
            let path = root.file("data.tdb");
            let config = Config {
                dim,
                storage_mode: mode,
                auto_build_quiver: false,
                memory_limit: 8 * 1024 * 1024,
                ..Config::default()
            };
            let runtime = ResourceRuntime::new(ResourceLimits {
                memory_limit: 32 * 1024 * 1024,
                max_threads: 2,
                ..ResourceLimits::default()
            })
            .unwrap();
            let mut db = Database::<T>::open_with_runtime(&path, config, runtime.clone()).unwrap();
            for (id, vector) in records(dim) {
                db.insert_with_id(
                    id,
                    &vector.into_iter().map(T::from_f32).collect::<Vec<_>>(),
                    json!({"id":id,"body":"文档证据"}),
                )
                .unwrap();
            }
            db.upsert_edge(7, 11, "引用", 0.5, json!({"source":7}))
                .unwrap();
            db.build_quiver_index(None).unwrap();
            db.close().unwrap();
            let original = files(&root.0);
            let index = std::fs::read(format!("{path}.quiver")).unwrap();
            assert_eq!(&index[4..8], &2u32.to_le_bytes());
            for access_mode in [AccessMode::ReadOnly, AccessMode::ReadWrite] {
                let mut db = Database::<T>::open_with_runtime(
                    &path,
                    Config {
                        access_mode,
                        missing_index_policy: MissingIndexPolicy::Error,
                        ..config
                    },
                    runtime.clone(),
                )
                .unwrap();
                assert!(db.quiver_sm_l2_enabled());
                facts(&db, dim);
                let retained = db.get(7).unwrap().unwrap();
                db.close().unwrap();
                assert_eq!(
                    retained
                        .vector
                        .iter()
                        .map(|value| value.to_f32())
                        .collect::<Vec<_>>(),
                    records(dim)[0].1
                );
                assert_eq!(runtime.stats().reserved_database_bytes, 0);
                assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
                assert_eq!(runtime.stats().attached_databases, 0);
                assert_eq!(files(&root.0), original);
            }
            // 所有Reader及索引映射完成释放后，文件组可移动并从新路径再次打开。
            let moved = root.0.join("moved");
            std::fs::create_dir(&moved).unwrap();
            for (name, _) in original {
                std::fs::rename(root.0.join(&name), moved.join(name)).unwrap();
            }
            let moved_path = moved.join("data.tdb");
            let mut db = Database::<T>::open_with_config(
                moved_path.to_str().unwrap(),
                Config {
                    access_mode: AccessMode::ReadOnly,
                    missing_index_policy: MissingIndexPolicy::Error,
                    ..config
                },
            )
            .unwrap();
            facts(&db, dim);
            db.close().unwrap();
        }
    }
}

#[test]
fn f32缓存重开保持完整事实配额归还与文件释放() {
    reopen::<f32>();
}

#[test]
fn f16缓存重开保持完整事实配额归还与文件释放() {
    reopen::<half::f16>();
}

#[test]
fn 缓存内部损坏在外层CRC有效时严格拒绝且只读回退保留原件() {
    let root = TestDir::new("quiver_cached_corrupt");
    let path = root.file("data.tdb");
    let config = Config {
        dim: 65,
        auto_build_quiver: false,
        memory_limit: 4 * 1024 * 1024,
        ..Config::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    for (id, vector) in records(65) {
        db.insert_with_id(id, &vector, json!({"id":id,"body":"文档证据"}))
            .unwrap();
    }
    db.upsert_edge(7, 11, "引用", 0.5, json!({"source":7}))
        .unwrap();
    db.build_quiver_index(None).unwrap();
    db.close().unwrap();
    let mut index = std::fs::read(format!("{path}.quiver")).unwrap();
    *index.last_mut().unwrap() ^= 1;
    let mut meta = std::fs::read(format!("{path}.quiver.meta")).unwrap();
    meta[44..48].copy_from_slice(&crc32fast::hash(&index).to_le_bytes());
    let crc = crc32fast::hash(&meta[..56]);
    meta[56..60].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(format!("{path}.quiver"), &index).unwrap();
    std::fs::write(format!("{path}.quiver.meta"), &meta).unwrap();
    let before = files(&root.0);
    let runtime = ResourceRuntime::new(ResourceLimits {
        memory_limit: 16 * 1024 * 1024,
        max_threads: 1,
        ..ResourceLimits::default()
    })
    .unwrap();
    assert!(matches!(
        Database::<f32>::open_with_runtime(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                missing_index_policy: MissingIndexPolicy::Error,
                ..config
            },
            runtime.clone(),
        ),
        Err(TriviumError::ImmutableArtifactInvalid { .. })
    ));
    assert_eq!(runtime.stats().reserved_database_bytes, 0);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert_eq!(runtime.stats().attached_databases, 0);
    assert_eq!(runtime.stats().active_tasks, 0);
    let saved_index = format!("{path}.quiver.saved");
    std::fs::rename(format!("{path}.quiver"), &saved_index).unwrap();
    std::fs::rename(&saved_index, format!("{path}.quiver")).unwrap();
    let mut reader = Database::<f32>::open_with_config(
        &path,
        Config {
            access_mode: AccessMode::ReadOnly,
            ..config
        },
    )
    .unwrap();
    facts(&reader, 65);
    reader.close().unwrap();
    assert_eq!(files(&root.0), before);
}

#[test]
fn WAL更新覆盖持久SM缓存并可再次持久化重开() {
    let root = TestDir::new("quiver_cached_wal");
    let path = root.file("data.tdb");
    let config = Config {
        dim: 2,
        auto_build_quiver: false,
        ..Config::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    db.insert_with_id(7, &[3., 0.], json!({"v":7})).unwrap();
    db.insert_with_id(11, &[0., 4.], json!({"v":11})).unwrap();
    db.build_quiver_index(None).unwrap();
    db.close().unwrap();
    let mut wal = Wal::open(&path).unwrap();
    wal.append(&WalEntry::UpdateVector {
        id: 11,
        vector: vec![2f32, 0.],
    })
    .unwrap();
    drop(wal);
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    assert_eq!(db.get(11).unwrap().unwrap().vector, [2., 0.]);
    assert_eq!(
        db.search(&[1., 0.], 2, 0, -1.)
            .unwrap()
            .iter()
            .map(|hit| (hit.id, hit.score, hit.payload.clone()))
            .collect::<Vec<_>>(),
        [(7, 1., json!({"v":7})), (11, 1., json!({"v":11}))]
    );
    db.flush().unwrap();
    db.close().unwrap();
    let mut reader = Database::<f32>::open_with_config(
        &path,
        Config {
            access_mode: AccessMode::ReadOnly,
            ..config
        },
    )
    .unwrap();
    assert_eq!(reader.get(7).unwrap().unwrap().vector, [3., 0.]);
    assert_eq!(reader.get(11).unwrap().unwrap().vector, [2., 0.]);
    assert_eq!(
        reader
            .search(&[1., 0.], 2, 0, -1.)
            .unwrap()
            .iter()
            .map(|hit| (hit.id, hit.score))
            .collect::<Vec<_>>(),
        [(7, 1.), (11, 1.)]
    );
    reader.close().unwrap();
}
