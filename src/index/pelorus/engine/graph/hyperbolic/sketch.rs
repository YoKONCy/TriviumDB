//! 方向草图与对称、非对称过滤算子。

/// 编译期高精度双精度平方根（牛顿迭代法），供静态只读查找表生成使用。
pub(crate) const fn const_sqrt_f64(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = if x >= 1.0 { x * 0.5 } else { 1.0 };
    let mut step = 0usize;
    while step < 12 {
        g = 0.5 * (g + x / g);
        step += 1;
    }
    g
}
/// 编译期精确计算的 256-bit 汉明距离余弦查找表：`COS_PI_256[h] = cos(pi * h / 256.0)`（1028B 常驻只读段）。
pub(crate) const fn build_cos_pi_256_lut() -> [f32; 257] {
    let mut lut = [0.0f32; 257];
    let half_pi: f64 = std::f64::consts::FRAC_PI_2;
    let mut h = 0usize;
    while h <= 256 {
        // 利用 cos(pi * h / 256) = sin(pi/2 * (1 - h/128)) 在 [-pi/2, pi/2] 上十一阶泰勒展开（截断误差 < 2e-10）
        let x = half_pi * (1.0 - (h as f64) / 128.0);
        let x2 = x * x;
        let s = x
            * (1.0
                - x2 / 6.0
                    * (1.0
                        - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0 * (1.0 - x2 / 110.0)))));
        lut[h] = if h == 0 {
            1.0
        } else if h == 128 {
            0.0
        } else if h == 256 {
            -1.0
        } else {
            s as f32
        };
        h += 1;
    }
    lut
}
pub(crate) static COS_PI_256: [f32; 257] = build_cos_pi_256_lut();
/// 编译期计算的 256-bit 草图二项标准差比例表（相对于 h=48 即 cos≈0.83 处归一化，下界钳制为 1.0）。
///
/// 对高相似语义近邻（h <= 48）比例恒为 1.0，保持原有紧致拦截率；对近正交高维均匀流形（h ≈ 115, cos ≈ 0.20）
/// 自动按 `sin(theta) * sqrt(p(1-p))` 放大至 ~2.26×，消除早筛过剪枝。
pub(crate) const fn build_sketch256_std_ratio_lut() -> [f32; 257] {
    let cos_lut = build_cos_pi_256_lut();
    let p_ref = 48.0 / 256.0;
    let c_ref = cos_lut[48] as f64;
    let sin_ref = const_sqrt_f64(1.0 - c_ref * c_ref);
    let std_ref = sin_ref * const_sqrt_f64(p_ref * (1.0 - p_ref));

    let mut out = [1.0f32; 257];
    let mut h = 0usize;
    while h <= 256 {
        let p = (h as f64) / 256.0;
        let c = cos_lut[h] as f64;
        let sin_v = const_sqrt_f64(if 1.0 > c * c { 1.0 - c * c } else { 0.0 });
        let std_v = sin_v * const_sqrt_f64(p * (1.0 - p));
        let ratio = std_v / std_ref;
        out[h] = if ratio > 1.0 { ratio as f32 } else { 1.0 };
        h += 1;
    }
    out
}
pub(crate) static SKETCH256_STD_RATIO: [f32; 257] = build_sketch256_std_ratio_lut();
/// 按均匀坐标抽样提取 256-bit 符号草图，768 维时保持原步长三的编码。
#[inline]
pub(crate) fn pack_strided_sketch256(vector: &[f32]) -> [u64; 4] {
    debug_assert!(!vector.is_empty());
    let mut out = [0u64; 4];
    for w in 0..4 {
        let mut bits = 0u64;
        let base_k = w * 64;
        for b in 0..64 {
            if vector[(base_k + b) * vector.len() / 256] > 0.0 {
                bits |= 1u64 << b;
            }
        }
        out[w] = bits;
    }
    out
}
/// 64B 缓存行对齐的 256 维查询无符号幅值权重数组（严格 4 条缓存行 = 256B）。
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub(crate) struct AlignedSketchWeights(pub(crate) [u8; 256]);
impl Default for AlignedSketchWeights {
    fn default() -> Self {
        Self([0u8; 256])
    }
}
#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AsymSketchSimd {
    Scalar,
    Avx2,
    #[cfg(not(coverage))]
    Avx512Bw,
}
#[cfg(target_arch = "x86_64")]
pub(crate) static ASYM_SKETCH_SIMD: std::sync::LazyLock<AsymSketchSimd> =
    std::sync::LazyLock::new(|| {
        #[cfg(not(coverage))]
        if !*crate::index::pelorus::engine::quantization::FORCE_NO_AVX512
            && is_x86_feature_detected!("avx512f")
            && is_x86_feature_detected!("avx512bw")
        {
            return AsymSketchSimd::Avx512Bw;
        }
        if is_x86_feature_detected!("avx2") {
            AsymSketchSimd::Avx2
        } else {
            AsymSketchSimd::Scalar
        }
    });
