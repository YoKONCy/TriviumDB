// ══════════════════════════════════════════════════════════════
//  Encoding Ablation Benchmark
//
//  对比三种 BQ 编码方案：
//    1. 1-bit sign-only (SimHash baseline) — 位运算
//    2. 2-bit Sign-Magnitude (QuIVer 默认, τ=mean|x|) — 位运算
//    3. 2-bit Scalar Quantization (4 均匀桶, L1 距离) — 整数运算
//
//  Phase 1: Top-K Overlap (三种编码的距离排序 vs 真实余弦)
//           + 单次距离计算延迟对比
//  Phase 2: QuIVer 图搜索 Recall@10 (仅 1-bit 和 2-bit SM,
//           因为 scalar 不兼容 QuIVer 的位运算管线)
//
//  用法：cargo bench --bench bench_encoding_ablation
// ══════════════════════════════════════════════════════════════

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::time::Instant;
use triviumdb::index::bq::{Bq2Signature, Bq2Store};
use triviumdb::index::quiver::{QuIVer, QuIVerConfig, QuIVerSearchConfig};

// ── 实验用辅助函数（不污染项目主体） ──

/// 1-bit sign-only 编码：strong 全零，距离退化为纯 sign Hamming
fn bq2_sign_only(vec: &[f32]) -> Bq2Signature {
    let max_chunks = Bq2Signature::MAX_CHUNKS;
    let mut sig = Bq2Signature {
        pos: [0u64; 48],    // MAX_CHUNKS = 48
        strong: [0u64; 48], // 全零 → 纯 1-bit
    };
    let chunks = vec.len().div_ceil(64).min(max_chunks);
    for i in 0..chunks {
        let mut chunk_pos = 0u64;
        for j in 0..64 {
            let idx = i * 64 + j;
            if idx < vec.len() && vec[idx] > 0.0 {
                chunk_pos |= 1u64 << j;
            }
        }
        sig.pos[i] = chunk_pos;
    }
    sig
}

// ── 参数 ──
const DIM: usize = 768;
const TOP_K: usize = 10;
const NQ: usize = 500;
const WARMUP: usize = 50;
const ROUNDS: usize = 3;
const EF_TESTS: [usize; 5] = [32, 64, 128, 256, 512];

// 合成数据参数
const SYNTH_N: usize = 50_000;
const SYNTH_CLUSTERS: usize = 200;
const SYNTH_NOISE: f32 = 0.12;

// ══════════════════════════════════════════════════════════════
//  真正的 2-bit Scalar Quantizer
//
//  每个维度映射到 4 个桶：
//    0 = strong negative (x ≤ -τ)
//    1 = weak negative   (-τ < x ≤ 0)
//    2 = weak positive   (0 < x ≤ τ)
//    3 = strong positive (x > τ)
//  其中 τ = mean(|x|)（和 SM 一致，保证公平对比）
//
//  距离 = Σ|bucket_a[i] - bucket_b[i]|  (L1, 整数运算)
// ══════════════════════════════════════════════════════════════

struct ScalarSig {
    buckets: Vec<u8>,
}

impl ScalarSig {
    fn from_vector(vec: &[f32]) -> Self {
        let sum_abs: f32 = vec.iter().map(|x| x.abs()).sum();
        let tau = if vec.is_empty() {
            0.0
        } else {
            sum_abs / vec.len() as f32
        };

        let buckets: Vec<u8> = vec
            .iter()
            .map(|&x| {
                if x <= -tau {
                    0 // 强负
                } else if x <= 0.0 {
                    1 // 弱负
                } else if x <= tau {
                    2 // 弱正
                } else {
                    3 // 强正
                }
            })
            .collect();

        Self { buckets }
    }

    /// L1 距离（逐维度整数差值求和）
    fn distance(&self, other: &ScalarSig) -> u32 {
        self.buckets
            .iter()
            .zip(other.buckets.iter())
            .map(|(&a, &b)| (a as i16 - b as i16).unsigned_abs() as u32)
            .sum()
    }
}

// ── 工具函数 ──

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

fn gen_clustered(c: usize, per: usize, d: usize, noise: f32, rng: &mut StdRng) -> Vec<f32> {
    let mut vecs = Vec::with_capacity(c * per * d);
    for _ in 0..c {
        let ctr = gen_unit(rng, d);
        for _ in 0..per {
            let mut v: Vec<f32> = ctr.iter().map(|&x| x + gauss(rng) * noise).collect();
            let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            for x in &mut v {
                *x /= n;
            }
            vecs.extend_from_slice(&v);
        }
    }
    vecs
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let ab: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    ab / (na * nb).max(1e-30)
}

