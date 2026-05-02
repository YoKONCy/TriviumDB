use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::time::Instant;
use triviumdb::index::dpp_graph::{DppBuildConfig, DppGraphIndex, DppSearchConfig};

const DIM: usize = 768;

fn gen_clustered_data(
    clusters: usize, per_cluster: usize, dim: usize, noise: f32, rng: &mut StdRng,
) -> (Vec<f32>, Vec<u64>) {
    let n = clusters * per_cluster;
    let mut vecs = Vec::with_capacity(n * dim);
    for _ in 0..clusters {
        let center = gen_unit(rng, dim);
        for _ in 0..per_cluster {
            let mut v: Vec<f32> = center.iter().map(|&c| c + gauss(rng) * noise).collect();
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            for x in &mut v { *x /= norm; }
            vecs.extend_from_slice(&v);
        }
    }
    (vecs, (0..n as u64).collect())
}

fn gen_unit(rng: &mut StdRng, dim: usize) -> Vec<f32> {
    let v: Vec<f32> = (0..dim).map(|_| gauss(rng)).collect();
    let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter().map(|x| x / n).collect()
}

fn gauss(rng: &mut StdRng) -> f32 {
    let u1 = rng.gen_range(1e-10f32..1.0);
    let u2 = rng.gen_range(0.0f32..1.0);
    (-2.0f32 * u1.ln()).sqrt() * (2.0f32 * std::f32::consts::PI * u2).cos()
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
    let clusters = 100;
    let per_cluster = 200;
    let n = clusters * per_cluster; // 20,000
    let noise = 0.15;
    let nq = 50;
    let top_k = 10;

    eprintln!("══════════════════════════════════════════════════════");
    eprintln!("  DPP Graph 中规模验证");
    eprintln!("  N={n} dim={DIM} clusters={clusters} noise={noise}");
    eprintln!("══════════════════════════════════════════════════════");

    let mut rng = StdRng::seed_from_u64(42);

    eprintln!("\n[1/5] 生成聚簇数据...");
    let t0 = Instant::now();
    let (vecs, ids) = gen_clustered_data(clusters, per_cluster, DIM, noise, &mut rng);
    eprintln!("      {:.2}s", t0.elapsed().as_secs_f64());

    let queries: Vec<Vec<f32>> = (0..nq).map(|_| {
        let idx = rng.gen_range(0..n);
        let base = &vecs[idx * DIM..(idx + 1) * DIM];
        let mut q: Vec<f32> = base.iter().map(|&x| x + gauss(&mut rng) * 0.05).collect();
        let norm: f32 = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        for x in &mut q { *x /= norm; }
        q
    }).collect();

    eprintln!("\n[2/5] BruteForce 基线...");
    let t1 = Instant::now();
    let gts: Vec<_> = queries.iter().map(|q| brute_force(&vecs, DIM, q, top_k)).collect();
    let bf_qps = nq as f64 / t1.elapsed().as_secs_f64();
    eprintln!("      QPS: {bf_qps:.2}");

    eprintln!("\n[3/5] 构建 DPP Graph (degree=16, pool=64, sample=1000)...");
    let t2 = Instant::now();
    let idx = DppGraphIndex::build(&vecs, &ids, DIM, &DppBuildConfig {
        degree: 16,
        candidate_pool: 64,
        num_entry_points: 12,
        sample_size: 1000,
    });
    let build_s = t2.elapsed().as_secs_f64();
    eprintln!("      构建: {build_s:.2}s");
    eprintln!("      平均度数: {:.1}", idx.avg_degree());
    eprintln!("      图内存: {:.2} MB", idx.memory_bytes() as f64 / 1e6);

    eprintln!("\n[4/5] DPP Graph 搜索...");
    eprintln!("{:<12} {:>10} {:>10} {:>10}", "ef", "Recall@10", "QPS", "加速比");
    eprintln!("{}", "─".repeat(45));

    for &ef in &[32, 64, 128, 256, 512, 1024] {
        let cfg = DppSearchConfig { top_k, ef_search: ef };
        let t = Instant::now();
        let mut total_r = 0.0;
        for (i, q) in queries.iter().enumerate() {
            total_r += recall(&gts[i], &idx.search(q, &cfg));
        }
        let qps = nq as f64 / t.elapsed().as_secs_f64();
        eprintln!("{:<12} {:>10.4} {:>10.2} {:>10.2}x",
            format!("ef={ef}"), total_r / nq as f64, qps, qps / bf_qps);
    }

    eprintln!("\n[5/5] 完成");
}
