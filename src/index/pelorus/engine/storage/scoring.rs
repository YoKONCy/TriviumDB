//! 压缩码字在寄存器中重构并直接累加距离或内积。

use super::{RerankConfig, RerankPrecision, ScalarQuantization};

struct CodeView<'a> {
    row: &'a [u8],
    precision: RerankPrecision,
    center: &'a [f32],
    code_bytes: usize,
    group: usize,
    affine: bool,
}

impl<'a> CodeView<'a> {
    fn new(row: &'a [u8], config: RerankConfig, center: &'a [f32], dim: usize) -> Self {
        Self {
            row,
            precision: config.precision,
            center,
            code_bytes: match config.precision {
                RerankPrecision::Fp32 => dim * 4,
                RerankPrecision::Fp16 | RerankPrecision::Bf16 => dim * 2,
                RerankPrecision::Sq8 => dim,
                RerankPrecision::Sq4 => dim.div_ceil(2),
            },
            group: match config.scalar_quantization {
                ScalarQuantization::Affine { group_size } => group_size,
                _ => dim,
            },
            affine: matches!(
                config.scalar_quantization,
                ScalarQuantization::Affine { .. }
            ),
        }
    }

    fn parameters(&self, index: usize) -> (f32, f32) {
        let offset = self.code_bytes
            + if self.affine {
                index / self.group * 8
            } else {
                0
            };
        let scale = f32::from_le_bytes(self.row[offset..offset + 4].try_into().unwrap());
        let bias = if self.affine {
            f32::from_le_bytes(self.row[offset + 4..offset + 8].try_into().unwrap())
        } else {
            0.0
        };
        (scale, bias)
    }

    #[inline]
    fn component(&self, index: usize) -> f32 {
        let value = match self.precision {
            RerankPrecision::Fp32 => {
                f32::from_le_bytes(self.row[index * 4..index * 4 + 4].try_into().unwrap())
            }
            RerankPrecision::Fp16 | RerankPrecision::Bf16 => {
                let bits =
                    u16::from_le_bytes(self.row[index * 2..index * 2 + 2].try_into().unwrap());
                if self.precision == RerankPrecision::Fp16 {
                    half::f16::from_bits(bits).to_f32()
                } else {
                    half::bf16::from_bits(bits).to_f32()
                }
            }
            RerankPrecision::Sq8 | RerankPrecision::Sq4 => {
                let code = if self.precision == RerankPrecision::Sq4 {
                    ((self.row[index / 2] >> (index % 2 * 4)) & 15) as i8 - 7
                } else {
                    self.row[index] as i8
                };
                let (scale, bias) = self.parameters(index);
                f32::from(code) * scale + bias
            }
        };
        if self.center.is_empty() {
            value
        } else {
            value + self.center[index]
        }
    }
}

#[inline]
pub(super) fn accumulate(
    query: &[f32],
    row: &[u8],
    config: RerankConfig,
    center: &[f32],
    scale: f64,
    dot: bool,
) -> f64 {
    let view = CodeView::new(row, config, center, query.len());
    #[cfg(all(target_arch = "x86_64", not(coverage)))]
    if *AVX512_READY && (!view.affine || view.group.is_multiple_of(16)) {
        // SAFETY：完整 AVX-512 子集经过运行时检测，入口一次性校验记录与中心切片长度。
        return unsafe {
            if dot {
                accumulate_avx512::<true>(query, &view, scale)
            } else {
                accumulate_avx512::<false>(query, &view, scale)
            }
        };
    }
    #[cfg(target_arch = "x86_64")]
    if *AVX2_READY && (!view.affine || view.group.is_multiple_of(8)) {
        // SAFETY：运行时已检测 AVX2+FMA+F16C，入口一次性校验记录与中心切片长度。
        return unsafe {
            if dot {
                accumulate_avx2::<true>(query, &view, scale)
            } else {
                accumulate_avx2::<false>(query, &view, scale)
            }
        };
    }
    accumulate_scalar(query, &view, scale, dot)
}

fn accumulate_scalar(query: &[f32], view: &CodeView<'_>, scale: f64, dot: bool) -> f64 {
    query
        .iter()
        .enumerate()
        .map(|(index, &q)| {
            let value = f64::from(view.component(index)) * scale;
            if dot {
                f64::from(q) * value
            } else {
                (f64::from(q) - value).powi(2)
            }
        })
        .sum()
}

