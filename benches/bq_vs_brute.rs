use criterion::{Criterion, black_box, criterion_group, criterion_main};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use triviumdb::Database;
use triviumdb::database::SearchConfig;

fn cleanup_db(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

fn disk_bytes(path: &str) -> u64 {
    ["", ".wal", ".vec", ".flush_ok"]
        .iter()
        .map(|ext| {
            fs::metadata(format!("{}{}", path, ext))
                .map(|meta| meta.len())
                .unwrap_or(0)
        })
        .sum()
}

fn rss_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let Ok(status) = fs::read_to_string("/proc/self/status") else {
            return 0;
        };
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                return rest
                    .split_whitespace()
                    .next()
                    .and_then(|kb| kb.parse::<u64>().ok())
                    .map(|kb| kb * 1024)
                    .unwrap_or(0);
            }
        }
        0
    }

    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

fn update_peak_rss(peak: &mut u64) -> u64 {
    let current = rss_bytes();
    *peak = (*peak).max(current);
    current
}

fn init_resource_report() {
    fs::create_dir_all("target/bench-report").unwrap();
    fs::write(
        "target/bench-report/bq_vs_brute_resource_report.md",
        "# BQ vs BruteForce 资源占用报告\n\n| 场景 | 节点 | 维度 | TopK | 查询数 | 内存估算(bytes) | RSS当前(bytes) | RSS峰值(bytes) | 磁盘占用(bytes) | BruteForce QPS | BQ 5% QPS | BQ 5% Recall | BQ 1% QPS | BQ 1% Recall |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    )
    .unwrap();
    fs::write("target/bench-report/bq_vs_brute_resource_report.jsonl", "").unwrap();
}

struct BqResourceRow<'a> {
    scenario: &'a str,
    n: usize,
    dim: usize,
    top_k: usize,
    num_queries: usize,
    memory_bytes: usize,
    rss_bytes: u64,
    rss_peak_bytes: u64,
    disk_bytes: u64,
    qps_brute: f64,
    qps_bq: f64,
    recall_bq: f64,
    qps_bq_1pct: f64,
    recall_bq_1pct: f64,
}

fn append_resource_report(row: &BqResourceRow<'_>) {
    let mut md = OpenOptions::new()
        .append(true)
        .open("target/bench-report/bq_vs_brute_resource_report.md")
        .unwrap();
    writeln!(
        md,
        "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {:.3} | {:.3} | {:.4} | {:.3} | {:.4} |",
        row.scenario,
        row.n,
        row.dim,
        row.top_k,
        row.num_queries,
        row.memory_bytes,
        row.rss_bytes,
        row.rss_peak_bytes,
        row.disk_bytes,
        row.qps_brute,
        row.qps_bq,
        row.recall_bq,
        row.qps_bq_1pct,
        row.recall_bq_1pct
    )
    .unwrap();

    let mut jsonl = OpenOptions::new()
        .append(true)
        .open("target/bench-report/bq_vs_brute_resource_report.jsonl")
        .unwrap();
    writeln!(
        jsonl,
        "{{\"scenario\":\"{}\",\"n\":{},\"dim\":{},\"top_k\":{},\"num_queries\":{},\"memory_bytes\":{},\"rss_bytes\":{},\"rss_peak_bytes\":{},\"disk_bytes\":{},\"qps_brute\":{:.6},\"qps_bq\":{:.6},\"recall_bq\":{:.6},\"qps_bq_1pct\":{:.6},\"recall_bq_1pct\":{:.6}}}",
        row.scenario,
        row.n,
        row.dim,
        row.top_k,
        row.num_queries,
        row.memory_bytes,
        row.rss_bytes,
        row.rss_peak_bytes,
        row.disk_bytes,
        row.qps_brute,
        row.qps_bq,
        row.recall_bq,
        row.qps_bq_1pct,
        row.recall_bq_1pct
    )
    .unwrap();
}

fn gen_vec(rng: &mut StdRng, dim: usize, center: Option<&[f32]>) -> Vec<f32> {
    let mut v: Vec<f32> = vec![0.0f32; dim];
    if let Some(c) = center {
        for (i, x) in v.iter_mut().enumerate() {
            *x = c[i] * 0.1 + rng.gen_range(-1.0f32..1.0) * 0.9;
        }
    } else {
        for x in v.iter_mut() {
            *x = rng.gen_range(-1.0f32..1.0);
        }
    }
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.into_iter().map(|x| x / norm).collect()
}

fn recall_at_k(ground_truth: &[u64], result: &[u64]) -> f64 {
    if ground_truth.is_empty() {
        return 1.0;
    }
    let gt_set: HashSet<u64> = ground_truth.iter().cloned().collect();
    let hits = result.iter().filter(|id| gt_set.contains(id)).count();
    hits as f64 / ground_truth.len() as f64
}

