//! 从 src/cognitive.rs 分离的内联测试 + 补齐辅助函数测试
//!
//! 覆盖: dot, l2_norm, vec_sub, vec_add, vec_scale, mat_vec_mul,
//!       soft_threshold, fista_solve, dpp_greedy, nmf_multiplicative_update

use triviumdb::cognitive::*;

// ════════════════════════════════════════════════════════════════
//  辅助函数 — 此前完全无测试
// ════════════════════════════════════════════════════════════════

#[test]
fn dot_基础() {
    assert!((dot(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]) - 32.0).abs() < 1e-6);
}

#[test]
fn dot_零向量() {
    assert_eq!(dot(&[0.0, 0.0], &[1.0, 2.0]), 0.0);
}

#[test]
fn l2_norm_单位向量() {
    assert!((l2_norm(&[1.0, 0.0, 0.0]) - 1.0).abs() < 1e-6);
}

#[test]
fn l2_norm_三维() {
    // |[3,4]| = 5
    assert!((l2_norm(&[3.0, 4.0]) - 5.0).abs() < 1e-5);
}

#[test]
fn l2_norm_零向量() {
    assert_eq!(l2_norm(&[0.0, 0.0, 0.0]), 0.0);
}

#[test]
fn vec_sub_基础() {
    let result = vec_sub(&[5.0, 3.0, 1.0], &[1.0, 1.0, 1.0]);
    assert_eq!(result, vec![4.0, 2.0, 0.0]);
}

#[test]
fn vec_add_基础() {
    let result = vec_add(&[1.0, 2.0], &[3.0, 4.0]);
    assert_eq!(result, vec![4.0, 6.0]);
}

#[test]
fn vec_scale_基础() {
    let result = vec_scale(&[1.0, 2.0, 3.0], 2.0);
    assert_eq!(result, vec![2.0, 4.0, 6.0]);
}

#[test]
fn vec_scale_零() {
    let result = vec_scale(&[1.0, 2.0], 0.0);
    assert_eq!(result, vec![0.0, 0.0]);
}

#[test]
fn mat_vec_mul_单位矩阵() {
    // 2×2 单位矩阵
    let identity = [1.0, 0.0, 0.0, 1.0];
    let v = [3.0, 5.0];
    let result = mat_vec_mul(&identity, 2, 2, &v);
    assert_eq!(result, vec![3.0, 5.0]);
}

#[test]
fn mat_vec_mul_一般情况() {
    // [[1,2],[3,4]] @ [1,1] = [3, 7]
    let m = [1.0, 2.0, 3.0, 4.0];
    let v = [1.0, 1.0];
    let result = mat_vec_mul(&m, 2, 2, &v);
    assert!((result[0] - 3.0).abs() < 1e-6);
    assert!((result[1] - 7.0).abs() < 1e-6);
}

#[test]
fn soft_threshold_基础() {
    let result = soft_threshold(&[1.0, -1.0, 0.3, -0.3, 0.0], 0.5);
    assert!((result[0] - 0.5).abs() < 1e-6, "1.0 - 0.5 = 0.5");
    assert!((result[1] - (-0.5)).abs() < 1e-6, "-1.0 + 0.5 = -0.5");
    assert_eq!(result[2], 0.0, "|0.3| < 0.5 应被截断到 0");
    assert_eq!(result[3], 0.0, "|-0.3| < 0.5 应被截断到 0");
    assert_eq!(result[4], 0.0, "0 应为 0");
}

// ════════════════════════════════════════════════════════════════
//  FISTA — 从内联分离 + 补齐边界条件
// ════════════════════════════════════════════════════════════════

#[test]
fn fista_基础分解() {
    let entities = vec![vec![1.0, 0.0, 0.0, 0.0], vec![0.0, 1.0, 0.0, 0.0]];
    let query = vec![0.8, 0.6, 0.0, 0.0];

    let (alpha, _residual, norm) = fista_solve(&query, &entities, 0.01, 100);

    assert!((alpha[0] - 0.79).abs() < 0.1, "alpha[0]={}", alpha[0]);
    assert!((alpha[1] - 0.59).abs() < 0.1, "alpha[1]={}", alpha[1]);
    assert!(norm < 0.1, "residual_norm={}", norm);
}

#[test]
fn fista_零查询() {
    let entities = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    let query = vec![0.0, 0.0];
    let (alpha, _residual, norm) = fista_solve(&query, &entities, 0.01, 100);
    assert!(alpha.iter().all(|&a| a.abs() < 0.01), "零查询应产生零系数");
    assert!(norm < 0.01);
}

#[test]
fn fista_单实体() {
    let entities = vec![vec![1.0, 0.0, 0.0]];
    let query = vec![1.0, 0.0, 0.0];
    let (alpha, _residual, norm) = fista_solve(&query, &entities, 0.01, 100);
    assert!((alpha[0] - 0.99).abs() < 0.1, "应完全投影到单一基");
    assert!(norm < 0.1);
}

