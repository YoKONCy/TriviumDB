//! 查询边界以固定 NodeId 次序、独立数值和跨存储会话验证结果。
use crate::support;

use serde_json::json;
use triviumdb::database::{Config, SearchConfig, StorageMode};
use triviumdb::index::quiver::{BqNavigationScorer, QuIVer, QuIVerConfig, QuIVerSearchConfig};
use triviumdb::{Database, VectorType};

#[test]
fn 精确开关与默认阈值遵守跨语言搜索契约() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/search_entry_cases.json")).unwrap();
    let root = support::TestDir::new("search_entry");
    let records = fixture["records"].as_array().unwrap();
    let query: Vec<f32> = serde_json::from_value(fixture["query"].clone()).unwrap();
    for storage_mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{storage_mode:?}.tdb"));
        let config = Config {
            dim: 2,
            storage_mode,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        for record in records {
            let vector: Vec<f32> = serde_json::from_value(record["vector"].clone()).unwrap();
            db.insert_with_id(
                record["id"].as_u64().unwrap(),
                &vector,
                record["payload"].clone(),
            )
            .unwrap();
        }
        for reopened in [false, true] {
            if reopened {
                db.close().unwrap();
                db = Database::<f32>::open_with_config(&path, config).unwrap();
            }
            for case in fixture["cases"].as_array().unwrap() {
                let mut search = SearchConfig {
                    top_k: case["top_k"].as_u64().unwrap_or(records.len() as u64) as usize,
                    expand_depth: 0,
                    payload_filter: case
                        .get("filter")
                        .map(|f| triviumdb::Filter::from_json(f).unwrap()),
                    ..Default::default()
                }
                .with_exact(case["exact"].as_bool().unwrap());
                if let Some(score) = case["min_score"].as_f64() {
                    search.min_score = score as f32;
                }
                let hits = db.search_advanced(&query, &search).unwrap();
                let expected: Vec<u64> = serde_json::from_value(case["ids"].clone()).unwrap();
                assert_eq!(
                    hits.iter().map(|h| h.id).collect::<Vec<_>>(),
                    expected,
                    "{case}"
                );
                for hit in &hits {
                    let record = records.iter().find(|r| r["id"] == hit.id).unwrap();
                    assert_eq!(hit.payload, record["payload"]);
                    assert!((hit.score as f64 - record["score"].as_f64().unwrap()).abs() < 1e-6);
                }
                let batch = db
                    .search_batch(std::slice::from_ref(&query), &search, &Default::default())
                    .unwrap();
                assert_eq!(batch.len(), 1);
                assert_eq!(batch[0].iter().map(|h| h.id).collect::<Vec<_>>(), expected);
                for hit in &batch[0] {
                    let record = records.iter().find(|r| r["id"] == hit.id).unwrap();
                    assert_eq!(hit.payload, record["payload"]);
                    assert!((hit.score as f64 - record["score"].as_f64().unwrap()).abs() < 1e-6);
                }
            }
        }
        db.close().unwrap();
    }
    assert_eq!(SearchConfig::default().min_score, 0.0);
    assert!(SearchConfig::default().with_exact(true).force_brute_force);
    assert!(
        !SearchConfig::default()
            .with_exact(true)
            .with_exact(false)
            .force_brute_force
    );
}

#[test]
fn 精确开关跳过自动建图并绕过已有索引() {
    let root = support::TestDir::new("exact_route");
    let mut db = Database::<f32>::open(&root.file("data.tdb"), 2).unwrap();
    // 超过当前维度的自动建图阈值，用同分集合独立约定最小 ID 的 Top-K。
    let count = triviumdb::storage::memtable::auto_quiver_node_threshold(2);
    for id in (1..=count as u64).rev() {
        db.insert_with_id(id, &[1., 0.], json!({"id":id})).unwrap();
    }
    let exact = SearchConfig {
        top_k: 3,
        expand_depth: 0,
        ..Default::default()
    }
    .with_exact(true);
    for indexed in [false, true] {
        if indexed {
            db.build_quiver_index(None).unwrap();
        }
        let (hits, ctx) = db
            .search_hybrid_with_context(None, Some(&[1., 0.]), &exact)
            .unwrap();
        assert!(
            ctx.observations
                .contains(&("vector_route_quiver".into(), 0))
        );
        assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), [1, 2, 3]);
        for hit in hits {
            assert_eq!(hit.score, 1.);
            assert_eq!(hit.payload, json!({"id":hit.id}));
        }
        assert_eq!(db.prepare_auto_quiver_build().unwrap().is_some(), !indexed);
    }
    db.close().unwrap();
}

