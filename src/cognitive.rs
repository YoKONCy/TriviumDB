//! 检索增强认知数学算子
//!
//! 提供基于纯 Rust 手写的核心认知搜索数学库：
//! - FISTA (Fast Iterative Shrinkage-Thresholding Algorithm): 稀疏编码残差发现
//! - DPP (Determinantal Point Process): 贪心多样性行列式采样
//! - NMF (Non-negative Matrix Factorization): 语义矩阵分解分析
//!
//! 在 TriviumDB 中完全采用零依赖纯手写，依赖底层 SIMD 进行矩阵加速计算，无缝支持向量管线的深度认知干预。

// ============================================================================
// 辅助函数：SIMD 友好的向量/矩阵运算
// ============================================================================

/// 点积 (利用 Rust 编译器自动向量化)
#[inline]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    a.iter().zip(b.iter()).map(|(&x, &y)| x * y).sum()
}

/// L2 范数
#[inline]
pub fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|&x| x * x).sum::<f32>().sqrt()
}

/// 向量减法: a - b
#[inline]
pub fn vec_sub(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b.iter()).map(|(&x, &y)| x - y).collect()
}

/// 向量加法: a + b
#[inline]
pub fn vec_add(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b.iter()).map(|(&x, &y)| x + y).collect()
}

/// 标量乘法: s * v
#[inline]
pub fn vec_scale(v: &[f32], s: f32) -> Vec<f32> {
    v.iter().map(|&x| x * s).collect()
}

/// 矩阵-向量乘法: M (m×m) @ v (m,) -> (m,)
/// M 以行优先 flat array 存储
#[inline]
pub fn mat_vec_mul(m_flat: &[f32], rows: usize, cols: usize, v: &[f32]) -> Vec<f32> {
    debug_assert_eq!(m_flat.len(), rows * cols);
    debug_assert_eq!(v.len(), cols);
    (0..rows)
        .map(|i| {
            let row = &m_flat[i * cols..(i + 1) * cols];
            dot(row, v)
        })
        .collect()
}

/// soft threshold (近端算子 for L1): sign(x) * max(|x| - t, 0)
#[inline]
pub fn soft_threshold(x: &[f32], threshold: f32) -> Vec<f32> {
    x.iter()
        .map(|&v| {
            let abs_v = v.abs();
            if abs_v > threshold {
                v.signum() * (abs_v - threshold)
            } else {
                0.0
            }
        })
        .collect()
}

// ============================================================================
// FISTA 稀疏编码残差 (Beck & Teboulle, 2009)
// ============================================================================

/// FISTA 求解 LASSO: min_α ½‖q - Eᵀα‖² + λ‖α‖₁
///
/// 用于在已经检索到的锚点向量池中，通过稀疏组合逼近 Query。
/// 其未能逼近的部分即为“潜特征残差”。将残差再次投入数据库查询，可能发现弱关联记忆。
///
/// 返回 (alpha, residual_vec, residual_norm)
pub fn fista_solve(
    query: &[f32],         // (d,)
    entities: &[Vec<f32>], // m 个 (d,) 向量
    lambda: f32,
    max_iter: usize,
) -> (Vec<f32>, Vec<f32>, f32) {
    let m = entities.len();
    let d = query.len();

    if m == 0 {
        return (vec![], query.to_vec(), l2_norm(query));
    }

    // 预计算 Gram 矩阵 EtE (m×m) — 密集矩阵, SIMD 友好
    let mut ete = vec![0.0f32; m * m];
    for i in 0..m {
        for j in i..m {
            let val = dot(&entities[i], &entities[j]);
            ete[i * m + j] = val;
            ete[j * m + i] = val; // 对称
        }
    }

    // 预计算 Etq (m,) = E @ q
    let etq: Vec<f32> = entities.iter().map(|e| dot(e, query)).collect();

    // Lipschitz 常数 = ‖EᵀE‖₂ (谱范数上界)
    // 快速近似: Gershgorin 圆盘定理上界 = max_i Σ_j |EtE[i,j]|
    let mut lip = 0.0f32;
    for i in 0..m {
        let row_sum: f32 = (0..m).map(|j| ete[i * m + j].abs()).sum();
        if row_sum > lip {
            lip = row_sum;
        }
    }
    if lip < 1e-10 {
        return (vec![0.0; m], query.to_vec(), l2_norm(query));
    }

    let step = 1.0 / lip;

    // FISTA 主循环
    let mut alpha = vec![0.0f32; m];
    let mut y = vec![0.0f32; m];
    let mut t = 1.0f32;

    for _ in 0..max_iter {
        // grad = EtE @ y - Etq
        let ete_y = mat_vec_mul(&ete, m, m, &y);
        let grad = vec_sub(&ete_y, &etq);

        // 近端步: soft_threshold(y - step * grad, lambda * step)
        let step_grad = vec_scale(&grad, step);
        let y_minus_sg = vec_sub(&y, &step_grad);
        let alpha_new = soft_threshold(&y_minus_sg, lambda * step);

        // FISTA 动量更新加速
        let t_new = (1.0 + (1.0 + 4.0 * t * t).sqrt()) / 2.0;
        let momentum = (t - 1.0) / t_new;

        // y = alpha_new + momentum * (alpha_new - alpha)
        let diff = vec_sub(&alpha_new, &alpha);
        let scaled_diff = vec_scale(&diff, momentum);
        y = vec_add(&alpha_new, &scaled_diff);

        alpha = alpha_new;
        t = t_new;
    }

    // 残差: r = q - Eᵀ @ α
    // Eᵀ @ α = Σ_i α_i * e_i
    let mut reconstruction = vec![0.0f32; d];
    for (i, &a) in alpha.iter().enumerate() {
        if a.abs() > 1e-10 {
            for (j, &e_val) in entities[i].iter().enumerate() {
                reconstruction[j] += a * e_val;
            }
        }
    }

    let residual = vec_sub(query, &reconstruction);
    let residual_norm = l2_norm(&residual);

    (alpha, residual, residual_norm)
}

