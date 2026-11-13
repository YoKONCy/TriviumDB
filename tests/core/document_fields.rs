//! 字段语料和文档／片段成员过滤的独立公共契约。
use super::support::TestDir;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use triviumdb::database::{AccessMode, Config, StorageMode};
use triviumdb::document::*;
use triviumdb::{Database, ResourceRuntime, VectorType};

fn contract() -> Value {
    serde_json::from_str(include_str!("../contracts/document_fields_cases.json")).unwrap()
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
    let mut value = contract()["query"].clone();
    let extra = case
        .get("query_json")
        .map(|raw| serde_json::from_str(raw.as_str().unwrap()).unwrap())
        .unwrap_or_else(|| case["query"].clone());
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}

fn ids(result: &DocumentSearchResult) -> Value {
    json!({
        "documents":result.groups.iter().map(|group| group.document_id.to_string()).collect::<Vec<_>>(),
        "hits":result.groups.iter().map(|group| group.hits.iter().map(|hit| hit.node_id.to_string()).collect::<Vec<_>>()).collect::<Vec<_>>()
    })
}

fn assert_case<T: VectorType + Serialize + DeserializeOwned>(
    db: &Database<T>,
    case: &Value,
    snapshots: &[DocumentSnapshot],
) -> DocumentSearchResult {
    let fixture = contract();
    let request = query(case);
    let result = db.search_documents(&request).unwrap();
    assert_eq!(
        ids(&result),
        json!({"documents":case["documents"],"hits":case["hits"]}),
        "{}",
        case["name"]
    );
    let expected_matched = if case["name"] == "filter-before-top-one" {
        1
    } else {
        case["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hits| hits.as_array().unwrap().len())
            .sum()
    };
    assert_eq!(result.matched_units, expected_matched, "{}", case["name"]);
    assert_eq!((result.corpus_documents, result.corpus_units), (4, 7));
    if case["name"] == "unknown-field" {
        assert_eq!(
            result.field_corpora,
            vec![DocumentFieldCorpus {
                kind: "absent".into(),
                weight: 1.,
                corpus_units: 0,
                average_length: 0.
            }]
        );
    } else {
        assert_eq!(result.field_corpora.len(), 2);
        for (actual, expected) in result
            .field_corpora
            .iter()
            .zip(fixture["field_corpora"].as_array().unwrap())
        {
            assert_eq!(actual.kind, expected["kind"]);
            assert_eq!(actual.weight as f64, expected["weight"].as_f64().unwrap());
            assert_eq!(
                actual.corpus_units as u64,
                expected["corpus_units"].as_u64().unwrap()
            );
            assert!(
                (actual.average_length as f64 - expected["average_length"].as_f64().unwrap()).abs()
                    < 1e-6
            );
        }
    }
    for group in &result.groups {
        let document_id = group.document_id.to_string();
        let document = fixture["documents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["root"]["id"].as_str() == Some(document_id.as_str()))
            .unwrap();
        let snapshot = snapshots
            .iter()
            .find(|row| row.document_id == group.document_id)
            .unwrap();
        assert_eq!(group.revision, snapshot.revision);
        assert_eq!(group.source_id, document["source_id"]);
        assert_eq!(group.source_revision, "v1");
        assert_eq!(group.score, group.hits[0].score);
        for hit in &group.hits {
            let node_id = hit.node_id.to_string();
            let block = document["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["node"]["id"].as_str() == Some(node_id.as_str()))
                .unwrap();
            let start = block["start_byte"].as_u64().unwrap() as usize;
            let end = block["end_byte"].as_u64().unwrap() as usize;
            assert_eq!(hit.kind, block["kind"]);
            assert_eq!(hit.block_key.as_deref(), block["key"].as_str());
            assert_eq!(
                hit.evidence.text,
                &document["content"].as_str().unwrap()[start..end]
            );
            assert_eq!(
                hit.evidence.reference,
                DocumentReference {
                    document_id: group.document_id,
                    revision: snapshot.revision.clone(),
                    start_byte: start,
                    end_byte: end
                }
            );
            assert_eq!(hit.evidence.source_id, document["source_id"]);
            assert_eq!(hit.evidence.source_revision, "v1");
            assert_eq!(hit.evidence.parser_id, "ranges-v1");
            if request.text == "alpha" {
                let expected = fixture["scores"][hit.node_id.to_string()].as_f64().unwrap();
                assert!(
                    (hit.score as f64 - expected).abs() < 1e-6,
                    "id={},score={},expected={expected}",
                    hit.node_id,
                    hit.score
                );
                assert!(hit.matches.is_empty());
            } else {
                assert_eq!(hit.matches.len(), 1);
                assert_eq!(hit.matches[0].evidence.text, "alpha beta");
                assert_eq!(hit.matches[0].evidence.reference.start_byte, 0);
                assert_eq!(hit.matches[0].evidence.reference.end_byte, 10);
                assert_eq!(
                    serde_json::to_value(&hit.matches[0].terms).unwrap(),
                    json!([
                        {"term":"alpha","position":0,"start_byte":0,"end_byte":5},
                        {"term":"beta","position":1,"start_byte":6,"end_byte":10}
                    ])
                );
            }
        }
    }
    result
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

fn roundtrip<T: VectorType + Serialize + DeserializeOwned>() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("document_fields_contract");
        let path = root.file("data.tdb");
        let mut db = Database::<T>::open_with_config(&path, config(mode)).unwrap();
        let snapshots = populate(&mut db);
        let fixture = contract();
        for case in fixture["cases"].as_array().unwrap() {
            let expected = assert_case(&db, case, &snapshots);
            assert_eq!(db.search_documents(&query(case)).unwrap(), expected);
            let mut uncached = query(case);
            uncached.use_cache = false;
            assert_eq!(db.search_documents(&uncached).unwrap(), expected);
            db.clear_document_index().unwrap();
            assert_eq!(assert_case(&db, case, &snapshots), expected);
        }
        let retained = assert_case(&db, &fixture["cases"][1], &snapshots);
        db.close().unwrap();
        let before = files(&root.0);
        let mut readonly = Database::<T>::open_with_config(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                ..config(mode)
            },
        )
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            assert_case(&readonly, case, &snapshots);
        }
        readonly.close().unwrap();
        let mut reader = triviumdb::DatabaseReader::<T>::open_with_config(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                ..config(mode)
            },
        )
        .unwrap();
        assert_eq!(
            reader
                .search_documents(&query(&fixture["cases"][1]))
                .unwrap(),
            retained
        );
        reader.close().unwrap();
        assert_eq!(files(&root.0), before);
        let moved = root.0.join("moved");
        std::fs::create_dir(&moved).unwrap();
        for name in before.keys() {
            std::fs::rename(root.0.join(name), moved.join(name)).unwrap();
        }
        assert_eq!(retained.groups[0].hits[0].evidence.text, "beta alpha");
        let mut reopened =
            Database::<T>::open_with_config(moved.join("data.tdb").to_str().unwrap(), config(mode))
                .unwrap();
        assert_eq!(
            assert_case(&reopened, &fixture["cases"][1], &snapshots),
            retained
        );
        reopened.close().unwrap();
    }
}

