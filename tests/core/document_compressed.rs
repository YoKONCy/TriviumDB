//! 文档压缩候选的独立分数、分组、配额与代际契约。
use super::support::TestDir;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use triviumdb::database::{Config, StorageMode};
use triviumdb::document::*;
use triviumdb::index::quiver::ssd::SsdRuntimeOptions;
use triviumdb::{Database, ResourceRuntime, VectorType};

fn fixture() -> Value {
    serde_json::from_str(include_str!("../contracts/document_compressed_cases.json")).unwrap()
}

fn direction<T: VectorType>(value: f32) -> Vec<T> {
    let mut vector = vec![T::from_f32(0.); 129];
    vector[128] = T::from_f32(value);
    vector
}

fn document<T: VectorType + DeserializeOwned>(row: &Value) -> DocumentWrite<T> {
    serde_json::from_value(json!({
        "source_id": format!("notes://{}", row["id"].as_str().unwrap()),
        "source_revision": "v1",
        "parser_id": "plain-v1",
        "content": "alpha",
        "root": {
            "id": row["id"],
            "vector": direction::<f32>(row["root_direction"].as_f64().unwrap() as f32),
            "payload": {"vault": row["vault"]}
        },
        "blocks": [{
            "key": "body", "kind": "body", "start_byte": 0, "end_byte": 5,
            "node": {
                "id": row["node_id"],
                "vector": direction::<f32>(row["direction"].as_f64().unwrap() as f32),
                "payload": {"tags": ["evidence"], "state": "ready"}
            }
        }]
    }))
    .unwrap()
}

