//! 共享中心的版本化压缩重排；历史 codec 1 继续使用原编码和评分定义。
use super::*;

#[path = "kernels.rs"]
mod kernels;

#[cfg(test)]
#[path = "rerank_tests.rs"]
mod tests;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RerankEncoding {
    #[default]
    Int8,
    CenteredInt8,
    CenteredInt4,
}

impl RerankEncoding {
    pub fn is_centered(self) -> bool {
        self != Self::Int8
    }

    fn codec(self) -> u32 {
        match self {
            Self::Int8 => 1,
            Self::CenteredInt8 => 2,
            Self::CenteredInt4 => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryPrecision {
    Fp32,
    #[default]
    Int8,
    Int4,
}

/// 编码身份与行几何；共享中心由制品持有，行租约借用同一中心。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColdLayout {
    legacy: Int8Layout,
    encoding: RerankEncoding,
    code_bytes: usize,
    payload_bytes: usize,
    stride: usize,
}

impl ColdLayout {
    pub fn new(dimension: usize, alignment: usize, encoding: RerankEncoding) -> Result<Self> {
        let legacy = Int8Layout::new(dimension, alignment)?;
        let code_bytes = if encoding == RerankEncoding::CenteredInt4 {
            dimension.div_ceil(2)
        } else {
            dimension
        };
        let payload_bytes = code_bytes + legacy.groups() * 8 + 8;
        let stride = payload_bytes
            .checked_add(alignment - 1)
            .map(|value| value & !(alignment - 1))
            .filter(|&value| u32::try_from(value).is_ok())
            .ok_or_else(|| invalid("冷行对齐溢出 (Cold row alignment overflow)"))?;
        Ok(Self {
            legacy: if encoding.is_centered() {
                Int8Layout::new(dimension, 1)?
            } else {
                legacy
            },
            encoding,
            code_bytes,
            payload_bytes,
            stride,
        })
    }

    pub fn dimension(self) -> usize {
        self.legacy.dimension()
    }
    pub fn encoding(self) -> RerankEncoding {
        self.encoding
    }
    pub fn row_stride_bytes(self) -> usize {
        self.stride
    }
    pub fn payload_bytes(self) -> usize {
        self.payload_bytes
    }
    pub fn center_bytes(self) -> usize {
        if self.encoding.is_centered() {
            self.dimension() * 4
        } else {
            0
        }
    }

    pub fn to_header(self) -> [u8; INT8_LAYOUT_HEADER_BYTES] {
        let mut header = self.legacy.to_header();
        header[12..16].copy_from_slice(&self.encoding.codec().to_le_bytes());
        header[24..28].copy_from_slice(&(self.payload_bytes as u32).to_le_bytes());
        header[28..32].copy_from_slice(&(self.stride as u32).to_le_bytes());
        let crc = crc32fast::hash(&header[..36]);
        header[36..].copy_from_slice(&crc.to_le_bytes());
        header
    }

    pub fn from_header(header: &[u8]) -> Result<Self> {
        if header.len() != INT8_LAYOUT_HEADER_BYTES
            || &header[..8] != MAGIC
            || read_u32(header, 8) != VERSION
            || read_u32(header, 20) != 128
            || read_u32(header, 32) != 0
            || read_u32(header, 36) != crc32fast::hash(&header[..36])
        {
            return Err(corrupt("冷编码头无效 (Invalid cold codec header)"));
        }
        let encoding = match read_u32(header, 12) {
            1 => RerankEncoding::Int8,
            2 => RerankEncoding::CenteredInt8,
            3 => RerankEncoding::CenteredInt4,
            _ => return Err(corrupt("冷编码身份无效 (Invalid cold codec identity)")),
        };
        let dimension = read_u32(header, 16) as usize;
        if !(1..=MAX_BQ_DIM).contains(&dimension) {
            return Err(corrupt("冷编码维度无效 (Invalid cold codec dimension)"));
        }
        let mut layout = Self::new(dimension, 1, encoding)?;
        layout.stride = read_u32(header, 28) as usize;
        if layout.stride < layout.payload_bytes
            || read_u32(header, 24) as usize != layout.payload_bytes
        {
            return Err(corrupt(
                "冷编码行几何无效 (Invalid cold codec row geometry)",
            ));
        }
        if encoding == RerankEncoding::Int8 {
            layout.legacy = Int8Layout::from_header(header)?;
        }
        Ok(layout)
    }

    pub fn validate_center(self, center: &[f32]) -> Result<()> {
        let expected = self.center_bytes() / 4;
        if center.len() != expected || center.iter().any(|x| !x.is_finite() || x.abs() > 1.) {
            return Err(invalid("重排中心无效 (Invalid reranking center)").at_field("center"));
        }
        Ok(())
    }

    pub fn encode(self, vector: &[f32], center: &[f32], output: &mut [u8]) -> Result<()> {
        self.validate_center(center)?;
        if self.encoding == RerankEncoding::Int8 {
            return self.legacy.encode(vector, output);
        }
        check_vector(vector, self.dimension())?;
        if output.len() != self.stride {
            return Err(
                invalid("冷行输出长度无效 (Invalid cold row output length)").at_field("output")
            );
        }
        let norm = vector
            .iter()
            .map(|&x| f64::from(x).powi(2))
            .sum::<f64>()
            .sqrt();
        let mut residual = [0f32; MAX_BQ_DIM];
        for (i, &value) in vector.iter().enumerate() {
            let normalized = if norm == 0. {
                0.
            } else {
                (f64::from(value) / norm) as f32
            };
            residual[i] = normalized - center[i];
        }
        output.fill(0);
        let four = self.encoding == RerankEncoding::CenteredInt4;
        for (group, values) in residual[..self.dimension()].chunks(128).enumerate() {
            let low = values.iter().copied().fold(f32::INFINITY, f32::min);
            let high = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let bias = if four {
                low
            } else {
                ((f64::from(low) + f64::from(high)) * 0.5) as f32
            };
            let scale = if high == low {
                1.
            } else {
                (((f64::from(high) - f64::from(low)) / if four { 15. } else { 254. }) as f32)
                    .max(f32::from_bits(1))
            };
            let offset = self.code_bytes + group * 8;
            output[offset..offset + 4].copy_from_slice(&scale.to_le_bytes());
            output[offset + 4..offset + 8].copy_from_slice(&bias.to_le_bytes());
            for (i, &value) in values.iter().enumerate() {
                let code =
                    ((f64::from(value) - f64::from(bias)) / f64::from(scale)).round_ties_even();
                let index = group * 128 + i;
                if four {
                    output[index / 2] |= (code.clamp(0., 15.) as u8) << ((index % 2) * 4);
                } else {
                    output[index] = code.clamp(-127., 127.) as i8 as u8;
                }
            }
        }
        let factor = if norm == 0. {
            0.
        } else {
            self.factor(output, center)
        };
        output[self.payload_bytes - 8..self.payload_bytes - 4]
            .copy_from_slice(&factor.to_le_bytes());
        let crc = crc32fast::hash(&output[..self.payload_bytes - 4]);
        output[self.payload_bytes - 4..self.payload_bytes].copy_from_slice(&crc.to_le_bytes());
        Ok(())
    }

    fn code(self, bytes: &[u8], i: usize) -> i32 {
        if self.encoding == RerankEncoding::CenteredInt4 {
            i32::from((bytes[i / 2] >> ((i % 2) * 4)) & 15)
        } else {
            i32::from(bytes[i] as i8)
        }
    }

    fn reconstructed(self, bytes: &[u8], center: &[f32], i: usize) -> f64 {
        let offset = self.code_bytes + i / 128 * 8;
        f64::from(center[i])
            + (f64::from(self.code(bytes, i)) * f64::from(read_f32(bytes, offset))
                + f64::from(read_f32(bytes, offset + 4)))
    }

    fn factor(self, bytes: &[u8], center: &[f32]) -> f32 {
        let squared = (0..self.dimension())
            .map(|i| self.reconstructed(bytes, center, i).powi(2))
            .sum::<f64>();
        if squared == 0. {
            0.
        } else {
            (1. / squared.sqrt()) as f32
        }
    }

    pub fn row<'a>(self, bytes: &'a [u8], center: &'a [f32]) -> Result<ColdRow<'a>> {
        self.validate_center(center)
            .map_err(|_| corrupt("冷行中心无效 (Invalid cold row center)"))?;
        if self.encoding == RerankEncoding::Int8 {
            self.legacy.row(bytes)?;
        } else {
            if bytes.len() != self.stride
                || read_u32(bytes, self.payload_bytes - 4)
                    != crc32fast::hash(&bytes[..self.payload_bytes - 4])
                || bytes[self.payload_bytes..].iter().any(|&x| x != 0)
                || (self.encoding == RerankEncoding::CenteredInt8
                    && bytes[..self.code_bytes].contains(&128))
                || (self.encoding == RerankEncoding::CenteredInt4
                    && self.dimension() % 2 == 1
                    && bytes[self.code_bytes - 1] & 0xf0 != 0)
            {
                return Err(corrupt("冷行编码或CRC无效 (Invalid cold row codes or CRC)"));
            }
            for group in 0..self.legacy.groups() {
                let offset = self.code_bytes + group * 8;
                let scale = read_f32(bytes, offset);
                if !scale.is_finite() || scale <= 0. || !read_f32(bytes, offset + 4).is_finite() {
                    return Err(corrupt("冷行参数无效 (Invalid cold row parameters)"));
                }
            }
            let factor = read_f32(bytes, self.payload_bytes - 8);
            if !factor.is_finite()
                || factor < 0.
                || (factor != 0. && factor.to_bits() != self.factor(bytes, center).to_bits())
            {
                return Err(corrupt("冷行范数无效 (Invalid cold row norm)"));
            }
            if factor == 0. {
                // 零因子仅接受原始零向量的规范编码，重算CRC后修改正向量因子仍会被拒绝。
                let zeros = [0f32; MAX_BQ_DIM];
                let mut expected = [0u8; MAX_BQ_DIM + MAX_GROUPS * 8 + 8];
                let mut compact = self;
                compact.stride = self.payload_bytes;
                compact.encode(
                    &zeros[..self.dimension()],
                    center,
                    &mut expected[..self.payload_bytes],
                )?;
                if expected[..self.payload_bytes] != bytes[..self.payload_bytes] {
                    return Err(corrupt(
                        "冷行零因子编码无效 (Invalid zero-factor cold row encoding)",
                    ));
                }
            }
        }
        Ok(ColdRow {
            layout: self,
            bytes,
            center,
        })
    }

    /// 仅供已通过行校验且字节不可变的自有租约复用，长度和中心由构造时固定。
    pub(in crate::index::quiver) fn validated_row<'a>(
        self,
        bytes: &'a [u8],
        center: &'a [f32],
    ) -> ColdRow<'a> {
        ColdRow {
            layout: self,
            bytes,
            center,
        }
    }
}

