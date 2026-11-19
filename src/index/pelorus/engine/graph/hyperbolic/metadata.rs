//! 极坐标节点元数据与安全下界。

use super::*;
/// 严格 32 字节（每条 64B CPU 缓存行容纳 2 个节点）的热路径极坐标与 SM 重构元数据。
///
/// 一次半缓存行读取即可同时完成：
/// 1. Stage-0 零开方 SM-Poincaré 代理距离计算（含可选切锥正交残差重构）；
/// 2. Stage-1 精确导航中的保守径向 Cauchy-Schwarz 下界预滤。
#[repr(C, align(32))]
#[derive(Clone, Copy)]
pub(crate) struct PolarNodeMeta {
    /// 欧氏半径 ||x||_2
    pub(crate) radius: f32,
    /// 边界共形因子 f_x = 1 / (1 - c ||x||_2^2)
    pub(crate) boundary_factor: f32,
    /// 无量纲测地深度 sqrt(c) * R_x
    pub(crate) depth: f32,
    /// 单位方向（或正交残差方向）弱幅值组条件均值 a_x
    pub(crate) sm_weak: f32,
    /// 单位方向（或正交残差方向）强弱幅值组均值差 delta_x = b_x - a_x
    pub(crate) sm_delta: f32,
    /// 单位方向（或正交残差方向）两级重构范数 sigma_x = ||u_hat_x||_2
    pub(crate) sm_sigma: f32,
    /// 紧凑位打包：低 20 位为 popcnt(strong)，高 12 位为切锥主轴编号（NO_CONE_ID = 0x0FFF 表示全局编码）
    pub(crate) packed_strong_cone: u32,
    /// 有切锥时存储主轴投影余弦 <u_x, mu_c>；无切锥时存储传统 2-bit 离散范数倒数供 A5/A6 消融零开销读取
    pub(crate) cone_alpha: f32,
}
const _: () = assert!(std::mem::size_of::<PolarNodeMeta>() == 32);
impl Default for PolarNodeMeta {
    #[inline]
    fn default() -> Self {
        Self {
            radius: 0.0,
            boundary_factor: 0.0,
            depth: 0.0,
            sm_weak: 0.0,
            sm_delta: 0.0,
            sm_sigma: 0.0,
            packed_strong_cone: u32::from(NO_CONE_ID) << 20,
            cone_alpha: 0.0,
        }
    }
}
impl PolarNodeMeta {
    #[inline]
    pub(crate) fn from_node(
        vector: &[f32],
        boundary_factor: f64,
        store: &Bq2Store,
        idx: usize,
        dim: usize,
    ) -> Self {
        let (sm, radius) = SmCodeParams::from_vector_and_store(vector, store, idx);
        let depth = dimensionless_depth(boundary_factor) as f32;
        let strong_clamped = sm.strong_ones.min(STRONG_ONES_MASK);
        Self {
            radius: radius as f32,
            boundary_factor: boundary_factor.min(f64::from(f32::MAX)) as f32,
            depth,
            sm_weak: sm.weak,
            sm_delta: sm.delta,
            sm_sigma: sm.sigma,
            packed_strong_cone: (u32::from(NO_CONE_ID) << 20) | strong_clamped,
            cone_alpha: bq2_inv_qnorm(dim, sm.strong_ones),
        }
    }

    #[inline(always)]
    pub(crate) fn strong_ones(self) -> u32 {
        self.packed_strong_cone & STRONG_ONES_MASK
    }

    #[inline(always)]
    pub(crate) fn cone_id(self) -> u16 {
        (self.packed_strong_cone >> 20) as u16
    }

    #[inline(always)]
    pub(crate) fn has_residual_cone(self) -> bool {
        self.cone_id() != NO_CONE_ID
    }

    #[inline]
    pub(crate) fn legacy_inv_qnorm(self, dim: usize) -> f32 {
        if self.cone_id() == NO_CONE_ID {
            self.cone_alpha
        } else {
            bq2_inv_qnorm(dim, self.strong_ones())
        }
    }

