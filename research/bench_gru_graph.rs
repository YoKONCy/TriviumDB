use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{BinaryHeap, HashSet};
use std::cmp::{Ordering, Reverse};
use std::time::Instant;
use triviumdb::index::gru_router::{GruGradients, MinGru};

#[derive(Clone, Copy)]
struct F32Ord(f32);
impl PartialEq for F32Ord { fn eq(&self, o: &Self) -> bool { self.0 == o.0 } }
impl Eq for F32Ord {}
impl PartialOrd for F32Ord {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) }
}
impl Ord for F32Ord {
    fn cmp(&self, o: &Self) -> Ordering { self.0.partial_cmp(&o.0).unwrap_or(Ordering::Equal) }
}

const DIM: usize = 768;
const H_DIM: usize = 16;
const C_DIM: usize = 16;
const T: usize = DIM / C_DIM;

fn gauss(rng: &mut StdRng) -> f32 {
    let u1 = rng.gen_range(1e-10f32..1.0);
    let u2 = rng.gen_range(0.0f32..1.0);
    (-2.0f32 * u1.ln()).sqrt() * (2.0f32 * std::f32::consts::PI * u2).cos()
}
fn gen_unit(rng: &mut StdRng, d: usize) -> Vec<f32> {
    let v: Vec<f32> = (0..d).map(|_| gauss(rng)).collect();
    let n = v.iter().map(|x| x*x).sum::<f32>().sqrt().max(1e-9);
    v.iter().map(|x| x/n).collect()
}
fn gen_clustered(c: usize, p: usize, d: usize, noise: f32, rng: &mut StdRng) -> (Vec<f32>, Vec<usize>) {
    let mut vecs = Vec::with_capacity(c*p*d);
    let mut labels = Vec::with_capacity(c*p);
    for ci in 0..c {
        let ctr = gen_unit(rng, d);
        for _ in 0..p {
            let mut v: Vec<f32> = ctr.iter().map(|&x| x + gauss(rng)*noise).collect();
            let n = v.iter().map(|x| x*x).sum::<f32>().sqrt().max(1e-9);
            for x in &mut v { *x /= n; }
            vecs.extend_from_slice(&v);
            labels.push(ci);
        }
    }
    (vecs, labels)
}
fn dot16(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x,y)| x*y).sum()
}
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let ab: f32 = a.iter().zip(b).map(|(x,y)| x*y).sum();
    let na = a.iter().map(|x| x*x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x*x).sum::<f32>().sqrt();
    ab / (na*nb).max(1e-30)
}

// ═══ GRU 初始化 + 训练 ═══
fn init_gru(rng: &mut StdRng) -> MinGru {
    let cd = H_DIM + C_DIM;
    let s = (2.0/cd as f32).sqrt();
    MinGru {
        h_dim: H_DIM, c_dim: C_DIM, num_chunks: T,
        w_z: (0..H_DIM*cd).map(|_| gauss(rng)*s).collect(),
        w_h: (0..H_DIM*C_DIM).map(|_| gauss(rng)*(2.0/C_DIM as f32).sqrt()).collect(),
        w_r: (0..H_DIM).map(|_| gauss(rng)*1.0).collect(),
        b_z: vec![0.0; H_DIM], b_r: 0.0,
    }
}

/// 提取 GRU 最终 hidden state 作为嵌入
fn gru_embed(gru: &MinGru, v: &[f32]) -> Vec<f32> {
    let cache = gru.forward_with_cache(v);
    cache.h[gru.num_chunks].clone()
}

