//! 向量标量类型抽象与跨架构相似度实现。
//!
//! VectorType 统一 f32、f16 与 u64 的存储、校验、序列化和距离语义；运行时 SIMD 路径
//! 必须先检测 CPU 特性并正确处理尾部元素，任何 NaN/Infinity 都在系统边界拒绝。

#[cfg(all(target_arch = "x86_64", not(coverage)))]
use crate::index::bq::FORCE_NO_AVX512;
use half::f16;
use std::fmt::Debug;

/// 二进制指纹保持整数位模式，采用汉明匹配位数评分。
pub(crate) fn is_binary_vector<T: VectorType>() -> bool {
    std::any::TypeId::of::<T>() == std::any::TypeId::of::<u64>()
}

/// 检索入口在候选生成与工作区分配前统一验证维度和有限值。
pub(crate) fn validate_query_vector<T: VectorType>(query: &[T], dim: usize) -> crate::Result<()> {
    if query.len() != dim {
        return Err(crate::TriviumError::DimensionMismatch {
            expected: dim,
            got: query.len(),
        });
    }
    if query.iter().any(|value| !value.to_f32().is_finite()) {
        return Err(crate::TriviumError::InvalidVector {
            reason: "查询向量包含 NaN 或 Infinity (Query vector contains NaN or Infinity)".into(),
        });
    }
    Ok(())
}

#[inline]
fn finish_cosine(dot: f32, norm_a: f32, norm_b: f32, a: &[f32], b: &[f32]) -> f32 {
    if norm_a.is_normal() && norm_b.is_normal() && dot.is_normal() {
        let denominator = norm_a.sqrt() * norm_b.sqrt();
        if denominator.is_normal() {
            let score = dot / denominator;
            if score.is_finite() {
                return score.clamp(-1., 1.);
            }
        }
    }
    cosine_similarity_wide(a, b)
}