#[cfg(target_arch = "x86_64")]
fn assert_view_bounds(dim: usize, view: &CodeView<'_>) {
    assert!(view.row.len() >= view.code_bytes);
    if !view.center.is_empty() {
        assert!(view.center.len() >= dim);
    }
    if matches!(view.precision, RerankPrecision::Sq8 | RerankPrecision::Sq4) {
        if view.affine {
            let groups = dim.div_ceil(view.group);
            assert!(view.group > 0 && view.row.len() >= view.code_bytes + groups * 8);
        } else if dim > 0 {
            assert!(view.row.len() >= view.code_bytes + 4);
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn read_f32_le(ptr: *const u8) -> f32 {
    // SAFETY：调用方已在进入 SIMD 热循环前一次性校验记录参数区边界。
    f32::from_le_bytes(unsafe { ptr.cast::<[u8; 4]>().read_unaligned() })
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn read_u32_le(ptr: *const u8) -> u32 {
    // SAFETY：调用方已在进入 SIMD 热循环前一次性校验码字区边界。
    u32::from_le_bytes(unsafe { ptr.cast::<[u8; 4]>().read_unaligned() })
}

#[cfg(target_arch = "x86_64")]
static AVX2_READY: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
    std::arch::is_x86_feature_detected!("avx2")
        && std::arch::is_x86_feature_detected!("fma")
        && std::arch::is_x86_feature_detected!("f16c")
});

#[cfg(all(target_arch = "x86_64", not(coverage)))]
static AVX512_READY: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
    !*crate::index::pelorus::engine::quantization::FORCE_NO_AVX512
        && std::arch::is_x86_feature_detected!("avx512f")
        && std::arch::is_x86_feature_detected!("avx512bw")
        && std::arch::is_x86_feature_detected!("avx512dq")
});

#[cfg(all(target_arch = "x86_64", not(coverage)))]
#[target_feature(enable = "avx512f,avx512bw,avx512dq")]
#[inline]
unsafe fn step_sixteen_avx512<const DOT: bool>(
    q_ptr: *const f32,
    v: std::arch::x86_64::__m512,
    factor: std::arch::x86_64::__m512d,
    low: &mut std::arch::x86_64::__m512d,
    high: &mut std::arch::x86_64::__m512d,
) {
    use std::arch::x86_64::*;
    // SAFETY：调用方保证 q_ptr 至少含 16 个有效 f32，外层已启用完整 AVX-512 子集。
    unsafe {
        let q0 = _mm512_cvtps_pd(_mm256_loadu_ps(q_ptr));
        let q1 = _mm512_cvtps_pd(_mm256_loadu_ps(q_ptr.add(8)));
        let v0 = _mm512_mul_pd(_mm512_cvtps_pd(_mm512_castps512_ps256(v)), factor);
        let v1 = _mm512_mul_pd(_mm512_cvtps_pd(_mm512_extractf32x8_ps::<1>(v)), factor);
        if DOT {
            *low = _mm512_fmadd_pd(q0, v0, *low);
            *high = _mm512_fmadd_pd(q1, v1, *high);
        } else {
            let d0 = _mm512_sub_pd(q0, v0);
            let d1 = _mm512_sub_pd(q1, v1);
            *low = _mm512_fmadd_pd(d0, d0, *low);
            *high = _mm512_fmadd_pd(d1, d1, *high);
        }
    }
}

#[cfg(all(target_arch = "x86_64", not(coverage)))]
#[target_feature(enable = "avx512f,avx512bw,avx512dq")]
#[inline]
unsafe fn accumulate_avx512_sq8_single<const DOT: bool, const CENTERED: bool>(
    query: &[f32],
    view: &CodeView<'_>,
    scale: f64,
) -> f64 {
    use std::arch::x86_64::*;
    // SAFETY：入口已校验码字、单尺度参数与中心向量边界；外提单尺度消除热循环切片检查与分支。
    unsafe {
        let dim = query.len();
        let q_ptr = query.as_ptr();
        let row_ptr = view.row.as_ptr();
        let center_ptr = view.center.as_ptr();
        let v_scale = if dim >= 16 {
            _mm512_set1_ps(read_f32_le(row_ptr.add(view.code_bytes)))
        } else {
            _mm512_setzero_ps()
        };
        let factor = _mm512_set1_pd(scale);
        let mut low = _mm512_setzero_pd();
        let mut high = _mm512_setzero_pd();
        let mut index = 0usize;
        while index + 32 <= dim {
            let c0 = _mm512_cvtepi8_epi32(_mm_loadu_si128(row_ptr.add(index).cast()));
            let c1 = _mm512_cvtepi8_epi32(_mm_loadu_si128(row_ptr.add(index + 16).cast()));
            let mut v0 = _mm512_mul_ps(_mm512_cvtepi32_ps(c0), v_scale);
            let mut v1 = _mm512_mul_ps(_mm512_cvtepi32_ps(c1), v_scale);
            if CENTERED {
                v0 = _mm512_add_ps(v0, _mm512_loadu_ps(center_ptr.add(index)));
                v1 = _mm512_add_ps(v1, _mm512_loadu_ps(center_ptr.add(index + 16)));
            }
            step_sixteen_avx512::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
            step_sixteen_avx512::<DOT>(q_ptr.add(index + 16), v1, factor, &mut low, &mut high);
            index += 32;
        }
        if index + 16 <= dim {
            let c0 = _mm512_cvtepi8_epi32(_mm_loadu_si128(row_ptr.add(index).cast()));
            let mut v0 = _mm512_mul_ps(_mm512_cvtepi32_ps(c0), v_scale);
            if CENTERED {
                v0 = _mm512_add_ps(v0, _mm512_loadu_ps(center_ptr.add(index)));
            }
            step_sixteen_avx512::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
            index += 16;
        }
        finish_avx512::<DOT>(query, view, scale, index, low, high)
    }
}

