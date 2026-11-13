//! 文档向量与混合召回的独立排序、分组、范围及生命周期契约。
use super::support::TestDir;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use triviumdb::database::{AccessMode, Config, StorageMode};
use triviumdb::document::*;
use triviumdb::{Database, ResourceRuntime, VectorType};

fn contract() -> Value {
    serde_json::from_str(include_str!("../contracts/document_vector_cases.json")).unwrap()
}
fn documents() -> Value {
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
    documents()["documents"]
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
    let mut query = json!({"hits_per_document":10});
    query
        .as_object_mut()
        .unwrap()
        .extend(case["query"].as_object().unwrap().clone());
    serde_json::from_value(query).unwrap()
}
fn options(case: &Value) -> Value {
    case.get("options").cloned().unwrap_or_else(|| {
        if case["operation"] == "search_hybrid" {
            json!({"vector":{"strategy":"exact"}})
        } else {
            json!({"strategy":"exact"})
        }
    })
}

fn request<T: VectorType + Serialize + DeserializeOwned>(
    db: &mut Database<T>,
    case: &Value,
) -> DocumentSearchResult {
    let command = json!({"operation":case["operation"],"query":query(case),"vector":[1,0],"options":options(case)});
    let result = triviumdb::sdk::documents::request(db, &command.to_string()).unwrap();
    serde_json::from_str(&result).unwrap()
}
fn assert_case<T: VectorType>(
    result: &DocumentSearchResult,
    case: &Value,
    snapshots: &[DocumentSnapshot],
) {
    let fixture = contract();
    let source = documents();
    assert_eq!(
        result
            .groups
            .iter()
            .map(|group| group.document_id.to_string())
            .collect::<Vec<_>>(),
        case["documents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap())
            .collect::<Vec<_>>(),
        "{}",
        case["name"]
    );
    assert_eq!(
        json!(
            result
                .groups
                .iter()
                .map(|group| group
                    .hits
                    .iter()
                    .map(|hit| hit.node_id.to_string())
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>()
        ),
        case["hits"],
        "{}",
        case["name"]
    );
    assert_eq!(
        result.matched_units,
        case["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hits| hits.as_array().unwrap().len())
            .sum::<usize>()
    );
    let info = result.retrieval.as_ref().unwrap();
    assert_eq!(info.strategy, DocumentVectorStrategy::Exact);
    assert!(!info.approximate);
    assert_eq!(info.ann_visits, 0);
    assert_eq!(info.ann_rounds, 0);
    assert_eq!(info.stop, DocumentRecallStop::ExactComplete);
    assert_eq!(
        info.eligible_units as u64,
        case["eligible"].as_u64().unwrap()
    );
    assert_eq!(info.vector_scores, info.eligible_units);
    assert_eq!(
        info.vector_candidates as u64,
        case.get("vector_candidates")
            .and_then(Value::as_u64)
            .unwrap_or(info.eligible_units as u64)
    );
    assert_eq!(info.hybrid, case["operation"] == "search_hybrid");
    let is_binary = std::any::TypeId::of::<T>() == std::any::TypeId::of::<u64>();
    for group in &result.groups {
        let snapshot = snapshots
            .iter()
            .find(|row| row.document_id == group.document_id)
            .unwrap();
        assert_eq!(group.revision, snapshot.revision);
        assert_eq!(group.source_id, snapshot.source_id);
        assert_eq!(group.source_revision, "v1");
        assert_eq!(group.score, group.hits[0].score);
        for hit in &group.hits {
            let id = hit.node_id.to_string();
            let document = source["documents"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| {
                    row["root"]["id"].as_str().unwrap().parse::<u64>().unwrap() == group.document_id
                })
                .unwrap();
            let expected = if case["query"]["level"] == "document" {
                (
                    "document",
                    None,
                    0,
                    document["content"].as_str().unwrap().len(),
                )
            } else {
                let block = document["blocks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["node"]["id"].as_str() == Some(id.as_str()))
                    .unwrap();
                (
                    block["kind"].as_str().unwrap(),
                    block["key"].as_str(),
                    block["start_byte"].as_u64().unwrap() as usize,
                    block["end_byte"].as_u64().unwrap() as usize,
                )
            };
            assert_eq!(hit.kind, expected.0);
            assert_eq!(hit.block_key.as_deref(), expected.1);
            assert_eq!(
                hit.evidence.reference,
                DocumentReference {
                    document_id: group.document_id,
                    revision: snapshot.revision.clone(),
                    start_byte: expected.2,
                    end_byte: expected.3
                }
            );
            assert_eq!(
                hit.evidence.text,
                &document["content"].as_str().unwrap()[expected.2..expected.3]
            );
            let scores = hit.scores.as_ref().unwrap();
            let title = hit.kind == "title" || hit.kind == "document";
            if case["name"] == "hybrid-sparse-only" {
                assert_eq!(scores.vector_score, None);
                assert_eq!(scores.vector_rank, None);
                let rank = fixture["text_field_order"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|value| value == &id)
                    .unwrap()
                    + 1;
                assert_eq!(scores.text_rank, Some(rank));
                assert!((hit.score - 120. / (60. + rank as f32)).abs() < 1e-6);
            } else {
                assert_eq!(
                    scores.vector_score,
                    Some(if is_binary {
                        if title { 128. } else { 126. }
                    } else if title {
                        1.
                    } else {
                        0.
                    })
                );
            }
            if case["name"] == "hybrid-fields" {
                let expected_score = fixture["fused_scores"][&id].as_f64().unwrap();
                assert!((hit.score as f64 - expected_score).abs() < 1e-6);
                let dense_rank = fixture["dense_block_order"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|value| value == &id)
                    .unwrap()
                    + 1;
                assert_eq!(scores.vector_rank, Some(dense_rank));
                let text_rank = fixture["text_field_order"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|value| value == &id)
                    .map(|rank| rank + 1);
                assert_eq!(scores.text_rank, text_rank);
                let text_score = source["scores"].get(&id).and_then(Value::as_f64);
                assert_eq!(scores.text_score.is_some(), text_score.is_some());
                if let Some(expected) = text_score {
                    assert!((scores.text_score.unwrap() as f64 - expected).abs() < 1e-6);
                }
            }
            if case["name"] == "hybrid-phrase-constraint" {
                assert!((hit.score - 120. / 61.).abs() < 1e-6);
                assert_eq!(hit.matches.len(), 1);
                assert_eq!(hit.matches[0].evidence.text, "alpha beta");
                assert_eq!(
                    serde_json::to_value(&hit.matches[0].terms).unwrap(),
                    json!([{"term":"alpha","position":0,"start_byte":0,"end_byte":5},{"term":"beta","position":1,"start_byte":6,"end_byte":10}])
                );
            } else {
                assert!(hit.matches.is_empty());
            }
            if case["operation"] == "search_vector" || case["name"] == "hybrid-dense-only" {
                assert_eq!(hit.score, scores.vector_score.unwrap());
                assert_eq!(scores.text_score, None);
            }
        }
    }
}

fn roundtrip<T: VectorType + Serialize + DeserializeOwned>() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("document_vectors_contract");
        let path = root.file("data.tdb");
        let mut db = Database::<T>::open_with_config(&path, config(mode)).unwrap();
        let snapshots = populate(&mut db);
        let fixture = contract();
        for case in fixture["cases"].as_array().unwrap() {
            let result = request(&mut db, case);
            assert_case::<T>(&result, case, &snapshots);
            let vector = vec![T::from_f32(1.), T::from_f32(0.)];
            let direct = if case["operation"] == "search_vector" {
                db.search_documents_vector(
                    &query(case),
                    &vector,
                    &serde_json::from_value(options(case)).unwrap(),
                )
                .unwrap()
            } else {
                db.search_documents_hybrid(
                    &query(case),
                    &vector,
                    &serde_json::from_value(options(case)).unwrap(),
                )
                .unwrap()
            };
            assert_eq!(direct, result);
            let mut uncached = query(case);
            uncached.use_cache = false;
            let cold = if case["operation"] == "search_vector" {
                db.search_documents_vector(
                    &uncached,
                    &vector,
                    &serde_json::from_value(options(case)).unwrap(),
                )
                .unwrap()
            } else {
                db.search_documents_hybrid(
                    &uncached,
                    &vector,
                    &serde_json::from_value(options(case)).unwrap(),
                )
                .unwrap()
            };
            assert_eq!(cold, result);
        }
        let retained = request(&mut db, &fixture["cases"][5]);
        db.close().unwrap();
        let before = files(&root.0);
        let mut reader = triviumdb::DatabaseReader::<T>::open_with_config(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                ..config(mode)
            },
        )
        .unwrap();
        let vector = [T::from_f32(1.), T::from_f32(0.)];
        assert_eq!(
            reader
                .search_documents_hybrid(
                    &query(&fixture["cases"][5]),
                    &vector,
                    &DocumentHybridOptions {
                        vector: DocumentVectorOptions {
                            strategy: DocumentVectorStrategy::Exact,
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                )
                .unwrap(),
            retained
        );
        assert_case::<T>(
            &reader
                .search_documents_vector(
                    &query(&fixture["cases"][0]),
                    &vector,
                    &DocumentVectorOptions {
                        strategy: DocumentVectorStrategy::Exact,
                        ..Default::default()
                    },
                )
                .unwrap(),
            &fixture["cases"][0],
            &snapshots,
        );
        reader.close().unwrap();
        assert_eq!(files(&root.0), before);
        let moved = root.0.join("moved");
        std::fs::create_dir(&moved).unwrap();
        for name in before.keys() {
            std::fs::rename(root.0.join(name), moved.join(name)).unwrap();
        }
        assert_eq!(retained.groups[0].hits[0].evidence.text, "alpha");
        let mut db =
            Database::<T>::open_with_config(moved.join("data.tdb").to_str().unwrap(), config(mode))
                .unwrap();
        assert_eq!(request(&mut db, &fixture["cases"][5]), retained);
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
fn 三种dtype及两种存储模式的文档向量与RRF符合独立样例() {
    roundtrip::<f32>();
    roundtrip::<half::f16>();
    roundtrip::<u64>();
}

#[test]
fn 向量及混合参数预算拒绝保留字段并允许重试() {
    let root = TestDir::new("document_vectors_validation");
    let mut db = Database::<f32>::open_with_runtime(
        &root.file("data.tdb"),
        config(StorageMode::Mmap),
        ResourceRuntime::new(Default::default()).unwrap(),
    )
    .unwrap();
    populate(&mut db);
    for case in contract()["invalid"].as_array().unwrap() {
        let error=triviumdb::sdk::documents::request(&mut db,&json!({"operation":case["operation"],"query":case["query"],"vector":[1,0],"options":case.get("options").cloned().unwrap_or(json!({}))}).to_string()).unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT");
        assert_eq!(
            serde_json::to_value(error.info().target.unwrap()).unwrap(),
            json!({"field":case["field"]})
        );
        assert!(error.info().write.is_none());
        assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
    }
    for vector in [vec![1.], vec![f32::NAN, 0.], vec![f32::INFINITY, 0.]] {
        let error = db
            .search_documents_vector(&Default::default(), &vector, &Default::default())
            .unwrap_err();
        assert_eq!(
            serde_json::to_value(error.info().target.unwrap()).unwrap(),
            json!({"field":"vector"})
        );
    }
    let case = &contract()["cases"][5];
    let expected = request(&mut db, case);
    let weighted = db
        .search_documents_hybrid(
            &query(case),
            &[1., 0.],
            &DocumentHybridOptions {
                text_weight: f32::MAX,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(weighted.groups[0].hits[0].node_id, 11);
    let expected_weighted = f64::from(f32::MAX) * 60. / 61.;
    assert!((f64::from(weighted.groups[0].score) / expected_weighted - 1.).abs() < 1e-6);
    assert!(
        weighted
            .groups
            .iter()
            .flat_map(|group| &group.hits)
            .all(|hit| hit.score.is_finite())
    );
    for resource in [
        "document_vector_scores",
        "document_filter_candidates",
        "document_candidates",
        "document_return_bytes",
    ] {
        for cache in [true, false] {
            let mut query = query(case);
            query.use_cache = cache;
            match resource {
                "document_vector_scores" => query.limits.max_vector_scores = 0,
                "document_filter_candidates" => query.limits.max_filter_candidates = 0,
                "document_candidates" => query.limits.max_candidates = 0,
                "document_return_bytes" => query.limits.max_return_bytes = 0,
                _ => unreachable!(),
            }
            let error = db
                .search_documents_hybrid(
                    &query,
                    &[1., 0.],
                    &DocumentHybridOptions {
                        vector: DocumentVectorOptions {
                            strategy: DocumentVectorStrategy::Exact,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
                .unwrap_err();
            assert_eq!(error.code(), "QueueFull");
            assert_eq!(error.details()["resource"], resource);
            assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
            assert_eq!(request(&mut db, case), expected);
        }
    }
    db.close().unwrap();
}

#[test]
fn 空文本片段的完整整数指纹与分数阈值保持原生位模式() {
    let root = TestDir::new("document_vectors_u64");
    let mut db =
        Database::<u64>::open_with_config(&root.file("data.tdb"), config(StorageMode::Mmap))
            .unwrap();
    let document = json!({"source_id":"bits://full","source_revision":"v1","parser_id":"plain-v1","content":"","root":{"id":"10","vector":[0,0]},"blocks":[
        {"key":"exact","kind":"body","start_byte":0,"end_byte":0,"node":{"id":"11","vector":[u64::MAX,1]}},
        {"key":"near","kind":"body","start_byte":0,"end_byte":0,"node":{"id":"12","vector":[u64::MAX-1,1]}}
    ]});
    db.put_document(serde_json::from_value(document).unwrap(), None)
        .unwrap();
    let raw = triviumdb::sdk::documents::request(
        &mut db,
        &json!({"operation":"search_vector","query":{"hits_per_document":2},"vector":[u64::MAX,1]})
            .to_string(),
    )
    .unwrap();
    let result: DocumentSearchResult = serde_json::from_str(&raw).unwrap();
    assert_eq!(result.corpus_units, 0);
    assert_eq!(result.retrieval.as_ref().unwrap().eligible_units, 2);
    assert_eq!(
        result.retrieval.as_ref().unwrap().strategy,
        DocumentVectorStrategy::Exact
    );
    assert_eq!(
        result.groups[0]
            .hits
            .iter()
            .map(|hit| (hit.node_id, hit.score, hit.evidence.text.as_str()))
            .collect::<Vec<_>>(),
        vec![(11, 128., ""), (12, 127., "")]
    );
    let limited = db
        .search_documents_vector(
            &DocumentSearchQuery {
                hits_per_document: 2,
                ..Default::default()
            },
            &[u64::MAX, 1],
            &DocumentVectorOptions {
                min_score: Some(128.),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(limited.groups[0].hits.len(), 1);
    assert_eq!(limited.groups[0].hits[0].node_id, 11);
    assert_eq!(limited.retrieval.as_ref().unwrap().vector_scores, 2);
    assert_eq!(limited.retrieval.as_ref().unwrap().vector_candidates, 1);
    let error = db
        .search_documents_vector(
            &Default::default(),
            &[u64::MAX, 1],
            &DocumentVectorOptions {
                strategy: DocumentVectorStrategy::Ann,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        serde_json::to_value(error.info().target.unwrap()).unwrap(),
        json!({"field":"options.strategy"})
    );
    db.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn WAL未知结果保留旧向量视图且重开后新向量与证据完整恢复() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("document_vectors_wal");
        let path = root.file("data.tdb");
        let config = Config {
            sync_mode: triviumdb::storage::wal::SyncMode::Full,
            ..config(mode)
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        let snapshots = populate(&mut db);
        db.build_quiver_index(None).unwrap();
        db.flush().unwrap();
        let query = DocumentSearchQuery {
            document_ids: vec![10],
            kinds: vec!["body".into()],
            ..Default::default()
        };
        let exact = DocumentVectorOptions {
            strategy: DocumentVectorStrategy::Exact,
            ..Default::default()
        };
        let old = db
            .search_documents_vector(&query, &[1., 0.], &exact)
            .unwrap();
        assert_eq!(old.groups[0].hits[0].score, 0.);
        let mut changed: DocumentWrite<f32> =
            serde_json::from_value(documents()["documents"][0].clone()).unwrap();
        changed.source_revision = "v2".into();
        changed.blocks[1].node.as_mut().unwrap().vector = vec![1., 0.];
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
        assert_eq!(
            db.search_documents_vector(&query, &[1., 0.], &exact)
                .unwrap(),
            old
        );
        assert_eq!(db.close().unwrap_err().code(), "TDB_WAL_CLOSED");
        drop(db);
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        let recovered = db
            .search_documents_vector(&query, &[1., 0.], &exact)
            .unwrap();
        assert_eq!(recovered.groups.len(), 1);
        assert_eq!(recovered.groups[0].hits.len(), 1);
        assert_eq!(
            (
                recovered.groups[0].hits[0].node_id,
                recovered.groups[0].hits[0].score,
                recovered.groups[0].hits[0].evidence.text.as_str()
            ),
            (12, 1., "beta alpha")
        );
        assert_eq!(recovered.groups[0].source_revision, "v2");
        assert_ne!(recovered.groups[0].revision, snapshots[0].revision);
        assert_eq!(recovered.groups[0].hits[0].evidence.reference.start_byte, 6);
        assert_eq!(recovered.groups[0].hits[0].evidence.reference.end_byte, 16);
        db.build_quiver_index(None).unwrap();
        let ann = DocumentVectorOptions {
            strategy: DocumentVectorStrategy::Ann,
            ..Default::default()
        };
        let indexed = db.search_documents_vector(&query, &[1., 0.], &ann).unwrap();
        assert_eq!(indexed.groups[0].hits[0].score, 1.);
        assert_eq!(
            indexed.groups[0].hits[0].evidence,
            recovered.groups[0].hits[0].evidence
        );
        db.flush().unwrap();
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        assert_eq!(
            db.search_documents_vector(&query, &[1., 0.], &ann).unwrap(),
            indexed
        );
        db.delete_document(10, &recovered.groups[0].revision)
            .unwrap();
        assert!(
            db.search_documents_vector(&query, &[1., 0.], &exact)
                .unwrap()
                .groups
                .is_empty()
        );
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        assert!(
            db.search_documents_vector(&query, &[1., 0.], &exact)
                .unwrap()
                .groups
                .is_empty()
        );
        db.flush().unwrap();
        db.close().unwrap();
    }
}

fn clustered<T: VectorType + Serialize + DeserializeOwned>(db: &mut Database<T>, dim: usize) {
    for (root, blocks, second) in [(100, 8, 0.), (200, 1, 1.), (300, 1, 2.)] {
        let mut vector = vec![T::zero(); dim];
        vector[0] = T::from_f32(if second == 2. { 0. } else { 1. });
        vector[1] = T::from_f32(if second == 0. { 0. } else { 1. });
        db.put_document(
            DocumentWrite {
                source_id: format!("cluster://{root}"),
                source_revision: "v1".into(),
                parser_id: "plain-v1".into(),
                content: "alpha".into(),
                root: DocumentNode {
                    id: root,
                    vector: vector.clone(),
                    payload: Value::Null,
                },
                blocks: (1..=blocks)
                    .map(|offset| DocumentBlock {
                        key: offset.to_string(),
                        parent: None,
                        kind: "body".into(),
                        start_byte: 0,
                        end_byte: 5,
                        node: Some(DocumentNode {
                            id: root + offset,
                            vector: vector.clone(),
                            payload: Value::Null,
                        }),
                    })
                    .collect(),
            },
            None,
        )
        .unwrap();
    }
    db.build_quiver_index(None).unwrap();
}

fn ann_groups<T: VectorType + Serialize + DeserializeOwned>() {
    for dim in [2, 384] {
        let root = TestDir::new("document_vectors_ann");
        let path = root.file("data.tdb");
        let mut db = Database::<T>::open_with_runtime(
            &path,
            Config {
                dim,
                ..config(StorageMode::Mmap)
            },
            ResourceRuntime::new(Default::default()).unwrap(),
        )
        .unwrap();
        clustered(&mut db, dim);
        let mut vector = vec![T::zero(); dim];
        vector[0] = T::from_f32(1.);
        let query = DocumentSearchQuery {
            max_documents: 3,
            hits_per_document: 1,
            ..Default::default()
        };
        let options = DocumentVectorOptions {
            strategy: DocumentVectorStrategy::Ann,
            initial_candidates: 1,
            candidate_limit: 13,
            ef_search: 13,
            ..Default::default()
        };
        let result = db
            .search_documents_vector(&query, &vector, &options)
            .unwrap();
        assert_eq!(
            result
                .groups
                .iter()
                .map(|group| group.document_id)
                .collect::<Vec<_>>(),
            vec![100, 200, 300]
        );
        assert_eq!(
            result
                .groups
                .iter()
                .map(|group| group.hits[0].node_id)
                .collect::<Vec<_>>(),
            vec![101, 201, 301]
        );
        for (group, expected) in result
            .groups
            .iter()
            .zip([1., std::f32::consts::FRAC_1_SQRT_2, 0.])
        {
            assert!((group.score - expected).abs() < 1e-6);
            assert_eq!(group.hits[0].evidence.text, "alpha");
        }
        let info = result.retrieval.as_ref().unwrap();
        assert!(info.approximate);
        assert_eq!(info.ann_rounds, 3);
        assert_eq!(info.stop, DocumentRecallStop::GroupsSatisfied);
        assert_eq!(info.vector_candidates, 10);
        let limited = db
            .search_documents_vector(
                &query,
                &vector,
                &DocumentVectorOptions {
                    candidate_limit: 3,
                    ..options.clone()
                },
            )
            .unwrap();
        assert_eq!(limited.groups.len(), 1);
        assert_eq!(limited.groups[0].document_id, 100);
        assert_eq!(
            limited.retrieval.as_ref().unwrap().stop,
            DocumentRecallStop::CandidateLimit
        );
        for resource in ["document_ann_visits", "document_vector_scores"] {
            let mut limited_query = query.clone();
            if resource == "document_ann_visits" {
                limited_query.limits.max_ann_visits = 0
            } else {
                limited_query.limits.max_vector_scores = 0
            }
            let error = db
                .search_documents_vector(&limited_query, &vector, &options)
                .unwrap_err();
            assert_eq!(error.code(), "QueueFull");
            assert_eq!(error.details()["resource"], resource);
            assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
            assert_eq!(
                db.search_documents_vector(&query, &vector, &options)
                    .unwrap(),
                result
            );
        }
        db.close().unwrap();
        let before = files(&root.0);
        let mut db = Database::<T>::open_with_config(
            &path,
            Config {
                dim,
                access_mode: AccessMode::ReadOnly,
                ..config(StorageMode::Mmap)
            },
        )
        .unwrap();
        assert_eq!(
            db.search_documents_vector(&query, &vector, &options)
                .unwrap(),
            result
        );
        db.close().unwrap();
        assert_eq!(files(&root.0), before);
    }
}
#[test]
fn ANN扩展覆盖不同来源且候选上限与工作预算可区分() {
    ann_groups::<f32>();
    ann_groups::<half::f16>();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 向量评分与ANN导航取消均归还资源并保持可重试() {
    for ann in [false, true] {
        let root = TestDir::new("document_vectors_cancel");
        let mut db = Database::<f32>::open_with_runtime(
            &root.file("data.tdb"),
            config(StorageMode::Mmap),
            ResourceRuntime::new(Default::default()).unwrap(),
        )
        .unwrap();
        populate(&mut db);
        if ann {
            db.build_quiver_index(None).unwrap();
        }
        let query = DocumentSearchQuery::default();
        let options = DocumentVectorOptions {
            strategy: if ann {
                DocumentVectorStrategy::Ann
            } else {
                DocumentVectorStrategy::Exact
            },
            ..Default::default()
        };
        let expected = db
            .search_documents_vector(&query, &[1., 0.], &options)
            .unwrap();
        let context = triviumdb::sdk::RequestContext::new(
            "document-vector-cancel",
            triviumdb::query::tql_executor::QueryControl::default(),
            false,
        )
        .unwrap();
        let db = std::sync::Arc::new(db);
        let point = if ann {
            triviumdb::test_hooks::ConcurrencyPoint::DocumentAnnStep
        } else {
            triviumdb::test_hooks::ConcurrencyPoint::DocumentVectorStep
        };
        let gate = triviumdb::test_hooks::pause_at(point);
        let worker_db = db.clone();
        let worker_context = context.clone();
        let worker_options = options.clone();
        let worker_query = query.clone();
        let worker = std::thread::spawn(move || {
            worker_context.run(|| {
                worker_db.search_documents_vector(&worker_query, &[1., 0.], &worker_options)
            })
        });
        gate.wait_until_arrived();
        context.cancel();
        drop(gate);
        assert_eq!(
            worker.join().unwrap().unwrap_err().code(),
            "TDB_QUERY_CANCELLED"
        );
        assert_eq!(db.resource_stats().reserved_workspace_bytes, 0);
        assert_eq!(
            db.search_documents_vector(&query, &[1., 0.], &options)
                .unwrap(),
            expected
        );
        let mut db = std::sync::Arc::into_inner(db).unwrap();
        db.close().unwrap();
    }
}