#[derive(Clone, Copy)]
pub struct ColdRow<'a> {
    layout: ColdLayout,
    bytes: &'a [u8],
    center: &'a [f32],
}

impl ColdRow<'_> {
    pub fn decode_into(self, output: &mut [f32]) -> Result<()> {
        if output.len() != self.layout.dimension() {
            return Err(TriviumError::DimensionMismatch {
                expected: self.layout.dimension(),
                got: output.len(),
            });
        }
        if self.layout.encoding == RerankEncoding::Int8 {
            return Int8Row {
                layout: self.layout.legacy,
                bytes: self.bytes,
            }
            .decode_into(output);
        }
        let factor = f64::from(read_f32(self.bytes, self.layout.payload_bytes - 8));
        if factor == 0. {
            output.fill(0.);
            return Ok(());
        }
        for (i, value) in output.iter_mut().enumerate() {
            *value = (self.layout.reconstructed(self.bytes, self.center, i) * factor) as f32;
        }
        Ok(())
    }
}

/// 查询表示只准备一次，浮点以f64累加，整数使用分组i32点积。
pub struct RerankQuery {
    precision: QueryPrecision,
    dimension: usize,
    values: [f64; MAX_BQ_DIM],
    integers: Int8Query,
    sums: [f64; MAX_GROUPS],
    center: [f32; MAX_BQ_DIM],
    center_len: usize,
    center_dot: f64,
    inverse_norm: f64,
}

