//! 仿 LLM Embedding 随机 1M 向量数据生成器 v2
//!
//! 使用低秩子空间 + Zipf 簇分布模型，生成分布接近真实 LLM embedding 的数据。
//!
//! 核心洞察：真实 LLM embedding 活在一个低秩流形上（有效维度 ~50-100），
//! 名义上 768 维但大部分方差集中在少数主成分方向。简单的高斯混合在 768 维
//! 归一化后会被距离浓缩碾平，但低秩信号 + 微弱全秩噪声的结构能存活。
//!
//! 模型：
//!   v = W × (centroid_k + σ × gaussian_k) + ε × noise_768
//!   v = v / ||v||
//!
//!   - W ∈ R^{768×k}: 正交基底（intrinsic manifold）
//!   - k = 64: 有效维度
//!   - 256 个簇，大小服从 Zipf(s=1.2)
//!   - σ = 0.3: 簇内扰动
//!   - ε = 0.05: 全秩噪声强度
//!
//! 用法：
//!   cargo bench --bench bench_random1m
//!   $env:TRIVIUM_ANN_NAME="random-1m"
//!   cargo bench --bench bench_cohere1m
use rayon::prelude::*;
use std::io::Write;
use std::time::Instant;

const DIM: usize = 768;
const N_TRAIN: usize = 1_000_000;
const N_TEST: usize = 1_000;
const TOP_K: usize = 10;
const SEED: u64 = 42;

// ═══ 低秩子空间模型参数 ═══
const INTRINSIC_DIM: usize = 64;      // 有效维度（真实 LLM embedding ~50-100）
const N_CLUSTERS: usize = 256;        // 语义簇数量
const CLUSTER_SPREAD: f32 = 0.30;     // 簇内扰动标准差（在 k 维空间中）
const NOISE_SCALE: f32 = 0.05;        // 全秩噪声强度
const ZIPF_S: f64 = 1.2;             // Zipf 分布参数（越大分布越偏斜）

// ════════════════════════════════════════════════════════
//  xoshiro256** PRNG
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

    /// Box-Muller 变换：标准正态分布 N(0,1)
    #[inline]
    fn next_gaussian(&mut self) -> f32 {
        let u1 = self.next_f32_01().max(1e-12);
        let u2 = self.next_f32_01();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    }

    /// Zipf 采样：返回 [0, n) 中按 Zipf(s) 分布的索引
    /// 使用拒绝采样法
    fn next_zipf(&mut self, n: usize, s: f64) -> usize {
        // 预计算 CDF 归一化常数
        // 直接用拒绝法：P(k) ∝ 1/k^s
        loop {
            let u = self.next_f32_01() as f64;
            // 逆变换近似：k ≈ floor((u * H_n)^(-1/s) + 0.5)
            // 简化：直接用 u^(-1/s) 映射
            let k = (u.max(1e-12).powf(-1.0 / s)).floor() as usize;
            if k < n {
                return k;
            }
        }
    }
}

// ════════════════════════════════════════════════════════
//  正交基底生成（Modified Gram-Schmidt）
// ════════════════════════════════════════════════════════

/// 生成 k 个 d 维正交基向量，返回 d×k 的列优先矩阵
fn gen_orthogonal_basis(rng: &mut Rng, d: usize, k: usize) -> Vec<f32> {
    // basis[col * d + row] = basis_col[row]，列优先存储
    let mut basis = vec![0.0f32; d * k];

    for col in 0..k {
        // 生成随机高斯向量
        let offset = col * d;
        for row in 0..d {
            basis[offset + row] = rng.next_gaussian();
        }

        // 对前面所有列做正交化
        for prev_col in 0..col {
            let prev_offset = prev_col * d;
            // 计算内积
            let dot: f32 = (0..d)
                .map(|i| basis[offset + i] * basis[prev_offset + i])
                .sum();
            // 减去投影
            for i in 0..d {
                basis[offset + i] -= dot * basis[prev_offset + i];
            }
        }

        // 归一化
        let norm: f32 = (0..d)
            .map(|i| basis[offset + i] * basis[offset + i])
            .sum::<f32>()
            .sqrt()
            .max(1e-12);
        for i in 0..d {
            basis[offset + i] /= norm;
        }
    }

    basis
}

