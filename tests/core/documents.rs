//! 文档来源、结构、修订和原子更新的非UI契约。
use super::support::TestDir;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use triviumdb::database::{AccessMode, Config, MissingIndexPolicy, StorageMode};
use triviumdb::document::{
    DOCUMENT_EDGE_LABEL, DocumentReference, DocumentSnapshot, DocumentWrite,
};
use triviumdb::{Database, VectorType};

fn contract() -> Value {
    serde_json::from_str(include_str!("../contracts/document_cases.json")).unwrap()
}

fn input<T: VectorType>() -> DocumentWrite<T> {
    serde_json::from_value(contract()["document"].clone()).unwrap()
}

fn config(mode: StorageMode) -> Config {
    Config {
        dim: 2,
        storage_mode: mode,
        auto_build_quiver: false,
        ..Config::default()
    }
}

fn assert_facts<T: VectorType + Serialize + DeserializeOwned>(
    db: &Database<T>,
    snapshot: &DocumentSnapshot,
) {
    let expected = contract();
    assert_eq!(snapshot.schema_version, 1);
    assert_eq!(snapshot.document_id, 9007199254740993);
    assert_eq!(snapshot.source_id, "vault://notes/cat.md");
    assert_eq!(snapshot.source_revision, "file-v1");
    assert_eq!(snapshot.parser_id, "plain-utf8-v1");
    assert_eq!(snapshot.content, "# 标题\r\n猫🐾 alpha\nbeta\n");
    assert_eq!(snapshot.content_hash, expected["expected"]["content_hash"]);
    assert_eq!(
        uuid::Uuid::parse_str(&snapshot.revision)
            .unwrap()
            .get_version_num(),
        4
    );
    assert_eq!(
        serde_json::to_value(&snapshot.blocks).unwrap(),
        json!([
            {"key":"section","parent":null,"kind":"section","start_byte":0,"end_byte":29,"node_id":null},
            {"key":"a","parent":"section","kind":"paragraph","start_byte":10,"end_byte":23,"node_id":"9007199254740995"},
            {"key":"word","parent":"a","kind":"term","start_byte":18,"end_byte":23,"node_id":"9007199254740997"},
            {"key":"b","parent":"section","kind":"line","start_byte":24,"end_byte":28,"node_id":"9007199254740999"}
        ])
    );
    assert_eq!(
        db.all_node_ids().unwrap(),
        [
            9007199254740993,
            9007199254740995,
            9007199254740997,
            9007199254740999
        ]
    );
    for (id, vector, data) in [
        (9007199254740993, [1., 0.], json!({"title":"标题"})),
        (9007199254740995, [0., 2.], json!({"role":"evidence"})),
        (9007199254740997, [1., 1.], json!({"term":"alpha"})),
        (9007199254740999, [2., 1.], json!({"role":"context"})),
    ] {
        let node = db.get(id).unwrap().unwrap();
        assert_eq!(node.id, id);
        assert_eq!(
            node.vector
                .iter()
                .map(|value| value.to_f32())
                .collect::<Vec<_>>(),
            vector
        );
        assert_eq!(node.payload["data"], data);
        if id == snapshot.document_id {
            assert_eq!(node.payload, json!({"_tdb_document":snapshot,"data":data}));
        } else {
            let (key, start, end, text) = match id {
                9007199254740995 => ("a", 10, 23, "猫🐾 alpha"),
                9007199254740997 => ("word", 18, 23, "alpha"),
                9007199254740999 => ("b", 24, 28, "beta"),
                _ => unreachable!(),
            };
            assert_eq!(
                node.payload,
                json!({"_tdb_document":{
                "schema_version":1,"document_id":"9007199254740993","revision":snapshot.revision,
                "key":key,"start_byte":start,"end_byte":end
            },"text":text,"data":data})
            );
        }
    }
    let mut edges = Vec::new();
    for id in db.all_node_ids().unwrap() {
        for edge in db.get_edges(id).unwrap() {
            assert_eq!(edge.label, DOCUMENT_EDGE_LABEL);
            assert_eq!(edge.weight, 1.);
            assert_eq!(edge.metadata["document_id"], "9007199254740993");
            assert_eq!(edge.metadata["revision"], snapshot.revision);
            let (key, parent, position) = match (id, edge.target_id) {
                (9007199254740993, 9007199254740995) => ("a", "section", 1),
                (9007199254740995, 9007199254740997) => ("word", "a", 2),
                (9007199254740993, 9007199254740999) => ("b", "section", 3),
                _ => panic!("出现未声明的文档结构边"),
            };
            assert_eq!(
                edge.metadata,
                json!({"document_id":"9007199254740993","revision":snapshot.revision,
                "key":key,"parent":parent,"position":position})
            );
            edges.push((id, edge.target_id));
        }
    }
    edges.sort_unstable();
    assert_eq!(
        edges,
        [
            (9007199254740993, 9007199254740995),
            (9007199254740993, 9007199254740999),
            (9007199254740995, 9007199254740997)
        ]
    );
    let evidence = db
        .resolve_document_reference(DocumentReference {
            document_id: snapshot.document_id,
            revision: snapshot.revision.clone(),
            start_byte: 10,
            end_byte: 23,
        })
        .unwrap();
    assert_eq!(evidence.text, "猫🐾 alpha");
    assert_eq!(
        (
            evidence.line,
            evidence.column,
            evidence.end_line,
            evidence.end_column
        ),
        (2, 1, 2, 9)
    );
    assert_eq!(evidence.source_id, snapshot.source_id);
    assert_eq!(evidence.source_revision, "file-v1");
    assert_eq!(evidence.content_hash, snapshot.content_hash);
    assert_eq!(evidence.parser_id, "plain-utf8-v1");
    assert_eq!(
        evidence.reference,
        DocumentReference {
            document_id: 9007199254740993,
            revision: snapshot.revision.clone(),
            start_byte: 10,
            end_byte: 23
        }
    );
}

