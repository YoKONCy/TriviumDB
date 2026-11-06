//! Rust/Python/Node 共用的资源配额与关闭回收契约。

use serde_json::{Value, json};
use triviumdb::database::Config;
use triviumdb::{
    Database, ResourceLimits, ResourceRuntime, ResourceStats, TriviumError, VectorType,
};

fn check(stats: ResourceStats, expected: &Value) {
    let actual = serde_json::to_value(stats).unwrap();
    for (field, value) in expected.as_object().unwrap() {
        assert_eq!(&actual[field], value, "{field}");
    }
    assert_eq!(actual["active_tasks"], 0);
    assert_eq!(actual["reserved_workspace_bytes"], 0);
    assert!(actual["allocated_worker_threads"].as_u64().unwrap() <= 2);
    assert!(actual["peak_worker_threads"].as_u64().unwrap() <= 2);
}

fn check_instance<T: VectorType>(db: &Database<T>, expected: &Value) {
    let stats = serde_json::to_value(db.instance_stats()).unwrap();
    assert_eq!(stats["memory_limit_bytes"], expected["memory_limit_bytes"]);
    assert_eq!(
        stats["memory"]["reserved_database_bytes"],
        expected["reserved_database_bytes"]
    );
    assert_eq!(stats["memory"]["node_count"], expected["node_count"]);
    assert_eq!(stats["shutdown"]["phase"], expected["phase"]);
    assert_eq!(stats["shutdown"]["active_operations"], 0);
    assert_eq!(stats["shutdown"]["last_failure"], Value::Null);
    assert!(std::path::Path::new(stats["database_path"].as_str().unwrap()).is_absolute());
}

fn check_budget(error: TriviumError, expected: &Value) {
    let TriviumError::ResourceBudgetExceeded {
        resource,
        requested,
        used,
        limit,
    } = error.underlying()
    else {
        panic!("应返回预算拒绝：{error}");
    };
    assert_eq!(
        json!({"resource":resource,"requested":requested,"used":used,"limit":limit}),
        *expected
    );
}

fn run<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(case: &Value, dtype: &str) {
    let dir = std::env::temp_dir().join(format!(
        "triviumdb_resource_contract_{}_{dtype}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = |name: &str| {
        dir.join(format!("{name}.tdb"))
            .to_string_lossy()
            .into_owned()
    };
    let mb = 1024 * 1024;
    let runtime = ResourceRuntime::new(ResourceLimits {
        maintenance_memory_limit: case["runtime"]["maintenance_memory_limit_mb"]
            .as_u64()
            .unwrap() as usize
            * mb,
        memory_limit: case["runtime"]["memory_limit_mb"].as_u64().unwrap() as usize * mb,
        max_threads: case["runtime"]["max_threads"].as_u64().unwrap() as usize,
        max_active_tasks: case["runtime"]["max_active_tasks"].as_u64().unwrap() as usize,
    })
    .unwrap();
    let config = |memory: &Value| Config {
        dim: case["database"]["dim"].as_u64().unwrap() as usize,
        memory_limit: memory.as_u64().unwrap() as usize * mb,
        auto_build_quiver: false,
        ..Default::default()
    };
    let vector = case["vector"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| T::from_f32(value.as_f64().unwrap() as f32))
        .collect::<Vec<_>>();
    let mut first = Database::<T>::open_with_runtime(
        &path("first"),
        config(&case["database"]["memory_limit_mb"]),
        runtime.clone(),
    )
    .unwrap();
    let mut second = Database::<T>::open_with_runtime(
        &path("second"),
        config(&case["database"]["memory_limit_mb"]),
        runtime.clone(),
    )
    .unwrap();
    check(runtime.stats(), &case["expected"]["opened"]);
    check_instance(&first, &case["instances"]["opened"]);
    check_instance(&second, &case["instances"]["opened"]);
    let error = match Database::<T>::open_with_runtime(
        &path("rejected"),
        config(&case["overbook_mb"]),
        runtime.clone(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("超额打开必须失败"),
    };
    check_budget(error, &case["budget_errors"]["open"]);
    assert!(!std::path::Path::new(&path("rejected")).exists());
    assert!(!std::path::Path::new(&format!("{}.lock", path("rejected"))).exists());
    check(runtime.stats(), &case["expected"]["opened"]);
    first
        .set_memory_limit(case["resize_mb"].as_u64().unwrap() as usize * mb)
        .unwrap();
    check(runtime.stats(), &case["expected"]["resized"]);
    check_instance(&first, &case["instances"]["resized"]);
    check_budget(
        second
            .set_memory_limit(case["failed_resize_mb"].as_u64().unwrap() as usize * mb)
            .unwrap_err(),
        &case["budget_errors"]["resize"],
    );
    check_instance(&second, &case["instances"]["opened"]);
    check(runtime.stats(), &case["expected"]["resized"]);
    first
        .set_memory_limit(case["database"]["memory_limit_mb"].as_u64().unwrap() as usize * mb)
        .unwrap();
    first
        .insert_with_id(101, &vector, json!({"owner": "first"}))
        .unwrap();
    second
        .insert_with_id(202, &vector, json!({"owner": "second"}))
        .unwrap();
    assert_eq!(first.search_exact(&vector, 1).unwrap()[0].id, 101);
    assert_eq!(second.search_exact(&vector, 1).unwrap()[0].id, 202);
    first
        .enable_auto_compaction(std::time::Duration::from_secs(3600))
        .unwrap();
    assert_eq!(
        first.instance_stats().compaction.unwrap().phase,
        triviumdb::MaintenancePhase::Scheduled
    );
    first.disable_auto_compaction();
    assert!(first.instance_stats().compaction.is_none());
    first.close().unwrap();
    check_instance(&first, &case["instances"]["closed"]);
    check(runtime.stats(), &case["expected"]["closed_one"]);
    assert!(matches!(
        first.insert(&vector, json!({})).unwrap_err().underlying(),
        TriviumError::DatabaseClosed
    ));
    let mut replacement = Database::<T>::open_with_runtime(
        &path("replacement"),
        config(&case["replacement_mb"]),
        runtime.clone(),
    )
    .unwrap();
    check(runtime.stats(), &case["expected"]["replaced"]);
    second.close().unwrap();
    replacement.close().unwrap();
    check(runtime.stats(), &case["expected"]["closed_all"]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn 共享资源json契约覆盖三种向量后端() {
    let case: Value = serde_json::from_str(include_str!("resource_cases.json")).unwrap();
    assert_eq!(case["schema_version"], 1);
    run::<f32>(&case, "f32");
    run::<half::f16>(&case, "f16");
    run::<u64>(&case, "u64");
}
