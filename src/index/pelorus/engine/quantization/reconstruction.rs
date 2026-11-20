//! 符号幅值重构参数与位计数。

use super::*;
// ═════════════════════════════════════════════════════════════
//  无训练两级符号-幅值（Sign-Magnitude, SM）球面方向重构内核
// ═════════════════════════════════════════════════════════════

/// 单位方向 S^{d-1} 上的无训练两级符号-幅值（SM）重构参数。
///
/// 对非零向量 x，记 r_x = ||x||_2，u_x = x / r_x，strong 位将坐标划分为弱组 (h_j=0) 与强组 (h_j=1)。
/// 两级条件均值重构 u_hat_{xj} = sgn_+(x_j) * (weak + delta * h_j) 满足精确正交投影恒等式：
/// <u_x, u_hat_x> = ||u_hat_x||_2^2 = sigma^2 ∈ [0, 1]，残差范数平方为 1 - sigma^2。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SmCodeParams {
    /// 单位方向弱幅值组条件算术均值 a_x >= 0
    pub(crate) weak: f32,
    /// 单位方向强弱幅值组均值差 delta_x = b_x - a_x >= 0
    pub(crate) delta: f32,
    /// 单位方向重构范数 sigma_x = sqrt(||u_hat_x||_2^2) ∈ [0, 1]
    pub(crate) sigma: f32,
    /// 强幅值位置位数 s_x = popcnt(strong)
    pub(crate) strong_ones: u32,
}
impl SmCodeParams {
    /// 从原始向量与对应的 strong 位切片单遍计算精确欧氏半径 r_x 与单位方向 SM 参数（零堆分配）。
    #[inline]
    pub(crate) fn from_vector_and_strong_words(
        vector: &[f32],
        strong_words: &[u64],
    ) -> (Self, f64) {
        let dim = vector.len();
        if dim == 0 {
            return (Self::default(), 0.0);
        }
        let mut norm_sq = 0.0f64;
        let mut sums = [0.0f64; 2];
        let mut strong_cnt = 0usize;

        let full_chunks = dim / 64;
        for chunk_idx in 0..full_chunks {
            let sw = strong_words[chunk_idx];
            strong_cnt += sw.count_ones() as usize;
            let base = chunk_idx * 64;
            for bit in 0..64 {
                let v = f64::from(vector[base + bit]);
                norm_sq += v * v;
                let g = ((sw >> bit) & 1) as usize;
                sums[g] += v.abs();
            }
        }
        let rem_base = full_chunks * 64;
        if rem_base < dim {
            let sw = strong_words[full_chunks];
            for bit in 0..(dim - rem_base) {
                let v = f64::from(vector[rem_base + bit]);
                norm_sq += v * v;
                let g = ((sw >> bit) & 1) as usize;
                strong_cnt += g;
                sums[g] += v.abs();
            }
        }

        let radius = norm_sq.sqrt();
        if radius <= 0.0 || !radius.is_finite() {
            return (
                Self {
                    weak: 0.0,
                    delta: 0.0,
                    sigma: 0.0,
                    strong_ones: strong_cnt as u32,
                },
                0.0,
            );
        }
        let inv_r = 1.0 / radius;
        let weak_cnt = dim - strong_cnt;
        let a = if weak_cnt > 0 {
            (sums[0] * inv_r) / (weak_cnt as f64)
        } else {
            0.0
        };
        let b = if strong_cnt > 0 {
            (sums[1] * inv_r) / (strong_cnt as f64)
        } else {
            0.0
        };
        let weak = a as f32;
        let delta = (b - a) as f32;
        let high = weak as f64 + delta as f64;
        let eta =
            (weak_cnt as f64) * (weak as f64) * (weak as f64) + (strong_cnt as f64) * high * high;
        let sigma = if eta > 0.0 { eta.sqrt() as f32 } else { 0.0 };
        (
            Self {
                weak,
                delta,
                sigma,
                strong_ones: strong_cnt as u32,
            },
            radius,
        )
    }

