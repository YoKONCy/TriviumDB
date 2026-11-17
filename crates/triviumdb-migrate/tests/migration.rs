//! 真实发布制品、完整事实与受控发布故障的离线迁移验收。
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use triviumdb::{
    Database, VectorType,
    database::{AccessMode, Config, StorageMode},
};
use triviumdb_migrate::{Options, Phase};

fn fixture(dtype: &str, storage: &str, kind: &str, directory: &Path) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/release_090");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let relative = format!("{dtype}/{storage}/{kind}");
    std::fs::create_dir(directory).unwrap();
    for entry in std::fs::read_dir(root.join(&relative)).unwrap() {
        let entry = entry.unwrap();
        let bytes = std::fs::read(entry.path()).unwrap();
        use sha2::Digest;
        assert_eq!(
            format!("{:x}", sha2::Sha256::digest(&bytes)),
            manifest["files"][format!("{relative}/{}", entry.file_name().to_str().unwrap())]["sha256"]
        );
        std::fs::write(directory.join(entry.file_name()), bytes).unwrap();
    }
    std::fs::write(directory.join("sample.tdb.lock"), []).unwrap();
    directory.join("sample.tdb")
}
fn options(source: PathBuf, destination: PathBuf) -> Options {
    Options {
        source_dtype: None,
        source,
        destination,
        check: false,
        memory_bytes: 64 * 1024 * 1024,
        max_source_bytes: 1024 * 1024,
        max_work_bytes: 1024 * 1024 * 1024,
        mmap: true,
    }
}
fn bytes(directory: &Path) -> BTreeMap<std::ffi::OsString, Vec<u8>> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (e.file_name(), std::fs::read(e.path()).unwrap())
        })
        .collect()
}
fn verify<T: VectorType + Serialize + DeserializeOwned>(path: &Path, vectors: [Vec<T>; 2]) {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let mut db = Database::<T>::open_with_config(
            path.to_str().unwrap(),
            Config {
                dim: 2,
                storage_mode: mode,
                auto_build_quiver: false,
                load_text_index: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(db.all_node_ids().unwrap(), [7, 9]);
        for (i, (id, code, rank, text)) in [(7, "alpha", 10, "原始甲"), (9, "beta", 20, "原始乙")]
            .into_iter()
            .enumerate()
        {
            let node = db.get(id).unwrap().unwrap();
            assert_eq!(bytemuck_bytes(&node.vector), bytemuck_bytes(&vectors[i]));
            assert_eq!(node.payload, json!({"code":code,"rank":rank,"text":text}));
        }
        let edges = db.get_edges(7).unwrap();
        assert_eq!(
            serde_json::to_value(edges).unwrap(),
            json!([{"target_id":9,"label":"关系","weight":0.5,"metadata":{"source":"release-0.9.0","rank":7}}])
        );
        for query in [
            "TEXT AC \"独立关键词\" TOP 10 AS n RETURN n",
            "TEXT BM25 \"迁移正文甲\" TOP 10 AS n RETURN n",
        ] {
            assert_eq!(
                db.tql_nodes(query)
                    .unwrap()
                    .iter()
                    .map(|row| row["n"].id)
                    .collect::<Vec<_>>(),
                [7]
            );
        }
        assert!(db.insert(&vectors[0], json!({"code":"alpha"})).is_err());
        let result = db
            .tql("MATCH (n) RETURN n.rank AS rank ORDER BY rank DESC")
            .unwrap();
        let ranks = result
            .iter()
            .map(|row| match row["rank"] {
                triviumdb::query::tql_executor::TqlValue::Int(n) => n,
                _ => panic!("排序字段应是整数"),
            })
            .collect::<Vec<_>>();
        assert_eq!(ranks, [20, 10]);
        db.close().unwrap();
    }
}
fn bytemuck_bytes<T: VectorType>(values: &[T]) -> &[u8] {
    bytemuck::cast_slice(values)
}

#[test]
fn released_facts_survive_conversion_and_reopen() {
    for dtype in ["f32", "f16", "u64"] {
        for storage in ["rom", "mmap"] {
            for kind in ["checkpoint", "wal"] {
                let root = tempfile::tempdir().unwrap();
                let source = fixture(dtype, storage, kind, &root.path().join("source"));
                let before = bytes(source.parent().unwrap());
                let mut opts = options(source.clone(), root.path().join("target"));
                opts.mmap = storage == "rom";
                opts.check = true;
                let checked = triviumdb_migrate::run(&opts).unwrap();
                assert!(checked.checked && !checked.committed);
                assert!(!opts.destination.exists());
                assert_eq!(bytes(source.parent().unwrap()), before);
                opts.check = false;
                let report = triviumdb_migrate::run(&opts).unwrap();
                assert!(report.committed && !report.reused);
                assert_eq!(
                    (report.facts.nodes, report.facts.edges, report.facts.next_id),
                    (2, 1, 10)
                );
                let retried = triviumdb_migrate::run(&opts).unwrap();
                assert!(retried.committed && retried.reused);
                assert_eq!(retried.facts, report.facts);
                match dtype {
                    "f32" => verify(&report.database, [vec![1.0f32, -0.0], vec![0.25, 1.0]]),
                    "f16" => verify(
                        &report.database,
                        [
                            vec![half::f16::ONE, half::f16::NEG_ZERO],
                            vec![half::f16::from_f32(0.25), half::f16::ONE],
                        ],
                    ),
                    "u64" => verify(
                        &report.database,
                        [
                            vec![9_007_199_254_740_993u64, u64::MAX - 1],
                            vec![0, u64::MAX],
                        ],
                    ),
                    _ => unreachable!(),
                }
                assert_eq!(bytes(source.parent().unwrap()), before);
            }
        }
    }
}

#[test]
fn admission_rejects_without_source_writes() {
    let root = tempfile::tempdir().unwrap();
    let source = fixture("f32", "rom", "checkpoint", &root.path().join("source"));
    let before = bytes(source.parent().unwrap());
    for access_mode in [
        AccessMode::ReadWrite,
        AccessMode::ReadOnly,
        AccessMode::Immutable,
    ] {
        let error = Database::<f32>::open_with_config(
            source.to_str().unwrap(),
            Config {
                dim: 2,
                access_mode,
                ..Default::default()
            },
        )
        .err()
        .unwrap();
        assert_eq!(error.code(), "TDB_MIGRATION_REQUIRED");
        assert_eq!(
            error.details(),
            json!({"component":"database","found":10,"current":15,"migration_entrypoint":"scripts/migrate.py"})
        );
    }
    let opts = options(source.clone(), root.path().join("target"));
    use fs2::FileExt;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(source.with_extension("tdb.lock"))
        .unwrap();
    lock.lock_shared().unwrap();
    assert_eq!(
        triviumdb_migrate::run(&opts).unwrap_err().code,
        "TDB_DATABASE_LOCKED"
    );
    FileExt::unlock(&lock).unwrap();
    let mut limited = opts.clone();
    limited.max_source_bytes = 1;
    assert_eq!(
        triviumdb_migrate::run(&limited).unwrap_err().code,
        "TDB_MIGRATION_BUDGET"
    );
    std::fs::create_dir(&opts.destination).unwrap();
    std::fs::write(opts.destination.join("keep"), b"user").unwrap();
    assert_eq!(
        triviumdb_migrate::run(&opts).unwrap_err().code,
        "TDB_MIGRATION_TARGET_EXISTS"
    );
    assert_eq!(
        std::fs::read(opts.destination.join("keep")).unwrap(),
        b"user"
    );
    assert_eq!(bytes(source.parent().unwrap()), before);
}

#[test]
fn before_and_after_publication_failures_retry_with_complete_facts() {
    for fail in [Phase::BeforePublish, Phase::AfterPublish] {
        let root = tempfile::tempdir().unwrap();
        let source = fixture("u64", "mmap", "wal", &root.path().join("source"));
        let before = bytes(source.parent().unwrap());
        let opts = options(source.clone(), root.path().join("target"));
        let error = triviumdb_migrate::run_controlled(&opts, |phase| {
            if phase == fail {
                return Err(
                    std::io::Error::other("受控发布失败 (Controlled publication failure)").into(),
                );
            }
            Ok(())
        })
        .unwrap_err();
        assert_eq!(
            error.outcome,
            if fail == Phase::AfterPublish {
                "committed"
            } else {
                "not_committed"
            }
        );
        assert_eq!(opts.destination.exists(), fail == Phase::AfterPublish);
        if fail == Phase::AfterPublish {
            let error =
                Database::<u64>::open(opts.destination.join("database.tdb").to_str().unwrap(), 2)
                    .err()
                    .unwrap();
            assert_eq!(error.code(), "TDB_DATABASE_LOCKED");
        }
        let report = triviumdb_migrate::run(&opts).unwrap();
        verify(
            &report.database,
            [
                vec![9_007_199_254_740_993u64, u64::MAX - 1],
                vec![0, u64::MAX],
            ],
        );
        assert_eq!(bytes(source.parent().unwrap()), before);
    }
}

#[test]
fn corruption_and_unowned_staging_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let source = fixture("f32", "rom", "checkpoint", &root.path().join("source"));
    let opts = options(source.clone(), root.path().join("target"));
    let stage = root.path().join(".target.migration-stage");
    std::fs::create_dir(&stage).unwrap();
    std::fs::write(stage.join("keep"), b"foreign").unwrap();
    assert!(triviumdb_migrate::run(&opts).is_err());
    assert_eq!(std::fs::read(stage.join("keep")).unwrap(), b"foreign");
    let mut header = std::fs::read(&source).unwrap();
    header[70] ^= 1;
    std::fs::write(&source, header).unwrap();
    let before = bytes(source.parent().unwrap());
    assert_eq!(
        triviumdb_migrate::run(&opts).unwrap_err().code,
        "TDB_CORRUPTED_FILE"
    );
    assert_eq!(bytes(source.parent().unwrap()), before);
    assert!(!opts.destination.exists());
}

