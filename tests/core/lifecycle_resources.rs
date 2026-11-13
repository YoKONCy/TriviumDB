use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;
use triviumdb::database::{Config, StorageMode};
use triviumdb::storage::wal::SyncMode;
use triviumdb::{Database, TriviumError, VectorType};

fn directory(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "tdb 生命周期 {name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&root).unwrap();
    root
}

fn assert_released(root: &Path) {
    #[cfg(target_os = "linux")]
    {
        let prefix = format!("{}/", root.canonicalize().unwrap().display());
        let maps = std::fs::read("/proc/self/maps").unwrap();
        assert!(
            !String::from_utf8_lossy(&maps).contains(&prefix),
            "关闭后仍保留数据库文件映射：{prefix}"
        );
    }
    #[cfg(windows)]
    for entry in std::fs::read_dir(root).unwrap() {
        use std::os::windows::fs::OpenOptionsExt;
        let path = entry.unwrap().path();
        if path.is_file() {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(&path)
                .unwrap_or_else(|error| panic!("关闭后仍占用 {}：{error}", path.display()));
        }
    }
    let moved = root.with_extension("moved");
    std::fs::rename(root, &moved).expect("关闭后必须允许立即移动完整目录");
    std::fs::rename(&moved, root).unwrap();
}

fn run_close<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(name: &str) {
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        for asynchronous in [false, true] {
            let root = directory(name);
            let path = root.join("sample.tdb");
            let mut db = Database::<T>::open_with_config(
                &path.to_string_lossy(),
                Config {
                    dim: 4,
                    storage_mode: mode,
                    auto_build_quiver: false,
                    ..Default::default()
                },
            )
            .unwrap();
            db.insert_with_id(1, &[T::from_f32(1.0); 4], json!({"正文":"关闭回归"}))
                .unwrap();
            db.index_text(1, "关闭回归").unwrap();
            db.build_text_index().unwrap();
            if asynchronous {
                assert!(db.close_timeout(Duration::from_secs(5)).unwrap());
            } else {
                db.close().unwrap();
            }
            assert_released(&root);
            let mut reopened = Database::<T>::open(&path.to_string_lossy(), 4).unwrap();
            assert_eq!(reopened.node_count().unwrap(), 1);
            reopened.close().unwrap();
            std::fs::remove_dir_all(&root).unwrap();
            std::hint::black_box(db);
        }
    }
}

#[test]
fn 三种后端关闭后保留对象仍释放完整文件组() {
    run_close::<f32>("f32");
    run_close::<half::f16>("f16");
    run_close::<u64>("u64");
}

