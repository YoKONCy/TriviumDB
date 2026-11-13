//! 位置倒排、短语、有序邻近与原文证据的独立公共契约。
use super::support::TestDir;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use triviumdb::database::{AccessMode, Config, StorageMode};
use triviumdb::document::*;
use triviumdb::{Database, ResourceRuntime, VectorType};

fn contract() -> Value {
    serde_json::from_str(include_str!("../contracts/document_positions_cases.json")).unwrap()
}

fn config(mode: StorageMode) -> Config {
    Config {
        dim: 2,
        storage_mode: mode,
        auto_build_quiver: false,
        ..Default::default()
    }
}

fn populate<T: VectorType + Serialize + DeserializeOwned>(
    db: &mut Database<T>,
) -> Vec<DocumentSnapshot> {
    contract()["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|document| {
            db.put_document(serde_json::from_value(document.clone()).unwrap(), None)
                .unwrap()
        })
        .collect()
}

fn query(case: &Value) -> DocumentSearchQuery {
    let mut request = json!({"level":"document","include_matches":true,"max_documents":10,"hits_per_document":10});
    request
        .as_object_mut()
        .unwrap()
        .extend(case["query"].as_object().unwrap().clone());
    serde_json::from_value(request).unwrap()
}

fn expected_matches(case: &Value, snapshot: &DocumentSnapshot) -> Value {
    let fixture = contract();
    let index = fixture["documents"]
        .as_array()
        .unwrap()
        .iter()
        .position(|row| {
            row["root"]["id"].as_str().unwrap().parse::<u64>().unwrap() == snapshot.document_id
        })
        .unwrap();
    Value::Array(case["matches"].as_array().unwrap().iter().map(|expected|json!({
        "evidence":{
            "reference":{"document_id":fixture["documents"][index]["root"]["id"],"revision":snapshot.revision,"start_byte":expected["start_byte"],"end_byte":expected["end_byte"]},
            "source_id":fixture["documents"][index]["source_id"],"source_revision":"v1","parser_id":"plain-v1","content_hash":fixture["hashes"][index],
            "text":expected["text"],"line":expected["line"],"column":expected["column"],"end_line":expected["end_line"],"end_column":expected["end_column"]
        },"terms":expected["terms"]
    })).collect())
}

fn assert_case<T: VectorType + Serialize + DeserializeOwned>(
    db: &Database<T>,
    case: &Value,
    snapshots: &[DocumentSnapshot],
) -> DocumentSearchResult {
    let result = db.search_documents(&query(case)).unwrap();
    assert_eq!(result.analyzer_id, contract()["analyzer_id"]);
    assert_eq!(
        (result.groups.len(), result.matched_units),
        (1, 1),
        "{}",
        case["name"]
    );
    assert_eq!(result.groups[0].hits.len(), 1, "{}", case["name"]);
    let snapshot = snapshots
        .iter()
        .find(|row| row.document_id == result.groups[0].document_id)
        .unwrap();
    let hit = &result.groups[0].hits[0];
    assert_eq!(
        hit.node_id,
        case["node_id"].as_str().unwrap().parse::<u64>().unwrap()
    );
    assert_eq!(
        serde_json::to_value(&hit.matches).unwrap(),
        expected_matches(case, snapshot),
        "{}",
        case["name"]
    );
    assert!(hit.score > 0.);
    result
}

