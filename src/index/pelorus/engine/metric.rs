//! 索引级几何契约与双曲距离内核。

use crate::index::pelorus::engine::{Error, Result};
#[cfg(target_arch = "x86_64")]
use std::sync::LazyLock;

#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum F64SimdKernel {
    #[cfg(not(coverage))]
    Avx512F,
    Avx2Fma,
    Scalar,
}

#[cfg(target_arch = "x86_64")]
static F64_SIMD_KERNEL: LazyLock<F64SimdKernel> = LazyLock::new(|| {
    #[cfg(not(coverage))]
    if !*crate::index::pelorus::engine::quantization::FORCE_NO_AVX512
        && std::arch::is_x86_feature_detected!("avx512f")
    {
        return F64SimdKernel::Avx512F;
    }
    if std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma") {
        return F64SimdKernel::Avx2Fma;
    }
    F64SimdKernel::Scalar
});

/// 索引在构图、寻路及结果评分中使用的几何。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Metric {
    /// 使用原有 BQ2 导航和余弦精排，分数越大越相近。
    #[default]
    Cosine,
    /// 曲率为 `-curvature` 的 Poincare 球；返回负测地距离，越大越相近。
    Poincare { curvature: f64 },
}

impl Metric {
    pub(crate) fn validate(self) -> Result<()> {
        if let Self::Poincare { curvature } = self
            && (!curvature.is_finite() || curvature <= 0.0)
        {
            return Err(Error::InvalidMetric("曲率绝对值必须是大于零的有限数"));
        }
        Ok(())
    }

    pub(crate) fn boundary_factor(self, vector: &[f32], index: usize) -> Result<f64> {
        match self {
            Self::Cosine => Ok(1.0),
            Self::Poincare { curvature } => {
                let norm_squared = norm_squared_f64(vector);
                let boundary = 1.0 - curvature * norm_squared;
                if boundary <= 0.0 || !boundary.is_finite() {
                    return Err(Error::OutsideManifold { index });
                }
                Ok(boundary.recip())
            }
        }
    }
}

/// 在 IEEE-754 f64 域内计算向量欧氏范数平方，避免高维累加精度损失。
#[inline]
pub(crate) fn norm_squared_f64(vector: &[f32]) -> f64 {
    #[cfg(target_arch = "x86_64")]
    {
        match *F64_SIMD_KERNEL {
            #[cfg(not(coverage))]
            F64SimdKernel::Avx512F => return unsafe { norm_squared_f64_avx512(vector) },
            F64SimdKernel::Avx2Fma => return unsafe { norm_squared_f64_avx2(vector) },
            F64SimdKernel::Scalar => {}
        }
    }
    norm_squared_f64_scalar(vector)
}

#[inline]
fn norm_squared_f64_scalar(vector: &[f32]) -> f64 {
    let len = vector.len();
    let mut s0 = 0.0f64;
    let mut s1 = 0.0f64;
    let mut s2 = 0.0f64;
    let mut s3 = 0.0f64;
    let chunks = len / 4 * 4;
    let mut i = 0;
    while i < chunks {
        let v0 = f64::from(vector[i]);
        let v1 = f64::from(vector[i + 1]);
        let v2 = f64::from(vector[i + 2]);
        let v3 = f64::from(vector[i + 3]);
        s0 += v0 * v0;
        s1 += v1 * v1;
        s2 += v2 * v2;
        s3 += v3 * v3;
        i += 4;
    }
    while i < len {
        let v = f64::from(vector[i]);
        s0 += v * v;
        i += 1;
    }
    (s0 + s1) + (s2 + s3)
}