#[test]
fn 普通析构后独立构建句柄拒绝新任务并保护文件租约() {
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        let root = directory("drop-build-handle");
        let path = root.join("sample.tdb").to_string_lossy().into_owned();
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 3,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        db.insert_with_id(1, &[1., 2., 3.], json!({"state":"original"}))
            .unwrap();
        db.flush().unwrap();
        let handle = db.quiver_build_handle();
        let clone = handle.clone();
        drop(db);
        assert!(matches!(
            handle.prepare_auto_build().err().unwrap().underlying(),
            TriviumError::DatabaseClosed
        ));
        assert!(matches!(
            Database::<f32>::open(&path, 3),
            Err(TriviumError::DatabaseLocked(_))
        ));
        drop(handle);
        assert!(matches!(
            Database::<f32>::open(&path, 3),
            Err(TriviumError::DatabaseLocked(_))
        ));
        drop(clone);
        assert_released(&root);
        let mut reopened = Database::<f32>::open(&path, 3).unwrap();
        let node = reopened.get(1).unwrap().unwrap();
        assert_eq!(node.vector, vec![1., 2., 3.]);
        assert_eq!(node.payload, json!({"state":"original"}));
        reopened.close().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn 原数据库析构后已准备任务执行或取消前保持独占且完成后释放映射() {
    for execute in [false, true] {
        let root = directory("drop-prepared-build");
        let path = root.join("sample.tdb").to_string_lossy().into_owned();
        let mut db = Database::<f32>::open(&path, 3).unwrap();
        // 834 是低维默认配置的自动构建节点门槛，确保确实捕获任务。
        for id in 1..=834 {
            db.insert_with_id(id, &[1., 2., 3.], json!({"id":id}))
                .unwrap();
        }
        db.flush().unwrap();
        let prepared = db
            .prepare_auto_quiver_build()
            .unwrap()
            .expect("必须捕获真实构建任务");
        drop(db);
        assert!(matches!(
            Database::<f32>::open(&path, 3),
            Err(TriviumError::DatabaseLocked(_))
        ));
        if execute {
            assert!(prepared.execute().unwrap());
        } else {
            drop(prepared);
        }
        assert_released(&root);
        let mut reopened = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 3,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(reopened.node_count().unwrap(), 834);
        for id in 1..=834 {
            let node = reopened.get(id).unwrap().unwrap();
            assert_eq!(node.vector, vec![1., 2., 3.]);
            assert_eq!(node.payload, json!({"id":id}));
        }
        reopened.close().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn Hook析构异常仍完成关闭并返回一次结构化错误() {
    struct FailingDestructor;
    impl triviumdb::SearchHook for FailingDestructor {}
    impl Drop for FailingDestructor {
        fn drop(&mut self) {
            panic!("模拟宿主 Hook 析构异常");
        }
    }
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        for asynchronous in [false, true] {
            let root = directory("hook-drop-panic");
            let path = root.join("sample.tdb").to_string_lossy().into_owned();
            let mut db = Database::<f32>::open_with_config(
                &path,
                Config {
                    dim: 3,
                    storage_mode: mode,
                    ..Default::default()
                },
            )
            .unwrap();
            db.insert_with_id(1, &[1., 2., 3.], json!({"state":"committed"}))
                .unwrap();
            db.set_hook(FailingDestructor).unwrap();
            let result = if asynchronous {
                db.close_timeout(Duration::from_secs(10)).map(|_| ())
            } else {
                db.close()
            };
            assert!(matches!(result, Err(TriviumError::ShutdownFailed { .. })));
            assert_eq!(db.shutdown_status().phase, triviumdb::ShutdownPhase::Closed);
            assert_eq!(db.shutdown_status().last_failure.unwrap().kind, "shutdown");
            assert!(db.close_timeout(Duration::ZERO).unwrap());
            assert_released(&root);
            let mut reopened = Database::<f32>::open(&path, 3).unwrap();
            assert_eq!(reopened.get(1).unwrap().unwrap().vector, vec![1., 2., 3.]);
            assert_eq!(
                reopened.get_payload(1).unwrap().unwrap(),
                json!({"state":"committed"})
            );
            drop(db);
            assert!(matches!(
                Database::<f32>::open(&path, 3),
                Err(TriviumError::DatabaseLocked(_))
            ));
            reopened.close().unwrap();
            assert_released(&root);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

fn retained_values<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(name: &str) {
    use triviumdb::resources::{ResourceLimits, ResourceRuntime};
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        let root = directory(&format!("持有查询结果-{name}-{mode:?}"));
        let path = root.join("sample.tdb").to_string_lossy().into_owned();
        let runtime = ResourceRuntime::new(ResourceLimits {
            memory_limit: 64 * 1024 * 1024,
            maintenance_memory_limit: 8 * 1024 * 1024,
            max_threads: 2,
            ..Default::default()
        })
        .unwrap();
        let config = || Config {
            dim: 4,
            storage_mode: mode,
            memory_limit: 8 * 1024 * 1024,
            auto_build_quiver: false,
            load_text_index: true,
            ..Default::default()
        };
        let mut db = Database::<T>::open_with_runtime(&path, config(), runtime.clone()).unwrap();
        db.insert_with_id(
            1,
            &[T::from_f32(1.); 4],
            json!({"kind":"old", "body":"独立正文"}),
        )
        .unwrap();
        db.insert_with_id(2, &[T::from_f32(2.); 4], json!({"kind":"peer"}))
            .unwrap();
        db.link(1, 2, "关联", 2.).unwrap();
        db.create_index("kind").unwrap();
        db.index_text(1, "original").unwrap();
        let prepared = db
            .prepare_tql(r#"FIND {kind: "old"} AS n WITH n RETURN n"#)
            .unwrap();
        let rows = db
            .execute_prepared_tql(&prepared, &Default::default())
            .unwrap();
        let node = db.get(1).unwrap().unwrap();
        let payload = db.get_payload(1).unwrap().unwrap();
        let edges = db.get_edges(1).unwrap();
        let hits = db.search(&[T::from_f32(1.); 4], 2, 0, -1.).unwrap();
        db.close().unwrap();
        assert_eq!(runtime.stats().attached_databases, 0);
        assert_eq!(runtime.stats().reserved_database_bytes, 0);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        assert_released(&root);
        let mut replacement =
            Database::<T>::open_with_runtime(&path, config(), runtime.clone()).unwrap();
        replacement
            .update_payload(1, json!({"kind":"new"}))
            .unwrap();
        replacement.flush().unwrap();
        // 旧对象的重复关闭和销毁须保持新实例的锁、配额与已写入数据。
        assert!(matches!(db.close(), Err(TriviumError::DatabaseClosed)));
        assert!(db.close_timeout(Duration::ZERO).unwrap());
        drop(db);
        assert_eq!(runtime.stats().attached_databases, 1);
        assert_eq!(runtime.stats().reserved_database_bytes, 8 * 1024 * 1024);
        assert!(matches!(
            Database::<T>::open(&path, 4),
            Err(TriviumError::DatabaseLocked(_))
        ));
        assert_eq!(
            replacement.get_payload(1).unwrap().unwrap(),
            json!({"kind":"new"})
        );
        assert!(
            replacement
                .execute_prepared_tql(&prepared, &Default::default())
                .unwrap()
                .is_empty()
        );
        let expected_payload = json!({"kind":"old", "body":"独立正文"});
        assert_eq!(node.id, 1);
        assert_eq!(node.vector, vec![T::from_f32(1.); 4]);
        assert_eq!(node.payload, expected_payload);
        assert_eq!(payload, expected_payload);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target_id, 2);
        assert_eq!(edges[0].label, "关联");
        assert_eq!(edges[0].weight, 2.);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 1);
        let row = rows[0]["n"].as_node().expect("查询列 n 必须保留节点值");
        assert_eq!(row.id, 1);
        assert_eq!(row.payload, expected_payload);
        assert_eq!(row.vector, vec![T::from_f32(1.); 4]);
        let mut hit_ids = hits.iter().map(|hit| hit.id).collect::<Vec<_>>();
        hit_ids.sort_unstable();
        assert_eq!(hit_ids, vec![1, 2]);
        for hit in &hits {
            assert_eq!(
                hit.payload,
                match hit.id {
                    1 => expected_payload.clone(),
                    2 => json!({"kind":"peer"}),
                    _ => unreachable!(),
                }
            );
        }
        replacement.close().unwrap();
        assert_eq!(runtime.stats().attached_databases, 0);
        assert_released(&root);
        for file in std::fs::read_dir(&root).unwrap() {
            std::fs::remove_file(file.unwrap().path()).unwrap();
        }
        std::fs::remove_dir(&root).unwrap();
        std::hint::black_box((replacement, prepared, rows, node, payload, edges, hits));
    }
}

#[test]
fn 持有查询结果和预编译查询时关闭并复用路径保持实例隔离() {
    retained_values::<f32>("f32");
    retained_values::<half::f16>("f16");
    retained_values::<u64>("u64");
}

fn retained_readers<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(name: &str) {
    use triviumdb::DatabaseReader;
    use triviumdb::database::AccessMode;
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        for access in [AccessMode::ReadOnly, AccessMode::Immutable] {
            let root = directory(&format!("多读者-{name}-{mode:?}-{access:?}"));
            let path = root.join("sample.tdb").to_string_lossy().into_owned();
            let mut writer = Database::<T>::open_with_config(
                &path,
                Config {
                    dim: 4,
                    storage_mode: mode,
                    auto_build_quiver: false,
                    ..Default::default()
                },
            )
            .unwrap();
            writer
                .insert_with_id(1, &[T::from_f32(1.); 4], json!({"kind":"old"}))
                .unwrap();
            writer
                .insert_with_id(2, &[T::from_f32(2.); 4], json!({"kind":"old"}))
                .unwrap();
            writer.link(1, 2, "共享图边", 1.).unwrap();
            writer.create_index("kind").unwrap();
            writer.index_text(1, "original").unwrap();
            writer.publish_generation_manifest("多读者制品").unwrap();
            writer.close().unwrap();
            let open = || {
                DatabaseReader::<T>::open_with_config(
                    &path,
                    Config {
                        dim: 4,
                        access_mode: access,
                        load_text_index: true,
                        auto_build_quiver: false,
                        ..Default::default()
                    },
                )
                .unwrap()
            };
            let mut first = open();
            let mut second = open();
            let saved = second.get(1).unwrap().unwrap();
            first.close().unwrap();
            assert_eq!(
                second.get_payload(1).unwrap().unwrap(),
                json!({"kind":"old"})
            );
            assert_eq!(second.get_edges(1).unwrap()[0].target_id, 2);
            let mut hit_ids = second
                .search(&[T::from_f32(1.); 4], 2, 0, -1.)
                .unwrap()
                .into_iter()
                .map(|hit| hit.id)
                .collect::<Vec<_>>();
            hit_ids.sort_unstable();
            assert_eq!(hit_ids, vec![1, 2]);
            if access == AccessMode::ReadOnly {
                assert!(matches!(
                    Database::<T>::open(&path, 4),
                    Err(TriviumError::DatabaseLocked(_))
                ));
            }
            assert!(second.close_timeout(Duration::from_secs(5)).unwrap());
            assert_released(&root);
            let mut replacement = Database::<T>::open(&path, 4).unwrap();
            assert!(matches!(first.close(), Err(TriviumError::DatabaseClosed)));
            assert!(matches!(second.close(), Err(TriviumError::DatabaseClosed)));
            assert!(matches!(
                Database::<T>::open(&path, 4),
                Err(TriviumError::DatabaseLocked(_))
            ));
            replacement
                .update_payload(1, json!({"kind":"replacement"}))
                .unwrap();
            replacement.close().unwrap();
            assert_eq!(saved.payload, json!({"kind":"old"}));
            assert_released(&root);
            for file in std::fs::read_dir(&root).unwrap() {
                std::fs::remove_file(file.unwrap().path()).unwrap();
            }
            std::fs::remove_dir(&root).unwrap();
            std::hint::black_box((writer, first, second, replacement, saved));
        }
    }
}

#[test]
fn 多读者独立关闭且持有已关闭reader不阻止文件组释放() {
    retained_readers::<f32>("f32");
    retained_readers::<half::f16>("f16");
    retained_readers::<u64>("u64");
}

#[test]
fn 发布manifest后无变更关闭保持制品逐字节不变() {
    for asynchronous in [false, true] {
        let root = directory("manifest");
        let path = root.join("sample.tdb");
        let mut db = Database::<f32>::open(&path.to_string_lossy(), 4).unwrap();
        db.insert(&[1.0; 4], json!({})).unwrap();
        db.publish_generation_manifest("关闭制品").unwrap();
        let before = std::fs::read_dir(&root)
            .unwrap()
            .filter(|entry| {
                !entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .to_string_lossy()
                    .ends_with(".lock")
            })
            .map(|entry| {
                let path = entry.unwrap().path();
                let data = std::fs::read(&path).unwrap();
                (path, data)
            })
            .collect::<Vec<_>>();
        if asynchronous {
            assert!(db.close_timeout(Duration::from_secs(5)).unwrap());
        } else {
            db.close().unwrap();
        }
        for (path, data) in &before {
            assert_eq!(std::fs::read(path).unwrap(), *data);
        }
        // 再次以可写模式打开当前格式，纯读取和关闭仍须保持已发布内容。
        let mut reopened = Database::<f32>::open_with_config(
            &path.to_string_lossy(),
            Config {
                dim: 4,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(reopened.node_count().unwrap(), 1);
        reopened.close().unwrap();
        for (path, data) in before {
            assert_eq!(std::fs::read(path).unwrap(), data);
        }
        let mut reader = Database::<f32>::open_immutable(&path.to_string_lossy(), 4).unwrap();
        assert_eq!(reader.node_count().unwrap(), 1);
        reader.close().unwrap();
        assert_released(&root);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn 已关闭数据库拒绝进入分组提交闭包() {
    let root = directory("closed-group");
    let mut db = Database::<f32>::open(&root.join("sample.tdb").to_string_lossy(), 4).unwrap();
    db.close().unwrap();
    let mut called = false;
    assert!(matches!(
        db.group_commit(|_| {
            called = true;
        })
        .unwrap_err()
        .underlying(),
        TriviumError::DatabaseClosed
    ));
    assert!(!called);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn 关闭后所有数据读取返回关闭错误且诊断仍可读() {
    let root = directory("closed-reads");
    let mut db = Database::<f32>::open(&root.join("sample.tdb").to_string_lossy(), 4).unwrap();
    db.insert_with_id(1, &[1.0; 4], json!({})).unwrap();
    db.close().unwrap();
    macro_rules! closed {
        ($value:expr) => {
            assert!(matches!($value, Err(TriviumError::DatabaseClosed)));
        };
    }
    closed!(db.get(1));
    closed!(db.get_payload(1));
    closed!(db.get_edges(1));
    closed!(db.get_edge(1, 2, "关联"));
    closed!(db.get_incoming_edges(1, None));
    closed!(db.contains(1));
    closed!(db.node_count());
    closed!(db.all_node_ids());
    closed!(db.get_all_ids());
    closed!(db.neighbors(1, 1));
    closed!(db.neighbors_with_labels(1, 1, None));
    closed!(db.shortest_path(1, 2, 3, None));
    closed!(db.graph_stats());
    closed!(db.validate_graph());
    closed!(db.list_indexes());
    closed!(db.list_unique_indexes());
    closed!(db.index_info());
    closed!(db.clear_search_state());
    closed!(db.set_hook(triviumdb::NoopHook));
    assert_eq!(db.dim(), 4);
    assert_eq!(db.shutdown_status().phase, triviumdb::ShutdownPhase::Closed);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn 分组闭包内关闭立即报错且不阻塞原同步组() {
    let root = directory("group-close");
    let mut db = Database::<f32>::open(&root.join("sample.tdb").to_string_lossy(), 4).unwrap();
    db.group_commit(|db| {
        assert!(matches!(
            db.close(),
            Err(triviumdb::TriviumError::ReentrantAccess {
                operation: "group_commit_shutdown"
            })
        ));
        assert!(matches!(
            db.request_shutdown(),
            Err(triviumdb::TriviumError::ReentrantAccess {
                operation: "group_commit_shutdown"
            })
        ));
        db.insert_with_id(1, &[1.0; 4], json!({})).unwrap();
    })
    .unwrap();
    db.close().unwrap();
    assert_released(&root);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn 检查点后文本与属性索引变化必须在关闭时落盘() {
    let root = directory("dirty-index");
    let path = root.join("sample.tdb");
    let mut db = Database::<f32>::open(&path.to_string_lossy(), 4).unwrap();
    db.insert_with_id(1, &[1.0; 4], json!({"类别":"回归"}))
        .unwrap();
    db.flush().unwrap();
    db.index_text(1, "检查点 checkpoint 正文").unwrap();
    db.build_text_index().unwrap();
    db.create_index("类别").unwrap();
    db.close().unwrap();
    let reopened = Database::<f32>::open_with_config(
        &path.to_string_lossy(),
        Config {
            dim: 4,
            load_text_index: true,
            auto_build_quiver: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(reopened.list_indexes().unwrap(), vec!["类别"]);
    assert!(
        !reopened
            .search_hybrid(
                Some("checkpoint"),
                None,
                &triviumdb::SearchConfig {
                    enable_text_hybrid_search: true,
                    min_score: 0.0,
                    ..Default::default()
                }
            )
            .unwrap()
            .is_empty()
    );
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn 分组提交异常子进程() {
    let Ok(path) = std::env::var("TDB_GROUP_UNWIND_CHILD") else {
        return;
    };
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: 4,
            sync_mode: SyncMode::Full,
            auto_build_quiver: false,
            ..Default::default()
        },
    )
    .unwrap();
    db.insert_with_id(1, &[1.0; 4], json!({})).unwrap();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = db.group_commit(|_| panic!("模拟调用方异常展开"));
    }));
    db.insert_with_id(2, &[2.0; 4], json!({})).unwrap();
    assert!(
        db.group_commit(|db| db.insert_with_id(3, &[3.0; 4], json!({})))
            .unwrap()
            .0
            .is_ok()
    );
    // 绕过析构，独立验证已经确认的 Full 写入可以恢复。
    std::process::exit(0);
}

#[test]
fn 分组异常后确认的Full写入跨进程恢复() {
    let root = directory("unwind");
    let path = root.join("sample.tdb");
    let executable = std::env::current_exe().unwrap();
    // Cargo 的仿真启动参数不会自动传给测试内部创建的子进程。
    let mut command = if std::env::var_os("TRIVIUM_TEST_QEMU_AARCH64").is_some() {
        let mut command = std::process::Command::new("qemu-aarch64");
        command
            .arg("-L")
            .arg("/usr/aarch64-linux-gnu")
            .arg(executable);
        command
    } else {
        std::process::Command::new(executable)
    };
    let status = command
        .args([
            "--exact",
            "lifecycle_resources::分组提交异常子进程",
            "--nocapture",
        ])
        .env("TDB_GROUP_UNWIND_CHILD", &path)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "分组提交异常子进程启动或执行失败：{status}"
    );
    let mut db = Database::<f32>::open(&path.to_string_lossy(), 4).unwrap();
    for id in 1..=3 {
        assert!(
            db.contains(id).unwrap(),
            "Full 确认写入的节点 {id} 必须恢复"
        );
    }
    db.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
