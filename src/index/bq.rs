use crate::VectorType;
use bytemuck::{Pod, Zeroable};

/// 512-bit (64-byte) 二进制量化指纹 (Binary Quantization Fingerprint)
///
/// 标准 1-bit LSH 实现，将 f32 向量降维到 512 位。
/// 使用 XOR + Popcount 计算 Hamming 距离。
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Pod, Zeroable, Default)]
pub struct BqSignature {
    pub data: [u64; 32],
}

impl BqSignature {
    /// 预分配一个全零签名
    pub fn empty() -> Self {
        Self::default()
    }

    /// 1-bit 二值量化：> 0.0 的维度置为 1
    pub fn from_vector<T: VectorType>(vec: &[T]) -> Self {
        let mut data = [0u64; 32];
        for i in 0..32 {
            let mut chunk_bits = 0u64;
            for j in 0..64 {
                let idx = i * 64 + j;
                if idx < vec.len() && vec[idx].to_f32() > 0.0 {
                    chunk_bits |= 1u64 << j;
                }
            }
            data[i] = chunk_bits;
        }
        Self { data }
    }

    /// Hamming 距离
    #[inline]
    pub fn hamming_distance(&self, other: &Self) -> u32 {
        self.data
            .iter()
            .zip(other.data.iter())
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    }
}

// ═════════════════════════════════════════════════════════════
//  2-bit Sign-Magnitude BQ 签名
// ═════════════════════════════════════════════════════════════

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Pod, Zeroable, Default)]
pub struct Bq2Signature {
    pub pos: [u64; 32],
    pub strong: [u64; 32],
}

impl Bq2Signature {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_vector<T: crate::VectorType>(vec: &[T]) -> Self {
        let mut pos = [0u64; 32];
        let mut strong = [0u64; 32];

        let mut sum_abs = 0.0;
        for v in vec {
            sum_abs += v.to_f32().abs();
        }
        let alpha = if vec.is_empty() {
            0.0
        } else {
            sum_abs / vec.len() as f32
        };

        for i in 0..32 {
            let mut chunk_pos = 0u64;
            let mut chunk_strong = 0u64;
            for j in 0..64 {
                let idx = i * 64 + j;
                if idx < vec.len() {
                    let val = vec[idx].to_f32();
                    if val > 0.0 {
                        chunk_pos |= 1u64 << j;
                    }
                    if val.abs() > alpha {
                        chunk_strong |= 1u64 << j;
                    }
                }
            }
            pos[i] = chunk_pos;
            strong[i] = chunk_strong;
        }
        Self { pos, strong }
    }

