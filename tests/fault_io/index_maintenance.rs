//! 维护请求在 WAL 关闭后拒绝，约束与文本发布按真实文件组提交点恢复。

#[path = "../support/index_maintenance.rs"]
mod maintenance;
use serde_json::json;
use triviumdb::database::{Config, StorageMode};
use triviumdb::storage::wal::SyncMode;
use triviumdb::test_hooks::{IoPoint, fail_io_at};
use triviumdb::{Database, VectorType};

#[test]
fn 图修复发布失败隔离实例并按真实提交点恢复完整事实() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for point in [IoPoint::GraphSidecarWrite, IoPoint::CheckpointWalClear] {
            let root = super::support::TestDir::new("graph-repair-publication");
            let path = root.file("db.tdb");
            let config = Config {
                dim: 2,
                storage_mode: mode,
                load_text_index: true,
                auto_build_quiver: false,
                ..Default::default()
            };
            let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
            maintenance::seed(&mut db);
            let failure = fail_io_at(point);
            let error = db.repair_graph_indexes().unwrap_err();
            drop(failure);
            assert_eq!(error.code(), "TDB_IO");
            let outcome = if point == IoPoint::CheckpointWalClear {
                "committed"
            } else {
                "unknown"
            };
            assert_eq!(
                serde_json::to_value(error.info()).unwrap()["write"],
                json!({"phase":"publish","outcome":outcome})
            );
            assert_eq!(db.node_count().unwrap_err().code(), "TDB_WAL_CLOSED");
            drop(db);
            for _ in 0..2 {
                let mut recovered = Database::<f32>::open_with_config(&path, config).unwrap();
                assert_eq!(
                    maintenance::facts(&recovered),
                    maintenance::expected(false, json!([]))
                );
                let report = recovered.repair_graph_indexes().unwrap();
                assert_eq!(report.removed_dangling_edges, 0);
                assert_eq!(report.removed_duplicate_edges, 0);
                assert!(report.rebuilt_indexes);
                recovered.close().unwrap();
            }
        }
    }
}

#[test]
fn 已捕获构建在开始及发布前拒绝失效writer并归还任务资源() {
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
    for (during_build, explicit) in [(false, false), (true, false), (true, true)] {
        let root = super::support::TestDir::new("prepared-wal-admission");
        let path = root.file("db.tdb");
        let config = Config {
            dim: 3,
            sync_mode: SyncMode::Full,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        // 834 是低维默认配置的自动构建门槛，捕获前保留尚未建图的真实事实。
        for id in 1..=834 {
            db.insert_with_id(id, &[1., 2., 3.], json!({"id":id}))
                .unwrap();
        }
        db.flush().unwrap();
        let handle = db.quiver_build_handle();
        let prepared = if explicit {
            None
        } else {
            Some(
                handle
                    .prepare_auto_build()
                    .unwrap()
                    .expect("必须捕获构建任务"),
            )
        };
        let db = std::sync::Arc::new(db);
        let worker_db = db.clone();
        let pause = during_build.then(|| pause_at(ConcurrencyPoint::BeforeQuiverPublish));
        let (start, ready) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            ready.recv().unwrap();
            if explicit {
                worker_db.build_quiver_index(None)
            } else {
                prepared.unwrap().execute().map(|_| ())
            }
        });
        if during_build {
            start.send(()).unwrap();
            pause.as_ref().unwrap().wait_until_arrived();
        }
        let failure = fail_io_at(IoPoint::WalSync);
        assert_eq!(
            db.update_vector(1, &[1., 2., 3.]).unwrap_err().code(),
            "TDB_IO"
        );
        drop(failure);
        if during_build {
            pause.unwrap().release();
        } else {
            start.send(()).unwrap();
        }
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.code(), "TDB_WAL_CLOSED");
        let phase = if during_build {
            "validation"
        } else {
            "admission"
        };
        assert_eq!(
            serde_json::to_value(error.info()).unwrap()["write"],
            json!({"phase":phase,"outcome":"not_committed"})
        );
        let error = handle.prepare_auto_build().err().unwrap();
        assert_eq!(error.code(), "TDB_WAL_CLOSED");
        assert_eq!(
            serde_json::to_value(error.info()).unwrap()["write"],
            json!({"phase":"admission","outcome":"not_committed"})
        );
        assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
        assert_eq!(db.resource_stats().active_tasks, 0);
        drop(db);
        // 独立句柄仍持有文件租约，释放后才移动目录并核对恢复事实。
        assert_eq!(
            Database::<f32>::open(&path, 3).err().unwrap().code(),
            "TDB_DATABASE_LOCKED"
        );
        drop(handle);
        let moved = root.0.with_extension("moved");
        std::fs::rename(&root.0, &moved).unwrap();
        let path = moved.join("db.tdb");
        for _ in 0..2 {
            let mut db = Database::<f32>::open_with_config(
                path.to_str().unwrap(),
                Config {
                    auto_build_quiver: false,
                    ..config
                },
            )
            .unwrap();
            assert_eq!(db.all_node_ids().unwrap(), (1..=834).collect::<Vec<_>>());
            for id in 1..=834 {
                let node = db.get(id).unwrap().unwrap();
                assert_eq!(node.vector, [1., 2., 3.]);
                assert_eq!(node.payload, json!({"id":id}));
                assert!(node.edges.is_empty());
            }
            db.close().unwrap();
        }
        std::fs::remove_dir_all(moved).unwrap();
    }
}