    /// 将当前节点升级为切锥正交残差编码元数据，严格保持 32 字节布局不变。
    #[inline]
    pub(crate) fn set_residual_cone(&mut self, cone_id: u16, alpha: f32, res_sm: SmCodeParams) {
        debug_assert!(cone_id < NO_CONE_ID);
        let strong_clamped = res_sm.strong_ones.min(STRONG_ONES_MASK);
        self.sm_weak = res_sm.weak;
        self.sm_delta = res_sm.delta;
        self.sm_sigma = res_sm.sigma;
        self.packed_strong_cone = (u32::from(cone_id) << 20) | strong_clamped;
        self.cone_alpha = alpha;
    }

    #[inline(always)]
    pub(crate) fn sm_params(self) -> SmCodeParams {
        SmCodeParams {
            weak: self.sm_weak,
            delta: self.sm_delta,
            sigma: self.sm_sigma,
            strong_ones: self.strong_ones(),
        }
    }

    /// 零开方、零冷存访问计算 SM-Poincaré 代理距离，量纲与精确 `poincare_proxy` 严格一致。
    #[inline]
    pub(crate) fn sm_poincare_proxy(self, q_radius: f64, q_factor: f64, cos_hat: f64) -> f64 {
        let rv = f64::from(self.radius);
        let fv = f64::from(self.boundary_factor);
        let dr = q_radius - rv;
        let angular = (q_radius * rv * (1.0 - cos_hat)).max(0.0);
        (dr * dr + 2.0 * angular) * q_factor * fv
    }

    /// 使用半径与因子的向外舍入裕量计算逆三角不等式下界。
    /// 极端数值无法给出有效保守区间时返回零。
    #[inline]
    pub(crate) fn radial_lower_bound(self, q_radius: f64, q_factor: f64) -> f64 {
        #[cfg(feature = "research")]
        if !geometry_components().radial_bound {
            return 0.0;
        }
        radial_bound_core(
            q_radius,
            f64::from(self.radius),
            q_factor,
            f64::from(self.boundary_factor),
        )
    }

    /// 热循环内已缓存 `radial_bound_enabled` 时的零原子读取下界计算。
    #[inline]
    pub(crate) fn radial_lower_bound_fast(
        self,
        q_radius: f64,
        q_factor: f64,
        enabled: bool,
    ) -> f64 {
        if !enabled {
            return 0.0;
        }
        radial_bound_core(
            q_radius,
            f64::from(self.radius),
            q_factor,
            f64::from(self.boundary_factor),
        )
    }
}
#[inline]
pub(crate) fn a8_search_budget() -> bool {
    #[cfg(feature = "research")]
    {
        matches!(
            hyperbolic_strategy(),
            HyperbolicStrategy::Full
                | HyperbolicStrategy::ScalarQuantizedDirection
                | HyperbolicStrategy::FullPrecisionDirection
        )
    }
    #[cfg(not(feature = "research"))]
    {
        true
    }
}
/// f32 半径与因子采用较宽的舍入区间，同时覆盖范数求和的浮点误差。
#[inline]
pub(crate) fn radial_bound(qr: f64, vr: f64, qf: f64, vf: f64) -> f64 {
    #[cfg(feature = "research")]
    if !geometry_components().radial_bound {
        return 0.0;
    }
    radial_bound_core(qr, vr, qf, vf)
}
#[inline]
pub(crate) fn radial_bound_core(qr: f64, vr: f64, qf: f64, vf: f64) -> f64 {
    if !qr.is_finite()
        || !vr.is_finite()
        || !qf.is_finite()
        || !vf.is_finite()
        || vf >= f64::from(f32::MAX) * 0.5
        || vr >= f64::from(f32::MAX) * 0.5
    {
        return 0.0;
    }
    let margin = 1e-6 * (qr + vr) + f64::from(f32::from_bits(1));
    let gap = ((qr - vr).abs() - margin).max(0.0);
    let lower = gap * gap * qf * vf * (1.0 - 1e-5);
    if lower.is_finite() { lower } else { 0.0 }
}