fn database<T: VectorType + Serialize + DeserializeOwned>(
    path: &str,
) -> (Database<T>, Vec<DocumentSnapshot>) {
    let mut db = Database::<T>::open_with_runtime(
        path,
        Config {
            dim: 129,
            storage_mode: StorageMode::Mmap,
            auto_build_quiver: false,
            ..Default::default()
        },
        ResourceRuntime::new(Default::default()).unwrap(),
    )
    .unwrap();
    let snapshots = fixture()["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| db.put_document(document(row), None).unwrap())
        .collect();
    // 高相似的普通节点参与导航，文档过滤在压缩取数和评分预算之前执行。
    for id in 100..116 {
        db.insert_with_id(id, &direction::<T>(1.), json!({"noise":true}))
            .unwrap();
    }
    (db, snapshots)
}

fn options() -> DocumentVectorOptions {
    DocumentVectorOptions {
        rerank: DocumentVectorRerank::Compressed,
        initial_candidates: 1,
        candidate_limit: 64,
        ef_search: 64,
        ..Default::default()
    }
}

fn query_vector<T: VectorType>() -> Vec<T> {
    let mut vector = direction(1.);
    vector[0] = T::from_f32(0.1);
    vector
}

fn check(
    result: &DocumentSearchResult,
    case: &Value,
    snapshots: &[DocumentSnapshot],
    positive: f32,
) {
    let rows = serde_json::to_value(result).unwrap();
    assert_eq!(
        json!(
            result
                .groups
                .iter()
                .map(|group| group.document_id.to_string())
                .collect::<Vec<_>>()
        ),
        case["documents"]
    );
    let info = result.retrieval.as_ref().unwrap();
    assert!(info.approximate);
    assert_eq!(info.strategy, DocumentVectorStrategy::Ann);
    assert_eq!(info.rerank, DocumentVectorRerank::Compressed);
    assert_eq!(serde_json::to_value(info.stop).unwrap(), case["stop"]);
    assert_eq!(result.groups.len(), case["nodes"].as_array().unwrap().len());
    for (offset, group) in result.groups.iter().enumerate() {
        let id = case["nodes"][offset].as_str().unwrap();
        let snapshot = snapshots
            .iter()
            .find(|row| row.document_id == group.document_id)
            .unwrap();
        assert_eq!(group.hits.len(), 1);
        let hit = &group.hits[0];
        assert_eq!(hit.node_id.to_string(), id);
        assert_eq!(hit.evidence.text, "alpha");
        assert_eq!(
            hit.evidence.reference,
            DocumentReference {
                document_id: group.document_id,
                revision: snapshot.revision.clone(),
                start_byte: 0,
                end_byte: 5
            }
        );
        assert_eq!(hit.evidence.source_id, snapshot.source_id);
        assert_eq!(hit.evidence.source_revision, "v1");
        let scores = hit.scores.as_ref().unwrap();
        let expected = positive * fixture()["signs"][id].as_i64().unwrap() as f32;
        assert!((scores.vector_score.unwrap() - expected).abs() < 1e-6);
        if case["operation"] == "search_hybrid" {
            let dense = fixture()["dense_ranks"][id].as_u64().unwrap() as usize;
            let text = fixture()["text_ranks"][id].as_u64().unwrap() as usize;
            assert_eq!(scores.vector_rank, Some(dense));
            assert_eq!(scores.text_rank, Some(text));
            assert!((scores.text_score.unwrap() - (8f32 / 7.).ln()).abs() < 1e-6);
            assert!(
                (hit.score - (60. / (60 + dense) as f32 + 60. / (60 + text) as f32)).abs() < 1e-6
            );
        } else {
            assert!((hit.score - expected).abs() < 1e-6);
            assert_eq!(scores.text_score, None);
            assert_eq!(scores.text_rank, None);
        }
        assert_eq!(group.score, hit.score);
        assert_eq!(rows["groups"][offset]["revision"], snapshot.revision);
    }
}

#[test]
fn 文档压缩矩阵核对原生类型过滤分组混合排名与证据() {
    fn run<T: VectorType + Serialize + DeserializeOwned>(dtype: &str) {
        let root = TestDir::new("document-compressed-matrix");
        let (mut db, snapshots) = database::<T>(&root.file("data.tdb"));
        let fixture = fixture();
        for encoding in fixture["encodings"].as_array().unwrap() {
            for precision in fixture["precisions"].as_array().unwrap() {
                for residency in ["ssd", "hot"] {
                    let runtime: SsdRuntimeOptions = serde_json::from_value(json!({
                        "encoding": encoding, "query_precision": precision,
                        "residency": residency, "cache_bytes": 64 * 1024,
                        "io_depth": 1, "use_for_ann": false
                    }))
                    .unwrap();
                    db.configure_quiver(runtime).unwrap();
                    for use_cache in [false, true] {
                        for case in fixture["cases"].as_array().unwrap() {
                            let mut query: DocumentSearchQuery =
                                serde_json::from_value(case["query"].clone()).unwrap();
                            query.max_documents = 3;
                            query.hits_per_document = 1;
                            query.use_cache = use_cache;
                            let result = if case["operation"] == "search_hybrid" {
                                db.search_documents_hybrid(
                                    &query,
                                    &query_vector::<T>(),
                                    &DocumentHybridOptions {
                                        vector: options(),
                                        ..Default::default()
                                    },
                                )
                                .unwrap()
                            } else {
                                db.search_documents_vector(&query, &query_vector::<T>(), &options())
                                    .unwrap()
                            };
                            check(
                                &result,
                                case,
                                &snapshots,
                                fixture["positive_scores"][dtype][precision.as_str().unwrap()]
                                    .as_f64()
                                    .unwrap() as f32,
                            );
                            assert_eq!(
                                serde_json::to_value(result.retrieval.unwrap().compression)
                                    .unwrap(),
                                json!({"encoding":encoding,"query_precision":precision,"residency":residency})
                            );
                        }
                    }
                }
            }
        }
        let result = db
            .search_documents_vector(
                &DocumentSearchQuery::default(),
                &query_vector::<T>(),
                &DocumentVectorOptions::default(),
            )
            .unwrap();
        assert_eq!(
            result.retrieval.as_ref().unwrap().rerank,
            DocumentVectorRerank::Original
        );
        assert_eq!(result.retrieval.as_ref().unwrap().compression, None);
        assert!(
            (result.groups[0].score
                - fixture["positive_scores"][dtype]["fp32"].as_f64().unwrap() as f32)
                .abs()
                < 1e-6
        );
        db.close().unwrap();
    }
    run::<f32>("f32");
    run::<half::f16>("f16");
}

#[test]
fn 文档压缩的预算拒绝发生于取数前并可完整重试() {
    let root = TestDir::new("document-compressed-budget");
    let (mut db, snapshots) = database::<f32>(&root.file("data.tdb"));
    db.configure_quiver(SsdRuntimeOptions {
        io_depth: 1,
        use_for_ann: false,
        ..SsdRuntimeOptions::compact()
    })
    .unwrap();
    let case = &fixture()["cases"][0];
    let query = DocumentSearchQuery {
        max_documents: 3,
        hits_per_document: 1,
        ..Default::default()
    };
    let full = db
        .search_documents_vector(&query, &query_vector::<f32>(), &options())
        .unwrap();
    check(
        &full,
        case,
        &snapshots,
        fixture()["positive_scores"]["f32"]["fp32"]
            .as_f64()
            .unwrap() as f32,
    );
    for (resource, vector_limit, nav_limit) in [
        ("document_vector_scores", 0, usize::MAX),
        ("document_ann_visits", usize::MAX, 0),
    ] {
        let baseline = db.quiver_stats().unwrap().unwrap();
        let mut limited = query.clone();
        limited.limits.max_vector_scores = vector_limit;
        limited.limits.max_ann_visits = nav_limit;
        let error = db
            .search_documents_vector(&limited, &query_vector::<f32>(), &options())
            .unwrap_err();
        assert_eq!(error.code(), "QueueFull");
        assert_eq!(error.details()["resource"], resource);
        let after = db.quiver_stats().unwrap().unwrap();
        assert_eq!(after.cold_read_calls, baseline.cold_read_calls);
        assert_eq!(after.reserved_bytes, baseline.reserved_bytes);
        assert_eq!(
            db.search_documents_vector(&query, &query_vector::<f32>(), &options())
                .unwrap(),
            full
        );
    }
    let limited = db
        .search_documents_vector(
            &query,
            &query_vector::<f32>(),
            &DocumentVectorOptions {
                initial_candidates: 1,
                candidate_limit: 1,
                ..options()
            },
        )
        .unwrap();
    assert_eq!(
        limited
            .groups
            .iter()
            .map(|group| group.document_id)
            .collect::<Vec<_>>(),
        [10]
    );
    assert_eq!(
        limited.retrieval.unwrap().stop,
        DocumentRecallStop::CandidateLimit
    );
    db.close().unwrap();
}

#[test]
fn 文档压缩更新自动刷新重开后只读查询零写() {
    let root = TestDir::new("document-compressed-reopen");
    let file = root.file("data.tdb");
    let (mut db, _) = database::<f32>(&file);
    let runtime = SsdRuntimeOptions {
        io_depth: 1,
        use_for_ann: false,
        ..SsdRuntimeOptions::compact()
    };
    db.configure_quiver(runtime).unwrap();
    db.update_vector(11, &direction::<f32>(0.)).unwrap();
    let query = DocumentSearchQuery {
        max_documents: 3,
        hits_per_document: 1,
        ..Default::default()
    };
    let changed = db
        .search_documents_vector(&query, &direction::<f32>(1.), &options())
        .unwrap();
    assert_eq!(
        changed
            .groups
            .iter()
            .map(|g| (g.document_id, g.score))
            .collect::<Vec<_>>(),
        [(10, 0.), (30, 0.), (20, -1.)]
    );
    db.close().unwrap();
    let snapshot = || {
        std::fs::read_dir(std::path::Path::new(&file).parent().unwrap())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), std::fs::read(entry.path()).unwrap())
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let before = snapshot();
    let mut reader = Database::<f32>::open_read_only(&file, 129).unwrap();
    reader.configure_quiver(runtime).unwrap();
    assert_eq!(
        reader
            .search_documents_vector(&query, &direction::<f32>(1.), &options())
            .unwrap(),
        changed
    );
    reader.close().unwrap();
    assert_eq!(snapshot(), before);
    std::fs::rename(&file, root.file("moved.tdb")).unwrap();
}

