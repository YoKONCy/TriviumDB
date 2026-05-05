use crate::VectorType;
use bytemuck::{Pod, Zeroable};

/// BQ 签名最大 chunks 数量（每个 u64 chunk 覆盖 64 维）
/// 32 chunks × 64 bits = 2048 维上限
const MAX_BQ_CHUNKS: usize = 32;

/// 二进制量化指纹 (Binary Quantization Fingerprint)
///
/// 标准 1-bit LSH 实现，将 f32 向量降维到位向量。
/// 使用 XOR + Popcount 计算 Hamming 距离。
///
/// 最大支持 2048 维（32 × 64 bits）。
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Pod, Zeroable, Default)]
pub struct BqSignature {
    pub data: [u64; MAX_BQ_CHUNKS],
}

impl BqSignature {
    /// 最大 chunks 数量
    pub const MAX_CHUNKS: usize = MAX_BQ_CHUNKS;

    /// 预分配一个全零签名
    pub fn empty() -> Self {
        Self::default()
    }

    /// 1-bit 二值量化：> 0.0 的维度置为 1
    pub fn from_vector<T: VectorType>(vec: &[T]) -> Self {
        let mut data = [0u64; MAX_BQ_CHUNKS];
        let chunks = vec.len().div_ceil(64).min(MAX_BQ_CHUNKS);
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

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Pod, Zeroable, Default)]
pub struct Bq2Signature {
    pub pos: [u64; MAX_BQ_CHUNKS],
    pub strong: [u64; MAX_BQ_CHUNKS],
}

impl Bq2Signature {
    /// 最大 chunks 数量
    pub const MAX_CHUNKS: usize = MAX_BQ_CHUNKS;

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_vector<T: crate::VectorType>(vec: &[T]) -> Self {
        let mut pos = [0u64; MAX_BQ_CHUNKS];
        let mut strong = [0u64; MAX_BQ_CHUNKS];

        let mut sum_abs = 0.0;
        for v in vec {
            sum_abs += v.to_f32().abs();
        }
        let alpha = if vec.is_empty() {
            0.0
        } else {
            sum_abs / vec.len() as f32
        };

        let chunks = vec.len().div_ceil(64).min(MAX_BQ_CHUNKS);
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

/// 零拷贝距离计算：直接在 pos/strong 指针上计算 2-bit 加权 Hamming 距离
///
/// 这是所有 BQ2 距离计算的核心热路径，避免 Bq2Signature 临时对象的创建。
#[inline]
pub fn bq2_distance_raw(
    pos_a: *const u64, strong_a: *const u64,
    pos_b: *const u64, strong_b: *const u64,
    dim: usize,
) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512vpopcntdq") && is_x86_feature_detected!("avx512f") {
            return unsafe { bq2_distance_raw_avx512(pos_a, strong_a, pos_b, strong_b, dim) };
        }
    }
    bq2_distance_raw_scalar(pos_a, strong_a, pos_b, strong_b, dim)
}

