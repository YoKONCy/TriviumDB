use serde_json::json;
use triviumdb::database::{Config, StorageMode};
use triviumdb::index::pelorus::{PelorusConfig, PelorusSearchOptions, PoincareNavigation};
use triviumdb::{Database, DatabaseReader, Filter};

#[test]
fn pelorus_uses_native_distance_and_preserves_default_search() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(9, &[0.5, 0.0], json!({"scope":"x"}))
        .unwrap();
    db.insert_with_id(2, &[0.25, 0.0], json!({"scope":"y"}))
        .unwrap();
    db.insert_with_id(3, &[-0.25, 0.0], json!({"scope":"x"}))
        .unwrap();
    db.build_quiver_index(None).unwrap();
    let default_results = db.search(&[1.0, 0.0], 3, 0, -1.0).unwrap();
    for navigation in [PoincareNavigation::Exact, PoincareNavigation::GeometryBq2] {
        db.build_pelorus_index(PelorusConfig {
            navigation,
            ..Default::default()
        })
        .unwrap();
        for exact in [false, true] {
            let options = PelorusSearchOptions {
                exact,
                top_k: 3,
                ..Default::default()
            };
            let hits = db.search_pelorus(&[0.0, 0.0], &options).unwrap();
            assert_eq!(hits.iter().map(|hit| hit.id).collect::<Vec<_>>(), [2, 3, 9]);
            assert!((hits[0].distance - (5.0f64 / 3.0).ln()).abs() < 1e-14);
            assert!((hits[2].distance - 3.0f64.ln()).abs() < 1e-14);
        }
    }
    let hits = db
        .search_pelorus(
            &[0.0, 0.0],
            &PelorusSearchOptions {
                top_k: 1,
                payload_filter: Some(Filter::Eq("scope".into(), json!("x"))),
                max_distance: Some(1.0),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, 3);
    assert_eq!(hits[0].payload, json!({"scope":"x"}));
    let after = db.search(&[1.0, 0.0], 3, 0, -1.0).unwrap();
    assert_eq!(
        default_results
            .iter()
            .map(|hit| (hit.id, hit.score.to_bits()))
            .collect::<Vec<_>>(),
        after
            .iter()
            .map(|hit| (hit.id, hit.score.to_bits()))
            .collect::<Vec<_>>()
    );
    db.close().unwrap();
}

#[test]
fn pelorus_reopen_and_write_validation() {
    for storage_mode in [StorageMode::Rom, StorageMode::Mmap] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reopen.tdb");
        let path = path.to_str().unwrap();
        let config = Config {
            dim: 2,
            storage_mode,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(path, config).unwrap();
        db.insert_with_id(1, &[0.5, 0.0], json!({"name":"original"}))
            .unwrap();
        db.build_pelorus_index(PelorusConfig {
            curvature: 2.0,
            ..Default::default()
        })
        .unwrap();
        db.close().unwrap();
        let mut reader = DatabaseReader::<f32>::open_read_only(path, 2).unwrap();
        let hits = reader
            .search_pelorus(&[0.0, 0.0], &PelorusSearchOptions::default())
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].payload, json!({"name":"original"}));
        let expected = 2.0 * std::f64::consts::FRAC_1_SQRT_2.atanh() / 2.0f64.sqrt();
        assert!((hits[0].distance - expected).abs() < 1e-14);
        reader.close().unwrap();

        let mut db = Database::<f32>::open_with_config(path, config).unwrap();
        let mut tx = db.begin_tx();
        tx.insert_with_id(2, &[0.25, 0.0], json!(null));
        tx.insert_with_id(3, &[0.75, 0.0], json!(null));
        assert_eq!(tx.commit().unwrap_err().code(), "TDB_INVALID_VECTOR");
        assert!(!db.contains(2).unwrap());
        assert!(!db.contains(3).unwrap());

        db.update_vector(1, &[0.125, 0.0]).unwrap();
        let hits = db
            .search_pelorus(&[0.125, 0.0], &PelorusSearchOptions::default())
            .unwrap();
        assert_eq!((hits[0].id, hits[0].distance), (1, 0.0));
        db.close().unwrap();
        let mut db = Database::<f32>::open_with_config(path, config).unwrap();
        assert!(!db.contains(2).unwrap());
        db.configure_pelorus_runtime(triviumdb::index::pelorus::PelorusRuntimeOptions {
            residency: triviumdb::index::pelorus::PelorusResidency::Mmap,
            ..Default::default()
        })
        .unwrap();
        let hits = db
            .search_pelorus(&[0.125, 0.0], &PelorusSearchOptions::default())
            .unwrap();
        assert_eq!((hits[0].id, hits[0].distance), (1, 0.0));
        db.close().unwrap();
    }
}