/// 训练 GRU 嵌入（对比学习 on hidden states）
fn train_gru(
    gru: &mut MinGru, vecs: &[f32], labels: &[usize],
    dim: usize, n: usize, rng: &mut StdRng,
    epochs: usize, pairs: usize,
) {
    let margin = 0.15f32;
    for ep in 0..epochs {
        let lr = 0.05 * (1.0 - ep as f32 / epochs as f32) + 0.005;
        let mut total_grad = GruGradients::zeros(gru.w_z.len(), gru.w_h.len(), gru.h_dim);
        let mut cnt = 0;
        let mut loss_sum = 0.0f32;

        for _ in 0..pairs {
            let i = rng.gen_range(0..n);
            let j = rng.gen_range(0..n);
            if i == j { continue; }
            let va = &vecs[i*dim..(i+1)*dim];
            let vb = &vecs[j*dim..(j+1)*dim];
            let near = labels[i] == labels[j];

            let ca = gru.forward_with_cache(va);
            let cb = gru.forward_with_cache(vb);
            let t = gru.num_chunks;
            let hd = gru.h_dim;

            // h_T cosine similarity loss
            let ha = &ca.h[t]; let hb = &cb.h[t];
            let dot: f32 = ha.iter().zip(hb).map(|(a,b)| a*b).sum();
            let na = ha.iter().map(|x| x*x).sum::<f32>().sqrt().max(1e-8);
            let nb = hb.iter().map(|x| x*x).sum::<f32>().sqrt().max(1e-8);
            let sim = dot / (na*nb);
            let dist = 1.0 - sim;

            let (loss, active) = if near {
                (dist.max(0.0), true)
            } else {
                let l = (margin - dist).max(0.0);
                (l, l > 0.0)
            };
            loss_sum += loss;
            if !active { cnt += 1; continue; }

            let sign = if near { -1.0f32 } else { 1.0f32 };
            let mut dh_ext = vec![vec![0.0f32; hd]; t + 1];
            for k in 0..hd {
                dh_ext[t][k] = sign * (hb[k]/(na*nb) - dot*ha[k]/(na*na*na*nb));
            }
            let mut dh_ext_b = vec![vec![0.0f32; hd]; t + 1];
            for k in 0..hd {
                dh_ext_b[t][k] = sign * (ha[k]/(na*nb) - dot*hb[k]/(na*nb*nb*nb));
            }

            let ga = gru.backward_from_hidden(va, &ca, &dh_ext);
            let gb = gru.backward_from_hidden(vb, &cb, &dh_ext_b);
            total_grad.accumulate(&ga);
            total_grad.accumulate(&gb);
            cnt += 1;
        }
        if cnt > 0 {
            total_grad.scale(1.0 / cnt as f32);
            let gn: f32 = total_grad.dw_z.iter().chain(&total_grad.dw_h).chain(&total_grad.dw_r)
                .chain(&total_grad.db_z).map(|x| x*x).sum::<f32>().sqrt();
            if gn > 1.0 { total_grad.scale(1.0 / gn); }
            gru.apply_gradients(&total_grad, lr);
        }
        if ep % 50 == 0 || ep == epochs - 1 {
            eprint!("\r      ep {}/{}: loss={:.5}", ep+1, epochs, loss_sum/cnt.max(1) as f32);
        }
    }
    eprintln!();
}

// ═══ 图构建 + 搜索 ═══

/// 采样构建 k-NN 图（用 768d 距离，保证图质量）
fn build_graph(vecs: &[f32], dim: usize, n: usize, k: usize, sample: usize, rng: &mut StdRng) -> Vec<Vec<u32>> {
    let mut adj = vec![Vec::new(); n];
    for i in 0..n {
        let vi = &vecs[i*dim..(i+1)*dim];
        // 采样候选
        let mut cands: Vec<(f32, u32)> = (0..sample)
            .map(|_| {
                let j = rng.gen_range(0..n);
                (cosine(vi, &vecs[j*dim..(j+1)*dim]), j as u32)
            })
            .collect();
        cands.sort_unstable_by(|a,b| b.0.partial_cmp(&a.0).unwrap());
        cands.dedup_by_key(|x| x.1);
        // DPP-like: 贪心选多样性邻居
        let mut selected = Vec::with_capacity(k);
        for &(sim, cid) in &cands {
            if selected.len() >= k { break; }
            if cid == i as u32 { continue; }
            let diverse = selected.iter().all(|&s: &u32| {
                cosine(&vecs[cid as usize*dim..(cid as usize+1)*dim],
                       &vecs[s as usize*dim..(s as usize+1)*dim]) < 0.95
            });
            if diverse || selected.len() < 4 {
                selected.push(cid);
            }
        }
        adj[i] = selected;
    }
    // 双向化
    let mut biadj = adj.clone();
    for i in 0..n {
        for &j in &adj[i] {
            if !biadj[j as usize].contains(&(i as u32)) {
                biadj[j as usize].push(i as u32);
            }
        }
    }
    biadj
}

