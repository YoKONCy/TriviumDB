use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::fs;
use triviumdb::Database;
use triviumdb::database::SearchConfig;

fn cleanup_db(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        fs::remove_file(format!("{}{}", path, ext)).ok();
    }
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
    if ground_truth.is_empty() { return 1.0; }
    let gt_set: HashSet<u64> = ground_truth.iter().cloned().collect();
    let hits = result.iter().filter(|id| gt_set.contains(id)).count();
    hits as f64 / ground_truth.len() as f64
}

fn main() {
    let n = 100_000;
    let dim = 1536;
    let top_k = 10;
    let num_queries = 50;
    let db_path = "bench_bq_n100k_d1536.tdb";
    cleanup_db(db_path);

    let mut db = Database::<f32>::open(db_path, dim).expect("无法创建数据库");
    db.disable_auto_compaction();

    let mut rng = StdRng::seed_from_u64(42);
    let num_clusters = 50;
    let centers: Vec<Vec<f32>> = (0..num_clusters).map(|_| gen_vec(&mut rng, dim, None)).collect();

    eprintln!("[测试] 正在插入 {} 条 {}维向量...", n, dim);
    let t0 = std::time::Instant::now();
    for i in 0..n {
        let v = gen_vec(&mut rng, dim, Some(&centers[i % num_clusters]));
        db.insert(&v, serde_json::json!({"idx": i})).unwrap();
    }
    db.flush().unwrap();
    eprintln!("[测试] 插入完成，耗时 {:.2}s", t0.elapsed().as_secs_f64());

    let queries: Vec<Vec<f32>> = (0..num_queries).map(|_| gen_vec(&mut rng, dim, Some(&centers[0]))).collect();

    let brute_config = SearchConfig {
        top_k, enable_bq_coarse_search: false, force_brute_force: true, ..Default::default()
    };
    eprintln!("[测试] 正在对 {} 个查询跑 BruteForce 真值...", num_queries);
    let mut total_time_brute = std::time::Duration::ZERO;
    let mut ground_truths = Vec::with_capacity(num_queries);

    for q in &queries {
        let t0 = std::time::Instant::now();
        let gt = db.search_hybrid(None, Some(q.as_slice()), &brute_config).unwrap();
        total_time_brute += t0.elapsed();
        ground_truths.push(gt.iter().map(|h| h.id).collect::<Vec<u64>>());
    }

    let bq_5pct_config = SearchConfig {
        top_k, enable_bq_coarse_search: true, bq_candidate_ratio: 0.05, ..Default::default()
    };

    let mut total_recall_bq = 0.0f64;
    let mut total_time_bq = std::time::Duration::ZERO;

    eprintln!("[测试] 正在运行 原版 5% BQ 检索...");
    for (i, q) in queries.iter().enumerate() {
        let gt_ids = &ground_truths[i];
        let t_bq = std::time::Instant::now();
        let res_bq = db.search_hybrid(None, Some(q.as_slice()), &bq_5pct_config).unwrap();
        total_time_bq += t_bq.elapsed();
        let bq_ids: Vec<u64> = res_bq.iter().map(|h| h.id).collect();
        total_recall_bq += recall_at_k(gt_ids, &bq_ids);
    }

    let avg_recall_bq = total_recall_bq / num_queries as f64;
    let qps_brute = num_queries as f64 / total_time_brute.as_secs_f64();
    let qps_bq = num_queries as f64 / total_time_bq.as_secs_f64();
    let speedup_bq = qps_bq / qps_brute;

    eprintln!("═══════════════════════════════════════════════════════════════");
    eprintln!("  原版 BQ 管线测试 (N={}, dim={}, Top{})", n, dim, top_k);
    eprintln!("═══════════════════════════════════════════════════════════════");
    eprintln!("  策略                  Recall@{}  QPS         加速比", top_k);
    eprintln!("  BruteForce（基线）     100.00%  {:>10.1}  1.00x", qps_brute);
    eprintln!("  原版 BQ 3-Stage (5%)   {:>6.2}%  {:>10.1}  {:.2}x", avg_recall_bq * 100.0, qps_bq, speedup_bq);
    eprintln!("═══════════════════════════════════════════════════════════════");

    drop(db);
    cleanup_db(db_path);
}