// ════════════════════════════════════════════════════════
//  簇中心生成（在 k 维空间中）
// ════════════════════════════════════════════════════════

fn gen_cluster_centroids_k(rng: &mut Rng, n: usize, k: usize) -> Vec<Vec<f32>> {
    let mut centroids = Vec::with_capacity(n);
    for _ in 0..n {
        let mut c = vec![0.0f32; k];
        for x in c.iter_mut() {
            *x = rng.next_gaussian();
        }
        // k 维空间中归一化
        let norm = c.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        for x in c.iter_mut() {
            *x /= norm;
        }
        centroids.push(c);
    }
    centroids
}

// ════════════════════════════════════════════════════════
//  流式向量生成：低秩子空间 + Zipf 簇 + 微弱全秩噪声
// ════════════════════════════════════════════════════════

fn gen_and_write_vectors(
    path: &str,
    n: usize,
    dim: usize,
    seed: u64,
    basis: &[f32],       // d×k 列优先正交基底
    k: usize,            // 有效维度
    centroids: &[Vec<f32>], // k 维簇中心
) {
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
            // 1) Zipf 采样选簇
            let cluster_idx = rng.next_zipf(centroids.len(), ZIPF_S);
            let centroid = &centroids[cluster_idx];

            // 2) 在 k 维空间中生成扰动点
            let mut point_k = vec![0.0f32; k];
            for (d, x) in point_k.iter_mut().enumerate() {
                *x = centroid[d] + CLUSTER_SPREAD * rng.next_gaussian();
            }

            // 3) 投影到 d 维：v = W × point_k
            let mut vec_d = vec![0.0f32; dim];
            for (col, &w) in point_k.iter().enumerate().take(k) {
                let basis_offset = col * dim;
                for row in 0..dim {
                    vec_d[row] += w * basis[basis_offset + row];
                }
            }

            // 4) 加全秩微弱噪声
            for x in vec_d.iter_mut() {
                *x += NOISE_SCALE * rng.next_gaussian();
            }

            // 5) L2 归一化
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
//  Ground Truth 计算
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
    println!("=== QuIVer 仿 LLM Embedding 数据生成器 v2 ===");
    println!("名义维度: {}, 有效维度: {}, 簇数: {}", DIM, INTRINSIC_DIM, N_CLUSTERS);
    println!("训练集: {}, 测试集: {}, Zipf s: {}", N_TRAIN, N_TEST, ZIPF_S);
    println!("簇内扰动 σ: {}, 全秩噪声 ε: {}", CLUSTER_SPREAD, NOISE_SCALE);
    println!();

    // 阶段 0：生成正交基底 W ∈ R^{768×64}（~192 KB）
    println!("[预处理] 生成 {} 维正交基底（Modified Gram-Schmidt）...", INTRINSIC_DIM);
    let t_pre = Instant::now();
    let mut meta_rng = Rng::new(SEED.wrapping_add(9999));
    let basis = gen_orthogonal_basis(&mut meta_rng, DIM, INTRINSIC_DIM);
    println!("  基底生成完成: {:.2}s", t_pre.elapsed().as_secs_f64());

    // 验证正交性
    {
        let mut max_dot = 0.0f32;
        for i in 0..INTRINSIC_DIM.min(10) {
            for j in (i + 1)..INTRINSIC_DIM.min(10) {
                let d: f32 = (0..DIM)
                    .map(|r| basis[i * DIM + r] * basis[j * DIM + r])
                    .sum();
                max_dot = max_dot.max(d.abs());
            }
        }
        println!("  正交性检验: 最大非对角内积 = {:.2e}（应 < 1e-5）", max_dot);
    }

    // 生成簇中心（在 k 维空间中）
    println!("[预处理] 生成 {} 个簇中心（{}维空间）...", N_CLUSTERS, INTRINSIC_DIM);
    let centroids = gen_cluster_centroids_k(&mut meta_rng, N_CLUSTERS, INTRINSIC_DIM);

    // 验证簇中心在 k 维空间中的分布
    {
        let mut min_sim = f32::MAX;
        let mut max_sim = f32::MIN;
        let mut sum_sim = 0.0f64;
        let mut count = 0u64;
        for i in 0..centroids.len().min(50) {
            for j in (i + 1)..centroids.len().min(50) {
                let sim: f32 = centroids[i].iter().zip(&centroids[j]).map(|(a, b)| a * b).sum();
                min_sim = min_sim.min(sim);
                max_sim = max_sim.max(sim);
                sum_sim += sim as f64;
                count += 1;
            }
        }
        println!(
            "  簇中心余弦相似度 (k维): min={:.3}, max={:.3}, avg={:.3}",
            min_sim, max_sim, sum_sim / count as f64
        );
    }

    // Zipf 分布预览
    {
        let mut counts = vec![0u32; N_CLUSTERS];
        let mut preview_rng = Rng::new(777);
        for _ in 0..100_000 {
            let idx = preview_rng.next_zipf(N_CLUSTERS, ZIPF_S);
            counts[idx] += 1;
        }
        counts.sort_unstable_by(|a, b| b.cmp(a));
        println!(
            "  Zipf 分布预览 (100K 采样): 最大簇 {}%, 前10簇 {}%, 尾部50簇 {}%",
            counts[0] as f32 / 1000.0,
            counts[..10].iter().sum::<u32>() as f32 / 1000.0,
            counts[N_CLUSTERS - 50..].iter().sum::<u32>() as f32 / 1000.0
        );
    }
    println!("  预处理总耗时: {:.2}s\n", t_pre.elapsed().as_secs_f64());

    // 阶段 1：生成训练集
    println!("[阶段 1/3] 生成训练集...");
    let t0 = Instant::now();
    gen_and_write_vectors(
        "random_train.f32", N_TRAIN, DIM, SEED,
        &basis, INTRINSIC_DIM, &centroids,
    );
    println!("  耗时: {:.2}s\n", t0.elapsed().as_secs_f64());

    // 阶段 2：生成测试集
    println!("[阶段 2/3] 生成测试集...");
    gen_and_write_vectors(
        "random_test.f32", N_TEST, DIM, SEED.wrapping_add(12345),
        &basis, INTRINSIC_DIM, &centroids,
    );

    // 释放基底和簇中心
    drop(basis);
    drop(centroids);

    // 阶段 3：计算 GroundTruth
    println!("\n[阶段 3/3] 计算精确 GroundTruth Top-{}...", TOP_K);
    println!("  加载训练集到内存...");
    let t_gt = Instant::now();
    let train_data = read_f32_bin("random_train.f32");
    let test_data = read_f32_bin("random_test.f32");
    println!(
        "  训练集: {} 条, 测试集: {} 条",
        train_data.len() / DIM,
        test_data.len() / DIM
    );

    // 验证生成质量
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
            "  两两余弦相似度: min={:.3}, p5={:.3}, avg={:.3}, p95={:.3}, max={:.3}",
            sims[0],
            sims[sims.len() / 20],
            avg,
            sims[sims.len() * 19 / 20],
            sims[sims.len() - 1],
        );
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
        std::fs::File::create("random_groundtruth.i32").unwrap(),
    );
    for row in &gt {
        for &id in row {
            gt_file.write_all(&id.to_le_bytes()).unwrap();
        }
    }
    gt_file.flush().unwrap();
    drop(gt);

    println!("  GroundTruth 写入完成! 耗时: {:.2}s", t_gt.elapsed().as_secs_f64());

    println!("\n=== 数据生成完毕！===");
    println!("文件列表:");
    println!("  random_train.f32       (~2.9 GB)");
    println!("  random_test.f32        (~3 MB)");
    println!("  random_groundtruth.i32 (~40 KB)");
    println!("\n运行 QuIVer 基准测试（只需一个环境变量！）:");
    println!("  $env:TRIVIUM_ANN_NAME=\"random-1m\"");
    println!("  cargo bench --bench bench_cohere1m");
}
