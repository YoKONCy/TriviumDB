use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::time::Instant;
use triviumdb::index::quiver::{QuIVer, QuIVerConfig, QuIVerSearchConfig};

// ══════════════════════════════════════════════════════════════
//  BQ-HNSW 消融实验 — 正式 Benchmark
//
//  修复项：
//    1. 多轮测量（ROUNDS=5），报告 mean ± std
//    2. Warmup（搜索前预热 20 个 query，丢弃结果）
//    3. Query 数量提升到 200
//    4. BruteForce 基线同样多轮测量
//    5. 单次延迟统计：p50 / p95 / p99
// ══════════════════════════════════════════════════════════════

const DIM: usize = 768;
const N: usize = 20000;
const CLUSTERS: usize = 100;
const NOISE: f32 = 0.15;
const NQ: usize = 200;       // 查询数
const TOP_K: usize = 10;
const WARMUP: usize = 20;    // 预热查询数
const ROUNDS: usize = 5;     // 每配置重复测量轮数

fn gauss(rng: &mut StdRng) -> f32 {
    let u1 = rng.gen_range(1e-10f32..1.0);
    let u2 = rng.gen_range(0.0f32..1.0);
    (-2.0f32 * u1.ln()).sqrt() * (2.0f32 * std::f32::consts::PI * u2).cos()
}

fn gen_unit(rng: &mut StdRng, d: usize) -> Vec<f32> {
    let v: Vec<f32> = (0..d).map(|_| gauss(rng)).collect();
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter().map(|x| x / n).collect()
}

fn gen_clustered(c: usize, p: usize, d: usize, noise: f32, rng: &mut StdRng) -> (Vec<f32>, Vec<u64>) {
    let mut vecs = Vec::with_capacity(c * p * d);
    let mut ids = Vec::with_capacity(c * p);
    for ci in 0..c {
        let ctr = gen_unit(rng, d);
        for pi in 0..p {
            let mut v: Vec<f32> = ctr.iter().map(|&x| x + gauss(rng) * noise).collect();
            let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            for x in &mut v { *x /= n; }
            vecs.extend_from_slice(&v);
            ids.push((ci * p + pi) as u64);
        }
    }
    (vecs, ids)
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let ab: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    ab / (na * nb).max(1e-30)
}

fn brute_force(vecs: &[f32], dim: usize, q: &[f32], k: usize) -> Vec<(u64, f32)> {
    let n = vecs.len() / dim;
    let mut s: Vec<(u64, f32)> = (0..n)
        .map(|i| (i as u64, cosine(q, &vecs[i * dim..(i + 1) * dim])))
        .collect();
    s.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    s.truncate(k);
    s
}

fn recall(gt: &[(u64, f32)], res: &[(u64, f32)]) -> f64 {
    let s: HashSet<u64> = gt.iter().map(|x| x.0).collect();
    res.iter().filter(|x| s.contains(&x.0)).count() as f64 / gt.len().max(1) as f64
}

/// 百分位数（线性插值）
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() { return 0.0; }
    let idx = p / 100.0 * (sorted.len() - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    if lo == hi { return sorted[lo]; }
    sorted[lo] * (hi as f64 - idx) + sorted[hi] * (idx - lo as f64)
}

/// 统计摘要
struct Stats {
    mean: f64,
    std: f64,
    p50: f64,
    p95: f64,
    p99: f64,
}

fn compute_stats(values: &[f64]) -> Stats {
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    let std = var.sqrt();
    let mut sorted = values.to_vec();
    sorted.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    Stats {
        mean,
        std,
        p50: percentile(&sorted, 50.0),
        p95: percentile(&sorted, 95.0),
        p99: percentile(&sorted, 99.0),
    }
}

