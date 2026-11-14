//! 普通查询与排队查询共用压缩重排的代际及评分类别契约。
use super::*;
use crate::index::quiver::ssd::{RerankEncoding, Residency};
use crate::test_support::TestDir;

fn database(encoding: RerankEncoding) -> (TestDir, Database<f32>, Vec<f32>) {
    let root = TestDir::new("quiver-configured-ann");
    let path = root.0.join("sample.tdb");
    let mut db = Database::<f32>::open_with_config(
        path.to_str().unwrap(),
        super::super::Config {
            dim: 129,
            auto_build_quiver: false,
            ..Default::default()
        },
    )
    .unwrap();
    let mut query = vec![0.; 129];
    query[128] = 1.;
    db.insert_with_id(7, &query, serde_json::json!({"text":"正向"}))
        .unwrap();
    db.insert_with_id(
        9,
        &query.iter().map(|x| -x).collect::<Vec<_>>(),
        serde_json::json!({"text":"反向"}),
    )
    .unwrap();
    db.configure_quiver(SsdRuntimeOptions {
        encoding,
        residency: Residency::Hot,
        ..SsdRuntimeOptions::accuracy()
    })
    .unwrap();
    (root, db, query)
}

#[test]
fn queued_ann_uses_current_generation_and_releases_files_on_close() {
    for encoding in [RerankEncoding::CenteredInt8, RerankEncoding::CenteredInt4] {
        let (root, mut db, query) = database(encoding);
        let handle = db.search_handle();
        db.update_vector(9, &vec![0.; 129]).unwrap();
        let results = handle
            .search_batch(
                &[query],
                &super::super::SearchConfig {
                    top_k: 2,
                    expand_depth: 0,
                    min_score: -1.,
                    enable_advanced_pipeline: false,
                    ..Default::default()
                },
                &super::super::BatchSearchConfig { parallelism: 1 },
            )
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0]
                .iter()
                .map(|hit| (hit.id, hit.score))
                .collect::<Vec<_>>(),
            [(7, 1.), (9, 0.)]
        );
        db.close().unwrap();
        // 排队句柄继续存活，已关闭数据库的原始文件仍可移动。
        std::fs::rename(root.0.join("sample.tdb"), root.0.join("moved.tdb")).unwrap();
        drop(handle);
    }
}

#[test]
fn rebuilding_only_the_route_refreshes_the_binding_and_compacted_row_order() {
    for residency in [Residency::Ssd, Residency::Hot] {
        let (_root, mut db, query) = database(RerankEncoding::CenteredInt4);
        db.configure_quiver(SsdRuntimeOptions {
            residency,
            ..SsdRuntimeOptions::compact()
        })
        .unwrap();
        db.update_vector(9, &vec![0.; 129]).unwrap();
        db.search(&query, 2, 0, -1.).unwrap();
        let generation = read_or_recover(&db.memtable).vector_generation();
        for m in [2, 4] {
            db.build_quiver_index(Some(crate::index::quiver::QuIVerConfig {
                m,
                ef_construction: 16,
                alpha: 1.2,
            }))
            .unwrap();
            {
                let mt = read_or_recover(&db.memtable);
                assert_eq!(mt.vector_generation(), generation);
                assert!(
                    !mt.quiver_ssd
                        .as_ref()
                        .unwrap()
                        .reader
                        .routing_matches(mt.quiver().unwrap())
                );
            }
            let hits = db.search(&query, 2, 0, -1.).unwrap();
            assert_eq!(
                hits.iter()
                    .map(|hit| (hit.id, hit.score))
                    .collect::<Vec<_>>(),
                [(7, 1.), (9, 0.)]
            );
            let mt = read_or_recover(&db.memtable);
            let active = mt.quiver_ssd.as_ref().unwrap();
            assert!(active.reader.routing_matches(mt.quiver().unwrap()));
            assert_eq!(active.reader.stats().metadata.rows, 2);
        }
        db.close().unwrap();
    }
}

#[test]
fn compressed_pipeline_scores_are_approximate_and_preserve_late_materialization() {
    use crate::query::pipeline::{
        NodeSet, PipelineBudget, PipelineContext, PipelineOperator, QuiverVectorSearch, ScoreKind,
    };
    for encoding in [RerankEncoding::CenteredInt8, RerankEncoding::CenteredInt4] {
        let (_root, mut db, query) = database(encoding);
        {
            let mt = read_or_recover(&db.memtable);
            let before = mt.payload_access_counters();
            let mut context = PipelineContext::new(&mt, PipelineBudget::default());
            let rows = QuiverVectorSearch {
                query,
                top_k: 2,
                ef_search: 32,
            }
            .apply(NodeSet::empty(), &mut context)
            .unwrap();
            assert_eq!(
                rows.rows()
                    .iter()
                    .map(|row| (row.id, row.similarity.as_ref().unwrap().value))
                    .collect::<Vec<_>>(),
                [(7, 1.), (9, -1.)]
            );
            for row in rows.rows() {
                assert_eq!(
                    row.similarity.as_ref().unwrap().kind,
                    ScoreKind::Approximate
                );
            }
            assert_eq!(mt.payload_access_counters(), before);
        }
        db.close().unwrap();
    }
}