#[test]
fn 三种dtype两种存储模式的字段评分与成员作用域保持独立预期() {
    roundtrip::<f32>();
    roundtrip::<half::f16>();
    roundtrip::<u64>();
}

#[test]
fn 字段与过滤参数拒绝提供稳定字段且归还工作区() {
    let root = TestDir::new("document_fields_invalid");
    let mut db = Database::<f32>::open_with_runtime(
        &root.file("data.tdb"),
        config(StorageMode::Mmap),
        ResourceRuntime::new(Default::default()).unwrap(),
    )
    .unwrap();
    populate(&mut db);
    for case in contract()["invalid"].as_array().unwrap() {
        let error = db.search_documents(&query(case)).unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT");
        assert_eq!(
            serde_json::to_value(error.info().target.unwrap()).unwrap(),
            json!({"field":case["field"]})
        );
        assert!(error.info().write.is_none());
        assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
    }
    for weight in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut request = query(&contract()["cases"][0]);
        request.fields[0].weight = weight;
        assert_eq!(
            db.search_documents(&request).unwrap_err().code(),
            "TDB_INVALID_INPUT"
        );
    }
    let mut overflow = query(&contract()["cases"][0]);
    overflow.text = "beta".into();
    overflow.fields = vec![DocumentSearchField {
        kind: "body".into(),
        weight: f32::MAX,
    }];
    let error = db.search_documents(&overflow).unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        serde_json::to_value(error.info().target.unwrap()).unwrap(),
        json!({"field":"query.fields"})
    );
    assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
    db.close().unwrap();
}