/// 查询期预编码的非对称 `u_q × 256-bit 符号草图` SIMD 掩码归约评分器（严格 320B，仅占 5 条 L1d 缓存行）。
///
/// 利用代数恒等式 `sum_k q_k * s_k = 2 * sum_k |q_k| * agree_k - sum_k |q_k|`：
/// - AVX-512BW 路径直接将 `agree: u64` 作为硬件掩码寄存器 `__mmask64`，仅需 4 条 `VMOVDQU8 {k}{z}` + 4 条 `VPSADBW`（约 6 周期）；
/// - AVX2 路径用 `VPSHUFB` 广播字节掩码 + `VPCMPEQB` + `VPSADBW` 并行归约（约 12 周期），与标量及 AVX-512 逐比特整数和严格一致。
pub(crate) struct AsymSketch256Query {
    pub(crate) weights: AlignedSketchWeights,
    pub(crate) q_pos_mask: [u64; 4],
    pub(crate) total_weight: i32,
    pub(crate) inv_norm_scale: f32,
    pub(crate) valid: bool,
}
impl Default for AsymSketch256Query {
    fn default() -> Self {
        Self {
            weights: AlignedSketchWeights::default(),
            q_pos_mask: [0u64; 4],
            total_weight: 0,
            inv_norm_scale: 0.0,
            valid: false,
        }
    }
}
impl AsymSketch256Query {
    /// 将查询的均匀抽样坐标编码为幅值数组与符号掩码。
    #[inline]
    pub(crate) fn encode_into(&mut self, query: &[f32]) {
        if query.is_empty() {
            self.valid = false;
            return;
        }
        let mut sub = [0.0f64; 256];
        let mut norm_sq = 0.0f64;
        let mut max_abs = 1e-12f64;
        for k in 0..256 {
            let v = f64::from(query[k * query.len() / 256]);
            sub[k] = v;
            norm_sq += v * v;
            let av = v.abs();
            if av > max_abs {
                max_abs = av;
            }
        }
        if norm_sq <= 1e-24 || !norm_sq.is_finite() {
            self.valid = false;
            return;
        }
        let inv_norm = 1.0 / norm_sq.sqrt();
        let scale = 255.0 / max_abs;
        // sqrt(2 / pi * 256) = 12.766152972845851
        const ASYM_GAUSS_GAIN: f64 = 12.766_152_972_845_851;
        self.inv_norm_scale = (inv_norm / (scale * ASYM_GAUSS_GAIN)) as f32;

        let mut total_w = 0i32;
        let mut pos_mask = [0u64; 4];
        for w in 0..4 {
            let base = w * 64;
            let mut m = 0u64;
            for b in 0..64 {
                let v = sub[base + b];
                if v >= 0.0 {
                    m |= 1u64 << b;
                }
                let wt = (v.abs() * scale).round() as u8;
                self.weights.0[base + b] = wt;
                total_w += i32::from(wt);
            }
            pos_mask[w] = m;
        }
        self.q_pos_mask = pos_mask;
        self.total_weight = total_w;
        self.valid = true;
    }

    /// 标量基准归约：计算符号一致坐标上的无符号权重和 `sum_k weights[k] * agree_k`。
    #[inline]
    pub(crate) fn agree_sum_scalar(&self, sketch: &[u64; 4]) -> i32 {
        let mut sum = 0i32;
        for w in 0..4 {
            let mut agree = !(sketch[w] ^ self.q_pos_mask[w]);
            let base = w * 64;
            while agree != 0 {
                let b = agree.trailing_zeros() as usize;
                sum += i32::from(unsafe { *self.weights.0.get_unchecked(base + b) });
                agree &= agree - 1;
            }
        }
        sum
    }