#[inline]
fn bq2_distance_raw_scalar(
    pos_a: *const u64, strong_a: *const u64,
    pos_b: *const u64, strong_b: *const u64,
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
        let mask = if i == chunks - 1 { valid_bits_last } else { !0u64 };
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

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx512vpopcntdq")]
unsafe fn bq2_distance_raw_avx512(
    pos_a: *const u64, strong_a: *const u64,
    pos_b: *const u64, strong_b: *const u64,
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

            acc_w4_pos = _mm512_add_epi64(acc_w4_pos, _mm512_popcnt_epi64(_mm512_and_si512(same, both_strong)));
            acc_w4_neg = _mm512_add_epi64(acc_w4_neg, _mm512_popcnt_epi64(_mm512_and_si512(diff, both_strong)));
            acc_w2_pos = _mm512_add_epi64(acc_w2_pos, _mm512_popcnt_epi64(_mm512_and_si512(same, one_strong)));
            acc_w2_neg = _mm512_add_epi64(acc_w2_neg, _mm512_popcnt_epi64(_mm512_and_si512(diff, one_strong)));
            acc_w1_pos = _mm512_add_epi64(acc_w1_pos, _mm512_popcnt_epi64(_mm512_and_si512(same, both_weak)));
            acc_w1_neg = _mm512_add_epi64(acc_w1_neg, _mm512_popcnt_epi64(_mm512_and_si512(diff, both_weak)));
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
            let valid_bits_last = if dim.is_multiple_of(64) { !0u64 } else { (1u64 << (dim % 64)) - 1 };
            for i in start..chunks {
                let mask = if i == chunks - 1 { valid_bits_last } else { !0u64 };
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
//  紧凑型 BQ2 签名存储（Struct-of-Arrays）
// ═════════════════════════════════════════════════════════════

/// 紧凑型 BQ2 签名存储
///
/// 将 N 个签名存储为两个 flat `Vec<u64>` 数组，
/// 每个签名只占用 `chunks × 8` 字节，消除固定大小结构体的零填充浪费。
pub struct Bq2Store {
    pos: Vec<u64>,
    strong: Vec<u64>,
    /// 每个签名的 u64 chunk 数量（= dim.div_ceil(64)）
    chunks: usize,
    /// 已存储的签名数量
    n: usize,
}

impl Bq2Store {
    /// 创建空的紧凑存储
    pub fn new(dim: usize) -> Self {
        let chunks = if dim == 0 { 1 } else { dim.div_ceil(64) };
        Self { pos: Vec::new(), strong: Vec::new(), chunks, n: 0 }
    }

    pub fn chunks(&self) -> usize { self.chunks }
    pub fn len(&self) -> usize { self.n }
    pub fn is_empty(&self) -> bool { self.n == 0 }

    /// 预分配空间
    pub fn reserve(&mut self, additional: usize) {
        self.pos.reserve(additional * self.chunks);
        self.strong.reserve(additional * self.chunks);
    }

    /// 从向量直接编码并追加（紧凑路径）
    pub fn push_from_vector<T: crate::VectorType>(&mut self, vec: &[T]) {
        let mut sum_abs = 0.0f32;
        for v in vec { sum_abs += v.to_f32().abs(); }
        let alpha = if vec.is_empty() { 0.0 } else { sum_abs / vec.len() as f32 };

        for i in 0..self.chunks {
            let mut cp = 0u64;
            let mut cs = 0u64;
            for j in 0..64 {
                let idx = i * 64 + j;
                if idx < vec.len() {
                    let val = vec[idx].to_f32();
                    if val > 0.0 { cp |= 1u64 << j; }
                    if val.abs() > alpha { cs |= 1u64 << j; }
                }
            }
            self.pos.push(cp);
            self.strong.push(cs);
        }
        self.n += 1;
    }

    /// 追加已有的 Bq2Signature（兼容路径）
    pub fn push_sig(&mut self, sig: &Bq2Signature) {
        self.pos.extend_from_slice(&sig.pos[..self.chunks]);
        self.strong.extend_from_slice(&sig.strong[..self.chunks]);
        self.n += 1;
    }

    /// 重建完整的 Bq2Signature（栈上临时对象）
    #[inline]
    pub fn get_sig(&self, idx: usize) -> Bq2Signature {
        let mut sig = Bq2Signature::empty();
        let off = idx * self.chunks;
        sig.pos[..self.chunks].copy_from_slice(&self.pos[off..off + self.chunks]);
        sig.strong[..self.chunks].copy_from_slice(&self.strong[off..off + self.chunks]);
        sig
    }

    /// 预取第 idx 个签名到 L1 cache（减少 beam search 中的 cache miss）
    #[inline]
    pub fn prefetch_sig(&self, idx: usize) {
        let off = idx * self.chunks;
        // 预取 pos 和 strong 的起始 cache line（64 bytes = 8 个 u64）
        // 对 768-d (12 chunks)，pos 96B = 2 cache lines，strong 96B = 2 cache lines
        unsafe {
            #[cfg(target_arch = "x86_64")]
            {
                use std::arch::x86_64::*;
                let pos_ptr = self.pos.as_ptr().add(off) as *const i8;
                let strong_ptr = self.strong.as_ptr().add(off) as *const i8;
                _mm_prefetch(pos_ptr, _MM_HINT_T0);
                _mm_prefetch(strong_ptr, _MM_HINT_T0);
                // 对高维向量（>512-d = >8 chunks），预取第二条 cache line
                if self.chunks > 8 {
                    _mm_prefetch(pos_ptr.add(64), _MM_HINT_T0);
                    _mm_prefetch(strong_ptr.add(64), _MM_HINT_T0);
                }
            }
            #[cfg(target_arch = "aarch64")]
            {
                // ARM64 使用 prefetch hint
                let pos_ptr = self.pos.as_ptr().add(off);
                let strong_ptr = self.strong.as_ptr().add(off);
                std::arch::asm!("prfm pldl1keep, [{0}]", in(reg) pos_ptr, options(nostack, preserves_flags));
                std::arch::asm!("prfm pldl1keep, [{0}]", in(reg) strong_ptr, options(nostack, preserves_flags));
            }
        }
    }

    /// 计算存储中第 idx 个签名与外部签名的距离（零拷贝）
    #[inline]
    pub fn distance_to_sig(&self, idx: usize, other: &Bq2Signature, dim: usize) -> u32 {
        let off = idx * self.chunks;
        bq2_distance_raw(
            self.pos[off..].as_ptr(), self.strong[off..].as_ptr(),
            other.pos.as_ptr(), other.strong.as_ptr(),
            dim,
        )
    }

    /// 计算两个存储中签名的距离（零拷贝）
    #[inline]
    pub fn distance(&self, i: usize, j: usize, dim: usize) -> u32 {
        let off_i = i * self.chunks;
        let off_j = j * self.chunks;
        bq2_distance_raw(
            self.pos[off_i..].as_ptr(), self.strong[off_i..].as_ptr(),
            self.pos[off_j..].as_ptr(), self.strong[off_j..].as_ptr(),
            dim,
        )
    }

    /// 紧凑 hot 内存占用（字节）
    pub fn hot_bytes(&self) -> usize {
        (self.pos.len() + self.strong.len()) * 8
    }

    /// 底层数据访问（序列化用）
    pub fn pos_data(&self) -> &[u64] { &self.pos }
    pub fn strong_data(&self) -> &[u64] { &self.strong }

    /// 从裸数据恢复（反序列化用）
    pub fn from_raw(pos: Vec<u64>, strong: Vec<u64>, chunks: usize) -> Self {
        assert_eq!(pos.len(), strong.len(), "pos/strong 长度必须一致");
        let n = if chunks > 0 { pos.len() / chunks } else { 0 };
        Self { pos, strong, chunks, n }
    }
}