#[cfg(all(target_arch = "x86_64", not(coverage)))]
#[target_feature(enable = "avx512f,avx512bw,avx512dq")]
#[inline]
unsafe fn accumulate_avx512_sq4_single<const DOT: bool, const CENTERED: bool>(
    query: &[f32],
    view: &CodeView<'_>,
    scale: f64,
) -> f64 {
    use std::arch::x86_64::*;
    // SAFETY：入口已校验半字节码字、单尺度参数与中心边界；外提常量并做 32 维两路展开。
    unsafe {
        let dim = query.len();
        let q_ptr = query.as_ptr();
        let row_ptr = view.row.as_ptr();
        let center_ptr = view.center.as_ptr();
        let v_scale = if dim >= 16 {
            _mm512_set1_ps(read_f32_le(row_ptr.add(view.code_bytes)))
        } else {
            _mm512_setzero_ps()
        };
        let mask = _mm_set1_epi8(15);
        let seven = _mm512_set1_epi32(7);
        let factor = _mm512_set1_pd(scale);
        let mut low = _mm512_setzero_pd();
        let mut high = _mm512_setzero_pd();
        let mut index = 0usize;
        while index + 32 <= dim {
            let b0 = _mm_loadl_epi64(row_ptr.add(index / 2).cast());
            let b1 = _mm_loadl_epi64(row_ptr.add(index / 2 + 8).cast());
            let u0 = _mm_unpacklo_epi8(
                _mm_and_si128(b0, mask),
                _mm_and_si128(_mm_srli_epi16::<4>(b0), mask),
            );
            let u1 = _mm_unpacklo_epi8(
                _mm_and_si128(b1, mask),
                _mm_and_si128(_mm_srli_epi16::<4>(b1), mask),
            );
            let c0 = _mm512_sub_epi32(_mm512_cvtepu8_epi32(u0), seven);
            let c1 = _mm512_sub_epi32(_mm512_cvtepu8_epi32(u1), seven);
            let mut v0 = _mm512_mul_ps(_mm512_cvtepi32_ps(c0), v_scale);
            let mut v1 = _mm512_mul_ps(_mm512_cvtepi32_ps(c1), v_scale);
            if CENTERED {
                v0 = _mm512_add_ps(v0, _mm512_loadu_ps(center_ptr.add(index)));
                v1 = _mm512_add_ps(v1, _mm512_loadu_ps(center_ptr.add(index + 16)));
            }
            step_sixteen_avx512::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
            step_sixteen_avx512::<DOT>(q_ptr.add(index + 16), v1, factor, &mut low, &mut high);
            index += 32;
        }
        if index + 16 <= dim {
            let b0 = _mm_loadl_epi64(row_ptr.add(index / 2).cast());
            let u0 = _mm_unpacklo_epi8(
                _mm_and_si128(b0, mask),
                _mm_and_si128(_mm_srli_epi16::<4>(b0), mask),
            );
            let c0 = _mm512_sub_epi32(_mm512_cvtepu8_epi32(u0), seven);
            let mut v0 = _mm512_mul_ps(_mm512_cvtepi32_ps(c0), v_scale);
            if CENTERED {
                v0 = _mm512_add_ps(v0, _mm512_loadu_ps(center_ptr.add(index)));
            }
            step_sixteen_avx512::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
            index += 16;
        }
        finish_avx512::<DOT>(query, view, scale, index, low, high)
    }
}

#[cfg(all(target_arch = "x86_64", not(coverage)))]
#[target_feature(enable = "avx512f,avx512bw,avx512dq")]
#[inline]
unsafe fn finish_avx512<const DOT: bool>(
    query: &[f32],
    view: &CodeView<'_>,
    scale: f64,
    index: usize,
    low: std::arch::x86_64::__m512d,
    high: std::arch::x86_64::__m512d,
) -> f64 {
    use std::arch::x86_64::*;
    let mut sum = _mm512_reduce_add_pd(_mm512_add_pd(low, high));
    for (i, &q) in query.iter().enumerate().skip(index) {
        let value = f64::from(view.component(i)) * scale;
        sum += if DOT {
            f64::from(q) * value
        } else {
            (f64::from(q) - value).powi(2)
        };
    }
    sum
}

