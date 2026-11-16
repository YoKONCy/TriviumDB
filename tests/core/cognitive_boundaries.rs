//! 通过公开查询入口验证认知算子的唯一性、数值尺度与有限结果。

use super::support::TestDir;
use serde_json::json;
use triviumdb::database::{Config, StorageMode};
use triviumdb::query::tql_executor::TqlValue;
use triviumdb::{Database, HookContext, SearchConfig, SearchHit, SearchHook};

struct HighQualityRecall;

fn scalar(value: &TqlValue<f32>) -> serde_json::Value {
    match value {
        TqlValue::Int(value) => json!(value),
        TqlValue::Float(value) => json!(value),
        _ => panic!("预期数值标量，实际为 {value:?}"),
    }
}

impl SearchHook for HighQualityRecall {
    fn on_custom_recall(
        &self,
        _: &[f32],
        _: &SearchConfig,
        _: &mut HookContext,
    ) -> Option<Vec<SearchHit>> {
        Some(
            (1..=3)
                .map(|id| SearchHit {
                    id,
                    score: 100.0,
                    payload: json!({}),
                })
                .collect(),
        )
    }
}

#[test]
fn dpp_高质量因子在高级检索和TQL中返回完整的不同节点() {
    let root = TestDir::new("dpp-finite-quality");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let mut db = Database::<f32>::open_with_config(
            &root.file(&format!("{mode:?}.tdb")),
            Config {
                dim: 2,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        for (id, vector) in [(1, [1.0, 0.0]), (2, [0.8, 0.6]), (3, [0.6, 0.8])] {
            db.insert_with_id(id, &vector, json!({})).unwrap();
            db.index_keyword(id, "alpha").unwrap();
        }
        db.set_hook(HighQualityRecall).unwrap();
        let hits = db
            .search_advanced(
                &[1.0, 0.0],
                &SearchConfig {
                    top_k: 2,
                    recall_k: 3,
                    rerank_k: 3,
                    min_score: -1.0,
                    expand_depth: 0,
                    enable_advanced_pipeline: true,
                    enable_dpp: true,
                    dpp_quality_weight: 10.0,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            hits.iter()
                .map(|hit| (hit.id, hit.score))
                .collect::<Vec<_>>(),
            [(1, 100.0), (3, 100.0)]
        );
        let text = "alpha ".repeat(100);
        let rows = db.tql(&format!("TEXT AC \"{text}\" TOP 3 AS hits WITH hits DIVERSIFY hits TOP 2 QUALITY_WEIGHT 10 AS selected WITH selected RETURN selected.id AS id, text_score(selected) AS quality, diversity_score(selected) AS rank ORDER BY rank DESC")).unwrap();
        assert_eq!(rows.len(), 2);
        for (row, id, rank) in [(&rows[0], 1, 1.0), (&rows[1], 3, 0.5)] {
            assert_eq!(scalar(&row["id"]), json!(id));
            assert_eq!(scalar(&row["quality"]), json!(100.0));
            assert_eq!(scalar(&row["rank"]), json!(rank));
        }
        db.close().unwrap();
    }
}

#[test]
fn nmf_大向量在两个存储模式保留全部有限主题权重() {
    let root = TestDir::new("nmf-finite-topics");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for scale in [1.0f32, 1e30, f32::MAX] {
            let mut db = Database::<f32>::open_with_config(
                &root.file(&format!("{mode:?}-{scale}.tdb")),
                Config {
                    dim: 2,
                    storage_mode: mode,
                    auto_build_quiver: false,
                    ..Default::default()
                },
            )
            .unwrap();
            for (id, vector) in [(1, [scale, 0.0]), (2, [0.0, scale]), (3, [scale, scale])] {
                db.insert_with_id(id, &vector, json!({})).unwrap();
            }
            let query = "SEARCH VECTOR [1, 0] TOP 3 AS seed WITH seed TOPICS seed K 2 ITERATIONS 8 AS clustered WITH clustered RETURN clustered.id AS id, topic(clustered) AS topic, topic_score(clustered) AS score ORDER BY id";
            let rows = db.tql(query).unwrap();
            assert_eq!(rows.len(), 3);
            for (index, row) in rows.iter().enumerate() {
                assert_eq!(scalar(&row["id"]), json!(index + 1));
                assert!(matches!(row["topic"], TqlValue::Int(0..=1)));
                assert!(
                    matches!(row["score"], TqlValue::Float(value) if value.is_finite() && value > 0.0),
                    "{mode:?}, {scale}, {row:?}"
                );
            }
            let again = db.tql(query).unwrap();
            assert_eq!(again.len(), 3);
            for (first, second) in rows.iter().zip(&again) {
                for field in ["id", "topic", "score"] {
                    assert_eq!(scalar(&first[field]), scalar(&second[field]));
                }
            }
            db.close().unwrap();
        }
    }
}

#[test]
fn 调和中心性在TQL投影排序和聚合中保留f64尺度() {
    let root = TestDir::new("harmonic-finite-score");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let mut db = Database::<f32>::open_with_config(
            &root.file(&format!("{mode:?}.tdb")),
            Config {
                dim: 2,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        db.insert_with_id(1, &[1.0, 0.0], json!({})).unwrap();
        db.insert_with_id(2, &[0.0, 1.0], json!({})).unwrap();
        let weight = 1e-40f32;
        db.link(1, 2, "rel", weight).unwrap();
        let prefix = "SEARCH VECTOR [0, 0] TOP 2 AS seed WITH seed HARMONIC_CENTRALITY seed LABEL rel AS ranked WITH ranked";
        let rows = db.tql(&format!("{prefix} RETURN ranked.id AS id, harmonic_centrality(ranked) AS score, graph_score(ranked) AS graph ORDER BY score DESC")).unwrap();
        assert_eq!(rows.len(), 2);
        let expected = 1.0 / f64::from(weight);
        for (row, id, score) in [(&rows[0], 1, expected), (&rows[1], 2, 0.0)] {
            assert_eq!(scalar(&row["id"]), json!(id));
            assert_eq!(scalar(&row["score"]), json!(score));
            assert_eq!(scalar(&row["graph"]), json!(score));
        }
        let aggregate = db
            .tql(&format!(
                "{prefix} RETURN AVG(harmonic_centrality(ranked)) AS average"
            ))
            .unwrap();
        assert_eq!(aggregate.len(), 1);
        assert_eq!(scalar(&aggregate[0]["average"]), json!(expected / 2.0));
        db.close().unwrap();
    }
}
