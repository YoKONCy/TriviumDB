//! 文本与认知 Pipeline 的生成式 metamorphic 契约。
//!
//! 认知算法不使用生产实现作为数值 oracle；验证集合、边界、有限值、确定性和多阶段组合关系。

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::BTreeSet;
use triviumdb::Database;
use triviumdb::query::tql_executor::TqlValue;

fn fixture(name: &str) -> (Database<f32>, String) {
    let root = std::env::temp_dir().join(format!("triviumdb_cognitive_generator_{name}"));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.tdb").to_string_lossy().to_string();
    super::matrix::cleanup(&path);
    let mut database = Database::<f32>::open(&path, 3).unwrap();
    let texts = [
        "rust memory safety database",
        "rust graph vector search",
        "python scripting language",
        "database graph query engine",
        "memory ownership and safety",
        "vector retrieval database",
        "graph community algorithm",
        "rust embedded storage",
    ];
    for (index, text) in texts.into_iter().enumerate() {
        let id = index as u64 + 1;
        database
            .insert_with_id(
                id,
                &[id as f32 / 8.0, (8 - index) as f32 / 8.0, 1.0],
                serde_json::json!({"id": id, "text": text}),
            )
            .unwrap();
        database.index_text(id, text).unwrap();
    }
    database.build_text_index().unwrap();
    (database, path)
}

fn ids(rows: &[std::collections::HashMap<String, TqlValue<f32>>], column: &str) -> Vec<u64> {
    rows.iter()
        .map(|row| match &row[column] {
            TqlValue::Node(node) => node.id,
            other => panic!("{column} 应为 Node，实际为 {other:?}"),
        })
        .collect()
}

fn finite_score(row: &std::collections::HashMap<String, TqlValue<f32>>, column: &str) -> f64 {
    match row[column] {
        TqlValue::Float(value) if value.is_finite() => value,
        ref other => panic!("{column} 应为有限 Float，实际为 {other:?}"),
    }
}

#[test]
fn 文本入口固定种子查询重复执行确定且top_k满足前缀关系() {
    let (database, path) = fixture("text");
    for kind in ["bm25", "ac", "hybrid"] {
        for term in ["rust", "database graph", "memory safety", "vector search"] {
            let small = format!(
                "TEXT {kind} \"{term}\" TOP 3 AS hit WITH hit RETURN hit, text_score(hit) AS score ORDER BY text_score(hit) DESC"
            );
            let large = format!(
                "TEXT {kind} \"{term}\" TOP 6 AS hit WITH hit RETURN hit, text_score(hit) AS score ORDER BY text_score(hit) DESC"
            );
            let first = database.tql_values(&small).unwrap();
            let second = database.tql_values(&small).unwrap();
            let expanded = database.tql_values(&large).unwrap();
            assert_eq!(ids(&first, "hit"), ids(&second, "hit"));
            assert_eq!(ids(&first, "hit"), ids(&expanded, "hit")[..first.len()]);
            assert!(
                first
                    .iter()
                    .all(|row| finite_score(row, "score").is_finite())
            );
        }
    }
    drop(database);
    super::matrix::cleanup(&path);
}

#[test]
fn dpp生成参数下输出是输入子集且top_k硬边界成立() {
    let (database, path) = fixture("dpp");
    let mut rng = StdRng::seed_from_u64(0x4450_5001);
    for _ in 0..80 {
        let source_k = rng.gen_range(2..=8);
        let output_k = rng.gen_range(1..=source_k);
        let quality = [0.0, 0.5, 1.0, 2.0][rng.gen_range(0..4)];
        let source =
            format!("SEARCH VECTOR [0.5, 0.5, 1] TOP {source_k} AS seed WITH seed RETURN seed");
        let query = format!(
            "SEARCH VECTOR [0.5, 0.5, 1] TOP {source_k} AS seed WITH seed DIVERSIFY seed TOP {output_k} QUALITY_WEIGHT {quality} AS diverse WITH diverse RETURN diverse, diversity_score(diverse) AS score"
        );
        let source_rows = database.tql_values(&source).unwrap();
        let first = database.tql_values(&query).unwrap();
        let second = database.tql_values(&query).unwrap();
        let source_ids = ids(&source_rows, "seed")
            .into_iter()
            .collect::<BTreeSet<_>>();
        let output_ids = ids(&first, "diverse");
        assert!(!first.is_empty());
        assert!(first.len() <= output_k);
        assert_eq!(output_ids, ids(&second, "diverse"));
        assert!(output_ids.iter().all(|id| source_ids.contains(id)));
        assert!(
            first
                .iter()
                .all(|row| finite_score(row, "score").is_finite())
        );
    }
    drop(database);
    super::matrix::cleanup(&path);
}