fn roundtrip<T: VectorType + Serialize + DeserializeOwned>() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("document_positions_contract");
        let path = root.file("data.tdb");
        let mut db = Database::<T>::open_with_config(&path, config(mode)).unwrap();
        let snapshots = populate(&mut db);
        let fixture = contract();
        for case in fixture["cases"].as_array().unwrap() {
            let expected = assert_case(&db, case, &snapshots);
            let mut uncached = query(case);
            uncached.use_cache = false;
            assert_eq!(db.search_documents(&uncached).unwrap(), expected);
            db.clear_document_index().unwrap();
            assert_eq!(db.search_documents(&query(case)).unwrap(), expected);
        }
        let retained = assert_case(&db, &fixture["cases"][0], &snapshots);
        let mut legacy = query(&fixture["cases"][0]);
        legacy.include_matches = false;
        assert!(
            db.search_documents(&legacy).unwrap().groups[0].hits[0]
                .matches
                .is_empty()
        );
        let mut seed = DocumentContextSeed {
            reference: retained.groups[0].hits[0].matches[1]
                .evidence
                .reference
                .clone(),
            block_key: Some("second".into()),
        };
        let context = db
            .document_context(&DocumentContextQuery {
                seeds: vec![seed.clone()],
                before: 0,
                after: 0,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(context[0].windows[0].text, "alpha beta beta");
        assert_eq!(context[0].windows[0].reference.start_byte, 26);
        seed.reference.revision = "stale".into();
        assert_eq!(
            db.document_context(&DocumentContextQuery {
                seeds: vec![seed],
                ..Default::default()
            })
            .unwrap_err()
            .code(),
            "TDB_DOCUMENT_REVISION_MISMATCH"
        );
        db.close().unwrap();
        let before = files(&root.0);
        let mut db = Database::<T>::open_with_config(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                load_text_index: false,
                ..config(mode)
            },
        )
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            assert_case(&db, case, &snapshots);
        }
        db.close().unwrap();
        assert_eq!(files(&root.0), before);
        let moved = root.0.join("moved");
        std::fs::create_dir(&moved).unwrap();
        for name in before.keys() {
            std::fs::rename(root.0.join(name), moved.join(name)).unwrap();
        }
        assert_eq!(
            retained.groups[0].hits[0].matches[0].evidence.text,
            "Alpha, beta"
        );
        let mut db =
            Database::<T>::open_with_config(moved.join("data.tdb").to_str().unwrap(), config(mode))
                .unwrap();
        assert_eq!(assert_case(&db, &fixture["cases"][0], &snapshots), retained);
        db.close().unwrap();
    }
}

fn files(path: &std::path::Path) -> std::collections::BTreeMap<std::ffi::OsString, Vec<u8>> {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect()
}

#[test]
fn 三种dtype两种存储模式的短语邻近与Unicode原文契约() {
    roundtrip::<f32>();
    roundtrip::<half::f16>();
    roundtrip::<u64>();
}

#[test]
fn 位置参数预算和无词项输入按结构化错误拒绝且归还工作区() {
    let root = TestDir::new("document_positions_limits");
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
    populate(&mut db);
    let request = query(&contract()["cases"][0]);
    for resource in [
        "document_positions",
        "document_query_terms",
        "document_position_visits",
        "document_matches",
        "document_return_bytes",
    ] {
        let mut limited = request.clone();
        match resource {
            "document_positions" => limited.limits.max_positions = 0,
            "document_query_terms" => limited.limits.max_query_terms = 1,
            "document_position_visits" => limited.limits.max_position_visits = 0,
            "document_matches" => limited.limits.max_matches = 1,
            "document_return_bytes" => limited.limits.max_return_bytes = 1,
            _ => unreachable!(),
        }
        for warm in [false, true] {
            db.clear_document_index().unwrap();
            if warm {
                db.search_documents(&request).unwrap();
            }
            let error = db.search_documents(&limited).unwrap_err();
            assert_eq!(error.code(), "QueueFull");
            assert_eq!(error.details()["resource"], resource, "{error:?}");
            assert!(error.info().write.is_none());
            assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        }
    }
    for (field, mutated) in [
        ("query.analyzer_id", json!({"analyzer_id":"unknown"})),
        ("query.max_gap", json!({"max_gap":1})),
        ("query.text", json!({"text":"!!!"})),
    ] {
        let mut value = serde_json::to_value(&request).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .extend(mutated.as_object().unwrap().clone());
        let query = serde_json::from_value(value).unwrap();
        let error = db.search_documents(&query).unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT");
        assert_eq!(
            serde_json::to_value(error.info().target).unwrap(),
            json!({"field":field})
        );
    }
    let malformed =
        json!({"operation":"search","query":{"text":"alpha","match_mode":"unordered_near"}})
            .to_string();
    let error = triviumdb::sdk::documents::request(&mut db, &malformed).unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        serde_json::to_value(error.info().target).unwrap(),
        json!({"field":"request_json"})
    );
    let punctuation = DocumentSearchQuery {
        text: "!!!".into(),
        ..Default::default()
    };
    assert!(db.search_documents(&punctuation).unwrap().groups.is_empty());
    let mut without_details = request.clone();
    without_details.include_matches = false;
    without_details.limits.max_matches = 0;
    assert_eq!(
        db.search_documents(&without_details).unwrap().groups.len(),
        1
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    db.close().unwrap();
    assert_eq!(runtime.stats().reserved_database_bytes, 0);
}

