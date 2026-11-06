//! 管理配置拒绝保留配额、事实与只读文件组，既有句柄使用最新构建预算。

use super::index_maintenance::maintenance;
use serde_json::json;
use triviumdb::database::{AccessMode, Config, StorageMode};
use triviumdb::{Database, ResourceLimits, ResourceRuntime};

#[test]
fn 配额拒绝保留配置并允许只读实例调整运行时额度() {
    const MB: usize = 1024 * 1024;
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = super::support::TestDir::new("management-quota");
        let path = root.file("db.tdb");
        let runtime = ResourceRuntime::new(ResourceLimits {
            memory_limit: 8 * MB,
            maintenance_memory_limit: MB,
            ..Default::default()
        })
        .unwrap();
        let config = Config {
            dim: 2,
            storage_mode: mode,
            memory_limit: 2 * MB,
            load_text_index: true,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_runtime(&path, config, runtime.clone()).unwrap();
        maintenance::seed(&mut db);
        let mut blocker = Database::<f32>::open_with_runtime(
            &root.file("blocker.tdb"),
            Config {
                memory_limit: 4 * MB,
                ..config
            },
            runtime.clone(),
        )
        .unwrap();
        let error = db.set_memory_limit(4 * MB).unwrap_err();
        assert_eq!(error.code(), "QueueFull");
        assert_eq!(
            error.details(),
            json!({"resource":"memory_bytes","requested":2 * MB,"used":6 * MB,"limit":7 * MB})
        );
        let info = serde_json::to_value(error.info()).unwrap();
        assert_eq!(info["target"], json!({"field":"bytes"}));
        assert_eq!(
            info["write"],
            json!({"phase":"validation","outcome":"not_committed"})
        );
        assert_eq!(runtime.stats().reserved_database_bytes, 6 * MB);
        let error = db.reserve_nodes(1_000_000).unwrap_err();
        assert_eq!(error.code(), "TDB_CAPACITY_RESERVATION_REJECTED");
        assert_eq!(error.details()["memory_limit"], 2 * MB);
        assert_eq!(
            serde_json::to_value(error.info()).unwrap()["target"],
            json!({"field":"additional"})
        );
        assert_eq!(
            maintenance::facts(&db),
            maintenance::expected(false, json!([]))
        );
        blocker.close().unwrap();
        db.set_memory_limit(4 * MB).unwrap();
        assert_eq!(runtime.stats().reserved_database_bytes, 4 * MB);
        db.close().unwrap();
        let error = db.set_memory_limit(0).unwrap_err();
        assert_eq!(error.code(), "TDB_DATABASE_CLOSED");
        assert_eq!(
            serde_json::to_value(error.info()).unwrap()["write"],
            json!({"phase":"admission","outcome":"not_committed"})
        );
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
        for access_mode in [AccessMode::ReadOnly, AccessMode::Immutable] {
            let mut reader = Database::<f32>::open_with_runtime(
                &path,
                Config {
                    access_mode,
                    ..config
                },
                runtime.clone(),
            )
            .unwrap();
            reader.set_memory_limit(MB).unwrap();
            assert_eq!(runtime.stats().reserved_database_bytes, MB);
            assert_eq!(
                maintenance::facts(&reader),
                maintenance::expected(false, json!([]))
            );
            reader.close().unwrap();
            assert_eq!(files(), before);
        }
        assert_eq!(runtime.stats().attached_databases, 0);
    }
}

#[test]
fn 既有构建句柄在配额变更后使用当前预算并支持重试() {
    let root = super::support::TestDir::new("build-current-quota");
    let path = root.file("db.tdb");
    let mut db = Database::<f32>::open(&path, 3).unwrap();
    // 834 个节点满足低维自动构建门槛；一字节预算明确低于实际构建工作区。
    for id in 1..=834 {
        db.insert_with_id(id, &[1., 0., 0.], json!({"id":id}))
            .unwrap();
    }
    db.flush().unwrap();
    let handle = db.quiver_build_handle();
    db.set_memory_limit(1).unwrap();
    let error = handle.prepare_auto_build().err().unwrap();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        json!({"phase":"validation","outcome":"not_committed"})
    );
    assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
    db.set_memory_limit(0).unwrap();
    assert!(
        handle
            .prepare_auto_build()
            .unwrap()
            .expect("释放预算后必须捕获真实任务")
            .execute()
            .unwrap()
    );
    assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
    db.close().unwrap();
    let mut reopened = Database::<f32>::open(&path, 3).unwrap();
    assert_eq!(
        reopened.all_node_ids().unwrap(),
        (1..=834).collect::<Vec<_>>()
    );
    for id in 1..=834 {
        let node = reopened.get(id).unwrap().unwrap();
        assert_eq!(node.vector, [1., 0., 0.]);
        assert_eq!(node.payload, json!({"id":id}));
        assert!(node.edges.is_empty());
    }
    reopened.close().unwrap();
}
