use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::time::Instant;
use triviumdb::index::gru_router::{
    GruGradients, GruRouterIndex, GruSearchConfig, MinGru,
};

const DIM: usize = 768;
const H_DIM: usize = 16;  // 增大隐藏维度
const C_DIM: usize = 16;
const NUM_CHUNKS: usize = DIM / C_DIM;

fn gauss(rng: &mut StdRng) -> f32 {
    let u1 = rng.gen_range(1e-10f32..1.0);
    let u2 = rng.gen_range(0.0f32..1.0);
    (-2.0f32 * u1.ln()).sqrt() * (2.0f32 * std::f32::consts::PI * u2).cos()
}

fn gen_unit(rng: &mut StdRng, dim: usize) -> Vec<f32> {
    let v: Vec<f32> = (0..dim).map(|_| gauss(rng)).collect();
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter().map(|x| x / n).collect()
}

fn gen_clustered(
    clusters: usize, per: usize, dim: usize, noise: f32, rng: &mut StdRng,
) -> (Vec<f32>, Vec<usize>) {
    // 返回 (向量, 每个向量的簇 ID)
    let mut vecs = Vec::with_capacity(clusters * per * dim);
    let mut labels = Vec::with_capacity(clusters * per);
    for c in 0..clusters {
        let center = gen_unit(rng, dim);
        for _ in 0..per {
            let mut v: Vec<f32> = center.iter().map(|&x| x + gauss(rng) * noise).collect();
            let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            for x in &mut v { *x /= n; }
            vecs.extend_from_slice(&v);
            labels.push(c);
        }
    }
    (vecs, labels)
}

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let ab: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    ab / (na * nb).max(1e-30)
}

fn init_gru(rng: &mut StdRng) -> MinGru {
    let cd = H_DIM + C_DIM;
    // Xavier init，但 w_r 用更大的值让 route 输出偏离 0.5
    let sz = (2.0 / cd as f32).sqrt();
    let sh = (2.0 / C_DIM as f32).sqrt();
    let sr = 1.0; // 放大路由权重
    MinGru {
        h_dim: H_DIM, c_dim: C_DIM, num_chunks: NUM_CHUNKS,
        w_z: (0..H_DIM * cd).map(|_| gauss(rng) * sz).collect(),
        w_h: (0..H_DIM * C_DIM).map(|_| gauss(rng) * sh).collect(),
        w_r: (0..H_DIM).map(|_| gauss(rng) * sr).collect(),
        b_z: vec![0.0; H_DIM],
        b_r: 0.0,
    }
}

