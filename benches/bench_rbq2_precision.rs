//! BQ2 vs RaBitQ 1-bit 公平对比
//!
//! 对比三种方案的 top-K 排序精度:
//! 1. BQ2: 我们的 2-bit sign-magnitude (无旋转, 加权距离)
//! 2. RaBitQ-sym: 4轮旋转 + 1-bit sign + 纯 hamming (对称)
//! 3. RaBitQ-asym: 4轮旋转 + 1-bit sign + f32 query 非对称点积 (RaBitQ 核心)
//!
//! 用法: cargo bench --bench bench_rbq2_precision

use rayon::prelude::*;
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::time::Instant;

const DIM: usize = 768;
const N_TRAIN: usize = 100_000;
const N_QUERIES: usize = 200;

fn read_f32_bin(path: &str) -> Vec<f32> {
    let mut file = File::open(path).unwrap_or_else(|_| panic!("无法打开 {}", path));
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    bytes.chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

// ═══ FHT-Kac 旋转 (同 v2) ═══

struct FhtKacRotator {
    flips: [Vec<u8>; 4],
    trunc_dim: usize,
    padded_dim: usize,
    fac: f32,
}

impl FhtKacRotator {
    fn new(dim: usize, seed: u64) -> Self {
        let padded_dim = (dim + 63) & !63;
        let log2 = (usize::BITS - 1) - dim.leading_zeros();
        let trunc_dim = 1usize << log2;
        let fac = 1.0 / (trunc_dim as f32).sqrt();

        let mut state = seed;
        let bytes_per_flip = padded_dim / 8;
        let mut flips = [
            vec![0u8; bytes_per_flip], vec![0u8; bytes_per_flip],
            vec![0u8; bytes_per_flip], vec![0u8; bytes_per_flip],
        ];
        for flip in &mut flips {
            for byte in flip.iter_mut() {
                state ^= state << 13; state ^= state >> 7; state ^= state << 17;
                *byte = state as u8;
            }
        }
        Self { flips, trunc_dim, padded_dim, fac }
    }

    fn rotate(&self, src: &[f32]) -> Vec<f32> {
        let mut data = vec![0.0f32; self.padded_dim];
        let copy_len = src.len().min(self.padded_dim);
        data[..copy_len].copy_from_slice(&src[..copy_len]);

        let start = self.padded_dim - self.trunc_dim;

        if self.trunc_dim == self.padded_dim {
            for round in 0..4 {
                flip_sign(&self.flips[round], &mut data);
                fht_in_place(&mut data[..self.trunc_dim]);
                vec_rescale(&mut data[..self.trunc_dim], self.fac);
            }
        } else {
            flip_sign(&self.flips[0], &mut data);
            fht_in_place(&mut data[..self.trunc_dim]);
            vec_rescale(&mut data[..self.trunc_dim], self.fac);
            kacs_walk(&mut data);

            flip_sign(&self.flips[1], &mut data);
            fht_in_place(&mut data[start..start + self.trunc_dim]);
            vec_rescale(&mut data[start..start + self.trunc_dim], self.fac);
            kacs_walk(&mut data);

            flip_sign(&self.flips[2], &mut data);
            fht_in_place(&mut data[..self.trunc_dim]);
            vec_rescale(&mut data[..self.trunc_dim], self.fac);
            kacs_walk(&mut data);

            flip_sign(&self.flips[3], &mut data);
            fht_in_place(&mut data[start..start + self.trunc_dim]);
            vec_rescale(&mut data[start..start + self.trunc_dim], self.fac);
            kacs_walk(&mut data);

            vec_rescale(&mut data, 0.25);
        }
        data
    }
}

fn flip_sign(flip: &[u8], data: &mut [f32]) {
    for (byte_idx, &byte) in flip.iter().enumerate() {
        if byte == 0 { continue; }
        for bit in 0..8 {
            let i = byte_idx * 8 + bit;
            if i < data.len() && (byte >> bit) & 1 != 0 {
                data[i] = -data[i];
            }
        }
    }
}

fn fht_in_place(x: &mut [f32]) {
    let n = x.len();
    let mut h = 1;
    while h < n {
        for i in (0..n).step_by(h * 2) {
            for j in i..i + h {
                let a = x[j];
                let b = x[j + h];
                x[j] = a + b;
                x[j + h] = a - b;
            }
        }
        h *= 2;
    }
}

fn vec_rescale(data: &mut [f32], fac: f32) {
    for v in data.iter_mut() { *v *= fac; }
}

fn kacs_walk(data: &mut [f32]) {
    let half = data.len() / 2;
    for i in 0..half {
        let a = data[i];
        let b = data[i + half];
        data[i] = a + b;
        data[i + half] = a - b;
    }
}

// ═══ 方案 1: BQ2 (我们的 2-bit sign-magnitude) ═══

struct Bq2Sig { pos: Vec<u64>, strong: Vec<u64> }

fn encode_bq2(x: &[f32], dim: usize) -> Bq2Sig {
    let chunks = dim.div_ceil(64);
    let sum_abs = x.iter().take(dim).map(|v| v.abs()).sum::<f32>();
    let alpha = sum_abs / dim as f32;

    let mut pos = vec![0u64; chunks];
    let mut strong = vec![0u64; chunks];
    for (i, &v) in x.iter().enumerate().take(dim) {
        let c = i / 64;
        let b = i % 64;
        if v > 0.0 { pos[c] |= 1u64 << b; }
        if v.abs() > alpha { strong[c] |= 1u64 << b; }
    }
    Bq2Sig { pos, strong }
}

fn bq2_distance(a: &Bq2Sig, b: &Bq2Sig, dim: usize) -> i32 {
    let chunks = dim.div_ceil(64);
    let valid_last = if dim.is_multiple_of(64) { !0u64 } else { (1u64 << (dim % 64)) - 1 };
    let mut dot = 0i32;
    for i in 0..chunks {
        let mask = if i == chunks - 1 { valid_last } else { !0u64 };
        let same = !(a.pos[i] ^ b.pos[i]) & mask;
        let diff = (a.pos[i] ^ b.pos[i]) & mask;
        let both_s = a.strong[i] & b.strong[i] & mask;
        let one_s = (a.strong[i] ^ b.strong[i]) & mask;
        let both_w = !(a.strong[i] | b.strong[i]) & mask;

        dot += 4 * (same & both_s).count_ones() as i32;
        dot -= 4 * (diff & both_s).count_ones() as i32;
        dot += 2 * (same & one_s).count_ones() as i32;
        dot -= 2 * (diff & one_s).count_ones() as i32;
        dot += (same & both_w).count_ones() as i32;
        dot -= (diff & both_w).count_ones() as i32;
    }
    -dot
}

// ═══ 方案 2: RaBitQ 1-bit 对称 (旋转后纯 hamming) ═══

struct Bit1Sig { bits: Vec<u64> }

fn encode_1bit(x: &[f32], dim: usize) -> Bit1Sig {
    let chunks = dim.div_ceil(64);
    let mut bits = vec![0u64; chunks];
    for i in 0..dim {
        if x[i] > 0.0 {
            bits[i / 64] |= 1u64 << (i % 64);
        }
    }
    Bit1Sig { bits }
}

fn hamming_distance(a: &Bit1Sig, b: &Bit1Sig) -> u32 {
    a.bits.iter().zip(b.bits.iter())
        .map(|(x, y)| (x ^ y).count_ones())
        .sum()
}

// ═══ 方案 3: RaBitQ 1-bit 非对称 (f32 query × binary DB) ═══
//
// 核心: dot(q_rot_f32, binary_as_pm1)
//     = Σ q_rot[i] * (2*bit[i] - 1)
//     = 2 * Σ(q_rot[i] where bit=1) - Σ(q_rot[i])
//
// 加上每向量修正因子:
//   score = f_add + f_rescale * raw_dot
//   f_rescale = -||x||² / dot(x, xu_cb)
//   其中 xu_cb[i] = sign(x[i])*0.5

struct RaBitQSig {
    bits: Vec<u64>,
    f_add: f32,
    f_rescale: f32,
}

fn encode_rabitq(x_rotated: &[f32], dim: usize) -> RaBitQSig {
    let chunks = dim.div_ceil(64);
    let mut bits = vec![0u64; chunks];

    let mut l2_sqr = 0.0f32;
    let mut ip_resi_xucb = 0.0f32;

    for i in 0..dim {
        let v = x_rotated[i];
        if v > 0.0 {
            bits[i / 64] |= 1u64 << (i % 64);
        }
        l2_sqr += v * v;
        // xu_cb[i] = sign(v) * 0.5, 所以 v * xu_cb[i] = |v| * 0.5
        ip_resi_xucb += v.abs() * 0.5;
    }

    // centroid=0 时: f_add = 1, f_rescale = -l2_sqr / ip_resi_xucb (IP 度量)
    let f_add = 1.0;
    let f_rescale = if ip_resi_xucb.abs() > 1e-12 {
        -l2_sqr / ip_resi_xucb
    } else {
        0.0
    };

    RaBitQSig { bits, f_add, f_rescale }
}

/// 非对称距离: f32 query × binary DB
fn rabitq_asym_distance(q_rot: &[f32], db: &RaBitQSig, dim: usize) -> f32 {
    // raw_dot = Σ q_rot[i] * xu_cb[i]
    // xu_cb[i] = bit[i] ? 0.5 : -0.5
    let mut raw_dot = 0.0f32;
    for (i, &qv) in q_rot.iter().enumerate().take(dim) {
        let bit = (db.bits[i / 64] >> (i % 64)) & 1;
        let xu_cb = if bit == 1 { 0.5f32 } else { -0.5f32 };
        raw_dot += qv * xu_cb;
    }
    // 越小越近（IP 度量下 estimated distance = 1 - IP）
    db.f_add + db.f_rescale * raw_dot
}

// ═══ 工具函数 ═══

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..a.len() {
        dot += a[i] * b[i]; na += a[i] * a[i]; nb += b[i] * b[i];
    }
    dot / (na.sqrt() * nb.sqrt()).max(1e-12)
}

