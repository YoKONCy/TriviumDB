use half::f16;
use std::fmt::Debug;

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

    /// 将单个元素转换为 f32（用于 HNSW 索引等需要统一浮点表示的场景）
    fn to_f32(self) -> f32;

    /// 从 f32 构造单元素（用于产生数学计算后的残差向量等机制）
    fn from_f32(v: f32) -> Self;
}

// ════════ SIMD 内核：AVX2 + FMA 余弦相似度 ════════

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

/// AVX2 + FMA 加速路径：每次并行处理 8 个 f32
/// 安全契约：调用方须确保 CPU 支持 avx2 + fma
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2", enable = "fma")]
unsafe fn cosine_similarity_avx2(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    let len = a.len().min(b.len());

    // SAFETY: 调用方已通过运行时检测确认 CPU 支持 avx2 + fma
    unsafe {
        let mut v_dot = _mm256_setzero_ps();
        let mut v_na = _mm256_setzero_ps();
        let mut v_nb = _mm256_setzero_ps();

        let chunks = len / 8;
        for i in 0..chunks {
            let offset = i * 8;
            let va = _mm256_loadu_ps(a.as_ptr().add(offset));
            let vb = _mm256_loadu_ps(b.as_ptr().add(offset));
            v_dot = _mm256_fmadd_ps(va, vb, v_dot); // dot += a * b
            v_na = _mm256_fmadd_ps(va, va, v_na); // na  += a * a
            v_nb = _mm256_fmadd_ps(vb, vb, v_nb); // nb  += b * b
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
        // 128-bit 内部水平加：[a,b,c,d] → hadd → [a+b,c+d,...] → hadd → [a+b+c+d,...]
        let s_dot = _mm_add_ps(_mm_hadd_ps(s_dot, s_dot), _mm_setzero_ps());
        let s_dot = _mm_hadd_ps(s_dot, s_dot);
        let s_na = _mm_hadd_ps(_mm_hadd_ps(s_na, s_na), _mm_hadd_ps(s_na, s_na));
        let s_nb = _mm_hadd_ps(_mm_hadd_ps(s_nb, s_nb), _mm_hadd_ps(s_nb, s_nb));

        let mut dot = _mm_cvtss_f32(s_dot);
        let mut norm_a = _mm_cvtss_f32(s_na);
        let mut norm_b = _mm_cvtss_f32(s_nb);

        // 处理尾部不足 8 个的元素
        let tail_start = chunks * 8;
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
#[inline]
pub fn cosine_similarity_f32(a: &[f32], b: &[f32]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            // SAFETY: 已通过运行时检测确认 CPU 支持 avx2 + fma
            return unsafe { cosine_similarity_avx2(a, b) };
        }
    }
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
