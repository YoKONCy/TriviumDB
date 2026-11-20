//! 非对称查询编码与标量、SIMD 内核。

use super::*;
// ═════════════════════════════════════════════════════════════
//  Stage-0.5 L1-Hot 非对称 FP32 查询 × 2-bit SM 正交投影守门算子
// ═════════════════════════════════════════════════════════════

/// 固定点缩放常数：由 Cauchy-Schwarz 不等式，任意单位向量 u_q 在单 SIMD 通道最多累加 16 项时
/// 幅值和恒 <= sqrt(16) * ||u_q||_2 = 4.0，故 4.0 * 8100 + 8 = 32408 < i16::MAX (32767)，数学保证永不溢出。
pub(crate) const ASYM_SM_SCALE: f64 = 8100.0;
pub(crate) const ASYM_SM_INV_SCALE: f64 = 1.0 / ASYM_SM_SCALE;
/// 预编码查询方向的非对称 `u_q × 2-bit SM` 快速评分器，复用线程局部缓冲区。
///
/// 利用两级重构方向及重构范数，计算校准余弦近似
/// `clamp(<u_q, u_hat_v> / max(sigma_v^2, 0.25), -1.0, 1.0)`。
/// 查询采用 i16 定点编码，误差同时包含定点舍入与文档方向重构残差。
#[derive(Default)]
pub(crate) struct AsymQuerySm {
    pub(crate) q_i16: Vec<i16>,
    pub(crate) sum_q: f64,
    pub(crate) chunks: usize,
    pub(crate) valid: bool,
}
impl AsymQuerySm {
    #[inline]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 将原始查询向量与已知欧氏半径 `q_radius` 原地编码为 64 维对齐的 i16 定点单位方向。
    #[inline]
    pub(crate) fn encode_into(&mut self, query: &[f32], q_radius: f64) {
        let dim = query.len();
        let chunks = if dim == 0 { 0 } else { dim.div_ceil(64) };
        self.chunks = chunks;
        if dim == 0 || q_radius <= 0.0 || !q_radius.is_finite() {
            self.valid = false;
            self.sum_q = 0.0;
            return;
        }
        let padded_len = chunks * 64;
        self.q_i16.clear();
        self.q_i16.resize(padded_len, 0);
        let scale = ASYM_SM_SCALE / q_radius;
        let mut sum_i64 = 0i64;
        for (j, &v) in query.iter().enumerate() {
            let qv = (f64::from(v) * scale).round() as i16;
            self.q_i16[j] = qv;
            sum_i64 += i64::from(qv);
        }
        self.sum_q = (sum_i64 as f64) * ASYM_SM_INV_SCALE;
        self.valid = true;
    }

    /// 零冷存访问计算查询单位方向 `u_q` 与第 `idx` 个 2-bit SM 签名的非对称校准球面余弦。
    #[inline]
    pub(crate) fn calibrated_cosine(
        &self,
        store: &Bq2Store,
        idx: usize,
        sm_v: SmCodeParams,
    ) -> f64 {
        if !self.valid || sm_v.sigma <= 0.0 {
            return 0.0;
        }
        self.calibrated_cosine_slices(store.pos_slice(idx), store.strong_slice(idx), sm_v)
    }

