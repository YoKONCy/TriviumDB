use triviumdb::database::{Config, StorageMode};
use triviumdb::index::quiver::{
    QuIVerSearchConfig,
    ssd::{Residency, SsdRuntimeOptions},
};
use triviumdb::{Database, ResourceLimits, ResourceRuntime};

fn query() -> QuIVerSearchConfig {
    QuIVerSearchConfig {
        top_k: 4,
        ef_search: 4,
        rerank_limit: Some(4),
    }
}
fn options() -> SsdRuntimeOptions {
    SsdRuntimeOptions {
        io_depth: 1,
        cache_bytes: 4096,
        ..Default::default()
    }
}
fn directory() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("tdb-sdk-ssd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    path
}
fn snapshot(root: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().to_str().unwrap().to_owned();
            let bytes = if name.ends_with(".lock") {
                assert_eq!(entry.metadata().unwrap().len(), 0);
                Vec::new()
            } else {
                std::fs::read(entry.path()).unwrap()
            };
            (name, bytes)
        })
        .collect()
}

#[track_caller]
fn assert_hits(actual: Vec<(u64, f32)>, expected: &[(u64, f32)]) {
    assert_eq!(
        actual.iter().map(|hit| hit.0).collect::<Vec<_>>(),
        expected.iter().map(|hit| hit.0).collect::<Vec<_>>()
    );
    for (hit, expected) in actual.iter().zip(expected) {
        assert!(
            (hit.1 - expected.1).abs() < 1e-6,
            "实际{hit:?}，预期{expected:?}"
        );
    }
}