fn roundtrip<T: VectorType + Serialize + DeserializeOwned>() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("document_roundtrip");
        let path = root.file("data.tdb");
        let mut db = Database::<T>::open_with_config(&path, config(mode)).unwrap();
        let snapshot = db.put_document(input(), None).unwrap();
        assert_facts(&db, &snapshot);
        db.close().unwrap();
        let before = files(&root.0);
        let mut db = Database::<T>::open_with_config(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                ..config(mode)
            },
        )
        .unwrap();
        assert_eq!(
            db.get_document(snapshot.document_id).unwrap().unwrap(),
            snapshot
        );
        assert_facts(&db, &snapshot);
        let rejected = db
            .put_document(input(), Some(&snapshot.revision))
            .unwrap_err();
        assert_eq!(rejected.code(), "TDB_READ_ONLY");
        db.close().unwrap();
        assert_eq!(files(&root.0), before);
        let moved = root.0.join("moved");
        std::fs::create_dir(&moved).unwrap();
        for (name, _) in before {
            std::fs::rename(root.0.join(&name), moved.join(name)).unwrap();
        }
        let mut db =
            Database::<T>::open_with_config(moved.join("data.tdb").to_str().unwrap(), config(mode))
                .unwrap();
        assert_facts(&db, &snapshot);
        db.delete_document(snapshot.document_id, &snapshot.revision)
            .unwrap();
        assert!(db.all_node_ids().unwrap().is_empty());
        db.close().unwrap();
        let mut db =
            Database::<T>::open_with_config(moved.join("data.tdb").to_str().unwrap(), config(mode))
                .unwrap();
        assert_eq!(db.get_document(snapshot.document_id).unwrap(), None);
        assert!(db.all_node_ids().unwrap().is_empty());
        db.close().unwrap();
    }
}

fn files(root: &std::path::Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
    let mut result = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect::<Vec<_>>();
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result
}

#[test]
fn 三种dtype完整结构证据及只读重开保持一致() {
    roundtrip::<f32>();
    roundtrip::<half::f16>();
    roundtrip::<u64>();
}