fn closed_writer<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(dtype: &str) {
    let root = super::support::TestDir::new(dtype);
    let contract = maintenance::contract();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let directory = root.0.join(format!("{mode:?}"));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("db.tdb");
        let config = Config {
            dim: 2,
            storage_mode: mode,
            load_text_index: true,
            auto_build_quiver: false,
            sync_mode: SyncMode::Full,
            ..Default::default()
        };
        let mut db = Database::<T>::open_with_config(path.to_str().unwrap(), config).unwrap();
        maintenance::seed(&mut db);
        let failure = fail_io_at(IoPoint::WalSync);
        assert_eq!(
            db.update_payload(1, contract["nodes"][0]["payload"].clone())
                .unwrap_err()
                .code(),
            "TDB_IO"
        );
        drop(failure);
        let files = || {
            std::fs::read_dir(&directory)
                .unwrap()
                // 当前 writer 持有 .lock 的排他字节区间，完整比较其余事实与发布制品。
                .filter(|entry| {
                    entry.as_ref().unwrap().path().extension() != Some(std::ffi::OsStr::new("lock"))
                })
                .map(|entry| {
                    let entry = entry.unwrap();
                    (entry.file_name(), std::fs::read(entry.path()).unwrap())
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let before = files();
        for case in contract["operations"].as_array().unwrap() {
            let error = maintenance::execute(&mut db, case).unwrap_err();
            assert_eq!(error.code(), "TDB_WAL_CLOSED", "{dtype}/{mode:?}/{case}");
            assert_eq!(
                serde_json::to_value(error.info()).unwrap()["write"],
                contract["admission"]
            );
        }
        assert_eq!(files(), before);
        assert_eq!(
            maintenance::facts(&db),
            maintenance::expected(false, json!([]))
        );
        drop(db);
        let moved = directory.with_extension("moved");
        std::fs::rename(&directory, &moved).unwrap();
        let path = moved.join("db.tdb");
        let mut db = Database::<T>::open_with_config(path.to_str().unwrap(), config).unwrap();
        assert_eq!(
            maintenance::facts(&db),
            maintenance::expected(false, json!([]))
        );
        db.close().unwrap();
        let mut db = Database::<T>::open_with_config(path.to_str().unwrap(), config).unwrap();
        assert_eq!(
            maintenance::facts(&db),
            maintenance::expected(false, json!([]))
        );
        db.close().unwrap();
    }
}

fn schema_publication<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(dtype: &str) {
    let root = super::support::TestDir::new(dtype);
    let contract = maintenance::contract();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for composite in [false, true] {
            for create in [false, true] {
                for point in [
                    IoPoint::PublicationBackupCopy,
                    IoPoint::PropertySidecarWrite,
                    IoPoint::GraphSidecarWrite,
                    IoPoint::CheckpointWalClear,
                ] {
                    let directory = root
                        .0
                        .join(format!("{mode:?}-{composite}-{create}-{point:?}"));
                    std::fs::create_dir(&directory).unwrap();
                    let path = directory.join("db.tdb");
                    let config = Config {
                        dim: 2,
                        storage_mode: mode,
                        load_text_index: true,
                        auto_build_quiver: false,
                        ..Default::default()
                    };
                    let mut db =
                        Database::<T>::open_with_config(path.to_str().unwrap(), config).unwrap();
                    maintenance::seed(&mut db);
                    let fields = if composite {
                        json!(["tenant", "tag"])
                    } else {
                        json!(["tag"])
                    };
                    let create_case = if composite {
                        json!({"api":"create_unique_composite_index","fields":fields})
                    } else {
                        json!({"api":"create_unique_index","field":"tag"})
                    };
                    if !create {
                        maintenance::execute(&mut db, &create_case).unwrap();
                    }
                    let case = if create {
                        create_case
                    } else if composite {
                        json!({"api":"drop_composite_index","fields":fields})
                    } else {
                        json!({"api":"drop_index","field":"tag"})
                    };
                    let failure = fail_io_at(point);
                    let error = maintenance::execute(&mut db, &case).unwrap_err();
                    drop(failure);
                    assert_eq!(error.code(), "TDB_IO", "{mode:?}/{case}/{point:?}");
                    let committed = point == IoPoint::CheckpointWalClear;
                    assert_eq!(
                        serde_json::to_value(error.info()).unwrap()["write"],
                        contract[if committed {
                            "publish_committed"
                        } else {
                            "publish_unknown"
                        }]
                    );
                    assert_eq!(db.node_count().unwrap_err().code(), "TDB_WAL_CLOSED");
                    let wal = std::fs::read(path.with_extension("tdb.wal")).unwrap();
                    let rejected = db.index_text(1, "rejected").unwrap_err();
                    assert_eq!(rejected.code(), "TDB_WAL_CLOSED");
                    assert_eq!(
                        serde_json::to_value(rejected.info()).unwrap()["write"],
                        contract["admission"]
                    );
                    assert_eq!(std::fs::read(path.with_extension("tdb.wal")).unwrap(), wal);
                    drop(db);
                    let moved = directory.with_extension("moved");
                    std::fs::rename(&directory, &moved).unwrap();
                    let path = moved.join("db.tdb");
                    let mut db =
                        Database::<T>::open_with_config(path.to_str().unwrap(), config).unwrap();
                    let has_constraint = if committed { create } else { !create };
                    let indexes = if has_constraint {
                        json!([{
                            "kind":if composite {"Composite"} else {"Hash"},"fields":fields,"unique":true
                        }])
                    } else {
                        json!([])
                    };
                    assert_eq!(
                        maintenance::facts(&db),
                        maintenance::expected(false, indexes.clone())
                    );
                    let duplicate = db.insert_with_id(
                        3,
                        &[T::from_f32(1.), T::from_f32(1.)],
                        json!({"tag":"alpha","tenant":"t","category":"same"}),
                    );
                    let tag = if has_constraint {
                        assert_eq!(duplicate.unwrap_err().code(), "TDB_UNIQUE_CONSTRAINT");
                        db.insert_with_id(
                            3,
                            &[T::from_f32(1.), T::from_f32(1.)],
                            json!({"tag":"gamma","tenant":"t","category":"same"}),
                        )
                        .unwrap();
                        "gamma"
                    } else {
                        duplicate.unwrap();
                        "alpha"
                    };
                    db.close().unwrap();
                    let mut db =
                        Database::<T>::open_with_config(path.to_str().unwrap(), config).unwrap();
                    let mut expected = maintenance::expected(false, indexes);
                    expected["nodes"].as_array_mut().unwrap().push(json!({
                        "id":3,"vector":[1.0,1.0],"payload":{"tag":tag,"tenant":"t","category":"same"},"edges":[]
                    }));
                    assert_eq!(maintenance::facts(&db), expected);
                    db.close().unwrap();
                }
            }
        }
    }
}

#[test]
fn maintenance_closed_writer_f32() {
    closed_writer::<f32>("closed-maintenance-f32");
}
#[test]
fn maintenance_closed_writer_f16() {
    closed_writer::<half::f16>("closed-maintenance-f16");
}
#[test]
fn maintenance_closed_writer_u64() {
    closed_writer::<u64>("closed-maintenance-u64");
}
#[test]
fn maintenance_schema_publication_f32() {
    schema_publication::<f32>("schema-maintenance-f32");
}
#[test]
fn maintenance_schema_publication_f16() {
    schema_publication::<half::f16>("schema-maintenance-f16");
}
#[test]
fn maintenance_schema_publication_u64() {
    schema_publication::<u64>("schema-maintenance-u64");
}

#[test]
fn text_checkpoint_outcomes_follow_the_publication_point() {
    let root = super::support::TestDir::new("text-checkpoint-outcomes");
    let contract = maintenance::contract();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for api in ["flush", "compact"] {
            for point in [IoPoint::TextMetadataWrite, IoPoint::CheckpointWalClear] {
                let path = root.file(&format!("{mode:?}-{api}-{point:?}.tdb"));
                let config = Config {
                    dim: 2,
                    storage_mode: mode,
                    load_text_index: true,
                    auto_build_quiver: false,
                    ..Default::default()
                };
                let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
                maintenance::seed(&mut db);
                maintenance::execute(&mut db, &json!({"api":"rebuild_text_index"})).unwrap();
                let failure = fail_io_at(point);
                let error = maintenance::execute(&mut db, &json!({"api":api})).unwrap_err();
                drop(failure);
                let committed = point == IoPoint::CheckpointWalClear;
                assert_eq!(error.code(), "TDB_IO");
                assert_eq!(
                    serde_json::to_value(error.info()).unwrap()["write"],
                    contract[if committed {
                        "publish_committed"
                    } else {
                        "publish_unknown"
                    }]
                );
                drop(db);
                let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
                assert_eq!(
                    maintenance::facts(&db),
                    maintenance::expected(committed, json!([]))
                );
                db.close().unwrap();
                let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
                assert_eq!(
                    maintenance::facts(&db),
                    maintenance::expected(committed, json!([]))
                );
                db.close().unwrap();
            }
        }
    }
}