    /// 直接对给定的 `pos` / `strong` 切片计算非对称校准球面余弦（供 256B 合表节点记录零二次寻址直调）。
    #[inline]
    pub(crate) fn calibrated_cosine_slices(
        &self,
        pos: &[u64],
        strong: &[u64],
        sm_v: SmCodeParams,
    ) -> f64 {
        if !self.valid || sm_v.sigma <= 0.0 {
            return 0.0;
        }
        let (sum_pos, sum_strong, sum_ps) =
            asym_sm_masked_sums(&self.q_i16, pos, strong, self.chunks);
        let f_pos = f64::from(sum_pos) * ASYM_SM_INV_SCALE;
        let f_strong = f64::from(sum_strong) * ASYM_SM_INV_SCALE;
        let f_ps = f64::from(sum_ps) * ASYM_SM_INV_SCALE;

        let dot_sign = 2.0 * f_pos - self.sum_q;
        let dot_strong = 2.0 * f_ps - f_strong;
        let dot_uhat = f64::from(sm_v.weak) * dot_sign + f64::from(sm_v.delta) * dot_strong;
        let denom = (f64::from(sm_v.sigma) * f64::from(sm_v.sigma)).max(0.25);
        (dot_uhat / denom).clamp(-1.0, 1.0)
    }
}
/// 256 项 8-bit → 8×i8(0x00/-1) 掩码查表（仅占 2KB，常驻 L1d 缓存），用于单指令扩展 16-lane AVX2 掩码。
#[cfg(target_arch = "x86_64")]
pub(crate) const fn build_mask8_to_i8x8_lut() -> [u64; 256] {
    let mut lut = [0u64; 256];
    let mut idx = 0usize;
    while idx < 256 {
        let mut word = 0u64;
        let mut bit = 0usize;
        while bit < 8 {
            if ((idx >> bit) & 1) != 0 {
                word |= 0xFFu64 << (bit * 8);
            }
            bit += 1;
        }
        lut[idx] = word;
        idx += 1;
    }
    lut
}
#[cfg(target_arch = "x86_64")]
pub(crate) static MASK8_TO_I8X8: [u64; 256] = build_mask8_to_i8x8_lut();
/// 消融实验专用：控制 768D 对称 SM 位核与非对称掩码核的微架构变体。
/// - `0`: 宽加载 AVX2 VPSHUFB 3-to-2 CSA 核 + 纯寄存器非对称核（实测百万级跨 L3 图遍历 Load-Buffer/ROB 压力最低，生产默认）
/// - `1`: 硬件 POPCNT 60 指令全展开核 + 2KB 查表非对称核（L1 常驻微基准最快）
/// - `2`: 硬件 POPCNT + 64-bit 3-to-2 CSA（仅 40 条 POPCNT）+ 2KB 查表非对称核
/// - `3`: Broadwell/Skylake 双端口异构协同核（Port-5 AVX2 CSA + Port-1 硬件 POPCNT CSA 并行）+ 2KB 查表非对称核
#[cfg(feature = "research")]
pub(crate) static SM_FAST_KERNEL_VARIANT: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);
/// 消融实验独立选择非对称掩码核，255 表示沿用既有位核关联规则。
#[cfg(feature = "research")]
pub(crate) static ASYM_SM_LUT_OVERRIDE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(255);
/// 实验期间必须在所有查询开始前设置，并在并发查询期间保持不变。
#[cfg(feature = "research")]
pub fn set_asym_sm_lut_override(enabled: Option<bool>) {
    ASYM_SM_LUT_OVERRIDE.store(
        enabled.map_or(255, u8::from),
        std::sync::atomic::Ordering::Relaxed,
    );
}
#[cfg(feature = "research")]
#[inline]
pub fn set_sm_fast_kernel_variant(variant: u8) {
    SM_FAST_KERNEL_VARIANT.store(variant.min(3), std::sync::atomic::Ordering::Relaxed);
}
#[cfg(feature = "research")]
#[inline]
pub fn sm_fast_kernel_variant() -> u8 {
    SM_FAST_KERNEL_VARIANT.load(std::sync::atomic::Ordering::Relaxed)
}
#[inline]
pub(crate) fn current_sm_kernel_variant() -> u8 {
    #[cfg(feature = "research")]
    {
        sm_fast_kernel_variant()
    }
    #[cfg(not(feature = "research"))]
    {
        0
    }
}
#[inline]
pub(crate) fn asym_sm_masked_sums(
    q_i16: &[i16],
    pos: &[u64],
    strong: &[u64],
    chunks: usize,
) -> (i32, i32, i32) {
    #[cfg(target_arch = "x86_64")]
    {
        let has_avx2 = matches!(
            *X86_BIT_KERNEL,
            X86BitKernel::Avx2Popcnt | X86BitKernel::Avx2Only
        ) || {
            #[cfg(not(coverage))]
            {
                *X86_BIT_KERNEL == X86BitKernel::Avx512Vpopcnt
            }
            #[cfg(coverage)]
            {
                false
            }
        };
        if has_avx2 {
            #[cfg(feature = "research")]
            let use_lut = match ASYM_SM_LUT_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed) {
                0 => false,
                1 => true,
                _ => current_sm_kernel_variant() > 0,
            };
            #[cfg(not(feature = "research"))]
            let use_lut = current_sm_kernel_variant() > 0;
            if use_lut {
                return unsafe { asym_sm_masked_sums_avx2_lut(q_i16, pos, strong, chunks) };
            }
            return unsafe { asym_sm_masked_sums_avx2(q_i16, pos, strong, chunks) };
        }
    }
    asym_sm_masked_sums_scalar(q_i16, pos, strong, chunks)
}
#[inline]
pub(crate) fn asym_sm_masked_sums_scalar(
    q_i16: &[i16],
    pos: &[u64],
    strong: &[u64],
    chunks: usize,
) -> (i32, i32, i32) {
    let mut sp = 0i32;
    let mut ss = 0i32;
    let mut sps = 0i32;
    for c in 0..chunks {
        let base = c * 64;
        let mut mp = pos[c];
        let mut ms = strong[c];
        let mut mps = mp & ms;
        while mp != 0 {
            let b = mp.trailing_zeros() as usize;
            sp += i32::from(q_i16[base + b]);
            mp &= mp - 1;
        }
        while ms != 0 {
            let b = ms.trailing_zeros() as usize;
            ss += i32::from(q_i16[base + b]);
            ms &= ms - 1;
        }
        while mps != 0 {
            let b = mps.trailing_zeros() as usize;
            sps += i32::from(q_i16[base + b]);
            mps &= mps - 1;
        }
    }
    (sp, ss, sps)
}
/// 基于 2KB L1 常驻查表 + `vpmovsxbw` 的高效 AVX2 非对称掩码求和核（消除逐位广播与比较指令链）。
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn asym_sm_masked_sums_avx2_lut(
    q_i16: &[i16],
    pos: &[u64],
    strong: &[u64],
    chunks: usize,
) -> (i32, i32, i32) {
    use std::arch::x86_64::*;
    unsafe {
        let ones16 = _mm256_set1_epi16(1);
        let q_ptr = q_i16.as_ptr();
        let lut_ptr = MASK8_TO_I8X8.as_ptr();

        let mut tot_p = _mm256_setzero_si256();
        let mut tot_s = _mm256_setzero_si256();
        let mut tot_ps = _mm256_setzero_si256();

        let mut c = 0usize;
        while c < chunks {
            let end = (c + 4).min(chunks);
            let mut acc_p = _mm256_setzero_si256();
            let mut acc_s = _mm256_setzero_si256();
            let mut acc_ps = _mm256_setzero_si256();

            while c < end {
                let pw = *pos.get_unchecked(c);
                let sw = *strong.get_unchecked(c);
                let base = c * 64;

                macro_rules! step16_lut {
                    ($shift:expr, $off:expr) => {{
                        let qv = _mm256_loadu_si256(q_ptr.add(base + $off) as *const __m256i);
                        let p_lo = *lut_ptr.add(((pw >> $shift) & 0xFF) as usize) as i64;
                        let p_hi = *lut_ptr.add(((pw >> ($shift + 8)) & 0xFF) as usize) as i64;
                        let s_lo = *lut_ptr.add(((sw >> $shift) & 0xFF) as usize) as i64;
                        let s_hi = *lut_ptr.add(((sw >> ($shift + 8)) & 0xFF) as usize) as i64;
                        let cmp_p = _mm256_cvtepi8_epi16(_mm_set_epi64x(p_hi, p_lo));
                        let cmp_s = _mm256_cvtepi8_epi16(_mm_set_epi64x(s_hi, s_lo));
                        let sel_p = _mm256_and_si256(qv, cmp_p);
                        let sel_s = _mm256_and_si256(qv, cmp_s);
                        let sel_ps = _mm256_and_si256(sel_p, cmp_s);
                        acc_p = _mm256_add_epi16(acc_p, sel_p);
                        acc_s = _mm256_add_epi16(acc_s, sel_s);
                        acc_ps = _mm256_add_epi16(acc_ps, sel_ps);
                    }};
                }

                step16_lut!(0, 0);
                step16_lut!(16, 16);
                step16_lut!(32, 32);
                step16_lut!(48, 48);
                c += 1;
            }

            tot_p = _mm256_add_epi32(tot_p, _mm256_madd_epi16(acc_p, ones16));
            tot_s = _mm256_add_epi32(tot_s, _mm256_madd_epi16(acc_s, ones16));
            tot_ps = _mm256_add_epi32(tot_ps, _mm256_madd_epi16(acc_ps, ones16));
        }

        let hsum32 = |v: __m256i| -> i32 {
            let lo = _mm256_castsi256_si128(v);
            let hi = _mm256_extracti128_si256(v, 1);
            let s = _mm_add_epi32(lo, hi);
            let s = _mm_add_epi32(s, _mm_srli_si128(s, 8));
            let s = _mm_add_epi32(s, _mm_srli_si128(s, 4));
            _mm_cvtsi128_si32(s)
        };

        (hsum32(tot_p), hsum32(tot_s), hsum32(tot_ps))
    }
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn asym_sm_masked_sums_avx2(
    q_i16: &[i16],
    pos: &[u64],
    strong: &[u64],
    chunks: usize,
) -> (i32, i32, i32) {
    use std::arch::x86_64::*;
    unsafe {
        let bit_mask = _mm256_setr_epi16(
            1 << 0,
            1 << 1,
            1 << 2,
            1 << 3,
            1 << 4,
            1 << 5,
            1 << 6,
            1 << 7,
            1 << 8,
            1 << 9,
            1 << 10,
            1 << 11,
            1 << 12,
            1 << 13,
            1 << 14,
            -32768i16,
        );
        let ones16 = _mm256_set1_epi16(1);
        let q_ptr = q_i16.as_ptr();

        let mut tot_p = _mm256_setzero_si256();
        let mut tot_s = _mm256_setzero_si256();
        let mut tot_ps = _mm256_setzero_si256();

        // 每最多 4 个 u64（16 个 16-lane 块 = 256 维）将 i16 通道无溢出归并至 i32 累加器
        let mut c = 0usize;
        while c < chunks {
            let end = (c + 4).min(chunks);
            let mut acc_p = _mm256_setzero_si256();
            let mut acc_s = _mm256_setzero_si256();
            let mut acc_ps = _mm256_setzero_si256();

            while c < end {
                let pw = *pos.get_unchecked(c);
                let sw = *strong.get_unchecked(c);
                let base = c * 64;

                macro_rules! step16 {
                    ($shift:expr, $off:expr) => {{
                        let qv = _mm256_loadu_si256(q_ptr.add(base + $off) as *const __m256i);
                        let bp = _mm256_set1_epi16((pw >> $shift) as i16);
                        let bs = _mm256_set1_epi16((sw >> $shift) as i16);
                        let cmp_p = _mm256_cmpeq_epi16(_mm256_and_si256(bp, bit_mask), bit_mask);
                        let cmp_s = _mm256_cmpeq_epi16(_mm256_and_si256(bs, bit_mask), bit_mask);
                        let sel_p = _mm256_and_si256(qv, cmp_p);
                        let sel_s = _mm256_and_si256(qv, cmp_s);
                        let sel_ps = _mm256_and_si256(sel_p, cmp_s);
                        acc_p = _mm256_add_epi16(acc_p, sel_p);
                        acc_s = _mm256_add_epi16(acc_s, sel_s);
                        acc_ps = _mm256_add_epi16(acc_ps, sel_ps);
                    }};
                }

                step16!(0, 0);
                step16!(16, 16);
                step16!(32, 32);
                step16!(48, 48);
                c += 1;
            }

            tot_p = _mm256_add_epi32(tot_p, _mm256_madd_epi16(acc_p, ones16));
            tot_s = _mm256_add_epi32(tot_s, _mm256_madd_epi16(acc_s, ones16));
            tot_ps = _mm256_add_epi32(tot_ps, _mm256_madd_epi16(acc_ps, ones16));
        }

        let hsum32 = |v: __m256i| -> i32 {
            let lo = _mm256_castsi256_si128(v);
            let hi = _mm256_extracti128_si256(v, 1);
            let s = _mm_add_epi32(lo, hi);
            let s = _mm_add_epi32(s, _mm_srli_si128(s, 8));
            let s = _mm_add_epi32(s, _mm_srli_si128(s, 4));
            _mm_cvtsi128_si32(s)
        };

        (hsum32(tot_p), hsum32(tot_s), hsum32(tot_ps))
    }
}
/// 768D 专用变体 1：12×u64 硬件 POPCNT 全展开核（零 VPSHUFB，纯走 Port-1）。
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "popcnt")]
pub(crate) unsafe fn sm_dot_counts_768_popcnt_unrolled(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
) -> [u32; 5] {
    unsafe {
        let mut s0 = 0u32;
        let mut s1 = 0u32;
        let mut s2 = 0u32;
        let mut s3 = 0u32;
        let mut s4 = 0u32;
        macro_rules! word_step {
            ($idx:expr) => {{
                let m = *pos_a.add($idx) ^ *pos_b.add($idx);
                let sa = *strong_a.add($idx);
                let sb = *strong_b.add($idx);
                let both = sa & sb;
                s0 += m.count_ones();
                s1 += (m & sa).count_ones();
                s2 += (m & sb).count_ones();
                s3 += both.count_ones();
                s4 += (m & both).count_ones();
            }};
        }
        word_step!(0);
        word_step!(1);
        word_step!(2);
        word_step!(3);
        word_step!(4);
        word_step!(5);
        word_step!(6);
        word_step!(7);
        word_step!(8);
        word_step!(9);
        word_step!(10);
        word_step!(11);
        [s0, s1, s2, s3, s4]
    }
}
/// 768D 专用变体 2：硬件 POPCNT + 64-bit 3-to-2 CSA 压缩核（将 60 次硬件 POPCNT 压缩为 40 次）。
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "popcnt")]
pub(crate) unsafe fn sm_dot_counts_768_popcnt_csa(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
) -> [u32; 5] {
    #[inline(always)]
    fn csa_popcnt_u64(x0: u64, x1: u64, x2: u64) -> u32 {
        let t = x0 ^ x1;
        let ones = t ^ x2;
        let twos = (x0 & x1) | (t & x2);
        (twos.count_ones() << 1) + ones.count_ones()
    }
    unsafe {
        let mut s0 = 0u32;
        let mut s1 = 0u32;
        let mut s2 = 0u32;
        let mut s3 = 0u32;
        let mut s4 = 0u32;
        macro_rules! tri_step {
            ($i0:expr, $i1:expr, $i2:expr) => {{
                let m0 = *pos_a.add($i0) ^ *pos_b.add($i0);
                let sa0 = *strong_a.add($i0);
                let sb0 = *strong_b.add($i0);
                let b0 = sa0 & sb0;

                let m1 = *pos_a.add($i1) ^ *pos_b.add($i1);
                let sa1 = *strong_a.add($i1);
                let sb1 = *strong_b.add($i1);
                let b1 = sa1 & sb1;

                let m2 = *pos_a.add($i2) ^ *pos_b.add($i2);
                let sa2 = *strong_a.add($i2);
                let sb2 = *strong_b.add($i2);
                let b2 = sa2 & sb2;

                s0 += csa_popcnt_u64(m0, m1, m2);
                s1 += csa_popcnt_u64(m0 & sa0, m1 & sa1, m2 & sa2);
                s2 += csa_popcnt_u64(m0 & sb0, m1 & sb1, m2 & sb2);
                s3 += csa_popcnt_u64(b0, b1, b2);
                s4 += csa_popcnt_u64(m0 & b0, m1 & b1, m2 & b2);
            }};
        }
        tri_step!(0, 1, 2);
        tri_step!(3, 4, 5);
        tri_step!(6, 7, 8);
        tri_step!(9, 10, 11);
        [s0, s1, s2, s3, s4]
    }
}
/// 768D 专用变体 3：Broadwell/Skylake 双执行端口异构协同核（Port-5 AVX2 VPSHUFB + Port-1 硬件 POPCNT 同时满载）。
///
/// - 统计量 `s0 (m)`, `s1 (m & sa)`, `s3 (both)` 由 256-bit AVX2 CSA 负责（共 6 次 `popcnt256` = 12 条 Port-5 `VPSHUFB`）；
/// - 统计量 `s2 (m & sb)`, `s4 (m & both)` 同时由 64-bit 标量硬件 `POPCNT` + CSA 负责（共 16 条 Port-1 `POPCNT`）；
///
/// 两条指令流分别使用向量与标量执行端口，以交叠位计数计算。
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,popcnt")]
pub(crate) unsafe fn sm_dot_counts_768_hybrid_port1_port5(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
) -> [u32; 5] {
    use std::arch::x86_64::*;
    #[inline(always)]
    fn csa_popcnt_u64(x0: u64, x1: u64, x2: u64) -> u32 {
        let t = x0 ^ x1;
        let ones = t ^ x2;
        let twos = (x0 & x1) | (t & x2);
        (twos.count_ones() << 1) + ones.count_ones()
    }
    unsafe {
        // AVX2 通道：一次覆盖 3 × 256-bit = 768-bit，计算 s0, s1, s3
        let pa0 = _mm256_loadu_si256(pos_a as *const __m256i);
        let sa0 = _mm256_loadu_si256(strong_a as *const __m256i);
        let pb0 = _mm256_loadu_si256(pos_b as *const __m256i);
        let sb0 = _mm256_loadu_si256(strong_b as *const __m256i);
        let vm0 = _mm256_xor_si256(pa0, pb0);
        let vb0 = _mm256_and_si256(sa0, sb0);

        let pa1 = _mm256_loadu_si256(pos_a.add(4) as *const __m256i);
        let sa1 = _mm256_loadu_si256(strong_a.add(4) as *const __m256i);
        let pb1 = _mm256_loadu_si256(pos_b.add(4) as *const __m256i);
        let sb1 = _mm256_loadu_si256(strong_b.add(4) as *const __m256i);
        let vm1 = _mm256_xor_si256(pa1, pb1);
        let vb1 = _mm256_and_si256(sa1, sb1);

        let pa2 = _mm256_loadu_si256(pos_a.add(8) as *const __m256i);
        let sa2 = _mm256_loadu_si256(strong_a.add(8) as *const __m256i);
        let pb2 = _mm256_loadu_si256(pos_b.add(8) as *const __m256i);
        let sb2 = _mm256_loadu_si256(strong_b.add(8) as *const __m256i);
        let vm2 = _mm256_xor_si256(pa2, pb2);
        let vb2 = _mm256_and_si256(sa2, sb2);

        let csa256 = |x0: __m256i, x1: __m256i, x2: __m256i| -> __m256i {
            let t = _mm256_xor_si256(x0, x1);
            let ones = _mm256_xor_si256(t, x2);
            let twos = _mm256_or_si256(_mm256_and_si256(x0, x1), _mm256_and_si256(t, x2));
            _mm256_add_epi64(_mm256_slli_epi64(popcnt256(twos), 1), popcnt256(ones))
        };

        let acc0 = csa256(vm0, vm1, vm2);
        let acc1 = csa256(
            _mm256_and_si256(vm0, sa0),
            _mm256_and_si256(vm1, sa1),
            _mm256_and_si256(vm2, sa2),
        );
        let acc3 = csa256(vb0, vb1, vb2);

        // 标量 Port-1 通道：并行计算 s2 与 s4（L1d 命中刚加载的同一批缓存行）
        let mut s2 = 0u32;
        let mut s4 = 0u32;
        macro_rules! tri_s24 {
            ($i0:expr, $i1:expr, $i2:expr) => {{
                let m0 = *pos_a.add($i0) ^ *pos_b.add($i0);
                let sb0 = *strong_b.add($i0);
                let msb0 = m0 & sb0;
                let mb0 = msb0 & *strong_a.add($i0);

                let m1 = *pos_a.add($i1) ^ *pos_b.add($i1);
                let sb1 = *strong_b.add($i1);
                let msb1 = m1 & sb1;
                let mb1 = msb1 & *strong_a.add($i1);

                let m2 = *pos_a.add($i2) ^ *pos_b.add($i2);
                let sb2 = *strong_b.add($i2);
                let msb2 = m2 & sb2;
                let mb2 = msb2 & *strong_a.add($i2);

                s2 += csa_popcnt_u64(msb0, msb1, msb2);
                s4 += csa_popcnt_u64(mb0, mb1, mb2);
            }};
        }
        tri_s24!(0, 1, 2);
        tri_s24!(3, 4, 5);
        tri_s24!(6, 7, 8);
        tri_s24!(9, 10, 11);

        let hsum = |v: __m256i| -> u32 {
            let hi = _mm256_extracti128_si256(v, 1);
            let lo = _mm256_castsi256_si128(v);
            let s = _mm_add_epi64(lo, hi);
            let hi64 = _mm_srli_si128(s, 8);
            _mm_cvtsi128_si64(_mm_add_epi64(s, hi64)) as u32
        };

        [hsum(acc0), hsum(acc1), s2, hsum(acc3), s4]
    }
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,popcnt")]
pub(crate) unsafe fn sm_dot_counts_raw_avx2_popcnt(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> [u32; 5] {
    unsafe {
        if dim == 768 {
            match current_sm_kernel_variant() {
                0 => return sm_dot_counts_raw_avx2(pos_a, strong_a, pos_b, strong_b, 768),
                1 => return sm_dot_counts_768_popcnt_unrolled(pos_a, strong_a, pos_b, strong_b),
                2 => return sm_dot_counts_768_popcnt_csa(pos_a, strong_a, pos_b, strong_b),
                _ => return sm_dot_counts_768_hybrid_port1_port5(pos_a, strong_a, pos_b, strong_b),
            }
        }
        sm_dot_counts_raw_avx2(pos_a, strong_a, pos_b, strong_b, dim)
    }
}
#[cfg(all(target_arch = "aarch64", not(coverage)))]
#[target_feature(enable = "neon")]
pub(crate) unsafe fn sm_dot_counts_raw_neon(
    pos_a: *const u64,
    strong_a: *const u64,
    pos_b: *const u64,
    strong_b: *const u64,
    dim: usize,
) -> [u32; 5] {
    use std::arch::aarch64::*;
    unsafe {
        let chunks = dim.div_ceil(64);
        let full_rounds = if dim.is_multiple_of(64) {
            chunks / 2
        } else {
            chunks.saturating_sub(1) / 2
        };
        let popcnt128 = |v: uint64x2_t| -> uint64x2_t {
            let bytes = vcntq_u8(vreinterpretq_u8_u64(v));
            let h16 = vpaddlq_u8(bytes);
            let h32 = vpaddlq_u16(h16);
            vpaddlq_u32(h32)
        };
        let mut acc0 = vdupq_n_u64(0);
        let mut acc1 = vdupq_n_u64(0);
        let mut acc2 = vdupq_n_u64(0);
        let mut acc3 = vdupq_n_u64(0);
        let mut acc4 = vdupq_n_u64(0);
        for r in 0..full_rounds {
            let off = r * 2;
            let pa = vld1q_u64(pos_a.add(off));
            let sa = vld1q_u64(strong_a.add(off));
            let pb = vld1q_u64(pos_b.add(off));
            let sb = vld1q_u64(strong_b.add(off));
            let m = veorq_u64(pa, pb);
            let both = vandq_u64(sa, sb);
            acc0 = vaddq_u64(acc0, popcnt128(m));
            acc1 = vaddq_u64(acc1, popcnt128(vandq_u64(m, sa)));
            acc2 = vaddq_u64(acc2, popcnt128(vandq_u64(m, sb)));
            acc3 = vaddq_u64(acc3, popcnt128(both));
            acc4 = vaddq_u64(acc4, popcnt128(vandq_u64(m, both)));
        }
        let mut s0 = vaddvq_u64(acc0) as u32;
        let mut s1 = vaddvq_u64(acc1) as u32;
        let mut s2 = vaddvq_u64(acc2) as u32;
        let mut s3 = vaddvq_u64(acc3) as u32;
        let mut s4 = vaddvq_u64(acc4) as u32;
        let valid_bits_last = if dim.is_multiple_of(64) {
            !0u64
        } else {
            (1u64 << (dim % 64)) - 1
        };
        for i in (full_rounds * 2)..chunks {
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
