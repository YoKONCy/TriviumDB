//! 来源分组BM25、修订证据、派生缓存与上下文的核心功能契约。
use super::support::TestDir;
use serde_json::{Value, json};
use triviumdb::database::{AccessMode, Config, StorageMode};
use triviumdb::document::*;
use triviumdb::{Database, TriviumError};

fn fixture() -> Value {
    serde_json::from_str(include_str!("../contracts/document_retrieval_cases.json")).unwrap()
}

fn config(mode: StorageMode) -> Config {
    Config {
        dim: 2,
        storage_mode: mode,
        auto_build_quiver: false,
        ..Config::default()
    }
}

fn populate(db: &mut Database<f32>) -> Vec<DocumentSnapshot> {
    fixture()["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| {
            db.put_document(serde_json::from_value(value.clone()).unwrap(), None)
                .unwrap()
        })
        .collect()
}

fn query() -> DocumentSearchQuery {
    serde_json::from_value(fixture()["query"].clone()).unwrap()
}

fn ids(result: &DocumentSearchResult) -> Vec<(u64, Vec<u64>)> {
    result
        .groups
        .iter()
        .map(|group| {
            (
                group.document_id,
                group.hits.iter().map(|hit| hit.node_id).collect(),
            )
        })
        .collect()
}

fn ascii_evidence(
    snapshot: &DocumentSnapshot,
    text: &str,
    range: std::ops::Range<usize>,
    coordinates: (usize, usize, usize, usize),
) -> DocumentEvidence {
    let contract = fixture();
    let index = contract["documents"]
        .as_array()
        .unwrap()
        .iter()
        .position(|document| {
            document["root"]["id"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                == snapshot.document_id
        })
        .unwrap();
    let document = &contract["documents"][index];
    DocumentEvidence {
        source_spans: Vec::new(),
        reference: DocumentReference {
            document_id: document["root"]["id"].as_str().unwrap().parse().unwrap(),
            revision: snapshot.revision.clone(),
            start_byte: range.start,
            end_byte: range.end,
        },
        source_id: document["source_id"].as_str().unwrap().into(),
        source_revision: document["source_revision"].as_str().unwrap().into(),
        parser_id: document["parser_id"].as_str().unwrap().into(),
        content_hash: contract["expected"]["content_hashes"][index]
            .as_str()
            .unwrap()
            .into(),
        text: text.into(),
        line: coordinates.0,
        column: coordinates.1,
        end_line: coordinates.2,
        end_column: coordinates.3,
    }
}

fn assert_scores(result: &DocumentSearchResult) {
    // 独立代入Okapi定义：四个非空片段长度2/2/2/1，alpha的df=3，k1=1.2、b=0.75。
    // 根语料单独计算：长度4/2/1，alpha的df=2。使用f64参考值容许f32舍入。
    let expected: Vec<(f64, u64)> = match result.level {
        DocumentSearchLevel::Block => vec![
            (
                (1.0_f64 + 1.5 / 3.5).ln() * 4.4 / (2.0 + 1.2 * (0.25 + 0.75 * 2.0 / 1.75)),
                101,
            ),
            (
                (1.0_f64 + 1.5 / 3.5).ln() * 2.2 / (1.0 + 1.2 * (0.25 + 0.75 * 2.0 / 1.75)),
                201,
            ),
        ],
        DocumentSearchLevel::Document => vec![
            (
                (1.0_f64 + 1.5 / 2.5).ln() * 8.8 / (4.0 + 1.2 * (0.25 + 0.75 * 4.0 / (7.0 / 3.0))),
                100,
            ),
            (
                (1.0_f64 + 1.5 / 2.5).ln() * 2.2 / (1.0 + 1.2 * (0.25 + 0.75 * 2.0 / (7.0 / 3.0))),
                200,
            ),
        ],
    };
    assert_eq!(result.groups.len(), expected.len());
    for (group, (score, id)) in result.groups.iter().zip(expected) {
        assert_eq!(group.hits.len(), 1);
        assert_eq!(group.hits[0].node_id, id);
        assert!(
            (f64::from(group.score) - score).abs() < 0.000001,
            "{group:?}，预期{score}"
        );
        assert_eq!(group.score, group.hits[0].score);
    }
}

#[test]
fn 完整候选先分组并保持独立语料统计和确定排序() {
    let root = TestDir::new("document_group_search");
    let mut db =
        Database::<f32>::open_with_config(&root.file("data.tdb"), config(StorageMode::Mmap))
            .unwrap();
    let snapshots = populate(&mut db);
    let result = db.search_documents(&query()).unwrap();
    assert_eq!(
        (
            result.corpus_documents,
            result.corpus_units,
            result.matched_units
        ),
        (3, 4, 3)
    );
    assert_eq!(ids(&result), [(100, vec![101]), (200, vec![201])]);
    assert_scores(&result);
    for (group, text) in result.groups.iter().zip(["alpha alpha", "alpha beta"]) {
        let source = snapshots
            .iter()
            .find(|row| row.document_id == group.document_id)
            .unwrap();
        let hit = &group.hits[0];
        assert_eq!(group.revision, source.revision);
        assert_eq!(group.source_id, source.source_id);
        assert_eq!(group.source_revision, "v1");
        assert_eq!(hit.block_key.as_deref(), Some("first"));
        assert_eq!(
            hit.evidence,
            ascii_evidence(source, text, 0..text.len(), (1, 1, 1, text.len() + 1))
        );
        assert_eq!(hit.evidence.text, text);
    }
    let full = db
        .search_documents(&DocumentSearchQuery {
            hits_per_document: 2,
            ..query()
        })
        .unwrap();
    assert_eq!(ids(&full), [(100, vec![101, 102]), (200, vec![201])]);
    let roots = db
        .search_documents(&DocumentSearchQuery {
            level: DocumentSearchLevel::Document,
            ..query()
        })
        .unwrap();
    assert_eq!((roots.corpus_units, roots.matched_units), (3, 2));
    assert_eq!(ids(&roots), [(100, vec![100]), (200, vec![200])]);
    assert_scores(&roots);
    for field in ["document_ids", "source_ids", "kinds"] {
        let mut filtered = query();
        match field {
            "document_ids" => filtered.document_ids = vec![200],
            "source_ids" => filtered.source_ids = vec!["notes://b".into()],
            "kinds" => {
                filtered.document_ids = vec![200];
                filtered.kinds = vec!["line".into()];
            }
            _ => unreachable!(),
        }
        let rows = db.search_documents(&filtered).unwrap();
        assert_eq!(rows.groups, result.groups[1..]);
        assert_eq!((rows.corpus_units, rows.matched_units), (4, 1));
    }
    let paragraphs = db
        .search_documents(&DocumentSearchQuery {
            kinds: vec!["paragraph".into()],
            ..query()
        })
        .unwrap();
    assert_eq!(ids(&paragraphs), [(100, vec![101])]);
    assert_eq!(paragraphs.groups, result.groups[..1]);
    let empty = db
        .search_documents(&DocumentSearchQuery {
            text: "unknown".into(),
            ..query()
        })
        .unwrap();
    assert_eq!(
        (
            empty.corpus_documents,
            empty.corpus_units,
            empty.matched_units
        ),
        (3, 4, 0)
    );
    assert!(empty.groups.is_empty());
    let mut no_output = DocumentSearchQuery {
        text: "unknown".into(),
        ..query()
    };
    no_output.limits.max_return_bytes = 0;
    assert_budget(
        db.search_documents(&no_output).unwrap_err(),
        "document_return_bytes",
    );
    db.close().unwrap();
}

#[test]
fn 缓存开关清理写入失效重开及只读文件组完整性() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("document_cache_lifecycle");
        let path = root.file("data.tdb");
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        let snapshots = populate(&mut db);
        db.index_text(201, "manual-only").unwrap();
        let result = db.search_documents(&query()).unwrap();
        assert_eq!(
            db.search_documents(&DocumentSearchQuery {
                use_cache: false,
                ..query()
            })
            .unwrap(),
            result
        );
        let hot = db.index_memory_stats().hot_bytes;
        db.clear_document_index().unwrap();
        assert!(db.index_memory_stats().hot_bytes < hot);
        assert_eq!(db.search_documents(&query()).unwrap(), result);
        let manual = db
            .tql_values("TEXT BM25 \"manual-only\" TOP 10 AS hits WITH hits RETURN hits")
            .unwrap();
        assert_eq!(
            manual
                .iter()
                .map(|row| row["hits"].as_node().unwrap().id)
                .collect::<Vec<_>>(),
            [201]
        );
        let mut changed: DocumentWrite<f32> =
            serde_json::from_value(fixture()["documents"][0].clone()).unwrap();
        changed.content = "delta delta\ndelta delta\n".into();
        changed.source_revision = "v2".into();
        let updated = db
            .put_document(changed, Some(&snapshots[0].revision))
            .unwrap();
        assert_eq!(
            ids(&db.search_documents(&query()).unwrap()),
            [(200, vec![201])]
        );
        assert_eq!(
            ids(&db
                .search_documents(&DocumentSearchQuery {
                    text: "delta".into(),
                    ..query()
                })
                .unwrap()),
            [(100, vec![101])]
        );
        db.delete_document(200, &snapshots[1].revision).unwrap();
        assert!(db.search_documents(&query()).unwrap().groups.is_empty());
        db.close().unwrap();
        let before = file_group(&root.0);
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                load_text_index: false,
                ..config(mode)
            },
        )
        .unwrap();
        let delta = db
            .search_documents(&DocumentSearchQuery {
                text: "delta".into(),
                ..query()
            })
            .unwrap();
        assert_eq!(ids(&delta), [(100, vec![101])]);
        assert_eq!(delta.groups[0].revision, updated.revision);
        assert_eq!(delta.groups[0].hits[0].evidence.text, "delta delta");
        db.clear_document_index().unwrap();
        db.close().unwrap();
        let mut reader = triviumdb::DatabaseReader::<f32>::open_with_runtime(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                ..config(mode)
            },
            triviumdb::ResourceRuntime::new(Default::default()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            reader
                .search_documents(&DocumentSearchQuery {
                    text: "delta".into(),
                    ..query()
                })
                .unwrap(),
            delta
        );
        let context = reader
            .document_context(&DocumentContextQuery {
                seeds: vec![seed(&updated, "first", 0, 11)],
                before: 0,
                after: 0,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(context[0].windows[0].text, "delta delta");
        reader.clear_document_index().unwrap();
        reader.close().unwrap();
        assert_eq!(file_group(&root.0), before);
        let moved = root.0.join("moved");
        std::fs::create_dir(&moved).unwrap();
        for (name, _) in &before {
            std::fs::rename(root.0.join(name), moved.join(name)).unwrap();
        }
        assert_eq!(delta.groups[0].hits[0].evidence.text, "delta delta");
        let mut db = Database::<f32>::open_with_config(
            moved.join("data.tdb").to_str().unwrap(),
            config(mode),
        )
        .unwrap();
        assert_eq!(
            db.search_documents(&DocumentSearchQuery {
                text: "delta".into(),
                ..query()
            })
            .unwrap(),
            delta
        );
        db.close().unwrap();
    }
}