// f64 覆盖全部有限 f32 分量的乘积与可寻址切片累加范围，异常幅值回退不改变原始向量。
#[cold]
fn cosine_similarity_wide(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut norm_a, mut norm_b) = (0f64, 0f64, 0f64);
    for (&a, &b) in a.iter().zip(b) {
        let (a, b) = (f64::from(a), f64::from(b));
        dot += a * b;
        norm_a += a * a;
        norm_b += b * b;
    }
    if !dot.is_finite() || !norm_a.is_finite() || !norm_b.is_finite() {
        return f32::NAN;
    }
    if norm_a == 0. || norm_b == 0. {
        return 0.;
    }
    (dot / (norm_a.sqrt() * norm_b.sqrt())).clamp(-1., 1.) as f32
}

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

    /// 按目标 dtype 校验并转换 JSON 数值；整数类型覆盖此方法以保留完整位模式。
    fn from_json_number(value: &serde_json::Number) -> crate::Result<Self> {
        let number = value
            .as_f64()
            .ok_or_else(|| crate::TriviumError::InvalidVector {
                reason: "向量分量必须为数值 (Vector components must be numeric)".into(),
            })?;
        let converted = Self::from_f32(number as f32);
        if !(number as f32).is_finite() || !converted.to_f32().is_finite() {
            return Err(crate::TriviumError::InvalidVector {
                reason: "向量分量超出目标类型的有限范围 (Vector component exceeds the finite range of its dtype)".into(),
            });
        }
        Ok(converted)
    }
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
    finish_cosine(dot, norm_a, norm_b, a, b)
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
        let dot256_hi = _mm512_castps512_ps256(_mm512_shuffle_f32x4(v_dot, v_dot, 0b_01_00_11_10));
        let d256 = _mm256_add_ps(dot256_lo, dot256_hi);
        let d128 = _mm_add_ps(_mm256_castps256_ps128(d256), _mm256_extractf128_ps(d256, 1));
        let d128 = _mm_hadd_ps(d128, d128);
        let d128 = _mm_hadd_ps(d128, d128);
        let mut dot = _mm_cvtss_f32(d128);

        let na256_lo = _mm512_castps512_ps256(v_na);
        let na256_hi = _mm512_castps512_ps256(_mm512_shuffle_f32x4(v_na, v_na, 0b_01_00_11_10));
        let n256 = _mm256_add_ps(na256_lo, na256_hi);
        let n128 = _mm_add_ps(_mm256_castps256_ps128(n256), _mm256_extractf128_ps(n256, 1));
        let n128 = _mm_hadd_ps(n128, n128);
        let n128 = _mm_hadd_ps(n128, n128);
        let mut norm_a = _mm_cvtss_f32(n128);

        let nb256_lo = _mm512_castps512_ps256(v_nb);
        let nb256_hi = _mm512_castps512_ps256(_mm512_shuffle_f32x4(v_nb, v_nb, 0b_01_00_11_10));
        let nb256 = _mm256_add_ps(nb256_lo, nb256_hi);
        let nb128 = _mm_add_ps(
            _mm256_castps256_ps128(nb256),
            _mm256_extractf128_ps(nb256, 1),
        );
        let nb128 = _mm_hadd_ps(nb128, nb128);
        let nb128 = _mm_hadd_ps(nb128, nb128);
        let mut norm_b = _mm_cvtss_f32(nb128);

        let tail_start = chunks * 16;
        for i in tail_start..len {
            dot += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }
        finish_cosine(dot, norm_a, norm_b, a, b)
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
        finish_cosine(dot, norm_a, norm_b, a, b)
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2", enable = "fma")]
unsafe fn norm_squared_avx2_384(vector: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    let mut acc0 = _mm256_setzero_ps();
    let mut acc1 = _mm256_setzero_ps();
    let mut acc2 = _mm256_setzero_ps();
    let mut acc3 = _mm256_setzero_ps();
    for offset in (0..384).step_by(32) {
        let v0 = unsafe { _mm256_loadu_ps(vector.as_ptr().add(offset)) };
        let v1 = unsafe { _mm256_loadu_ps(vector.as_ptr().add(offset + 8)) };
        let v2 = unsafe { _mm256_loadu_ps(vector.as_ptr().add(offset + 16)) };
        let v3 = unsafe { _mm256_loadu_ps(vector.as_ptr().add(offset + 24)) };
        acc0 = _mm256_fmadd_ps(v0, v0, acc0);
        acc1 = _mm256_fmadd_ps(v1, v1, acc1);
        acc2 = _mm256_fmadd_ps(v2, v2, acc2);
        acc3 = _mm256_fmadd_ps(v3, v3, acc3);
    }
    unsafe {
        horizontal_sum_avx2(_mm256_add_ps(
            _mm256_add_ps(acc0, acc1),
            _mm256_add_ps(acc2, acc3),
        ))
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2", enable = "fma")]
unsafe fn cosine_similarity_avx2_384_with_query_norm(
    query: &[f32],
    vector: &[f32],
    query_norm_squared: f32,
) -> f32 {
    use std::arch::x86_64::*;
    let mut dot0 = _mm256_setzero_ps();
    let mut dot1 = _mm256_setzero_ps();
    let mut dot2 = _mm256_setzero_ps();
    let mut dot3 = _mm256_setzero_ps();
    let mut norm0 = _mm256_setzero_ps();
    let mut norm1 = _mm256_setzero_ps();
    let mut norm2 = _mm256_setzero_ps();
    let mut norm3 = _mm256_setzero_ps();
    for offset in (0..384).step_by(32) {
        let q0 = unsafe { _mm256_loadu_ps(query.as_ptr().add(offset)) };
        let q1 = unsafe { _mm256_loadu_ps(query.as_ptr().add(offset + 8)) };
        let q2 = unsafe { _mm256_loadu_ps(query.as_ptr().add(offset + 16)) };
        let q3 = unsafe { _mm256_loadu_ps(query.as_ptr().add(offset + 24)) };
        let v0 = unsafe { _mm256_loadu_ps(vector.as_ptr().add(offset)) };
        let v1 = unsafe { _mm256_loadu_ps(vector.as_ptr().add(offset + 8)) };
        let v2 = unsafe { _mm256_loadu_ps(vector.as_ptr().add(offset + 16)) };
        let v3 = unsafe { _mm256_loadu_ps(vector.as_ptr().add(offset + 24)) };
        dot0 = _mm256_fmadd_ps(q0, v0, dot0);
        dot1 = _mm256_fmadd_ps(q1, v1, dot1);
        dot2 = _mm256_fmadd_ps(q2, v2, dot2);
        dot3 = _mm256_fmadd_ps(q3, v3, dot3);
        norm0 = _mm256_fmadd_ps(v0, v0, norm0);
        norm1 = _mm256_fmadd_ps(v1, v1, norm1);
        norm2 = _mm256_fmadd_ps(v2, v2, norm2);
        norm3 = _mm256_fmadd_ps(v3, v3, norm3);
    }
    let dot = unsafe {
        horizontal_sum_avx2(_mm256_add_ps(
            _mm256_add_ps(dot0, dot1),
            _mm256_add_ps(dot2, dot3),
        ))
    };
    let vector_norm_squared = unsafe {
        horizontal_sum_avx2(_mm256_add_ps(
            _mm256_add_ps(norm0, norm1),
            _mm256_add_ps(norm2, norm3),
        ))
    };
    finish_cosine(dot, query_norm_squared, vector_norm_squared, query, vector)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn horizontal_sum_avx2(value: std::arch::x86_64::__m256) -> f32 {
    use std::arch::x86_64::*;
    let sum = _mm_add_ps(
        _mm256_castps256_ps128(value),
        _mm256_extractf128_ps(value, 1),
    );
    let sum = _mm_hadd_ps(sum, sum);
    _mm_cvtss_f32(_mm_hadd_ps(sum, sum))
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
        finish_cosine(dot, norm_a, norm_b, a, b)
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

        finish_cosine(dot, norm_a, norm_b, a, b)
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

pub(crate) fn cosine_query_norm_f32_384(query: &[f32]) -> Option<f32> {
    if query.len() != 384 {
        return None;
    }
    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
        return Some(unsafe { norm_squared_avx2_384(query) });
    }
    None
}

pub(crate) fn cosine_similarity_f32_384_with_query_norm(
    query: &[f32],
    vector: &[f32],
    query_norm_squared: f32,
) -> f32 {
    #[cfg(target_arch = "x86_64")]
    if query.len() == 384
        && vector.len() == 384
        && is_x86_feature_detected!("avx2")
        && is_x86_feature_detected!("fma")
    {
        return unsafe {
            cosine_similarity_avx2_384_with_query_norm(query, vector, query_norm_squared)
        };
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = query_norm_squared;
    cosine_similarity_f32(query, vector)
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

    fn from_json_number(value: &serde_json::Number) -> crate::Result<Self> {
        if let Some(integer) = value.as_u64() {
            return Ok(integer);
        }
        if let Some(number) = value.as_f64()
            && number.is_finite()
            && (0.0..18_446_744_073_709_551_616.0).contains(&number)
            && number.fract() == 0.0
        {
            return Ok(number as u64);
        }
        Err(crate::TriviumError::InvalidVector {
            reason: "u64 向量分量必须是范围内的非负整数 (u64 vector components must be non-negative integers within range)".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use half::f16;

    #[test]
    fn 所有余弦实现对极端有限尺度保持解析结果() {
        for dim in [1, 2, 7, 8, 15, 16, 17, 383, 384, 385, 3072, 65536] {
            for scale in [1e20f32, 1e-30, f32::MAX, f32::from_bits(1)] {
                let a = vec![scale; dim];
                let b = vec![-scale; dim];
                let check = |name: &str, score: f32, expected: f32| {
                    assert!(
                        score.is_finite() && (score - expected).abs() < 2e-6,
                        "{name}/{dim}/{scale}: {score}, 预期 {expected}"
                    );
                };
                check("dispatch", cosine_similarity_f32(&a, &a), 1.);
                check("scalar", cosine_similarity_scalar(&a, &b), -1.);
                #[cfg(target_arch = "x86_64")]
                {
                    if is_x86_feature_detected!("sse3") {
                        // SAFETY: 已检查 CPU 特性，切片等长并由当前作用域持有。
                        check("sse3", unsafe { cosine_similarity_sse3(&a, &a) }, 1.);
                    }
                    if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
                        // SAFETY: 已检查 AVX2/FMA，切片等长且所有 SIMD 尾部按长度处理。
                        check("avx2", unsafe { cosine_similarity_avx2(&a, &b) }, -1.);
                    }
                    #[cfg(not(coverage))]
                    if is_x86_feature_detected!("avx512f") {
                        // SAFETY: 已检查 AVX-512F，切片等长并由当前作用域持有。
                        check("avx512", unsafe { cosine_similarity_avx512(&a, &a) }, 1.);
                    }
                }
                if dim == 384 {
                    let norm = cosine_query_norm_f32_384(&a).unwrap_or(0.);
                    check(
                        "cached384",
                        cosine_similarity_f32_384_with_query_norm(&a, &b, norm),
                        -1.,
                    );
                }
                check("zero", cosine_similarity_f32(&a, &vec![0.; dim]), 0.);
            }
        }
    }

    #[test]
    fn 余弦缩放和抵消符合独立几何样例() {
        for (a, b, expected) in [
            ([3e20f32, 4e20], [1e-30, 0.], 0.6),
            ([3e-30, 4e-30], [1e20, 0.], 0.6),
            ([1e20, 1e20], [1e20, -1e20], 0.),
        ] {
            assert!((cosine_similarity_f32(&a, &b) - expected).abs() < 1e-6);
        }
        // 两个范数均正常，点积单独下溢；几何比例约为 1e-24，仍在 f32 表示范围内。
        let score = cosine_similarity_f32(&[1e-30, 1e-18, 0.], &[1e-30, 0., 1e-18]);
        assert!((score / 1e-24 - 1.).abs() < 1e-5, "点积下溢: {score}");
        assert!((f16::similarity(&[f16::MAX; 384], &[f16::MAX; 384]) - 1.).abs() < 2e-6);
    }

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
    fn dim384专用余弦与通用路径误差受控() {
        let query = (0..384)
            .map(|index| (index as f32 * 0.017).sin())
            .collect::<Vec<_>>();
        let query_norm = cosine_query_norm_f32_384(&query);
        if let Some(query_norm) = query_norm {
            for row in 0..1000 {
                let vector = (0..384)
                    .map(|index| ((row * 389 + index * 17) as f32 * 0.013).cos())
                    .collect::<Vec<_>>();
                let expected = cosine_similarity_f32(&query, &vector);
                let actual = cosine_similarity_f32_384_with_query_norm(&query, &vector, query_norm);
                assert!((actual - expected).abs() < 2e-6);
            }
        }
    }

    #[test]
    fn dim384专用余弦处理零向量() {
        let query = vec![0.0f32; 384];
        let vector = vec![1.0f32; 384];
        if let Some(query_norm) = cosine_query_norm_f32_384(&query) {
            assert_eq!(
                cosine_similarity_f32_384_with_query_norm(&query, &vector, query_norm),
                0.0
            );
        }
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