// ============================================================================
// DPP 贪心多样性采样 (Kulesza & Taskar, 2012)
// ============================================================================

/// 贪心 DPP: 增量 Cholesky 分解行列式采样算法
///
/// 目的是在 `candidates` 中选取不重复的 `k` 个样本。兼顾个体的得分 `scores`，
/// 以及它们在向量空间彼此之间的多样性（互相距离越远越好）。
pub fn dpp_greedy(
    vecs: &[Vec<f32>], // (N, d)
    scores: &[f32],    // (N,)
    k: usize,
    quality_weight: f32,
) -> Vec<usize> {
    let n = scores.len();
    if n <= k {
        return (0..n).collect();
    }

    // L2 归一化，用于计算余弦相似度核
    let normed: Vec<Vec<f32>> = vecs
        .iter()
        .map(|v| {
            let norm = l2_norm(v).max(1e-10);
            let inv = 1.0 / norm;
            v.iter().map(|&x| x * inv).collect()
        })
        .collect();

    // 质量因子 q_i = score_i ^ quality_weight
    let q: Vec<f32> = scores
        .iter()
        .map(|&s| s.max(1e-10).powf(quality_weight))
        .collect();

    // d[i] = 当前条件方差 (初始 = L_ii = q_i^2 + 1e-8 微小正则防止坍缩)
    let mut diag: Vec<f32> = q.iter().map(|&qi| qi * qi + 1e-8).collect();

    // Cholesky 增量列: O(nk^2)
    let mut c = vec![vec![0.0f32; n]; k];

    let mut selected = Vec::with_capacity(k);

    for j in 0..k {
        // 找条件方差 d[i] 最大的（即边际极大化行列式的）
        let mut best = 0usize;
        let mut best_val = f32::NEG_INFINITY;
        for i in 0..n {
            if diag[i] > best_val && !selected.contains(&i) {
                best_val = diag[i];
                best = i;
            }
        }

        selected.push(best);

        if j == k - 1 {
            break;
        }

        // 已经无法增加行列式总值（池子里剩下的全都是高度相似的冗余内容）
        if diag[best] < 1e-10 {
            break;
        }

        // 惰性求值构建 L 矩阵
        let l_best_row: Vec<f32> = (0..n)
            .map(|i| q[best] * dot(&normed[best], &normed[i]) * q[i])
            .collect();

        // 增量消元: c[j, :] = L[best, :] - Σ_{i<j} c[i, best] * c[i, :]
        let mut cj = l_best_row;
        for i in 0..j {
            let c_i_best = c[i][best];
            for idx in 0..n {
                cj[idx] -= c_i_best * c[i][idx];
            }
        }

        let inv_sqrt = 1.0 / diag[best].sqrt();
        for idx in 0..n {
            cj[idx] *= inv_sqrt;
        }

        // 更新剩余候选的条件方差: d -= c[j]^2
        for i in 0..n {
            diag[i] -= cj[i] * cj[i];
            if diag[i] < 0.0 {
                diag[i] = 0.0;
            }
        }

        c[j] = cj;
    }

    selected
}

