use half::f16;
use std::fmt::Debug;
use crate::index::bq::FORCE_NO_AVX512;

/// 定义通用向量类型的 Trait，支持多种引擎底层数据 (f32 / f16 / u64)
pub trait VectorType:
    Sized
    + Copy
    + Default
    + PartialEq
    + Debug
    + Send
    + Sync
    + bytemuck::Zeroable
    + bytemuck::Pod
    + 'static
{
    /// 计算两个等长特征切片之间的“相似度”得分。
    /// 返回值越大，表示越相近。
    fn similarity(a: &[Self], b: &[Self]) -> f32;

    /// 返回类型的零值（用于逻辑删除时清空底座）
    fn zero() -> Self;

    /// 将单个元素转换为 f32（用于 QuIVer 索引等需要统一浮点表示的场景）
    fn to_f32(self) -> f32;

    /// 从 f32 构造单元素（用于产生数学计算后的残差向量等机制）
    fn from_f32(v: f32) -> Self;
}

// ════════ SIMD 多级回退内核：余弦相似度 ════════
//
// 分发优先级（运行时检测）：
//   x86_64: AVX-512F → AVX2+FMA → SSE3 → 标量
//   aarch64: NEON → 标量
//
// 注：AVX10/512 与 AVX-512F 共享同一指令集，无需额外检测。