#[test]
fn JSON等值预过滤在写入更新与重开后保留完整命中() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/search_filter_cases.json")).unwrap();
    let records = fixture["records"].as_array().unwrap();
    let root = support::TestDir::new("equality_prefilter");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let cfg = Config {
            dim: 2,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
        for record in records {
            db.insert_with_id(
                record["id"].as_u64().unwrap(),
                &[1., 0.],
                record["payload"].clone(),
            )
            .unwrap();
        }
        // 每轮独立检查期望集合，分别覆盖新签名、冷记录和重新计算后的签名。
        for phase in 0..3 {
            if phase == 1 {
                db.close().unwrap();
                db = Database::<f32>::open_with_config(&path, cfg).unwrap();
            }
            if phase == 2 {
                for record in records {
                    db.update_payload(record["id"].as_u64().unwrap(), record["payload"].clone())
                        .unwrap();
                }
            }
            for case in fixture["cases"].as_array().unwrap() {
                let expected: Vec<u64> = serde_json::from_value(case["ids"].clone()).unwrap();
                let config = SearchConfig {
                    top_k: records.len(),
                    recall_k: records.len(),
                    rerank_k: records.len(),
                    expand_depth: 0,
                    min_score: -1.,
                    force_brute_force: true,
                    payload_filter: Some(triviumdb::Filter::from_json(&case["filter"]).unwrap()),
                    ..Default::default()
                };
                let hits = db.search_advanced(&[1., 0.], &config).unwrap();
                assert_eq!(
                    hits.iter().map(|hit| hit.id).collect::<Vec<_>>(),
                    expected,
                    "{mode:?}/{phase}/{case}"
                );
                for hit in hits {
                    let record = records
                        .iter()
                        .find(|record| record["id"] == hit.id)
                        .unwrap();
                    assert_eq!(hit.payload, record["payload"]);
                    assert_eq!(hit.score, 1.);
                }
            }
        }
        db.close().unwrap();
    }
}

struct RewriteQuery(Vec<f32>);
impl triviumdb::hook::SearchHook for RewriteQuery {
    fn on_pre_search(
        &self,
        query: &mut Vec<f32>,
        _: &mut SearchConfig,
        _: &mut triviumdb::hook::HookContext,
    ) {
        query.clone_from(&self.0);
    }
}

#[test]
fn Hook向量在f16转换后执行有限性校验且失败后可继续查询() {
    let root = support::TestDir::new("hook_f16_overflow");
    let mut db = Database::<half::f16>::open(&root.file("data.tdb"), 2).unwrap();
    let query = [half::f16::ONE, half::f16::ZERO];
    db.insert_with_id(1, &query, json!({"value":"kept"}))
        .unwrap();
    assert_eq!(
        db.search(&[half::f16::from_f32(70000.), half::f16::ZERO], 1, 0, -1.)
            .unwrap_err()
            .code(),
        "TDB_INVALID_VECTOR"
    );
    db.set_hook(RewriteQuery(vec![70000., 0.])).unwrap();
    assert_eq!(
        db.search(&query, 1, 0, -1.).unwrap_err().code(),
        "TDB_INVALID_VECTOR"
    );
    db.set_hook(RewriteQuery(vec![60000., 0.])).unwrap();
    let result = db.search(&query, 1, 0, -1.).unwrap();
    assert_eq!(result.iter().map(|hit| hit.id).collect::<Vec<_>>(), [1]);
    assert_eq!(result[0].payload, json!({"value":"kept"}));
    db.close().unwrap();
}

struct StageErrorHook {
    stage: &'static str,
    calls: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
}

impl StageErrorHook {
    fn enter(&self, stage: &'static str, ctx: &mut triviumdb::hook::HookContext) {
        self.calls.lock().unwrap().push(stage);
        if self.stage == stage {
            ctx.error = Some("受控回调错误 (Controlled callback error)".into());
        }
    }
}

impl triviumdb::hook::SearchHook for StageErrorHook {
    fn on_custom_recall(
        &self,
        _: &[f32],
        _: &SearchConfig,
        ctx: &mut triviumdb::hook::HookContext,
    ) -> Option<Vec<triviumdb::SearchHit>> {
        self.enter("custom", ctx);
        None
    }
    fn on_post_recall(
        &self,
        _: &mut Vec<triviumdb::SearchHit>,
        ctx: &mut triviumdb::hook::HookContext,
    ) {
        self.enter("post_recall", ctx);
    }
    fn on_pre_graph_expand(
        &self,
        _: &mut Vec<triviumdb::SearchHit>,
        ctx: &mut triviumdb::hook::HookContext,
    ) {
        self.enter("pre_graph", ctx);
    }
    fn on_post_search(
        &self,
        _: &mut Vec<triviumdb::SearchHit>,
        ctx: &mut triviumdb::hook::HookContext,
    ) {
        self.enter("post_search", ctx);
    }
}

