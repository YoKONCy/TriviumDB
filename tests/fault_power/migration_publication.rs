//! 迁移发布前后强杀子进程，独立核对源事实、目标提交边界和只读零写。

use super::{child_command, support::TestDir};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};
use triviumdb::database::{Config, StorageMode};
use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
use triviumdb::{Database, TriviumError};

fn config(mode: StorageMode, dim: usize) -> Config {
    Config {
        dim,
        storage_mode: mode,
        auto_build_quiver: false,
        ..Default::default()
    }
}

#[test]
fn __migration_child() {
    let Ok(root) = std::env::var("TDB_MIGRATION_POWER_ROOT") else {
        return;
    };
    let phase = std::env::var("TDB_MIGRATION_POWER_PHASE").unwrap();
    let mode = match std::env::var("TDB_MIGRATION_POWER_MODE").unwrap().as_str() {
        "rom" => StorageMode::Rom,
        "mmap" => StorageMode::Mmap,
        _ => panic!("迁移强杀子进程的存储模式无效"),
    };
    let root = std::path::PathBuf::from(root);
    let point = match phase.as_str() {
        "first_file" => ConcurrencyPoint::MigrationFilePublished,
        "before_commit" => ConcurrencyPoint::MigrationBeforeCommit,
        "after_commit" => ConcurrencyPoint::MigrationCommitted,
        _ => panic!("迁移强杀阶段无效"),
    };
    let pause = pause_at(point);
    let worker_root = root.clone();
    let worker = std::thread::spawn(move || {
        let db = Database::<f32>::open_with_config(
            worker_root.join("source.tdb").to_str().unwrap(),
            config(mode, 2),
        )
        .unwrap();
        db.migrate_to(worker_root.join("target.tdb").to_str().unwrap(), 3)
            .unwrap();
    });
    pause.wait_until_arrived();
    std::fs::write(root.join("ready"), "迁移已到达发布屏障").unwrap();
    // 由父进程在屏障确认后强杀，不允许析构替测试完成暂存清理。
    worker.join().unwrap();
}

fn files(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let leaf = path.file_name().unwrap().to_str().unwrap();
    std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            (name == leaf || name.starts_with(&format!("{leaf}.")))
                .then(|| (name, std::fs::read(entry.path()).unwrap()))
        })
        .collect()
}

fn assert_facts(db: &Database<f32>, migrated: bool) {
    assert_eq!(db.all_node_ids().unwrap(), [7, 42]);
    for id in [7, 42] {
        let node = db.get(id).unwrap().unwrap();
        assert_eq!(node.payload, json!({"id":id,"text":"迁移事实"}));
        assert_eq!(
            node.vector,
            if migrated {
                vec![0.; 3]
            } else {
                vec![id as f32, -1.]
            }
        );
        if id == 7 {
            assert_eq!(node.edges.len(), 1);
            let edge = &node.edges[0];
            assert_eq!(edge.target_id, 42);
            assert_eq!(edge.label, "关联");
            assert_eq!(edge.weight, 1.5);
            assert_eq!(edge.metadata, json!({"kind":"完整关系"}));
        } else {
            assert!(node.edges.is_empty());
        }
    }
}

#[test]
fn 迁移强杀后按提交点恢复且保留全部源事实() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for phase in ["first_file", "before_commit", "after_commit"] {
            let root = TestDir::new("migration_power");
            let source = root.file("source.tdb");
            let target = root.file("target.tdb");
            let mut db = Database::<f32>::open_with_config(&source, config(mode, 2)).unwrap();
            for id in [7, 42] {
                db.insert_with_id(id, &[id as f32, -1.], json!({"id":id,"text":"迁移事实"}))
                    .unwrap();
            }
            db.upsert_edge(7, 42, "关联", 1.5, json!({"kind":"完整关系"}))
                .unwrap();
            db.close().unwrap();
            drop(db);
            let original = files(Path::new(&source));
            let mut child = child_command()
                .args([
                    "--exact",
                    "migration_publication::__migration_child",
                    "--nocapture",
                ])
                .env("TDB_MIGRATION_POWER_ROOT", &root.0)
                .env("TDB_MIGRATION_POWER_PHASE", phase)
                .env(
                    "TDB_MIGRATION_POWER_MODE",
                    if mode == StorageMode::Rom {
                        "rom"
                    } else {
                        "mmap"
                    },
                )
                .spawn()
                .unwrap();
            let ready = root.0.join("ready");
            let deadline = Instant::now() + Duration::from_secs(30);
            while !ready.exists() && Instant::now() < deadline {
                if child.try_wait().unwrap().is_some() {
                    panic!("迁移子进程提前退出：{mode:?}/{phase}");
                }
                // 文件回执来自已到达的屏障；轮询等待不作为同步完成证据。
                std::thread::sleep(Duration::from_millis(5));
            }
            let arrived = ready.exists();
            child.kill().unwrap();
            child.wait().unwrap();
            assert!(arrived, "迁移子进程未到达屏障：{mode:?}/{phase}");
            assert_eq!(
                files(Path::new(&source)),
                original,
                "源文件组必须逐字节保留"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                assert_eq!(std::fs::metadata(&target).unwrap().nlink(), 1);
            }
            let committed = phase == "after_commit";
            let before = files(Path::new(&target));
            let read_only = Database::<f32>::open_read_only(&target, 3);
            if committed {
                let mut reader = read_only.unwrap();
                assert_facts(&reader, true);
                reader.close().unwrap();
            } else {
                assert!(matches!(
                    read_only,
                    Err(TriviumError::RecoveryRequired { .. })
                ));
            }
            assert_eq!(files(Path::new(&target)), before, "只读路径必须物理零写");
            for _ in 0..2 {
                let mut source_db =
                    Database::<f32>::open_with_config(&source, config(mode, 2)).unwrap();
                assert_facts(&source_db, false);
                source_db.flush().unwrap();
                source_db.close().unwrap();
                let mut target_db = Database::<f32>::open(&target, 3).unwrap();
                if committed {
                    assert_facts(&target_db, true);
                } else {
                    // 迁移尚未提交；目标回滚为空，完整事实仍由源库独立验证。
                    assert_eq!(target_db.node_count().unwrap(), 0);
                    assert!(target_db.all_node_ids().unwrap().is_empty());
                }
                target_db.flush().unwrap();
                target_db.close().unwrap();
            }
        }
    }
}