impl RerankQuery {
    pub fn from_int8(query: &Int8Query, center: &[f32]) -> Result<Self> {
        let mut values = [0f32; MAX_BQ_DIM];
        for (value, &code) in values.iter_mut().zip(&query.codes[..query.dimension]) {
            *value = f32::from(code);
        }
        Self::new(&values[..query.dimension], QueryPrecision::Int8, center)
    }
    pub fn new(query: &[f32], precision: QueryPrecision, center: &[f32]) -> Result<Self> {
        check_dimension(query.len())?;
        check_vector(query, query.len())?;
        if (!center.is_empty() && center.len() != query.len())
            || center.iter().any(|x| !x.is_finite() || x.abs() > 1.)
        {
            return Err(invalid("查询中心无效 (Invalid query center)").at_field("center"));
        }
        let mut integers = if precision == QueryPrecision::Int8 {
            Int8Query::new(query)?
        } else {
            Int8Query {
                dimension: query.len(),
                codes: [0; MAX_BQ_DIM],
                group_sums: [0; MAX_GROUPS],
                inverse_code_norm: 0.,
            }
        };
        if precision == QueryPrecision::Int4 {
            let maximum = query
                .iter()
                .map(|&x| f64::from(x).abs())
                .fold(0., f64::max)
                .max(f64::MIN_POSITIVE);
            for (code, &value) in integers.codes.iter_mut().zip(query) {
                *code = (f64::from(value) / maximum * 7.)
                    .round_ties_even()
                    .clamp(-7., 7.) as i8;
            }
            for (sum, codes) in integers
                .group_sums
                .iter_mut()
                .zip(integers.codes[..query.len()].chunks(128))
            {
                *sum = codes.iter().map(|&x| i32::from(x)).sum();
            }
        }
        let mut result = Self {
            precision,
            dimension: query.len(),
            values: [0.; MAX_BQ_DIM],
            integers,
            sums: [0.; MAX_GROUPS],
            center: [0.; MAX_BQ_DIM],
            center_len: center.len(),
            center_dot: 0.,
            inverse_norm: 0.,
        };
        for (i, &value) in query.iter().enumerate() {
            result.values[i] = if precision == QueryPrecision::Fp32 {
                f64::from(value)
            } else {
                f64::from(result.integers.codes[i])
            };
        }
        let norm = result.values[..query.len()]
            .iter()
            .map(|x| x * x)
            .sum::<f64>()
            .sqrt();
        result.inverse_norm = if norm == 0. { 0. } else { 1. / norm };
        if precision == QueryPrecision::Int4 {
            result.integers.inverse_code_norm = result.inverse_norm;
        }
        result.center[..center.len()].copy_from_slice(center);
        result.center_dot = result
            .values
            .iter()
            .zip(center)
            .map(|(&q, &m)| q * f64::from(m))
            .sum();
        for (sum, group) in result
            .sums
            .iter_mut()
            .zip(result.values[..query.len()].chunks(128))
        {
            *sum = group.iter().sum();
        }
        Ok(result)
    }

