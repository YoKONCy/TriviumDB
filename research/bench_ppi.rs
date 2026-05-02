use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::time::Instant;
use triviumdb::index::ppi::{PpiIndex, PpiQuery};

fn gen_unit_vec(rng: &mut StdRng, dim: usize) -> Vec<f32> {
    let v: Vec<f32> = (0..dim).map(|_| rng.gen_range(-1.0f32..1.0)).collect();
    let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter().map(|x| x / n).collect()
}

fn gen_gaussian_anchors(rng: &mut StdRng, k: usize, dim: usize) -> Vec<f32> {
    let mut anchors = Vec::with_capacity(k * dim);
    for _ in 0..k {
        for _ in 0..dim {
            let u1 = rng.gen_range(1e-10f32..1.0);
            let u2 = rng.gen_range(0.0f32..1.0);
            anchors.push((-2.0f32 * u1.ln()).sqrt() * (2.0f32 * std::f32::consts::PI * u2).cos());
        }
    }
    anchors
}

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let ab: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    ab / (na * nb).max(1e-30)
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
    let dim = 768;
    let n = 200_000;
    let nq = 50;
    let top_k = 10;
    let k_anchors = 16; // 16 anchors → u64 编码

    eprintln!("══════════════════════════════════════════════════════");
    eprintln!("  PPI 原型验证  N={n} dim={dim} K={k_anchors} top_k={top_k}");
    eprintln!("══════════════════════════════════════════════════════");

    let mut rng = StdRng::seed_from_u64(42);

    eprintln!("\n[1/5] 生成 {n} 个 {dim}维 向量...");
    let t0 = Instant::now();
    let mut vecs = Vec::with_capacity(n * dim);
    for _ in 0..n { vecs.extend(gen_unit_vec(&mut rng, dim)); }
    let ids: Vec<u64> = (0..n as u64).collect();
    eprintln!("      {:.2}s", t0.elapsed().as_secs_f64());

    let queries: Vec<Vec<f32>> = (0..nq).map(|_| gen_unit_vec(&mut rng, dim)).collect();

    eprintln!("\n[2/5] BruteForce 基线...");
    let t1 = Instant::now();
    let gts: Vec<_> = queries.iter().map(|q| brute_force(&vecs, dim, q, top_k)).collect();
    let bf_s = t1.elapsed().as_secs_f64();
    let bf_qps = nq as f64 / bf_s;
    eprintln!("      QPS: {bf_qps:.2}");

    eprintln!("\n[3/5] 生成 {k_anchors} 个高斯锚点 + 构建 PPI...");
    let mut anchor_rng = StdRng::seed_from_u64(99999);
    let anchors = gen_gaussian_anchors(&mut anchor_rng, k_anchors, dim);
    let t2 = Instant::now();
    let idx = PpiIndex::build(&vecs, &ids, dim, &anchors);
    let build_s = t2.elapsed().as_secs_f64();
    eprintln!("      构建: {build_s:.2}s");
    eprintln!("      索引额外开销: {:.2} MB", idx.overhead_bytes() as f64 / 1e6);
    eprintln!("      总内存: {:.1} MB", idx.memory_bytes() as f64 / 1e6);

    eprintln!("\n[4/5] 参数扫描...");
    eprintln!("{:<16} {:>10} {:>10} {:>10} {:>10}",
        "Window", "Recall@10", "QPS", "加速比", "扫描/N");
    eprintln!("{}", "─".repeat(60));

    let sqrt_n = (n as f64).sqrt() as usize;
    for &w in &[
        sqrt_n / 4,
        sqrt_n / 2,
        sqrt_n,
        sqrt_n * 2,
        sqrt_n * 3,
        sqrt_n * 5,
        sqrt_n * 8,
        sqrt_n * 12,
        sqrt_n * 20,
        n / 4,
    ] {
        let cfg = PpiQuery { top_k, window_size: w };
        let t = Instant::now();
        let mut total_r = 0.0;
        for (i, q) in queries.iter().enumerate() {
            total_r += recall(&gts[i], &idx.search(q, &cfg));
        }
        let s = t.elapsed().as_secs_f64();
        let qps = nq as f64 / s;
        eprintln!("{:<16} {:>10.4} {:>10.2} {:>10.2}x {:>10.4}",
            format!("W={w}"), total_r / nq as f64, qps, qps / bf_qps,
            w as f64 / n as f64);
    }

    eprintln!("\n[5/5] 完成");
}