/// 用簇标签做 hard pair mining 的 BPTT 训练
fn train_epoch_bptt(
    gru: &mut MinGru,
    vecs: &[f32],
    labels: &[usize],
    dim: usize,
    n: usize,
    rng: &mut StdRng,
    num_pairs: usize,
    lr: f32,
    margin: f32,
) -> (f32, f32) {
    let mut total_loss = 0.0f32;
    let mut total_grad = GruGradients::zeros(gru.w_z.len(), gru.w_h.len(), gru.h_dim);
    let mut count = 0;
    let mut near_dist_sum = 0.0f32;
    let mut far_dist_sum = 0.0f32;
    let mut near_cnt = 0;
    let mut far_cnt = 0;

    for _ in 0..num_pairs {
        let i = rng.gen_range(0..n);
        let j = rng.gen_range(0..n);
        if i == j { continue; }

        let va = &vecs[i * dim..(i + 1) * dim];
        let vb = &vecs[j * dim..(j + 1) * dim];
        let is_near = labels[i] == labels[j];

        let cache_a = gru.forward_with_cache(va);
        let cache_b = gru.forward_with_cache(vb);

        let t = gru.num_chunks;
        let hd = gru.h_dim;
        let checkpoints = [t / 4, t / 2, 3 * t / 4, t];

        // 多检查点 cosine agreement 损失
        let mut total_agree = 0.0f32;
        let ncp = checkpoints.len() as f32;
        for &cp in &checkpoints {
            let ha = &cache_a.h[cp];
            let hb = &cache_b.h[cp];
            let dot: f32 = ha.iter().zip(hb).map(|(a, b)| a * b).sum();
            let na = ha.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
            let nb = hb.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
            total_agree += dot / (na * nb);
        }
        let avg_agree = total_agree / ncp;
        let h_dist = 1.0 - avg_agree;

        if is_near { near_dist_sum += h_dist; near_cnt += 1; }
        else { far_dist_sum += h_dist; far_cnt += 1; }

        let (loss, active) = if is_near {
            (h_dist.max(0.0), true)
        } else {
            let l = (margin - h_dist).max(0.0);
            (l, l > 0.0)
        };
        total_loss += loss;
        if !active { count += 1; continue; }

        let sign = if is_near { -1.0f32 } else { 1.0f32 };

        // 构造 dh_external：在 4 个 checkpoint 注入 cosine 梯度
        let mut dh_ext_a = vec![vec![0.0f32; hd]; t + 1];
        let mut dh_ext_b = vec![vec![0.0f32; hd]; t + 1];

        for &cp in &checkpoints {
            let ha = &cache_a.h[cp];
            let hb = &cache_b.h[cp];
            let na = ha.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
            let nb = hb.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
            let dot: f32 = ha.iter().zip(hb).map(|(a, b)| a * b).sum();

            for k in 0..hd {
                dh_ext_a[cp][k] = sign / ncp * (hb[k] / (na * nb) - dot * ha[k] / (na * na * na * nb));
                dh_ext_b[cp][k] = sign / ncp * (ha[k] / (na * nb) - dot * hb[k] / (na * nb * nb * nb));
            }
        }

        // 直接反向传播 — 不走 route sigmoid
        let grad_a = gru.backward_from_hidden(va, &cache_a, &dh_ext_a);
        let grad_b = gru.backward_from_hidden(vb, &cache_b, &dh_ext_b);

        total_grad.accumulate(&grad_a);
        total_grad.accumulate(&grad_b);
        count += 1;
    }

    if count > 0 {
        total_grad.scale(1.0 / count as f32);
        // gradient clipping
        let grad_norm = grad_norm(&total_grad);
        if grad_norm > 1.0 {
            total_grad.scale(1.0 / grad_norm);
        }
        gru.apply_gradients(&total_grad, lr);
    }

    let avg_near = if near_cnt > 0 { near_dist_sum / near_cnt as f32 } else { 0.0 };
    let avg_far = if far_cnt > 0 { far_dist_sum / far_cnt as f32 } else { 0.0 };

    (total_loss / count.max(1) as f32, avg_far - avg_near) // 返回 loss 和 gap
}

fn grad_norm(g: &GruGradients) -> f32 {
    let s: f32 = g.dw_z.iter().map(|x| x*x).sum::<f32>()
        + g.dw_h.iter().map(|x| x*x).sum::<f32>()
        + g.dw_r.iter().map(|x| x*x).sum::<f32>()
        + g.db_z.iter().map(|x| x*x).sum::<f32>()
        + g.db_r * g.db_r;
    s.sqrt()
}

fn brute_force(vecs: &[f32], dim: usize, q: &[f32], k: usize) -> Vec<(u64, f32)> {
    let n = vecs.len() / dim;
    let mut s: Vec<(u64, f32)> = (0..n)
        .map(|i| (i as u64, cosine_sim(q, &vecs[i * dim..(i + 1) * dim])))
        .collect();
    s.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    s.truncate(k);
    s
}

fn recall(gt: &[(u64, f32)], res: &[(u64, f32)]) -> f64 {
    let s: HashSet<u64> = gt.iter().map(|x| x.0).collect();
    res.iter().filter(|x| s.contains(&x.0)).count() as f64 / gt.len().max(1) as f64
}

