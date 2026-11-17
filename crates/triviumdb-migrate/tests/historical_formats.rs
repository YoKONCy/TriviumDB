//! 用历史原生制品保存的格式 11/12 样本验证外部迁移与后续写入。

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use triviumdb::database::{AccessMode, Config, StorageMode};
use triviumdb::{ConsoleSession, Database, VectorType};
use triviumdb_migrate::Options;

fn snapshot(directory: &Path) -> BTreeMap<std::ffi::OsString, Vec<u8>> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect()
}

fn fixture(format: u16, case: &str, directory: &Path) -> PathBuf {
    let family = match format {
        11 => "pre_named_graph_format11",
        12 => "pre_document_publication_format12",
        _ => panic!("测试要求明确的历史格式"),
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(family);
    let provenance: Value =
        serde_json::from_slice(&std::fs::read(root.join("provenance.json")).unwrap()).unwrap();
    let prefix = format!("{case}/");
    let mut expected = BTreeMap::new();
    std::fs::create_dir(directory).unwrap();
    for record in provenance["files"].as_array().unwrap() {
        let relative = record["path"].as_str().unwrap();
        if let Some(name) = relative.strip_prefix(&prefix) {
            let content = std::fs::read(root.join(relative)).unwrap();
            assert_eq!(content.len() as u64, record["bytes"].as_u64().unwrap());
            assert_eq!(format!("{:x}", Sha256::digest(&content)), record["sha256"]);
            std::fs::write(directory.join(name), &content).unwrap();
            expected.insert(std::ffi::OsString::from(name), content);
        }
    }
    assert_eq!(snapshot(&root.join(case)), expected);
    let source = directory.join("sample.tdb");
    assert_eq!(
        &std::fs::read(&source).unwrap()[4..6],
        &format.to_le_bytes()
    );
    assert_eq!(
        &std::fs::read(directory.join("sample.tdb.wal")).unwrap()[4..6],
        &(format - 7).to_le_bytes()
    );
    std::fs::write(directory.join("sample.tdb.lock"), []).unwrap();
    source
}

fn options(source: PathBuf, destination: PathBuf, mmap: bool) -> Options {
    Options {
        source_dtype: None,
        source,
        destination,
        check: true,
        memory_bytes: 64 * 1024 * 1024,
        max_source_bytes: 1024 * 1024,
        max_work_bytes: 1024 * 1024 * 1024,
        mmap,
    }
}

fn check_nodes_and_graph<T: VectorType + Serialize + DeserializeOwned>(
    db: &Database<T>,
    format: u16,
    case: &Value,
) {
    for (id, coordinates) in [(11, [1., 0.]), (22, [0., 1.]), (33, [1., 1.])] {
        let node = db.get(id).unwrap().unwrap();
        let expected = coordinates.map(T::from_f32);
        assert_eq!(
            bytemuck::cast_slice::<T, u8>(&node.vector),
            bytemuck::cast_slice::<T, u8>(&expected)
        );
        let payload = match (format, id) {
            (11, 11) => json!({"name":"共享节点"}),
            (11, 22) => json!({"name":"target"}),
            (11, 33) => json!({"confirmed":"wal-v4"}),
            (12, _) => json!({"id":id}),
            _ => panic!("历史节点预期必须明确"),
        };
        assert_eq!(node.payload, payload);
        let edges = serde_json::to_value(db.get_edges(id).unwrap()).unwrap();
        let expected_edges = if format == 11 && id == 11 {
            json!([{"target_id":22,"label":"same","weight":0.5,"metadata":null}])
        } else {
            json!([])
        };
        assert_eq!(edges, expected_edges);
    }
    let graphs = serde_json::to_value(db.list_named_graphs(&Default::default()).unwrap()).unwrap();
    if format == 12 {
        assert_eq!(
            graphs,
            json!([{
                "graph_id":"历史命名图", "revision":case["after"]["revision"],
                "member_count":3, "edge_count":1
            }])
        );
        assert_eq!(
            serde_json::to_value(
                db.get_named_graph("历史命名图", &Default::default())
                    .unwrap()
            )
            .unwrap(),
            case["after"]
        );
    } else {
        assert_eq!(graphs, json!([]));
    }
    assert!(db.list_document_uploads(None).unwrap().is_empty());
}

fn migration_matrix<T: VectorType + Serialize + DeserializeOwned>(dtype: &str, format: u16) {
    for mmap in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("source");
        let source = fixture(format, dtype, &directory);
        let case = if format == 12 {
            serde_json::from_slice(&std::fs::read(directory.join("case.json")).unwrap()).unwrap()
        } else {
            Value::Null
        };
        let before = snapshot(&directory);
        for access_mode in [
            AccessMode::ReadWrite,
            AccessMode::ReadOnly,
            AccessMode::Immutable,
        ] {
            let error = Database::<T>::open_with_config(
                source.to_str().unwrap(),
                Config {
                    dim: 2,
                    access_mode,
                    ..Default::default()
                },
            )
            .err()
            .expect("历史数据库应交给外部迁移器");
            assert_eq!(error.code(), "TDB_MIGRATION_REQUIRED");
            assert_eq!(
                error.details(),
                json!({
                    "component":"database", "found":format, "current":15,
                    "migration_entrypoint":"scripts/migrate.py"
                })
            );
            assert_eq!(snapshot(&directory), before);
        }
        let mut opts = options(source, root.path().join("target"), mmap);
        let checked = triviumdb_migrate::run(&opts).unwrap();
        assert!(checked.checked && !checked.committed);
        assert!(!opts.destination.exists());
        assert_eq!(snapshot(&directory), before);
        opts.check = false;
        let report = triviumdb_migrate::run(&opts).unwrap();
        assert_eq!((report.source_format, report.target_format), (format, 15));
        assert_eq!(
            (report.facts.nodes, report.facts.edges, report.facts.next_id),
            (3, usize::from(format == 11), 34)
        );
        assert_eq!(report.facts, checked.facts);
        let retried = triviumdb_migrate::run(&opts).unwrap();
        assert!(retried.committed && retried.reused);
        assert_eq!(retried.facts, report.facts);
        let target_config = Config {
            dim: 2,
            storage_mode: if mmap {
                StorageMode::Mmap
            } else {
                StorageMode::Rom
            },
            auto_build_quiver: false,
            ..Default::default()
        };
        let target_before = snapshot(&opts.destination);
        for access_mode in [AccessMode::ReadOnly, AccessMode::ReadWrite] {
            let mut db = Database::<T>::open_with_config(
                report.database.to_str().unwrap(),
                Config {
                    access_mode,
                    ..target_config
                },
            )
            .unwrap();
            assert_eq!(db.all_node_ids().unwrap(), [11, 22, 33]);
            check_nodes_and_graph(&db, format, &case);
            db.close().unwrap();
            if access_mode != AccessMode::ReadWrite {
                assert_eq!(snapshot(&opts.destination), target_before);
            }
        }
        // 新增节点验证恢复后的 ID 水位，唯一约束由真实写入及再次重开核对。
        let mut db =
            Database::<T>::open_with_config(report.database.to_str().unwrap(), target_config)
                .unwrap();
        db.create_unique_index("new_key").unwrap();
        let vector = [T::zero(), T::from_f32(1.)];
        assert_eq!(db.insert(&vector, json!({"new_key":"after"})).unwrap(), 34);
        db.close().unwrap();
        let mut db =
            Database::<T>::open_with_config(report.database.to_str().unwrap(), target_config)
                .unwrap();
        assert_eq!(db.all_node_ids().unwrap(), [11, 22, 33, 34]);
        check_nodes_and_graph(&db, format, &case);
        let node = db.get(34).unwrap().unwrap();
        assert_eq!(node.vector, vector);
        assert_eq!(node.payload, json!({"new_key":"after"}));
        assert_eq!(
            db.insert(&vector, json!({"new_key":"after"}))
                .unwrap_err()
                .code(),
            "TDB_UNIQUE_CONSTRAINT"
        );
        db.close().unwrap();
        assert_eq!(snapshot(&directory), before);
    }
}