#[test]
fn Hook错误在空召回普通返回与DPP返回处一致传播() {
    let root = support::TestDir::new("hook_errors");
    let mut db = Database::<f32>::open(&root.file("data.tdb"), 2).unwrap();
    for populated in [false, true] {
        if populated {
            db.insert_with_id(1, &[1., 0.], json!({"id":1})).unwrap();
            db.insert_with_id(2, &[0., 1.], json!({"id":2})).unwrap();
        }
        for dpp in [false, true] {
            let stages: &[&str] = if populated {
                &["custom", "post_recall", "pre_graph", "post_search"]
            } else {
                &["custom", "post_recall"]
            };
            for &stage in stages {
                let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
                db.set_hook(StageErrorHook {
                    stage,
                    calls: calls.clone(),
                })
                .unwrap();
                let config = SearchConfig {
                    top_k: 1,
                    recall_k: 2,
                    min_score: -1.,
                    enable_advanced_pipeline: true,
                    enable_dpp: dpp,
                    ..Default::default()
                };
                let error = db
                    .search_hybrid_with_context(None, Some(&[1., 0.]), &config)
                    .unwrap_err();
                assert_eq!(error.code(), "TDB_HOOK_EXECUTION");
                assert!(matches!(
                    error,
                    triviumdb::TriviumError::HookExecutionError(_)
                ));
                let expected = ["custom", "post_recall", "pre_graph", "post_search"]
                    .into_iter()
                    .take_while(|current| *current != stage)
                    .chain(std::iter::once(stage))
                    .collect::<Vec<_>>();
                assert_eq!(*calls.lock().unwrap(), expected);
            }
        }
    }
    db.close().unwrap();
}

fn slot_ties<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>() {
    let root = support::TestDir::new("slot_ties");
    let vector = [T::from_f32(1.), T::from_f32(0.)];
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let cfg = Config {
            dim: 2,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        let mut db = Database::<T>::open_with_config(&path, cfg).unwrap();
        for id in [1, 2, 3] {
            db.insert_with_id(id, &vector, json!({"id":id})).unwrap();
        }
        db.delete(1).unwrap();
        db.insert_with_id(10, &vector, json!({"id":10})).unwrap();
        let search = SearchConfig {
            top_k: 1,
            recall_k: 1,
            rerank_k: 1,
            force_brute_force: true,
            expand_depth: 0,
            min_score: 0.,
            ..Default::default()
        };
        assert_eq!(
            db.search_advanced(&vector, &search)
                .unwrap()
                .iter()
                .map(|h| h.id)
                .collect::<Vec<_>>(),
            [2]
        );
        db.close().unwrap();
        let mut reopened = Database::<T>::open_with_config(&path, cfg).unwrap();
        assert_eq!(
            reopened
                .search_advanced(&vector, &search)
                .unwrap()
                .iter()
                .map(|h| h.id)
                .collect::<Vec<_>>(),
            [2]
        );
        reopened.close().unwrap();
    }
}

#[test]
fn 三种dtype在墓碑复用后同分截断选择最小nodeid() {
    slot_ties::<f32>();
    slot_ties::<half::f16>();
    slot_ties::<u64>();
}

#[test]
fn quiver完整候选池公开路径在同分和软删除后保持nodeid顺序() {
    for dim in [3, 384, 768] {
        let n = 48;
        let vectors = vec![1f32; n * dim];
        let ids = (0..n).map(|i| (i * 17 % n + 1) as u64).collect::<Vec<_>>();
        let slots = (0..n).collect::<Vec<_>>();
        let mut index = QuIVer::new(
            dim,
            &QuIVerConfig {
                m: 8,
                ef_construction: 64,
                alpha: 1.2,
            },
        );
        index.batch_build(&vectors, &ids, &slots);
        for deleted in [false, true] {
            if deleted {
                index.soft_delete(1);
            }
            for k in [1, 3, 13, 48] {
                let expected = ((if deleted { 2 } else { 1 })..=48)
                    .take(k)
                    .collect::<Vec<u64>>();
                let config = QuIVerSearchConfig {
                    top_k: k,
                    ef_search: n,
                    rerank_limit: Some(n),
                };
                let query = vec![1.; dim];
                let fill = |_: usize, out: &mut Vec<f32>| {
                    out.clear();
                    out.resize(dim, 1.);
                    true
                };
                let outputs = [
                    index.search(&query, fill, &config),
                    index.search_with_scorer(&query, fill, &config, &mut BqNavigationScorer),
                ];
                for hits in outputs {
                    assert_eq!(
                        hits.iter().map(|h| h.0).collect::<Vec<_>>(),
                        expected,
                        "dim={dim}, k={k}"
                    );
                    assert!(
                        hits.iter()
                            .all(|h| h.1.is_finite() && (h.1 - 1.).abs() < 1e-5)
                    );
                }
            }
        }
    }
}

