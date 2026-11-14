//! AVX2压紧码评分；查询四位码每条查询展开一次，候选阶段复用整数工作区。
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[inline]
pub fn code_at(bytes: &[u8], index: usize, bits: usize) -> i32 {
    if bits == 4 {
        i32::from((bytes[index / 2] >> ((index % 2) * 4)) & 15)
    } else {
        i32::from(bytes[index] as i8)
    }
}

pub fn integer_scalar(query: &[i8], codes: &[u8], bits: usize) -> i32 {
    query
        .iter()
        .enumerate()
        .map(|(i, &q)| i32::from(q) * code_at(codes, i, bits))
        .sum()
}

pub fn float_scalar(query: &[f64], codes: &[u8], bits: usize) -> f64 {
    query
        .iter()
        .enumerate()
        .map(|(i, &q)| q * f64::from(code_at(codes, i, bits)))
        .sum()
}

pub fn integer(query: &[i8], codes: &[u8], bits: usize) -> i32 {
    #[cfg(target_arch = "x86_64")]
    if query.len() == 128
        && codes.len() == if bits == 4 { 64 } else { 128 }
        && std::is_x86_feature_detected!("avx2")
    {
        // SAFETY：调用前确认AVX2和完整组长度，读取范围限于128个查询码及对应编码组。
        return unsafe { integer_avx2(query, codes, bits) };
    }
    integer_scalar(query, codes, bits)
}

pub fn floating(query: &[f64], codes: &[u8], bits: usize) -> f64 {
    #[cfg(target_arch = "x86_64")]
    if query.len() == 128
        && codes.len() == if bits == 4 { 64 } else { 128 }
        && std::is_x86_feature_detected!("avx2")
    {
        // SAFETY：借用覆盖整组，查询和编码只读，函数内全部采用非对齐读取。
        return unsafe { floating_avx2(query, codes, bits) };
    }
    float_scalar(query, codes, bits)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn unpack32(pointer: *const u8, bits: usize) -> __m256i {
    unsafe {
        if bits == 4 {
            let packed = _mm256_cvtepu8_epi16(_mm_loadu_si128(pointer.cast()));
            let low = _mm256_and_si256(packed, _mm256_set1_epi16(15));
            let high = _mm256_and_si256(_mm256_srli_epi16::<4>(packed), _mm256_set1_epi16(15));
            _mm256_or_si256(low, _mm256_slli_epi16::<8>(high))
        } else {
            _mm256_loadu_si256(pointer.cast())
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn integer_avx2(query: &[i8], codes: &[u8], bits: usize) -> i32 {
    unsafe {
        let mut sum = _mm256_setzero_si256();
        for i in (0..128).step_by(32) {
            let db = unpack32(codes.as_ptr().add(if bits == 4 { i / 2 } else { i }), bits);
            let q = _mm256_loadu_si256(query.as_ptr().add(i).cast());
            let a = _mm256_madd_epi16(
                _mm256_cvtepi8_epi16(_mm256_castsi256_si128(db)),
                _mm256_cvtepi8_epi16(_mm256_castsi256_si128(q)),
            );
            let b = _mm256_madd_epi16(
                _mm256_cvtepi8_epi16(_mm256_extracti128_si256::<1>(db)),
                _mm256_cvtepi8_epi16(_mm256_extracti128_si256::<1>(q)),
            );
            sum = _mm256_add_epi32(sum, _mm256_add_epi32(a, b));
        }
        // 完整组最大127×127×128，整数扩展及累加均处于i32范围内。
        let mut lanes = [0i32; 8];
        _mm256_storeu_si256(lanes.as_mut_ptr().cast(), sum);
        lanes.iter().sum()
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn floating_avx2(query: &[f64], codes: &[u8], bits: usize) -> f64 {
    unsafe {
        let mut sum = _mm256_setzero_pd();
        for i in (0..128).step_by(32) {
            let db = unpack32(codes.as_ptr().add(if bits == 4 { i / 2 } else { i }), bits);
            let halves = [
                _mm256_castsi256_si128(db),
                _mm256_extracti128_si256::<1>(db),
            ];
            for (half, packed) in halves.into_iter().enumerate() {
                let first = _mm256_cvtepi8_epi32(packed);
                let second = _mm256_cvtepi8_epi32(_mm_srli_si128::<8>(packed));
                for (part, expanded) in [first, second].into_iter().enumerate() {
                    let low = _mm256_cvtepi32_pd(_mm256_castsi256_si128(expanded));
                    let high = _mm256_cvtepi32_pd(_mm256_extracti128_si256::<1>(expanded));
                    let offset = i + half * 16 + part * 8;
                    sum = _mm256_add_pd(
                        sum,
                        _mm256_mul_pd(_mm256_loadu_pd(query.as_ptr().add(offset)), low),
                    );
                    sum = _mm256_add_pd(
                        sum,
                        _mm256_mul_pd(_mm256_loadu_pd(query.as_ptr().add(offset + 4)), high),
                    );
                }
            }
        }
        let mut lanes = [0f64; 4];
        _mm256_storeu_pd(lanes.as_mut_ptr(), sum);
        lanes.iter().sum()
    }
}
