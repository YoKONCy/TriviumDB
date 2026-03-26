use half::f16;
use std::fmt::Debug;

/// 定义通用向量类型的 Trait，支持多种引擎底层数据 (f32 / f16 / u64)
pub trait VectorType:
    Sized + Copy + Default + PartialEq + Debug + Send + Sync + bytemuck::Zeroable + bytemuck::Pod + 'static
{
    /// 计算两个等长特征切片之间的“相似度”得分。
    /// 返回值越大，表示越相近。
    fn similarity(a: &[Self], b: &[Self]) -> f32;

    /// 返回类型的零值（用于逻辑删除时清空底座）
    fn zero() -> Self;
}

// ════════ f32：普通高精度向量（余弦相似度） ════════
impl VectorType for f32 {
    #[inline]
    fn similarity(a: &[f32], b: &[f32]) -> f32 {
        let mut dot = 0.0;
        let mut norm_a = 0.0;
        let mut norm_b = 0.0;
        // 实际工业界此处应使用 AVX / std::simd 优化
        for (x, y) in a.iter().zip(b.iter()) {
            dot += x * y;
            norm_a += x * x;
            norm_b += y * y;
        }
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }

    #[inline]
    fn zero() -> Self { 0.0 }
}

// ════════ f16：半精度压缩向量（省 50% 内存） ════════
impl VectorType for f16 {
    #[inline]
    fn similarity(a: &[f16], b: &[f16]) -> f32 {
        let mut dot = 0.0;
        let mut norm_a = 0.0;
        let mut norm_b = 0.0;
        for (x, y) in a.iter().zip(b.iter()) {
            let xf = x.to_f32();
            let yf = y.to_f32();
            dot += xf * yf;
            norm_a += xf * xf;
            norm_b += yf * yf;
        }
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }

    #[inline]
    fn zero() -> Self { f16::from_f32(0.0) }
}

// ════════ u64：二进制哈希向量（如 SimHash / PEDSA ChaosFingerprint） ════════
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
    fn zero() -> Self { 0 }
}