#[test]
fn quiver非法查询与冷向量在普通和384维路径一致拒绝() {
    for dim in [3, 384] {
        let mut index = QuIVer::new(dim, &QuIVerConfig::default());
        index.batch_build(&vec![1.; 4 * dim], &[4, 3, 2, 1], &[0, 1, 2, 3]);
        let config = QuIVerSearchConfig {
            top_k: 3,
            ef_search: 4,
            rerank_limit: Some(4),
        };
        let fill = |_: usize, out: &mut Vec<f32>| {
            out.clear();
            out.resize(dim, 1.);
            true
        };
        for query in [
            vec![],
            vec![1.; dim - 1],
            vec![1.; dim + 1],
            vec![f32::NAN; dim],
            vec![f32::INFINITY; dim],
        ] {
            assert!(index.search(&query, fill, &config).is_empty());
            assert!(
                index
                    .search_with_scorer(&query, fill, &config, &mut BqNavigationScorer)
                    .is_empty()
            );
        }
        let query = vec![1.; dim];
        let zero = QuIVerSearchConfig {
            top_k: 0,
            ef_search: usize::MAX,
            rerank_limit: Some(usize::MAX),
        };
        assert!(
            index
                .search_with_scorer(&query, fill, &zero, &mut BqNavigationScorer)
                .is_empty()
        );
        for invalid in [f32::NAN, f32::INFINITY] {
            let fill = |_: usize, out: &mut Vec<f32>| {
                out.clear();
                out.resize(dim, invalid);
                true
            };
            assert!(
                index
                    .search_with_scorer(&query, fill, &config, &mut BqNavigationScorer)
                    .is_empty()
            );
        }
        let fill = |_: usize, out: &mut Vec<f32>| {
            out.clear();
            out.resize(dim - 1, 1.);
            true
        };
        assert!(
            index
                .search_with_scorer(&query, fill, &config, &mut BqNavigationScorer)
                .is_empty()
        );
    }
}

#[test]
fn quiver精排名额由有效冷向量占用且同分在截断前处理() {
    for dim in [3, 384] {
        let mut index = QuIVer::new(dim, &QuIVerConfig::default());
        index.batch_build(&vec![1.; 4 * dim], &[4, 3, 2, 1], &[0, 1, 2, 3]);
        index.soft_delete(1);
        let cfg = QuIVerSearchConfig {
            top_k: 1,
            ef_search: 4,
            rerank_limit: Some(1),
        };
        let fill = |slot: usize, out: &mut Vec<f32>| {
            out.clear();
            out.resize(dim, if slot == 2 { f32::NAN } else { 1. });
            true
        };
        let query = vec![1.; dim];
        let hits = index.search_with_scorer(&query, fill, &cfg, &mut BqNavigationScorer);
        // ID 1 已删除，ID 2 回调含非法分量，剩余同分中 ID 3 优先。
        assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), [3]);
        assert!((hits[0].1 - 1.).abs() < 1e-5);
    }
}

#[test]
fn 默认sm精排预算截断前按外部nodeid处理同分() {
    let root = support::TestDir::new("sm_low_budget_ties");
    for dim in [3, 384, 768] {
        let path = root.file(&format!("{dim}.tdb"));
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                dim,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        for id in (1..=40).rev() {
            db.insert_with_id(id, &vec![1.; dim], json!({"id":id}))
                .unwrap();
        }
        db.build_quiver_index(Some(QuIVerConfig {
            m: 8,
            ef_construction: 64,
            alpha: 1.2,
        }))
        .unwrap();
        let cfg = SearchConfig {
            top_k: 1,
            recall_k: 40,
            rerank_k: 1,
            expand_depth: 0,
            min_score: 0.,
            ..Default::default()
        };
        assert_eq!(
            db.search_advanced(&vec![1.; dim], &cfg)
                .unwrap()
                .iter()
                .map(|h| h.id)
                .collect::<Vec<_>>(),
            [1]
        );
        db.close().unwrap();
    }
}