#[test]
fn current_document_named_graph_and_receipt_facts_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("source");
    std::fs::create_dir(&directory).unwrap();
    let source = directory.join("sample.tdb");
    let mut db = Database::<f32>::open_with_config(
        source.to_str().unwrap(),
        Config {
            dim: 2,
            auto_build_quiver: false,
            ..Default::default()
        },
    )
    .unwrap();
    let call = |db: &mut Database<f32>, request: Value| -> Value {
        serde_json::from_str(&triviumdb::sdk::documents::request(db, &request.to_string()).unwrap())
            .unwrap()
    };
    let parsed = call(
        &mut db,
        json!({"operation":"parse","input":{"source_id":"note.md","source_revision":"r1","content":"# 标题\n\n甲乙"}}),
    );
    let document = call(
        &mut db,
        json!({"operation":"put_parsed","document":{"parsed":parsed,"root":{"id":"10","vector":[1,0],"payload":{"kind":"document"}}},"expected_revision":null}),
    );
    db.insert_with_id(20, &[0., 1.], json!({"kind":"before"}))
        .unwrap();
    let graph: triviumdb::named_graph::NamedGraphWrite = serde_json::from_value(
        json!({"graph_id":"词","members":["10","20"],"edges":[{"source_id":"10","target_id":"20","label":"关联","weight":0.5,"metadata":{"source":"migration"}}],"metadata":{"kind":"shared"}}),
    )
    .unwrap();
    let graph = db.put_named_graph(graph, None).unwrap();
    let session = triviumdb::ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let fetch = json!({"version":1,"requestId":"读取","command":{"operation":"getNodes","ids":["20"],"withRevision":true}});
    let revision: Value =
        serde_json::from_str(&db.console_request(&session, &fetch.to_string()).unwrap()).unwrap();
    let command = json!({"version":1,"requestId":"修改","command":{"operation":"guardedTransaction","expectedRevision":revision["data"]["revision"],"idempotencyKey":"migration-receipt","operations":[{"op":"updatePayload","id":"20","payload":{"kind":"updated"}}]}});
    let acknowledged = db.console_request(&session, &command.to_string()).unwrap();
    db.close().unwrap();
    drop(db);
    let before = bytes(&directory);
    let report = triviumdb_migrate::run(&options(source, root.path().join("target"))).unwrap();
    let mut reopened = Database::<f32>::open(report.database.to_str().unwrap(), 2).unwrap();
    let restored = call(&mut reopened, json!({"operation":"get","document_id":"10"}));
    assert_eq!(restored["content"], document["content"]);
    assert_eq!(restored["source"], document["source"]);
    assert_eq!(
        serde_json::to_value(
            reopened
                .get_named_graph("词", &Default::default())
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(graph).unwrap()
    );
    let retry_session = triviumdb::ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
    let replayed: Value = serde_json::from_str(
        &reopened
            .console_request(&retry_session, &command.to_string())
            .unwrap(),
    )
    .unwrap();
    let acknowledged: Value = serde_json::from_str(&acknowledged).unwrap();
    assert_eq!(replayed["data"]["replayed"], true);
    assert_eq!(
        replayed["data"]["createdIds"],
        acknowledged["data"]["createdIds"]
    );
    assert_eq!(
        replayed["data"]["revision"],
        acknowledged["data"]["revision"]
    );
    assert_eq!(
        reopened.get_payload(20).unwrap(),
        Some(json!({"kind":"updated"}))
    );
    reopened.close().unwrap();
    drop(reopened);
    assert_eq!(bytes(&directory), before);
    let renamed = root.path().join("relocated");
    std::fs::rename(report.database.parent().unwrap(), &renamed).unwrap();
    let mut final_db =
        Database::<f32>::open_read_only(renamed.join("database.tdb").to_str().unwrap(), 2).unwrap();
    assert_eq!(
        final_db.get_payload(20).unwrap(),
        Some(json!({"kind":"updated"}))
    );
    final_db.close().unwrap();
}

