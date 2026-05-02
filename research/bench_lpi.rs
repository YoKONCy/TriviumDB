use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::time::Instant;
use triviumdb::index::lpi::{LpiIndex, LpiQuery};

fn gen_vec(rng: &mut StdRng, dim: usize) -> Vec<f32> {
    let v: Vec<f32> = (0..dim).map(|_| rng.gen_range(-1.0f32..1.0)).collect();
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter().map(|x| x / norm).collect()
}

fn gen_gaussian_proj(rng: &mut StdRng, dim: usize) -> Vec<f32> {
    let raw: Vec<f32> = (0..dim).map(|_| {
        let u1: f32 = rng.gen_range(1e-10f32..1.0);
        let u2: f32 = rng.gen_range(0.0f32..1.0);
        (-2.0_f32 * u1.ln()).sqrt() * (2.0_f32 * std::f32::consts::PI * u2).cos()
    }).collect();
    let norm: f32 = raw.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-30);
    raw.iter().map(|x| x / norm).collect()
}

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let ab: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    ab / (na * nb).max(1e-30)
}

fn brute_force(vectors: &[f32], dim: usize, query: &[f32], top_k: usize) -> Vec<(u64, f32)> {
    let n = vectors.len() / dim;
    let mut scores: Vec<(u64, f32)> = (0..n)
        .map(|i| (i as u64, cosine_sim(query, &vectors[i * dim..(i + 1) * dim])))
        .collect();
    scores.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    scores.truncate(top_k);
    scores
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

    eprintln!("══════════════════════════════════════════════════");
    eprintln!("  LPI 原型验证  N={n} dim={dim} top_k={top_k}");
    eprintln!("══════════════════════════════════════════════════");

    let mut rng = StdRng::seed_from_u64(42);
    eprintln!("\n[1/5] 生成数据...");
    let t0 = Instant::now();
    let mut vecs = Vec::with_capacity(n * dim);
    for _ in 0..n { vecs.extend(gen_vec(&mut rng, dim)); }
    let ids: Vec<u64> = (0..n as u64).collect();
    eprintln!("      {:.2}s", t0.elapsed().as_secs_f64());

    let queries: Vec<Vec<f32>> = (0..nq).map(|_| gen_vec(&mut rng, dim)).collect();

    eprintln!("\n[2/5] BruteForce 基线...");
    let t1 = Instant::now();
    let gts: Vec<_> = queries.iter().map(|q| brute_force(&vecs, dim, q, top_k)).collect();
    let bf_secs = t1.elapsed().as_secs_f64();
    let bf_qps = nq as f64 / bf_secs;
    eprintln!("      QPS: {bf_qps:.2}");

    let segs = (n as f64).sqrt() as usize;
    eprintln!("\n[3/5] 构建 LPI (segments={segs})...");
    let mut proj_rng = StdRng::seed_from_u64(12345);
    let p1 = gen_gaussian_proj(&mut proj_rng, dim);
    let p2 = gen_gaussian_proj(&mut proj_rng, dim);
    let t2 = Instant::now();
    let idx = LpiIndex::build(&vecs, &ids, dim, segs, p1, p2);
    eprintln!("      构建: {:.2}s  内存: {:.1}MB", t2.elapsed().as_secs_f64(), idx.memory_bytes() as f64 / 1e6);

    eprintln!("\n[4/5] 参数扫描...");
    eprintln!("{:<22} {:>10} {:>10} {:>10} {:>10}", "配置", "Recall@10", "QPS", "加速比", "扫描/N");
    eprintln!("{}", "─".repeat(65));

    for &(w_mul, probes, label) in &[
        (1, 1, "W=√N, P=1"),
        (1, 3, "W=√N, P=3"),
        (1, 5, "W=√N, P=5"),
        (2, 3, "W=2√N, P=3"),
        (2, 5, "W=2√N, P=5"),
        (3, 5, "W=3√N, P=5"),
        (3, 7, "W=3√N, P=7"),
        (4, 7, "W=4√N, P=7"),
        (5, 9, "W=5√N, P=9"),
    ] {
        let cfg = LpiQuery { top_k, window_size: segs * w_mul, num_probes: probes };
        let t = Instant::now();
        let mut total_r = 0.0;
        for (i, q) in queries.iter().enumerate() {
            total_r += recall(&gts[i], &idx.search(q, &cfg));
        }
        let secs = t.elapsed().as_secs_f64();
        let qps = nq as f64 / secs;
        eprintln!("{:<22} {:>10.4} {:>10.2} {:>10.2}x {:>10.4}",
            label, total_r / nq as f64, qps, qps / bf_qps,
            (segs * w_mul * probes) as f64 / n as f64);
    }
    eprintln!("\n[5/5] 完成");
}