fn topk_f32_desc(scores: &[f32], k: usize) -> Vec<usize> {
    let mut v: Vec<(usize, f32)> = scores.iter().enumerate().map(|(i, &s)| (i, s)).collect();
    v.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    v.truncate(k);
    v.into_iter().map(|(i, _)| i).collect()
}

fn topk_i32_asc(dists: &[i32], k: usize) -> Vec<usize> {
    let mut v: Vec<(usize, i32)> = dists.iter().enumerate().map(|(i, &d)| (i, d)).collect();
    v.sort_unstable_by_key(|x| x.1);
    v.truncate(k);
    v.into_iter().map(|(i, _)| i).collect()
}

fn topk_u32_asc(dists: &[u32], k: usize) -> Vec<usize> {
    let mut v: Vec<(usize, u32)> = dists.iter().enumerate().map(|(i, &d)| (i, d)).collect();
    v.sort_unstable_by_key(|x| x.1);
    v.truncate(k);
    v.into_iter().map(|(i, _)| i).collect()
}

fn topk_f32_asc(dists: &[f32], k: usize) -> Vec<usize> {
    let mut v: Vec<(usize, f32)> = dists.iter().enumerate().map(|(i, &d)| (i, d)).collect();
    v.sort_unstable_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    v.truncate(k);
    v.into_iter().map(|(i, _)| i).collect()
}

