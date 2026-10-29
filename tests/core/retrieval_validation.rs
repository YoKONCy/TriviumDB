//! 非有限向量和数量算术在所有 Result 检索入口保持一致。
use super::support::TestDir;
use serde_json::json;
use triviumdb::database::{BatchSearchConfig, Config, SearchConfig, StorageMode};
use triviumdb::{Database, TriviumError, VectorType};

fn invalid_vectors<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("invalid_query_vectors");
        let path = root.file("data.tdb");
        let config = Config {
            dim: 2,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<T>::open_with_config(&path, config).unwrap();
        for phase in 0..3 {
            if phase == 1 {
                db.insert_with_id(1, &[T::from_f32(1.), T::zero()], json!({}))
                    .unwrap();
            }
            if phase == 2 {
                db.close().unwrap();
                db = Database::open_with_config(&path, config).unwrap();
            }
            for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let query = [T::from_f32(invalid), T::zero()];
                assert!(matches!(
                    db.search_exact(&query, 1),
                    Err(TriviumError::InvalidVector { .. })
                ));
                assert!(matches!(
                    db.search(&query, 1, 0, -1.),
                    Err(TriviumError::InvalidVector { .. })
                ));
                assert!(matches!(
                    db.search_advanced(&query, &SearchConfig::default()),
                    Err(TriviumError::InvalidVector { .. })
                ));
                for anchors in [vec![], vec![1]] {
                    assert!(matches!(
                        db.search_graph_first(&query, &anchors, 1, 10),
                        Err(TriviumError::InvalidVector { .. })
                    ));
                }
                assert!(matches!(
                    db.search_batch(
                        &[query.to_vec()],
                        &SearchConfig::default(),
                        &BatchSearchConfig::default()
                    ),
                    Err(TriviumError::InvalidVector { .. })
                ));
            }
            assert!(matches!(
                db.search_exact(&[], 1),
                Err(TriviumError::DimensionMismatch {
                    expected: 2,
                    got: 0
                })
            ));
        }
        db.close().unwrap();
    }
}

#[test]
fn 浮点检索在空库持有库及重开后一致拒绝非有限向量() {
    invalid_vectors::<f32>();
    invalid_vectors::<half::f16>();
}

#[test]
fn 极大候选数量在精确和索引路径有界返回实际内容() {
    let root = TestDir::new("huge_topk");
    let mut db = Database::<f32>::open_with_config(
        &root.file("data.tdb"),
        Config {
            dim: 2,
            auto_build_quiver: false,
            ..Default::default()
        },
    )
    .unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"sign":1})).unwrap();
    db.insert_with_id(2, &[-1., 0.], json!({"sign":-1}))
        .unwrap();
    let tuples = |hits: Vec<triviumdb::node::SearchHit>| {
        hits.into_iter()
            .map(|hit| (hit.id, hit.score, hit.payload))
            .collect::<Vec<_>>()
    };
    let expected = vec![(1, 1., json!({"sign":1})), (2, -1., json!({"sign":-1}))];
    for indexed in [false, true] {
        if indexed {
            db.build_quiver_index(None).unwrap();
        }
        for count in [usize::MAX / 8 + 1, usize::MAX / 2 + 1, usize::MAX] {
            assert_eq!(
                tuples(db.search(&[1., 0.], count, 0, -1.).unwrap()),
                expected
            );
            assert_eq!(tuples(db.search_exact(&[1., 0.], count).unwrap()), expected);
            assert_eq!(
                tuples(db.search_graph_first(&[1., 0.], &[2, 1], count, 2).unwrap()),
                expected
            );
            for (recall_k, rerank_k) in [(0, 0), (count, count)] {
                let config = SearchConfig {
                    top_k: count,
                    recall_k,
                    rerank_k,
                    expand_depth: 0,
                    min_score: -1.,
                    ..Default::default()
                };
                assert_eq!(
                    tuples(db.search_advanced(&[1., 0.], &config).unwrap()),
                    expected
                );
                let batches = db
                    .search_batch(&[vec![1., 0.]], &config, &BatchSearchConfig::default())
                    .unwrap();
                assert_eq!(tuples(batches.into_iter().next().unwrap()), expected);
            }
        }
    }
    db.close().unwrap();
}

#[test]
fn 直接物理检索算子在空候选前拒绝非有限输入() {
    use triviumdb::query::pipeline::{
        ExactRerank, ExactVectorSearch, FistaResidualRecall, NodeSet, PipelineBudget,
        PipelineContext, PipelineOperator, QuiverVectorSearch,
    };
    let mt = triviumdb::storage::memtable::MemTable::<f32>::new(2);
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let query = vec![invalid, 0.];
        let operators: Vec<Box<dyn PipelineOperator<f32>>> = vec![
            Box::new(ExactVectorSearch {
                query: query.clone(),
                top_k: 1,
            }),
            Box::new(QuiverVectorSearch {
                query: query.clone(),
                top_k: 1,
                ef_search: 8,
            }),
            Box::new(ExactRerank {
                query: query.clone(),
                top_k: Some(1),
            }),
            Box::new(FistaResidualRecall {
                query,
                top_k: 1,
                lambda: 0.1,
                threshold: 0.1,
                iterations: 1,
            }),
        ];
        for operator in operators {
            let mut context = PipelineContext::new(&mt, PipelineBudget::default());
            assert!(
                matches!(
                    operator.apply(NodeSet::empty(), &mut context),
                    Err(TriviumError::InvalidVector { .. })
                ),
                "{}",
                operator.name()
            );
        }
    }
}
