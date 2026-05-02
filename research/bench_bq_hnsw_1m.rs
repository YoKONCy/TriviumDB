use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::time::Instant;
use triviumdb::index::quiver::{QuIVer, QuIVerConfig, SelectMode};

fn gauss(rng: &mut StdRng) -> f32 {
    let u1 = rng.gen_range(1e-10f32..1.0);
    let u2 = rng.gen_range(0.0f32..1.0);
    (-2.0f32 * u1.ln()).sqrt() * (2.0f32 * std::f32::consts::PI * u2).cos()
}

fn main() {
    let dim = 3072;
    let n: usize = 1_000_000;
    let clusters = 500;
    let per = n / clusters;
    let noise = 0.15f32;

    eprintln!("═══════════════════════════════════════════════════════════════");
    eprintln!("  BQ-HNSW 构图吞吐量测试  dim={dim}  N={n}  clusters={clusters}");
    eprintln!("═══════════════════════════════════════════════════════════════");

    let mut rng = StdRng::seed_from_u64(42);

    // 预生成簇心（节省内存，不一次性生成所有向量）
    eprintln!("  生成 {clusters} 个簇心...");
    let centers: Vec<Vec<f32>> = (0..clusters).map(|_| {
        let v: Vec<f32> = (0..dim).map(|_| gauss(&mut rng)).collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        v.iter().map(|x| x / norm).collect()
    }).collect();

    // 构建索引 — 流式生成向量，边生成边插入，减少峰值内存
    let config = QuIVerConfig { m: 16, ef_construction: 128, select_mode: SelectMode::Heuristic };
    let mut index = QuIVer::new(dim, &config);
    let mut lcg: u64 = 12345;

    eprintln!("  开始构图...");
    let t_start = Instant::now();
    let mut last_report = Instant::now();
    let mut vec_buf = vec![0.0f32; dim];

    for i in 0..n {
        // 流式生成向量
        let ci = i / per;
        let center = &centers[ci.min(clusters - 1)];
        for d in 0..dim {
            vec_buf[d] = center[d] + gauss(&mut rng) * noise;
        }
        let norm = vec_buf.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        for x in vec_buf.iter_mut() { *x /= norm; }

        index.insert(&vec_buf, i as u64, &mut lcg);

        // 每 10 万个报告一次
        if (i + 1) % 100_000 == 0 {
            let elapsed = t_start.elapsed().as_secs_f64();
            let seg = last_report.elapsed().as_secs_f64();
            let avg_ips = (i + 1) as f64 / elapsed;
            let seg_ips = 100_000.0 / seg;
            eprintln!("  [{:>7}] 累计: {:.1}s ({:.0} ins/s) | 本段: {:.1}s ({:.0} ins/s)",
                i + 1, elapsed, avg_ips, seg, seg_ips);
            last_report = Instant::now();
        }
    }

    let total = t_start.elapsed().as_secs_f64();
    let stats = index.stats();

    eprintln!("\n{}", "─".repeat(60));
    eprintln!("  完成！");
    eprintln!("  总耗时:      {:.2}s", total);
    eprintln!("  平均吞吐量:  {:.0} ins/s", n as f64 / total);
    eprintln!("  Hot 内存:    {:.1} MB", stats.hot_bytes as f64 / 1048576.0);
    eprintln!("  Cold 内存:   {:.1} MB", stats.cold_bytes as f64 / 1048576.0);
    eprintln!("  总内存:      {:.1} GB", (stats.hot_bytes + stats.cold_bytes) as f64 / 1073741824.0);
    eprintln!("  平均度数:    {:.1}", stats.avg_degree_l0);
    eprintln!("  最大层数:    {}", stats.max_level);
    eprintln!("{}", "─".repeat(60));
}