#[test]
fn pelorus_ssd_shares_reranking_with_hot_and_mmap() {
    use triviumdb::index::pelorus::{
        PelorusResidency, PelorusRuntimeOptions, RerankConfig, RerankPrecision,
        RerankRepresentation,
    };
    use triviumdb::index::quiver::runtime::IoMode;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ssd.tdb");
    let path = path.to_str().unwrap();
    let mut db = Database::<f32>::open(path, 3).unwrap();
    for (id, vector) in [
        (1, [0.0, 0.0, 0.0]),
        (2, [0.125, 0.25, 0.0]),
        (3, [0.0, 0.5, 0.0]),
    ] {
        db.insert_with_id(id, &vector, json!(id)).unwrap();
    }
    db.build_pelorus_index(PelorusConfig {
        rerank: RerankConfig {
            representation: RerankRepresentation::Polar,
            precision: RerankPrecision::Sq4,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    let options = PelorusSearchOptions {
        top_k: 3,
        ..Default::default()
    };
    let query = [0.0, 0.0, 0.0];
    let hot = db.search_pelorus(&query, &options).unwrap();
    assert_eq!(hot.iter().map(|hit| hit.id).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(hot[0].distance, 0.0);
    assert!((hot[2].distance - 3.0f64.ln()).abs() < 1e-14);
    db.flush().unwrap();
    for residency in [
        PelorusResidency::Mmap,
        PelorusResidency::Ssd,
        PelorusResidency::Hot,
    ] {
        db.configure_pelorus_runtime(PelorusRuntimeOptions {
            residency,
            io_mode: IoMode::Buffered,
            cache_bytes: 4096,
            inflight_bytes: 4096,
            ..Default::default()
        })
        .unwrap();
        let (other, io) =
            triviumdb::sdk::quiver::measure_io(|| db.search_pelorus(&query, &options));
        let other = other.unwrap();
        assert_eq!(
            other
                .iter()
                .map(|hit| (hit.id, hit.distance.to_bits()))
                .collect::<Vec<_>>(),
            hot.iter()
                .map(|hit| (hit.id, hit.distance.to_bits()))
                .collect::<Vec<_>>()
        );
        if residency == PelorusResidency::Ssd {
            assert!(io.physical_read_calls > 0);
            let (repeat, io) =
                triviumdb::sdk::quiver::measure_io(|| db.search_pelorus(&query, &options));
            assert_eq!(repeat.unwrap().len(), 3);
            assert_eq!(io.row_cache_hits, 3);
            assert_eq!(io.physical_read_calls, 0);
        } else {
            assert_eq!(io.physical_read_calls, 0);
        }
    }
    db.close().unwrap();
}

#[test]
fn pelorus_tql_preserves_metric_in_exact_rerank() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tql.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(7, &[0.5, 0.0], json!(7)).unwrap();
    db.insert_with_id(4, &[0.25, 0.0], json!(4)).unwrap();
    db.build_pelorus_index(PelorusConfig::default()).unwrap();
    let rows = db.tql_values(
        r#"SEARCH PELORUS {vector: [0.5, 0.0], options: {top_k: 2}} RANK _ BY VECTOR [0.0, 0.0] TOP 2 RETURN _.id AS id, similarity(_) AS score ORDER BY score DESC"#,
    ).unwrap();
    assert_eq!(rows.len(), 2);
    use triviumdb::query::tql_executor::TqlValue;
    assert!(matches!(rows[0].get("id"), Some(TqlValue::Int(4))));
    assert!(
        matches!(rows[0].get("score"), Some(TqlValue::Float(score)) if (*score + (5.0f64 / 3.0).ln()).abs() < 1e-14)
    );
    assert!(matches!(rows[1].get("id"), Some(TqlValue::Int(7))));
    db.close().unwrap();
}

#[test]
fn pelorus_named_graph_filters_before_top_k() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scoped.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    db.insert_with_id(1, &[0.0, 0.0], json!(1)).unwrap();
    db.insert_with_id(2, &[0.5, 0.0], json!(2)).unwrap();
    db.put_named_graph(
        triviumdb::named_graph::NamedGraphWrite {
            graph_id: "scope".into(),
            metadata: json!(null),
            members: vec![2],
            edges: vec![],
        },
        None,
    )
    .unwrap();
    db.build_pelorus_index(PelorusConfig::default()).unwrap();
    let rows = db.tql_values(
        "GRAPH {graph_ids:['scope']} SEARCH PELORUS {vector:[0,0], options:{top_k:1}} RETURN _.id AS id",
    ).unwrap();
    assert_eq!(rows.len(), 1);
    use triviumdb::query::tql_executor::TqlValue;
    assert!(matches!(rows[0].get("id"), Some(TqlValue::Int(2))));
    db.close().unwrap();
}

#[test]
fn pelorus_document_search_scopes_before_grouping() {
    use triviumdb::document::{DocumentSearchQuery, DocumentVectorOptions, DocumentVectorStrategy};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("document.tdb");
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    for (id, vault, coordinate) in [(10, "external", 0.0), (20, "internal", 0.25)] {
        let document = json!({
            "source_id": format!("notes://{id}"), "source_revision": "first",
            "parser_id": "ranges", "content": "alpha",
            "root": {"id": id.to_string(), "vector":[coordinate,0.0], "payload":{"vault":vault}},
            "blocks":[{"key":"body", "kind":"paragraph", "start_byte":0, "end_byte":5,
                "node":{"id":(id+1).to_string(), "vector":[coordinate,0.0], "payload":{}}}]
        });
        db.put_document(serde_json::from_value(document).unwrap(), None)
            .unwrap();
    }
    db.build_pelorus_index(PelorusConfig::default()).unwrap();
    let mut query: DocumentSearchQuery = serde_json::from_value(json!({
        "document_filter":{"vault":"internal"}, "max_documents":1, "hits_per_document":1
    }))
    .unwrap();
    let options = DocumentVectorOptions {
        strategy: DocumentVectorStrategy::Ann,
        ..Default::default()
    };
    let result = db
        .search_documents_pelorus(&query, &[0.0, 0.0], &options, None)
        .unwrap();
    assert_eq!(result.groups.len(), 1);
    assert_eq!(result.groups[0].document_id, 20);
    let hit = &result.groups[0].hits[0];
    assert_eq!(hit.node_id, 21);
    assert!((f64::from(hit.score) + (5.0f64 / 3.0).ln()).abs() < 1e-7);
    assert!(result.retrieval.unwrap().ann_visits > 0);

    query.limits.max_ann_visits = 1;
    let failure = db
        .search_documents_pelorus(&query, &[0.0, 0.0], &options, None)
        .unwrap_err();
    assert!(matches!(
        failure,
        triviumdb::TriviumError::ResourceBudgetExceeded {
            resource: "pelorus_ann_visits",
            used: 1,
            limit: 1,
            requested: 1
        }
    ));
    db.close().unwrap();
}