fn main() {
    let clusters = 100;
    let per = 200;
    let n = clusters * per;
    let noise = 0.15;
    let nq = 50;
    let top_k = 10;

    eprintln!("══════════════════════════════════════════════════════════════");
    eprintln!("  GRU Router + BPTT v2 (hard mining + larger init)");
    eprintln!("  N={n} dim={DIM} h_dim={H_DIM} chunks={NUM_CHUNKS}");
    eprintln!("══════════════════════════════════════════════════════════════");

    let mut rng = StdRng::seed_from_u64(42);

    eprintln!("\n[1/6] 生成聚簇数据...");
    let (vecs, labels) = gen_clustered(clusters, per, DIM, noise, &mut rng);
    let ids: Vec<u64> = (0..n as u64).collect();

    let queries: Vec<Vec<f32>> = (0..nq).map(|_| {
        let idx = rng.gen_range(0..n);
        let base = &vecs[idx * DIM..(idx + 1) * DIM];
        let mut q: Vec<f32> = base.iter().map(|&x| x + gauss(&mut rng) * 0.05).collect();
        let norm = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        for x in &mut q { *x /= norm; }
        q
    }).collect();

    eprintln!("\n[2/6] BruteForce 基线...");
    let t1 = Instant::now();
    let gts: Vec<_> = queries.iter().map(|q| brute_force(&vecs, DIM, q, top_k)).collect();
    let bf_qps = nq as f64 / t1.elapsed().as_secs_f64();
    eprintln!("      QPS: {bf_qps:.2}");

    eprintln!("\n[3/6] 初始化 MinGRU (h_dim={H_DIM})...");
    let mut gru = init_gru(&mut StdRng::seed_from_u64(777));
    eprintln!("      参数量: {}", gru.num_params());

    // 先看初始化后 route 分布
    {
        let sample_routes = gru.forward_soft(&vecs[0..DIM]);
        let min_r = sample_routes.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_r = sample_routes.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mean_r: f32 = sample_routes.iter().sum::<f32>() / sample_routes.len() as f32;
        eprintln!("      初始 route 分布: min={min_r:.4} max={max_r:.4} mean={mean_r:.4}");
    }

    let epochs = 500;
    let pairs = 2000;
    let margin = 0.15;

    eprintln!("\n[4/6] BPTT 训练 ({epochs} ep, {pairs} pairs, lr decay, margin={margin})...");
    let t_train = Instant::now();
    for ep in 0..epochs {
        // lr 从 0.05 线性衰减到 0.005
        let lr = 0.05 * (1.0 - ep as f32 / epochs as f32) + 0.005;
        let (loss, gap) = train_epoch_bptt(
            &mut gru, &vecs, &labels, DIM, n, &mut rng,
            pairs, lr, margin,
        );
        if ep % 50 == 0 || ep == epochs - 1 {
            eprint!("\r      ep {}/{}: loss={:.6} gap={:.6} lr={:.4}", ep + 1, epochs, loss, gap, lr);
        }
    }
    eprintln!("\n      训练耗时: {:.2}s", t_train.elapsed().as_secs_f64());

    // 训练后 route 分布
    {
        let r = gru.forward_soft(&vecs[0..DIM]);
        let min_r = r.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_r = r.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        eprintln!("      训练后 route: min={min_r:.4} max={max_r:.4}");
    }

    // ═══ 新策略：用 hidden state 的符号位直接做 hash ═══
    // 不再走 sigmoid route，避免所有 route ≈ 0.5 的问题
    eprintln!("\n[5/7] 构建 hidden-state sign-bit 索引...");

    // 自定义 hash：取 h[T/2] 和 h[T] 的符号位，各 16 维 = 32 bit
    let hash_fn = |v: &[f32]| -> u64 {
        let cache = gru.forward_with_cache(v);
        let t = gru.num_chunks;
        let hd = gru.h_dim;
        let mut hash: u64 = 0;
        let mut bit = 63u32;

        // 从 4 个时间检查点取符号位
        for &cp in &[t / 4, t / 2, 3 * t / 4, t] {
            for i in 0..hd.min(16) {
                if cache.h[cp][i] > 0.0 {
                    hash |= 1u64 << bit;
                }
                if bit == 0 { break; }
                bit = bit.wrapping_sub(1);
            }
        }
        hash
    };

    // 诊断 hash 多样性
    {
        let mut unique = std::collections::HashSet::new();
        for i in 0..n {
            unique.insert(hash_fn(&vecs[i * DIM..(i + 1) * DIM]));
        }
        eprintln!("      hash 多样性: {}/{n} 个不同 hash", unique.len());

        // 看看同簇的 Hamming 距离 vs 异簇
        let mut same_ham = 0u64;
        let mut diff_ham = 0u64;
        let mut same_cnt = 0u64;
        let mut diff_cnt = 0u64;
        let sample = 500;
        for _ in 0..sample {
            let i = rng.gen_range(0..n);
            let j = rng.gen_range(0..n);
            if i == j { continue; }
            let ha = hash_fn(&vecs[i * DIM..(i + 1) * DIM]);
            let hb = hash_fn(&vecs[j * DIM..(j + 1) * DIM]);
            let ham = (ha ^ hb).count_ones() as u64;
            if labels[i] == labels[j] {
                same_ham += ham; same_cnt += 1;
            } else {
                diff_ham += ham; diff_cnt += 1;
            }
        }
        let avg_same = same_ham as f64 / same_cnt.max(1) as f64;
        let avg_diff = diff_ham as f64 / diff_cnt.max(1) as f64;
        eprintln!("      同簇 Hamming: {avg_same:.1}, 异簇 Hamming: {avg_diff:.1}");
    }

    // 构建索引：按 hash 排序
    let t_build = Instant::now();
    let mut entries: Vec<(u64, usize)> = (0..n)
        .map(|i| (hash_fn(&vecs[i * DIM..(i + 1) * DIM]), i))
        .collect();
    entries.sort_unstable_by_key(|e| e.0);

    let mut sorted_vecs = Vec::with_capacity(n * DIM);
    let mut sorted_ids = Vec::with_capacity(n);
    let mut sorted_hashes = Vec::with_capacity(n);
    for &(hash, orig) in &entries {
        sorted_vecs.extend_from_slice(&vecs[orig * DIM..(orig + 1) * DIM]);
        sorted_ids.push(ids[orig]);
        sorted_hashes.push(hash);
    }
    eprintln!("      构建: {:.2}s", t_build.elapsed().as_secs_f64());

    eprintln!("\n[6/7] 搜索...");
    eprintln!("{:<12} {:>10} {:>10} {:>10}", "Window", "Recall@10", "QPS", "加速比");
    eprintln!("{}", "─".repeat(45));

    let sqrt_n = (n as f64).sqrt() as usize;
    for &w in &[sqrt_n, sqrt_n*2, sqrt_n*4, sqrt_n*8, sqrt_n*16, n/4] {
        let t = Instant::now();
        let mut tr = 0.0;
        for (qi, q) in queries.iter().enumerate() {
            let q_hash = hash_fn(q);
            let landing = sorted_hashes.partition_point(|&h| h < q_hash);
            let half = w / 2;
            let lo = landing.saturating_sub(half);
            let hi = (landing + half).min(n);

            let mut cands: Vec<(u64, f32)> = (lo..hi)
                .map(|i| {
                    let v = &sorted_vecs[i * DIM..(i + 1) * DIM];
                    (sorted_ids[i], cosine_sim(q, v))
                })
                .collect();
            cands.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            cands.truncate(top_k);
            tr += recall(&gts[qi], &cands);
        }
        let qps = nq as f64 / t.elapsed().as_secs_f64();
        eprintln!("{:<12} {:>10.4} {:>10.2} {:>10.2}x",
            format!("W={w}"), tr / nq as f64, qps, qps / bf_qps);
    }
}