#[test]
fn cancelled_tql_does_not_publish_a_stale_compressed_generation() {
    let (root, mut db, query) = database(RerankEncoding::CenteredInt4);
    db.update_vector(9, &vec![0.; 129]).unwrap();
    let path = root.0.join("sample.tdb.qssd");
    let before = std::fs::read(&path).unwrap();
    let control = crate::query::tql_executor::QueryControl::default();
    control.cancel();
    let input = format!(
        "SEARCH VECTOR {} TOP 2 RETURN *",
        serde_json::to_string(&query).unwrap()
    );
    let error = db.tql_with_control(&input, control.clone()).unwrap_err();
    let info = serde_json::to_value(error.info()).unwrap();
    assert_eq!(info["code"], "TDB_QUERY_CANCELLED");
    assert_eq!(info["stage"], "execute");
    assert_eq!(
        info["target"]["query_span"],
        serde_json::json!({"start_byte":0,"end_byte":input.len(),"line":1,"column":1})
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let prepared = db.prepare_tql(&input).unwrap();
    assert_eq!(
        db.execute_prepared_tql_with_control(&prepared, &Default::default(), control)
            .unwrap_err()
            .code(),
        "TDB_QUERY_CANCELLED"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let hits = db.search(&query, 2, 0, -1.).unwrap();
    assert_eq!(
        hits.iter()
            .map(|hit| (hit.id, hit.score))
            .collect::<Vec<_>>(),
        [(7, 1.), (9, 0.)]
    );
    db.close().unwrap();
}

#[test]
fn explicit_native_l2_keeps_its_original_vector_metric() {
    let (_root, mut db, query) = database(RerankEncoding::CenteredInt4);
    db.enable_quiver_sm_native_l2().unwrap();
    assert_eq!(
        db.search(&query, 2, 0, -4.).unwrap_err().code(),
        "TDB_INVALID_INPUT"
    );
    db.configure_quiver(SsdRuntimeOptions {
        use_for_ann: false,
        ..SsdRuntimeOptions::compact()
    })
    .unwrap();
    let hits = db.search(&query, 2, 0, -4.).unwrap();
    assert_eq!(
        hits.iter()
            .map(|hit| (hit.id, hit.score))
            .collect::<Vec<_>>(),
        [(7, 0.), (9, -4.)]
    );
    db.close().unwrap();
}

#[test]
fn exact_and_nonvector_reads_leave_stale_compressed_artifacts_untouched() {
    let (root, mut db, query) = database(RerankEncoding::CenteredInt4);
    let generation = read_or_recover(&db.memtable)
        .quiver_ssd
        .as_ref()
        .unwrap()
        .generation;
    db.update_vector(9, &vec![0.; 129]).unwrap();
    let path = root.0.join("sample.tdb.qssd");
    let before = std::fs::read(&path).unwrap();
    let config = super::super::SearchConfig {
        top_k: 2,
        min_score: -1.,
        expand_depth: 0,
        force_brute_force: true,
        enable_advanced_pipeline: false,
        ..Default::default()
    };
    let mut hits = vec![db.search_exact(&query, 2).unwrap()];
    hits.push(db.search_hybrid(None, Some(&query), &config).unwrap());
    hits.push(
        db.search_hybrid_with_context(None, Some(&query), &config)
            .unwrap()
            .0,
    );
    hits.push(
        db.search_hybrid_grouped(None, Some(&query), &config)
            .unwrap()
            .semantic_hits,
    );
    hits.extend(
        db.search_batch(
            std::slice::from_ref(&query),
            &config,
            &super::super::BatchSearchConfig { parallelism: 1 },
        )
        .unwrap(),
    );
    for result in hits {
        assert_eq!(
            result
                .iter()
                .map(|hit| (hit.id, hit.score))
                .collect::<Vec<_>>(),
            [(7, 1.), (9, 0.)]
        );
    }
    let rows = db
        .tql_nodes("FIND {text: {$exists: true}} RETURN *")
        .unwrap();
    let mut ids = rows
        .iter()
        .flat_map(|row| row.values().map(|node| node.id))
        .collect::<Vec<_>>();
    ids.sort_unstable();
    assert_eq!(ids, [7, 9]);
    let input = "FIND {text: {$exists: true}} RETURN *";
    let prepared = db.prepare_tql(input).unwrap();
    let results = [
        db.tql(input).unwrap(),
        db.query(input).unwrap(),
        db.tql_with_control(input, Default::default()).unwrap(),
        db.execute_prepared_tql(&prepared, &Default::default())
            .unwrap(),
        db.execute_prepared_tql_with_control(&prepared, &Default::default(), Default::default())
            .unwrap(),
    ];
    for rows in results {
        let mut nodes = rows
            .iter()
            .flat_map(|row| row.values())
            .map(|value| value.as_node().unwrap())
            .collect::<Vec<_>>();
        nodes.sort_unstable_by_key(|node| node.id);
        assert_eq!(nodes.iter().map(|node| node.id).collect::<Vec<_>>(), [7, 9]);
        assert_eq!(nodes[0].vector, query);
        assert_eq!(nodes[1].vector, vec![0.; 129]);
        assert_eq!(nodes[0].payload, serde_json::json!({"text":"正向"}));
        assert_eq!(nodes[1].payload, serde_json::json!({"text":"反向"}));
    }
    let explain = format!(
        "EXPLAIN SEARCH VECTOR {} TOP 2 RETURN *",
        serde_json::to_string(&query).unwrap()
    );
    let plan = db.tql_nodes(&explain).unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        read_or_recover(&db.memtable)
            .quiver_ssd
            .as_ref()
            .unwrap()
            .generation,
        generation
    );
    db.close().unwrap();
}