#[cfg(all(target_arch = "x86_64", not(coverage)))]
#[target_feature(enable = "avx512f,avx512bw,avx512dq")]
unsafe fn accumulate_avx512<const DOT: bool>(
    query: &[f32],
    view: &CodeView<'_>,
    scale: f64,
) -> f64 {
    use std::arch::x86_64::*;
    assert_view_bounds(query.len(), view);
    let centered = !view.center.is_empty();
    // SAFETY：外层已校验指令集及切片边界，按编码格式特化消除热循环内部分支和逐块切片检查。
    unsafe {
        match (view.precision, view.affine, centered) {
            (RerankPrecision::Sq8, false, false) => {
                accumulate_avx512_sq8_single::<DOT, false>(query, view, scale)
            }
            (RerankPrecision::Sq8, false, true) => {
                accumulate_avx512_sq8_single::<DOT, true>(query, view, scale)
            }
            (RerankPrecision::Sq4, false, false) => {
                accumulate_avx512_sq4_single::<DOT, false>(query, view, scale)
            }
            (RerankPrecision::Sq4, false, true) => {
                accumulate_avx512_sq4_single::<DOT, true>(query, view, scale)
            }
            (RerankPrecision::Fp16, _, false) => {
                let dim = query.len();
                let q_ptr = query.as_ptr();
                let row_ptr = view.row.as_ptr();
                let factor = _mm512_set1_pd(scale);
                let mut low = _mm512_setzero_pd();
                let mut high = _mm512_setzero_pd();
                let mut index = 0usize;
                while index + 32 <= dim {
                    let v0 = _mm512_cvtph_ps(_mm256_loadu_si256(row_ptr.add(index * 2).cast()));
                    let v1 =
                        _mm512_cvtph_ps(_mm256_loadu_si256(row_ptr.add((index + 16) * 2).cast()));
                    step_sixteen_avx512::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
                    step_sixteen_avx512::<DOT>(
                        q_ptr.add(index + 16),
                        v1,
                        factor,
                        &mut low,
                        &mut high,
                    );
                    index += 32;
                }
                if index + 16 <= dim {
                    let v0 = _mm512_cvtph_ps(_mm256_loadu_si256(row_ptr.add(index * 2).cast()));
                    step_sixteen_avx512::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
                    index += 16;
                }
                finish_avx512::<DOT>(query, view, scale, index, low, high)
            }
            (RerankPrecision::Bf16, _, false) => {
                let dim = query.len();
                let q_ptr = query.as_ptr();
                let row_ptr = view.row.as_ptr();
                let factor = _mm512_set1_pd(scale);
                let mut low = _mm512_setzero_pd();
                let mut high = _mm512_setzero_pd();
                let mut index = 0usize;
                while index + 16 <= dim {
                    let packed = _mm256_loadu_si256(row_ptr.add(index * 2).cast());
                    let v =
                        _mm512_castsi512_ps(_mm512_slli_epi32::<16>(_mm512_cvtepu16_epi32(packed)));
                    step_sixteen_avx512::<DOT>(q_ptr.add(index), v, factor, &mut low, &mut high);
                    index += 16;
                }
                finish_avx512::<DOT>(query, view, scale, index, low, high)
            }
            (RerankPrecision::Fp32, _, false) => {
                let dim = query.len();
                let q_ptr = query.as_ptr();
                let row_ptr = view.row.as_ptr();
                let factor = _mm512_set1_pd(scale);
                let mut low = _mm512_setzero_pd();
                let mut high = _mm512_setzero_pd();
                let mut index = 0usize;
                while index + 32 <= dim {
                    let v0 = _mm512_loadu_ps(row_ptr.add(index * 4).cast());
                    let v1 = _mm512_loadu_ps(row_ptr.add((index + 16) * 4).cast());
                    step_sixteen_avx512::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
                    step_sixteen_avx512::<DOT>(
                        q_ptr.add(index + 16),
                        v1,
                        factor,
                        &mut low,
                        &mut high,
                    );
                    index += 32;
                }
                if index + 16 <= dim {
                    let v0 = _mm512_loadu_ps(row_ptr.add(index * 4).cast());
                    step_sixteen_avx512::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
                    index += 16;
                }
                finish_avx512::<DOT>(query, view, scale, index, low, high)
            }
            _ => {
                // 分组仿射（group_size 为 16 的整数倍）及兜底组合：按组步进更新参数指针，消除逐块整数除法。
                let dim = query.len();
                let q_ptr = query.as_ptr();
                let row_ptr = view.row.as_ptr();
                let center_ptr = view.center.as_ptr();
                let mask = _mm_set1_epi8(15);
                let seven = _mm512_set1_epi32(7);
                let factor = _mm512_set1_pd(scale);
                let mut low = _mm512_setzero_pd();
                let mut high = _mm512_setzero_pd();
                let mut param_ptr = row_ptr.add(view.code_bytes);
                let mut rem_in_group = 0usize;
                let mut v_scale = _mm512_setzero_ps();
                let mut v_bias = _mm512_setzero_ps();
                let mut index = 0usize;
                while index + 16 <= dim {
                    if rem_in_group == 0 {
                        v_scale = _mm512_set1_ps(read_f32_le(param_ptr));
                        v_bias = if view.affine {
                            _mm512_set1_ps(read_f32_le(param_ptr.add(4)))
                        } else {
                            _mm512_setzero_ps()
                        };
                        if view.affine {
                            param_ptr = param_ptr.add(8);
                        }
                        rem_in_group = view.group;
                    }
                    let codes = if view.precision == RerankPrecision::Sq8 {
                        _mm512_cvtepi8_epi32(_mm_loadu_si128(row_ptr.add(index).cast()))
                    } else {
                        let bytes = _mm_loadl_epi64(row_ptr.add(index / 2).cast());
                        let unpacked = _mm_unpacklo_epi8(
                            _mm_and_si128(bytes, mask),
                            _mm_and_si128(_mm_srli_epi16::<4>(bytes), mask),
                        );
                        _mm512_sub_epi32(_mm512_cvtepu8_epi32(unpacked), seven)
                    };
                    let mut v =
                        _mm512_add_ps(_mm512_mul_ps(_mm512_cvtepi32_ps(codes), v_scale), v_bias);
                    if centered {
                        v = _mm512_add_ps(v, _mm512_loadu_ps(center_ptr.add(index)));
                    }
                    step_sixteen_avx512::<DOT>(q_ptr.add(index), v, factor, &mut low, &mut high);
                    index += 16;
                    rem_in_group -= 16;
                }
                finish_avx512::<DOT>(query, view, scale, index, low, high)
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma,f16c")]
#[inline]
unsafe fn step_eight_avx2<const DOT: bool>(
    q_ptr: *const f32,
    v: std::arch::x86_64::__m256,
    factor: std::arch::x86_64::__m256d,
    low: &mut std::arch::x86_64::__m256d,
    high: &mut std::arch::x86_64::__m256d,
) {
    use std::arch::x86_64::*;
    // SAFETY：调用方保证 q_ptr 至少含 8 个有效 f32，直接融合 128 位载入与 f64 拓宽指令。
    unsafe {
        let q0 = _mm256_cvtps_pd(_mm_loadu_ps(q_ptr));
        let q1 = _mm256_cvtps_pd(_mm_loadu_ps(q_ptr.add(4)));
        let v0 = _mm256_mul_pd(_mm256_cvtps_pd(_mm256_castps256_ps128(v)), factor);
        let v1 = _mm256_mul_pd(_mm256_cvtps_pd(_mm256_extractf128_ps::<1>(v)), factor);
        if DOT {
            *low = _mm256_fmadd_pd(q0, v0, *low);
            *high = _mm256_fmadd_pd(q1, v1, *high);
        } else {
            let d0 = _mm256_sub_pd(q0, v0);
            let d1 = _mm256_sub_pd(q1, v1);
            *low = _mm256_fmadd_pd(d0, d0, *low);
            *high = _mm256_fmadd_pd(d1, d1, *high);
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma,f16c")]
#[inline]
unsafe fn finish_avx2<const DOT: bool>(
    query: &[f32],
    view: &CodeView<'_>,
    scale: f64,
    index: usize,
    low: std::arch::x86_64::__m256d,
    high: std::arch::x86_64::__m256d,
) -> f64 {
    use std::arch::x86_64::*;
    let lanes = _mm256_add_pd(low, high);
    let pair = _mm_add_pd(
        _mm256_castpd256_pd128(lanes),
        _mm256_extractf128_pd::<1>(lanes),
    );
    let mut sum = _mm_cvtsd_f64(_mm_add_sd(pair, _mm_unpackhi_pd(pair, pair)));
    for (i, &q) in query.iter().enumerate().skip(index) {
        let value = f64::from(view.component(i)) * scale;
        sum += if DOT {
            f64::from(q) * value
        } else {
            (f64::from(q) - value).powi(2)
        };
    }
    sum
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma,f16c")]
#[inline]
unsafe fn accumulate_avx2_sq8_single<const DOT: bool, const CENTERED: bool>(
    query: &[f32],
    view: &CodeView<'_>,
    scale: f64,
) -> f64 {
    use std::arch::x86_64::*;
    // SAFETY：入口已校验码字、单尺度参数与中心向量长度；每次载入 16 字节码字并外提尺度广播，严格保持原逐维舍入与累加顺序。
    unsafe {
        let dim = query.len();
        let q_ptr = query.as_ptr();
        let row_ptr = view.row.as_ptr();
        let center_ptr = view.center.as_ptr();
        let v_scale = if dim >= 8 {
            _mm256_set1_ps(read_f32_le(row_ptr.add(view.code_bytes)))
        } else {
            _mm256_setzero_ps()
        };
        let factor = _mm256_set1_pd(scale);
        let mut low = _mm256_setzero_pd();
        let mut high = _mm256_setzero_pd();
        let mut index = 0usize;
        while index + 16 <= dim {
            let raw = _mm_loadu_si128(row_ptr.add(index).cast());
            let c0 = _mm256_cvtepi8_epi32(raw);
            let c1 = _mm256_cvtepi8_epi32(_mm_bsrli_si128::<8>(raw));
            let mut v0 = _mm256_mul_ps(_mm256_cvtepi32_ps(c0), v_scale);
            let mut v1 = _mm256_mul_ps(_mm256_cvtepi32_ps(c1), v_scale);
            if CENTERED {
                v0 = _mm256_add_ps(v0, _mm256_loadu_ps(center_ptr.add(index)));
                v1 = _mm256_add_ps(v1, _mm256_loadu_ps(center_ptr.add(index + 8)));
            }
            step_eight_avx2::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
            step_eight_avx2::<DOT>(q_ptr.add(index + 8), v1, factor, &mut low, &mut high);
            index += 16;
        }
        if index + 8 <= dim {
            let raw = _mm_loadl_epi64(row_ptr.add(index).cast());
            let c0 = _mm256_cvtepi8_epi32(raw);
            let mut v0 = _mm256_mul_ps(_mm256_cvtepi32_ps(c0), v_scale);
            if CENTERED {
                v0 = _mm256_add_ps(v0, _mm256_loadu_ps(center_ptr.add(index)));
            }
            step_eight_avx2::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
            index += 8;
        }
        finish_avx2::<DOT>(query, view, scale, index, low, high)
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma,f16c")]
#[inline]
unsafe fn accumulate_avx2_sq4_single<const DOT: bool, const CENTERED: bool>(
    query: &[f32],
    view: &CodeView<'_>,
    scale: f64,
) -> f64 {
    use std::arch::x86_64::*;
    // SAFETY：入口已校验半字节码字、单尺度参数与中心边界；单次解包 16 个 4-bit 码字并严格保持累加顺序。
    unsafe {
        let dim = query.len();
        let q_ptr = query.as_ptr();
        let row_ptr = view.row.as_ptr();
        let center_ptr = view.center.as_ptr();
        let v_scale = if dim >= 8 {
            _mm256_set1_ps(read_f32_le(row_ptr.add(view.code_bytes)))
        } else {
            _mm256_setzero_ps()
        };
        let mask = _mm_set1_epi8(15);
        let seven = _mm256_set1_epi32(7);
        let factor = _mm256_set1_pd(scale);
        let mut low = _mm256_setzero_pd();
        let mut high = _mm256_setzero_pd();
        let mut index = 0usize;
        while index + 16 <= dim {
            let bytes = _mm_loadl_epi64(row_ptr.add(index / 2).cast());
            let unpacked = _mm_unpacklo_epi8(
                _mm_and_si128(bytes, mask),
                _mm_and_si128(_mm_srli_epi16::<4>(bytes), mask),
            );
            let c0 = _mm256_sub_epi32(_mm256_cvtepu8_epi32(unpacked), seven);
            let c1 = _mm256_sub_epi32(_mm256_cvtepu8_epi32(_mm_bsrli_si128::<8>(unpacked)), seven);
            let mut v0 = _mm256_mul_ps(_mm256_cvtepi32_ps(c0), v_scale);
            let mut v1 = _mm256_mul_ps(_mm256_cvtepi32_ps(c1), v_scale);
            if CENTERED {
                v0 = _mm256_add_ps(v0, _mm256_loadu_ps(center_ptr.add(index)));
                v1 = _mm256_add_ps(v1, _mm256_loadu_ps(center_ptr.add(index + 8)));
            }
            step_eight_avx2::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
            step_eight_avx2::<DOT>(q_ptr.add(index + 8), v1, factor, &mut low, &mut high);
            index += 16;
        }
        if index + 8 <= dim {
            let packed = read_u32_le(row_ptr.add(index / 2)) as i32;
            let bytes = _mm_cvtsi32_si128(packed);
            let unpacked = _mm_unpacklo_epi8(
                _mm_and_si128(bytes, mask),
                _mm_and_si128(_mm_srli_epi16::<4>(bytes), mask),
            );
            let c0 = _mm256_sub_epi32(_mm256_cvtepu8_epi32(unpacked), seven);
            let mut v0 = _mm256_mul_ps(_mm256_cvtepi32_ps(c0), v_scale);
            if CENTERED {
                v0 = _mm256_add_ps(v0, _mm256_loadu_ps(center_ptr.add(index)));
            }
            step_eight_avx2::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
            index += 8;
        }
        finish_avx2::<DOT>(query, view, scale, index, low, high)
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma,f16c")]
unsafe fn accumulate_avx2<const DOT: bool>(query: &[f32], view: &CodeView<'_>, scale: f64) -> f64 {
    use std::arch::x86_64::*;
    assert_view_bounds(query.len(), view);
    let centered = !view.center.is_empty();
    // SAFETY：外层已校验指令集及切片边界，按编码格式单态特化热循环并保持逐维浮点舍入语义。
    unsafe {
        match (view.precision, view.affine, centered) {
            (RerankPrecision::Sq8, false, false) => {
                accumulate_avx2_sq8_single::<DOT, false>(query, view, scale)
            }
            (RerankPrecision::Sq8, false, true) => {
                accumulate_avx2_sq8_single::<DOT, true>(query, view, scale)
            }
            (RerankPrecision::Sq4, false, false) => {
                accumulate_avx2_sq4_single::<DOT, false>(query, view, scale)
            }
            (RerankPrecision::Sq4, false, true) => {
                accumulate_avx2_sq4_single::<DOT, true>(query, view, scale)
            }
            (RerankPrecision::Fp16, _, false) => {
                let dim = query.len();
                let q_ptr = query.as_ptr();
                let row_ptr = view.row.as_ptr();
                let factor = _mm256_set1_pd(scale);
                let mut low = _mm256_setzero_pd();
                let mut high = _mm256_setzero_pd();
                let mut index = 0usize;
                while index + 16 <= dim {
                    let v0 = _mm256_cvtph_ps(_mm_loadu_si128(row_ptr.add(index * 2).cast()));
                    let v1 = _mm256_cvtph_ps(_mm_loadu_si128(row_ptr.add((index + 8) * 2).cast()));
                    step_eight_avx2::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
                    step_eight_avx2::<DOT>(q_ptr.add(index + 8), v1, factor, &mut low, &mut high);
                    index += 16;
                }
                if index + 8 <= dim {
                    let v0 = _mm256_cvtph_ps(_mm_loadu_si128(row_ptr.add(index * 2).cast()));
                    step_eight_avx2::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
                    index += 8;
                }
                finish_avx2::<DOT>(query, view, scale, index, low, high)
            }
            (RerankPrecision::Bf16, _, false) => {
                let dim = query.len();
                let q_ptr = query.as_ptr();
                let row_ptr = view.row.as_ptr();
                let factor = _mm256_set1_pd(scale);
                let mut low = _mm256_setzero_pd();
                let mut high = _mm256_setzero_pd();
                let mut index = 0usize;
                while index + 8 <= dim {
                    let packed = _mm_loadu_si128(row_ptr.add(index * 2).cast());
                    let v =
                        _mm256_castsi256_ps(_mm256_slli_epi32::<16>(_mm256_cvtepu16_epi32(packed)));
                    step_eight_avx2::<DOT>(q_ptr.add(index), v, factor, &mut low, &mut high);
                    index += 8;
                }
                finish_avx2::<DOT>(query, view, scale, index, low, high)
            }
            (RerankPrecision::Fp32, _, false) => {
                let dim = query.len();
                let q_ptr = query.as_ptr();
                let row_ptr = view.row.as_ptr();
                let factor = _mm256_set1_pd(scale);
                let mut low = _mm256_setzero_pd();
                let mut high = _mm256_setzero_pd();
                let mut index = 0usize;
                while index + 16 <= dim {
                    let v0 = _mm256_loadu_ps(row_ptr.add(index * 4).cast());
                    let v1 = _mm256_loadu_ps(row_ptr.add((index + 8) * 4).cast());
                    step_eight_avx2::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
                    step_eight_avx2::<DOT>(q_ptr.add(index + 8), v1, factor, &mut low, &mut high);
                    index += 16;
                }
                if index + 8 <= dim {
                    let v0 = _mm256_loadu_ps(row_ptr.add(index * 4).cast());
                    step_eight_avx2::<DOT>(q_ptr.add(index), v0, factor, &mut low, &mut high);
                    index += 8;
                }
                finish_avx2::<DOT>(query, view, scale, index, low, high)
            }
            _ => {
                // 分组仿射（group_size 为 8 的整数倍）及兜底组合：按组步进更新尺度与偏置，避免逐块整数除法。
                let dim = query.len();
                let q_ptr = query.as_ptr();
                let row_ptr = view.row.as_ptr();
                let center_ptr = view.center.as_ptr();
                let mask = _mm_set1_epi8(15);
                let seven = _mm256_set1_epi32(7);
                let factor = _mm256_set1_pd(scale);
                let mut low = _mm256_setzero_pd();
                let mut high = _mm256_setzero_pd();
                let mut param_ptr = row_ptr.add(view.code_bytes);
                let mut rem_in_group = 0usize;
                let mut v_scale = _mm256_setzero_ps();
                let mut v_bias = _mm256_setzero_ps();
                let mut index = 0usize;
                while index + 8 <= dim {
                    if rem_in_group == 0 {
                        v_scale = _mm256_set1_ps(read_f32_le(param_ptr));
                        v_bias = if view.affine {
                            _mm256_set1_ps(read_f32_le(param_ptr.add(4)))
                        } else {
                            _mm256_setzero_ps()
                        };
                        if view.affine {
                            param_ptr = param_ptr.add(8);
                        }
                        rem_in_group = view.group;
                    }
                    let codes = if view.precision == RerankPrecision::Sq8 {
                        _mm256_cvtepi8_epi32(_mm_loadl_epi64(row_ptr.add(index).cast()))
                    } else {
                        let packed = read_u32_le(row_ptr.add(index / 2)) as i32;
                        let bytes = _mm_cvtsi32_si128(packed);
                        let unpacked = _mm_unpacklo_epi8(
                            _mm_and_si128(bytes, mask),
                            _mm_and_si128(_mm_srli_epi16::<4>(bytes), mask),
                        );
                        _mm256_sub_epi32(_mm256_cvtepu8_epi32(unpacked), seven)
                    };
                    let mut v =
                        _mm256_add_ps(_mm256_mul_ps(_mm256_cvtepi32_ps(codes), v_scale), v_bias);
                    if centered {
                        v = _mm256_add_ps(v, _mm256_loadu_ps(center_ptr.add(index)));
                    }
                    step_eight_avx2::<DOT>(q_ptr.add(index), v, factor, &mut low, &mut high);
                    index += 8;
                    rem_in_group -= 8;
                }
                finish_avx2::<DOT>(query, view, scale, index, low, high)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::pelorus::engine::Metric;
    use crate::index::pelorus::engine::storage::{Buffer, ColdStore};

    #[test]
    fn 各精度分组尾部与两种评分的标量和硬件内核一致() {
        for dim in [
            1, 7, 8, 15, 16, 17, 24, 31, 32, 33, 48, 63, 64, 65, 129, 512,
        ] {
            let source = (0..3 * dim)
                .map(|i| ((i * 73 % 127) as f32 - 63.0) * 0.002 / (dim as f32).sqrt())
                .collect::<Vec<_>>();
            let query = &source[dim..2 * dim];
            for precision in [
                RerankPrecision::Fp32,
                RerankPrecision::Fp16,
                RerankPrecision::Bf16,
                RerankPrecision::Sq8,
                RerankPrecision::Sq4,
            ] {
                for quant in [
                    ScalarQuantization::Symmetric,
                    ScalarQuantization::Centered,
                    ScalarQuantization::Affine { group_size: 3 },
                    ScalarQuantization::Affine { group_size: 8 },
                    ScalarQuantization::Affine { group_size: 16 },
                    ScalarQuantization::Affine { group_size: 64 },
                ] {
                    for representation in [
                        super::super::RerankRepresentation::Cartesian,
                        super::super::RerankRepresentation::Polar,
                    ] {
                        let config = RerankConfig {
                            precision,
                            scalar_quantization: quant,
                            representation,
                            norm_correction: false,
                        };
                        if config.validate().is_err() {
                            continue;
                        }
                        let store = ColdStore::build(
                            config,
                            dim,
                            &Buffer::owned(source.clone()),
                            &[true; 3],
                            Metric::Poincare { curvature: 1.0 },
                        )
                        .unwrap();
                        let view = CodeView::new(
                            store.bytes.row(0, store.stride),
                            config,
                            &store.center,
                            dim,
                        );
                        for scale in [0.0, 0.31, 1.0, 1.27] {
                            for dot in [false, true] {
                                let expected = accumulate_scalar(query, &view, scale, dot);
                                let actual =
                                    accumulate(query, view.row, config, &store.center, scale, dot);
                                assert!(
                                    (actual - expected).abs() <= 1e-12 * expected.abs().max(1.0),
                                    "默认内核不符，维度{dim}，配置{config:?}"
                                );
                                #[cfg(target_arch = "x86_64")]
                                if *AVX2_READY && (!view.affine || view.group.is_multiple_of(8)) {
                                    // SAFETY：测试检查指令集及分组边界，记录由正式编码器生成。
                                    let value = unsafe {
                                        if dot {
                                            accumulate_avx2::<true>(query, &view, scale)
                                        } else {
                                            accumulate_avx2::<false>(query, &view, scale)
                                        }
                                    };
                                    assert!(
                                        (value - expected).abs() <= 1e-12 * expected.abs().max(1.0),
                                        "AVX2不符，维度{dim}，配置{config:?}"
                                    );
                                }
                                #[cfg(all(target_arch = "x86_64", not(coverage)))]
                                if *AVX512_READY && (!view.affine || view.group.is_multiple_of(16))
                                {
                                    // SAFETY：测试检查完整AVX-512子集及分组边界。
                                    let value = unsafe {
                                        if dot {
                                            accumulate_avx512::<true>(query, &view, scale)
                                        } else {
                                            accumulate_avx512::<false>(query, &view, scale)
                                        }
                                    };
                                    assert!(
                                        (value - expected).abs() <= 1e-12 * expected.abs().max(1.0),
                                        "AVX-512不符，维度{dim}，配置{config:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