fn brute_force_topk(vecs: &[f32], dim: usize, q: &[f32], k: usize) -> Vec<(usize, f32)> {
    let n = vecs.len() / dim;
    let mut s: Vec<(usize, f32)> = (0..n)
        .map(|i| (i, cosine(q, &vecs[i * dim..(i + 1) * dim])))
        .collect();
    s.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    s.truncate(k);
    s
}

fn recall_at_k(gt: &[(usize, f32)], res: &[(u64, f32)]) -> f64 {
    let gt_set: HashSet<u64> = gt.iter().map(|x| x.0 as u64).collect();
    res.iter().filter(|x| gt_set.contains(&x.0)).count() as f64 / gt.len().max(1) as f64
}

fn load_or_generate_data() -> (Vec<f32>, usize, String) {
    let cohere_path = "cohere_train.f32";
    if std::path::Path::new(cohere_path).exists() {
        eprintln!("  📂 加载 Cohere-1M 数据集...");
        let data = std::fs::read(cohere_path).unwrap();
        let n_floats = data.len() / 4;
        let n_vecs = n_floats / DIM;
        let use_n = n_vecs.min(100_000);
        let bytes = &data[..use_n * DIM * 4];
        let vecs: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect();
        eprintln!("  ✅ 已加载 {} 个向量 (dim={})", use_n, DIM);
        (vecs, use_n, format!("Cohere-{}K", use_n / 1000))
    } else {
        eprintln!("  ⚠️ 未找到 cohere_train.f32, 使用合成聚类数据");
        let mut rng = StdRng::seed_from_u64(42);
        let per = SYNTH_N / SYNTH_CLUSTERS;
        let vecs = gen_clustered(SYNTH_CLUSTERS, per, DIM, SYNTH_NOISE, &mut rng);
        (vecs, SYNTH_N, format!("Synthetic-{}K", SYNTH_N / 1000))
    }
}