#[test]
fn 旧修订及非法范围在WAL前拒绝并保留完整文档() {
    let root = TestDir::new("document_rejections");
    let path = root.file("data.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    let snapshot = db.put_document(input(), None).unwrap();
    let before_wal = std::fs::read(format!("{path}.wal")).unwrap();
    let bad_reference = DocumentReference {
        document_id: snapshot.document_id,
        revision: snapshot.revision.clone(),
        start_byte: 11,
        end_byte: 23,
    };
    let error = db.resolve_document_reference(bad_reference).unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        serde_json::to_value(error.info().target).unwrap(),
        json!({"field":"reference"})
    );
    let error = db.put_document(input(), None).unwrap_err();
    assert_eq!(error.code(), "TDB_DOCUMENT_REVISION_MISMATCH");
    assert_eq!(
        error.details(),
        json!({"document_id":"9007199254740993","expected":"absent","actual":snapshot.revision})
    );
    for mutation in ["unicode", "parent", "duplicate", "dimension", "source"] {
        let mut document = input::<f32>();
        match mutation {
            "unicode" => document.blocks[1].start_byte = 11,
            "parent" => document.blocks[1].parent = Some("later".into()),
            "duplicate" => document.blocks[2].node.as_mut().unwrap().id = document.root.id,
            "dimension" => document.blocks[1].node.as_mut().unwrap().vector = vec![1.],
            "source" => document.source_id = "other".into(),
            _ => unreachable!(),
        }
        let error = db
            .put_document(document, Some(&snapshot.revision))
            .unwrap_err();
        assert!(
            matches!(error.code(), "TDB_INVALID_INPUT" | "TDB_DIMENSION_MISMATCH"),
            "{mutation}: {error:?}"
        );
        assert!(error.info().target.is_some());
        assert_eq!(
            error.info().write.unwrap().outcome,
            triviumdb::error::WriteOutcome::NotCommitted
        );
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before_wal);
        assert_facts(&db, &snapshot);
    }
    let mut changed = input::<f32>();
    changed.source_revision = "file-v2".into();
    let updated = db.put_document(changed, Some(&snapshot.revision)).unwrap();
    let reference = DocumentReference {
        document_id: snapshot.document_id,
        revision: snapshot.revision.clone(),
        start_byte: 10,
        end_byte: 23,
    };
    let error = db.resolve_document_reference(reference).unwrap_err();
    assert_eq!(error.code(), "TDB_DOCUMENT_REVISION_MISMATCH");
    assert_eq!(error.details()["actual"], updated.revision);
    assert_eq!(
        db.delete_document(snapshot.document_id, &snapshot.revision)
            .unwrap_err()
            .code(),
        "TDB_DOCUMENT_REVISION_MISMATCH"
    );
    db.close().unwrap();
}

#[test]
fn 替换删除失效片段并保留复用节点的外部关系与严格索引重开() {
    let root = TestDir::new("document_replace");
    let path = root.file("data.tdb");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    let original = db.put_document(input(), None).unwrap();
    db.insert_with_id(100, &[0., 1.], json!({"external":true}))
        .unwrap();
    db.upsert_edge(
        100,
        9007199254740995,
        "evidence",
        0.5,
        json!({"external":true}),
    )
    .unwrap();
    db.build_quiver_index(None).unwrap();
    let mut changed = input::<f32>();
    changed.source_revision = "file-v2".into();
    changed.blocks[1].node.as_mut().unwrap().vector = vec![2., 0.];
    let updated = db.put_document(changed, Some(&original.revision)).unwrap();
    assert_ne!(updated.revision, original.revision);
    assert_eq!(db.get(9007199254740995).unwrap().unwrap().vector, [2., 0.]);
    assert_eq!(
        db.get_edges(100).unwrap()[0].metadata,
        json!({"external":true})
    );
    db.close().unwrap();
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            missing_index_policy: MissingIndexPolicy::Error,
            ..config(StorageMode::Mmap)
        },
    )
    .unwrap();
    assert_eq!(
        db.get_document(original.document_id).unwrap().unwrap(),
        updated
    );
    assert_eq!(db.get(9007199254740995).unwrap().unwrap().vector, [2., 0.]);
    assert_eq!(db.get_edges(100).unwrap()[0].target_id, 9007199254740995);
    let mut reduced = input::<f32>();
    reduced.source_revision = "file-v3".into();
    reduced.blocks.truncate(2);
    let final_snapshot = db.put_document(reduced, Some(&updated.revision)).unwrap();
    assert_eq!(
        db.all_node_ids().unwrap(),
        [100, 9007199254740993, 9007199254740995]
    );
    assert_eq!(
        db.get_document(original.document_id).unwrap().unwrap(),
        final_snapshot
    );
    db.delete_document(original.document_id, &final_snapshot.revision)
        .unwrap();
    assert_eq!(db.all_node_ids().unwrap(), [100]);
    assert_eq!(db.get_edges(100).unwrap(), []);
    db.close().unwrap();
}