fn run_precision_report(n: usize, dim: usize, top_k: usize, num_queries: usize) {
    let db_path = format!("bench_bq_n{}_d{}.tdb", n, dim);
    cleanup_db(&db_path);
    let mut rss_peak = rss_bytes();

    let mut db = Database::<f32>::open(&db_path, dim).expect("无法创建数据库");
    db.disable_auto_compaction();
    update_peak_rss(&mut rss_peak);

    let mut rng = StdRng::seed_from_u64(42);
    let num_clusters = 50.max(n / 2000);
    let centers: Vec<Vec<f32>> = (0..num_clusters)
        .map(|_| gen_vec(&mut rng, dim, None))
        .collect();
    update_peak_rss(&mut rss_peak);

    eprintln!("[精度测试] 正在插入 {} 条 {}维向量...", n, dim);
    for i in 0..n {
        let v = gen_vec(&mut rng, dim, Some(&centers[i % num_clusters]));
        db.insert(&v, serde_json::json!({"idx": i})).unwrap();
        if i % 10_000 == 0 {
            update_peak_rss(&mut rss_peak);
        }
    }
    db.flush().unwrap();
    let memory_bytes = db.estimated_memory();
    let storage_bytes = disk_bytes(&db_path);
    update_peak_rss(&mut rss_peak);

    let queries: Vec<Vec<f32>> = (0..num_queries)
        .map(|_| gen_vec(&mut rng, dim, Some(&centers[0])))
        .collect();
    update_peak_rss(&mut rss_peak);

    let brute_config = SearchConfig {
        top_k,
        force_brute_force: true,
        ..Default::default()
    };
    eprintln!(
        "[精度测试] 正在对 {} 个查询跑 BruteForce 真值...",
        num_queries
    );
    let mut total_time_brute = std::time::Duration::ZERO;
    let mut ground_truths = Vec::with_capacity(num_queries);

    for q in &queries {
        let t0 = std::time::Instant::now();
        let gt = db
            .search_hybrid(None, Some(q.as_slice()), &brute_config)
            .unwrap();
        total_time_brute += t0.elapsed();
        ground_truths.push(gt.iter().map(|h| h.id).collect::<Vec<u64>>());
        update_peak_rss(&mut rss_peak);
    }

    let bq_light_config = SearchConfig {
        top_k,
        ..Default::default()
    };
    let bq_1pct_config = SearchConfig {
        top_k,
        ..Default::default()
    };

    let mut total_recall_bq = 0.0f64;
    let mut total_time_bq = std::time::Duration::ZERO;
    let mut total_recall_bq_1pct = 0.0f64;
    let mut total_time_bq_1pct = std::time::Duration::ZERO;

    eprintln!("[精度测试] 正在运行 BQ 极速检索...");
    for (i, q) in queries.iter().enumerate() {
        let gt_ids = &ground_truths[i];

        let t_bq = std::time::Instant::now();
        let res_bq = db
            .search_hybrid(None, Some(q.as_slice()), &bq_light_config)
            .unwrap();
        total_time_bq += t_bq.elapsed();
        let bq_ids: Vec<u64> = res_bq.iter().map(|h| h.id).collect();
        total_recall_bq += recall_at_k(gt_ids, &bq_ids);
        update_peak_rss(&mut rss_peak);

        let t_bq_1pct = std::time::Instant::now();
        let res_bq_1pct = db
            .search_hybrid(None, Some(q.as_slice()), &bq_1pct_config)
            .unwrap();
        total_time_bq_1pct += t_bq_1pct.elapsed();
        let bq_1pct_ids: Vec<u64> = res_bq_1pct.iter().map(|h| h.id).collect();
        total_recall_bq_1pct += recall_at_k(gt_ids, &bq_1pct_ids);
        update_peak_rss(&mut rss_peak);
    }

    let avg_recall_bq = total_recall_bq / num_queries as f64;
    let qps_brute = num_queries as f64 / total_time_brute.as_secs_f64();
    let qps_bq = num_queries as f64 / total_time_bq.as_secs_f64();
    let speedup_bq = qps_bq / qps_brute;

    let avg_recall_bq_1pct = total_recall_bq_1pct / num_queries as f64;
    let qps_bq_1pct = num_queries as f64 / total_time_bq_1pct.as_secs_f64();
    let speedup_bq_1pct = qps_bq_1pct / qps_brute;
    let current_rss = update_peak_rss(&mut rss_peak);

    eprintln!("\n╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║       重构后 BQ vs BruteForce 精度 & 性能报告              ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!(
        "║  数据规模: {:>6} 条  维度: {:>4}  Top: {:>3}  查询: {:>4}    ║",
        n, dim, top_k, num_queries
    );
    eprintln!(
        "║  内存估算: {:>12} bytes  RSS: {:>12} bytes  ║",
        memory_bytes, current_rss
    );
    eprintln!(
        "║  RSS峰值: {:>12} bytes  磁盘占用: {:>12} bytes ║",
        rss_peak, storage_bytes
    );
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!(
        "║  策略                  Recall@{}  QPS         加速比       ║",
        top_k
    );
    eprintln!(
        "║  BruteForce（基准基线）   100.00%  {:>10.1}  1.00x       ║",
        qps_brute
    );
    eprintln!(
        "║  BQ 极速排序 (精查5%)  {:>6.2}%  {:>10.1}  {:.2}x       ║",
        avg_recall_bq * 100.0,
        qps_bq,
        speedup_bq
    );
    eprintln!(
        "║  BQ 极速排序 (精查1%)  {:>6.2}%  {:>10.1}  {:.2}x       ║",
        avg_recall_bq_1pct * 100.0,
        qps_bq_1pct,
        speedup_bq_1pct
    );
    eprintln!("╚══════════════════════════════════════════════════════════════╝\n");

    append_resource_report(&BqResourceRow {
        scenario: "precision_report",
        n,
        dim,
        top_k,
        num_queries,
        memory_bytes,
        rss_bytes: current_rss,
        rss_peak_bytes: rss_peak,
        disk_bytes: storage_bytes,
        qps_brute,
        qps_bq,
        recall_bq: avg_recall_bq,
        qps_bq_1pct,
        recall_bq_1pct: avg_recall_bq_1pct,
    });

    drop(db);
    cleanup_db(&db_path);
}