fn main() {
    eprintln!("═══════════════════════════════════════════════════════════════");
    eprintln!("  Encoding Ablation Benchmark — QuIVer Paper §Revision");
    eprintln!("═══════════════════════════════════════════════════════════════");

    let (vecs, n, dataset_name) = load_or_generate_data();

    // ── 生成查询集 ──
    let mut rng = StdRng::seed_from_u64(2024);
    let queries: Vec<Vec<f32>> = (0..NQ)
        .map(|_| {
            let idx = rng.gen_range(0..n);
            let base = &vecs[idx * DIM..(idx + 1) * DIM];
            let mut q: Vec<f32> = base.iter().map(|&x| x + gauss(&mut rng) * 0.03).collect();
            let norm = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            for x in &mut q {
                *x /= norm;
            }
            q
        })
        .collect();
    let warmup_queries: Vec<Vec<f32>> = (0..WARMUP)
        .map(|_| {
            let idx = rng.gen_range(0..n);
            let base = &vecs[idx * DIM..(idx + 1) * DIM];
            let mut q: Vec<f32> = base.iter().map(|&x| x + gauss(&mut rng) * 0.03).collect();
            let norm = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            for x in &mut q {
                *x /= norm;
            }
            q
        })
        .collect();

    // ── 计算 Ground Truth ──
    eprintln!("  📐 计算 brute-force ground truth ({}个查询)...", NQ);
    let gts: Vec<Vec<(usize, f32)>> = queries
        .iter()
        .map(|q| brute_force_topk(&vecs, DIM, q, TOP_K))
        .collect();

    // ═══════════════════════════════════════════════════════
    //  Phase 1: Top-K Overlap + 距离计算延迟对比
    // ═══════════════════════════════════════════════════════
    eprintln!("\n┌──────────────────────────────────────────────────────────────────────┐");
    eprintln!(
        "│  Phase 1: Top-{} Overlap + 距离计算延迟 (BQ 排序 vs 真实余弦排序) │",
        TOP_K
    );
    eprintln!("│  数据集: {:<56}│", dataset_name);
    eprintln!("├──────────────────────────────────────────────────────────────────────┤");

    let overlap_queries = &queries[..100.min(NQ)];

    // ── 1-bit sign-only ──
    {
        let sigs: Vec<Bq2Signature> = (0..n)
            .map(|i| bq2_sign_only(&vecs[i * DIM..(i + 1) * DIM]))
            .collect();

        // Top-K Overlap
        let mut total_overlap = 0.0;
        for q in overlap_queries {
            let gt = brute_force_topk(&vecs, DIM, q, TOP_K);
            let gt_set: HashSet<usize> = gt.iter().map(|x| x.0).collect();
            let q_sig = bq2_sign_only(q);
            let mut bq_dists: Vec<(usize, u32)> = sigs
                .iter()
                .enumerate()
                .map(|(i, s)| (i, s.distance(&q_sig, DIM)))
                .collect();
            bq_dists.sort_unstable_by_key(|x| x.1);
            let bq_topk: HashSet<usize> = bq_dists.iter().take(TOP_K).map(|x| x.0).collect();
            total_overlap += gt_set.intersection(&bq_topk).count() as f64 / TOP_K as f64;
        }
        let overlap = total_overlap / overlap_queries.len() as f64;

        // 距离计算延迟
        let q_sig = bq2_sign_only(&queries[0]);
        let iters = 100_000usize;
        let t0 = Instant::now();
        let mut dummy = 0u32;
        for i in 0..iters {
            dummy = dummy.wrapping_add(sigs[i % n].distance(&q_sig, DIM));
        }
        let ns_per = t0.elapsed().as_nanos() as f64 / iters as f64;
        std::hint::black_box(dummy);

        eprintln!(
            "│  1-bit sign (位运算)     overlap: {:5.1}%  dist: {:5.1} ns/call │",
            overlap * 100.0,
            ns_per
        );
    }

    // ── 2-bit SM (位运算) ──
    {
        let sigs: Vec<Bq2Signature> = (0..n)
            .map(|i| Bq2Signature::from_vector(&vecs[i * DIM..(i + 1) * DIM]))
            .collect();

        let mut total_overlap = 0.0;
        for q in overlap_queries {
            let gt = brute_force_topk(&vecs, DIM, q, TOP_K);
            let gt_set: HashSet<usize> = gt.iter().map(|x| x.0).collect();
            let q_sig = Bq2Signature::from_vector(q);
            let mut bq_dists: Vec<(usize, u32)> = sigs
                .iter()
                .enumerate()
                .map(|(i, s)| (i, s.distance(&q_sig, DIM)))
                .collect();
            bq_dists.sort_unstable_by_key(|x| x.1);
            let bq_topk: HashSet<usize> = bq_dists.iter().take(TOP_K).map(|x| x.0).collect();
            total_overlap += gt_set.intersection(&bq_topk).count() as f64 / TOP_K as f64;
        }
        let overlap = total_overlap / overlap_queries.len() as f64;

        let q_sig = Bq2Signature::from_vector(&queries[0]);
        let iters = 100_000usize;
        let t0 = Instant::now();
        let mut dummy = 0u32;
        for i in 0..iters {
            dummy = dummy.wrapping_add(sigs[i % n].distance(&q_sig, DIM));
        }
        let ns_per = t0.elapsed().as_nanos() as f64 / iters as f64;
        std::hint::black_box(dummy);

        eprintln!(
            "│  2-bit SM  (位运算)     overlap: {:5.1}%  dist: {:5.1} ns/call │",
            overlap * 100.0,
            ns_per
        );
    }

    // ── 2-bit Scalar (整数运算) ──
    {
        let sigs: Vec<ScalarSig> = (0..n)
            .map(|i| ScalarSig::from_vector(&vecs[i * DIM..(i + 1) * DIM]))
            .collect();

        let mut total_overlap = 0.0;
        for q in overlap_queries {
            let gt = brute_force_topk(&vecs, DIM, q, TOP_K);
            let gt_set: HashSet<usize> = gt.iter().map(|x| x.0).collect();
            let q_sig = ScalarSig::from_vector(q);
            let mut sq_dists: Vec<(usize, u32)> = sigs
                .iter()
                .enumerate()
                .map(|(i, s)| (i, s.distance(&q_sig)))
                .collect();
            sq_dists.sort_unstable_by_key(|x| x.1);
            let sq_topk: HashSet<usize> = sq_dists.iter().take(TOP_K).map(|x| x.0).collect();
            total_overlap += gt_set.intersection(&sq_topk).count() as f64 / TOP_K as f64;
        }
        let overlap = total_overlap / overlap_queries.len() as f64;

        let q_sig = ScalarSig::from_vector(&queries[0]);
        let iters = 100_000usize;
        let t0 = Instant::now();
        let mut dummy = 0u32;
        for i in 0..iters {
            dummy = dummy.wrapping_add(sigs[i % n].distance(&q_sig));
        }
        let ns_per = t0.elapsed().as_nanos() as f64 / iters as f64;
        std::hint::black_box(dummy);

        eprintln!(
            "│  2-bit SQ  (整数运算)   overlap: {:5.1}%  dist: {:5.1} ns/call │",
            overlap * 100.0,
            ns_per
        );
    }

    eprintln!("└──────────────────────────────────────────────────────────────────────┘");

    // ═══════════════════════════════════════════════════════
    //  Phase 2: QuIVer 图搜索 Recall@10 — 三种编码全对比
    //
    //  1-bit / 2-bit SM: 用各自编码构图 + QuIVer 原生搜索
    //  2-bit Scalar: 复用 2-bit SM 图拓扑 + 独立 beam search
    //                (ScalarSig L1 距离导航 + f32 精排)
    //  控制变量：图拓扑相同（2-bit SM），仅搜索距离函数不同
    // ═══════════════════════════════════════════════════════
    eprintln!("\n┌──────────────────────────────────────────────────────────────────────┐");
    eprintln!(
        "│  Phase 2: Recall@{} — 三种编码的图搜索对比                        │",
        TOP_K
    );
    eprintln!("├──────────────────────────────────────────────────────────────────────┤");

    let config = QuIVerConfig {
        m: 32,
        ef_construction: 128,
        alpha: 1.2,
    };

    // ── 2a: 1-bit sign-only (位运算, 自编码构图) ──
    {
        eprintln!("\n  ── 1-bit sign-only (位运算) ──");
        let mut store = Bq2Store::new(DIM);
        store.reserve(n);
        for i in 0..n {
            store.push_sig(&bq2_sign_only(&vecs[i * DIM..(i + 1) * DIM]));
        }
        let mut index = QuIVer::new(DIM, &config);
        let ids: Vec<u64> = (0..n as u64).collect();
        let slots: Vec<usize> = (0..n).collect();
        let t0 = Instant::now();
        index.batch_build_with_store(&vecs, &ids, &slots, store);
        let build_s = t0.elapsed().as_secs_f64();
        let stats = index.stats();
        eprintln!(
            "  构建: {:.2}s | 平均度数: {:.1}",
            build_s, stats.avg_degree_l0
        );

        run_recall_bench(&index, &vecs, &queries, &warmup_queries, &gts);
    }

    // ── 2b: 2-bit SM (位运算, 默认编码构图) ──
    let sm_index;
    {
        eprintln!("\n  ── 2-bit SM (位运算, 默认) ──");
        let mut index = QuIVer::new(DIM, &config);
        let ids: Vec<u64> = (0..n as u64).collect();
        let slots: Vec<usize> = (0..n).collect();
        let t0 = Instant::now();
        index.batch_build_experimental_v2(&vecs, &ids, &slots);
        let build_s = t0.elapsed().as_secs_f64();
        let stats = index.stats();
        eprintln!(
            "  构建: {:.2}s | 平均度数: {:.1}",
            build_s, stats.avg_degree_l0
        );

        run_recall_bench(&index, &vecs, &queries, &warmup_queries, &gts);
        sm_index = index;
    }

    // ── 2c: 2-bit Scalar (整数运算, 复用 SM 图拓扑) ──
    {
        eprintln!("\n  ── 2-bit Scalar (整数运算, 复用 SM 图拓扑) ──");
        eprintln!("  (图拓扑来自 2-bit SM 构图, 仅导航距离换为 Scalar L1)");

        // 预编码所有向量为 ScalarSig
        let scalar_sigs: Vec<ScalarSig> = (0..n)
            .map(|i| ScalarSig::from_vector(&vecs[i * DIM..(i + 1) * DIM]))
            .collect();

        // 表头
        eprintln!("  {:<8} {:>10} {:>10}", "ef", "Recall@10", "QPS");

        for &ef in &EF_TESTS {
            let mut total_recall = 0.0;
            let mut total_qps_samples = Vec::with_capacity(ROUNDS);

            for _ in 0..ROUNDS {
                let mut round_recall = 0.0;
                let t0 = Instant::now();
                for (qi, q) in queries.iter().enumerate() {
                    let res = scalar_beam_search(&sm_index, &scalar_sigs, q, &vecs, DIM, ef, TOP_K);
                    round_recall += recall_at_k(&gts[qi], &res);
                }
                let elapsed = t0.elapsed().as_secs_f64();
                total_recall += round_recall / NQ as f64;
                total_qps_samples.push(NQ as f64 / elapsed);
            }

            let avg_recall = total_recall / ROUNDS as f64 * 100.0;
            let avg_qps: f64 = total_qps_samples.iter().sum::<f64>() / ROUNDS as f64;
            eprintln!("  ef={:<5} {:>8.1}% {:>8.0}", ef, avg_recall, avg_qps);
        }
    }

    eprintln!("\n═══════════════════════════════════════════════════════════════");
    eprintln!("  ✅ Encoding ablation 完成");
    eprintln!("═══════════════════════════════════════════════════════════════");
}