#[test]
fn 文档SDK预算取消及节点身份争用拒绝保留完整事实() {
    let root = TestDir::new("document_sdk_admission");
    let path = root.file("data.tdb");
    let runtime = triviumdb::ResourceRuntime::new(triviumdb::ResourceLimits {
        memory_limit: 16 * 1024 * 1024,
        max_threads: 2,
        ..Default::default()
    })
    .unwrap();
    let mut db = Database::<f32>::open_with_runtime(
        &path,
        Config {
            memory_limit: 4 * 1024 * 1024,
            ..config(StorageMode::Mmap)
        },
        runtime.clone(),
    )
    .unwrap();
    let snapshot = db.put_document(input(), None).unwrap();
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    let context = triviumdb::sdk::RequestContext::new(
        "cancel-document",
        triviumdb::query::tql_executor::QueryControl::default(),
        true,
    )
    .unwrap();
    context.cancel();
    let error = context
        .run(|| db.put_document(input(), Some(&snapshot.revision)))
        .unwrap_err();
    assert_eq!(error.code(), "TDB_QUERY_CANCELLED");
    let malformed =
        json!({"operation":"put","document":{"root":{}},"expected_revision":snapshot.revision})
            .to_string();
    let error = triviumdb::sdk::documents::request(&mut db, &malformed).unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        error.info().write.unwrap().outcome,
        triviumdb::error::WriteOutcome::NotCommitted
    );
    let mut collision = input::<f32>();
    collision.root.id = 200;
    let error = db.put_document(collision, None).unwrap_err();
    assert_eq!(error.code(), "TDB_NODE_ALREADY_EXISTS");
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert_facts(&db, &snapshot);
    let large = json!({"operation":"put","document":{"content":"x".repeat(300_000)}}).to_string();
    let error = triviumdb::sdk::documents::request(&mut db, &large).unwrap_err();
    assert_eq!(error.code(), "QueueFull");
    assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
    assert_facts(&db, &snapshot);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    db.close().unwrap();
    assert_eq!(runtime.stats().reserved_database_bytes, 0);
    assert_eq!(runtime.stats().attached_databases, 0);
}

#[test]
fn 文档JSON数值转换保留f16范围错误及u64完整整数() {
    let root = TestDir::new("document_json_numbers");
    let mut db = Database::<half::f16>::open(&root.file("half.tdb"), 2).unwrap();
    let mut request = json!({"operation":"put","document":contract()["document"]});
    request["document"]["root"]["vector"][0] = json!(65536);
    let error = triviumdb::sdk::documents::request(&mut db, &request.to_string()).unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_VECTOR");
    assert_eq!(
        serde_json::to_value(error.info().target).unwrap(),
        json!({"field":"/document/root/vector/0"})
    );
    assert!(db.all_node_ids().unwrap().is_empty());
    db.close().unwrap();
    let path = root.file("binary.tdb");
    let mut db = Database::<u64>::open(&path, 2).unwrap();
    request["document"]["root"]["vector"][0] = json!(u64::MAX);
    let response = triviumdb::sdk::documents::request(&mut db, &request.to_string()).unwrap();
    let snapshot: DocumentSnapshot = serde_json::from_str(&response).unwrap();
    assert_eq!(
        db.get(snapshot.document_id).unwrap().unwrap().vector,
        [u64::MAX, 0]
    );
    db.close().unwrap();
    let mut db = Database::<u64>::open(&path, 2).unwrap();
    assert_eq!(
        db.get_document(snapshot.document_id).unwrap().unwrap(),
        snapshot
    );
    assert_eq!(
        db.get(snapshot.document_id).unwrap().unwrap().vector,
        [u64::MAX, 0]
    );
    db.close().unwrap();
}