fn file_group(path: &std::path::Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
    let mut rows = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn seed(snapshot: &DocumentSnapshot, key: &str, start: usize, end: usize) -> DocumentContextSeed {
    DocumentContextSeed {
        reference: DocumentReference {
            document_id: snapshot.document_id,
            revision: snapshot.revision.clone(),
            start_byte: start,
            end_byte: end,
        },
        block_key: Some(key.into()),
    }
}

#[test]
fn 上下文按字节顺序展开混合粒度同级及纯目录祖先并合并重叠范围() {
    let root = TestDir::new("document_context");
    let mut db = Database::<f32>::open(&root.file("data.tdb"), 2).unwrap();
    let snapshots = populate(&mut db);
    let query = DocumentContextQuery {
        seeds: vec![
            seed(&snapshots[0], "first", 0, 5),
            seed(&snapshots[0], "second", 12, 17),
            seed(&snapshots[1], "first", 0, 5),
        ],
        before: 1,
        after: 1,
        ..Default::default()
    };
    let rows = db.document_context(&query).unwrap();
    assert_eq!(
        rows.iter().map(|row| row.document_id).collect::<Vec<_>>(),
        [100, 200]
    );
    assert_eq!(rows[0].block_keys, ["second", "first"]);
    assert_eq!(
        rows[0].windows,
        [ascii_evidence(
            &snapshots[0],
            "alpha alpha\nalpha alpha",
            0..23,
            (1, 1, 2, 12)
        )]
    );
    assert_eq!(rows[1].block_keys, ["first"]);
    assert_eq!(rows[1].windows[0].text, "alpha beta");
    let ancestors = db
        .document_context(&DocumentContextQuery {
            seeds: vec![seed(&snapshots[0], "first", 0, 5)],
            before: 0,
            after: 0,
            ancestor_depth: 1,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(ancestors[0].block_keys, ["section", "first"]);
    assert_eq!(ancestors[0].windows[0].text, "alpha alpha\nalpha alpha\n");
    assert_eq!(ancestors[0].windows.len(), 1);
    let unicode: Value =
        serde_json::from_str(include_str!("../contracts/document_cases.json")).unwrap();
    let unicode = db
        .put_document(
            serde_json::from_value(unicode["document"].clone()).unwrap(),
            None,
        )
        .unwrap();
    let result = db
        .document_context(&DocumentContextQuery {
            seeds: vec![seed(&unicode, "a", 10, 23), seed(&unicode, "word", 18, 23)],
            before: 0,
            after: 0,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(result[0].block_keys, ["a", "word"]);
    assert_eq!(
        result[0].windows,
        [DocumentEvidence {
            source_spans: Vec::new(),
            reference: DocumentReference {
                document_id: 9007199254740993,
                revision: unicode.revision.clone(),
                start_byte: 10,
                end_byte: 23
            },
            source_id: "vault://notes/cat.md".into(),
            source_revision: "file-v1".into(),
            parser_id: "plain-utf8-v1".into(),
            content_hash: "sha256:6e9866572221cc397e5b2faa1ea0ada7d1313e807a7876167221cad7db7f7331"
                .into(),
            text: "猫🐾 alpha".into(),
            line: 2,
            column: 1,
            end_line: 2,
            end_column: 9,
        }]
    );
    let mut bad_boundary = seed(&unicode, "a", 11, 23);
    let error = db
        .document_context(&DocumentContextQuery {
            seeds: vec![bad_boundary.clone()],
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        serde_json::to_value(error.info().target).unwrap(),
        json!({"field":"query.seeds.reference"})
    );
    bad_boundary.reference.start_byte = 10;
    bad_boundary.reference.revision = "stale".into();
    assert_eq!(
        db.document_context(&DocumentContextQuery {
            seeds: vec![seed(&snapshots[0], "first", 0, 5), bad_boundary],
            ..Default::default()
        })
        .unwrap_err()
        .code(),
        "TDB_DOCUMENT_REVISION_MISMATCH"
    );
    db.close().unwrap();
}

fn assert_budget(error: TriviumError, resource: &str) {
    assert_eq!(error.code(), "QueueFull");
    let TriviumError::ResourceBudgetExceeded {
        resource: actual,
        requested,
        limit,
        ..
    } = error.underlying()
    else {
        panic!("预算错误结构缺失：{error:?}")
    };
    assert_eq!(*actual, resource);
    assert!(requested > limit);
}

#[test]
fn 冷热查询预算一致拒绝取消和修订失配并归还工作区() {
    let root = TestDir::new("document_query_limits");
    let runtime = triviumdb::ResourceRuntime::new(triviumdb::ResourceLimits {
        memory_limit: 32 * 1024 * 1024,
        max_threads: 2,
        ..Default::default()
    })
    .unwrap();
    let mut db = Database::<f32>::open_with_runtime(
        &root.file("data.tdb"),
        Config {
            memory_limit: 8 * 1024 * 1024,
            ..config(StorageMode::Mmap)
        },
        runtime.clone(),
    )
    .unwrap();
    let snapshots = populate(&mut db);
    for resource in [
        "document_scan_nodes",
        "document_payload_bytes",
        "document_text_bytes",
        "document_edges",
        "document_postings",
        "document_candidates",
        "document_return_bytes",
    ] {
        let mut limited = query();
        match resource {
            "document_scan_nodes" => limited.limits.max_scan_nodes = 0,
            "document_payload_bytes" => limited.limits.max_payload_bytes = 0,
            "document_text_bytes" => limited.limits.max_text_bytes = 0,
            "document_edges" => limited.limits.max_edges = 0,
            "document_postings" => limited.limits.max_postings = 0,
            "document_candidates" => limited.limits.max_candidates = 1,
            "document_return_bytes" => limited.limits.max_return_bytes = 1,
            _ => unreachable!(),
        }
        for warm in [false, true] {
            db.clear_document_index().unwrap();
            if warm {
                db.search_documents(&query()).unwrap();
            }
            assert_budget(db.search_documents(&limited).unwrap_err(), resource);
            assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        }
    }
    let mut stale = query();
    stale.expected_revisions.insert(100, "stale".into());
    assert_eq!(
        db.search_documents(&stale).unwrap_err().code(),
        "TDB_DOCUMENT_REVISION_MISMATCH"
    );
    let context = DocumentContextQuery {
        seeds: vec![seed(&snapshots[0], "first", 0, 5)],
        ..Default::default()
    };
    let mut stale = context.clone();
    stale.seeds[0].reference.revision = "stale".into();
    assert_eq!(
        db.document_context(&stale).unwrap_err().code(),
        "TDB_DOCUMENT_REVISION_MISMATCH"
    );
    let mut invalid = context.clone();
    invalid.seeds[0].block_key = Some("missing".into());
    assert_eq!(
        db.document_context(&invalid).unwrap_err().code(),
        "TDB_INVALID_INPUT"
    );
    let mut outside = context.clone();
    outside.seeds[0].reference.end_byte = 12;
    assert_eq!(
        db.document_context(&outside).unwrap_err().code(),
        "TDB_INVALID_INPUT"
    );
    let mut limited = context.clone();
    limited.limits.max_return_bytes = 1;
    assert_budget(
        db.document_context(&limited).unwrap_err(),
        "document_return_bytes",
    );
    for resource in [
        "document_scan_nodes",
        "document_payload_bytes",
        "document_edges",
        "document_postings",
        "document_candidates",
    ] {
        let mut limited = context.clone();
        match resource {
            "document_scan_nodes" => limited.limits.max_scan_nodes = 0,
            "document_payload_bytes" => limited.limits.max_payload_bytes = 0,
            "document_edges" => limited.limits.max_edges = 0,
            "document_postings" => limited.limits.max_postings = 0,
            "document_candidates" => limited.limits.max_candidates = 1,
            _ => unreachable!(),
        }
        assert_budget(db.document_context(&limited).unwrap_err(), resource);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }
    let cancelled = triviumdb::sdk::RequestContext::new(
        "document-query-cancel",
        triviumdb::query::tql_executor::QueryControl::default(),
        false,
    )
    .unwrap();
    cancelled.cancel();
    assert_eq!(
        cancelled
            .run(|| db.search_documents(&query()))
            .unwrap_err()
            .code(),
        "TDB_QUERY_CANCELLED"
    );
    assert_eq!(
        cancelled
            .run(|| db.document_context(&context))
            .unwrap_err()
            .code(),
        "TDB_QUERY_CANCELLED"
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert_eq!(db.get_document(100).unwrap().unwrap(), snapshots[0]);
    db.close().unwrap();
    assert_eq!(runtime.stats().reserved_database_bytes, 0);
}

#[test]
fn 普通CRUD破坏已缓存片段或结构时检索拒绝旧缓存() {
    for damage in [
        "fragment",
        "missing_fragment",
        "root",
        "null_key",
        "edge",
        "orphan",
    ] {
        let root = TestDir::new("document_query_integrity");
        let mut db = Database::<f32>::open(&root.file("data.tdb"), 2).unwrap();
        populate(&mut db);
        db.search_documents(&query()).unwrap();
        match damage {
            "fragment" => {
                let mut payload = db.get_payload(101).unwrap().unwrap();
                payload["text"] = json!("corrupted");
                db.update_payload(101, payload).unwrap();
            }
            "root" => {
                let mut payload = db.get_payload(100).unwrap().unwrap();
                payload["_tdb_document"]["content_hash"] = json!("wrong");
                db.update_payload(100, payload).unwrap();
            }
            "null_key" => {
                let mut payload = db.get_payload(100).unwrap().unwrap();
                payload["_tdb_document"]["key"] = Value::Null;
                db.update_payload(100, payload).unwrap();
            }
            "edge" => db.unlink_label(100, 101, DOCUMENT_EDGE_LABEL).unwrap(),
            "orphan" => {
                db.delete(100).unwrap();
            }
            "missing_fragment" => {
                db.delete(101).unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            db.search_documents(&query()).unwrap_err().code(),
            "TDB_CORRUPTED_FILE",
            "{damage}"
        );
        db.close().unwrap();
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn 文档检索在WAL同步失败后保留旧事实并在恢复后采用完整新修订() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("document_search_wal");
        let path = root.file("data.tdb");
        let config = Config {
            sync_mode: triviumdb::storage::wal::SyncMode::Full,
            ..config(mode)
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        let snapshots = populate(&mut db);
        db.flush().unwrap();
        let original = db.search_documents(&query()).unwrap();
        let mut changed: DocumentWrite<f32> =
            serde_json::from_value(fixture()["documents"][0].clone()).unwrap();
        changed.content = "delta delta\ndelta delta\n".into();
        changed.source_revision = "v2".into();
        let failure = triviumdb::test_hooks::fail_io_at(triviumdb::test_hooks::IoPoint::WalSync);
        let error = db
            .put_document(changed, Some(&snapshots[0].revision))
            .unwrap_err();
        drop(failure);
        assert_eq!(error.code(), "TDB_IO");
        assert_eq!(
            error.info().write.unwrap().outcome,
            triviumdb::error::WriteOutcome::Unknown
        );
        assert_eq!(db.search_documents(&query()).unwrap(), original);
        let delta_query = DocumentSearchQuery {
            text: "delta".into(),
            ..query()
        };
        assert!(db.search_documents(&delta_query).unwrap().groups.is_empty());
        drop(db);
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        let recovered = db.search_documents(&delta_query).unwrap();
        assert_eq!(ids(&recovered), [(100, vec![101])]);
        assert_ne!(recovered.groups[0].revision, snapshots[0].revision);
        assert_eq!(recovered.groups[0].source_revision, "v2");
        assert_eq!(recovered.groups[0].hits[0].evidence.text, "delta delta");
        assert_eq!(
            ids(&db.search_documents(&query()).unwrap()),
            [(200, vec![201])]
        );
        let context_query = DocumentContextQuery {
            seeds: vec![DocumentContextSeed {
                reference: recovered.groups[0].hits[0].evidence.reference.clone(),
                block_key: Some("first".into()),
            }],
            ..Default::default()
        };
        let context = db.document_context(&context_query).unwrap();
        assert_eq!(context[0].windows[0].text, "delta delta\ndelta delta");
        assert_eq!(
            context[0].windows[0].reference.revision,
            recovered.groups[0].revision
        );
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        assert_eq!(db.search_documents(&delta_query).unwrap(), recovered);
        assert_eq!(db.document_context(&context_query).unwrap(), context);
        db.close().unwrap();
    }
}

#[test]
fn JSON检索参数拒绝非法身份及未知字段并保留正确字段位置() {
    let root = TestDir::new("document_query_json");
    let mut db = Database::<f32>::open(&root.file("data.tdb"), 2).unwrap();
    populate(&mut db);
    for query in [
        json!({"text":"alpha","document_ids":[100]}),
        json!({"text":"alpha","document_ids":["0100"]}),
        json!({"text":"alpha","expected_revisions":{"0100":"stale"}}),
        json!({"text":"alpha","limits":{"unknown":1}}),
        json!({"text":"alpha","unknown":1}),
    ] {
        let error = triviumdb::sdk::documents::request(
            &mut db,
            &json!({"operation":"search","query":query}).to_string(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT");
        assert_eq!(
            serde_json::to_value(error.info().target).unwrap(),
            json!({"field":"request_json"})
        );
        assert!(error.info().write.is_none());
    }
    for (query, field) in [
        (json!({"text":" "}), "query.text"),
        (
            json!({"text":"alpha","max_documents":0}),
            "query.max_documents",
        ),
        (
            json!({"text":"alpha","hits_per_document":0}),
            "query.hits_per_document",
        ),
    ] {
        let error = triviumdb::sdk::documents::request(
            &mut db,
            &json!({"operation":"search","query":query}).to_string(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT");
        assert_eq!(
            serde_json::to_value(error.info().target).unwrap(),
            json!({"field":field})
        );
    }
    assert_eq!(
        ids(&db.search_documents(&query()).unwrap()),
        [(100, vec![101]), (200, vec![201])]
    );
    db.close().unwrap();
}

#[test]
fn 大量纯目录块的输出预算覆盖完整JSON分隔符且空正文保持零语料() {
    let root = TestDir::new("document_context_output_budget");
    let mut db = Database::<f32>::open(&root.file("data.tdb"), 2).unwrap();
    let snapshot = db
        .put_document(
            DocumentWrite {
                source_id: "notes://empty".into(),
                source_revision: "v1".into(),
                parser_id: "plain-v1".into(),
                content: String::new(),
                root: DocumentNode {
                    id: 10,
                    vector: vec![1., 0.],
                    payload: Value::Null,
                },
                // 两千个键让数组分隔符开销超过固定结构预留，独立核对实际编码上限。
                blocks: (0..2048)
                    .map(|i| DocumentBlock {
                        key: format!("key-{i}"),
                        parent: None,
                        kind: "section".into(),
                        start_byte: 0,
                        end_byte: 0,
                        node: None,
                    })
                    .collect(),
            },
            None,
        )
        .unwrap();
    let empty = db.search_documents(&query()).unwrap();
    assert_eq!(
        (
            empty.corpus_documents,
            empty.corpus_units,
            empty.matched_units
        ),
        (1, 0, 0)
    );
    assert!(empty.groups.is_empty());
    let mut query = DocumentContextQuery {
        seeds: vec![seed(&snapshot, "key-0", 0, 0)],
        before: 0,
        after: usize::MAX,
        ..Default::default()
    };
    let context = db.document_context(&query).unwrap();
    assert_eq!(context.len(), 1);
    assert_eq!(
        context[0].block_keys,
        (0..2048).map(|i| format!("key-{i}")).collect::<Vec<_>>()
    );
    assert_eq!(context[0].windows.len(), 1);
    assert_eq!(context[0].windows[0].text, "");
    query.limits.max_return_bytes = serde_json::to_vec(&context).unwrap().len() - 1;
    assert_budget(
        db.document_context(&query).unwrap_err(),
        "document_return_bytes",
    );
    db.close().unwrap();
}
