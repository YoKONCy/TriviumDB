//! 二值和两位签名与距离内核。

use super::*;
/// 二进制量化指纹 (Binary Quantization Fingerprint)
///
/// 标准 1-bit LSH 实现，将 f32 向量降维到位向量。
/// 使用 XOR + Popcount 计算 Hamming 距离。
///
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BqSignature {
    pub data: Vec<u64>,
}
impl BqSignature {
    /// 预分配一个全零签名
    pub fn empty() -> Self {
        Self::default()
    }

    /// 1-bit 二值量化：> 0.0 的维度置为 1
    pub fn from_vector<T: VectorType>(vec: &[T]) -> Self {
        let chunks = vec.len().div_ceil(64);
        let mut data = vec![0u64; chunks];
        for i in 0..chunks {
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bq2Signature {
    pub pos: Vec<u64>,
    pub strong: Vec<u64>,
}
impl Bq2Signature {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_vector<T: crate::index::pelorus::engine::VectorType>(vec: &[T]) -> Self {
        let chunks = vec.len().div_ceil(64);
        let mut pos = vec![0u64; chunks];
        let mut strong = vec![0u64; chunks];

        let mut sum_abs = 0.0;
        for v in vec {
            sum_abs += v.to_f32().abs();
        }
        let alpha = if vec.is_empty() {
            0.0
        } else {
            sum_abs / vec.len() as f32
        };

        for i in 0..chunks {
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

    /// 将单精度向量原地编码为 2-bit SM 符号位与强幅值位切片（零堆分配，逐比特与 `from_vector` 等价）。
    #[inline]
    pub(crate) fn encode_f32_slices(vec: &[f32], pos: &mut [u64], strong: &mut [u64]) {
        let chunks = vec.len().div_ceil(64);
        debug_assert!(pos.len() >= chunks && strong.len() >= chunks);
        let mut sum_abs = 0.0f32;
        for &v in vec {
            sum_abs += v.abs();
        }
        let alpha = if vec.is_empty() {
            0.0
        } else {
            sum_abs / vec.len() as f32
        };

        for i in 0..chunks {
            let mut chunk_pos = 0u64;
            let mut chunk_strong = 0u64;
            let base = i * 64;
            let take = (vec.len() - base).min(64);
            for j in 0..take {
                let val = vec[base + j];
                if val > 0.0 {
                    chunk_pos |= 1u64 << j;
                }
                if val.abs() > alpha {
                    chunk_strong |= 1u64 << j;
                }
            }
            pos[i] = chunk_pos;
            strong[i] = chunk_strong;
        }
    }

    /// 复用当前签名内部缓冲原地重编码单精度向量（消除热路径重复堆分配）。
    #[inline]
    pub(crate) fn encode_f32_into(&mut self, vec: &[f32]) {
        let chunks = vec.len().div_ceil(64);
        self.pos.resize(chunks, 0);
        self.strong.resize(chunks, 0);
        Self::encode_f32_slices(vec, &mut self.pos[..chunks], &mut self.strong[..chunks]);
    }

    /// 统计当前签名中幅值强位（strong）的置位数，用于恢复 2-bit 离散向量的精确范数。
    #[inline]
    pub fn strong_ones(&self) -> u32 {
        slice_popcnt_u64(&self.strong)
    }

    /// 2-bit 加权 Hamming 距离
    ///
    /// 运行时自动选择最优实现：
    ///   x86: AVX-512 VPOPCNTDQ → AVX2+POPCNT / 硬件 POPCNT → AVX2 → 标量
    ///   ARM: NEON (vcntq_u8) → 标量
    #[inline]
    pub fn distance(&self, other: &Self, dim: usize) -> u32 {
        if dim == 0 {
            return 0;
        }
        bq2_distance_raw(
            self.pos.as_ptr(),
            self.strong.as_ptr(),
            other.pos.as_ptr(),
            other.strong.as_ptr(),
            dim,
        )
    }

    /// 标量回退路径（所有平台通用）
    #[inline]
    pub(crate) fn distance_scalar(&self, other: &Self, dim: usize) -> u32 {
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
    #[cfg(all(target_arch = "x86_64", not(coverage)))]
    #[target_feature(enable = "avx512f,avx512vpopcntdq")]
    pub(crate) unsafe fn distance_avx512(&self, other: &Self, dim: usize) -> u32 {
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
/// 加速 u64 切片的置位总数统计（x86_64 硬件 POPCNT 路径）。
#[inline]
pub(crate) fn slice_popcnt_u64(words: &[u64]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    if !matches!(
        *X86_BIT_KERNEL,
        X86BitKernel::Scalar | X86BitKernel::Avx2Only
    ) {
        return unsafe { slice_popcnt_u64_hw(words) };
    }
    words.iter().map(|w| w.count_ones()).sum()
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "popcnt")]
pub(crate) unsafe fn slice_popcnt_u64_hw(words: &[u64]) -> u32 {
    let mut sum = 0u32;
    for &w in words {
        sum += w.count_ones();
    }
    sum
}
/// 零拷贝距离计算：直接在 pos/strong 指针上计算 2-bit 加权 Hamming 距离
///
/// 这是所有 BQ2 距离计算的核心热路径，避免 Bq2Signature 临时对象的创建。
/// 分发：AVX-512 VPOPCNTDQ → AVX2+POPCNT / 硬件 POPCNT → AVX2 → NEON → 标量
#[inline]
pub(crate) fn bq2_distance_raw(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> u32 {
    if dim == 0 {
        return 0;
    }
    #[cfg(target_arch = "x86_64")]
    {
        match *X86_BIT_KERNEL {
            #[cfg(not(coverage))]
            X86BitKernel::Avx512Vpopcnt => {
                if dim.is_multiple_of(64) {
                    return unsafe {
                        bq2_distance_raw_avx512(pos_a, strong_a, pos_b, strong_b, dim)
                    };
                }
                return unsafe { bq2_distance_raw_popcnt(pos_a, strong_a, pos_b, strong_b, dim) };
            }
            X86BitKernel::Avx2Popcnt => {
                if dim >= 512 && dim.is_multiple_of(64) {
                    return unsafe {
                        bq2_distance_raw_avx2_popcnt(pos_a, strong_a, pos_b, strong_b, dim)
                    };
                }
                return unsafe { bq2_distance_raw_popcnt(pos_a, strong_a, pos_b, strong_b, dim) };
            }
            X86BitKernel::Popcnt => {
                return unsafe { bq2_distance_raw_popcnt(pos_a, strong_a, pos_b, strong_b, dim) };
            }
            X86BitKernel::Avx2Only => {
                if dim.is_multiple_of(64) {
                    return unsafe { bq2_distance_raw_avx2(pos_a, strong_a, pos_b, strong_b, dim) };
                }
            }
            X86BitKernel::Scalar => {}
        }
        return bq2_distance_raw_scalar(pos_a, strong_a, pos_b, strong_b, dim);
    }
    #[cfg(all(target_arch = "aarch64", not(coverage)))]
    {
        if dim.is_multiple_of(64) {
            return unsafe { bq2_distance_raw_neon(pos_a, strong_a, pos_b, strong_b, dim) };
        }
    }
    #[allow(unreachable_code)]
    bq2_distance_raw_scalar(pos_a, strong_a, pos_b, strong_b, dim)
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "popcnt")]
pub(crate) unsafe fn bq2_distance_raw_popcnt(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> u32 {
    unsafe {
        if dim == 64 {
            let a1 = *pos_a;
            let b1 = *strong_a;
            let a2 = *pos_b;
            let b2 = *strong_b;
            let diff = a1 ^ a2;
            let same = !diff;
            let both_strong = b1 & b2;
            let one_strong = b1 ^ b2;
            let both_weak = !(b1 | b2);
            let dot = 4 * (same & both_strong).count_ones() as i32
                - 4 * (diff & both_strong).count_ones() as i32
                + 2 * (same & one_strong).count_ones() as i32
                - 2 * (diff & one_strong).count_ones() as i32
                + (same & both_weak).count_ones() as i32
                - (diff & both_weak).count_ones() as i32;
            return (256 - dot) as u32;
        }
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
            let a1 = *pos_a.add(i) & mask;
            let b1 = *strong_a.add(i) & mask;
            let a2 = *pos_b.add(i) & mask;
            let b2 = *strong_b.add(i) & mask;
            let diff = (a1 ^ a2) & mask;
            let same = !(a1 ^ a2) & mask;
            let both_strong = b1 & b2 & mask;
            let one_strong = (b1 ^ b2) & mask;
            let both_weak = !(b1 | b2) & mask;
            dot += 4 * (same & both_strong).count_ones() as i32;
            dot -= 4 * (diff & both_strong).count_ones() as i32;
            dot += 2 * (same & one_strong).count_ones() as i32;
            dot -= 2 * (diff & one_strong).count_ones() as i32;
            dot += (same & both_weak).count_ones() as i32;
            dot -= (diff & both_weak).count_ones() as i32;
        }
        (4 * dim as i32 - dot) as u32
    }
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,popcnt")]
pub(crate) unsafe fn bq2_distance_raw_avx2_popcnt(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> u32 {
    unsafe { bq2_distance_raw_avx2(pos_a, strong_a, pos_b, strong_b, dim) }
}
#[cfg(all(target_arch = "x86_64", test))]
#[target_feature(enable = "popcnt")]
pub(crate) unsafe fn bq2_distance_raw_popcnt_384(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
) -> u32 {
    let mut dot = 0i32;
    for index in 0..6 {
        let a1 = unsafe { *pos_a.add(index) };
        let b1 = unsafe { *strong_a.add(index) };
        let a2 = unsafe { *pos_b.add(index) };
        let b2 = unsafe { *strong_b.add(index) };
        let diff = a1 ^ a2;
        let same = !diff;
        let both_strong = b1 & b2;
        let one_strong = b1 ^ b2;
        let both_weak = !(b1 | b2);
        dot += 4 * (same & both_strong).count_ones() as i32;
        dot -= 4 * (diff & both_strong).count_ones() as i32;
        dot += 2 * (same & one_strong).count_ones() as i32;
        dot -= 2 * (diff & one_strong).count_ones() as i32;
        dot += (same & both_weak).count_ones() as i32;
        dot -= (diff & both_weak).count_ones() as i32;
    }

    (1536 - dot) as u32
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "popcnt")]
pub(crate) unsafe fn bq2_distance_cheap_popcnt_384(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
) -> u32 {
    let mut distance = 0u32;
    for index in 0..6 {
        distance += (unsafe { *pos_a.add(index) } ^ unsafe { *pos_b.add(index) }).count_ones();
        distance +=
            (unsafe { *strong_a.add(index) } ^ unsafe { *strong_b.add(index) }).count_ones();
    }
    distance
}
#[inline]
pub(crate) fn bq2_distance_raw_scalar(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> u32 {
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
        let a1 = unsafe { *pos_a.add(i) } & mask;
        let b1 = unsafe { *strong_a.add(i) } & mask;
        let a2 = unsafe { *pos_b.add(i) } & mask;
        let b2 = unsafe { *strong_b.add(i) } & mask;

        let same = !(a1 ^ a2) & mask;
        let diff = (a1 ^ a2) & mask;
        let both_strong = b1 & b2 & mask;
        let one_strong = (b1 ^ b2) & mask;
        let both_weak = !(b1 | b2) & mask;

        dot += 4 * (same & both_strong).count_ones() as i32;
        dot -= 4 * (diff & both_strong).count_ones() as i32;
        dot += 2 * (same & one_strong).count_ones() as i32;
        dot -= 2 * (diff & one_strong).count_ones() as i32;
        dot += (same & both_weak).count_ones() as i32;
        dot -= (diff & both_weak).count_ones() as i32;
    }
    let max_dot = 4 * dim as i32;
    (max_dot - dot) as u32
}
#[cfg(all(target_arch = "x86_64", not(coverage)))]
#[target_feature(enable = "avx512f,avx512vpopcntdq")]
pub(crate) unsafe fn bq2_distance_raw_avx512(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> u32 {
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
            let a1 = _mm512_loadu_si512(pos_a.add(off) as *const _);
            let b1 = _mm512_loadu_si512(strong_a.add(off) as *const _);
            let a2 = _mm512_loadu_si512(pos_b.add(off) as *const _);
            let b2 = _mm512_loadu_si512(strong_b.add(off) as *const _);

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
                let a1 = *pos_a.add(i) & mask;
                let b1 = *strong_a.add(i) & mask;
                let a2 = *pos_b.add(i) & mask;
                let b2 = *strong_b.add(i) & mask;
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
// ═════════════════════════════════════════════════════════════
//  AVX2 BQ2 距离（nibble-lookup popcount）
// ═════════════════════════════════════════════════════════════

/// AVX2 软件 popcount：nibble 查表法，返回每个 64-bit lane 的 popcount
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[inline]
pub(crate) unsafe fn popcnt256(v: std::arch::x86_64::__m256i) -> std::arch::x86_64::__m256i {
    use std::arch::x86_64::*;
    {
        let low_mask = _mm256_set1_epi8(0x0F);
        // 每个 nibble (0-15) 的 popcount 查找表
        let lookup = _mm256_setr_epi8(
            0, 1, 1, 2, 1, 2, 2, 3, 1, 2, 2, 3, 2, 3, 3, 4, 0, 1, 1, 2, 1, 2, 2, 3, 1, 2, 2, 3, 2,
            3, 3, 4,
        );
        let lo = _mm256_and_si256(v, low_mask);
        let hi = _mm256_and_si256(_mm256_srli_epi16(v, 4), low_mask);
        let cnt = _mm256_add_epi8(
            _mm256_shuffle_epi8(lookup, lo),
            _mm256_shuffle_epi8(lookup, hi),
        );
        // 字节级 popcount → 每 64-bit lane 求和（sad_epu8 对零做差的绝对值求和）
        _mm256_sad_epu8(cnt, _mm256_setzero_si256())
    }
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn bq2_distance_raw_avx2(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> u32 {
    use std::arch::x86_64::*;
    unsafe {
        let chunks = dim.div_ceil(64);
        let full_rounds = chunks / 4;
        let remainder = chunks % 4;

        let mut acc_w4_pos = _mm256_setzero_si256();
        let mut acc_w4_neg = _mm256_setzero_si256();
        let mut acc_w2_pos = _mm256_setzero_si256();
        let mut acc_w2_neg = _mm256_setzero_si256();
        let mut acc_w1_pos = _mm256_setzero_si256();
        let mut acc_w1_neg = _mm256_setzero_si256();
        let ones = _mm256_set1_epi64x(-1i64);

        for r in 0..full_rounds {
            let off = r * 4;
            let a1 = _mm256_loadu_si256(pos_a.add(off) as *const __m256i);
            let b1 = _mm256_loadu_si256(strong_a.add(off) as *const __m256i);
            let a2 = _mm256_loadu_si256(pos_b.add(off) as *const __m256i);
            let b2 = _mm256_loadu_si256(strong_b.add(off) as *const __m256i);

            let xor_ab = _mm256_xor_si256(a1, a2);
            let same = _mm256_xor_si256(xor_ab, ones);
            let diff = xor_ab;
            let both_strong = _mm256_and_si256(b1, b2);
            let one_strong = _mm256_xor_si256(b1, b2);
            let both_weak = _mm256_xor_si256(_mm256_or_si256(b1, b2), ones);

            acc_w4_pos =
                _mm256_add_epi64(acc_w4_pos, popcnt256(_mm256_and_si256(same, both_strong)));
            acc_w4_neg =
                _mm256_add_epi64(acc_w4_neg, popcnt256(_mm256_and_si256(diff, both_strong)));
            acc_w2_pos =
                _mm256_add_epi64(acc_w2_pos, popcnt256(_mm256_and_si256(same, one_strong)));
            acc_w2_neg =
                _mm256_add_epi64(acc_w2_neg, popcnt256(_mm256_and_si256(diff, one_strong)));
            acc_w1_pos = _mm256_add_epi64(acc_w1_pos, popcnt256(_mm256_and_si256(same, both_weak)));
            acc_w1_neg = _mm256_add_epi64(acc_w1_neg, popcnt256(_mm256_and_si256(diff, both_weak)));
        }

        // 水平归约：4 个 i64 lane 求和
        let hsum = |v: __m256i| -> i64 {
            let hi = _mm256_extracti128_si256(v, 1);
            let lo = _mm256_castsi256_si128(v);
            let s = _mm_add_epi64(lo, hi);
            let hi64 = _mm_srli_si128(s, 8);
            _mm_cvtsi128_si64(_mm_add_epi64(s, hi64))
        };

        let mut dot = 0i64;
        dot += 4 * hsum(acc_w4_pos);
        dot -= 4 * hsum(acc_w4_neg);
        dot += 2 * hsum(acc_w2_pos);
        dot -= 2 * hsum(acc_w2_neg);
        dot += hsum(acc_w1_pos);
        dot -= hsum(acc_w1_neg);

        // 余数（标量）
        if remainder > 0 {
            let start = full_rounds * 4;
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
                let a1 = *pos_a.add(i) & mask;
                let b1 = *strong_a.add(i) & mask;
                let a2 = *pos_b.add(i) & mask;
                let b2 = *strong_b.add(i) & mask;
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
// ═════════════════════════════════════════════════════════════
//  NEON BQ2 距离（vcntq_u8 硬件 popcount）
// ═════════════════════════════════════════════════════════════

#[cfg(all(target_arch = "aarch64", not(coverage)))]
#[target_feature(enable = "neon")]
pub(crate) unsafe fn bq2_distance_raw_neon(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> u32 {
    use std::arch::aarch64::*;
    unsafe {
        let chunks = dim.div_ceil(64);

        // NEON popcount：字节级 vcnt → 逐级宽化求和 → 得到每 64-bit lane 的 popcount
        let popcnt128 = |v: uint64x2_t| -> int64x2_t {
            let bytes = vcntq_u8(vreinterpretq_u8_u64(v)); // 每字节 popcount
            let h16 = vpaddlq_u8(bytes); // u8→u16 对加
            let h32 = vpaddlq_u16(h16); // u16→u32 对加
            vreinterpretq_s64_u64(vpaddlq_u32(h32)) // u32→u64 对加
        };

        let mut dot = 0i64;
        let valid_bits_last = if dim.is_multiple_of(64) {
            !0u64
        } else {
            (1u64 << (dim % 64)) - 1
        };

        // 每次处理 2 个 u64 chunk
        let full_rounds = if dim.is_multiple_of(64) {
            chunks / 2
        } else {
            chunks.saturating_sub(1) / 2
        };
        let mut acc_w4_pos = vdupq_n_s64(0);
        let mut acc_w4_neg = vdupq_n_s64(0);
        let mut acc_w2_pos = vdupq_n_s64(0);
        let mut acc_w2_neg = vdupq_n_s64(0);
        let mut acc_w1_pos = vdupq_n_s64(0);
        let mut acc_w1_neg = vdupq_n_s64(0);

        for r in 0..full_rounds {
            let off = r * 2;
            let a1 = vld1q_u64(pos_a.add(off));
            let b1 = vld1q_u64(strong_a.add(off));
            let a2 = vld1q_u64(pos_b.add(off));
            let b2 = vld1q_u64(strong_b.add(off));

            let xor_ab = veorq_u64(a1, a2);
            let ones = vdupq_n_u64(!0u64);
            let same = veorq_u64(xor_ab, ones);
            let diff = xor_ab;
            let both_strong = vandq_u64(b1, b2);
            let one_strong = veorq_u64(b1, b2);
            let both_weak = veorq_u64(vorrq_u64(b1, b2), ones);

            acc_w4_pos = vaddq_s64(acc_w4_pos, popcnt128(vandq_u64(same, both_strong)));
            acc_w4_neg = vaddq_s64(acc_w4_neg, popcnt128(vandq_u64(diff, both_strong)));
            acc_w2_pos = vaddq_s64(acc_w2_pos, popcnt128(vandq_u64(same, one_strong)));
            acc_w2_neg = vaddq_s64(acc_w2_neg, popcnt128(vandq_u64(diff, one_strong)));
            acc_w1_pos = vaddq_s64(acc_w1_pos, popcnt128(vandq_u64(same, both_weak)));
            acc_w1_neg = vaddq_s64(acc_w1_neg, popcnt128(vandq_u64(diff, both_weak)));
        }

        // 水平归约
        let hsum = |v: int64x2_t| -> i64 { vaddvq_s64(v) };
        dot += 4 * hsum(acc_w4_pos);
        dot -= 4 * hsum(acc_w4_neg);
        dot += 2 * hsum(acc_w2_pos);
        dot -= 2 * hsum(acc_w2_neg);
        dot += hsum(acc_w1_pos);
        dot -= hsum(acc_w1_neg);

        // 余数（标量）
        let start = full_rounds * 2;
        for i in start..chunks {
            let mask = if i == chunks - 1 {
                valid_bits_last
            } else {
                !0u64
            };
            let a1 = *pos_a.add(i) & mask;
            let b1 = *strong_a.add(i) & mask;
            let a2 = *pos_b.add(i) & mask;
            let b2 = *strong_b.add(i) & mask;
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
        let max_dot = 4 * dim as i64;
        (max_dot - dot) as u32
    }
}
#[cfg(all(target_arch = "x86_64", not(coverage)))]
#[target_feature(enable = "avx512f,avx512vpopcntdq")]
pub(crate) unsafe fn bq2_distance_cheap_avx512_768(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
) -> u32 {
    unsafe {
        use std::arch::x86_64::*;

        let pa0 = _mm512_loadu_si512(pos_a as *const _);
        let qa0 = _mm512_loadu_si512(pos_b as *const _);
        let sa0 = _mm512_loadu_si512(strong_a as *const _);
        let qs0 = _mm512_loadu_si512(strong_b as *const _);
        let pa1 = _mm256_loadu_si256(pos_a.add(8) as *const _);
        let qa1 = _mm256_loadu_si256(pos_b.add(8) as *const _);
        let sa1 = _mm256_loadu_si256(strong_a.add(8) as *const _);
        let qs1 = _mm256_loadu_si256(strong_b.add(8) as *const _);

        let acc512 = _mm512_add_epi64(
            _mm512_popcnt_epi64(_mm512_xor_si512(pa0, qa0)),
            _mm512_popcnt_epi64(_mm512_xor_si512(sa0, qs0)),
        );
        let acc256 = _mm256_add_epi64(
            _mm256_popcnt_epi64(_mm256_xor_si256(pa1, qa1)),
            _mm256_popcnt_epi64(_mm256_xor_si256(sa1, qs1)),
        );

        let acc256_as512 = _mm512_castsi256_si512(acc256);
        (_mm512_reduce_add_epi64(acc512) + _mm512_reduce_add_epi64(acc256_as512)) as u32
    }
}
#[cfg(all(target_arch = "x86_64", not(coverage)))]
#[target_feature(enable = "avx512f,avx512vpopcntdq")]
pub(crate) unsafe fn bq2_distance_cheap_avx512(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> u32 {
    unsafe {
        use std::arch::x86_64::*;

        let chunks = dim.div_ceil(64);
        let full_rounds = chunks / 8;
        let remainder = chunks % 8;
        let mut acc_pos = _mm512_setzero_si512();
        let mut acc_strong = _mm512_setzero_si512();

        for r in 0..full_rounds {
            let off = r * 8;
            let pa = _mm512_loadu_si512(pos_a.add(off) as *const _);
            let qa = _mm512_loadu_si512(pos_b.add(off) as *const _);
            let sa = _mm512_loadu_si512(strong_a.add(off) as *const _);
            let qs = _mm512_loadu_si512(strong_b.add(off) as *const _);
            acc_pos = _mm512_add_epi64(acc_pos, _mm512_popcnt_epi64(_mm512_xor_si512(pa, qa)));
            acc_strong =
                _mm512_add_epi64(acc_strong, _mm512_popcnt_epi64(_mm512_xor_si512(sa, qs)));
        }

        let mut pos_dist = _mm512_reduce_add_epi64(acc_pos) as u32;
        let mut strong_dist = _mm512_reduce_add_epi64(acc_strong) as u32;

        if remainder > 0 {
            let start = full_rounds * 8;
            for i in start..chunks {
                pos_dist += (*pos_a.add(i) ^ *pos_b.add(i)).count_ones();
                strong_dist += (*strong_a.add(i) ^ *strong_b.add(i)).count_ones();
            }
        }

        pos_dist + strong_dist
    }
}