/// 标量回退路径（四路展开，减少循环依赖链提升 IPC）
#[inline]
fn cosine_similarity_scalar(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    let (mut dot0, mut dot1) = (0.0f32, 0.0f32);
    let (mut na0, mut na1) = (0.0f32, 0.0f32);
    let (mut nb0, mut nb1) = (0.0f32, 0.0f32);

    let chunks = len / 4 * 4;
    let mut i = 0;
    while i < chunks {
        let (a0, a1, a2, a3) = (a[i], a[i + 1], a[i + 2], a[i + 3]);
        let (b0, b1, b2, b3) = (b[i], b[i + 1], b[i + 2], b[i + 3]);
        dot0 += a0 * b0 + a2 * b2;
        dot1 += a1 * b1 + a3 * b3;
        na0 += a0 * a0 + a2 * a2;
        na1 += a1 * a1 + a3 * a3;
        nb0 += b0 * b0 + b2 * b2;
        nb1 += b1 * b1 + b3 * b3;
        i += 4;
    }
    // 处理剩余元素
    while i < len {
        dot0 += a[i] * b[i];
        na0 += a[i] * a[i];
        nb0 += b[i] * b[i];
        i += 1;
    }
    let dot = dot0 + dot1;
    let norm_a = na0 + na1;
    let norm_b = nb0 + nb1;
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

/// AVX-512F 加速路径：每次并行处理 16 个 f32
#[cfg(all(target_arch = "x86_64", not(coverage)))]
#[target_feature(enable = "avx512f")]
unsafe fn cosine_similarity_avx512(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    let len = a.len().min(b.len());
    unsafe {
        let mut v_dot = _mm512_setzero_ps();
        let mut v_na = _mm512_setzero_ps();
        let mut v_nb = _mm512_setzero_ps();

        let chunks = len / 16;
        for i in 0..chunks {
            let offset = i * 16;
            let va = _mm512_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm512_loadu_ps(b.as_ptr().add(offset));
            v_dot = _mm512_fmadd_ps(va, vb, v_dot);
            v_na = _mm512_fmadd_ps(va, va, v_na);
            v_nb = _mm512_fmadd_ps(vb, vb, v_nb);
        }

        // 水平归约：512-bit → 256-bit → 128-bit → 标量
        let dot256_lo = _mm512_castps512_ps256(v_dot);
        let dot256_hi = _mm512_castps512_ps256(
            _mm512_shuffle_f32x4(v_dot, v_dot, 0b_01_00_11_10),
        );
        let d256 = _mm256_add_ps(dot256_lo, dot256_hi);
        let d128 = _mm_add_ps(_mm256_castps256_ps128(d256), _mm256_extractf128_ps(d256, 1));
        let d128 = _mm_hadd_ps(d128, d128);
        let d128 = _mm_hadd_ps(d128, d128);
        let mut dot = _mm_cvtss_f32(d128);

        let na256_lo = _mm512_castps512_ps256(v_na);
        let na256_hi = _mm512_castps512_ps256(
            _mm512_shuffle_f32x4(v_na, v_na, 0b_01_00_11_10),
        );
        let n256 = _mm256_add_ps(na256_lo, na256_hi);
        let n128 = _mm_add_ps(_mm256_castps256_ps128(n256), _mm256_extractf128_ps(n256, 1));
        let n128 = _mm_hadd_ps(n128, n128);
        let n128 = _mm_hadd_ps(n128, n128);
        let mut norm_a = _mm_cvtss_f32(n128);

        let nb256_lo = _mm512_castps512_ps256(v_nb);
        let nb256_hi = _mm512_castps512_ps256(
            _mm512_shuffle_f32x4(v_nb, v_nb, 0b_01_00_11_10),
        );
        let nb256 = _mm256_add_ps(nb256_lo, nb256_hi);
        let nb128 = _mm_add_ps(_mm256_castps256_ps128(nb256), _mm256_extractf128_ps(nb256, 1));
        let nb128 = _mm_hadd_ps(nb128, nb128);
        let nb128 = _mm_hadd_ps(nb128, nb128);
        let mut norm_b = _mm_cvtss_f32(nb128);

        let tail_start = chunks * 16;
        for i in tail_start..len {
            dot += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }
        if norm_a == 0.0 || norm_b == 0.0 { return 0.0; }
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

/// AVX2 + FMA 加速路径：每次并行处理 8 个 f32
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2", enable = "fma")]
unsafe fn cosine_similarity_avx2(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    let len = a.len().min(b.len());
    unsafe {
        let mut v_dot = _mm256_setzero_ps();
        let mut v_na = _mm256_setzero_ps();
        let mut v_nb = _mm256_setzero_ps();

        let chunks = len / 8;
        for i in 0..chunks {
            let offset = i * 8;
            let va = _mm256_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm256_loadu_ps(b.as_ptr().add(offset));
            v_dot = _mm256_fmadd_ps(va, vb, v_dot);
            v_na = _mm256_fmadd_ps(va, va, v_na);
            v_nb = _mm256_fmadd_ps(vb, vb, v_nb);
        }

        // 水平归约：256-bit → 128-bit → 标量
        let h_dot = _mm256_extractf128_ps(v_dot, 1);
        let h_na = _mm256_extractf128_ps(v_na, 1);
        let h_nb = _mm256_extractf128_ps(v_nb, 1);
        let l_dot = _mm256_castps256_ps128(v_dot);
        let l_na = _mm256_castps256_ps128(v_na);
        let l_nb = _mm256_castps256_ps128(v_nb);
        let s_dot = _mm_add_ps(l_dot, h_dot);
        let s_na = _mm_add_ps(l_na, h_na);
        let s_nb = _mm_add_ps(l_nb, h_nb);
        let s_dot = _mm_add_ps(_mm_hadd_ps(s_dot, s_dot), _mm_setzero_ps());
        let s_dot = _mm_hadd_ps(s_dot, s_dot);
        let s_na = _mm_hadd_ps(_mm_hadd_ps(s_na, s_na), _mm_hadd_ps(s_na, s_na));
        let s_nb = _mm_hadd_ps(_mm_hadd_ps(s_nb, s_nb), _mm_hadd_ps(s_nb, s_nb));

        let mut dot = _mm_cvtss_f32(s_dot);
        let mut norm_a = _mm_cvtss_f32(s_na);
        let mut norm_b = _mm_cvtss_f32(s_nb);

        let tail_start = chunks * 8;
        for i in tail_start..len {
            dot += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }
        if norm_a == 0.0 || norm_b == 0.0 { return 0.0; }
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

/// SSE3 加速路径：每次并行处理 4 个 f32（为无 AVX2 的老 x86_64 CPU 兜底）
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse3")]
unsafe fn cosine_similarity_sse3(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    let len = a.len().min(b.len());
    unsafe {
        let mut v_dot = _mm_setzero_ps();
        let mut v_na = _mm_setzero_ps();
        let mut v_nb = _mm_setzero_ps();

        let chunks = len / 4;
        for i in 0..chunks {
            let offset = i * 4;
            let va = _mm_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm_loadu_ps(b.as_ptr().add(offset));
            v_dot = _mm_add_ps(v_dot, _mm_mul_ps(va, vb));
            v_na = _mm_add_ps(v_na, _mm_mul_ps(va, va));
            v_nb = _mm_add_ps(v_nb, _mm_mul_ps(vb, vb));
        }

        // 水平归约（SSE3 hadd）
        let s_dot = _mm_hadd_ps(v_dot, v_dot);
        let s_dot = _mm_hadd_ps(s_dot, s_dot);
        let s_na = _mm_hadd_ps(v_na, v_na);
        let s_na = _mm_hadd_ps(s_na, s_na);
        let s_nb = _mm_hadd_ps(v_nb, v_nb);
        let s_nb = _mm_hadd_ps(s_nb, s_nb);

        let mut dot = _mm_cvtss_f32(s_dot);
        let mut norm_a = _mm_cvtss_f32(s_na);
        let mut norm_b = _mm_cvtss_f32(s_nb);

        let tail_start = chunks * 4;
        for i in tail_start..len {
            dot += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }
        if norm_a == 0.0 || norm_b == 0.0 { return 0.0; }
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

/// ARM NEON 加速路径：每次并行处理 4 个 f32
/// ARM64 (aarch64) 默认支持 NEON，无需运行时检测
#[cfg(all(target_arch = "aarch64", not(coverage)))]
#[target_feature(enable = "neon")]
unsafe fn cosine_similarity_neon(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::aarch64::*;

    let len = a.len().min(b.len());

    unsafe {
        let mut v_dot = vdupq_n_f32(0.0);
        let mut v_na = vdupq_n_f32(0.0);
        let mut v_nb = vdupq_n_f32(0.0);

        let chunks = len / 4;
        for i in 0..chunks {
            let offset = i * 4;
            let va = vld1q_f32(a.as_ptr().add(offset));
            let vb = vld1q_f32(b.as_ptr().add(offset));
            v_dot = vfmaq_f32(v_dot, va, vb); // dot += a * b
            v_na = vfmaq_f32(v_na, va, va); // na  += a * a
            v_nb = vfmaq_f32(v_nb, vb, vb); // nb  += b * b
        }

        // 水平归约：128-bit → 标量
        let mut dot = vaddvq_f32(v_dot);
        let mut norm_a = vaddvq_f32(v_na);
        let mut norm_b = vaddvq_f32(v_nb);

        // 处理尾部不足 4 个的元素
        let tail_start = chunks * 4;
        for i in tail_start..len {
            dot += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }

        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

/// 公开的分发函数：运行时自动选择最快路径
///
/// 优先级：x86 AVX-512F → AVX2+FMA → SSE3 → ARM NEON → 标量回退
#[inline]
pub fn cosine_similarity_f32(a: &[f32], b: &[f32]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        #[cfg(not(coverage))]
        if !*FORCE_NO_AVX512 && is_x86_feature_detected!("avx512f") {
            return unsafe { cosine_similarity_avx512(a, b) };
        }
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            return unsafe { cosine_similarity_avx2(a, b) };
        }
        if is_x86_feature_detected!("sse3") {
            return unsafe { cosine_similarity_sse3(a, b) };
        }
    }
    #[cfg(all(target_arch = "aarch64", not(coverage)))]
    {
        // ARM64 默认支持 NEON（ARMv8 基线指令集），无需运行时检测
        return unsafe { cosine_similarity_neon(a, b) };
    }
    #[allow(unreachable_code)]
    cosine_similarity_scalar(a, b)
}

// ════════ f32：普通高精度向量（余弦相似度） ════════
impl VectorType for f32 {
    #[inline]
    fn similarity(a: &[f32], b: &[f32]) -> f32 {
        cosine_similarity_f32(a, b)
    }

    #[inline]
    fn zero() -> Self {
        0.0
    }

    #[inline]
    fn to_f32(self) -> f32 {
        self
    }

    #[inline]
    fn from_f32(v: f32) -> Self {
        v
    }
}

// ════════ f16：半精度压缩向量（省 50% 内存） ════════
impl VectorType for f16 {
    #[inline]
    fn similarity(a: &[f16], b: &[f16]) -> f32 {
        // 批量转换为 f32 后复用 SIMD 加速的余弦相似度内核
        let af: Vec<f32> = a.iter().map(|x| x.to_f32()).collect();
        let bf: Vec<f32> = b.iter().map(|x| x.to_f32()).collect();
        cosine_similarity_f32(&af, &bf)
    }

    #[inline]
    fn zero() -> Self {
        f16::from_f32(0.0)
    }

    #[inline]
    fn to_f32(self) -> f32 {
        half::f16::to_f32(self)
    }

    #[inline]
    fn from_f32(v: f32) -> Self {
        half::f16::from_f32(v)
    }
}

// ════════ u64：二进制哈希向量（如 SimHash 或其他指纹） ════════
impl VectorType for u64 {
    #[inline]
    fn similarity(a: &[u64], b: &[u64]) -> f32 {
        let mut matches = 0;
        for (x, y) in a.iter().zip(b.iter()) {
            // 异或求不同位（汉明距离），64减去不同位 = 相同位的个数
            matches += 64 - (x ^ y).count_ones();
        }
        // 对于汉明相似度，数值就是匹配位的个数（越大越近）
        matches as f32
    }

    #[inline]
    fn zero() -> Self {
        0
    }

    #[inline]
    fn to_f32(self) -> f32 {
        self as f32
    }

    #[inline]
    fn from_f32(v: f32) -> Self {
        v as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use half::f16;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0, 0.0, 0.0];
        let c = vec![-1.0, 0.0, 0.0, 0.0, 0.0];
        let d = vec![0.0, 1.0, 0.0, 0.0, 0.0];

        assert!((f32::similarity(&a, &b) - 1.0).abs() < 1e-5);
        assert!((f32::similarity(&a, &c) + 1.0).abs() < 1e-5);
        assert!((f32::similarity(&a, &d)).abs() < 1e-5);

        let a16 = vec![f16::from_f32(1.0), f16::from_f32(0.0)];
        let b16 = vec![f16::from_f32(0.0), f16::from_f32(1.0)];
        assert!((f16::similarity(&a16, &b16)).abs() < 1e-5);

        let au = vec![0b1010, 0b1100];
        let bu = vec![0b0010, 0b0100];
        assert_eq!(u64::similarity(&au, &bu), 63.0 + 63.0); 
    }

    #[test]
    fn test_cosine_scalar_loop() {
        let a = vec![1.0; 20];
        let b = vec![1.0; 20];
        assert!((f32::similarity(&a, &b) - 1.0).abs() < 1e-5);
        
        // 边界情况：零向量
        let zeros = vec![0.0; 20];
        assert_eq!(f32::similarity(&zeros, &a), 0.0);
    }

    #[test]
    fn test_zero_and_f32_conv() {
        assert_eq!(f32::zero(), 0.0);
        assert_eq!(f16::zero(), f16::from_f32(0.0));
        assert_eq!(u64::zero(), 0);

        assert_eq!(1.5f32.to_f32(), 1.5);
        assert_eq!(f16::from_f32(1.5).to_f32(), 1.5);
        assert_eq!(1u64.to_f32(), 1.0);

        assert_eq!(f32::from_f32(2.0), 2.0);
        assert_eq!(f16::from_f32(2.0), f16::from_f32(2.0));
        assert_eq!(u64::from_f32(2.0), 2);
    }
}