// ============================================================================
// NMF 非负矩阵分解 (Lee & Seung, 1999)
// ============================================================================

/// NMF 乘法更新算法: V ≈ W × H
///
/// 将大量实体嵌入矩阵分解，提炼出隐特征。
/// * V: (m, d), W: (m, k), H: (k, d)
///
/// 返回: (W, H) 行优先平坦组合
pub fn nmf_multiplicative_update(
    v_flat: &[f32],
    m: usize,
    d: usize,
    k: usize,
    max_iter: usize,
    tol: f32,
) -> (Vec<f32>, Vec<f32>) {
    let eps = 1e-10f32;

    // 绝对值截断保证非负
    let mut v_abs = vec![0.0f32; m * d];
    for (i, &val) in v_flat.iter().enumerate() {
        v_abs[i] = val.abs();
    }

    let v_mean = v_abs.iter().sum::<f32>() / (m * d) as f32;
    let avg = (v_mean / k as f32).sqrt().max(0.01);

    // 极简实现的伪随机数，剥离 rand 依赖
    let mut seed: u64 = 42;
    let mut next_rand = || -> f32 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let val = ((seed >> 33) as f32) / (u32::MAX as f32);
        (avg + avg * 0.5 * (val - 0.5)).abs() + eps
    };

    let mut w: Vec<f32> = (0..m * k).map(|_| next_rand()).collect();
    let mut h: Vec<f32> = (0..k * d).map(|_| next_rand()).collect();

    for iter in 0..max_iter {
        // --- 1. 更新隐特征 H ---
        let mut wtv = vec![0.0f32; k * d];
        for ki in 0..k {
            for di in 0..d {
                let mut sum = 0.0f32;
                for mi in 0..m {
                    sum += w[mi * k + ki] * v_abs[mi * d + di];
                }
                wtv[ki * d + di] = sum;
            }
        }

        let mut wtw = vec![0.0f32; k * k];
        for i in 0..k {
            for j in i..k {
                let mut sum = 0.0f32;
                for mi in 0..m {
                    sum += w[mi * k + i] * w[mi * k + j];
                }
                wtw[i * k + j] = sum;
                wtw[j * k + i] = sum;
            }
        }

        let mut wtwh = vec![0.0f32; k * d];
        for i in 0..k {
            for di in 0..d {
                let mut sum = 0.0f32;
                for j in 0..k {
                    sum += wtw[i * k + j] * h[j * d + di];
                }
                wtwh[i * d + di] = sum;
            }
        }

        // H 规则迭代
        for i in 0..k * d {
            h[i] *= wtv[i] / (wtwh[i] + eps);
        }

        // --- 2. 更新权重基 W ---
        let mut vht = vec![0.0f32; m * k];
        for mi in 0..m {
            for ki in 0..k {
                let mut sum = 0.0f32;
                for di in 0..d {
                    sum += v_abs[mi * d + di] * h[ki * d + di];
                }
                vht[mi * k + ki] = sum;
            }
        }

        let mut hht = vec![0.0f32; k * k];
        for i in 0..k {
            for j in i..k {
                let mut sum = 0.0f32;
                for di in 0..d {
                    sum += h[i * d + di] * h[j * d + di];
                }
                hht[i * k + j] = sum;
                hht[j * k + i] = sum;
            }
        }

        let mut whht = vec![0.0f32; m * k];
        for mi in 0..m {
            for ki in 0..k {
                let mut sum = 0.0f32;
                for j in 0..k {
                    sum += w[mi * k + j] * hht[j * k + ki];
                }
                whht[mi * k + ki] = sum;
            }
        }

        // W 规则迭代
        for i in 0..m * k {
            w[i] *= vht[i] / (whht[i] + eps);
        }

        // 收敛残差
        if iter > 0 && iter % 10 == 0 {
            let mut res_sq = 0.0f32;
            let mut v_sq = 0.0f32;
            for mi in 0..m {
                for di in 0..d {
                    let mut wh = 0.0f32;
                    for ki in 0..k {
                        wh += w[mi * k + ki] * h[ki * d + di];
                    }
                    let diff = v_abs[mi * d + di] - wh;
                    res_sq += diff * diff;
                    v_sq += v_abs[mi * d + di] * v_abs[mi * d + di];
                }
            }
            if v_sq > 0.0 && (res_sq / v_sq).sqrt() < tol {
                break;
            }
        }
    }

    (w, h)
}