#[test]
fn 文档压缩扩展候选以覆盖完整来源组并解释分数阈值() {
    let root = TestDir::new("document-compressed-groups");
    let (mut db, snapshots) = database::<f32>(&root.file("data.tdb"));
    let mut changed: DocumentWrite<f32> = document(&fixture()["documents"][0]);
    for id in 12..19 {
        changed.blocks.push(DocumentBlock {
            key: format!("extra-{id}"),
            parent: None,
            kind: "body".into(),
            start_byte: 0,
            end_byte: 5,
            node: Some(DocumentNode {
                id,
                vector: direction(1.),
                payload: json!({}),
            }),
        });
    }
    db.put_document(changed, Some(&snapshots[0].revision))
        .unwrap();
    db.configure_quiver(SsdRuntimeOptions {
        io_depth: 1,
        use_for_ann: false,
        ..SsdRuntimeOptions::compact()
    })
    .unwrap();
    let query = DocumentSearchQuery {
        max_documents: 3,
        hits_per_document: 1,
        ..Default::default()
    };
    let result = db
        .search_documents_vector(&query, &direction::<f32>(1.), &options())
        .unwrap();
    assert_eq!(
        result
            .groups
            .iter()
            .map(|g| (g.document_id, g.hits[0].node_id, g.score))
            .collect::<Vec<_>>(),
        [(10, 11, 1.), (30, 31, 0.), (20, 21, -1.)]
    );
    let info = result.retrieval.unwrap();
    assert_eq!(info.ann_rounds, 3);
    assert_eq!(info.vector_scores, 30);
    assert_eq!(info.stop, DocumentRecallStop::GroupsSatisfied);
    let filtered = db
        .search_documents_vector(
            &query,
            &direction::<f32>(1.),
            &DocumentVectorOptions {
                min_score: Some(0.),
                ..options()
            },
        )
        .unwrap();
    assert_eq!(
        filtered
            .groups
            .iter()
            .map(|g| (g.document_id, g.score))
            .collect::<Vec<_>>(),
        [(10, 1.), (30, 0.)]
    );
    assert_eq!(
        filtered.retrieval.unwrap().stop,
        DocumentRecallStop::IndexExhausted
    );
    db.close().unwrap();
}

