use serde_json::json;
use triviumdb::graph::reachability::ReachabilityDirection;
use triviumdb::{
    Database, Filter, GraphSignalQuery, TsngBudget, TsngQuery, TsngWeights, quality_metrics,
};

const DIM: usize = 3;

fn database(name: &str) -> (String, Database<f32>) {
    let directory = std::env::temp_dir().join("triviumdb_tsng_c0");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{}_{}.tdb", name, std::process::id()));
    let path = path.to_string_lossy().to_string();
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for (id, vector, payload) in [
        (1, [1.0, 0.0, 0.0], json!({"kind": "allowed"})),
        (2, [0.9, 0.1, 0.0], json!({"kind": "blocked"})),
        (3, [0.8, 0.2, 0.0], json!({"kind": "allowed"})),
        (4, [0.0, 1.0, 0.0], json!({"kind": "allowed"})),
        (5, [-1.0, 0.0, 0.0], json!({"kind": "allowed"})),
        (6, [0.0, 0.0, 1.0], json!({"kind": "allowed"})),
    ] {
        db.insert_with_id(id, &vector, payload).unwrap();
    }
    (path, db)
}

fn cleanup(path: &str) {
    for suffix in ["", ".wal", ".vec", ".lock", ".flush_ok", ".pidx"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

fn query<'a>(vector: &'a [f32], top_k: usize) -> TsngQuery<'a, f32> {
    TsngQuery {
        vector,
        payload_filter: None,
        graph: None,
        top_k,
        weights: TsngWeights::default(),
        budget: TsngBudget::default(),
    }
}

#[test]
fn c0_纯向量_ground_truth_与精确余弦排序一致且确定() {
    let (path, mut db) = database("vector_exact");
    let vector = [1.0, 0.0, 0.0];
    let expected = vec![1, 2, 3, 4, 6, 5];
    for _ in 0..20 {
        let output = db.tsng_ground_truth(&query(&vector, 6)).unwrap();
        assert_eq!(
            output.hits.iter().map(|hit| hit.id).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(output.cost.candidates_scanned, 6);
        assert_eq!(output.cost.vector_comparisons, 6);
        assert_eq!(output.cost.payload_checks, 0);
        assert_eq!(output.cost.graph_examined_edges, 0);
    }
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c0_payload_filter_是最终候选硬约束() {
    let (path, mut db) = database("payload_filter");
    let vector = [1.0, 0.0, 0.0];
    let filter = Filter::from_json(&json!({"kind": "allowed"})).unwrap();
    let mut filtered = query(&vector, 6);
    filtered.payload_filter = Some(&filter);
    filtered.weights = TsngWeights {
        vector: 0.7,
        property: 0.3,
        graph: 0.0,
    };
    let output = db.tsng_ground_truth(&filtered).unwrap();
    assert_eq!(
        output.hits.iter().map(|hit| hit.id).collect::<Vec<_>>(),
        vec![1, 3, 4, 6, 5]
    );
    assert!(output.hits.iter().all(|hit| hit.property_signal == 1.0));
    assert_eq!(output.cost.payload_checks, 6);
    assert_eq!(output.cost.vector_comparisons, 5);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c0_图权重在扩展前过滤并保留更长合法路径() {
    let (path, mut db) = database("graph_exact");
    db.link(1, 3, "road", 0.2).unwrap();
    db.link(1, 4, "road", 0.9).unwrap();
    db.link(4, 3, "road", 0.9).unwrap();
    db.link(1, 6, "noise", 1.0).unwrap();
    let vector = [0.0, 1.0, 0.0];
    let graph = GraphSignalQuery {
        anchor_id: 1,
        direction: ReachabilityDirection::Outgoing,
        labels: Some(vec!["road".into()]),
        min_edge_weight: 0.5,
        max_hops: 2,
    };
    let output = db
        .tsng_ground_truth(&TsngQuery {
            vector: &vector,
            payload_filter: None,
            graph: Some(graph),
            top_k: 6,
            weights: TsngWeights {
                vector: 0.0,
                property: 0.0,
                graph: 1.0,
            },
            budget: TsngBudget::default(),
        })
        .unwrap();
    assert_eq!(output.hits[0].id, 4);
    assert_eq!(output.hits[0].graph_depth, Some(1));
    let node3 = output.hits.iter().find(|hit| hit.id == 3).unwrap();
    assert_eq!(node3.graph_depth, Some(2));
    assert_eq!(node3.graph_signal, 0.5);
    assert_eq!(
        output
            .hits
            .iter()
            .find(|hit| hit.id == 6)
            .unwrap()
            .graph_signal,
        0.0
    );
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c0_三信号冲突按精确加权分数与_node_id_稳定决胜() {
    let (path, mut db) = database("tri_signal");
    db.link(1, 4, "related", 1.0).unwrap();
    db.link(1, 3, "related", 1.0).unwrap();
    let vector = [1.0, 0.0, 0.0];
    let filter = Filter::from_json(&json!({"kind": "allowed"})).unwrap();
    let output = db
        .tsng_ground_truth(&TsngQuery {
            vector: &vector,
            payload_filter: Some(&filter),
            graph: Some(GraphSignalQuery {
                anchor_id: 1,
                direction: ReachabilityDirection::Outgoing,
                labels: Some(vec!["related".into()]),
                min_edge_weight: 0.0,
                max_hops: 1,
            }),
            top_k: 5,
            weights: TsngWeights {
                vector: 0.4,
                property: 0.2,
                graph: 0.4,
            },
            budget: TsngBudget::default(),
        })
        .unwrap();
    assert_eq!(output.hits[0].id, 3);
    assert_eq!(output.hits[1].id, 4);
    assert!(!output.hits.iter().any(|hit| hit.id == 2));
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c0_质量指标覆盖完全命中乱序重复与零_k() {
    let (path, mut db) = database("quality");
    let vector = [1.0, 0.0, 0.0];
    let exact = db.tsng_ground_truth(&query(&vector, 5)).unwrap();
    let perfect = quality_metrics(&exact.hits, &[1, 2, 3, 4, 6], 5);
    assert_eq!(perfect.recall_at_k, 1.0);
    assert_eq!(perfect.ndcg_at_k, 1.0);
    let reversed = quality_metrics(&exact.hits, &[6, 4, 3, 2, 1], 5);
    assert_eq!(reversed.recall_at_k, 1.0);
    assert!(reversed.ndcg_at_k < 1.0);
    let duplicate = quality_metrics(&exact.hits, &[1, 1, 1, 1, 1], 5);
    assert_eq!(duplicate.recall_at_k, 0.2);
    assert!(duplicate.ndcg_at_k > 0.0);
    assert!(duplicate.ndcg_at_k < 1.0);
    assert_eq!(quality_metrics(&exact.hits, &[], 0).recall_at_k, 1.0);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn c0_非法维度权重图配置和预算明确报错() {
    let (path, mut db) = database("validation");
    let bad_dim = [1.0, 0.0];
    assert!(db.tsng_ground_truth(&query(&bad_dim, 2)).is_err());

    let vector = [1.0, 0.0, 0.0];
    let mut invalid = query(&vector, 2);
    invalid.weights.vector = f32::NAN;
    assert!(db.tsng_ground_truth(&invalid).is_err());

    let mut missing_filter = query(&vector, 2);
    missing_filter.weights.property = 1.0;
    assert!(db.tsng_ground_truth(&missing_filter).is_err());

    let mut exhausted = query(&vector, 2);
    exhausted.budget.max_candidates = 5;
    assert!(db.tsng_ground_truth(&exhausted).is_err());
    db.close().unwrap();
    cleanup(&path);
}