/// NMF 查询深度分析：评估原意图中的不均衡性与新领域
///
/// 返回: (semantic_depth, topic_coverage, novelty, top_topics_distribution)
pub fn nmf_analyze_query(
    query: &[f32],  // (d,)
    h_flat: &[f32], // (k, d)
    k: usize,
    d: usize,
) -> (f32, usize, f32, Vec<f32>) {
    let q_abs: Vec<f32> = query.iter().map(|&x| x.abs()).collect();

    // 投影主题 q @ H.T -> (k,)
    let mut raw_scores = vec![0.0f32; k];
    for ki in 0..k {
        raw_scores[ki] = dot(&q_abs, &h_flat[ki * d..(ki + 1) * d]);
    }

    // 主题权重 Softmax 操作
    let max_s = raw_scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exp_scores: Vec<f32> = raw_scores.iter().map(|&s| (s - max_s).exp()).collect();
    let sum_exp: f32 = exp_scores.iter().sum();
    let q_topics: Vec<f32> = exp_scores.iter().map(|&e| e / (sum_exp + 1e-10)).collect();

    // 1. 语义聚焦深度 = 1 - 归一化信息熵 (完全均匀发布意味着毫无深度)
    let entropy: f32 = q_topics
        .iter()
        .map(|&p| if p > 1e-10 { -p * p.ln() } else { 0.0 })
        .sum();
    let max_entropy = if k > 1 { (k as f32).ln() } else { 1.0 };
    let semantic_depth = 1.0 - entropy / max_entropy;

    // 2. 覆盖话题广度
    let threshold = 0.5 / k as f32;
    let topic_coverage = q_topics.iter().filter(|&&p| p > threshold).count();

    // 3. 意图新颖脱轨程度: 当 H 矩阵重构不出 Query 的真实全貌，即代表有未被索引的“新颖逻辑”
    let mut q_recon = vec![0.0f32; d];
    for ki in 0..k {
        let w = q_topics[ki];
        if w > 1e-10 {
            for di in 0..d {
                q_recon[di] += w * h_flat[ki * d + di];
            }
        }
    }
    let diff = vec_sub(&q_abs, &q_recon);
    let q_norm = l2_norm(&q_abs).max(1e-10);
    // 约束到 0..=1 内部
    let novelty = (l2_norm(&diff) / q_norm).min(1.0);

    (semantic_depth, topic_coverage, novelty, q_topics)
}

// ============================================================================
// 纯数学白盒测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fista_basic() {
        let entities = vec![vec![1.0, 0.0, 0.0, 0.0], vec![0.0, 1.0, 0.0, 0.0]];
        let query = vec![0.8, 0.6, 0.0, 0.0];

        let (alpha, _residual, norm) = fista_solve(&query, &entities, 0.01, 100);

        assert!((alpha[0] - 0.79).abs() < 0.1, "alpha[0]={}", alpha[0]);
        assert!((alpha[1] - 0.59).abs() < 0.1, "alpha[1]={}", alpha[1]);
        assert!(norm < 0.1, "residual_norm={}", norm);
    }

    #[test]
    fn test_dpp_diversity() {
        let vecs = vec![
            vec![1.0, 0.0],  // Dir 1
            vec![0.99, 0.1], // Similar to Dir 1
            vec![0.0, 1.0],  // Dir 2
        ];
        // Element 1 is objectively the best, but elements 0 and 1 are too similar.
        let scores = vec![1.0, 0.9, 0.8];

        let selected = dpp_greedy(&vecs, &scores, 2, 1.0);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0], 0); // Takes the highest score
        assert_eq!(selected[1], 2); // Takes the diverse one over the 0.9 score
    }

    #[test]
    fn test_nmf() {
        let v = vec![1.0, 0.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.5, 0.5, 0.5, 0.5, 0.5];
        let (w, h) = nmf_multiplicative_update(&v, 3, 4, 2, 100, 1e-3);
        assert_eq!(w.len(), 3 * 2);
        assert_eq!(h.len(), 2 * 4);
    }
}