#[test]
fn centered_presets_share_public_precisions_normal_ann_and_readonly_reopen() {
    use triviumdb::index::quiver::ssd::{QueryPrecision, RerankEncoding};
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/quiver_ssd_cases.json")).unwrap();
    for encoding in [RerankEncoding::CenteredInt8, RerankEncoding::CenteredInt4] {
        let root = directory();
        let path = root.join("centered.tdb");
        let dimension = fixture["dimension"].as_u64().unwrap() as usize;
        let mut db = Database::<f32>::open_with_config(
            path.to_str().unwrap(),
            Config {
                dim: dimension,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        for node in fixture["nodes"].as_array().unwrap() {
            let mut vector = vec![0.; dimension];
            vector[dimension - 1] = node["direction"].as_i64().unwrap() as f32;
            db.insert_with_id(
                node["id"].as_str().unwrap().parse().unwrap(),
                &vector,
                serde_json::json!({"keep":true}),
            )
            .unwrap();
        }
        let mut vector = vec![0.; dimension];
        vector[dimension - 1] = 1.;
        let expected = [(9, 1.), (9007199254740993, 1.), (40, 0.), (77, -1.)];
        // 更新会保留旧slot墓碑，完整覆盖这组数据需要候选宽度包含活跃行与墓碑。
        let full = QuIVerSearchConfig {
            ef_search: 32,
            ..query()
        };
        let base = SsdRuntimeOptions {
            encoding,
            io_depth: 1,
            cache_bytes: 0,
            ..SsdRuntimeOptions::accuracy()
        };
        for precision in [
            QueryPrecision::Fp32,
            QueryPrecision::Int8,
            QueryPrecision::Int4,
        ] {
            for residency in [Residency::Ssd, Residency::Hot] {
                let opts = SsdRuntimeOptions {
                    query_precision: precision,
                    residency,
                    ..base
                };
                assert!(db.configure_quiver(opts).unwrap().routing_shared);
                assert_hits(db.search_quiver(&vector, &full).unwrap(), &expected);
                assert_hits(
                    db.search(&vector, 4, 0, -1.)
                        .unwrap()
                        .into_iter()
                        .map(|hit| (hit.id, hit.score))
                        .collect(),
                    &expected,
                );
            }
        }
        db.update_vector(40, &vector).unwrap();
        let changed = [(9, 1.), (40, 1.), (9007199254740993, 1.), (77, -1.)];
        assert_hits(
            db.search(&vector, 4, 0, -1.)
                .unwrap()
                .into_iter()
                .map(|hit| (hit.id, hit.score))
                .collect(),
            &changed,
        );
        db.close().unwrap();
        let before = snapshot(&root);
        let mut reader =
            Database::<f32>::open_read_only(path.to_str().unwrap(), dimension).unwrap();
        assert!(reader.configure_quiver(base).unwrap().routing_shared);
        assert_hits(reader.search_quiver(&vector, &full).unwrap(), &changed);
        let other = if encoding == RerankEncoding::CenteredInt4 {
            RerankEncoding::CenteredInt8
        } else {
            RerankEncoding::CenteredInt4
        };
        assert_eq!(
            reader
                .configure_quiver(SsdRuntimeOptions {
                    encoding: other,
                    ..base
                })
                .unwrap_err()
                .code(),
            "TDB_IMMUTABLE_ARTIFACT"
        );
        assert_hits(reader.search_quiver(&vector, &full).unwrap(), &changed);
        reader.close().unwrap();
        assert_eq!(snapshot(&root), before);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn database_entry_binds_facts_refreshes_updates_and_reopens_read_only() {
    for storage_mode in [StorageMode::Mmap, StorageMode::Rom] {
        let root = directory();
        let path = root.join("sample.tdb");
        let mut db = Database::<f32>::open_with_config(
            path.to_str().unwrap(),
            Config {
                dim: 2,
                storage_mode,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        db.insert_with_id(9007199254740993, &[1., 0.], serde_json::json!({"name":"a"}))
            .unwrap();
        db.insert_with_id(2, &[0., 1.], serde_json::json!({"name":"b"}))
            .unwrap();
        let first = db.configure_quiver_int8(options()).unwrap();
        assert_hits(
            db.search_quiver_int8(&[1., 0.], &query()).unwrap(),
            &[(9007199254740993, 1.), (2, 0.)],
        );
        db.update_vector(9007199254740993, &[-1., 0.]).unwrap();
        assert_hits(
            db.search_quiver_int8(&[1., 0.], &query()).unwrap(),
            &[(2, 0.), (9007199254740993, -1.)],
        );
        assert_ne!(
            first.metadata.source,
            db.quiver_int8_stats().unwrap().unwrap().metadata.source
        );
        db.delete(2).unwrap();
        db.compact().unwrap();
        // 关闭负责checkpoint与派生代际刷新，随后只读重开可以直接使用已发布制品。
        db.close().unwrap();
        let before = snapshot(&root);
        let mut reader = Database::<f32>::open_read_only(path.to_str().unwrap(), 2).unwrap();
        reader.configure_quiver_int8(options()).unwrap();
        assert_hits(
            reader.search_quiver_int8(&[1., 0.], &query()).unwrap(),
            &[(9007199254740993, -1.)],
        );
        reader
            .configure_quiver_int8(SsdRuntimeOptions {
                residency: Residency::Hot,
                ..options()
            })
            .unwrap();
        assert_hits(
            reader.search_quiver_int8(&[1., 0.], &query()).unwrap(),
            &[(9007199254740993, -1.)],
        );
        assert_eq!(
            reader.collect_quiver_temporary().unwrap_err().code(),
            "TDB_READ_ONLY"
        );
        reader.close().unwrap();
        assert_eq!(snapshot(&root), before);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn readonly_missing_artifact_and_invalid_configuration_preserve_files() {
    let root = directory();
    let path = root.join("sample.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(7, &[1., 0.], serde_json::Value::Null)
        .unwrap();
    db.build_quiver_index(None).unwrap();
    db.flush().unwrap();
    let before = snapshot(&root);
    assert_eq!(
        db.configure_quiver_int8(SsdRuntimeOptions {
            io_depth: 0,
            ..options()
        })
        .unwrap_err()
        .code(),
        "TDB_INVALID_INPUT"
    );
    assert_eq!(snapshot(&root), before);
    db.close().unwrap();
    let before = snapshot(&root);
    let mut reader = Database::<f32>::open_read_only(path.to_str().unwrap(), 2).unwrap();
    assert_eq!(
        reader.configure_quiver_int8(options()).unwrap_err().code(),
        "TDB_IO"
    );
    reader.close().unwrap();
    assert_eq!(snapshot(&root), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sdk_protocol_preserves_ids_unknown_fields_and_closed_admission() {
    let root = directory();
    let path = root.join("sample.tdb");
    let runtime = ResourceRuntime::new(ResourceLimits {
        max_threads: 4,
        ..Default::default()
    })
    .unwrap();
    let mut db = Database::<half::f16>::open_with_runtime(
        path.to_str().unwrap(),
        Config {
            dim: 2,
            ..Default::default()
        },
        runtime.clone(),
    )
    .unwrap();
    db.insert_with_id(
        9007199254740993,
        &[half::f16::ONE, half::f16::ZERO],
        serde_json::Value::Null,
    )
    .unwrap();
    let request = |db: &Database<half::f16>, json| triviumdb::sdk::quiver::request(db, json);
    let error = request(
        &db,
        r#"{"operation":"configure","options":{"cacheBytes":1}}"#,
    )
    .unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    request(
        &db,
        r#"{"operation":"configure","options":{"io_depth":1,"cache_bytes":4096}}"#,
    )
    .unwrap();
    let hits: serde_json::Value = serde_json::from_str(
        &request(
            &db,
            r#"{"operation":"search","vector":[1,0],"top_k":1,"ef_search":4}"#,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        hits,
        serde_json::json!({"hits":[{"id":"9007199254740993","score":1.0}]})
    );
    let error = request(
        &db,
        r#"{"operation":"search","vector":[1],"top_k":1,"ef_search":4}"#,
    )
    .unwrap_err();
    assert_eq!(error.code(), "TDB_DIMENSION_MISMATCH");
    db.close().unwrap();
    assert_eq!(
        request(&db, r#"{"operation":"stats"}"#).unwrap_err().code(),
        "TDB_DATABASE_CLOSED"
    );
    assert_eq!(runtime.stats().attached_databases, 0);
    assert_eq!(runtime.stats().reserved_database_bytes, 0);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert_eq!(runtime.stats().active_tasks, 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rust_sdk_uses_the_shared_independent_residency_contract() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/quiver_ssd_cases.json")).unwrap();
    let root = directory();
    let path = root.join("shared.tdb");
    let dim = fixture["dimension"].as_u64().unwrap() as usize;
    let mut db = Database::<f32>::open(path.to_str().unwrap(), dim).unwrap();
    for node in fixture["nodes"].as_array().unwrap() {
        let mut vector = vec![0.; dim];
        vector[dim - 1] = node["direction"].as_i64().unwrap() as f32;
        db.insert_with_id(
            node["id"].as_str().unwrap().parse().unwrap(),
            &vector,
            serde_json::Value::Null,
        )
        .unwrap();
    }
    let command = serde_json::json!({"operation":"configure", "options":fixture["options"]});
    triviumdb::sdk::quiver::request(&db, &command.to_string()).unwrap();
    let mut query = vec![0.; dim];
    query[dim - 1] = 1.;
    let mut search = fixture["search"].clone();
    search["vector"] = serde_json::json!(query);
    let actual: serde_json::Value =
        serde_json::from_str(&triviumdb::sdk::quiver::request(&db, &search.to_string()).unwrap())
            .unwrap();
    assert_eq!(actual, serde_json::json!({"hits":fixture["expected_hits"]}));
    for operation in ["stats", "close", "collect_temporary"] {
        let error = triviumdb::sdk::quiver::request(
            &db,
            &serde_json::json!({"operation":operation, "unexpected":true}).to_string(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT");
    }
    db.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn matching_source_tag_cannot_substitute_a_different_hot_graph() {
    use triviumdb::index::quiver::{
        QuIVer, QuIVerConfig,
        ssd::{SsdOptions, SsdQuiver},
    };
    let root = directory();
    let path = root.join("binding.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(7, &[1., 0.], serde_json::Value::Null)
        .unwrap();
    let metadata = db.configure_quiver_int8(options()).unwrap().metadata;
    db.close().unwrap();
    let mut foreign = QuIVer::new(2, &QuIVerConfig::default());
    foreign.batch_build(&[1., 0.], &[99], &[0]);
    SsdQuiver::publish(
        &foreign,
        &path.with_extension("tdb.qssd"),
        metadata.source,
        SsdOptions::default(),
        &ResourceRuntime::new(ResourceLimits::default()).unwrap(),
        |_, output| {
            output.copy_from_slice(&[1., 0.]);
            Ok(())
        },
    )
    .unwrap();
    let before = snapshot(&root);
    let mut reader = Database::<f32>::open_read_only(path.to_str().unwrap(), 2).unwrap();
    assert_eq!(
        reader.configure_quiver_int8(options()).unwrap_err().code(),
        "TDB_IMMUTABLE_ARTIFACT"
    );
    reader.close().unwrap();
    assert_eq!(snapshot(&root), before);
    let mut writer = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    writer.configure_quiver_int8(options()).unwrap();
    assert_hits(
        writer.search_quiver_int8(&[1., 0.], &query()).unwrap(),
        &[(7, 1.)],
    );
    writer.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn single_thread_runtime_keeps_refresh_and_close_available() {
    let root = directory();
    let path = root.join("one-worker.tdb");
    let runtime = ResourceRuntime::new(ResourceLimits {
        max_threads: 1,
        ..Default::default()
    })
    .unwrap();
    let mut db = Database::<f32>::open_with_runtime(
        path.to_str().unwrap(),
        Config {
            dim: 2,
            ..Default::default()
        },
        runtime.clone(),
    )
    .unwrap();
    db.insert_with_id(7, &[1., 0.], serde_json::Value::Null)
        .unwrap();
    db.insert_with_id(9, &[0., 1.], serde_json::Value::Null)
        .unwrap();
    db.configure_quiver_int8(SsdRuntimeOptions::default())
        .unwrap();
    assert_hits(
        db.search_quiver_int8(&[1., 0.], &query()).unwrap(),
        &[(7, 1.), (9, 0.)],
    );
    db.update_vector(7, &[-1., 0.]).unwrap();
    assert_hits(
        db.search_quiver_int8(&[1., 0.], &query()).unwrap(),
        &[(9, 0.), (7, -1.)],
    );
    db.delete(9).unwrap();
    db.delete(7).unwrap();
    db.flush().unwrap();
    assert!(db.quiver_int8_stats().unwrap().unwrap().closed);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert!(
        db.search_quiver_int8(&[1., 0.], &query())
            .unwrap()
            .is_empty()
    );
    db.insert_with_id(11, &[1., 0.], serde_json::Value::Null)
        .unwrap();
    assert_hits(
        db.search_quiver_int8(&[1., 0.], &query()).unwrap(),
        &[(11, 1.)],
    );
    db.close().unwrap();
    assert!(runtime.stats().allocated_worker_threads <= 1);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn database_budget_covers_base_heap_and_ssd_runtime_together() {
    let root = directory();
    let path = root.join("budget.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.reserve_nodes(16384).unwrap();
    db.insert_with_id(7, &[1., 0.], serde_json::Value::Null)
        .unwrap();
    db.insert_with_id(9, &[0., 1.], serde_json::Value::Null)
        .unwrap();
    db.build_quiver_index(None).unwrap();
    db.flush().unwrap();
    let base = db.estimated_memory();
    assert!(
        base > 512 * 1024,
        "预留容量须构成真实的基础堆占用，实际{base}"
    );
    db.set_memory_limit(base + 128 * 1024).unwrap();
    let error = db.configure_quiver_int8(options()).unwrap_err();
    assert_eq!(error.code(), "QueueFull");
    assert!(db.quiver_int8_stats().unwrap().is_none());
    assert_eq!(db.get(7).unwrap().unwrap().vector, vec![1., 0.]);
    assert_eq!(db.get(9).unwrap().unwrap().vector, vec![0., 1.]);
    db.set_memory_limit(base + 2 * 1024 * 1024).unwrap();
    let stats = db.configure_quiver_int8(options()).unwrap();
    assert!(db.estimated_memory() >= base + stats.reserved_bytes);
    assert_hits(
        db.search_quiver_int8(&[1., 0.], &query()).unwrap(),
        &[(7, 1.), (9, 0.)],
    );
    db.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn readonly_pending_wal_requires_recovery_before_cold_queries() {
    let root = directory();
    let path = root.join("wal-overlay.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(7, &[1., 0.], serde_json::Value::Null)
        .unwrap();
    db.insert_with_id(9, &[0., 1.], serde_json::Value::Null)
        .unwrap();
    db.configure_quiver_int8(options()).unwrap();
    db.update_vector(7, &[-1., 0.]).unwrap();
    drop(db);
    let before = snapshot(&root);
    let error = Database::<f32>::open_read_only(path.to_str().unwrap(), 2)
        .err()
        .unwrap();
    assert_eq!(error.code(), "TDB_RECOVERY_REQUIRED");
    assert_eq!(snapshot(&root), before);
    let mut writer = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    writer.configure_quiver_int8(options()).unwrap();
    assert_hits(
        writer.search_quiver_int8(&[1., 0.], &query()).unwrap(),
        &[(9, 0.), (7, -1.)],
    );
    writer.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn sdk_shared_read_requests_remain_available_during_a_cold_query() {
    use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};
    let root = directory();
    let path = root.join("shared-read.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 1).unwrap();
    db.insert_with_id(7, &[1.], serde_json::Value::Null)
        .unwrap();
    db.insert_with_id(9, &[-1.], serde_json::Value::Null)
        .unwrap();
    let baseline = db.configure_quiver_int8(options()).unwrap().reserved_bytes;
    let shared = triviumdb::SharedDatabase::new(db);
    let gate = pause_at(ConcurrencyPoint::QuiverColdBeforeRead);
    let worker_shared = shared.clone();
    let worker = std::thread::spawn(move || {
        worker_shared.try_read(|core| {
            triviumdb::sdk::quiver::request(
                core,
                r#"{"operation":"search","vector":[1],"top_k":2,"ef_search":4}"#,
            )
        })
    });
    gate.wait_until_arrived();
    let stats: serde_json::Value = serde_json::from_str(
        &shared
            .try_read(|core| triviumdb::sdk::quiver::request(core, r#"{"operation":"stats"}"#))
            .unwrap(),
    )
    .unwrap();
    assert!(stats["reserved_bytes"].as_u64().unwrap() > baseline as u64);
    drop(gate);
    let hits: serde_json::Value = serde_json::from_str(&worker.join().unwrap().unwrap()).unwrap();
    assert_eq!(
        hits,
        serde_json::json!({"hits":[{"id":"7","score":1.0},{"id":"9","score":-1.0}]})
    );
    shared.close().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