#[test]
fn 文档压缩显式拒绝缺失运行时和精确策略且原始模式独立执行() {
    let root = TestDir::new("document-compressed-input");
    let (mut db, _) = database::<f32>(&root.file("data.tdb"));
    let query = DocumentSearchQuery::default();
    let error = db
        .search_documents_vector(&query, &direction::<f32>(1.), &options())
        .unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        serde_json::to_value(error.info().target).unwrap(),
        json!({"field":"options.rerank"})
    );
    db.configure_quiver(SsdRuntimeOptions::compact()).unwrap();
    let artifact = std::fs::read(format!("{}.qssd", root.file("data.tdb"))).unwrap();
    db.update_vector(11, &direction::<f32>(0.)).unwrap();
    let error = db
        .search_documents_hybrid(
            &DocumentSearchQuery {
                text: "alpha".into(),
                ..Default::default()
            },
            &direction::<f32>(1.),
            &DocumentHybridOptions {
                vector: DocumentVectorOptions {
                    strategy: DocumentVectorStrategy::Exact,
                    ..options()
                },
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(error.info().target).unwrap(),
        json!({"field":"options.vector.rerank"})
    );
    let original = db
        .search_documents_vector(
            &query,
            &direction::<f32>(1.),
            &DocumentVectorOptions {
                strategy: DocumentVectorStrategy::Exact,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        original
            .groups
            .iter()
            .map(|g| (g.document_id, g.score))
            .collect::<Vec<_>>(),
        [(10, 0.), (30, 0.), (20, -1.)]
    );
    assert_eq!(
        std::fs::read(format!("{}.qssd", root.file("data.tdb"))).unwrap(),
        artifact
    );
    db.close().unwrap();
    let mut binary = Database::<u64>::open(&root.file("binary.tdb"), 129).unwrap();
    binary.insert_with_id(1, &vec![0; 129], json!({})).unwrap();
    assert_eq!(
        binary
            .search_documents_vector(&query, &vec![0; 129], &options())
            .unwrap_err()
            .code(),
        "TDB_INVALID_INPUT"
    );
    binary.close().unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn 文档压缩取消归还组合额度并保持原文与重试结果() {
    for point in [
        triviumdb::test_hooks::ConcurrencyPoint::DocumentAnnStep,
        triviumdb::test_hooks::ConcurrencyPoint::QuiverColdBeforeRead,
    ] {
        let root = TestDir::new("document-compressed-cancel");
        let (db, _) = database::<f32>(&root.file("data.tdb"));
        db.configure_quiver(SsdRuntimeOptions {
            io_depth: 1,
            use_for_ann: false,
            ..SsdRuntimeOptions::compact()
        })
        .unwrap();
        let query = DocumentSearchQuery {
            max_documents: 3,
            hits_per_document: 1,
            ..Default::default()
        };
        let expected = db
            .search_documents_vector(&query, &direction::<f32>(1.), &options())
            .unwrap();
        let baseline = db.quiver_stats().unwrap().unwrap().reserved_bytes;
        let global = db.resource_stats().reserved_workspace_bytes;
        let context = triviumdb::sdk::RequestContext::new(
            "document-compressed-cancel",
            Default::default(),
            false,
        )
        .unwrap();
        let gate = triviumdb::test_hooks::pause_at(point);
        let db = std::sync::Arc::new(db);
        let worker_db = db.clone();
        let worker_context = context.clone();
        let worker_query = query.clone();
        let worker = std::thread::spawn(move || {
            worker_context.run(|| {
                worker_db.search_documents_vector(&worker_query, &direction::<f32>(1.), &options())
            })
        });
        gate.wait_until_arrived();
        // 外层文档工作区在Reader局部账本组合计量，在共享Runtime中取得一次额度。
        assert_eq!(
            db.quiver_stats().unwrap().unwrap().reserved_bytes - baseline,
            db.resource_stats().reserved_workspace_bytes - global
        );
        context.cancel();
        drop(gate);
        assert_eq!(
            worker.join().unwrap().unwrap_err().code(),
            "TDB_QUERY_CANCELLED"
        );
        assert_eq!(db.quiver_stats().unwrap().unwrap().reserved_bytes, baseline);
        assert_eq!(db.resource_stats().reserved_workspace_bytes, global);
        assert_eq!(
            db.search_documents_vector(&query, &direction::<f32>(1.), &options())
                .unwrap(),
            expected
        );
        let mut db = std::sync::Arc::into_inner(db).unwrap();
        db.close().unwrap();
        std::fs::rename(root.file("data.tdb"), root.file("moved.tdb")).unwrap();
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn 文档压缩读取失败归还额度且完整重试保留证据() {
    let root = TestDir::new("document-compressed-io");
    let (mut db, snapshots) = database::<f32>(&root.file("data.tdb"));
    db.configure_quiver(SsdRuntimeOptions {
        io_depth: 1,
        use_for_ann: false,
        ..SsdRuntimeOptions::compact()
    })
    .unwrap();
    let query = DocumentSearchQuery {
        max_documents: 3,
        hits_per_document: 1,
        ..Default::default()
    };
    // 先构建文档缓存，使故障前后的额度基线只包含稳定驻留状态。
    db.search_documents_vector(&query, &query_vector::<f32>(), &options())
        .unwrap();
    let local = db.quiver_stats().unwrap().unwrap().reserved_bytes;
    let global = db.resource_stats().reserved_workspace_bytes;
    let failure = triviumdb::test_hooks::fail_io_at(triviumdb::test_hooks::IoPoint::QuiverColdRead);
    let error = db
        .search_documents_vector(&query, &query_vector::<f32>(), &options())
        .unwrap_err();
    drop(failure);
    assert_eq!(error.code(), "TDB_IO");
    assert_eq!(db.quiver_stats().unwrap().unwrap().reserved_bytes, local);
    assert_eq!(db.resource_stats().reserved_workspace_bytes, global);
    let result = db
        .search_documents_vector(&query, &query_vector::<f32>(), &options())
        .unwrap();
    let fixture = fixture();
    check(
        &result,
        &fixture["cases"][0],
        &snapshots,
        fixture["positive_scores"]["f32"]["fp32"].as_f64().unwrap() as f32,
    );
    db.close().unwrap();
    std::fs::rename(root.file("data.tdb"), root.file("moved.tdb")).unwrap();
}
