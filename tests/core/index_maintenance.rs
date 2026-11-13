//! 维护准入和构建拒绝保持完整事实，文本与属性候选分别按完成点持久化。

#[path = "../support/index_maintenance.rs"]
pub(super) mod maintenance;
use serde_json::json;
use triviumdb::database::{AccessMode, Config, StorageMode};
use triviumdb::{Database, VectorType};

fn matrix<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(dtype: &str) {
    let root = super::support::TestDir::new(dtype);
    let contract = maintenance::contract();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let config = Config {
            dim: 2,
            storage_mode: mode,
            load_text_index: true,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<T>::open_with_config(&path, config).unwrap();
        maintenance::seed(&mut db);
        let before_wal = std::fs::read(format!("{path}.wal")).unwrap();
        for case in contract["validation"].as_array().unwrap() {
            let error = maintenance::execute(&mut db, case).unwrap_err();
            assert_eq!(error.code(), case["code"].as_str().unwrap(), "{case}");
            assert_eq!(
                serde_json::to_value(error.info()).unwrap()["target"],
                case["target"]
            );
            if case["reason"] == true {
                assert!(
                    error.details()["reason"]
                        .as_str()
                        .is_some_and(|reason| !reason.is_empty())
                );
                assert_eq!(error.details().as_object().unwrap().len(), 1);
            } else {
                assert_eq!(error.details(), case["details"]);
            }
            assert_eq!(
                serde_json::to_value(error.info()).unwrap()["write"],
                contract["validation_write"]
            );
            assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before_wal);
            assert_eq!(
                maintenance::facts(&db),
                maintenance::expected(false, json!([]))
            );
        }
        // 1 KiB 余量足以保留当前事实，长正文和关键词的构建峰值需要额外准入。
        db.set_memory_limit(db.estimated_memory() + 1024).unwrap();
        for api in ["index_text", "index_keyword"] {
            let case = json!({"api":api,"id":1,"text":"x".repeat(4096),"keyword":"x".repeat(4096)});
            let error = maintenance::execute(&mut db, &case).unwrap_err();
            assert_eq!(error.code(), "TDB_CAPACITY_RESERVATION_REJECTED");
            assert_eq!(
                serde_json::to_value(error.info()).unwrap()["write"],
                contract["validation_write"]
            );
        }
        db.set_memory_limit(0).unwrap();
        assert_eq!(
            maintenance::facts(&db),
            maintenance::expected(false, json!([]))
        );
        db.close().unwrap();
        for case in contract["operations"].as_array().unwrap() {
            let error = maintenance::execute(&mut db, case).unwrap_err();
            assert_eq!(error.code(), "TDB_DATABASE_CLOSED", "{case}");
            assert_eq!(
                serde_json::to_value(error.info()).unwrap()["write"],
                contract["admission"]
            );
        }
        let file_group = || {
            std::fs::read_dir(&root.0)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    (entry.file_name(), std::fs::read(entry.path()).unwrap())
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let before_files = file_group();
        for access in [AccessMode::ReadOnly, AccessMode::Immutable] {
            let mut reader = Database::<T>::open_with_config(
                &path,
                Config {
                    access_mode: access,
                    ..config
                },
            )
            .unwrap();
            for case in contract["operations"].as_array().unwrap() {
                let error = maintenance::execute(&mut reader, case).unwrap_err();
                assert_eq!(error.code(), "TDB_READ_ONLY", "{case}/{access:?}");
                assert_eq!(error.details(), json!({"operation":case["api"]}));
                assert_eq!(
                    serde_json::to_value(error.info()).unwrap()["write"],
                    contract["admission"]
                );
            }
            assert_eq!(
                maintenance::facts(&reader),
                maintenance::expected(false, json!([]))
            );
            reader.close().unwrap();
            assert_eq!(file_group(), before_files);
        }
        let mut db = Database::<T>::open_with_config(&path, config).unwrap();
        maintenance::execute(&mut db, &json!({"api":"rebuild_text_index"})).unwrap();
        db.create_unique_index("tag").unwrap();
        let indexes = json!([{"kind":"Hash","fields":["tag"],"unique":true}]);
        assert_eq!(
            maintenance::facts(&db),
            maintenance::expected(true, indexes.clone())
        );
        db.close().unwrap();
        let mut reopened = Database::<T>::open_with_config(&path, config).unwrap();
        assert_eq!(
            maintenance::facts(&reopened),
            maintenance::expected(true, indexes)
        );
        reopened.close().unwrap();
    }
}

#[test]
fn maintenance_admission_f32() {
    matrix::<f32>("maintenance-f32");
}
#[test]
fn maintenance_admission_f16() {
    matrix::<half::f16>("maintenance-f16");
}
#[test]
fn maintenance_admission_u64() {
    matrix::<u64>("maintenance-u64");
}