fn bench_brute_vs_bq(c: &mut Criterion) {
    init_resource_report();

    eprintln!("=== [小规模] 50,000 条 / 512维 / Top10 ===");
    run_precision_report(50_000, 512, 10, 100);

    eprintln!("=== [大规模] 200,000 条 / 1536维 / Top10 ===");
    run_precision_report(200_000, 1536, 10, 20);

    eprintln!("[Criterion] 正在构建 200k×1536d 计时数据库...");
    let db_path = "bench_bq_speed_200k.tdb";
    cleanup_db(db_path);
    let mut rss_peak = rss_bytes();
    let mut db = Database::<f32>::open(db_path, 1536).unwrap();
    db.disable_auto_compaction();

    let mut rng = StdRng::seed_from_u64(99);
    for i in 0..200_000usize {
        let v = gen_vec(&mut rng, 1536, None);
        db.insert(&v, serde_json::json!({"i": i})).unwrap();
        if i % 10_000 == 0 {
            update_peak_rss(&mut rss_peak);
        }
    }
    db.flush().unwrap();
    let criterion_memory_bytes = db.estimated_memory();
    let criterion_storage_bytes = disk_bytes(db_path);
    let criterion_current_rss = update_peak_rss(&mut rss_peak);
    eprintln!(
        "[Criterion] 200k×1536d 资源占用: 内存估算={} bytes, RSS={} bytes, RSS峰值={} bytes, 磁盘占用={} bytes",
        criterion_memory_bytes, criterion_current_rss, rss_peak, criterion_storage_bytes
    );

    let query = gen_vec(&mut rng, 1536, None);

    let brute_cfg = SearchConfig {
        top_k: 10,
        force_brute_force: true,
        ..Default::default()
    };
    let mut group = c.benchmark_group("BQ_Rocket_vs_Brute_200k_dim1536");
    group.sample_size(20);

    group.bench_function("BruteForce", |b| {
        b.iter(|| {
            db.search_hybrid(None, Some(black_box(query.as_slice())), &brute_cfg)
                .unwrap()
        })
    });
    update_peak_rss(&mut rss_peak);

    let bq_cfg = SearchConfig {
        top_k: 10,
        ..Default::default()
    };
    group.bench_function("BQ 3-Stage Rocket (5%)", |b| {
        b.iter(|| {
            db.search_hybrid(None, Some(black_box(query.as_slice())), &bq_cfg)
                .unwrap()
        })
    });
    update_peak_rss(&mut rss_peak);

    let bq_1pct_cfg = SearchConfig {
        top_k: 10,
        ..Default::default()
    };
    group.bench_function("BQ 3-Stage Rocket (1%)", |b| {
        b.iter(|| {
            db.search_hybrid(None, Some(black_box(query.as_slice())), &bq_1pct_cfg)
                .unwrap()
        })
    });
    update_peak_rss(&mut rss_peak);

    group.finish();
    let criterion_final_rss = update_peak_rss(&mut rss_peak);
    append_resource_report(&BqResourceRow {
        scenario: "criterion_speed_db",
        n: 200_000,
        dim: 1536,
        top_k: 10,
        num_queries: 1,
        memory_bytes: criterion_memory_bytes,
        rss_bytes: criterion_final_rss,
        rss_peak_bytes: rss_peak,
        disk_bytes: criterion_storage_bytes,
        qps_brute: 0.0,
        qps_bq: 0.0,
        recall_bq: 0.0,
        qps_bq_1pct: 0.0,
        recall_bq_1pct: 0.0,
    });
    drop(db);

    cleanup_db(db_path);
}

criterion_group!(benches, bench_brute_vs_bq);
criterion_main!(benches);
