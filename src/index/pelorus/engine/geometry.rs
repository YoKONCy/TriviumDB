//! 基于条件均值的两级符号-幅值（SM）球面重构与精确径向元数据耦合的 Poincaré 双曲代理距离（SM-Poincaré）。

use crate::index::pelorus::engine::quantization::{Bq2Signature, Bq2Store, SmCodeParams};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoincareNavigation {
    #[default]
    Exact,
    GeometryBq2,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct GeometryCode {
    pub(crate) radius: f64,
    pub(crate) boundary_factor: f64,
    pub(crate) sm: SmCodeParams,
}

/// 将固定 {1, 2} 权重 2-bit 离散向量的归一化内积 c_q ∈ [-1, 1] 经三阶 Hermite 逆映射校准为球面余弦估计（保留供 A6 消融对照）。
#[inline]
pub(crate) fn calibrate_bq2_cosine(cq: f64) -> f64 {
    let clamped = cq.clamp(-1.0, 1.0);
    (clamped / (0.835 + 0.165 * clamped * clamped)).clamp(-1.0, 1.0)
}

/// 两级重构内积经范数归一化与有理函数校准得到的球面余弦近似。
/// 校准保留正负一端点，统计偏差取决于输入分布。
#[inline]
pub(crate) fn sm_calibrated_cosine(
    a: SmCodeParams,
    b: SmCodeParams,
    counts: [u32; 5],
    dim: usize,
) -> f64 {
    let norm_sq_prod = a.exact_norm_sq(dim) * b.exact_norm_sq(dim);
    if norm_sq_prod <= 0.0 {
        return 0.0;
    }
    let denom = norm_sq_prod.sqrt();
    let dot = a.reconstructed_dot(b, counts, dim);
    let c_sm = (dot / denom).clamp(-1.0, 1.0);
    let kappa = denom.clamp(0.5, 1.0);
    (c_sm / (kappa + (1.0 - kappa) * c_sm * c_sm)).clamp(-1.0, 1.0)
}

/// 热循环零开方（0-sqrt）版本的两级条件均值自校准球面余弦算子，直接复用预计算的 f32 重构范数 sigma。
#[inline]
pub(crate) fn sm_calibrated_cosine_fast(
    a: SmCodeParams,
    b: SmCodeParams,
    counts: [u32; 5],
    dim: usize,
) -> f64 {
    let denom = f64::from(a.sigma) * f64::from(b.sigma);
    if denom <= 0.0 {
        return 0.0;
    }
    let dot = a.reconstructed_dot(b, counts, dim);
    let c_sm = (dot / denom).clamp(-1.0, 1.0);
    let kappa = denom.clamp(0.5, 1.0);
    (c_sm / (kappa + (1.0 - kappa) * c_sm * c_sm)).clamp(-1.0, 1.0)
}

impl GeometryCode {
    #[inline]
    pub(crate) fn new(vector: &[f32], boundary_factor: f64, signature: &Bq2Signature) -> Self {
        let (sm, radius) = SmCodeParams::from_vector_and_sig(vector, signature);
        Self {
            radius,
            boundary_factor,
            sm,
        }
    }

    #[inline]
    pub(crate) fn from_store(
        vector: &[f32],
        boundary_factor: f64,
        store: &Bq2Store,
        idx: usize,
    ) -> Self {
        let (sm, radius) = SmCodeParams::from_vector_and_store(vector, store, idx);
        Self {
            radius,
            boundary_factor,
            sm,
        }
    }

    /// 第三代 SM-Poincaré 代理距离：精确径向边界因子 × 两级条件均值两级符号-幅值球面余弦。
    #[inline]
    pub(crate) fn proxy_sm(self, other: Self, counts: [u32; 5], dimensions: usize) -> f64 {
        let denom = self.radius * other.radius;
        let angular = if denom > 0.0 {
            let cos_hat = sm_calibrated_cosine(self.sm, other.sm, counts, dimensions);
            (denom * (1.0 - cos_hat)).max(0.0)
        } else {
            0.0
        };
        ((self.radius - other.radius).powi(2) + 2.0 * angular)
            * self.boundary_factor
            * other.boundary_factor
    }

    /// 兼容传统固定 {1, 2} 权重 BQ2 校准代理（供历史消融对照使用）。
    #[inline]
    pub(crate) fn proxy(self, other: Self, bq_distance: u32, dimensions: usize) -> f64 {
        let dot = 4.0 * dimensions as f64 - f64::from(bq_distance);
        let denom = self.radius * other.radius;
        let angular = if denom > 0.0 {
            let ea = dimensions as f64 + 3.0 * f64::from(self.sm.strong_ones);
            let eb = dimensions as f64 + 3.0 * f64::from(other.sm.strong_ones);
            let cq = dot / (ea * eb).sqrt();
            let cos_hat = calibrate_bq2_cosine(cq);
            (denom * (1.0 - cos_hat)).max(0.0)
        } else {
            0.0
        };
        ((self.radius - other.radius).powi(2) + 2.0 * angular)
            * self.boundary_factor
            * other.boundary_factor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::pelorus::engine::metric::{Metric, poincare_proxy};
    use crate::index::pelorus::engine::quantization::sm_dot_counts_raw;

    #[test]
    fn 同码不同半径保留双曲距离且球心有效() {
        for (a, b) in [
            ([0.1, 0.2], [0.2, 0.4]),
            ([0.1, 0.2], [-0.2, -0.4]),
            ([0., 0.], [0.2, 0.4]),
        ] {
            let metric = Metric::Poincare { curvature: 1.0 };
            let fa = metric.boundary_factor(&a, 0).unwrap();
            let fb = metric.boundary_factor(&b, 1).unwrap();
            let sa = Bq2Signature::from_vector(&a);
            let sb = Bq2Signature::from_vector(&b);
            let counts = sm_dot_counts_raw(
                sa.pos.as_ptr(),
                sa.strong.as_ptr(),
                sb.pos.as_ptr(),
                sb.strong.as_ptr(),
                2,
            );
            let ca = GeometryCode::new(&a, fa, &sa);
            let cb = GeometryCode::new(&b, fb, &sb);
            let actual_sm = ca.proxy_sm(cb, counts, 2);
            let expected = poincare_proxy(&a, &b, fa * fb);
            assert!((actual_sm - expected).abs() < 1e-12);
            let actual_legacy = ca.proxy(cb, sa.distance(&sb, 2), 2);
            assert!((actual_legacy - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn 两级符号幅值重构消除稀疏零坐标符号污染() {
        let a = [0.8f32, 0.0, 0.0, 0.0];
        let b = [0.0f32, 0.8, 0.0, 0.0];
        let metric = Metric::Poincare { curvature: 1.0 };
        let fa = metric.boundary_factor(&a, 0).unwrap();
        let fb = metric.boundary_factor(&b, 1).unwrap();
        let sa = Bq2Signature::from_vector(&a);
        let sb = Bq2Signature::from_vector(&b);
        let counts = sm_dot_counts_raw(
            sa.pos.as_ptr(),
            sa.strong.as_ptr(),
            sb.pos.as_ptr(),
            sb.strong.as_ptr(),
            4,
        );
        let ca = GeometryCode::new(&a, fa, &sa);
        let cb = GeometryCode::new(&b, fb, &sb);
        let expected = poincare_proxy(&a, &b, fa * fb);
        let err_sm = (ca.proxy_sm(cb, counts, 4) - expected).abs();
        let err_bq2 = (ca.proxy(cb, sa.distance(&sb, 4), 4) - expected).abs();
        assert!(
            err_sm < 1e-12,
            "SM-Poincaré 在稀疏正交向量上应精确无损: err={err_sm}"
        );
        assert!(
            err_bq2 > 1.0,
            "固定权重 BQ2 因零坐标负号污染应存在显著误差: err={err_bq2}"
        );
    }
}
