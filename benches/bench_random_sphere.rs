//! Random-Sphere 数据集生成器
//!
//! 纯高斯球面向量（无任何结构），用于验证 BQ 在完全随机数据上的表现。
//! 与 Synthetic-LR (bench_random1m.rs) 形成控制变量对比：
//!   - Random-Sphere: 无低秩、无聚簇 → BQ 几乎无法工作 (~0.3% recall)
//!   - Synthetic-LR:  有低秩、有聚簇 → BQ 部分可用 (~50% recall)
//!   - Cohere-1M:     真实对比学习嵌入 → BQ 高效 (~95% recall)
//!
//! 用法：
//!   cargo bench --bench bench_random_sphere
//!   $env:TRIVIUM_ANN_NAME="sphere-1m"
//!   cargo bench --bench bench_sensitivity

use rayon::prelude::*;
use std::io::Write;
use std::time::Instant;

const DIM: usize = 768;
const N_TRAIN: usize = 1_000_000;
const N_TEST: usize = 1_000;
const TOP_K: usize = 10;
const SEED: u64 = 12345;

// ════════════════════════════════════════════════════════
//  xoshiro256** PRNG (与 bench_random1m.rs 相同)
// ════════════════════════════════════════════════════════

struct Rng {
    s: [u64; 4],
}