#[test]
fn residual与topics固定种子参数满足有界有限和行保持契约() {
    let (database, path) = fixture("residual_topics");
    let mut rng = StdRng::seed_from_u64(0x4E4D_4601);
    for _ in 0..60 {
        let source_k = rng.gen_range(2..=8);
        let output_k = rng.gen_range(1..=source_k);
        let iterations = rng.gen_range(1..=16);
        let residual = format!(
            "SEARCH VECTOR [0.5, 0.5, 1] TOP {source_k} AS seed WITH seed RESIDUAL seed BY VECTOR [1, 0, 1] TOP {output_k} LAMBDA 0.1 THRESHOLD 0 ITERATIONS {iterations} AS shadow WITH shadow RETURN shadow, residual_score(shadow) AS score"
        );
        let first = database.tql_values(&residual).unwrap();
        let second = database.tql_values(&residual).unwrap();
        assert!(first.len() <= output_k);
        assert_eq!(ids(&first, "shadow"), ids(&second, "shadow"));
        assert!(
            first
                .iter()
                .all(|row| finite_score(row, "score").is_finite())
        );

        let topics = rng.gen_range(1..=source_k.min(4));
        let query = format!(
            "SEARCH VECTOR [0.5, 0.5, 1] TOP {source_k} AS seed WITH seed TOPICS seed K {topics} ITERATIONS {iterations} AS clustered WITH clustered RETURN clustered, topic(clustered) AS topic_id, topic_score(clustered) AS score"
        );
        let first = database.tql_values(&query).unwrap();
        let second = database.tql_values(&query).unwrap();
        assert_eq!(first.len(), source_k);
        assert_eq!(ids(&first, "clustered"), ids(&second, "clustered"));
        for row in &first {
            let TqlValue::Int(topic) = row["topic_id"] else {
                panic!("topic_id 应为 Int")
            };
            assert!((0..topics as i64).contains(&topic));
            finite_score(row, "score");
        }
    }
    drop(database);
    super::matrix::cleanup(&path);
}

#[test]
fn 多阶段认知with保留前序指标并且聚合与逐行结果一致() {
    let (database, path) = fixture("multi_stage");
    let diverse = database
        .tql_values(
            "SEARCH VECTOR [0.5, 0.5, 1] TOP 8 AS seed WITH seed DIVERSIFY seed TOP 6 QUALITY_WEIGHT 1 AS diverse WITH diverse RETURN diverse, diversity_score(diverse) AS diversity",
        )
        .unwrap();
    let diversity = diverse
        .iter()
        .map(|row| {
            let TqlValue::Node(node) = &row["diverse"] else {
                panic!("diverse 应为 Node")
            };
            (node.id, finite_score(row, "diversity"))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let rows = database
        .tql_values(
            "SEARCH VECTOR [0.5, 0.5, 1] TOP 8 AS seed WITH seed DIVERSIFY seed TOP 6 QUALITY_WEIGHT 1 AS diverse WITH diverse RESIDUAL diverse BY VECTOR [1, 0, 1] TOP 4 LAMBDA 0.1 THRESHOLD 0 ITERATIONS 8 AS shadow WITH shadow TOPICS shadow K 2 ITERATIONS 8 AS clustered WITH clustered RETURN clustered, diversity_score(clustered) AS diversity, residual_score(clustered) AS residual, topic(clustered) AS topic_id, topic_score(clustered) AS topic_score ORDER BY clustered.id ASC",
        )
        .unwrap();
    assert!(!rows.is_empty());
    assert!(rows.len() <= 4);
    let expected_ids = ids(&rows, "clustered");
    assert!(rows.iter().all(|row| {
        finite_score(row, "residual").is_finite()
            && finite_score(row, "topic_score").is_finite()
            && matches!(row["topic_id"], TqlValue::Int(_))
    }));
    for row in &rows {
        let TqlValue::Node(node) = &row["clustered"] else {
            panic!("clustered 应为 Node")
        };
        match diversity.get(&node.id) {
            Some(expected) => assert!((finite_score(row, "diversity") - expected).abs() < 1e-6),
            None => assert!(matches!(row["diversity"], TqlValue::Null)),
        }
    }
    let aggregate = database
        .tql_values(
            "SEARCH VECTOR [0.5, 0.5, 1] TOP 8 AS seed WITH seed DIVERSIFY seed TOP 6 QUALITY_WEIGHT 1 AS diverse WITH diverse RESIDUAL diverse BY VECTOR [1, 0, 1] TOP 4 LAMBDA 0.1 THRESHOLD 0 ITERATIONS 8 AS shadow WITH shadow TOPICS shadow K 2 ITERATIONS 8 AS clustered WITH clustered RETURN count(*) AS total, collect(clustered.id) AS ids, min(topic_score(clustered)) AS min_topic, max(topic_score(clustered)) AS max_topic",
        )
        .unwrap();
    assert_eq!(aggregate.len(), 1);
    assert!(matches!(aggregate[0]["total"], TqlValue::Int(total) if total == rows.len() as i64));
    let TqlValue::List(ids) = &aggregate[0]["ids"] else {
        panic!("ids 应为 List")
    };
    assert_eq!(
        ids.iter()
            .map(|id| id.as_u64().unwrap())
            .collect::<Vec<_>>(),
        expected_ids
    );
    assert!(finite_score(&aggregate[0], "min_topic") <= finite_score(&aggregate[0], "max_topic"));
    drop(database);
    super::matrix::cleanup(&path);
}