#[test]
fn 大整数有序索引的范围排序分页与独立数值次序一致() {
    let root = support::TestDir::new("ordered_number_boundaries");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for values in [
            vec![
                9_007_199_254_740_993i64,
                9_007_199_254_740_992,
                9_007_199_254_740_994,
            ],
            vec![
                -9_007_199_254_740_992,
                -9_007_199_254_740_993,
                -9_007_199_254_740_994,
            ],
        ] {
            let path = root.file(&format!("{mode:?}_{}.tdb", values[0]));
            let cfg = Config {
                dim: 2,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            };
            let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
            for (i, value) in values.iter().enumerate() {
                db.insert_with_id(i as u64 + 1, &[1., 0.], json!({"value":value}))
                    .unwrap();
            }
            let mut expected = values
                .iter()
                .enumerate()
                .map(|(i, v)| (*v, i as u64 + 1))
                .collect::<Vec<_>>();
            expected.sort_unstable();
            db.create_ordered_index("value").unwrap();
            for descending in [false, true] {
                let mut expected = expected.clone();
                if descending {
                    expected.reverse();
                }
                let query = format!(
                    "FIND {{value: {{$exists: true}}}} RETURN * ORDER BY _.value {} LIMIT 1 OFFSET 1",
                    if descending { "DESC" } else { "ASC" }
                );
                let ids = db
                    .tql_nodes(&query)
                    .unwrap()
                    .iter()
                    .map(|row| row["_"].id)
                    .collect::<Vec<_>>();
                assert_eq!(ids, [expected[1].1]);
            }
            db.close().unwrap();
            let mut reader = Database::<f32>::open_with_config(
                &path,
                Config {
                    access_mode: triviumdb::database::AccessMode::ReadOnly,
                    ..cfg
                },
            )
            .unwrap();
            let rows = reader
                .tql_nodes("FIND {value: {$exists: true}} RETURN * ORDER BY _.value ASC LIMIT 1")
                .unwrap();
            assert_eq!(rows[0]["_"].id, expected[0].1);
            reader.close().unwrap();
        }
    }
}

#[test]
fn 完整json整数域与浮点混合排序在索引前后保持数值顺序() {
    // i64/u64 端点、2^53 精度边界和两侧小数分别对应独立的数值次序。
    let values = [
        json!(u64::MAX),
        json!(u64::MAX - 1),
        json!(i64::MAX as u64 + 1),
        json!(9_007_199_254_740_993u64),
        json!(9_007_199_254_740_992u64),
        json!(-1),
        json!(i64::MIN),
        json!(0.25),
        json!(-0.25),
        json!(18_446_744_073_709_551_616f64),
    ];
    let ascending = vec![7, 6, 9, 8, 5, 4, 3, 2, 1, 10];
    let root = support::TestDir::new("mixed_json_order");
    let path = root.file("sample.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    for (i, value) in values.iter().enumerate() {
        db.insert_with_id(i as u64 + 1, &[1., 0.], json!({"value":value}))
            .unwrap();
    }
    for indexed in [false, true] {
        if indexed {
            db.create_ordered_index("value").unwrap();
        }
        for descending in [false, true] {
            let mut expected = ascending.clone();
            if descending {
                expected.reverse();
            }
            let query = format!(
                "FIND {{value: {{$exists: true}}}} RETURN * ORDER BY _.value {}",
                if descending { "DESC" } else { "ASC" }
            );
            assert_eq!(
                db.tql_nodes(&query)
                    .unwrap()
                    .iter()
                    .map(|row| row["_"].id)
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
    db.close().unwrap();
}

#[test]
fn 管线字段排序和标量别名保留u64精度() {
    let root = support::TestDir::new("pipeline_integer_order");
    let mut db = Database::<f32>::open(&root.file("sample.tdb"), 2).unwrap();
    for (id, value) in [
        (1, u64::MAX),
        (2, u64::MAX - 1),
        (3, 9_007_199_254_740_993),
        (4, 9_007_199_254_740_992),
    ] {
        db.insert_with_id(id, &[1., 0.], json!({"value":value}))
            .unwrap();
    }
    for query in [
        "SEARCH VECTOR [1,0] TOP 4 AS n WITH n RETURN n ORDER BY n.value ASC",
        "SEARCH VECTOR [1,0] TOP 4 AS n WITH n, n.value AS number RETURN n ORDER BY number ASC",
    ] {
        let rows = db.tql(query).unwrap();
        let ids = rows
            .iter()
            .map(|row| match &row["n"] {
                triviumdb::query::tql_executor::TqlValue::Node(n) => n.id,
                _ => panic!("预期节点结果"),
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, [4, 3, 2, 1]);
    }
    db.close().unwrap();
}
