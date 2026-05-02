use instant_distance::{Builder, Point, Search};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use triviumdb::Database;
use triviumdb::database::SearchConfig;

#[derive(Clone)]
struct HnswPoint(Vec<f32>);

struct LargeScaleReport {
    n: usize,
    dim: usize,
    top_k: usize,
    num_queries: usize,
    raw_vector_bytes: usize,
    hnsw_input_bytes: usize,
    trivium_memory_bytes: usize,
    trivium_disk_bytes: u64,
    rss_after_dataset_generation: u64,
    rss_after_hnsw_build: u64,
    rss_after_hnsw_drop: u64,
    rss_after_trivium_flush: u64,
    rss_after_bruteforce: u64,
    rss_after_bq: u64,
    rss_peak_bytes: u64,
    hnsw_build_secs: f64,
    trivium_insert_flush_secs: f64,
    hnsw_qps: f64,
    hnsw_recall: f64,
    brute_qps: f64,
    bq_qps: f64,
    bq_recall: f64,
}

impl Point for HnswPoint {
    fn distance(&self, other: &Self) -> f32 {
        self.0
            .iter()
            .zip(other.0.iter())
            .map(|(a, b)| {
                let diff = a - b;
                diff * diff
            })
            .sum()
    }
}

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

fn write_large_scale_report(report: &LargeScaleReport) {
    fs::create_dir_all("target/bench-report").unwrap();
    let md = format!(
        "# 500k 大规模资源占用与性能报告\n\n\
| 指标 | 数值 |\n\
|---|---:|\n\
| 节点数 | {} |\n\
| 维度 | {} |\n\
| TopK | {} |\n\
| 查询数 | {} |\n\
| 原始向量缓存估算(bytes) | {} |\n\
| HNSW 输入点估算(bytes) | {} |\n\
| TriviumDB 内存估算(bytes) | {} |\n\
| TriviumDB 磁盘占用(bytes) | {} |\n\
| 数据集生成后 RSS(bytes) | {} |\n\
| HNSW 建图后 RSS(bytes) | {} |\n\
| HNSW 释放后 RSS(bytes) | {} |\n\
| TriviumDB flush 后 RSS(bytes) | {} |\n\
| BruteForce 查询后 RSS(bytes) | {} |\n\
| BQ 查询后 RSS(bytes) | {} |\n\
| RSS 峰值(bytes) | {} |\n\
| HNSW 建图耗时(s) | {:.3} |\n\
| TriviumDB 插入并 flush 耗时(s) | {:.3} |\n\
| HNSW QPS | {:.3} |\n\
| HNSW Recall | {:.6} |\n\
| BruteForce QPS | {:.3} |\n\
| BQ 5% QPS | {:.3} |\n\
| BQ 5% Recall | {:.6} |\n",
        report.n,
        report.dim,
        report.top_k,
        report.num_queries,
        report.raw_vector_bytes,
        report.hnsw_input_bytes,
        report.trivium_memory_bytes,
        report.trivium_disk_bytes,
        report.rss_after_dataset_generation,
        report.rss_after_hnsw_build,
        report.rss_after_hnsw_drop,
        report.rss_after_trivium_flush,
        report.rss_after_bruteforce,
        report.rss_after_bq,
        report.rss_peak_bytes,
        report.hnsw_build_secs,
        report.trivium_insert_flush_secs,
        report.hnsw_qps,
        report.hnsw_recall,
        report.brute_qps,
        report.bq_qps,
        report.bq_recall
    );
    fs::write("target/bench-report/bench_500k_resource_report.md", md).unwrap();

    let mut jsonl = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("target/bench-report/bench_500k_resource_report.jsonl")
        .unwrap();
    writeln!(
        jsonl,
        "{{\"n\":{},\"dim\":{},\"top_k\":{},\"num_queries\":{},\"raw_vector_bytes\":{},\"hnsw_input_bytes\":{},\"trivium_memory_bytes\":{},\"trivium_disk_bytes\":{},\"rss_after_dataset_generation\":{},\"rss_after_hnsw_build\":{},\"rss_after_hnsw_drop\":{},\"rss_after_trivium_flush\":{},\"rss_after_bruteforce\":{},\"rss_after_bq\":{},\"rss_peak_bytes\":{},\"hnsw_build_secs\":{:.6},\"trivium_insert_flush_secs\":{:.6},\"hnsw_qps\":{:.6},\"hnsw_recall\":{:.6},\"brute_qps\":{:.6},\"bq_qps\":{:.6},\"bq_recall\":{:.6}}}",
        report.n,
        report.dim,
        report.top_k,
        report.num_queries,
        report.raw_vector_bytes,
        report.hnsw_input_bytes,
        report.trivium_memory_bytes,
        report.trivium_disk_bytes,
        report.rss_after_dataset_generation,
        report.rss_after_hnsw_build,
        report.rss_after_hnsw_drop,
        report.rss_after_trivium_flush,
        report.rss_after_bruteforce,
        report.rss_after_bq,
        report.rss_peak_bytes,
        report.hnsw_build_secs,
        report.trivium_insert_flush_secs,
        report.hnsw_qps,
        report.hnsw_recall,
        report.brute_qps,
        report.bq_qps,
        report.bq_recall
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

fn main() {
    let n = 500_000;
    let dim = 1536;
    let top_k = 10;
    let num_queries = 20;
    let mut rss_peak = rss_bytes();

    let db_path = format!("bench_500k_fight_n{}_d{}.tdb", n, dim);
    cleanup_db(&db_path);

    let mut db = Database::<f32>::open(&db_path, dim).expect("无法创建数据库");
    db.disable_auto_compaction();
    update_peak_rss(&mut rss_peak);

    let mut rng = StdRng::seed_from_u64(42);
    let num_clusters = 50;
    let centers: Vec<Vec<f32>> = (0..num_clusters)
        .map(|_| gen_vec(&mut rng, dim, None))
        .collect();

    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║          中坚级压测：500,000 节点 1536 维                  ║");
    eprintln!("╚══════════════════════════════════════════════════════════════╝");

    eprintln!("[阶段一] 生成数据集...");
    let mut hnsw_points = Vec::with_capacity(n);
    let mut hnsw_values = Vec::with_capacity(n);
    let mut raw_vectors = Vec::with_capacity(n);

    for i in 0..n {
        let v = gen_vec(&mut rng, dim, Some(&centers[i % num_clusters]));
        raw_vectors.push(v.clone());
        hnsw_points.push(HnswPoint(v));
        hnsw_values.push(i as u64);
        if i % 10_000 == 0 {
            update_peak_rss(&mut rss_peak);
        }
    }
    let rss_after_dataset_generation = update_peak_rss(&mut rss_peak);
    let raw_vector_bytes = raw_vectors.len() * dim * std::mem::size_of::<f32>();
    let hnsw_input_bytes = hnsw_points.len() * dim * std::mem::size_of::<f32>()
        + hnsw_values.len() * std::mem::size_of::<u64>();
    eprintln!(
        "[资源] 原始向量缓存估算={} bytes, HNSW 输入点估算={} bytes, RSS={} bytes",
        raw_vector_bytes, hnsw_input_bytes, rss_after_dataset_generation
    );

    let queries: Vec<Vec<f32>> = (0..num_queries)
        .map(|_| gen_vec(&mut rng, dim, Some(&centers[0])))
        .collect();
    update_peak_rss(&mut rss_peak);

    eprintln!("\n[阶段二：HNSW] 开始建图...");
    let t0 = std::time::Instant::now();
    let hnsw_index = Builder::default().build(hnsw_points, hnsw_values);
    let hnsw_build_secs = t0.elapsed().as_secs_f64();
    let rss_after_hnsw_build = update_peak_rss(&mut rss_peak);
    eprintln!(
        "HNSW 建图完成！耗时: {:.2}s, RSS={} bytes",
        hnsw_build_secs, rss_after_hnsw_build
    );

    eprintln!("   - 正在计算 HNSW Ground Truth...");
    let mut hnsw_ground_truths: Vec<Vec<u64>> = Vec::with_capacity(num_queries);
    for q in &queries {
        let q_pt = HnswPoint(q.clone());
        let mut dists: Vec<(u64, f32)> = (0..n as u64)
            .map(|i| {
                (
                    i,
                    q_pt.distance(&HnswPoint(raw_vectors[i as usize].clone())),
                )
            })
            .collect();
        dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        hnsw_ground_truths.push(dists.iter().take(top_k).map(|(id, _)| *id).collect());
        update_peak_rss(&mut rss_peak);
    }

    let t_hnsw_search = std::time::Instant::now();
    let mut total_recall_hnsw = 0.0;
    for (i, q) in queries.iter().enumerate() {
        let q_pt = HnswPoint(q.clone());
        let mut search = Search::default();
        let hnsw_results: Vec<u64> = hnsw_index
            .search(&q_pt, &mut search)
            .take(top_k)
            .map(|item| *item.value)
            .collect();
        total_recall_hnsw += recall_at_k(&hnsw_ground_truths[i], &hnsw_results);
        update_peak_rss(&mut rss_peak);
    }
    let hnsw_qps = num_queries as f64 / t_hnsw_search.elapsed().as_secs_f64();
    let avg_recall_hnsw = total_recall_hnsw / num_queries as f64;
    eprintln!(
        "   - HNSW 测试 QPS: {:.2}, Recall@{}: {:.2}%",
        hnsw_qps,
        top_k,
        avg_recall_hnsw * 100.0
    );

    drop(hnsw_index);
    drop(hnsw_ground_truths);
    let rss_after_hnsw_drop = update_peak_rss(&mut rss_peak);

    eprintln!("\n[阶段三：TriviumDB BQ vs Brute] 数据倾泻入库...");
    let t_insert = std::time::Instant::now();
    for (i, v) in raw_vectors.into_iter().enumerate() {
        db.insert(&v, serde_json::json!({"idx": i})).unwrap();
        if i % 10_000 == 0 {
            update_peak_rss(&mut rss_peak);
        }
    }
    db.flush().unwrap();
    let trivium_insert_flush_secs = t_insert.elapsed().as_secs_f64();
    let trivium_memory_bytes = db.estimated_memory();
    let trivium_disk_bytes = disk_bytes(&db_path);
    let rss_after_trivium_flush = update_peak_rss(&mut rss_peak);
    eprintln!(
        "TriviumDB 插入并 flush 耗时: {:.2}s, 内存估算={} bytes, 磁盘占用={} bytes, RSS={} bytes",
        trivium_insert_flush_secs,
        trivium_memory_bytes,
        trivium_disk_bytes,
        rss_after_trivium_flush
    );

    let brute_cfg = SearchConfig {
        top_k,
        force_brute_force: true,
        ..Default::default()
    };
    let bq_cfg = SearchConfig {
        top_k,
        ..Default::default()
    };

    eprintln!("\n[开始查询比对] BruteForce 搜索中...");
    let t_brute = std::time::Instant::now();
    let mut ground_truths = Vec::new();
    for q in &queries {
        let gt = db
            .search_hybrid(None, Some(q.as_slice()), &brute_cfg)
            .unwrap();
        ground_truths.push(gt.iter().map(|h| h.id).collect::<Vec<u64>>());
        update_peak_rss(&mut rss_peak);
    }
    let brute_qps = num_queries as f64 / t_brute.elapsed().as_secs_f64();
    let rss_after_bruteforce = update_peak_rss(&mut rss_peak);

    eprintln!("[开始查询比对] BQ 三段式火箭 (极速粗排 5%) 搜索中...");
    let t_bq = std::time::Instant::now();
    let mut total_recall_bq = 0.0;
    for (i, q) in queries.iter().enumerate() {
        let gt_ids = &ground_truths[i];
        let res_bq = db.search_hybrid(None, Some(q.as_slice()), &bq_cfg).unwrap();
        let bq_ids: Vec<u64> = res_bq.iter().map(|h| h.id).collect();
        total_recall_bq += recall_at_k(gt_ids, &bq_ids);
        update_peak_rss(&mut rss_peak);
    }
    let bq_qps = num_queries as f64 / t_bq.elapsed().as_secs_f64();
    let avg_recall_bq = total_recall_bq / num_queries as f64;
    let rss_after_bq = update_peak_rss(&mut rss_peak);

    eprintln!("\n╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║              50万大测战报                                  ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!(
        "║  数据规模: {:>6} 条  维度: {:>4}  Top: {:>3}  查询: {:>4}    ║",
        n, dim, top_k, num_queries
    );
    eprintln!(
        "║  TriviumDB 内存估算: {:>12} bytes  磁盘: {:>12} bytes ║",
        trivium_memory_bytes, trivium_disk_bytes
    );
    eprintln!(
        "║  RSS当前: {:>12} bytes  RSS峰值: {:>12} bytes ║",
        rss_after_bq, rss_peak
    );
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!(
        "║  策略                   Recall@{}   QPS         加速比      ║",
        top_k
    );
    eprintln!(
        "║  HNSW (instant-distance) {:>6.2}%  {:>10.1}  {:.2}x        ║",
        avg_recall_hnsw * 100.0,
        hnsw_qps,
        hnsw_qps / brute_qps
    );
    eprintln!(
        "║  BruteForce（绝对真值）  100.00%  {:>10.1}  1.00x        ║",
        brute_qps
    );
    eprintln!(
        "║  BQ 三段火箭 (精查5%)   {:>6.2}%  {:>10.1}  {:.2}x        ║",
        avg_recall_bq * 100.0,
        bq_qps,
        bq_qps / brute_qps
    );
    eprintln!("╚══════════════════════════════════════════════════════════════╝");

    write_large_scale_report(&LargeScaleReport {
        n,
        dim,
        top_k,
        num_queries,
        raw_vector_bytes,
        hnsw_input_bytes,
        trivium_memory_bytes,
        trivium_disk_bytes,
        rss_after_dataset_generation,
        rss_after_hnsw_build,
        rss_after_hnsw_drop,
        rss_after_trivium_flush,
        rss_after_bruteforce,
        rss_after_bq,
        rss_peak_bytes: rss_peak,
        hnsw_build_secs,
        trivium_insert_flush_secs,
        hnsw_qps,
        hnsw_recall: avg_recall_hnsw,
        brute_qps,
        bq_qps,
        bq_recall: avg_recall_bq,
    });

    drop(db);
    cleanup_db(&db_path);
}
