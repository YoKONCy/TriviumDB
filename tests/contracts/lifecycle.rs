//! 三端共享的快照隔离、恢复和可观测关闭契约。

use serde_json::Value;
use triviumdb::database::{AccessMode, Config};
use triviumdb::storage::snapshot::{restore_snapshot_with_runtime, verify_snapshot_with_runtime};
use triviumdb::{
    Database, ResourceLimits, ResourceRuntime, ShutdownPhase, TriviumError, VectorType,
};

fn run<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(case: &Value, dtype: &str) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "tdb_lifecycle_contract_{}_{nonce}_{dtype}",
        std::process::id()
    ));
    std::fs::create_dir(&root).unwrap();
    let runtime = ResourceRuntime::new(ResourceLimits {
        memory_limit: 64 * 1024 * 1024,
        maintenance_memory_limit: 8 * 1024 * 1024,
        max_threads: 2,
        max_active_tasks: 1,
    })
    .unwrap();
    let config = Config {
        dim: case["dimension"].as_u64().unwrap() as usize,
        memory_limit: 8 * 1024 * 1024,
        auto_build_quiver: false,
        ..Default::default()
    };
    let mut db = Database::<T>::open_with_runtime(
        &root.join("live.tdb").to_string_lossy(),
        config,
        runtime.clone(),
    )
    .unwrap();
    let vector = case["vector"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| T::from_f32(v.as_f64().unwrap() as f32))
        .collect::<Vec<_>>();
    let id = case["node_id"].as_u64().unwrap();
    db.insert_with_id(id, &vector, case["original_payload"].clone())
        .unwrap();
    let snapshot = root.join("snapshot");
    let generation = case["generation_id"].as_str().unwrap();
    let manifest = db
        .create_snapshot(&snapshot.to_string_lossy(), generation)
        .unwrap();
    assert_eq!(manifest.node_count, 1);
    assert!(manifest.complete);
    db.update_payload(id, case["later_payload"].clone())
        .unwrap();
    db.insert_with_id(
        case["later_node_id"].as_u64().unwrap(),
        &vector,
        serde_json::json!({}),
    )
    .unwrap();
    let error = db
        .create_snapshot(&snapshot.to_string_lossy(), generation)
        .unwrap_err();
    assert!(matches!(
        error.underlying(),
        TriviumError::SnapshotDestinationExists { .. }
    ));
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        serde_json::json!({"phase":"validation","outcome":"not_committed"})
    );
    assert_eq!(
        verify_snapshot_with_runtime(&snapshot.to_string_lossy(), &runtime)
            .unwrap()
            .node_count,
        1
    );
    let restored = root.join("restored");
    let manifest = restore_snapshot_with_runtime(
        &snapshot.to_string_lossy(),
        &restored.to_string_lossy(),
        &runtime,
    )
    .unwrap();
    assert_eq!(manifest.generation_id, generation);
    let error = restore_snapshot_with_runtime(
        &snapshot.to_string_lossy(),
        &restored.to_string_lossy(),
        &runtime,
    )
    .unwrap_err();
    assert!(matches!(
        error.underlying(),
        TriviumError::SnapshotDestinationExists { .. }
    ));
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        serde_json::json!({"phase":"validation","outcome":"not_committed"})
    );
    let mut reader = Database::<T>::open_with_runtime(
        &restored
            .join(case["database_file"].as_str().unwrap())
            .to_string_lossy(),
        Config {
            access_mode: AccessMode::Immutable,
            ..config
        },
        runtime.clone(),
    )
    .unwrap();
    assert_eq!(
        reader.get_payload(id).unwrap().unwrap(),
        case["original_payload"]
    );
    assert_eq!(reader.all_node_ids().unwrap(), [id]);
    let node = reader.get(id).unwrap().unwrap();
    assert_eq!(node.vector, vector);
    assert!(node.edges.is_empty());
    assert!(
        !reader
            .contains(case["later_node_id"].as_u64().unwrap())
            .unwrap()
    );
    assert!(
        reader
            .close_timeout(std::time::Duration::from_millis(
                case["close_timeout_ms"].as_u64().unwrap()
            ))
            .unwrap()
    );
    verify_snapshot_with_runtime(&restored.to_string_lossy(), &runtime).unwrap();
    db.request_shutdown().unwrap();
    assert!(db.close_timeout(std::time::Duration::from_secs(5)).unwrap());
    assert_eq!(db.shutdown_status().phase, ShutdownPhase::Closed);
    let stats = runtime.stats();
    assert_eq!(stats.attached_databases, 0);
    assert_eq!(stats.active_tasks, 0);
    assert_eq!(stats.active_maintenance_tasks, 0);
    assert_eq!(stats.reserved_maintenance_bytes, 0);
    assert!(stats.peak_reserved_bytes <= stats.memory_limit);
}

#[test]
fn 快照恢复与关闭json契约覆盖三种向量后端() {
    let case: Value = serde_json::from_str(include_str!("lifecycle_cases.json")).unwrap();
    run::<f32>(&case, "f32");
    run::<half::f16>(&case, "f16");
    run::<u64>(&case, "u64");
}
