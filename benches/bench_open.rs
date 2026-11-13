//! 独立记录数据库重开、首查与稳定查询，输出完整结果供版本间核对。
//! 合成数据覆盖不同维度与分布，计时只用于基准报告。

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Instant;
use triviumdb::Database;
use triviumdb::database::{AccessMode, Config, MissingIndexPolicy, SearchConfig};
use triviumdb::index::quiver::QuIVerConfig;

mod support;

fn cosine(left: &[f32], right: &[f32]) -> f64 {
    let mut dot = 0.;
    let mut left_norm = 0.;
    let mut right_norm = 0.;
    for (&left, &right) in left.iter().zip(right) {
        let (left, right) = (f64::from(left), f64::from(right));
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    dot / (left_norm * right_norm).sqrt()
}

fn vectors(kind: usize, nodes: usize, dim: usize) -> Vec<f32> {
    let mut seed = 20261112u64;
    (0..nodes * dim)
        .map(|offset| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let noise = (seed >> 32) as i32 as f32 / i32::MAX as f32;
            let row = offset / dim;
            let axis = offset % dim;
            match kind {
                1 => ((row % 16 + axis * 3) as f32 * 0.19).sin() + noise * 0.05,
                2 if !axis.is_multiple_of(8) => 0.,
                3 => ((row / 4 + axis) as f32 * 0.03).cos() + noise * 1e-4,
                _ => noise,
            }
        })
        .collect()
}