/// 图 beam search — 可选择用 emb（低维）或 vec（高维）做距离
fn beam_search(
    adj: &[Vec<u32>],
    vecs: &[f32], dim: usize,       // 768d 向量
    embs: &[f32], edim: usize,      // 16d 嵌入
    query: &[f32],                   // 768d query
    query_emb: &[f32],              // 16d query 嵌入
    ef: usize,
    top_k: usize,
    use_emb_for_routing: bool,       // 是否用 16d 做路由
) -> (Vec<(u64, f32)>, usize) {     // (结果, 距离计算次数)
    let n = adj.len();
    let entry = 0u32; // 简单入口
    let mut visited = HashSet::new();
    // (neg_sim, id)
    let mut candidates: BinaryHeap<Reverse<(F32Ord, u32)>> = BinaryHeap::new();
    let mut results: BinaryHeap<(F32Ord, u32)> = BinaryHeap::new();
    let mut dist_count = 0usize;

    let entry_sim = if use_emb_for_routing {
        dist_count += 1;
        dot16(query_emb, &embs[entry as usize * edim..(entry as usize + 1) * edim])
    } else {
        dist_count += 1;
        cosine(query, &vecs[entry as usize * dim..(entry as usize + 1) * dim])
    };

    visited.insert(entry);
    candidates.push(Reverse((F32Ord(-entry_sim), entry)));
    results.push((F32Ord(-entry_sim), entry));

    while let Some(Reverse((neg_sim, cur))) = candidates.pop() {
        // 检查是否比 results 中最差的还差
        if results.len() >= ef {
            if neg_sim > results.peek().unwrap().0 { break; }
        }

        for &nb in &adj[cur as usize] {
            if visited.contains(&nb) { continue; }
            visited.insert(nb);

            let nb_sim = if use_emb_for_routing {
                dist_count += 1;
                dot16(query_emb, &embs[nb as usize * edim..(nb as usize + 1) * edim])
            } else {
                dist_count += 1;
                cosine(query, &vecs[nb as usize * dim..(nb as usize + 1) * dim])
            };

            let neg = F32Ord(-nb_sim);
            if results.len() < ef || neg < results.peek().unwrap().0 {
                candidates.push(Reverse((neg, nb)));
                results.push((neg, nb));
                if results.len() > ef { results.pop(); }
            }
        }
    }

    // 精排：用 768d cosine 重排 top candidates
    let mut final_results: Vec<(u64, f32)> = results.into_sorted_vec().iter()
        .map(|&(_, id)| {
            let sim = cosine(query, &vecs[id as usize * dim..(id as usize + 1) * dim]);
            (id as u64, sim)
        })
        .collect();
    final_results.sort_unstable_by(|a,b| b.1.partial_cmp(&a.1).unwrap());
    final_results.truncate(top_k);
    (final_results, dist_count)
}

fn brute_force(vecs: &[f32], dim: usize, q: &[f32], k: usize) -> Vec<(u64, f32)> {
    let n = vecs.len() / dim;
    let mut s: Vec<(u64, f32)> = (0..n).map(|i| (i as u64, cosine(q, &vecs[i*dim..(i+1)*dim]))).collect();
    s.sort_unstable_by(|a,b| b.1.partial_cmp(&a.1).unwrap());
    s.truncate(k);
    s
}
fn recall(gt: &[(u64,f32)], res: &[(u64,f32)]) -> f64 {
    let s: HashSet<u64> = gt.iter().map(|x| x.0).collect();
    res.iter().filter(|x| s.contains(&x.0)).count() as f64 / gt.len().max(1) as f64
}