/// QuIVer 原生搜索的 Recall 测试（用于 1-bit 和 2-bit SM）
fn run_recall_bench(
    index: &QuIVer,
    vecs: &[f32],
    queries: &[Vec<f32>],
    warmup_queries: &[Vec<f32>],
    gts: &[Vec<(usize, f32)>],
) {
    let search_cfg = QuIVerSearchConfig {
        top_k: TOP_K,
        ef_search: 128,
        rerank_limit: None,
    };
    for q in warmup_queries {
        let _ = index.search_flat(q, vecs, &search_cfg);
    }

    eprintln!("  {:<8} {:>10} {:>10}", "ef", "Recall@10", "QPS");

    for &ef in &EF_TESTS {
        let cfg = QuIVerSearchConfig {
            top_k: TOP_K,
            ef_search: ef,
            rerank_limit: None,
        };
        let mut total_recall = 0.0;
        let mut total_qps_samples = Vec::with_capacity(ROUNDS);

        for _ in 0..ROUNDS {
            let mut round_recall = 0.0;
            let t0 = Instant::now();
            for (qi, q) in queries.iter().enumerate() {
                let res = index.search_flat(q, vecs, &cfg);
                round_recall += recall_at_k(&gts[qi], &res);
            }
            let elapsed = t0.elapsed().as_secs_f64();
            total_recall += round_recall / queries.len() as f64;
            total_qps_samples.push(queries.len() as f64 / elapsed);
        }

        let avg_recall = total_recall / ROUNDS as f64 * 100.0;
        let avg_qps: f64 = total_qps_samples.iter().sum::<f64>() / ROUNDS as f64;
        eprintln!("  ef={:<5} {:>8.1}% {:>8.0}", ef, avg_recall, avg_qps);
    }
}