fn main() {
    let root = PathBuf::from(
        std::env::var("TDB_OPEN_ROOT")
            .unwrap_or_else(|_| "target/bench-reports/open-databases".into()),
    );
    std::fs::create_dir_all(&root).expect("创建重开基准目录失败");
    let nodes = std::env::var("TDB_OPEN_NODES")
        .map(|value| value.parse::<usize>().expect("节点数量无效"))
        .unwrap_or(5_000);
    assert!(nodes >= 32, "重开基准至少需要32个节点");
    let rewrite = std::env::var("TDB_OPEN_REWRITE").as_deref() == Ok("1");
    let mut reports = Vec::new();
    for (kind, (name, dim)) in [
        ("随机", 128),
        ("聚簇", 384),
        ("稀疏", 768),
        ("近重复", 1536),
    ]
    .into_iter()
    .enumerate()
    {
        let base = vectors(kind, nodes, dim);
        let path = root.join(format!("case-{kind}.tdb"));
        let path = path.to_str().expect("基准路径必须为UTF-8");
        let config = Config {
            dim,
            auto_build_quiver: false,
            missing_index_policy: MissingIndexPolicy::Error,
            ..Config::default()
        };
        if !std::path::Path::new(path).exists() {
            let mut db =
                Database::<f32>::open_with_config(path, config).expect("创建重开基准数据库失败");
            let mut transaction = db.begin_tx();
            for (row, vector) in base.chunks_exact(dim).enumerate() {
                transaction.insert_with_id(
                    row as u64 + 1,
                    vector,
                    json!({"dataset": name, "row": row}),
                );
            }
            transaction.commit().expect("写入重开基准数据失败");
            db.build_quiver_index(Some(QuIVerConfig {
                m: 16,
                ef_construction: 64,
                alpha: 1.2,
            }))
            .expect("构建重开基准索引失败");
            db.close().expect("关闭基准数据库失败");
        }
        if rewrite {
            let mut db = Database::<f32>::open_with_config(path, config)
                .expect("打开待重写的基准数据库失败");
            db.flush().expect("重写基准检查点失败");
            db.close().expect("关闭重写的基准数据库失败");
        }
        let queries = (0..16)
            .map(|index| {
                let row = (index * 313 + 7) % nodes;
                base[row * dim..(row + 1) * dim].to_vec()
            })
            .collect::<Vec<_>>();
        let truth = queries
            .iter()
            .map(|query| {
                let mut scores = base
                    .chunks_exact(dim)
                    .enumerate()
                    .map(|(row, vector)| (row as u64 + 1, cosine(query, vector)))
                    .collect::<Vec<_>>();
                scores.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
                scores[..10].iter().map(|entry| entry.0).collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let mut rounds = Vec::new();
        for _ in 0..3 {
            let started = Instant::now();
            let mut db = Database::<f32>::open_with_config(
                path,
                Config {
                    access_mode: AccessMode::ReadOnly,
                    ..config
                },
            )
            .expect("重开基准数据库失败");
            let open_ms = started.elapsed().as_secs_f64() * 1000.;
            assert!(db.quiver_sm_l2_enabled(), "重开基准要求默认SM索引就绪");
            let resident =
                triviumdb::observability::process_memory_snapshot().expect("读取进程内存观测失败");
            assert_eq!(db.node_count().unwrap(), nodes);
            let started = Instant::now();
            assert_eq!(db.search(&queries[0], 10, 0, -1.).unwrap().len(), 10);
            let first_query_ms = started.elapsed().as_secs_f64() * 1000.;
            let mut points = Vec::new();
            for recall_k in [20, 80, 160] {
                let search = SearchConfig {
                    top_k: 10,
                    recall_k,
                    rerank_k: 64,
                    expand_depth: 0,
                    min_score: -1.,
                    ..SearchConfig::default()
                };
                let started = Instant::now();
                let answers = queries
                    .iter()
                    .map(|query| db.search_advanced(query, &search).unwrap())
                    .collect::<Vec<_>>();
                let elapsed = started.elapsed().as_secs_f64();
                let mut matched = 0;
                let mut serialized = Vec::new();
                for (query_index, hits) in answers.iter().enumerate() {
                    assert_eq!(hits.len(), 10);
                    assert!(hits.windows(2).all(|pair| pair[0].score > pair[1].score
                        || (pair[0].score == pair[1].score && pair[0].id < pair[1].id)));
                    let mut seen = std::collections::BTreeSet::new();
                    let mut rows = Vec::new();
                    for hit in hits {
                        assert!(seen.insert(hit.id), "查询结果包含重复身份");
                        let row = usize::try_from(hit.id - 1).expect("查询身份超出范围");
                        assert!(row < nodes);
                        assert_eq!(hit.payload, json!({"dataset": name, "row": row}));
                        let expected =
                            cosine(&queries[query_index], &base[row * dim..(row + 1) * dim]);
                        assert!((f64::from(hit.score) - expected).abs() < 1e-4);
                        matched += usize::from(truth[query_index].contains(&hit.id));
                        rows.push(json!({"id": hit.id, "score_bits": hit.score.to_bits()}));
                    }
                    serialized.push(rows);
                }
                points.push(json!({
                    "recall_k": recall_k,
                    "recall_at_10": matched as f64 / 160.,
                    "qps": 16. / elapsed,
                    "mean_latency_ms": elapsed * 1000. / 16.,
                    "answers": serialized,
                }));
            }
            rounds.push(json!({
                "open_ms": open_ms,
                "first_query_ms": first_query_ms,
                "rss_after_open": resident.rss_bytes,
                "estimated_memory_bytes": db.estimated_memory(),
                "index_memory": db.index_memory_stats(),
                "resources": db.resource_stats(),
                "points": points,
            }));
            db.close().expect("关闭重开基准Reader失败");
        }
        reports.push(json!({"name": name, "dim": dim, "nodes": nodes, "rounds": rounds}));
    }
    let output = std::env::var("TDB_OPEN_REPORT").unwrap_or_else(|_| "open.json".into());
    let report: Value = json!({
        "schema_version": 1,
        "parameters": { "nodes": nodes, "rounds": 3, "queries": 16, "m": 16, "ef_construction": 64, "alpha": 1.2 },
        "platform": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH },
        "cache_condition": "同进程连续重开，OS文件缓存可能已预热",
        "cases": reports,
    });
    support::write_json_report(output, &report);
}