fn main() {
    let clusters = 100;
    let per = 200;
    let n = clusters * per;
    let nq = 50;
    let top_k = 10;

    eprintln!("════════════════════════════════════════════════════════════");
    eprintln!("  GRU-Accelerated Graph Search");
    eprintln!("  N={n} dim={DIM} h_dim={H_DIM}");
    eprintln!("  GRU 16d 做路由距离 → DPP 图结构 → 768d 精排");
    eprintln!("════════════════════════════════════════════════════════════");

    let mut rng = StdRng::seed_from_u64(42);
    eprintln!("\n[1/7] 生成聚簇数据...");
    let (vecs, labels) = gen_clustered(clusters, per, DIM, 0.15, &mut rng);

    let queries: Vec<Vec<f32>> = (0..nq).map(|_| {
        let idx = rng.gen_range(0..n);
        let base = &vecs[idx*DIM..(idx+1)*DIM];
        let mut q: Vec<f32> = base.iter().map(|&x| x + gauss(&mut rng)*0.05).collect();
        let norm = q.iter().map(|x| x*x).sum::<f32>().sqrt().max(1e-9);
        for x in &mut q { *x /= norm; }
        q
    }).collect();

    eprintln!("\n[2/7] BruteForce 基线...");
    let t0 = Instant::now();
    let gts: Vec<_> = queries.iter().map(|q| brute_force(&vecs, DIM, q, top_k)).collect();
    let bf_qps = nq as f64 / t0.elapsed().as_secs_f64();
    eprintln!("      QPS: {bf_qps:.2}");

    eprintln!("\n[3/7] 训练 GRU 嵌入器...");
    let mut gru = init_gru(&mut StdRng::seed_from_u64(777));
    let t0 = Instant::now();
    train_gru(&mut gru, &vecs, &labels, DIM, n, &mut rng, 300, 2000);
    eprintln!("      训练耗时: {:.1}s, 参数: {}", t0.elapsed().as_secs_f64(), gru.num_params());

    eprintln!("\n[4/7] 计算 GRU 嵌入...");
    let t0 = Instant::now();
    let mut embs = Vec::with_capacity(n * H_DIM);
    for i in 0..n {
        let e = gru_embed(&gru, &vecs[i*DIM..(i+1)*DIM]);
        embs.extend_from_slice(&e);
    }
    eprintln!("      {:.2}s, 嵌入维度: {H_DIM}d", t0.elapsed().as_secs_f64());

    // 评估嵌入质量
    {
        let mut same_sim = 0.0f64; let mut diff_sim = 0.0f64;
        let mut sc = 0u64; let mut dc = 0u64;
        for _ in 0..2000 {
            let i = rng.gen_range(0..n);
            let j = rng.gen_range(0..n);
            if i == j { continue; }
            let s = dot16(&embs[i*H_DIM..(i+1)*H_DIM], &embs[j*H_DIM..(j+1)*H_DIM]);
            if labels[i] == labels[j] { same_sim += s as f64; sc += 1; }
            else { diff_sim += s as f64; dc += 1; }
        }
        eprintln!("      嵌入质量: 同簇 dot={:.4}, 异簇 dot={:.4}, gap={:.4}",
            same_sim/sc as f64, diff_sim/dc as f64,
            same_sim/sc as f64 - diff_sim/dc as f64);
    }

    let query_embs: Vec<Vec<f32>> = queries.iter().map(|q| gru_embed(&gru, q)).collect();

    eprintln!("\n[5/7] 构建图 (采样 k-NN + DPP 多样性)...");
    let t0 = Instant::now();
    let adj = build_graph(&vecs, DIM, n, 16, 200, &mut rng);
    let avg_deg = adj.iter().map(|a| a.len()).sum::<usize>() as f64 / n as f64;
    eprintln!("      {:.1}s, 平均度数: {avg_deg:.1}", t0.elapsed().as_secs_f64());

    eprintln!("\n[6/7] 768d 全精度图搜索 (对照组)...");
    eprintln!("{:<8} {:>10} {:>10} {:>10} {:>10}", "ef", "Recall@10", "QPS", "加速比", "Avg跳数");
    eprintln!("{}", "─".repeat(52));
    for &ef in &[32, 64, 128, 256, 512] {
        let t0 = Instant::now();
        let mut tr = 0.0;
        let mut td = 0usize;
        for (qi, q) in queries.iter().enumerate() {
            let (res, dc) = beam_search(&adj, &vecs, DIM, &embs, H_DIM,
                q, &query_embs[qi], ef, top_k, false);
            tr += recall(&gts[qi], &res);
            td += dc;
        }
        let qps = nq as f64 / t0.elapsed().as_secs_f64();
        eprintln!("{:<8} {:>10.4} {:>10.2} {:>10.2}x {:>10.0}",
            format!("ef={ef}"), tr/nq as f64, qps, qps/bf_qps, td as f64/nq as f64);
    }

    eprintln!("\n[7/7] GRU 16d 加速图搜索...");
    eprintln!("{:<8} {:>10} {:>10} {:>10} {:>10}", "ef", "Recall@10", "QPS", "加速比", "Avg跳数");
    eprintln!("{}", "─".repeat(52));
    for &ef in &[32, 64, 128, 256, 512] {
        let t0 = Instant::now();
        let mut tr = 0.0;
        let mut td = 0usize;
        for (qi, q) in queries.iter().enumerate() {
            let (res, dc) = beam_search(&adj, &vecs, DIM, &embs, H_DIM,
                q, &query_embs[qi], ef, top_k, true);
            tr += recall(&gts[qi], &res);
            td += dc;
        }
        let qps = nq as f64 / t0.elapsed().as_secs_f64();
        eprintln!("{:<8} {:>10.4} {:>10.2} {:>10.2}x {:>10.0}",
            format!("ef={ef}"), tr/nq as f64, qps, qps/bf_qps, td as f64/nq as f64);
    }

    // ═══ 随机投影对照组 ═══
    eprintln!("\n[8/8] 随机投影 16d 对照...");
    let rdim = 16;
    // 生成随机投影矩阵 768 → 16 (JL)
    let mut proj_rng = StdRng::seed_from_u64(123);
    let proj: Vec<f32> = (0..DIM*rdim).map(|_| gauss(&mut proj_rng) / (rdim as f32).sqrt()).collect();

    let project = |v: &[f32]| -> Vec<f32> {
        (0..rdim).map(|j| {
            let mut s = 0.0f32;
            for i in 0..DIM { s += v[i] * proj[i * rdim + j]; }
            s
        }).collect()
    };

    let mut rp_embs = Vec::with_capacity(n * rdim);
    for i in 0..n {
        let e = project(&vecs[i*DIM..(i+1)*DIM]);
        rp_embs.extend_from_slice(&e);
    }
    let rp_query_embs: Vec<Vec<f32>> = queries.iter().map(|q| project(q)).collect();

    // 评估 RP 嵌入质量
    {
        let mut same_sim = 0.0f64; let mut diff_sim = 0.0f64;
        let mut sc = 0u64; let mut dc = 0u64;
        for _ in 0..2000 {
            let i = rng.gen_range(0..n);
            let j = rng.gen_range(0..n);
            if i == j { continue; }
            let s = dot16(&rp_embs[i*rdim..(i+1)*rdim], &rp_embs[j*rdim..(j+1)*rdim]);
            if labels[i] == labels[j] { same_sim += s as f64; sc += 1; }
            else { diff_sim += s as f64; dc += 1; }
        }
        eprintln!("      RP 嵌入质量: 同簇 dot={:.4}, 异簇 dot={:.4}, gap={:.4}",
            same_sim/sc as f64, diff_sim/dc as f64,
            same_sim/sc as f64 - diff_sim/dc as f64);
    }

    eprintln!("{:<8} {:>10} {:>10} {:>10} {:>10}", "ef", "Recall@10", "QPS", "加速比", "Avg跳数");
    eprintln!("{}", "─".repeat(52));
    for &ef in &[32, 64, 128, 256, 512] {
        let t0 = Instant::now();
        let mut tr = 0.0;
        let mut td = 0usize;
        for (qi, q) in queries.iter().enumerate() {
            let (res, dc) = beam_search(&adj, &vecs, DIM, &rp_embs, rdim,
                q, &rp_query_embs[qi], ef, top_k, true);
            tr += recall(&gts[qi], &res);
            td += dc;
        }
        let qps = nq as f64 / t0.elapsed().as_secs_f64();
        eprintln!("{:<8} {:>10.4} {:>10.2} {:>10.2}x {:>10.0}",
            format!("ef={ef}"), tr/nq as f64, qps, qps/bf_qps, td as f64/nq as f64);
    }
}
