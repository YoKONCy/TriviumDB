//! 用发布版及冻结前源码生成的独立制品核对文档结构升级与确认语义。
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};
use triviumdb::database::{Config, StorageMode};
use triviumdb::{ConsoleSession, Database};
use triviumdb_migrate::Options;

fn files(path: &Path) -> BTreeMap<std::ffi::OsString, Vec<u8>> {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect()
}

#[test]
fn schema_one_migrates_to_current_without_changing_document_or_receipt_identity() {
    check_fixture("document_schema1", 13);
}

#[test]
fn format_fourteen_container_migrates_with_document_identity_and_confirmed_receipt() {
    check_fixture("pre_externalization_format14", 14);
}

fn check_fixture(family: &str, source_format: u16) {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(family);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(fixtures.join("manifest.json")).unwrap()).unwrap();
    for storage in ["rom", "mmap"] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        for entry in std::fs::read_dir(fixtures.join(storage)).unwrap() {
            let entry = entry.unwrap();
            let bytes = std::fs::read(entry.path()).unwrap();
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                manifest["files"][format!("{storage}/{}", entry.file_name().to_str().unwrap())]["sha256"]
            );
            std::fs::write(source.join(entry.file_name()), bytes).unwrap();
        }
        std::fs::write(source.join("sample.tdb.lock"), []).unwrap();
        let before = files(&source);
        let mut expected: Value =
            serde_json::from_slice(&std::fs::read(source.join("expected.json")).unwrap()).unwrap();
        assert_eq!(expected["snapshot"]["schema_version"], 1);
        expected["snapshot"]["schema_version"] = json!(2);
        expected["root"]["_tdb_document"]["schema_version"] = json!(2);
        expected["fragment"]["_tdb_document"]["schema_version"] = json!(2);
        let options = Options {
            source: source.join("sample.tdb"),
            source_dtype: None,
            destination: root.path().join("target"),
            check: false,
            mmap: storage == "mmap",
            memory_bytes: 128 * 1024 * 1024,
            max_source_bytes: 1024 * 1024,
            max_work_bytes: 2048 * 1024 * 1024,
        };
        let report = triviumdb_migrate::run(&options).unwrap();
        assert_eq!(report.source_format, source_format);
        assert_eq!(report.target_format, 15);
        let retry = triviumdb_migrate::run(&options).unwrap();
        assert_eq!(retry.facts, report.facts);
        assert!(retry.reused);
        for _ in 0..2 {
            let mut db = Database::<f32>::open_with_config(
                report.database.to_str().unwrap(),
                Config {
                    dim: 2,
                    load_text_index: true,
                    auto_build_quiver: false,
                    storage_mode: if storage == "mmap" {
                        StorageMode::Mmap
                    } else {
                        StorageMode::Rom
                    },
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(db.all_node_ids().unwrap(), [7, 8, 9]);
            assert_eq!(
                serde_json::to_value(db.get_document(7).unwrap().unwrap()).unwrap(),
                expected["snapshot"]
            );
            assert_eq!(db.get_payload(7).unwrap().unwrap(), expected["root"]);
            assert_eq!(db.get_payload(8).unwrap().unwrap(), expected["fragment"]);
            assert_eq!(
                serde_json::to_value(db.get_edges(7).unwrap()).unwrap(),
                expected["edges"]
            );
            assert_eq!(db.get(7).unwrap().unwrap().vector, [1., 0.]);
            assert_eq!(db.get(8).unwrap().unwrap().vector, [0., 1.]);
            for (query, id) in [
                ("TEXT AC \"结构锚点\" TOP 10 AS n RETURN n", 8),
                ("TEXT BM25 \"alpha\" TOP 10 AS n RETURN n", 7),
            ] {
                assert_eq!(
                    db.tql_nodes(query)
                        .unwrap()
                        .iter()
                        .map(|row| row["n"].id)
                        .collect::<Vec<_>>(),
                    [id]
                );
            }
            let session = ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
            let response: Value = serde_json::from_str(
                &db.console_request(
                    &session,
                    &json!({
                        "version":1,"requestId":"retry","command":expected["command"]
                    })
                    .to_string(),
                )
                .unwrap(),
            )
            .unwrap();
            let mut acknowledgement = expected["acknowledged"].clone();
            acknowledgement["replayed"] = json!(true);
            assert_eq!(response["data"], acknowledgement);
            assert_eq!(db.get_payload(9).unwrap(), Some(json!({"state":"after"})));
            assert_eq!(
                db.insert(&[1., 0.], json!({"code":"root"}))
                    .unwrap_err()
                    .code(),
                "TDB_UNIQUE_CONSTRAINT"
            );
            db.close().unwrap();
        }
        assert_eq!(files(&source), before);
    }
}