#[test]
fn generation_id拒绝先于源检查点且保留完整事实() {
    let root = super::support::TestDir::new("generation-validation-checkpoint");
    let path = root.file("db.tdb");
    let config = Config {
        dim: 2,
        load_text_index: true,
        auto_build_quiver: false,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    maintenance::seed(&mut db);
    db.update_payload(1, maintenance::contract()["nodes"][0]["payload"].clone())
        .unwrap();
    let source_files = || {
        ["", ".wal", ".flush_ok"].map(|suffix| std::fs::read(format!("{path}{suffix}")).unwrap())
    };
    let before = source_files();
    let destination = root.file("snapshot");
    for generation in ["".to_owned(), " ".to_owned(), "学".repeat(86)] {
        for error in [
            db.create_snapshot(&destination, &generation).unwrap_err(),
            db.publish_generation_manifest(&generation).unwrap_err(),
        ] {
            assert_eq!(error.code(), "TDB_INVALID_INPUT");
            assert_eq!(
                serde_json::to_value(error.info()).unwrap()["write"],
                serde_json::json!({"phase":"validation","outcome":"not_committed"})
            );
        }
        assert_eq!(source_files(), before);
        assert!(!std::path::Path::new(&destination).exists());
        assert_eq!(
            maintenance::facts(&db),
            maintenance::expected(false, json!([]))
        );
    }
    db.close().unwrap();
    let mut reopened = Database::<f32>::open_with_config(&path, config).unwrap();
    assert_eq!(
        maintenance::facts(&reopened),
        maintenance::expected(false, json!([]))
    );
    reopened.close().unwrap();
}

#[test]
fn text_workspace_rejection_preserves_facts_and_retries_after_quota_release() {
    use triviumdb::resources::{ResourceLimits, ResourceRuntime};
    let root = super::support::TestDir::new("text-workspace-quota");
    let path = root.file("db.tdb");
    let config = Config {
        dim: 2,
        load_text_index: true,
        auto_build_quiver: false,
        ..Default::default()
    };
    let mut seed = Database::<f32>::open_with_config(&path, config).unwrap();
    maintenance::seed(&mut seed);
    seed.close().unwrap();
    // 预先发布空库，使占位实例关闭时只释放配额，避免额外的检查点工作区干扰重试条件。
    let blocker_path = root.file("blocker.tdb");
    Database::<f32>::open_with_config(&blocker_path, config)
        .unwrap()
        .close()
        .unwrap();
    // 总额 2 MiB 保护 256 KiB 维护空间，两个实例声明 1 MiB 与 512 KiB，前台余量为 256 KiB。
    let runtime = ResourceRuntime::new(ResourceLimits {
        memory_limit: 2 * 1024 * 1024,
        maintenance_memory_limit: 256 * 1024,
        ..Default::default()
    })
    .unwrap();
    let mut db = Database::<f32>::open_with_runtime(
        &path,
        Config {
            memory_limit: 1024 * 1024,
            ..config
        },
        runtime.clone(),
    )
    .unwrap();
    let mut blocker = Database::<f32>::open_with_runtime(
        &blocker_path,
        Config {
            memory_limit: 512 * 1024,
            ..config
        },
        runtime.clone(),
    )
    .unwrap();
    assert_eq!(runtime.stats().available_foreground_bytes, Some(256 * 1024));
    let text = "x".repeat(4096);
    let error = db.index_text(1, &text).unwrap_err();
    assert_eq!(error.code(), "QueueFull");
    assert_eq!(
        error.details(),
        json!({"resource":"memory_bytes","requested":262656,"used":1572864,"limit":1835008})
    );
    assert_eq!(
        serde_json::to_value(error.info()).unwrap()["write"],
        maintenance::contract()["validation_write"]
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert_eq!(
        maintenance::facts(&db),
        maintenance::expected(false, json!([]))
    );
    blocker.close().unwrap();
    db.index_text(1, &text).unwrap();
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    let rows = db
        .tql_values(&format!(
            "TEXT BM25 \"{text}\" TOP 10 AS hits WITH hits RETURN hits"
        ))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["hits"].as_node().unwrap().id, 1);
    db.index_text(1, "oldalpha").unwrap();
    db.set_memory_limit(db.estimated_memory() + 1024).unwrap();
    db.close().unwrap();
    let mut reopened = Database::<f32>::open_with_config(&path, config).unwrap();
    assert_eq!(
        maintenance::facts(&reopened),
        maintenance::expected(false, json!([]))
    );
    reopened.close().unwrap();
}