impl Rng {
    fn new(seed: u64) -> Self {
        let mut state = seed;
        let mut s = [0u64; 4];
        for slot in s.iter_mut() {
            state = state.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            *slot = z ^ (z >> 31);
        }
        Self { s }
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        let result = (self.s[1].wrapping_mul(5)).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// 均匀 [0, 1)
    #[inline]
    fn next_f32_01(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Box-Muller: 标准正态 N(0,1)
    #[inline]
    fn next_gaussian(&mut self) -> f32 {
        let u1 = self.next_f32_01().max(1e-12);
        let u2 = self.next_f32_01();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    }
}

// ════════════════════════════════════════════════════════
//  纯高斯球面向量生成（核心区别：无低秩、无聚簇）
// ════════════════════════════════════════════════════════

fn gen_sphere_vectors(path: &str, n: usize, dim: usize, seed: u64) {
    let mut rng = Rng::new(seed);
    let mut file = std::io::BufWriter::with_capacity(
        8 * 1024 * 1024,
        std::fs::File::create(path).unwrap_or_else(|e| panic!("无法创建 {}: {}", path, e)),
    );

    let chunk_size = 10_000;
    let mut written = 0;

    while written < n {
        let batch = chunk_size.min(n - written);
        let mut buf = Vec::with_capacity(batch * dim * 4);

        for _ in 0..batch {
            // 生成 768 维高斯向量
            let mut vec_d = vec![0.0f32; dim];
            for x in vec_d.iter_mut() {
                *x = rng.next_gaussian();
            }

            // L2 归一化 → 均匀球面
            let norm = vec_d.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
            for x in vec_d.iter_mut() {
                *x /= norm;
            }

            for &x in &vec_d {
                buf.extend_from_slice(&x.to_le_bytes());
            }
        }

        file.write_all(&buf).unwrap();
        written += batch;

        if written % 100_000 == 0 {
            print!("  已写入 {}K 条...\r", written / 1000);
        }
    }

    file.flush().unwrap();
    println!("  写入完成: {} ({} 条 x {} 维)", path, n, dim);
}

// ════════════════════════════════════════════════════════
//  Ground Truth
// ════════════════════════════════════════════════════════

fn read_f32_bin(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("无法读取 {}: {}", path, e));
    assert_eq!(bytes.len() % 4, 0);
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

#[inline(always)]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

fn exact_topk(train: &[f32], query: &[f32], dim: usize, top_k: usize) -> Vec<i32> {
    let n = train.len() / dim;
    let mut best: Vec<(i32, f32)> = Vec::with_capacity(top_k + 1);

    for i in 0..n {
        let base = &train[i * dim..(i + 1) * dim];
        let sim = dot(query, base);
        if best.len() < top_k {
            best.push((i as i32, sim));
            if best.len() == top_k {
                best.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            }
        } else if sim > best[top_k - 1].1 {
            best[top_k - 1] = (i as i32, sim);
            let mut j = top_k - 1;
            while j > 0 && best[j].1 > best[j - 1].1 {
                best.swap(j, j - 1);
                j -= 1;
            }
        }
    }
    best.into_iter().map(|(id, _)| id).collect()
}

fn main() {
    println!("=== Random-Sphere 数据集生成器 ===");
    println!("维度: {}, 训练集: {}, 测试集: {}", DIM, N_TRAIN, N_TEST);
    println!("特点: 纯高斯球面，无低秩结构，无聚簇");
    println!();

    // 阶段 1：生成训练集
    println!("[阶段 1/3] 生成训练集（纯高斯球面）...");
    let t0 = Instant::now();
    gen_sphere_vectors("sphere_train.f32", N_TRAIN, DIM, SEED);
    println!("  耗时: {:.2}s\n", t0.elapsed().as_secs_f64());

    // 阶段 2：生成测试集
    println!("[阶段 2/3] 生成测试集...");
    gen_sphere_vectors("sphere_test.f32", N_TEST, DIM, SEED.wrapping_add(99999));

    // 阶段 3：计算 GroundTruth
    println!("\n[阶段 3/3] 计算精确 GroundTruth Top-{}...", TOP_K);
    let t_gt = Instant::now();
    let train_data = read_f32_bin("sphere_train.f32");
    let test_data = read_f32_bin("sphere_test.f32");
    println!(
        "  训练集: {} 条, 测试集: {} 条",
        train_data.len() / DIM,
        test_data.len() / DIM
    );

    // 验证分布：纯球面的两两余弦应该很接近 0
    {
        let mut rng = Rng::new(777);
        let mut sims = Vec::with_capacity(2000);
        for _ in 0..2000 {
            let i = (rng.next_u64() as usize) % N_TRAIN;
            let j = (rng.next_u64() as usize) % N_TRAIN;
            if i != j {
                let a = &train_data[i * DIM..(i + 1) * DIM];
                let b = &train_data[j * DIM..(j + 1) * DIM];
                sims.push(dot(a, b));
            }
        }
        sims.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        let avg = sims.iter().copied().sum::<f32>() / sims.len() as f32;
        println!(
            "  两两余弦相似度: min={:.4}, p5={:.4}, avg={:.4}, p95={:.4}, max={:.4}",
            sims[0],
            sims[sims.len() / 20],
            avg,
            sims[sims.len() * 19 / 20],
            sims[sims.len() - 1],
        );
        println!("  （纯球面 768 维，avg 应接近 0，std 应约 1/sqrt(768)≈0.036）");
    }

    println!("  并行计算 GroundTruth...");
    let gt: Vec<Vec<i32>> = (0..N_TEST)
        .into_par_iter()
        .map(|i| {
            let q = &test_data[i * DIM..(i + 1) * DIM];
            exact_topk(&train_data, q, DIM, TOP_K)
        })
        .collect();

    drop(train_data);
    drop(test_data);

    let mut gt_file = std::io::BufWriter::new(
        std::fs::File::create("sphere_groundtruth.i32").unwrap(),
    );
    for row in &gt {
        for &id in row {
            gt_file.write_all(&id.to_le_bytes()).unwrap();
        }
    }
    gt_file.flush().unwrap();

    println!("  GroundTruth 写入完成! 耗时: {:.2}s", t_gt.elapsed().as_secs_f64());

    println!("\n=== 数据生成完毕！===");
    println!("文件列表:");
    println!("  sphere_train.f32       (~2.9 GB)");
    println!("  sphere_test.f32        (~3 MB)");
    println!("  sphere_groundtruth.i32 (~40 KB)");
    println!("\n运行 QuIVer 基准测试:");
    println!("  $env:TRIVIUM_ANN_NAME=\"sphere-1m\"");
    println!("  cargo bench --bench bench_sensitivity");
}