    /// 计算两级重构向量 q_x 的精确 f64 欧氏范数平方 ||q_x||_2^2 = (dim - s_x) w_x^2 + s_x (w_x + delta_x)^2。
    #[inline]
    pub(crate) fn exact_norm_sq(self, dim: usize) -> f64 {
        let s = (self.strong_ones as usize).min(dim);
        let w = f64::from(self.weak);
        let h = w + f64::from(self.delta);
        ((dim - s) as f64) * w * w + (s as f64) * h * h
    }

    /// 从原始向量与外部 Bq2Signature 计算单位方向 SM 参数与精确欧氏半径。
    #[inline]
    pub(crate) fn from_vector_and_sig(vector: &[f32], sig: &Bq2Signature) -> (Self, f64) {
        Self::from_vector_and_strong_words(vector, &sig.strong)
    }

    /// 从原始向量与 Bq2Store 指定槽位零拷贝计算单位方向 SM 参数与精确欧氏半径。
    #[inline]
    pub(crate) fn from_vector_and_store(
        vector: &[f32],
        store: &Bq2Store,
        idx: usize,
    ) -> (Self, f64) {
        Self::from_vector_and_strong_words(vector, store.strong_slice(idx))
    }

    /// 由 5-popcount 位计数还原两端重构单位向量的精确内积 <u_hat_a, u_hat_b>。
    #[inline]
    pub(crate) fn reconstructed_dot(self, other: Self, counts: [u32; 5], dim: usize) -> f64 {
        let c0 = dim as i64 - 2 * i64::from(counts[0]);
        let ca = i64::from(self.strong_ones) - 2 * i64::from(counts[1]);
        let cb = i64::from(other.strong_ones) - 2 * i64::from(counts[2]);
        let cab = i64::from(counts[3]) - 2 * i64::from(counts[4]);
        let wa = f64::from(self.weak);
        let da = f64::from(self.delta);
        let wb = f64::from(other.weak);
        let db = f64::from(other.delta);
        wa * (wb * (c0 as f64) + db * (cb as f64)) + da * (wb * (ca as f64) + db * (cab as f64))
    }
}
/// 零拷贝 5-popcount 内核：返回
/// `[popcnt(M), popcnt(M & h_a), popcnt(M & h_b), popcnt(h_a & h_b), popcnt(M & h_a & h_b)]`，
/// 其中 `M = pos_a ^ pos_b`，尾部非整块严格按 `dim` 掩码截断。
#[inline]
pub(crate) fn sm_dot_counts_raw(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> [u32; 5] {
    if dim == 0 {
        return [0; 5];
    }
    #[cfg(target_arch = "x86_64")]
    {
        match *X86_BIT_KERNEL {
            #[cfg(not(coverage))]
            X86BitKernel::Avx512Vpopcnt => {
                return unsafe { sm_dot_counts_raw_avx512(pos_a, strong_a, pos_b, strong_b, dim) };
            }
            X86BitKernel::Avx2Popcnt => {
                if dim >= 512 && dim.is_multiple_of(64) {
                    return unsafe {
                        sm_dot_counts_raw_avx2_popcnt(pos_a, strong_a, pos_b, strong_b, dim)
                    };
                }
                return unsafe { sm_dot_counts_raw_popcnt(pos_a, strong_a, pos_b, strong_b, dim) };
            }
            X86BitKernel::Popcnt => {
                return unsafe { sm_dot_counts_raw_popcnt(pos_a, strong_a, pos_b, strong_b, dim) };
            }
            X86BitKernel::Avx2Only => {
                if dim >= 256 && dim.is_multiple_of(64) {
                    return unsafe {
                        sm_dot_counts_raw_avx2(pos_a, strong_a, pos_b, strong_b, dim)
                    };
                }
            }
            X86BitKernel::Scalar => {}
        }
        return sm_dot_counts_raw_scalar(pos_a, strong_a, pos_b, strong_b, dim);
    }
    #[cfg(all(target_arch = "aarch64", not(coverage)))]
    {
        return unsafe { sm_dot_counts_raw_neon(pos_a, strong_a, pos_b, strong_b, dim) };
    }
    #[allow(unreachable_code)]
    sm_dot_counts_raw_scalar(pos_a, strong_a, pos_b, strong_b, dim)
}
#[inline]
pub(crate) fn sm_dot_counts_raw_scalar(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> [u32; 5] {
    let chunks = dim.div_ceil(64);
    let valid_bits_last = if dim.is_multiple_of(64) {
        !0u64
    } else {
        (1u64 << (dim % 64)) - 1
    };
    let mut sums = [0u32; 5];
    for i in 0..chunks {
        let mask = if i == chunks - 1 {
            valid_bits_last
        } else {
            !0u64
        };
        let pa = unsafe { *pos_a.add(i) } & mask;
        let sa = unsafe { *strong_a.add(i) } & mask;
        let pb = unsafe { *pos_b.add(i) } & mask;
        let sb = unsafe { *strong_b.add(i) } & mask;
        let m = pa ^ pb;
        let both = sa & sb;
        sums[0] += m.count_ones();
        sums[1] += (m & sa).count_ones();
        sums[2] += (m & sb).count_ones();
        sums[3] += both.count_ones();
        sums[4] += (m & both).count_ones();
    }
    sums
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "popcnt")]
pub(crate) unsafe fn sm_dot_counts_raw_popcnt(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> [u32; 5] {
    unsafe {
        if dim == 64 {
            let m = *pos_a ^ *pos_b;
            let sa = *strong_a;
            let sb = *strong_b;
            let both = sa & sb;
            return [
                m.count_ones(),
                (m & sa).count_ones(),
                (m & sb).count_ones(),
                both.count_ones(),
                (m & both).count_ones(),
            ];
        }
        let chunks = dim.div_ceil(64);
        let valid_bits_last = if dim.is_multiple_of(64) {
            !0u64
        } else {
            (1u64 << (dim % 64)) - 1
        };
        let mut s0 = 0u32;
        let mut s1 = 0u32;
        let mut s2 = 0u32;
        let mut s3 = 0u32;
        let mut s4 = 0u32;
        let full = if dim.is_multiple_of(64) {
            chunks
        } else {
            chunks - 1
        };
        for i in 0..full {
            let m = *pos_a.add(i) ^ *pos_b.add(i);
            let sa = *strong_a.add(i);
            let sb = *strong_b.add(i);
            let both = sa & sb;
            s0 += m.count_ones();
            s1 += (m & sa).count_ones();
            s2 += (m & sb).count_ones();
            s3 += both.count_ones();
            s4 += (m & both).count_ones();
        }
        if full < chunks {
            let pa = *pos_a.add(full) & valid_bits_last;
            let sa = *strong_a.add(full) & valid_bits_last;
            let pb = *pos_b.add(full) & valid_bits_last;
            let sb = *strong_b.add(full) & valid_bits_last;
            let m = pa ^ pb;
            let both = sa & sb;
            s0 += m.count_ones();
            s1 += (m & sa).count_ones();
            s2 += (m & sb).count_ones();
            s3 += both.count_ones();
            s4 += (m & both).count_ones();
        }
        [s0, s1, s2, s3, s4]
    }
}
#[cfg(all(target_arch = "x86_64", not(coverage)))]
#[target_feature(enable = "avx512f,avx512vpopcntdq,popcnt")]
pub(crate) unsafe fn sm_dot_counts_raw_avx512(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> [u32; 5] {
    use std::arch::x86_64::*;
    unsafe {
        let chunks = dim.div_ceil(64);
        let full_words = if dim.is_multiple_of(64) {
            chunks
        } else {
            chunks - 1
        };
        let full_rounds = full_words / 8;
        let mut acc0 = _mm512_setzero_si512();
        let mut acc1 = _mm512_setzero_si512();
        let mut acc2 = _mm512_setzero_si512();
        let mut acc3 = _mm512_setzero_si512();
        let mut acc4 = _mm512_setzero_si512();

        for r in 0..full_rounds {
            let off = r * 8;
            let pa = _mm512_loadu_si512(pos_a.add(off) as *const _);
            let sa = _mm512_loadu_si512(strong_a.add(off) as *const _);
            let pb = _mm512_loadu_si512(pos_b.add(off) as *const _);
            let sb = _mm512_loadu_si512(strong_b.add(off) as *const _);
            let m = _mm512_xor_si512(pa, pb);
            let both = _mm512_and_si512(sa, sb);
            acc0 = _mm512_add_epi64(acc0, _mm512_popcnt_epi64(m));
            acc1 = _mm512_add_epi64(acc1, _mm512_popcnt_epi64(_mm512_and_si512(m, sa)));
            acc2 = _mm512_add_epi64(acc2, _mm512_popcnt_epi64(_mm512_and_si512(m, sb)));
            acc3 = _mm512_add_epi64(acc3, _mm512_popcnt_epi64(both));
            acc4 = _mm512_add_epi64(acc4, _mm512_popcnt_epi64(_mm512_and_si512(m, both)));
        }

        let mut s0 = _mm512_reduce_add_epi64(acc0) as u32;
        let mut s1 = _mm512_reduce_add_epi64(acc1) as u32;
        let mut s2 = _mm512_reduce_add_epi64(acc2) as u32;
        let mut s3 = _mm512_reduce_add_epi64(acc3) as u32;
        let mut s4 = _mm512_reduce_add_epi64(acc4) as u32;

        let valid_bits_last = if dim.is_multiple_of(64) {
            !0u64
        } else {
            (1u64 << (dim % 64)) - 1
        };
        for i in (full_rounds * 8)..chunks {
            let mask = if i == chunks - 1 {
                valid_bits_last
            } else {
                !0u64
            };
            let pa = *pos_a.add(i) & mask;
            let sa = *strong_a.add(i) & mask;
            let pb = *pos_b.add(i) & mask;
            let sb = *strong_b.add(i) & mask;
            let m = pa ^ pb;
            let both = sa & sb;
            s0 += m.count_ones();
            s1 += (m & sa).count_ones();
            s2 += (m & sb).count_ones();
            s3 += both.count_ones();
            s4 += (m & both).count_ones();
        }
        [s0, s1, s2, s3, s4]
    }
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn sm_dot_counts_raw_avx2(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> [u32; 5] {
    use std::arch::x86_64::*;
    unsafe {
        let chunks = dim.div_ceil(64);
        let full_words = if dim.is_multiple_of(64) {
            chunks
        } else {
            chunks - 1
        };
        let full_rounds = full_words / 4;
        let mut acc0 = _mm256_setzero_si256();
        let mut acc1 = _mm256_setzero_si256();
        let mut acc2 = _mm256_setzero_si256();
        let mut acc3 = _mm256_setzero_si256();
        let mut acc4 = _mm256_setzero_si256();

        // 3-to-2 进位保存全加器（Carry-Save Adder）：每 3 轮（768 bit，恰好一次覆盖 768D 向量）
        // 将 15 次 popcnt256 压缩为 10 次 popcnt256，消除 Broadwell/Skylake Port-5 pshufb 瓶颈。
        let csa_add = |acc: __m256i, x0: __m256i, x1: __m256i, x2: __m256i| -> __m256i {
            let t = _mm256_xor_si256(x0, x1);
            let ones = _mm256_xor_si256(t, x2);
            let twos = _mm256_or_si256(_mm256_and_si256(x0, x1), _mm256_and_si256(t, x2));
            let sum_pair = _mm256_add_epi64(_mm256_slli_epi64(popcnt256(twos), 1), popcnt256(ones));
            _mm256_add_epi64(acc, sum_pair)
        };

        let mut r = 0usize;
        while r + 3 <= full_rounds {
            let off0 = r * 4;
            let off1 = (r + 1) * 4;
            let off2 = (r + 2) * 4;

            let pa0 = _mm256_loadu_si256(pos_a.add(off0) as *const __m256i);
            let sa0 = _mm256_loadu_si256(strong_a.add(off0) as *const __m256i);
            let pb0 = _mm256_loadu_si256(pos_b.add(off0) as *const __m256i);
            let sb0 = _mm256_loadu_si256(strong_b.add(off0) as *const __m256i);
            let m0 = _mm256_xor_si256(pa0, pb0);
            let b0 = _mm256_and_si256(sa0, sb0);

            let pa1 = _mm256_loadu_si256(pos_a.add(off1) as *const __m256i);
            let sa1 = _mm256_loadu_si256(strong_a.add(off1) as *const __m256i);
            let pb1 = _mm256_loadu_si256(pos_b.add(off1) as *const __m256i);
            let sb1 = _mm256_loadu_si256(strong_b.add(off1) as *const __m256i);
            let m1 = _mm256_xor_si256(pa1, pb1);
            let b1 = _mm256_and_si256(sa1, sb1);

            let pa2 = _mm256_loadu_si256(pos_a.add(off2) as *const __m256i);
            let sa2 = _mm256_loadu_si256(strong_a.add(off2) as *const __m256i);
            let pb2 = _mm256_loadu_si256(pos_b.add(off2) as *const __m256i);
            let sb2 = _mm256_loadu_si256(strong_b.add(off2) as *const __m256i);
            let m2 = _mm256_xor_si256(pa2, pb2);
            let b2 = _mm256_and_si256(sa2, sb2);

            acc0 = csa_add(acc0, m0, m1, m2);
            acc1 = csa_add(
                acc1,
                _mm256_and_si256(m0, sa0),
                _mm256_and_si256(m1, sa1),
                _mm256_and_si256(m2, sa2),
            );
            acc2 = csa_add(
                acc2,
                _mm256_and_si256(m0, sb0),
                _mm256_and_si256(m1, sb1),
                _mm256_and_si256(m2, sb2),
            );
            acc3 = csa_add(acc3, b0, b1, b2);
            acc4 = csa_add(
                acc4,
                _mm256_and_si256(m0, b0),
                _mm256_and_si256(m1, b1),
                _mm256_and_si256(m2, b2),
            );
            r += 3;
        }

        while r < full_rounds {
            let off = r * 4;
            let pa = _mm256_loadu_si256(pos_a.add(off) as *const __m256i);
            let sa = _mm256_loadu_si256(strong_a.add(off) as *const __m256i);
            let pb = _mm256_loadu_si256(pos_b.add(off) as *const __m256i);
            let sb = _mm256_loadu_si256(strong_b.add(off) as *const __m256i);
            let m = _mm256_xor_si256(pa, pb);
            let both = _mm256_and_si256(sa, sb);
            acc0 = _mm256_add_epi64(acc0, popcnt256(m));
            acc1 = _mm256_add_epi64(acc1, popcnt256(_mm256_and_si256(m, sa)));
            acc2 = _mm256_add_epi64(acc2, popcnt256(_mm256_and_si256(m, sb)));
            acc3 = _mm256_add_epi64(acc3, popcnt256(both));
            acc4 = _mm256_add_epi64(acc4, popcnt256(_mm256_and_si256(m, both)));
            r += 1;
        }

        let hsum = |v: __m256i| -> u32 {
            let hi = _mm256_extracti128_si256(v, 1);
            let lo = _mm256_castsi256_si128(v);
            let s = _mm_add_epi64(lo, hi);
            let hi64 = _mm_srli_si128(s, 8);
            _mm_cvtsi128_si64(_mm_add_epi64(s, hi64)) as u32
        };

        let mut s0 = hsum(acc0);
        let mut s1 = hsum(acc1);
        let mut s2 = hsum(acc2);
        let mut s3 = hsum(acc3);
        let mut s4 = hsum(acc4);

        let valid_bits_last = if dim.is_multiple_of(64) {
            !0u64
        } else {
            (1u64 << (dim % 64)) - 1
        };
        for i in (full_rounds * 4)..chunks {
            let mask = if i == chunks - 1 {
                valid_bits_last
            } else {
                !0u64
            };
            let pa = *pos_a.add(i) & mask;
            let sa = *strong_a.add(i) & mask;
            let pb = *pos_b.add(i) & mask;
            let sb = *strong_b.add(i) & mask;
            let m = pa ^ pb;
            let both = sa & sb;
            s0 += m.count_ones();
            s1 += (m & sa).count_ones();
            s2 += (m & sb).count_ones();
            s3 += both.count_ones();
            s4 += (m & both).count_ones();
        }
        [s0, s1, s2, s3, s4]
    }
}