#[test]
fn 整篇成员过滤与旧结果解码兼容且片段词项边界独立() {
    let root = TestDir::new("document_fields_document_level");
    let mut db =
        Database::<f32>::open_with_config(&root.file("data.tdb"), config(StorageMode::Mmap))
            .unwrap();
    populate(&mut db);
    let mut request = query(&contract()["cases"][2]);
    request.fields.clear();
    request.level = DocumentSearchLevel::Document;
    request.text = "alpha beta".into();
    request.match_mode = DocumentMatchMode::Phrase;
    request.include_matches = true;
    let result = db.search_documents(&request).unwrap();
    assert_eq!(ids(&result), json!({"documents":["10"],"hits":[["10"]]}));
    assert_eq!(result.corpus_units, 4);
    assert!(result.field_corpora.is_empty());
    assert_eq!(result.groups[0].hits[0].evidence.text, "alpha\nbeta alpha");
    assert_eq!(result.groups[0].hits[0].matches.len(), 1);
    assert_eq!(
        result.groups[0].hits[0].matches[0].evidence.text,
        "alpha\nbeta"
    );
    assert_eq!(
        result.groups[0].hits[0].matches[0]
            .evidence
            .reference
            .start_byte,
        0
    );
    assert_eq!(
        result.groups[0].hits[0].matches[0]
            .evidence
            .reference
            .end_byte,
        10
    );
    let encoded = serde_json::to_value(&result).unwrap();
    assert!(encoded.get("field_corpora").is_none());
    assert_eq!(
        serde_json::from_value::<DocumentSearchResult>(encoded).unwrap(),
        result
    );
    db.close().unwrap();
}

#[test]
fn 成员派生缓存按需升级并在冷热预算拒绝后可重试() {
    let root = TestDir::new("document_fields_budget");
    let mut db = Database::<f32>::open_with_runtime(
        &root.file("data.tdb"),
        config(StorageMode::Mmap),
        ResourceRuntime::new(Default::default()).unwrap(),
    )
    .unwrap();
    populate(&mut db);
    let plain = DocumentSearchQuery {
        text: "alpha".into(),
        ..Default::default()
    };
    let before = db.search_documents(&plain).unwrap();
    let hot = db.index_memory_stats().hot_bytes;
    let request = query(&contract()["cases"][11]);
    let expected = db.search_documents(&request).unwrap();
    assert!(db.index_memory_stats().hot_bytes > hot);
    assert_eq!(db.search_documents(&plain).unwrap(), before);
    for resource in [
        "document_member_values",
        "document_filter_candidates",
        "document_postings",
        "document_candidates",
        "document_return_bytes",
    ] {
        for cache in [true, false] {
            for cold in [true, false] {
                if cold {
                    db.clear_document_index().unwrap();
                } else {
                    db.search_documents(&request).unwrap();
                }
                let mut limited = request.clone();
                limited.use_cache = cache;
                match resource {
                    "document_member_values" => limited.limits.max_member_values = 0,
                    "document_filter_candidates" => limited.limits.max_filter_candidates = 0,
                    "document_postings" => limited.limits.max_postings = 0,
                    "document_candidates" => limited.limits.max_candidates = 0,
                    "document_return_bytes" => limited.limits.max_return_bytes = 0,
                    _ => unreachable!(),
                }
                let error = db.search_documents(&limited).unwrap_err();
                assert_eq!(error.code(), "QueueFull");
                assert_eq!(error.details()["resource"], resource);
                assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
                assert_eq!(db.search_documents(&request).unwrap(), expected);
            }
        }
    }
    let mut rejected = request.clone();
    rejected.document_filter = Some(json!({"vault":"absent"}));
    rejected.limits.max_candidates = 0;
    assert!(db.search_documents(&rejected).unwrap().groups.is_empty());
    db.close().unwrap();
}

