//! 生产INT8路径的缓存容量、I/O、召回及资源测量。计时用于报告，结果等价按完整命中核对。
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;
use std::time::Instant;
use triviumdb::index::quiver::ssd::{
    IoMode, QueryPrecision, RerankEncoding, Residency, SsdOptions, SsdQuiver, SsdRuntimeOptions,
};
use triviumdb::index::quiver::{QuIVer, QuIVerConfig, QuIVerSearchConfig};
use triviumdb::{ResourceLimits, ResourceRuntime};

mod support;

fn file_hash(path: &std::path::Path) -> String {
    let mut file = std::fs::File::open(path).expect("打开基准制品失败");
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let bytes = file.read(&mut buffer).expect("读取基准制品失败");
        if bytes == 0 {
            break;
        }
        hash.update(&buffer[..bytes]);
    }
    format!("{:x}", hash.finalize())
}

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

fn main() {
    let nodes = support::env_usize("TDB_SSD_NODES", 4096);
    let query_count = support::env_usize("TDB_SSD_QUERIES", 64);
    let rounds = support::env_usize("TDB_SSD_ROUNDS", 3);
    let encoding: RerankEncoding = serde_json::from_value(json!(
        std::env::var("TDB_SSD_ENCODING").unwrap_or_else(|_| "int8".into())
    ))
    .expect("重排编码无效 (Invalid reranking encoding)");
    let precision: QueryPrecision = serde_json::from_value(json!(
        std::env::var("TDB_SSD_QUERY_PRECISION").unwrap_or_else(|_| "int8".into())
    ))
    .expect("查询精度无效 (Invalid query precision)");
    assert!(nodes >= 32 && query_count > 0 && rounds > 0, "基准规模无效");
    let root = PathBuf::from(
        std::env::var("TDB_SSD_ROOT").unwrap_or_else(|_| "target/bench-reports/quiver-ssd".into()),
    );
    std::fs::create_dir_all(&root).expect("创建SSD基准目录失败");
    let mut reports = Vec::new();
    for (dataset, dim) in [("uniform", 128), ("clustered", 129), ("sparse", 768)] {
        let mut seed = 20261113u64;
        let mut noise = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 32) as i32 as f32 / i32::MAX as f32
        };
        let base = (0..nodes * dim)
            .map(|offset| {
                let value = noise();
                match dataset {
                    "clustered" => {
                        ((offset / dim % 32 + offset % dim * 3) as f32 * 0.19).sin() + value * 0.1
                    }
                    "sparse" if !(offset % dim).is_multiple_of(8) => 0.,
                    _ => value,
                }
            })
            .collect::<Vec<_>>();
        let queries = (0..query_count)
            .map(|row| {
                let start = (row * 37 % nodes) * dim;
                base[start..start + dim]
                    .iter()
                    .map(|&value| value + noise() * 0.05)
                    .collect::<Vec<_>>()
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
                scores
                    .into_iter()
                    .take(10)
                    .map(|hit| hit.0)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let mut digest = Sha256::new();
        digest.update(b"TDB-SSD-BENCH-20261113");
        for value in &base {
            digest.update(value.to_le_bytes());
        }
        let source: [u8; 32] = digest.finalize().into();
        let path = root.join(format!("{dataset}-{nodes}-{dim}-{encoding:?}.qssd"));
        let graph_path = root.join(format!("{dataset}-{nodes}-{dim}.quiver"));
        let runtime = ResourceRuntime::new(ResourceLimits {
            max_threads: 4,
            ..Default::default()
        })
        .unwrap();
        if !path.exists() {
            let index = if graph_path.exists() {
                QuIVer::load_from_file(&graph_path)
                    .expect("读取固定基准图失败 (Failed to load the fixed benchmark graph)")
            } else {
                let mut index = QuIVer::new(
                    dim,
                    &QuIVerConfig {
                        m: 16,
                        ef_construction: 96,
                        alpha: 1.2,
                    },
                );
                let ids = (1..=nodes as u64).collect::<Vec<_>>();
                let slots = (0..nodes).collect::<Vec<_>>();
                index.batch_build(&base, &ids, &slots);
                index
                    .save_to_file(&graph_path)
                    .expect("保存固定基准图失败 (Failed to save the fixed benchmark graph)");
                index
            };
            SsdQuiver::publish(
                &index,
                &path,
                source,
                SsdOptions {
                    encoding,
                    ..Default::default()
                },
                &runtime,
                |slot, output| {
                    output.copy_from_slice(&base[slot * dim..(slot + 1) * dim]);
                    Ok(())
                },
            )
            .unwrap();
        }
        let artifact_hash = file_hash(&path);
        for ef in [32, 96] {
            let config = QuIVerSearchConfig {
                top_k: 10,
                ef_search: ef,
                rerank_limit: Some(ef),
            };
            let mut reference = None;
            for round in 0..rounds {
                for position in 0..6 {
                    // 每轮轮换执行顺序，各配置重新打开并使用相同的预热查询。
                    let variant = (position + round) % 6;
                    let runtime = ResourceRuntime::new(ResourceLimits {
                        max_threads: 4,
                        ..Default::default()
                    })
                    .unwrap();
                    let opened = Instant::now();
                    let mut reader =
                        SsdQuiver::open(&path, source, SsdOptions::default(), runtime.clone())
                            .unwrap();
                    let open_ms = opened.elapsed().as_secs_f64() * 1000.;
                    let full_cache = nodes * (reader.stats().metadata.record_bytes + 512);
                    let (mode, residency, cache_bytes) = match variant {
                        0 => (IoMode::Buffered, Residency::Ssd, 0),
                        1 => (IoMode::Direct, Residency::Ssd, 0),
                        2 => (IoMode::Direct, Residency::Ssd, full_cache / 16),
                        3 => (IoMode::Direct, Residency::Ssd, full_cache / 4),
                        4 => (IoMode::Direct, Residency::Ssd, full_cache),
                        _ => (IoMode::Direct, Residency::Hot, 0),
                    };
                    let configured = Instant::now();
                    reader
                        .configure(SsdRuntimeOptions {
                            residency,
                            cache_bytes,
                            io_mode: mode,
                            io_depth: 3,
                            encoding,
                            query_precision: precision,
                            ..Default::default()
                        })
                        .unwrap();
                    let configure_ms = configured.elapsed().as_secs_f64() * 1000.;
                    for query in queries.iter().take(8) {
                        reader.search(query, &config).unwrap();
                    }
                    let before = serde_json::to_value(reader.stats()).unwrap();
                    let mut latencies = Vec::new();
                    let mut answers = Vec::new();
                    let started = Instant::now();
                    for query in &queries {
                        let started = Instant::now();
                        answers.push(reader.search(query, &config).unwrap());
                        latencies.push(started.elapsed().as_secs_f64() * 1000.);
                    }
                    let elapsed = started.elapsed().as_secs_f64();
                    let after = serde_json::to_value(reader.stats()).unwrap();
                    if let Some(reference) = &reference {
                        assert_eq!(&answers, reference, "驻留及缓存策略改变了完整结果");
                    } else {
                        reference = Some(answers.clone());
                    }
                    let mut matched = 0;
                    let mut hash = 0;
                    for (hits, truth) in answers.iter().zip(&truth) {
                        assert_eq!(hits.len(), 10);
                        for &(id, score) in hits {
                            matched += usize::from(truth.contains(&id));
                            hash = support::mix_hash(
                                support::mix_hash(hash, id),
                                u64::from(score.to_bits()),
                            );
                        }
                    }
                    latencies.sort_by(f64::total_cmp);
                    reports.push(json!({"dataset":dataset,"dimension":dim,"nodes":nodes,"queries":query_count,
                        "round":round,"variant":variant,"ef":ef,"source":source,"artifact_sha256":artifact_hash,"open_ms":open_ms,"configure_ms":configure_ms,
                        "qps":query_count as f64 / elapsed,"p50_ms":support::percentile_sorted(&latencies,50),
                        "p95_ms":support::percentile_sorted(&latencies,95),"p99_ms":support::percentile_sorted(&latencies,99),
                        "recall_at_10":matched as f64 / (query_count * 10) as f64,"result_hash":hash,
                        "before":before,"after":after,"runtime":runtime.stats(),
                        "process_memory":triviumdb::observability::process_memory_snapshot()}));
                    reader.close();
                    assert_eq!(
                        runtime.stats().reserved_workspace_bytes,
                        0,
                        "关闭后仍有工作区租约"
                    );
                }
            }
        }
    }
    let report = support::write_json_report(
        "quiver-ssd.json",
        &json!({"schema_version":1,
        "platform":std::env::consts::OS,"architecture":std::env::consts::ARCH,
        "encoding":encoding,"query_precision":precision,"rounds":reports}),
    );
    println!(
        "SSD测量完成 (SSD measurement completed) {}",
        report.display()
    );
}
