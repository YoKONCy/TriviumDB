use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::time::Instant;
use triviumdb::index::quiver::{QuIVer, QuIVerConfig, QuIVerSearchConfig, SelectMode};

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
fn gen_clustered(c: usize, p: usize, d: usize, noise: f32, rng: &mut StdRng) -> (Vec<f32>, Vec<u64>) {
    let mut vecs = Vec::with_capacity(c*p*d);
    let mut ids = Vec::with_capacity(c*p);
    for ci in 0..c {
        let ctr = gen_unit(rng, d);
        for pi in 0..p {
            let mut v: Vec<f32> = ctr.iter().map(|&x| x + gauss(rng)*noise).collect();
            let n = v.iter().map(|x| x*x).sum::<f32>().sqrt().max(1e-9);
            for x in &mut v { *x /= n; }
            vecs.extend_from_slice(&v);
            ids.push((ci * p + pi) as u64);
        }
    }
    (vecs, ids)
}
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let ab: f32 = a.iter().zip(b).map(|(x,y)| x*y).sum();
    let na = a.iter().map(|x| x*x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x*x).sum::<f32>().sqrt();
    ab / (na*nb).max(1e-30)
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

fn run_bench(dim: usize, n: usize, clusters: usize, noise: f32) {
    let per = n / clusters;
    let nq = 50;
    let top_k = 10;

    eprintln!("\n{}", "═".repeat(60));
    eprintln!("  BQ-HNSW  dim={dim}  N={n}  clusters={clusters}  noise={noise}");
    eprintln!("{}", "═".repeat(60));

    let mut rng = StdRng::seed_from_u64(42);
    let (vecs, ids) = gen_clustered(clusters, per, dim, noise, &mut rng);

    let queries: Vec<Vec<f32>> = (0..nq).map(|_| {
        let idx = rng.gen_range(0..n);
        let base = &vecs[idx*dim..(idx+1)*dim];
        let mut q: Vec<f32> = base.iter().map(|&x| x + gauss(&mut rng)*0.05).collect();
        let norm = q.iter().map(|x| x*x).sum::<f32>().sqrt().max(1e-9);
        for x in &mut q { *x /= norm; }
        q
    }).collect();

    let t0 = Instant::now();
    let gts: Vec<_> = queries.iter().map(|q| brute_force(&vecs, dim, q, top_k)).collect();
    let bf_qps = nq as f64 / t0.elapsed().as_secs_f64();
    eprintln!("  BruteForce QPS: {bf_qps:.2}");

    // BQ Hamming gap 诊断
    {
        use triviumdb::index::bq::BqSignature;
        let mut same = 0u64; let mut diff = 0u64;
        let mut sc = 0u64; let mut dc = 0u64;
        for _ in 0..2000 {
            let i = rng.gen_range(0..n);
            let j = rng.gen_range(0..n);
            if i == j { continue; }
            let si = BqSignature::from_vector(&vecs[i*dim..(i+1)*dim]);
            let sj = BqSignature::from_vector(&vecs[j*dim..(j+1)*dim]);
            let h = si.hamming_distance(&sj);
            if i/per == j/per { same += h as u64; sc += 1; }
            else { diff += h as u64; dc += 1; }
        }
        eprintln!("  BQ gap: 同簇={:.1} 异簇={:.1} Δ={:.1} ({:.2}%)",
            same as f64/sc as f64, diff as f64/dc as f64,
            diff as f64/dc as f64 - same as f64/sc as f64,
            (diff as f64/dc as f64 - same as f64/sc as f64) / (dim as f64) * 100.0);
    }

    let ef_tests = [64, 128, 256, 512, 1024];

    for (label, mode) in [("Heuristic", SelectMode::Heuristic), ("BCM", SelectMode::BCM)] {
        eprintln!("\n  ── {} ──", label);
        let config = QuIVerConfig { m: 16, ef_construction: 128, select_mode: mode };
        let mut index = QuIVer::new(dim, &config);
        let mut lcg: u64 = 12345;

        let t0 = Instant::now();
        for i in 0..n {
            index.insert(&vecs[i*dim..(i+1)*dim], ids[i], &mut lcg);
        }
        let build_s = t0.elapsed().as_secs_f64();
        let stats = index.stats();
        eprintln!("  构建: {:.2}s ({:.0} ins/s) | Hot: {:.1}MB Cold: {:.1}MB",
            build_s, n as f64/build_s,
            stats.hot_bytes as f64/1048576.0, stats.cold_bytes as f64/1048576.0);

        eprintln!("  {:<8} {:>8} {:>8} {:>8}", "ef", "Recall", "QPS", "加速");
        for &ef in &ef_tests {
            let cfg = QuIVerSearchConfig { top_k, ef_search: ef, rerank_limit: None };
            let t0 = Instant::now();
            let mut tr = 0.0;
            for (qi, q) in queries.iter().enumerate() {
                let res = index.search(q, &cfg);
                tr += recall(&gts[qi], &res);
            }
            let qps = nq as f64 / t0.elapsed().as_secs_f64();
            eprintln!("  {:<8} {:>7.1}% {:>8.0} {:>7.1}x",
                format!("ef={ef}"), tr/nq as f64*100.0, qps, qps/bf_qps);
        }
    }
}

fn main() {
    // 768d (GPT-3 / BGE)
    run_bench(768, 20000, 100, 0.15);

    // 1536d (OpenAI text-embedding-3-large)
    run_bench(1536, 20000, 100, 0.15);
}