    pub fn score(&self, row: ColdRow<'_>) -> Result<f32> {
        if self.dimension != row.layout.dimension() {
            return Err(TriviumError::DimensionMismatch {
                expected: self.dimension,
                got: row.layout.dimension(),
            });
        }
        if self.center_len != row.center.len() || self.center[..row.center.len()] != *row.center {
            return Err(
                invalid("查询与冷行中心不匹配 (Query and cold row centers differ)")
                    .at_field("center"),
            );
        }
        let factor = f64::from(read_f32(row.bytes, row.layout.payload_bytes - 8));
        if factor == 0. || self.inverse_norm == 0. {
            return Ok(0.);
        }
        if row.layout.encoding == RerankEncoding::Int8 && self.precision == QueryPrecision::Int8 {
            return self.integers.score(Int8Row {
                layout: row.layout.legacy,
                bytes: row.bytes,
            });
        }
        let mut score = self.center_dot;
        for (group, query) in self.values[..self.dimension].chunks(128).enumerate() {
            let start = group * 128;
            let offset = row.layout.code_bytes + group * 8;
            let bits = if row.layout.encoding == RerankEncoding::CenteredInt4 {
                4
            } else {
                8
            };
            let code_start = if bits == 4 { start / 2 } else { start };
            let code_len = if bits == 4 {
                query.len().div_ceil(2)
            } else {
                query.len()
            };
            let codes = &row.bytes[code_start..code_start + code_len];
            let dot = if self.precision == QueryPrecision::Fp32 {
                kernels::floating(query, codes, bits)
            } else {
                f64::from(kernels::integer(
                    &self.integers.codes[start..start + query.len()],
                    codes,
                    bits,
                ))
            };
            score += dot * f64::from(read_f32(row.bytes, offset))
                + self.sums[group] * f64::from(read_f32(row.bytes, offset + 4));
        }
        Ok((score * factor * self.inverse_norm).clamp(-1., 1.) as f32)
    }
}