    /// AVX2 并行字节掩码归约：每 32-bit 掩码用 `VPSHUFB + VPAND + VPCMPEQB + VPSADBW` 无表展开求和。
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    pub(crate) unsafe fn agree_sum_avx2(&self, sketch: &[u64; 4]) -> i32 {
        use std::arch::x86_64::*;
        unsafe {
            let zero = _mm256_setzero_si256();
            let shuf = _mm256_setr_epi8(
                0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3,
                3, 3, 3, 3,
            );
            let bit_sel = _mm256_setr_epi8(
                1, 2, 4, 8, 16, 32, 64, -128i8, 1, 2, 4, 8, 16, 32, 64, -128i8, 1, 2, 4, 8, 16, 32,
                64, -128i8, 1, 2, 4, 8, 16, 32, 64, -128i8,
            );
            let w_ptr = self.weights.0.as_ptr();
            let mut acc = _mm256_setzero_si256();
            for w_idx in 0..4 {
                let agree64 = !(sketch[w_idx] ^ self.q_pos_mask[w_idx]);
                let lo32 = agree64 as u32 as i32;
                let hi32 = (agree64 >> 32) as u32 as i32;

                let bytes_lo = _mm256_shuffle_epi8(_mm256_set1_epi32(lo32), shuf);
                let mask_lo = _mm256_cmpeq_epi8(_mm256_and_si256(bytes_lo, bit_sel), bit_sel);
                let w_lo = _mm256_load_si256(w_ptr.add(w_idx * 64) as *const __m256i);
                acc = _mm256_add_epi64(acc, _mm256_sad_epu8(_mm256_and_si256(w_lo, mask_lo), zero));

                let bytes_hi = _mm256_shuffle_epi8(_mm256_set1_epi32(hi32), shuf);
                let mask_hi = _mm256_cmpeq_epi8(_mm256_and_si256(bytes_hi, bit_sel), bit_sel);
                let w_hi = _mm256_load_si256(w_ptr.add(w_idx * 64 + 32) as *const __m256i);
                acc = _mm256_add_epi64(acc, _mm256_sad_epu8(_mm256_and_si256(w_hi, mask_hi), zero));
            }
            let hi128 = _mm256_extracti128_si256(acc, 1);
            let lo128 = _mm256_castsi256_si128(acc);
            let sum128 = _mm_add_epi64(lo128, hi128);
            let sum64 = _mm_add_epi64(sum128, _mm_srli_si128(sum128, 8));
            _mm_cvtsi128_si64(sum64) as i32
        }
    }

    /// AVX-512BW 硬件掩码寄存器归约：直接用 `VMOVDQU8 zmm {k}{z}` + `VPSADBW` 单周期完成 64 维条件累加。
    #[cfg(all(target_arch = "x86_64", not(coverage)))]
    #[target_feature(enable = "avx512f,avx512bw")]
    pub(crate) unsafe fn agree_sum_avx512bw(&self, sketch: &[u64; 4]) -> i32 {
        use std::arch::x86_64::*;
        unsafe {
            let zero = _mm512_setzero_si512();
            let w_ptr = self.weights.0.as_ptr();
            let agree0 = !(sketch[0] ^ self.q_pos_mask[0]);
            let agree1 = !(sketch[1] ^ self.q_pos_mask[1]);
            let agree2 = !(sketch[2] ^ self.q_pos_mask[2]);
            let agree3 = !(sketch[3] ^ self.q_pos_mask[3]);

            let m0 = _mm512_maskz_mov_epi8(agree0, _mm512_load_si512(w_ptr as *const __m512i));
            let m1 =
                _mm512_maskz_mov_epi8(agree1, _mm512_load_si512(w_ptr.add(64) as *const __m512i));
            let m2 =
                _mm512_maskz_mov_epi8(agree2, _mm512_load_si512(w_ptr.add(128) as *const __m512i));
            let m3 =
                _mm512_maskz_mov_epi8(agree3, _mm512_load_si512(w_ptr.add(192) as *const __m512i));

            let s01 = _mm512_add_epi64(_mm512_sad_epu8(m0, zero), _mm512_sad_epu8(m1, zero));
            let s23 = _mm512_add_epi64(_mm512_sad_epu8(m2, zero), _mm512_sad_epu8(m3, zero));
            _mm512_reduce_add_epi64(_mm512_add_epi64(s01, s23)) as i32
        }
    }

    /// 仅凭 Cache Line 0 中的 32B 符号草图计算非对称球面余弦估计。
    #[inline(always)]
    pub(crate) fn cosine_hat(&self, sketch: &[u64; 4]) -> f64 {
        if !self.valid {
            return 1.0;
        }
        #[cfg(target_arch = "x86_64")]
        let agree_sum = match *ASYM_SKETCH_SIMD {
            #[cfg(not(coverage))]
            AsymSketchSimd::Avx512Bw => unsafe { self.agree_sum_avx512bw(sketch) },
            AsymSketchSimd::Avx2 => unsafe { self.agree_sum_avx2(sketch) },
            AsymSketchSimd::Scalar => self.agree_sum_scalar(sketch),
        };
        #[cfg(not(target_arch = "x86_64"))]
        let agree_sum = self.agree_sum_scalar(sketch);

        let signed_sum = 2 * agree_sum - self.total_weight;
        (f64::from(signed_sum) * f64::from(self.inv_norm_scale)).clamp(-1.0, 1.0)
    }
}
