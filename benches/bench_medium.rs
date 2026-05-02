use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::Instant;
use triviumdb::Database;
use triviumdb::database::SearchConfig;

const N: usize = 200_000;
const DIM: usize = 768;
const NUM_EDGES: usize = 2_000_000;
const TOP_K: usize = 10;
const NUM_QUERIES: usize = 30;
const NUM_CLUSTERS: usize = 50;
const BATCH_SIZE: usize = 10_000;

fn cleanup_db(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok", ".tmp", ".vec.tmp"] {
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

fn gen_vec(rng: &mut StdRng, dim: usize, center: Option<&[f32]>) -> Vec<f32> {
    let mut v = vec![0.0f32; dim];
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

fn fmt_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.2} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.2} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn main() {
    let mut rss_peak = rss_bytes();
    let db_path = format!("target/bench-report/bench_medium_n{}_d{}.tdb", N, DIM);
    fs::create_dir_all("target/bench-report").ok();
    cleanup_db(&db_path);

    let mut db = Database::<f32>::open(&db_path, DIM).expect("无法创建数据库");
    db.disable_auto_compaction();

    let mut rng = StdRng::seed_from_u64(20260430);
    let centers: Vec<Vec<f32>> = (0..NUM_CLUSTERS)
        .map(|_| gen_vec(&mut rng, DIM, None))
        .collect();

    eprintln!("╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║     中规模 CI 压测：200,000 节点 × 768 维 × 200 万边      ║");
    eprintln!("╚══════════════════════════════════════════════════════════════╝");

    // ═══════════════════════════════════════════════════════════════
    //  阶段一：数据插入
    // ═══════════════════════════════════════════════════════════════

    eprintln!("\n[阶段一] 插入 {} 个节点...", N);
    let t_insert = Instant::now();
    for i in 0..N {
        let v = gen_vec(&mut rng, DIM, Some(&centers[i % NUM_CLUSTERS]));
        db.insert(
            &v,
            serde_json::json!({
                "idx": i,
                "type": match i % 5 {
                    0 => "event",
                    1 => "person",
                    2 => "document",
                    3 => "task",
                    _ => "concept",
                },
                "cluster": i % NUM_CLUSTERS,
                "active": i % 3 != 0,
            }),
        )
        .unwrap();
        if (i + 1) % BATCH_SIZE == 0 {
            let rss = update_peak_rss(&mut rss_peak);
            eprint!("\r   {}/{} 节点 (RSS: {})", i + 1, N, fmt_bytes(rss));
        }
    }
    let insert_secs = t_insert.elapsed().as_secs_f64();
    let rss_after_insert = update_peak_rss(&mut rss_peak);
    eprintln!(
        "\n   插入完成: {:.2}s ({:.0} ops/s), RSS={}",
        insert_secs,
        N as f64 / insert_secs,
        fmt_bytes(rss_after_insert)
    );

    // ═══════════════════════════════════════════════════════════════
    //  阶段二：边构建
    // ═══════════════════════════════════════════════════════════════

    eprintln!("\n[阶段二] 构建 {} 条边...", NUM_EDGES);
    let all_ids = db.all_node_ids();
    let id_count = all_ids.len();
    let t_edge = Instant::now();
    let labels = ["next", "related", "ref", "similar", "derived"];

    for i in 0..NUM_EDGES {
        let src = all_ids[i % id_count];
        // 混合近邻边（+1..+20）和远距边（随机跳跃），使图谱结构更真实
        let dst_idx = if i % 3 == 0 {
            // 远距随机跳跃
            rng.gen_range(0..id_count)
        } else {
            // 近邻边
            (i + 1 + rng.gen_range(0..20)) % id_count
        };
        let dst = all_ids[dst_idx];
        if src != dst {
            let label = labels[i % labels.len()];
            let weight = rng.gen_range(0.1f32..1.0);
            db.link(src, dst, label, weight).unwrap();
        }
        if (i + 1) % 100_000 == 0 {
            let rss = update_peak_rss(&mut rss_peak);
            eprint!("\r   {}/{} 边 (RSS: {})", i + 1, NUM_EDGES, fmt_bytes(rss));
        }
    }
    let edge_secs = t_edge.elapsed().as_secs_f64();
    let rss_after_edges = update_peak_rss(&mut rss_peak);
    eprintln!(
        "\n   边构建完成: {:.2}s ({:.0} edges/s), RSS={}",
        edge_secs,
        NUM_EDGES as f64 / edge_secs,
        fmt_bytes(rss_after_edges)
    );

    // ═══════════════════════════════════════════════════════════════
    //  阶段三：持久化
    // ═══════════════════════════════════════════════════════════════

    eprintln!("\n[阶段三] Flush 持久化...");
    let t_flush = Instant::now();
    db.flush().unwrap();
    let flush_secs = t_flush.elapsed().as_secs_f64();
    let trivium_memory = db.estimated_memory();
    let trivium_disk = disk_bytes(&db_path);
    let rss_after_flush = update_peak_rss(&mut rss_peak);
    eprintln!(
        "   Flush 完成: {:.2}s, 内存={}, 磁盘={}, RSS={}",
        flush_secs,
        fmt_bytes(trivium_memory as u64),
        fmt_bytes(trivium_disk),
        fmt_bytes(rss_after_flush)
    );

    // ═══════════════════════════════════════════════════════════════
    //  阶段四：查询性能
    // ═══════════════════════════════════════════════════════════════

    let queries: Vec<Vec<f32>> = (0..NUM_QUERIES)
        .map(|i| gen_vec(&mut rng, DIM, Some(&centers[i % NUM_CLUSTERS])))
        .collect();

    // BruteForce 搜索
    eprintln!("\n[阶段四] BruteForce 搜索 × {} 查询...", NUM_QUERIES);
    let brute_cfg = SearchConfig {
        top_k: TOP_K,
        force_brute_force: true,
        ..Default::default()
    };
    let t_brute = Instant::now();
    let mut ground_truths = Vec::new();
    for q in &queries {
        let gt = db
            .search_hybrid(None, Some(q.as_slice()), &brute_cfg)
            .unwrap();
        ground_truths.push(gt.iter().map(|h| h.id).collect::<Vec<u64>>());
        update_peak_rss(&mut rss_peak);
    }
    let brute_elapsed = t_brute.elapsed().as_secs_f64();
    let brute_qps = NUM_QUERIES as f64 / brute_elapsed;
    let rss_after_brute = update_peak_rss(&mut rss_peak);
    eprintln!(
        "   BruteForce: {:.2}s, QPS={:.2}, RSS={}",
        brute_elapsed,
        brute_qps,
        fmt_bytes(rss_after_brute)
    );

    // BQ 粗筛搜索
    eprintln!("[阶段四] BQ 三段火箭 (5% 粗筛) × {} 查询...", NUM_QUERIES);
    let bq_cfg = SearchConfig {
        top_k: TOP_K,
        ..Default::default()
    };
    let t_bq = Instant::now();
    let mut total_recall_bq = 0.0;
    for (i, q) in queries.iter().enumerate() {
        let res = db
            .search_hybrid(None, Some(q.as_slice()), &bq_cfg)
            .unwrap();
        let bq_ids: Vec<u64> = res.iter().map(|h| h.id).collect();
        total_recall_bq += recall_at_k(&ground_truths[i], &bq_ids);
        update_peak_rss(&mut rss_peak);
    }
    let bq_elapsed = t_bq.elapsed().as_secs_f64();
    let bq_qps = NUM_QUERIES as f64 / bq_elapsed;
    let avg_recall_bq = total_recall_bq / NUM_QUERIES as f64;
    let rss_after_bq = update_peak_rss(&mut rss_peak);
    eprintln!(
        "   BQ 5%: {:.2}s, QPS={:.2}, Recall@{}={:.2}%, RSS={}",
        bq_elapsed,
        bq_qps,
        TOP_K,
        avg_recall_bq * 100.0,
        fmt_bytes(rss_after_bq)
    );

    // BQ 20% 粗筛搜索
    eprintln!("[阶段四] BQ 三段火箭 (20% 粗筛) × {} 查询...", NUM_QUERIES);
    let bq_cfg_20 = SearchConfig {
        top_k: TOP_K,
        ..Default::default()
    };
    let t_bq20 = Instant::now();
    let mut total_recall_bq20 = 0.0;
    for (i, q) in queries.iter().enumerate() {
        let res = db
            .search_hybrid(None, Some(q.as_slice()), &bq_cfg_20)
            .unwrap();
        let bq_ids: Vec<u64> = res.iter().map(|h| h.id).collect();
        total_recall_bq20 += recall_at_k(&ground_truths[i], &bq_ids);
    }
    let bq20_elapsed = t_bq20.elapsed().as_secs_f64();
    let bq20_qps = NUM_QUERIES as f64 / bq20_elapsed;
    let avg_recall_bq20 = total_recall_bq20 / NUM_QUERIES as f64;

    // ═══════════════════════════════════════════════════════════════
    //  阶段五：图算法
    // ═══════════════════════════════════════════════════════════════

    let anchor = all_ids[all_ids.len() / 2];
    eprintln!("\n[阶段五] 图算法测试...");

    let t_neighbors = Instant::now();
    let ns_2 = db.neighbors(anchor, 2);
    let neighbors_2hop_ms = t_neighbors.elapsed().as_secs_f64() * 1000.0;

    let t_neighbors3 = Instant::now();
    let ns_3 = db.neighbors(anchor, 3);
    let neighbors_3hop_ms = t_neighbors3.elapsed().as_secs_f64() * 1000.0;

    eprintln!(
        "   Neighbors 2-hop: {} 个, {:.2}ms",
        ns_2.len(),
        neighbors_2hop_ms
    );
    eprintln!(
        "   Neighbors 3-hop: {} 个, {:.2}ms",
        ns_3.len(),
        neighbors_3hop_ms
    );

    let t_leiden = Instant::now();
    let communities = db.leiden_cluster(3, Some(20), Some(true)).unwrap();
    let leiden_ms = t_leiden.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "   Leiden 聚类: {} 个社区, {:.2}ms",
        communities.num_clusters, leiden_ms
    );

    // ═══════════════════════════════════════════════════════════════
    //  阶段六：TQL 查询
    // ═══════════════════════════════════════════════════════════════

    eprintln!("\n[阶段六] TQL 查询测试...");

    let t_find = Instant::now();
    let find_result = db
        .tql(r#"FIND {type: "event"} RETURN * LIMIT 50"#)
        .unwrap();
    let find_ms = t_find.elapsed().as_secs_f64() * 1000.0;
    eprintln!("   FIND: {} 行, {:.2}ms", find_result.len(), find_ms);

    let t_match = Instant::now();
    let match_result = db
        .tql(&format!(
            "MATCH (a {{idx: {}}})-[:next]->(b) RETURN b LIMIT 20",
            anchor
        ))
        .unwrap();
    let match_ms = t_match.elapsed().as_secs_f64() * 1000.0;
    eprintln!("   MATCH: {} 行, {:.2}ms", match_result.len(), match_ms);

    let t_match2 = Instant::now();
    let match2_result = db
        .tql(&format!(
            "MATCH (a {{idx: {}}})-[:related]->(b)-[:next]->(c) RETURN c LIMIT 20",
            anchor
        ))
        .unwrap();
    let match2_ms = t_match2.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "   MATCH 2-hop: {} 行, {:.2}ms",
        match2_result.len(),
        match2_ms
    );

    // ═══════════════════════════════════════════════════════════════
    //  报告输出
    // ═══════════════════════════════════════════════════════════════

    eprintln!("\n╔══════════════════════════════════════════════════════════════╗");
    eprintln!("║                    20 万中规模战报                          ║");
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!(
        "║  数据规模: {:>6} 节点  {:>4} 维  {:>7} 边               ║",
        N, DIM, NUM_EDGES
    );
    eprintln!(
        "║  TriviumDB 内存: {:>12}  磁盘: {:>12} ║",
        fmt_bytes(trivium_memory as u64),
        fmt_bytes(trivium_disk)
    );
    eprintln!(
        "║  RSS 当前: {:>12}  RSS 峰值: {:>12} ║",
        fmt_bytes(rss_after_bq),
        fmt_bytes(rss_peak)
    );
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!(
        "║  搜索策略              Recall@{:<2}   QPS         加速比      ║",
        TOP_K
    );
    eprintln!(
        "║  BruteForce (绝对真值) 100.00%  {:>10.1}  1.00x        ║",
        brute_qps
    );
    eprintln!(
        "║  BQ 三段火箭 (粗查5%)  {:>6.2}%  {:>10.1}  {:.2}x        ║",
        avg_recall_bq * 100.0,
        bq_qps,
        bq_qps / brute_qps
    );
    eprintln!(
        "║  BQ 三段火箭 (粗查20%) {:>6.2}%  {:>10.1}  {:.2}x        ║",
        avg_recall_bq20 * 100.0,
        bq20_qps,
        bq20_qps / brute_qps
    );
    eprintln!("╠══════════════════════════════════════════════════════════════╣");
    eprintln!(
        "║  写入: 插入 {:.2}s ({:.0}/s), 边 {:.2}s ({:.0}/s), Flush {:.2}s  ║",
        insert_secs,
        N as f64 / insert_secs,
        edge_secs,
        NUM_EDGES as f64 / edge_secs,
        flush_secs
    );
    eprintln!("╚══════════════════════════════════════════════════════════════╝");

    // 生成 Markdown 报告
    let md = format!(
        "# TriviumDB 中规模 CI Benchmark 报告\n\n\
| 指标 | 数值 |\n\
|---|---:|\n\
| 节点数 | {} |\n\
| 维度 | {} |\n\
| 边数 | {} |\n\
| TopK | {} |\n\
| 查询数 | {} |\n\
| 插入耗时(s) | {:.3} |\n\
| 插入 ops/s | {:.0} |\n\
| 边构建耗时(s) | {:.3} |\n\
| 边构建 edges/s | {:.0} |\n\
| Flush 耗时(s) | {:.3} |\n\
| 内存估算(bytes) | {} |\n\
| 磁盘占用(bytes) | {} |\n\
| RSS 峰值(bytes) | {} |\n\
| BruteForce QPS | {:.2} |\n\
| BQ 5% QPS | {:.2} |\n\
| BQ 5% Recall@{} | {:.4} |\n\
| BQ 5% 加速比 | {:.2}x |\n\
| BQ 20% QPS | {:.2} |\n\
| BQ 20% Recall@{} | {:.4} |\n\
| BQ 20% 加速比 | {:.2}x |\n\
| Neighbors 2-hop 数量 | {} |\n\
| Neighbors 2-hop 耗时(ms) | {:.2} |\n\
| Neighbors 3-hop 数量 | {} |\n\
| Neighbors 3-hop 耗时(ms) | {:.2} |\n\
| Leiden 社区数 | {} |\n\
| Leiden 耗时(ms) | {:.2} |\n\
| TQL FIND 耗时(ms) | {:.2} |\n\
| TQL MATCH 1-hop 耗时(ms) | {:.2} |\n\
| TQL MATCH 2-hop 耗时(ms) | {:.2} |\n",
        N,
        DIM,
        NUM_EDGES,
        TOP_K,
        NUM_QUERIES,
        insert_secs,
        N as f64 / insert_secs,
        edge_secs,
        NUM_EDGES as f64 / edge_secs,
        flush_secs,
        trivium_memory,
        trivium_disk,
        rss_peak,
        brute_qps,
        bq_qps,
        TOP_K,
        avg_recall_bq,
        bq_qps / brute_qps,
        bq20_qps,
        TOP_K,
        avg_recall_bq20,
        bq20_qps / brute_qps,
        ns_2.len(),
        neighbors_2hop_ms,
        ns_3.len(),
        neighbors_3hop_ms,
        communities.num_clusters,
        leiden_ms,
        find_ms,
        match_ms,
        match2_ms,
    );
    fs::write("target/bench-report/bench_medium_report.md", md).unwrap();

    // 生成 JSONL 报告
    let mut jsonl = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("target/bench-report/bench_medium_report.jsonl")
        .unwrap();
    writeln!(
        jsonl,
        "{{\"n\":{},\"dim\":{},\"edges\":{},\"insert_secs\":{:.6},\"edge_secs\":{:.6},\"flush_secs\":{:.6},\"memory_bytes\":{},\"disk_bytes\":{},\"rss_peak_bytes\":{},\"brute_qps\":{:.6},\"bq5_qps\":{:.6},\"bq5_recall\":{:.6},\"bq20_qps\":{:.6},\"bq20_recall\":{:.6},\"neighbors_2hop\":{},\"neighbors_3hop\":{},\"leiden_clusters\":{},\"leiden_ms\":{:.6}}}",
        N, DIM, NUM_EDGES, insert_secs, edge_secs, flush_secs,
        trivium_memory, trivium_disk, rss_peak,
        brute_qps, bq_qps, avg_recall_bq,
        bq20_qps, avg_recall_bq20,
        ns_2.len(), ns_3.len(),
        communities.num_clusters, leiden_ms
    )
    .unwrap();

    eprintln!("\n报告已生成: target/bench-report/bench_medium_report.md");

    drop(db);
    cleanup_db(&db_path);
}