#[test]
#[ignore = "由父测试显式启动，进程退出模拟发布边界崩溃"]
fn migration_crash_child() {
    let source = PathBuf::from(std::env::var_os("TDB_MIGRATION_SOURCE").unwrap());
    let destination = PathBuf::from(std::env::var_os("TDB_MIGRATION_TARGET").unwrap());
    let after = std::env::var("TDB_MIGRATION_AFTER").unwrap() == "1";
    triviumdb_migrate::run_controlled(&options(source, destination), |phase| {
        if phase
            == if after {
                Phase::AfterPublish
            } else {
                Phase::BeforePublish
            }
        {
            std::process::exit(73);
        }
        Ok(())
    })
    .unwrap();
    panic!("必须到达受控中断点");
}

#[test]
fn process_death_releases_leases_and_retry_confirms_contents() {
    for after in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let source = fixture("f32", "mmap", "wal", &root.path().join("source"));
        let before = bytes(source.parent().unwrap());
        let opts = options(source.clone(), root.path().join("target"));
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "migration_crash_child",
                "--ignored",
                "--test-threads=1",
            ])
            .env("TDB_MIGRATION_SOURCE", &source)
            .env("TDB_MIGRATION_TARGET", &opts.destination)
            .env("TDB_MIGRATION_AFTER", if after { "1" } else { "0" })
            .output()
            .unwrap();
        assert_eq!(
            status.status.code(),
            Some(73),
            "{}",
            String::from_utf8_lossy(&status.stdout)
        );
        let report = triviumdb_migrate::run(&opts).unwrap();
        assert_eq!(report.reused, after);
        verify(&report.database, [vec![1.0f32, -0.0], vec![0.25, 1.0]]);
        assert_eq!(bytes(source.parent().unwrap()), before);
    }
}

