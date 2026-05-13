// ══════════════════════════════════════════════════════════════
//  构图方差 + 尾延迟分布实验
//
//  两个目标：
//    1. 多次独立构图方差：不同 insert order seed → recall RSD
//    2. 尾延迟分布：逐条计时 → P50/P95/P99/P99.9/Max
//
//  不改任何生产代码，不改 bench_cohere1m.rs。
//
//  用法：cargo bench --bench bench_variance --features ablation
// ══════════════════════════════════════════════════════════════

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use std::collections::HashSet;
use std::io::Read;
use std::time::Instant;
use triviumdb::index::quiver::{QuIVer, QuIVerConfig, QuIVerSearchConfig};

// ── 参数 ──
const DIM: usize = 768;
const TOP_K: usize = 10;
const WARMUP: usize = 50;

// 构图方差：不同 seed 的独立构图次数
const BUILD_SEEDS: [u64; 5] = [42, 123, 456, 789, 2024];

// 搜索延迟分布的 ef 值
const EF_TESTS: [usize; 4] = [32, 64, 128, 256];

// ── 文件读取（和 bench_cohere1m 一致） ──

fn read_f32_bin(path: &str) -> Vec<f32> {
    let mut file = std::fs::File::open(path).unwrap_or_else(|_| panic!("无法打开 {}", path));
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() % 4, 0);
    bytes.chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn read_i32_bin(path: &str) -> Vec<i32> {
    let mut file = std::fs::File::open(path).unwrap_or_else(|_| panic!("无法打开 {}", path));
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() % 4, 0);
    bytes.chunks_exact(4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn recall_at_k_ids(gt: &[u64], res: &[(u64, f32)]) -> f64 {
    let gt_set: HashSet<u64> = gt.iter().copied().collect();
    res.iter().filter(|x| gt_set.contains(&x.0)).count() as f64 / gt.len().max(1) as f64
}

/// 用打乱顺序的向量构建 QuIVer 索引
/// seed 控制 shuffle 的随机性
fn build_with_shuffled_order(
    vecs: &[f32], n: usize, dim: usize, config: &QuIVerConfig, seed: u64
) -> QuIVer {
    let mut rng = StdRng::seed_from_u64(seed);

    // 生成打乱的插入顺序
    let mut order: Vec<usize> = (0..n).collect();
    order.shuffle(&mut rng);

    // 按打乱顺序重排向量
    let mut shuffled_vecs = vec![0.0f32; n * dim];
    let mut shuffled_ids = vec![0u64; n];
    let mut shuffled_slots = vec![0usize; n];

    for (new_idx, &orig_idx) in order.iter().enumerate() {
        shuffled_vecs[new_idx * dim..(new_idx + 1) * dim]
            .copy_from_slice(&vecs[orig_idx * dim..(orig_idx + 1) * dim]);
        shuffled_ids[new_idx] = orig_idx as u64;
        // slot 指向原始 train_data 中的位置，rerank 时 ext_vectors[orig_idx * dim..] 才是正确向量
        shuffled_slots[new_idx] = orig_idx;
    }

    let mut index = QuIVer::new(dim, config);
    index.batch_build_experimental_v2(&shuffled_vecs, &shuffled_ids, &shuffled_slots);
    index
}

/// 收集延迟百分位
struct LatencyStats {
    p50: f64,
    p95: f64,
    p99: f64,
    p999: f64,
    max: f64,
    avg: f64,
}

fn compute_latency_stats(latencies: &mut Vec<f64>) -> LatencyStats {
    latencies.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let n = latencies.len();
    LatencyStats {
        p50: latencies[(n as f64 * 0.50) as usize],
        p95: latencies[(n as f64 * 0.95) as usize],
        p99: latencies[(n as f64 * 0.99) as usize],
        p999: latencies[((n as f64 * 0.999) as usize).min(n - 1)],
        max: latencies[n - 1],
        avg: latencies.iter().sum::<f64>() / n as f64,
    }
}

fn main() {
    eprintln!("═══════════════════════════════════════════════════════════════");
    eprintln!("  构图方差 + 尾延迟分布实验 — QuIVer Paper §Revision");
    eprintln!("═══════════════════════════════════════════════════════════════");

    // ── 加载数据（和 bench_cohere1m 一致） ──
    eprintln!("  📂 加载 Cohere-1M 数据集...");
    let train_data = read_f32_bin("cohere_train.f32");
    let test_data = read_f32_bin("cohere_test.f32");
    let gt_data = read_i32_bin("cohere_groundtruth.i32");

    let n_train = train_data.len() / DIM;
    let n_test = test_data.len() / DIM;
    let k_gt = gt_data.len() / n_test;
    eprintln!("  ✅ 训练集: {} × {}d | 测试集: {} × {}d | GT K: {}",
              n_train, DIM, n_test, DIM, k_gt);

    // 解析 Ground Truth → Vec<Vec<u64>>（取 Top-10）
    let eval_gts: Vec<Vec<u64>> = (0..n_test)
        .map(|i| {
            gt_data[i * k_gt..i * k_gt + TOP_K]
                .iter()
                .map(|&id| id as u64)
                .collect()
        })
        .collect();

    // 测试查询
    let queries: Vec<Vec<f32>> = (0..n_test)
        .map(|i| test_data[i * DIM..(i + 1) * DIM].to_vec())
        .collect();

    let config = QuIVerConfig {
        m: 32,
        ef_construction: 128,
        alpha: 1.2,
    };

    // ══════════════════════════════════════════════════════════════
    //  Phase 1: 多次独立构图方差
    //
    //  通过打乱插入顺序模拟不同构图 seed，
    //  测量 recall 在不同图拓扑下的方差
    // ══════════════════════════════════════════════════════════════
    eprintln!("\n┌────────────────────────────────────────────────────────────────────┐");
    eprintln!("│  Phase 1: 多次独立构图方差 ({} 个 seed)                           │", BUILD_SEEDS.len());
    eprintln!("│  数据集: Cohere-1M ({} vecs × {}d){:>35}│", n_train, DIM, "");
    eprintln!("├────────────────────────────────────────────────────────────────────┤");

    // 存储每个 ef × seed 的 recall
    let mut all_recalls: Vec<Vec<f64>> = vec![Vec::new(); EF_TESTS.len()];

    for (si, &seed) in BUILD_SEEDS.iter().enumerate() {
        eprintln!("\n  ── Seed {} ({}/{}) ──", seed, si + 1, BUILD_SEEDS.len());
        let t0 = Instant::now();
        let index = build_with_shuffled_order(&train_data, n_train, DIM, &config, seed);
        let build_s = t0.elapsed().as_secs_f64();
        let stats = index.stats();
        eprintln!("  构建: {:.2}s | 平均度数: {:.1}", build_s, stats.avg_degree_l0);

        // Warmup
        let warmup_cfg = QuIVerSearchConfig { top_k: TOP_K, ef_search: 128, rerank_limit: None };
        for q in &queries[..WARMUP.min(n_test)] {
            let _ = index.search(q, &train_data, &warmup_cfg);
        }

        eprintln!("  {:<8} {:>10} {:>10}", "ef", "Recall@10", "QPS");

        for (ei, &ef) in EF_TESTS.iter().enumerate() {
            let cfg = QuIVerSearchConfig { top_k: TOP_K, ef_search: ef, rerank_limit: None };

            let t0 = Instant::now();
            let mut total_recall = 0.0;
            for (qi, q) in queries.iter().enumerate() {
                let res = index.search(q, &train_data, &cfg);
                total_recall += recall_at_k_ids(&eval_gts[qi], &res);
            }
            let elapsed = t0.elapsed().as_secs_f64();
            let recall = total_recall / n_test as f64 * 100.0;
            let qps = n_test as f64 / elapsed;

            eprintln!("  ef={:<5} {:>8.1}% {:>8.0}", ef, recall, qps);
            all_recalls[ei].push(recall);
        }
    }

    // 汇总方差
    eprintln!("\n  ── 构图方差汇总 ──");
    eprintln!("  {:<8} {:>10} {:>10} {:>10} {:>10}", "ef", "Mean(%)", "Std(%)", "RSD(%)", "Range(%)");

    for (ei, &ef) in EF_TESTS.iter().enumerate() {
        let recalls = &all_recalls[ei];
        let mean = recalls.iter().sum::<f64>() / recalls.len() as f64;
        let variance = recalls.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / recalls.len() as f64;
        let std = variance.sqrt();
        let rsd = std / mean * 100.0;
        let min = recalls.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = recalls.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        eprintln!("  ef={:<5} {:>8.2} {:>8.3} {:>8.2} {:>5.1}–{:.1}",
                  ef, mean, std, rsd, min, max);
    }
    eprintln!("└────────────────────────────────────────────────────────────────────┘");

    // ══════════════════════════════════════════════════════════════
    //  Phase 2: 尾延迟分布
    //
    //  用默认构图（seed=42）的索引，逐条计时，
    //  报告 P50/P95/P99/P99.9/Max
    // ══════════════════════════════════════════════════════════════
    eprintln!("\n┌────────────────────────────────────────────────────────────────────┐");
    eprintln!("│  Phase 2: 尾延迟分布 (单位: μs)                                   │");
    eprintln!("├────────────────────────────────────────────────────────────────────┤");

    // 用第一个 seed 的索引（或重建默认索引）
    eprintln!("  重建默认索引...");
    let index = build_with_shuffled_order(&train_data, n_train, DIM, &config, BUILD_SEEDS[0]);

    // Warmup
    let warmup_cfg = QuIVerSearchConfig { top_k: TOP_K, ef_search: 256, rerank_limit: None };
    for q in &queries[..WARMUP.min(n_test)] {
        let _ = index.search(q, &train_data, &warmup_cfg);
    }

    eprintln!("  {:<8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
              "ef", "Avg", "P50", "P95", "P99", "P99.9", "Max");

    for &ef in &EF_TESTS {
        let cfg = QuIVerSearchConfig { top_k: TOP_K, ef_search: ef, rerank_limit: None };

        // 跑 3 轮取所有延迟
        let rounds = 3;
        let mut all_lats: Vec<f64> = Vec::with_capacity(n_test * rounds);

        for _ in 0..rounds {
            for q in &queries {
                let t0 = Instant::now();
                let res = index.search(q, &train_data, &cfg);
                std::hint::black_box(&res);
                let lat_us = t0.elapsed().as_secs_f64() * 1e6;
                all_lats.push(lat_us);
            }
        }

        let stats = compute_latency_stats(&mut all_lats);
        eprintln!("  ef={:<5} {:>6.0} {:>6.0} {:>6.0} {:>6.0} {:>6.0} {:>6.0}",
                  ef, stats.avg, stats.p50, stats.p95, stats.p99, stats.p999, stats.max);
    }
    eprintln!("└────────────────────────────────────────────────────────────────────┘");

    // ══════════════════════════════════════════════════════════════
    //  Phase 3: 并发客户端 Throughput-Latency Curve
    //
    //  模拟 1/2/4/8/16 并发客户端，每个客户端串行发 query，
    //  报告总 QPS 和 per-query P99 延迟
    // ══════════════════════════════════════════════════════════════
    eprintln!("\n┌────────────────────────────────────────────────────────────────────┐");
    eprintln!("│  Phase 3: 并发客户端 Throughput-Latency Curve                     │");
    eprintln!("├────────────────────────────────────────────────────────────────────┤");

    let concurrency_levels = [1, 2, 4, 8, 16];
    let test_ef = 128; // 固定 ef=128 来观察并发效果
    let queries_per_client = 200;

    let index_arc = std::sync::Arc::new(index);
    let vecs_arc = std::sync::Arc::new(train_data);

    eprintln!("  ef={}, 每客户端 {} 查询", test_ef, queries_per_client);
    eprintln!("  {:<10} {:>10} {:>10} {:>10} {:>10} {:>10}",
              "Clients", "Total QPS", "Avg(μs)", "P50(μs)", "P99(μs)", "Max(μs)");

    for &n_clients in &concurrency_levels {
        let cfg = QuIVerSearchConfig { top_k: TOP_K, ef_search: test_ef, rerank_limit: None };

        // 预分配每个客户端的查询（循环使用查询集）
        let client_queries: Vec<Vec<Vec<f32>>> = (0..n_clients)
            .map(|c| {
                (0..queries_per_client)
                    .map(|i| queries[(c * queries_per_client + i) % n_test].clone())
                    .collect()
            })
            .collect();

        // Warmup
        for q in &queries[..WARMUP.min(n_test)] {
            let _ = index_arc.search(q, &vecs_arc, &cfg);
        }

        // 并发执行
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(n_clients));
        let mut handles = Vec::with_capacity(n_clients);

        let wall_start = Instant::now();

        for client_id in 0..n_clients {
            let idx = index_arc.clone();
            let vs = vecs_arc.clone();
            let qs = client_queries[client_id].clone();
            let bar = barrier.clone();
            let cfg_clone = QuIVerSearchConfig { top_k: TOP_K, ef_search: test_ef, rerank_limit: None };

            handles.push(std::thread::spawn(move || {
                let mut lats = Vec::with_capacity(queries_per_client);
                // 所有线程同时开始
                bar.wait();
                for q in &qs {
                    let t0 = Instant::now();
                    let res = idx.search(q, &vs, &cfg_clone);
                    std::hint::black_box(&res);
                    lats.push(t0.elapsed().as_secs_f64() * 1e6);
                }
                lats
            }));
        }

        // 收集所有延迟
        let mut all_lats: Vec<f64> = Vec::new();
        for h in handles {
            all_lats.extend(h.join().unwrap());
        }

        let wall_time = wall_start.elapsed().as_secs_f64();
        let total_queries = n_clients * queries_per_client;
        let total_qps = total_queries as f64 / wall_time;

        let stats = compute_latency_stats(&mut all_lats);

        eprintln!("  {:<10} {:>8.0} {:>8.0} {:>8.0} {:>8.0} {:>8.0}",
                  n_clients, total_qps, stats.avg, stats.p50, stats.p99, stats.max);
    }

    eprintln!("└────────────────────────────────────────────────────────────────────┘");

    eprintln!("\n═══════════════════════════════════════════════════════════════");
    eprintln!("  ✅ 构图方差 + 尾延迟 + 并发 curve 实验完成");
    eprintln!("═══════════════════════════════════════════════════════════════");
}