// ════════════════════════════════════════════════════════════════
//  DPP — 从内联分离 + 补齐
// ════════════════════════════════════════════════════════════════

#[test]
fn dpp_多样性选择() {
    let vecs = vec![
        vec![1.0, 0.0],
        vec![0.99, 0.1], // 与 0 高度相似
        vec![0.0, 1.0],  // 与 0 正交
    ];
    let scores = vec![1.0, 0.9, 0.8];

    let selected = dpp_greedy(&vecs, &scores, 2, 1.0);
    assert_eq!(selected.len(), 2);
    assert_eq!(selected[0], 0, "最高分应被选中");
    assert_eq!(selected[1], 2, "DPP 应选多样化的而非高相似的");
}

#[test]
fn dpp_k等于n() {
    let vecs = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
    let scores = vec![1.0, 0.8];
    let selected = dpp_greedy(&vecs, &scores, 2, 1.0);
    assert_eq!(selected.len(), 2, "k=n 应选全部");
}

#[test]
fn dpp_k大于n() {
    let vecs = vec![vec![1.0, 0.0]];
    let scores = vec![1.0];
    let selected = dpp_greedy(&vecs, &scores, 5, 1.0);
    assert_eq!(selected.len(), 1, "k>n 时应返回全部 n 个");
}

#[test]
fn dpp_quality_weight为零_纯多样性() {
    let vecs = vec![vec![1.0, 0.0], vec![0.99, 0.1], vec![0.0, 1.0]];
    let scores = vec![1.0, 0.9, 0.1]; // 0.1 分很低但方向独特

    let selected = dpp_greedy(&vecs, &scores, 2, 0.0);
    assert_eq!(selected.len(), 2);
}

#[test]
fn dpp_大质量因子和大向量保持独立方向与稳定选择() {
    for scale in [1.0f32, 1e30] {
        let vectors = vec![
            vec![scale, 0.0],
            vec![0.8 * scale, 0.6 * scale],
            vec![0.6 * scale, 0.8 * scale],
        ];
        for score in [100.0, f32::MAX] {
            // 同分时先取编号 0，再取与其夹角更大的编号 2。
            assert_eq!(dpp_greedy(&vectors, &[score; 3], 2, 10.0), [0, 2]);
        }
    }
}

// ════════════════════════════════════════════════════════════════
//  NMF — 从内联分离 + 补齐
// ════════════════════════════════════════════════════════════════

#[test]
fn nmf_有限大数的标量分解保持原始尺度() {
    for value in [1.0f32, 1e30, f32::MAX] {
        let (w, h) = nmf_multiplicative_update(&[value], 1, 1, 1, 8, 1e-4);
        assert_eq!(w.len(), 1);
        assert_eq!(h.len(), 1);
        assert!(w[0].is_finite() && w[0] > 0.0);
        assert!(h[0].is_finite() && h[0] > 0.0);
        let reconstructed = f64::from(w[0]) * f64::from(h[0]);
        assert!((reconstructed / f64::from(value) - 1.0).abs() < 1e-5);
    }
}

#[test]
fn nmf_基本分解() {
    // 3×4 矩阵，分解为 rank=2
    let v = vec![1.0, 0.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.5, 0.5, 0.5, 0.5, 0.5];
    let (w, h) = nmf_multiplicative_update(&v, 3, 4, 2, 100, 1e-3);
    assert_eq!(w.len(), 3 * 2);
    assert_eq!(h.len(), 2 * 4);
    // 所有元素应非负
    assert!(w.iter().all(|&x| x >= 0.0), "W 应全部非负");
    assert!(h.iter().all(|&x| x >= 0.0), "H 应全部非负");
}

#[test]
fn nmf_重建误差应下降() {
    let v = vec![1.0, 0.0, 0.0, 1.0, 0.5, 0.5];
    let (w, h) = nmf_multiplicative_update(&v, 2, 3, 2, 200, 1e-4);

    // 重建: V_approx = W @ H
    let mut v_approx = [0.0f32; 6];
    for i in 0..2 {
        for j in 0..3 {
            for k in 0..2 {
                v_approx[i * 3 + j] += w[i * 2 + k] * h[k * 3 + j];
            }
        }
    }

    // 计算重建误差（Frobenius 范数）
    let err: f32 = v
        .iter()
        .zip(v_approx.iter())
        .map(|(a, b)| (a - b).powi(2))
        .sum();
    assert!(err < 0.5, "NMF 重建误差应较小，实际 {}", err);
}

#[test]
fn nmf_1x1_退化情况() {
    let v = vec![5.0f32];
    let (w, h) = nmf_multiplicative_update(&v, 1, 1, 1, 100, 1e-3);
    assert_eq!(w.len(), 1);
    assert_eq!(h.len(), 1);
    assert!((w[0] * h[0] - 5.0).abs() < 0.5, "1×1 分解应接近原值");
}