fn main() {
    let per = N / CLUSTERS;

    eprintln!("═══════════════════════════════════════════════════════════════");
    eprintln!("  BQ-HNSW Ablation Benchmark  dim={DIM}  N={N}  Q={NQ}  rounds={ROUNDS}");
    eprintln!("═══════════════════════════════════════════════════════════════");

    // ── 数据生成 ──
    let mut rng = StdRng::seed_from_u64(42);
    let (vecs, ids) = gen_clustered(CLUSTERS, per, DIM, NOISE, &mut rng);

    // 查询生成（200 个）
    let queries: Vec<Vec<f32>> = (0..NQ)
        .map(|_| {
            let idx = rng.gen_range(0..N);
            let base = &vecs[idx * DIM..(idx + 1) * DIM];
            let mut q: Vec<f32> = base.iter().map(|&x| x + gauss(&mut rng) * 0.05).collect();
            let norm = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            for x in &mut q { *x /= norm; }
            q
        })
        .collect();

    // 额外 warmup 查询集（独立于正式查询）
    let warmup_queries: Vec<Vec<f32>> = (0..WARMUP)
        .map(|_| {
            let idx = rng.gen_range(0..N);
            let base = &vecs[idx * DIM..(idx + 1) * DIM];
            let mut q: Vec<f32> = base.iter().map(|&x| x + gauss(&mut rng) * 0.05).collect();
            let norm = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            for x in &mut q { *x /= norm; }
            q
        })
        .collect();

    // ── BruteForce 基线（多轮测量）──
    // 预热
    for q in &warmup_queries {
        let _ = brute_force(&vecs, DIM, q, TOP_K);
    }

    let gts: Vec<_> = queries.iter().map(|q| brute_force(&vecs, DIM, q, TOP_K)).collect();

    let mut bf_qps_samples = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let t0 = Instant::now();
        for q in &queries {
            let _ = brute_force(&vecs, DIM, q, TOP_K);
        }
        bf_qps_samples.push(NQ as f64 / t0.elapsed().as_secs_f64());
    }
    let bf_stats = compute_stats(&bf_qps_samples);
    eprintln!(
        "  BruteForce QPS: {:.1} ± {:.1} ({}轮)",
        bf_stats.mean, bf_stats.std, ROUNDS
    );

    // ── 实验配置 ──
    let ef_tests = [64, 128, 256, 512, 1024];

    struct Exp {
        label: &'static str,
        m: usize,
        ef_c: usize,
        alpha: f32,
    }
    let experiments = [
        // α=1.2 是基准
        Exp { label: "1. m=32 (ef_c=256, α=1.2)", m: 32, ef_c: 256, alpha: 1.2 },
        Exp { label: "2. m=48 (ef_c=256, α=1.2)", m: 48, ef_c: 256, alpha: 1.2 },
        Exp { label: "3. m=64 (ef_c=512, α=1.2)", m: 64, ef_c: 512, alpha: 1.2 },
    ];

    for exp in experiments.iter() {
        eprintln!("\n  ── {} ──", exp.label);
        let config = QuIVerConfig {
            m: exp.m,
            ef_construction: exp.ef_c,
            alpha: exp.alpha,
        };
        let mut index = QuIVer::new(DIM, &config);

        // 建图（v2 并发构图：批量预安装 + 大粒度并行连边）
        let t0 = Instant::now();
        let slot_idxs: Vec<usize> = (0..ids.len()).collect();
        index.batch_build_experimental_v2(&vecs, &ids, &slot_idxs);
        let build_s = t0.elapsed().as_secs_f64();
        let build_vps = ids.len() as f64 / build_s;

        let stats = index.stats();
        eprintln!(
            "  构建耗时: {:.2}s ({:.0} vecs/s) | 平均度数: {:.1}",
            build_s, build_vps, stats.avg_degree_l0
        );

        // 表头
        eprintln!(
            "  {:<8} {:>10} {:>10} {:>8} {:>10} {:>10}",
            "ef", "Recall", "QPS", "加速比", "p50(μs)", "p95(μs)"
        );

        for &ef in &ef_tests {
            let cfg = QuIVerSearchConfig { top_k: TOP_K, ef_search: ef };

            // Warmup（丢弃结果，预热 cache）
            for q in &warmup_queries {
                let _ = index.search(q, &vecs, &cfg);
            }

            // 多轮正式测量
            let mut round_recalls = Vec::with_capacity(ROUNDS);
            let mut round_qps = Vec::with_capacity(ROUNDS);
            let mut all_latencies_us: Vec<f64> = Vec::with_capacity(ROUNDS * NQ);

            for _ in 0..ROUNDS {
                let mut round_recall_sum = 0.0;
                let mut latencies = Vec::with_capacity(NQ);

                let round_start = Instant::now();
                for (qi, q) in queries.iter().enumerate() {
                    let q_start = Instant::now();
                    let res = index.search(q, &vecs, &cfg);
                    let elapsed_us = q_start.elapsed().as_secs_f64() * 1_000_000.0;
                    latencies.push(elapsed_us);
                    round_recall_sum += recall(&gts[qi], &res);
                }
                let round_elapsed = round_start.elapsed().as_secs_f64();

                round_recalls.push(round_recall_sum / NQ as f64 * 100.0);
                round_qps.push(NQ as f64 / round_elapsed);
                all_latencies_us.extend_from_slice(&latencies);
            }

            let recall_stats = compute_stats(&round_recalls);
            let qps_stats = compute_stats(&round_qps);
            all_latencies_us.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
            let lat_p50 = percentile(&all_latencies_us, 50.0);
            let lat_p95 = percentile(&all_latencies_us, 95.0);

            eprintln!(
                "  ef={:<5} {:>5.1}±{:<4.1}% {:>6.0}±{:<4.0} {:>6.1}x {:>8.0} {:>8.0}",
                ef,
                recall_stats.mean,
                recall_stats.std,
                qps_stats.mean,
                qps_stats.std,
                qps_stats.mean / bf_stats.mean,
                lat_p50,
                lat_p95,
            );
        }
    }
}