#[cfg(all(target_arch = "x86_64", not(coverage)))]
#[target_feature(enable = "avx512f")]
unsafe fn norm_squared_f64_avx512(vector: &[f32]) -> f64 {
    use std::arch::x86_64::*;
    unsafe {
        let len = vector.len();
        let ptr = vector.as_ptr();
        let mut acc0 = _mm512_setzero_pd();
        let mut acc1 = _mm512_setzero_pd();
        let mut i = 0usize;
        while i + 16 <= len {
            let v0 = _mm512_cvtps_pd(_mm256_loadu_ps(ptr.add(i)));
            let v1 = _mm512_cvtps_pd(_mm256_loadu_ps(ptr.add(i + 8)));
            acc0 = _mm512_fmadd_pd(v0, v0, acc0);
            acc1 = _mm512_fmadd_pd(v1, v1, acc1);
            i += 16;
        }
        let mut sum = _mm512_reduce_add_pd(_mm512_add_pd(acc0, acc1));
        while i < len {
            let v = f64::from(*ptr.add(i));
            sum += v * v;
            i += 1;
        }
        sum
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn norm_squared_f64_avx2(vector: &[f32]) -> f64 {
    use std::arch::x86_64::*;
    unsafe {
        let len = vector.len();
        let ptr = vector.as_ptr();
        let mut acc0 = _mm256_setzero_pd();
        let mut acc1 = _mm256_setzero_pd();
        let mut acc2 = _mm256_setzero_pd();
        let mut acc3 = _mm256_setzero_pd();
        let mut i = 0usize;
        while i + 16 <= len {
            let v0 = _mm256_cvtps_pd(_mm_loadu_ps(ptr.add(i)));
            let v1 = _mm256_cvtps_pd(_mm_loadu_ps(ptr.add(i + 4)));
            let v2 = _mm256_cvtps_pd(_mm_loadu_ps(ptr.add(i + 8)));
            let v3 = _mm256_cvtps_pd(_mm_loadu_ps(ptr.add(i + 12)));
            acc0 = _mm256_fmadd_pd(v0, v0, acc0);
            acc1 = _mm256_fmadd_pd(v1, v1, acc1);
            acc2 = _mm256_fmadd_pd(v2, v2, acc2);
            acc3 = _mm256_fmadd_pd(v3, v3, acc3);
            i += 16;
        }
        while i + 4 <= len {
            let v0 = _mm256_cvtps_pd(_mm_loadu_ps(ptr.add(i)));
            acc0 = _mm256_fmadd_pd(v0, v0, acc0);
            i += 4;
        }
        let sum256 = _mm256_add_pd(_mm256_add_pd(acc0, acc1), _mm256_add_pd(acc2, acc3));
        let lo = _mm256_castpd256_pd128(sum256);
        let hi = _mm256_extractf128_pd(sum256, 1);
        let sum128 = _mm_add_pd(lo, hi);
        let mut sum = _mm_cvtsd_f64(_mm_add_pd(sum128, _mm_unpackhi_pd(sum128, sum128)));
        while i < len {
            let v = f64::from(*ptr.add(i));
            sum += v * v;
            i += 1;
        }
        sum
    }
}

/// 返回 `sinh(sqrt(c) * d / 2)^2 / c`，严格保持固定曲率下的双曲距离排序。
/// 在 f64 中直接累加坐标差，避免近邻处由范数减内积造成的消减误差。
#[inline]
pub(crate) fn poincare_proxy(a: &[f32], b: &[f32], factors: f64) -> f64 {
    delta_squared_f64(a, b) * factors
}

#[inline]
fn delta_squared_f64(a: &[f32], b: &[f32]) -> f64 {
    #[cfg(target_arch = "x86_64")]
    {
        match *F64_SIMD_KERNEL {
            #[cfg(not(coverage))]
            F64SimdKernel::Avx512F => return unsafe { delta_squared_f64_avx512(a, b) },
            F64SimdKernel::Avx2Fma => return unsafe { delta_squared_f64_avx2(a, b) },
            F64SimdKernel::Scalar => {}
        }
    }
    delta_squared_f64_scalar(a, b)
}

#[inline]
fn delta_squared_f64_scalar(a: &[f32], b: &[f32]) -> f64 {
    let len = a.len().min(b.len());
    let mut s0 = 0.0f64;
    let mut s1 = 0.0f64;
    let mut s2 = 0.0f64;
    let mut s3 = 0.0f64;
    let chunks = len / 4 * 4;
    let mut i = 0usize;
    while i < chunks {
        let d0 = f64::from(a[i]) - f64::from(b[i]);
        let d1 = f64::from(a[i + 1]) - f64::from(b[i + 1]);
        let d2 = f64::from(a[i + 2]) - f64::from(b[i + 2]);
        let d3 = f64::from(a[i + 3]) - f64::from(b[i + 3]);
        s0 += d0 * d0;
        s1 += d1 * d1;
        s2 += d2 * d2;
        s3 += d3 * d3;
        i += 4;
    }
    while i < len {
        let d = f64::from(a[i]) - f64::from(b[i]);
        s0 += d * d;
        i += 1;
    }
    (s0 + s1) + (s2 + s3)
}

#[cfg(all(target_arch = "x86_64", not(coverage)))]
#[target_feature(enable = "avx512f")]
unsafe fn delta_squared_f64_avx512(a: &[f32], b: &[f32]) -> f64 {
    use std::arch::x86_64::*;
    unsafe {
        let len = a.len().min(b.len());
        let pa = a.as_ptr();
        let pb = b.as_ptr();
        let mut acc0 = _mm512_setzero_pd();
        let mut acc1 = _mm512_setzero_pd();
        let mut i = 0usize;
        while i + 16 <= len {
            let d0 = _mm512_sub_pd(
                _mm512_cvtps_pd(_mm256_loadu_ps(pa.add(i))),
                _mm512_cvtps_pd(_mm256_loadu_ps(pb.add(i))),
            );
            let d1 = _mm512_sub_pd(
                _mm512_cvtps_pd(_mm256_loadu_ps(pa.add(i + 8))),
                _mm512_cvtps_pd(_mm256_loadu_ps(pb.add(i + 8))),
            );
            acc0 = _mm512_fmadd_pd(d0, d0, acc0);
            acc1 = _mm512_fmadd_pd(d1, d1, acc1);
            i += 16;
        }
        let mut sum = _mm512_reduce_add_pd(_mm512_add_pd(acc0, acc1));
        while i < len {
            let d = f64::from(*pa.add(i)) - f64::from(*pb.add(i));
            sum += d * d;
            i += 1;
        }
        sum
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn delta_squared_f64_avx2(a: &[f32], b: &[f32]) -> f64 {
    use std::arch::x86_64::*;
    unsafe {
        let len = a.len().min(b.len());
        let pa = a.as_ptr();
        let pb = b.as_ptr();
        let mut acc0 = _mm256_setzero_pd();
        let mut acc1 = _mm256_setzero_pd();
        let mut acc2 = _mm256_setzero_pd();
        let mut acc3 = _mm256_setzero_pd();
        let mut i = 0usize;
        while i + 16 <= len {
            let d0 = _mm256_sub_pd(
                _mm256_cvtps_pd(_mm_loadu_ps(pa.add(i))),
                _mm256_cvtps_pd(_mm_loadu_ps(pb.add(i))),
            );
            let d1 = _mm256_sub_pd(
                _mm256_cvtps_pd(_mm_loadu_ps(pa.add(i + 4))),
                _mm256_cvtps_pd(_mm_loadu_ps(pb.add(i + 4))),
            );
            let d2 = _mm256_sub_pd(
                _mm256_cvtps_pd(_mm_loadu_ps(pa.add(i + 8))),
                _mm256_cvtps_pd(_mm_loadu_ps(pb.add(i + 8))),
            );
            let d3 = _mm256_sub_pd(
                _mm256_cvtps_pd(_mm_loadu_ps(pa.add(i + 12))),
                _mm256_cvtps_pd(_mm_loadu_ps(pb.add(i + 12))),
            );
            acc0 = _mm256_fmadd_pd(d0, d0, acc0);
            acc1 = _mm256_fmadd_pd(d1, d1, acc1);
            acc2 = _mm256_fmadd_pd(d2, d2, acc2);
            acc3 = _mm256_fmadd_pd(d3, d3, acc3);
            i += 16;
        }
        while i + 4 <= len {
            let d0 = _mm256_sub_pd(
                _mm256_cvtps_pd(_mm_loadu_ps(pa.add(i))),
                _mm256_cvtps_pd(_mm_loadu_ps(pb.add(i))),
            );
            acc0 = _mm256_fmadd_pd(d0, d0, acc0);
            i += 4;
        }
        let sum256 = _mm256_add_pd(_mm256_add_pd(acc0, acc1), _mm256_add_pd(acc2, acc3));
        let lo = _mm256_castpd256_pd128(sum256);
        let hi = _mm256_extractf128_pd(sum256, 1);
        let sum128 = _mm_add_pd(lo, hi);
        let mut sum = _mm_cvtsd_f64(_mm_add_pd(sum128, _mm_unpackhi_pd(sum128, sum128)));
        while i < len {
            let d = f64::from(*pa.add(i)) - f64::from(*pb.add(i));
            sum += d * d;
            i += 1;
        }
        sum
    }
}

/// 单调代理只用于排序；含 alpha 的距离比较必须先恢复测地距离。
#[inline]
pub(crate) fn poincare_distance(proxy: f64, curvature: f64) -> f64 {
    let root = curvature.sqrt();
    2.0 * ((root * proxy.sqrt()).asinh() / root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 双精度坐标差_simd_内核与标量参考跨维度严格一致() {
        for dim in [1, 2, 3, 7, 15, 16, 17, 64, 65, 256, 384, 768, 1025] {
            let a: Vec<f32> = (0..dim)
                .map(|i| ((i * 17 + 3) as f32 * 0.13).sin() * 0.05)
                .collect();
            let b: Vec<f32> = (0..dim)
                .map(|i| ((i * 31 + 9) as f32 * 0.11).cos() * 0.05)
                .collect();
            let expected_d: f64 = a
                .iter()
                .zip(&b)
                .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
                .sum();
            let actual_d = delta_squared_f64(&a, &b);
            assert!(
                (actual_d - expected_d).abs() <= 1e-13 * expected_d.max(1.0),
                "dim={dim}: actual={actual_d}, expected={expected_d}"
            );
            let expected_n: f64 = a.iter().map(|&x| f64::from(x).powi(2)).sum();
            let actual_n = norm_squared_f64(&a);
            assert!(
                (actual_n - expected_n).abs() <= 1e-13 * expected_n.max(1.0),
                "dim={dim}: actual_n={actual_n}, expected_n={expected_n}"
            );
        }
    }
}