#[test]
fn 元数据原位修改与文档替换删除使候选失效且恢复后保持新事实() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("document_fields_mutation");
        let path = root.file("data.tdb");
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        let snapshots = populate(&mut db);
        let request = query(&contract()["cases"][1]);
        assert_case(&db, &contract()["cases"][1], &snapshots);
        let mut payload = db.get_payload(12).unwrap().unwrap();
        payload["data"]["tags"] = json!(["changed"]);
        db.update_payload(12, payload).unwrap();
        assert!(db.search_documents(&request).unwrap().groups.is_empty());
        let mut changed: DocumentWrite<f32> =
            serde_json::from_value(contract()["documents"][0].clone()).unwrap();
        changed.source_revision = "v2".into();
        changed.root.payload["vault"] = json!("green");
        changed.blocks[0].kind = "body".into();
        let updated = db
            .put_document(changed, Some(&snapshots[0].revision))
            .unwrap();
        let mut current = request.clone();
        current.document_filter = Some(json!({"vault":"green"}));
        let result = db.search_documents(&current).unwrap();
        assert_eq!(ids(&result), json!({"documents":["10"],"hits":[["12"]]}));
        assert_eq!(result.groups[0].source_revision, "v2");
        assert_eq!(
            result
                .field_corpora
                .iter()
                .map(|field| (field.kind.as_str(), field.corpus_units))
                .collect::<Vec<_>>(),
            vec![("body", 5), ("title", 2)]
        );
        let mut stale = current.clone();
        stale
            .expected_revisions
            .insert(10, snapshots[0].revision.clone());
        stale.document_filter = Some(json!({"vault":"absent"}));
        assert_eq!(
            db.search_documents(&stale).unwrap_err().code(),
            "TDB_DOCUMENT_REVISION_MISMATCH"
        );
        db.flush().unwrap();
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(db.search_documents(&current).unwrap(), result);
        db.delete_document(10, &updated.revision).unwrap();
        assert!(db.search_documents(&current).unwrap().groups.is_empty());
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        assert!(db.search_documents(&current).unwrap().groups.is_empty());
        db.flush().unwrap();
        db.close().unwrap();
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn 成员posting执行中的取消归还资源且可重试() {
    let root = TestDir::new("document_members_cancel");
    let mut db = Database::<f32>::open_with_runtime(
        &root.file("data.tdb"),
        config(StorageMode::Mmap),
        ResourceRuntime::new(Default::default()).unwrap(),
    )
    .unwrap();
    populate(&mut db);
    let request = query(&contract()["cases"][2]);
    let expected = db.search_documents(&request).unwrap();
    let context = triviumdb::sdk::RequestContext::new(
        "member-query",
        triviumdb::query::tql_executor::QueryControl::default(),
        false,
    )
    .unwrap();
    let db = std::sync::Arc::new(db);
    let gate = triviumdb::test_hooks::pause_at(
        triviumdb::test_hooks::ConcurrencyPoint::DocumentMemberStep,
    );
    let worker_db = db.clone();
    let worker_request = request.clone();
    let worker_context = context.clone();
    let worker = std::thread::spawn(move || {
        worker_context.run(|| worker_db.search_documents(&worker_request))
    });
    gate.wait_until_arrived();
    context.cancel();
    drop(gate);
    assert_eq!(
        worker.join().unwrap().unwrap_err().code(),
        "TDB_QUERY_CANCELLED"
    );
    assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
    assert_eq!(db.search_documents(&request).unwrap(), expected);
    let mut db = std::sync::Arc::into_inner(db).unwrap();
    db.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn WAL未知结果保留旧成员视图且重开恢复全部新字段事实() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("document_members_wal");
        let path = root.file("data.tdb");
        let config = Config {
            sync_mode: triviumdb::storage::wal::SyncMode::Full,
            ..config(mode)
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        let snapshots = populate(&mut db);
        db.flush().unwrap();
        let request = query(&contract()["cases"][2]);
        let old = db.search_documents(&request).unwrap();
        let mut changed: DocumentWrite<f32> =
            serde_json::from_value(contract()["documents"][0].clone()).unwrap();
        changed.source_revision = "v2".into();
        changed.root.payload["tags"] = json!(["new"]);
        changed.blocks[0].kind = "body".into();
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
        assert_eq!(db.close().unwrap_err().code(), "TDB_WAL_CLOSED");
        drop(db);
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        assert!(db.search_documents(&request).unwrap().groups.is_empty());
        let mut new = request.clone();
        new.members[0].values = vec![json!("new")];
        let result = db.search_documents(&new).unwrap();
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].document_id, 10);
        assert_eq!(result.groups[0].source_revision, "v2");
        assert_eq!(
            result.groups[0]
                .hits
                .iter()
                .map(|hit| (hit.node_id, hit.kind.as_str()))
                .collect::<Vec<_>>(),
            vec![(11, "body"), (12, "body")]
        );
        assert_eq!(
            result.groups[0]
                .hits
                .iter()
                .map(|hit| hit.evidence.text.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta alpha"]
        );
        db.flush().unwrap();
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        assert_eq!(db.search_documents(&new).unwrap(), result);
        db.delete_document(10, &result.groups[0].revision).unwrap();
        assert!(db.search_documents(&new).unwrap().groups.is_empty());
        db.close().unwrap();
    }
}