    /// 2-bit 加权 Hamming 距离
    ///
    /// 运行时自动选择最优实现：
    ///   - AVX-512 VPOPCNTDQ（如果 CPU 支持）
    ///   - 标量回退（所有 x86_64 CPU）
    #[inline]
    pub fn distance(&self, other: &Self, dim: usize) -> u32 {
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx512vpopcntdq") && is_x86_feature_detected!("avx512f") {
                return unsafe { self.distance_avx512(other, dim) };
            }
        }
        self.distance_scalar(other, dim)
    }

    /// 标量回退路径（所有平台通用）
    #[inline]
    fn distance_scalar(&self, other: &Self, dim: usize) -> u32 {
        let chunks = dim.div_ceil(64);
        let valid_bits_last = if dim.is_multiple_of(64) {
            !0u64
        } else {
            (1u64 << (dim % 64)) - 1
        };

        let mut dot = 0i32;
        for i in 0..chunks {
            let mask = if i == chunks - 1 {
                valid_bits_last
            } else {
                !0u64
            };
            let a1 = self.pos[i] & mask;
            let b1 = self.strong[i] & mask;
            let a2 = other.pos[i] & mask;
            let b2 = other.strong[i] & mask;

            let same = !(a1 ^ a2) & mask;
            let diff = (a1 ^ a2) & mask;
            let both_strong = b1 & b2 & mask;
            let one_strong = (b1 ^ b2) & mask;
            let both_weak = !(b1 | b2) & mask;

            let w4_pos = same & both_strong;
            let w4_neg = diff & both_strong;
            let w2_pos = same & one_strong;
            let w2_neg = diff & one_strong;
            let w1_pos = same & both_weak;
            let w1_neg = diff & both_weak;

            dot += 4 * w4_pos.count_ones() as i32;
            dot -= 4 * w4_neg.count_ones() as i32;
            dot += 2 * w2_pos.count_ones() as i32;
            dot -= 2 * w2_neg.count_ones() as i32;
            dot += w1_pos.count_ones() as i32;
            dot -= w1_neg.count_ones() as i32;
        }

        let max_dot = 4 * dim as i32;
        (max_dot - dot) as u32
    }

    /// AVX-512 VPOPCNTDQ 加速路径
    ///
    /// 核心思路：每次处理 8 个 u64（512 bit），用硬件 popcount 指令
    /// 将 12 个 chunk（dim=768）的循环压缩到 2 轮 512-bit 操作。
    ///
    /// 6 类权重的 popcount 结果分别累加到 __m512i 寄存器中，
    /// 最后做一次水平求和。
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx512f,avx512vpopcntdq")]
    unsafe fn distance_avx512(&self, other: &Self, dim: usize) -> u32 {
        unsafe {
            use std::arch::x86_64::*;

            let chunks = dim.div_ceil(64);
            let full_rounds = chunks / 8;
            let remainder = chunks % 8;

            let mut acc_w4_pos = _mm512_setzero_si512();
            let mut acc_w4_neg = _mm512_setzero_si512();
            let mut acc_w2_pos = _mm512_setzero_si512();
            let mut acc_w2_neg = _mm512_setzero_si512();
            let mut acc_w1_pos = _mm512_setzero_si512();
            let mut acc_w1_neg = _mm512_setzero_si512();

            let ones_mask = _mm512_set1_epi64(-1i64);

            for r in 0..full_rounds {
                let off = r * 8;
                let a1 = _mm512_loadu_si512(self.pos.as_ptr().add(off) as *const _);
                let b1 = _mm512_loadu_si512(self.strong.as_ptr().add(off) as *const _);
                let a2 = _mm512_loadu_si512(other.pos.as_ptr().add(off) as *const _);
                let b2 = _mm512_loadu_si512(other.strong.as_ptr().add(off) as *const _);

                let xor_ab = _mm512_xor_si512(a1, a2);
                let same = _mm512_xor_si512(xor_ab, ones_mask);
                let diff = xor_ab;

                let both_strong = _mm512_and_si512(b1, b2);
                let one_strong = _mm512_xor_si512(b1, b2);
                let b_or = _mm512_or_si512(b1, b2);
                let both_weak = _mm512_xor_si512(b_or, ones_mask);

                acc_w4_pos = _mm512_add_epi64(
                    acc_w4_pos,
                    _mm512_popcnt_epi64(_mm512_and_si512(same, both_strong)),
                );
                acc_w4_neg = _mm512_add_epi64(
                    acc_w4_neg,
                    _mm512_popcnt_epi64(_mm512_and_si512(diff, both_strong)),
                );
                acc_w2_pos = _mm512_add_epi64(
                    acc_w2_pos,
                    _mm512_popcnt_epi64(_mm512_and_si512(same, one_strong)),
                );
                acc_w2_neg = _mm512_add_epi64(
                    acc_w2_neg,
                    _mm512_popcnt_epi64(_mm512_and_si512(diff, one_strong)),
                );
                acc_w1_pos = _mm512_add_epi64(
                    acc_w1_pos,
                    _mm512_popcnt_epi64(_mm512_and_si512(same, both_weak)),
                );
                acc_w1_neg = _mm512_add_epi64(
                    acc_w1_neg,
                    _mm512_popcnt_epi64(_mm512_and_si512(diff, both_weak)),
                );
            }

            let hsum = |v: __m512i| -> i64 { _mm512_reduce_add_epi64(v) };

            let mut dot = 0i64;
            dot += 4 * hsum(acc_w4_pos);
            dot -= 4 * hsum(acc_w4_neg);
            dot += 2 * hsum(acc_w2_pos);
            dot -= 2 * hsum(acc_w2_neg);
            dot += hsum(acc_w1_pos);
            dot -= hsum(acc_w1_neg);

            // 余数部分（标量）
            if remainder > 0 {
                let start = full_rounds * 8;
                let valid_bits_last = if dim.is_multiple_of(64) {
                    !0u64
                } else {
                    (1u64 << (dim % 64)) - 1
                };
                for i in start..chunks {
                    let mask = if i == chunks - 1 {
                        valid_bits_last
                    } else {
                        !0u64
                    };
                    let a1 = self.pos[i] & mask;
                    let b1 = self.strong[i] & mask;
                    let a2 = other.pos[i] & mask;
                    let b2 = other.strong[i] & mask;

                    let same = !(a1 ^ a2) & mask;
                    let diff = (a1 ^ a2) & mask;
                    let both_strong = b1 & b2 & mask;
                    let one_strong = (b1 ^ b2) & mask;
                    let both_weak = !(b1 | b2) & mask;

                    dot += 4 * (same & both_strong).count_ones() as i64;
                    dot -= 4 * (diff & both_strong).count_ones() as i64;
                    dot += 2 * (same & one_strong).count_ones() as i64;
                    dot -= 2 * (diff & one_strong).count_ones() as i64;
                    dot += (same & both_weak).count_ones() as i64;
                    dot -= (diff & both_weak).count_ones() as i64;
                }
            }

            let max_dot = 4 * dim as i64;
            (max_dot - dot) as u32
        }
    }
}
