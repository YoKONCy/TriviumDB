//! 浮点与整数聚合在同一二进制定点尺度上累加，最后统一舍入。

use std::cmp::Ordering;

// 以最小 f64 次正规数为单位。含最高有效位以及 64 位项数所需的进位空间。
const WORDS: usize = 34;
const UNIT_EXPONENT: usize = 1074;
type Magnitude = [u64; WORDS];

pub(super) struct ExactSum {
    positive: Magnitude,
    negative: Magnitude,
}

impl Default for ExactSum {
    fn default() -> Self {
        Self {
            positive: [0; WORDS],
            negative: [0; WORDS],
        }
    }
}

impl ExactSum {
    pub(super) fn push_number(&mut self, number: &serde_json::Number) {
        if let Some(integer) = number.as_i64() {
            self.add(integer.is_negative(), integer.unsigned_abs(), UNIT_EXPONENT);
        } else if let Some(integer) = number.as_u64() {
            self.add(false, integer, UNIT_EXPONENT);
        } else {
            self.push_float(number.as_f64().expect("JSON 浮点数"));
        }
    }

    pub(super) fn push_float(&mut self, value: f64) {
        debug_assert!(value.is_finite());
        let bits = value.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as usize;
        let fraction = bits & ((1u64 << 52) - 1);
        let significand = fraction | (u64::from(exponent != 0) << 52);
        self.add(
            value.is_sign_negative(),
            significand,
            exponent.saturating_sub(1),
        );
    }

    fn add(&mut self, negative: bool, significand: u64, shift: usize) {
        let words = if negative {
            &mut self.negative
        } else {
            &mut self.positive
        };
        let index = shift / 64;
        let offset = shift % 64;
        add_word(words, index, significand << offset);
        if offset != 0 {
            add_word(words, index + 1, significand >> (64 - offset));
        }
    }

    pub(super) fn into_f64(self, divisor: usize) -> f64 {
        let (negative, significand, shift) = self.rounded(divisor, 52, 0);
        let bits = float_bits(significand, shift, 52, 0x7ff);
        f64::from_bits((u64::from(negative) << 63) | bits)
    }

    pub(super) fn into_f32(self, divisor: usize) -> f32 {
        const F32_UNIT_SHIFT: usize = UNIT_EXPONENT - 149;
        let (negative, significand, shift) = self.rounded(divisor, 23, F32_UNIT_SHIFT);
        let bits = float_bits(significand, shift - F32_UNIT_SHIFT, 23, 0xff);
        f32::from_bits((u32::from(negative) << 31) | bits as u32)
    }

    fn rounded(self, divisor: usize, fraction_bits: usize, min_shift: usize) -> (bool, u64, usize) {
        let (mut larger, smaller, negative) =
            match self.positive.iter().rev().cmp(self.negative.iter().rev()) {
                Ordering::Equal => return (false, 0, min_shift),
                Ordering::Greater => (self.positive, self.negative, false),
                Ordering::Less => (self.negative, self.positive, true),
            };
        let mut borrow = false;
        for (word, subtrahend) in larger.iter_mut().zip(smaller) {
            (*word, borrow) = word.borrowing_sub(subtrahend, borrow);
        }
        let divisor = divisor as u128;
        let mut remainder = 0;
        if divisor != 1 {
            for word in larger.iter_mut().rev() {
                let dividend = (remainder << 64) | u128::from(*word);
                *word = (dividend / divisor) as u64;
                remainder = dividend % divisor;
            }
        }
        let length = larger
            .iter()
            .rposition(|&word| word != 0)
            .map_or(0, |i| i * 64 + (64 - larger[i].leading_zeros() as usize));
        let shift = length.saturating_sub(fraction_bits + 1).max(min_shift);
        let index = shift / 64;
        let offset = shift % 64;
        let mut significand = larger[index] >> offset;
        if offset != 0 {
            significand |= larger.get(index + 1).copied().unwrap_or(0) << (64 - offset);
        }
        // 整数商被截去的位与除法余数一起参与最近偶数舍入。
        let round_up = if shift == 0 {
            let twice = remainder * 2;
            twice > divisor || (twice == divisor && significand & 1 != 0)
        } else {
            let midpoint = shift - 1;
            let word = larger[midpoint / 64];
            let bit = 1u64 << (midpoint % 64);
            word & bit != 0
                && (word & (bit - 1) != 0
                    || larger[..midpoint / 64].iter().any(|&word| word != 0)
                    || remainder != 0
                    || significand & 1 != 0)
        };
        (negative, significand + u64::from(round_up), shift)
    }
}

fn add_word(words: &mut Magnitude, mut index: usize, mut value: u64) {
    while value != 0 {
        let (sum, carry) = words[index].overflowing_add(value);
        words[index] = sum;
        value = u64::from(carry);
        index += 1;
    }
}

fn float_bits(significand: u64, shift: usize, fraction_bits: usize, max_exponent: u64) -> u64 {
    let implicit_bit = 1u64 << fraction_bits;
    if significand < implicit_bit {
        return significand;
    }
    let bits = ((shift as u64 + 1) << fraction_bits) + (significand - implicit_bit);
    bits.min(max_exponent << fraction_bits)
}