fn model_matches(words: &[&str], query: &[&str], gap: usize) -> Vec<Vec<usize>> {
    // 小输入穷举所有递增下标组合，以末位置和整条词序独立选择每个起点的最短链。
    fn enumerate(words: &[&str], query: &[&str], path: &mut Vec<usize>, all: &mut Vec<Vec<usize>>) {
        if path.len() == query.len() {
            all.push(path.clone());
            return;
        }
        let start = path.last().map_or(0, |position| position + 1);
        for index in start..words.len() {
            if words[index] == query[path.len()] {
                path.push(index);
                enumerate(words, query, path, all);
                path.pop();
            }
        }
    }
    let mut all = Vec::new();
    enumerate(words, query, &mut Vec::new(), &mut all);
    let mut best = std::collections::BTreeMap::<usize, Vec<usize>>::new();
    for path in all {
        let last = *path.last().unwrap();
        if last - path[0] - (query.len() - 1) > gap {
            continue;
        }
        let entry = best.entry(path[0]).or_insert_with(|| path.clone());
        if (last, &path) < (*entry.last().unwrap(), &*entry) {
            *entry = path;
        }
    }
    best.into_values().collect()
}

#[test]
fn 有序位置查询对小语料穷举组合保持完整窗口及确定词序() {
    let root = TestDir::new("document_positions_differential");
    let mut db =
        Database::<f32>::open_with_config(&root.file("data.tdb"), config(StorageMode::Mmap))
            .unwrap();
    let mut previous: Option<String> = None;
    let mut seed = 0x2874_6af1_u64;
    for sample in 0..48 {
        let count = sample % 7;
        let words = (0..count)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                ["a", "b", "c"][(seed >> 32) as usize % 3]
            })
            .collect::<Vec<_>>();
        let content = words.join(" ");
        let snapshot = db
            .put_document(
                DocumentWrite {
                    source_id: "notes://model".into(),
                    source_revision: sample.to_string(),
                    parser_id: "plain-v1".into(),
                    content: content.clone(),
                    root: DocumentNode {
                        id: 10,
                        vector: vec![1., 0.],
                        payload: Value::Null,
                    },
                    blocks: vec![],
                },
                previous.as_deref(),
            )
            .unwrap();
        previous = Some(snapshot.revision);
        for terms in [
            vec!["a"],
            vec!["a", "a"],
            vec!["a", "b"],
            vec!["b", "a", "c"],
            vec!["a", "b", "a"],
        ] {
            for gap in 0..=2 {
                let expected = model_matches(&words, &terms, gap);
                let result = db
                    .search_documents(&DocumentSearchQuery {
                        text: terms.join(" "),
                        level: DocumentSearchLevel::Document,
                        match_mode: DocumentMatchMode::OrderedNear,
                        max_gap: gap,
                        include_matches: true,
                        ..Default::default()
                    })
                    .unwrap();
                let actual = result
                    .groups
                    .iter()
                    .flat_map(|group| group.hits.iter())
                    .flat_map(|hit| hit.matches.iter())
                    .collect::<Vec<_>>();
                assert_eq!(
                    actual.len(),
                    expected.len(),
                    "sample={sample},words={words:?},query={terms:?},gap={gap}"
                );
                for (actual, path) in actual.into_iter().zip(expected) {
                    let first = path[0];
                    let last = *path.last().unwrap();
                    assert_eq!(actual.evidence.reference.start_byte, 2 * first);
                    assert_eq!(actual.evidence.reference.end_byte, 2 * last + 1);
                    assert_eq!(actual.evidence.text, words[first..=last].join(" "));
                    assert_eq!(
                        (
                            actual.evidence.line,
                            actual.evidence.column,
                            actual.evidence.end_line,
                            actual.evidence.end_column
                        ),
                        (1, 2 * first + 1, 1, 2 * last + 2)
                    );
                    assert_eq!(
                        actual
                            .terms
                            .iter()
                            .map(|term| (
                                term.term.as_str(),
                                term.position,
                                term.start_byte,
                                term.end_byte
                            ))
                            .collect::<Vec<_>>(),
                        path.iter()
                            .map(|&position| (
                                words[position],
                                position,
                                2 * position,
                                2 * position + 1
                            ))
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
    }
    db.close().unwrap();
}

#[test]
fn 邻近后继选择覆盖前一个双字词无法成链的情形() {
    let root = TestDir::new("document_positions_suffix");
    let mut db = Database::<f32>::open(&root.file("data.tdb"), 2).unwrap();
    db.put_document(
        DocumentWrite {
            source_id: "notes://suffix".into(),
            source_revision: "v1".into(),
            parser_id: "plain-v1".into(),
            content: "x 甲乙 a 甲乙丙".into(),
            root: DocumentNode {
                id: 10,
                vector: vec![1., 0.],
                payload: Value::Null,
            },
            blocks: vec![],
        },
        None,
    )
    .unwrap();
    let query = DocumentSearchQuery {
        text: "x 甲乙丙".into(),
        level: DocumentSearchLevel::Document,
        match_mode: DocumentMatchMode::OrderedNear,
        max_gap: 2,
        include_matches: true,
        ..Default::default()
    };
    let result = db.search_documents(&query).unwrap();
    assert_eq!(result.groups[0].hits[0].matches.len(), 1);
    let matched = &result.groups[0].hits[0].matches[0];
    assert_eq!(matched.evidence.text, "x 甲乙 a 甲乙丙");
    assert_eq!(
        matched
            .terms
            .iter()
            .map(|term| term.position)
            .collect::<Vec<_>>(),
        [0, 3, 4]
    );
    assert!(
        db.search_documents(&DocumentSearchQuery {
            max_gap: 1,
            ..query
        })
        .unwrap()
        .groups
        .is_empty()
    );
    db.close().unwrap();
}

#[test]
fn 短语过滤在分组截断前执行并沿用完整语料BM25排序() {
    let root = TestDir::new("document_positions_before_topk");
    let mut db =
        Database::<f32>::open_with_config(&root.file("data.tdb"), config(StorageMode::Mmap))
            .unwrap();
    for (id, content) in [(1, "a a a x b b b"), (2, "a b")] {
        db.put_document(
            DocumentWrite {
                source_id: format!("notes://{id}"),
                source_revision: "v1".into(),
                parser_id: "plain-v1".into(),
                content: content.into(),
                root: DocumentNode {
                    id,
                    vector: vec![1., 0.],
                    payload: Value::Null,
                },
                blocks: vec![],
            },
            None,
        )
        .unwrap();
    }
    let query = DocumentSearchQuery {
        text: "a b".into(),
        level: DocumentSearchLevel::Document,
        max_documents: 1,
        ..Default::default()
    };
    assert_eq!(
        db.search_documents(&query).unwrap().groups[0].document_id,
        1
    );
    let phrase = db
        .search_documents(&DocumentSearchQuery {
            match_mode: DocumentMatchMode::Phrase,
            include_matches: true,
            ..query
        })
        .unwrap();
    assert_eq!((phrase.groups.len(), phrase.matched_units), (1, 1));
    assert_eq!(phrase.groups[0].document_id, 2);
    assert_eq!(phrase.groups[0].hits[0].matches[0].evidence.text, "a b");
    db.close().unwrap();
}

#[test]
fn 重复词查询在动态规划大分配前检查访问预算() {
    let root = TestDir::new("document_positions_adversarial");
    let mut db = Database::<f32>::open_with_runtime(
        &root.file("data.tdb"),
        Config {
            memory_limit: 1024 * 1024,
            ..config(StorageMode::Mmap)
        },
        ResourceRuntime::new(Default::default()).unwrap(),
    )
    .unwrap();
    db.put_document(
        DocumentWrite {
            source_id: "notes://repeated".into(),
            source_revision: "v1".into(),
            parser_id: "plain-v1".into(),
            content: vec!["a"; 512].join(" "),
            root: DocumentNode {
                id: 10,
                vector: vec![1., 0.],
                payload: Value::Null,
            },
            blocks: vec![],
        },
        None,
    )
    .unwrap();
    let mut query = DocumentSearchQuery {
        text: vec!["a"; 32].join(" "),
        level: DocumentSearchLevel::Document,
        match_mode: DocumentMatchMode::OrderedNear,
        max_gap: usize::MAX,
        ..Default::default()
    };
    query.limits.max_position_visits = 64;
    let error = db.search_documents(&query).unwrap_err();
    assert_eq!(error.code(), "QueueFull");
    assert_eq!(error.details()["resource"], "document_position_visits");
    assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
    db.close().unwrap();
}

#[test]
fn 普通词频缓存按需升级位置表且冷热缓存采用相同位置预算语义() {
    let root = TestDir::new("document_positions_lazy_cache");
    let mut db = Database::<f32>::open_with_runtime(
        &root.file("data.tdb"),
        config(StorageMode::Mmap),
        ResourceRuntime::new(Default::default()).unwrap(),
    )
    .unwrap();
    populate(&mut db);
    let mut plain = query(&contract()["cases"][0]);
    plain.match_mode = DocumentMatchMode::Any;
    plain.include_matches = false;
    plain.limits.max_positions = 0;
    let expected = db.search_documents(&plain).unwrap();
    assert!(expected.groups[0].hits[0].matches.is_empty());
    let before = db.index_memory_stats().hot_bytes;
    let detailed = query(&contract()["cases"][0]);
    let precise = db.search_documents(&detailed).unwrap();
    assert_eq!(precise.groups[0].hits[0].matches.len(), 2);
    assert!(db.index_memory_stats().hot_bytes > before);
    assert_eq!(db.search_documents(&plain).unwrap(), expected);
    let mut uncached = plain.clone();
    uncached.use_cache = false;
    assert_eq!(db.search_documents(&uncached).unwrap(), expected);
    db.clear_document_index().unwrap();
    assert_eq!(db.search_documents(&plain).unwrap(), expected);
    let mut no_positions = detailed.clone();
    no_positions.limits.max_positions = 0;
    assert_eq!(
        db.search_documents(&no_positions).unwrap_err().details()["resource"],
        "document_positions"
    );
    assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
    assert_eq!(db.search_documents(&plain).unwrap(), expected);
    assert_eq!(db.search_documents(&detailed).unwrap(), precise);
    db.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 位置查询实际执行中的取消归还工作区并保留可重试状态() {
    let root = TestDir::new("document_positions_cancel");
    let mut db = Database::<f32>::open_with_runtime(
        &root.file("data.tdb"),
        config(StorageMode::Mmap),
        ResourceRuntime::new(Default::default()).unwrap(),
    )
    .unwrap();
    populate(&mut db);
    let query = query(&contract()["cases"][0]);
    let expected = db.search_documents(&query).unwrap();
    let context = triviumdb::sdk::RequestContext::new(
        "position-query",
        triviumdb::query::tql_executor::QueryControl::default(),
        false,
    )
    .unwrap();
    let db = std::sync::Arc::new(db);
    let gate = triviumdb::test_hooks::pause_at(
        triviumdb::test_hooks::ConcurrencyPoint::DocumentPositionStep,
    );
    let worker_db = db.clone();
    let worker_query = query.clone();
    let worker_context = context.clone();
    let worker = std::thread::spawn(move || {
        worker_context.run(|| worker_db.search_documents(&worker_query))
    });
    gate.wait_until_arrived();
    context.cancel();
    drop(gate);
    assert_eq!(
        worker.join().unwrap().unwrap_err().code(),
        "TDB_QUERY_CANCELLED"
    );
    assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
    assert_eq!(db.search_documents(&query).unwrap(), expected);
    let mut db = std::sync::Arc::into_inner(db).unwrap();
    db.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn WAL失败后的旧位置视图和重开后的新位置事实保持完整() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("document_positions_wal");
        let path = root.file("data.tdb");
        let config = Config {
            sync_mode: triviumdb::storage::wal::SyncMode::Full,
            ..config(mode)
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        let snapshots = populate(&mut db);
        db.flush().unwrap();
        let request = query(&contract()["cases"][0]);
        let old = db.search_documents(&request).unwrap();
        let mut changed: DocumentWrite<f32> =
            serde_json::from_value(contract()["documents"][0].clone()).unwrap();
        changed.content = changed
            .content
            .replace("Alpha", "Delta")
            .replace("alpha", "delta");
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
        assert_eq!(db.search_documents(&request).unwrap(), old);
        drop(db);
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        assert!(db.search_documents(&request).unwrap().groups.is_empty());
        let new_request = DocumentSearchQuery {
            text: "delta beta".into(),
            ..request
        };
        let recovered = db.search_documents(&new_request).unwrap();
        assert_eq!(recovered.groups.len(), 1);
        assert_eq!(recovered.groups[0].source_revision, "v2");
        assert_ne!(recovered.groups[0].revision, snapshots[0].revision);
        assert_eq!(
            recovered.groups[0].hits[0]
                .matches
                .iter()
                .map(|found| (
                    found.evidence.text.as_str(),
                    found.evidence.reference.start_byte,
                    found.evidence.reference.end_byte
                ))
                .collect::<Vec<_>>(),
            [("Delta, beta", 0, 11), ("delta beta", 26, 36)]
        );
        assert_eq!(
            db.resolve_document_reference(
                old.groups[0].hits[0].matches[0].evidence.reference.clone()
            )
            .unwrap_err()
            .code(),
            "TDB_DOCUMENT_REVISION_MISMATCH"
        );
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        assert_eq!(db.search_documents(&new_request).unwrap(), recovered);
        db.delete_document(1000, &recovered.groups[0].revision)
            .unwrap();
        assert!(db.search_documents(&new_request).unwrap().groups.is_empty());
        db.close().unwrap();
    }
}