fn overlap(a: &[usize], b: &[usize]) -> usize {
    let set: HashSet<usize> = a.iter().cloned().collect();
    b.iter().filter(|x| set.contains(x)).count()
}

// ═══ 主函数 ═══

fn main() {
    println!("======================================================================");
    println!("BQ2 vs RaBitQ 1-bit 公平对比");
    println!("======================================================================\n");

    let t0 = Instant::now();
    let raw_data = read_f32_bin("cohere_train.f32");
    let raw_queries = read_f32_bin("cohere_test.f32");
    let n_train = N_TRAIN.min(raw_data.len() / DIM);
    let n_queries = N_QUERIES.min(raw_queries.len() / DIM);
    println!("加载 {} 训练 {} 查询, 耗时 {:.2}s\n", n_train, n_queries, t0.elapsed().as_secs_f64());

    let rotator = FhtKacRotator::new(DIM, 42);
    let padded = rotator.padded_dim;

    // ─── 编码所有方案 ───
    println!("编码中...");
    let t = Instant::now();

    // 方案 1: BQ2
    let bq2_sigs: Vec<Bq2Sig> = (0..n_train).into_par_iter()
        .map(|i| encode_bq2(&raw_data[i*DIM..(i+1)*DIM], DIM)).collect();

    // 方案 2 & 3: 先旋转
    let rotated: Vec<Vec<f32>> = (0..n_train).into_par_iter()
        .map(|i| rotator.rotate(&raw_data[i*DIM..(i+1)*DIM])).collect();

    // 方案 2: 1-bit 对称
    let bit1_sigs: Vec<Bit1Sig> = rotated.par_iter()
        .map(|v| encode_1bit(v, padded)).collect();

    // 方案 3: RaBitQ 非对称 (带修正因子)
    let rabitq_sigs: Vec<RaBitQSig> = rotated.par_iter()
        .map(|v| encode_rabitq(v, padded)).collect();

    println!("  编码完成: {:.3}s\n", t.elapsed().as_secs_f64());

    // ─── 测试 ───
    let k_values = [1, 10, 100, 500];

    print!("{:<6}", "K");
    print!("  {:>12}", "BQ2(2bit)");
    print!("  {:>12}", "RBQ-sym");
    print!("  {:>12}", "RBQ-asym");
    println!();
    println!("{}", "-".repeat(48));

    for &k in &k_values {
        let t_k = Instant::now();

        let results: Vec<(f64, f64, f64)> = (0..n_queries)
            .into_par_iter()
            .map(|qi| {
                let q = &raw_queries[qi*DIM..(qi+1)*DIM];

                // f32 真值
                let f32_scores: Vec<f32> = (0..n_train)
                    .map(|i| cosine_sim(q, &raw_data[i*DIM..(i+1)*DIM]))
                    .collect();
                let gt = topk_f32_desc(&f32_scores, k);

                // 方案 1: BQ2
                let q_bq2 = encode_bq2(q, DIM);
                let bq2_d: Vec<i32> = (0..n_train)
                    .map(|i| bq2_distance(&q_bq2, &bq2_sigs[i], DIM)).collect();
                let bq2_top = topk_i32_asc(&bq2_d, k);

                // 方案 2: RaBitQ 1-bit 对称
                let q_rot = rotator.rotate(q);
                let q_1bit = encode_1bit(&q_rot, padded);
                let sym_d: Vec<u32> = (0..n_train)
                    .map(|i| hamming_distance(&q_1bit, &bit1_sigs[i])).collect();
                let sym_top = topk_u32_asc(&sym_d, k);

                // 方案 3: RaBitQ 1-bit 非对称
                let asym_d: Vec<f32> = (0..n_train)
                    .map(|i| rabitq_asym_distance(&q_rot, &rabitq_sigs[i], padded)).collect();
                let asym_top = topk_f32_asc(&asym_d, k);

                (
                    overlap(&bq2_top, &gt) as f64 / k as f64,
                    overlap(&sym_top, &gt) as f64 / k as f64,
                    overlap(&asym_top, &gt) as f64 / k as f64,
                )
            })
            .collect();

        let avg_bq2: f64 = results.iter().map(|r| r.0).sum::<f64>() / n_queries as f64;
        let avg_sym: f64 = results.iter().map(|r| r.1).sum::<f64>() / n_queries as f64;
        let avg_asym: f64 = results.iter().map(|r| r.2).sum::<f64>() / n_queries as f64;

        println!("{:<6}  {:>11.2}%  {:>11.2}%  {:>11.2}%    ({:.1}s)",
            k, avg_bq2*100.0, avg_sym*100.0, avg_asym*100.0, t_k.elapsed().as_secs_f64());
    }

    println!("\n说明:");
    println!("  BQ2(2bit): 我们的 2-bit sign-magnitude, 无旋转, 加权距离");
    println!("  RBQ-sym:   RaBitQ 4轮旋转 + 1-bit, 对称 hamming");
    println!("  RBQ-asym:  RaBitQ 4轮旋转 + 1-bit, f32 query 非对称 + 修正项");
    println!("======================================================================");
}