#[test]
fn 来源片段及结构边失配在只读重开时拒绝并保留原件() {
    for damaged in [
        "source_hash",
        "fragment_revision",
        "fragment_text",
        "hierarchy",
    ] {
        let root = TestDir::new("document_integrity");
        let path = root.file("data.tdb");
        let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
        let snapshot = db.put_document(input(), None).unwrap();
        match damaged {
            "source_hash" => {
                let mut payload = db.get_payload(snapshot.document_id).unwrap().unwrap();
                payload["_tdb_document"]["content"] = json!("# 标题\r\n狗🐾 alpha\nbeta\n");
                db.update_payload(snapshot.document_id, payload).unwrap();
            }
            "fragment_revision" | "fragment_text" => {
                let mut payload = db.get_payload(9007199254740995).unwrap().unwrap();
                if damaged == "fragment_revision" {
                    payload["_tdb_document"]["revision"] = json!("wrong-revision");
                } else {
                    payload["text"] = json!("other text");
                }
                db.update_payload(9007199254740995, payload).unwrap();
            }
            "hierarchy" => db
                .upsert_edge(
                    snapshot.document_id,
                    9007199254740995,
                    DOCUMENT_EDGE_LABEL,
                    0.5,
                    json!({"wrong":true}),
                )
                .unwrap(),
            _ => unreachable!(),
        }
        db.close().unwrap();
        let before = files(&root.0);
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                ..config(StorageMode::Mmap)
            },
        )
        .unwrap();
        let error = db.get_document(snapshot.document_id).unwrap_err();
        assert_eq!(error.code(), "TDB_CORRUPTED_FILE", "{damaged}: {error:?}");
        assert_eq!(
            db.all_node_ids().unwrap(),
            [
                9007199254740993,
                9007199254740995,
                9007199254740997,
                9007199254740999
            ]
        );
        db.close().unwrap();
        assert_eq!(files(&root.0), before);
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn 文档WAL同步失败保留旧视图并恢复完整新修订及再次持久化() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("document_wal_failure");
        let path = root.file("data.tdb");
        let config = Config {
            sync_mode: triviumdb::storage::wal::SyncMode::Full,
            ..config(mode)
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        let original = db.put_document(input(), None).unwrap();
        db.flush().unwrap();
        let mut changed = input::<f32>();
        changed.source_revision = "file-v2".into();
        changed.blocks[1].node.as_mut().unwrap().vector = vec![2., 0.];
        let failure = triviumdb::test_hooks::fail_io_at(triviumdb::test_hooks::IoPoint::WalSync);
        let error = db
            .put_document(changed, Some(&original.revision))
            .unwrap_err();
        drop(failure);
        assert_eq!(error.code(), "TDB_IO");
        assert_eq!(
            error.info().write.unwrap().outcome,
            triviumdb::error::WriteOutcome::Unknown
        );
        assert_eq!(
            db.get_document(original.document_id).unwrap().unwrap(),
            original
        );
        assert_facts(&db, &original);
        drop(db);
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        let recovered = db.get_document(original.document_id).unwrap().unwrap();
        let mut expected = original.clone();
        expected.revision = recovered.revision.clone();
        expected.source_revision = "file-v2".into();
        assert_ne!(recovered.revision, original.revision);
        assert_eq!(recovered, expected);
        assert_eq!(
            db.all_node_ids().unwrap(),
            [
                9007199254740993,
                9007199254740995,
                9007199254740997,
                9007199254740999
            ]
        );
        for (id, vector) in [
            (9007199254740993, [1., 0.]),
            (9007199254740995, [2., 0.]),
            (9007199254740997, [1., 1.]),
            (9007199254740999, [2., 1.]),
        ] {
            assert_eq!(db.get(id).unwrap().unwrap().vector, vector);
        }
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        assert_eq!(
            db.get_document(original.document_id).unwrap().unwrap(),
            recovered
        );
        assert_eq!(db.get(9007199254740995).unwrap().unwrap().vector, [2., 0.]);
        db.close().unwrap();
    }
}