#[test]
fn format_11_confirmed_wal_and_default_edges_survive_migration() {
    migration_matrix::<f32>("f32", 11);
    migration_matrix::<half::f16>("f16", 11);
    migration_matrix::<u64>("u64", 11);
}

#[test]
fn format_12_named_graph_checkpoint_and_wal_survive_migration() {
    migration_matrix::<f32>("f32", 12);
    migration_matrix::<half::f16>("f16", 12);
    migration_matrix::<u64>("u64", 12);
}

#[test]
fn format_11_edit_receipt_retains_acknowledgement_across_migration_and_reopen() {
    for mmap in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("source");
        let source = fixture(11, "receipts", &directory);
        let case: Value =
            serde_json::from_slice(&std::fs::read(directory.join("case.json")).unwrap()).unwrap();
        let before = snapshot(&directory);
        let mut opts = options(source, root.path().join("target"), mmap);
        opts.check = false;
        let report = triviumdb_migrate::run(&opts).unwrap();
        assert_eq!(
            (report.facts.nodes, report.facts.edges, report.facts.next_id),
            (2, 1, 3)
        );
        let target_config = Config {
            dim: 2,
            storage_mode: if mmap {
                StorageMode::Mmap
            } else {
                StorageMode::Rom
            },
            auto_build_quiver: false,
            ..Default::default()
        };
        for _ in 0..2 {
            let mut db =
                Database::<f32>::open_with_config(report.database.to_str().unwrap(), target_config)
                    .unwrap();
            let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
            let mut command = case["command"].clone();
            command["operation"] = json!("transactionStatus");
            let response: Value = serde_json::from_str(
                &db.console_request(
                    &session,
                    &json!({
                        "version":1,"requestId":"history","command":command
                    })
                    .to_string(),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(
                response["data"],
                json!({"outcome":"committed", "createdIds":["2"], "revision":case["acknowledged"]["revision"]})
            );
            let replay: Value = serde_json::from_str(
                &db.console_request(
                    &session,
                    &json!({
                        "version":1, "requestId":"retry", "command":case["command"]
                    })
                    .to_string(),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(
                replay["data"],
                json!({
                    "createdIds":["2"], "revision":case["acknowledged"]["revision"], "replayed":true
                })
            );
            assert_eq!(db.all_node_ids().unwrap(), [1, 2]);
            for (id, vector, payload) in [
                (1, [1., 0.], json!({"state":"after"})),
                (2, [0., 1.], json!({"state":"created"})),
            ] {
                let node = db.get(id).unwrap().unwrap();
                assert_eq!(node.vector, vector);
                assert_eq!(node.payload, payload);
            }
            assert_eq!(
                serde_json::to_value(db.get_edges(1).unwrap()).unwrap(),
                json!([{
                    "target_id":2,"label":"rel","weight":0.5,"metadata":{"source":"旧回执"}
                }])
            );
            assert!(db.get_edges(2).unwrap().is_empty());
            db.close().unwrap();
        }
        assert_eq!(snapshot(&directory), before);
    }
}