/// 独立的 beam search：用 ScalarSig L1 距离导航 + f32 精排
///
/// 复用 QuIVer 的图拓扑（邻接表），仅替换导航距离函数。
/// 这样与 QuIVer 原生搜索的唯一区别就是距离计算方式。
fn scalar_beam_search(
    index: &QuIVer,
    sigs: &[ScalarSig],
    query: &[f32],
    ext_vectors: &[f32],
    dim: usize,
    ef: usize,
    top_k: usize,
) -> Vec<(u64, f32)> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let n = sigs.len();
    let q_sig = ScalarSig::from_vector(query);
    let entry = index.ablation_entry_point();

    // visited bitset (简化版：用 Vec<bool>)
    let mut visited = vec![false; n];

    let mut candidates: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::with_capacity(ef * 2);
    let mut results: BinaryHeap<(u32, u32)> = BinaryHeap::with_capacity(ef + 1);

    let d = q_sig.distance(&sigs[entry as usize]);
    visited[entry as usize] = true;
    candidates.push(Reverse((d, entry)));
    results.push((d, entry));

    while let Some(Reverse((cd, cur))) = candidates.pop() {
        if results.len() >= ef && cd > results.peek().unwrap().0 {
            break;
        }

        let nbs = index.layer0_neighbors(cur);
        for &nb in nbs {
            if visited[nb as usize] {
                continue;
            }
            visited[nb as usize] = true;

            let nd = q_sig.distance(&sigs[nb as usize]);
            if results.len() < ef || nd < results.peek().unwrap().0 {
                candidates.push(Reverse((nd, nb)));
                results.push((nd, nb));
                if results.len() > ef {
                    results.pop();
                }
            }
        }
    }

    // f32 精排（与 QuIVer::search 相同）
    let mut reranked: Vec<(u64, f32)> = results
        .into_vec()
        .iter()
        .map(|&(_, node)| {
            let idx = node as usize;
            let v = &ext_vectors[idx * dim..(idx + 1) * dim];
            let sim = cosine(query, v);
            (node as u64, sim)
        })
        .collect();
    reranked.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    reranked.truncate(top_k);
    reranked
}