#[test]
fn missing_text_and_changed_destination_never_report_complete_migration() {
    let root = tempfile::tempdir().unwrap();
    let source = fixture("f32", "rom", "checkpoint", &root.path().join("source"));
    let opts = options(source.clone(), root.path().join("target"));
    let text = std::fs::read(source.with_extension("tdb.text")).unwrap();
    std::fs::remove_file(source.with_extension("tdb.text")).unwrap();
    let broken = bytes(source.parent().unwrap());
    assert_eq!(
        triviumdb_migrate::run(&opts).unwrap_err().code,
        "TDB_CORRUPTED_FILE"
    );
    assert!(!opts.destination.exists());
    assert_eq!(bytes(source.parent().unwrap()), broken);
    std::fs::write(source.with_extension("tdb.text"), text).unwrap();
    // 来源变更后使用新的目标，旧暂存目录仍按原归属保留。
    let opts = Options {
        destination: root.path().join("restored"),
        ..opts
    };
    let report = triviumdb_migrate::run(&opts).unwrap();
    let mut target = Database::<f32>::open(report.database.to_str().unwrap(), 2).unwrap();
    target.update_payload(7, json!({"business":"new"})).unwrap();
    target.close().unwrap();
    drop(target);
    let saved = bytes(&opts.destination);
    assert_eq!(
        triviumdb_migrate::run(&opts).unwrap_err().code,
        "TDB_MIGRATION_CONFLICT"
    );
    assert_eq!(bytes(&opts.destination), saved);
}

#[test]
fn hardlinked_source_and_missing_check_lease_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let source = fixture("f32", "rom", "checkpoint", &root.path().join("source"));
    let mut opts = options(source.clone(), root.path().join("target"));
    opts.check = true;
    std::fs::remove_file(source.with_extension("tdb.lock")).unwrap();
    let before = bytes(source.parent().unwrap());
    assert_eq!(
        triviumdb_migrate::run(&opts).unwrap_err().code,
        "TDB_MIGRATION_LEASE_REQUIRED"
    );
    assert_eq!(bytes(source.parent().unwrap()), before);
    std::fs::write(source.with_extension("tdb.lock"), []).unwrap();
    let alias = root.path().join("alias.tdb");
    std::fs::hard_link(&source, &alias).unwrap();
    let before = bytes(source.parent().unwrap());
    assert_eq!(
        triviumdb_migrate::run(&opts).unwrap_err().code,
        "TDB_INVALID_INPUT"
    );
    assert_eq!(bytes(source.parent().unwrap()), before);
    assert!(!opts.destination.exists());
}

#[test]
fn released_091_documents_graphs_and_receipts_survive_migration() {
    use sha2::Digest;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/release_091");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(fixtures.join("manifest.json")).unwrap()).unwrap();
    for storage in ["rom", "mmap"] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        for entry in std::fs::read_dir(fixtures.join(storage)).unwrap() {
            let entry = entry.unwrap();
            let content = std::fs::read(entry.path()).unwrap();
            assert_eq!(
                format!("{:x}", sha2::Sha256::digest(&content)),
                manifest["files"][format!("{storage}/{}", entry.file_name().to_str().unwrap())]["sha256"]
            );
            std::fs::write(source.join(entry.file_name()), content).unwrap();
        }
        std::fs::write(source.join("sample.tdb.lock"), []).unwrap();
        let expected: Value =
            serde_json::from_slice(&std::fs::read(source.join("expected.json")).unwrap()).unwrap();
        let before = bytes(&source);
        let report = triviumdb_migrate::run(&options(
            source.join("sample.tdb"),
            root.path().join("target"),
        ))
        .unwrap();
        assert_eq!(report.source_format, 13);
        // 两次打开验证回执、正文、来源映射与独立图身份；第二次读取经过新的检查点。
        for _ in 0..2 {
            let mut db = Database::<f32>::open(report.database.to_str().unwrap(), 2).unwrap();
            assert_eq!(db.all_node_ids().unwrap(), [10, 20]);
            let document: Value = serde_json::from_str(
                &triviumdb::sdk::documents::request(
                    &mut db,
                    &json!({"operation":"get","document_id":"10"}).to_string(),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(document["content"], expected["document"]["content"]);
            assert_eq!(document["source"], expected["document"]["source"]);
            assert_eq!(
                serde_json::to_value(
                    db.get_named_graph("词", &Default::default())
                        .unwrap()
                        .unwrap()
                )
                .unwrap(),
                expected["graph"]
            );
            let session = triviumdb::ConsoleSession::from_json(r#"{"readOnly":false}"#).unwrap();
            let response: Value = serde_json::from_str(
                &db.console_request(&session, &expected["command"].to_string())
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(response["data"]["replayed"], true);
            assert_eq!(
                response["data"]["revision"],
                expected["acknowledged"]["data"]["revision"]
            );
            assert_eq!(db.get_payload(20).unwrap(), Some(json!({"kind":"updated"})));
            db.close().unwrap();
        }
        assert_eq!(bytes(&source), before);
    }
}

#[test]
fn uncommitted_source_transaction_and_torn_tail_do_not_enter_target() {
    let root = tempfile::tempdir().unwrap();
    let source = fixture("f32", "mmap", "wal", &root.path().join("source"));
    let wal = source.with_extension("tdb.wal");
    let mut content = std::fs::read(&wal).unwrap();
    // 独立编码 TxBegin 与 Delete；省略 Commit，并留下不足长度字段的尾帧。
    for (tag, value) in [(0u32, 100u64), (4u32, 7u64)] {
        let mut frame = tag.to_le_bytes().to_vec();
        frame.extend_from_slice(&value.to_le_bytes());
        content.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        content.extend_from_slice(&frame);
        content.extend_from_slice(&crc32fast::hash(&frame).to_le_bytes());
    }
    content.extend_from_slice(&[8, 0]);
    std::fs::write(&wal, content).unwrap();
    let before = bytes(source.parent().unwrap());
    let report =
        triviumdb_migrate::run(&options(source.clone(), root.path().join("target"))).unwrap();
    verify(&report.database, [vec![1.0f32, -0.0], vec![0.25, 1.0]]);
    assert_eq!(bytes(source.parent().unwrap()), before);
}
