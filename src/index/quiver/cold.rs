//! QuIVer 冷向量的版本化 INT8 编码基底。
//!
//! 每行先稳定归一化，再按实际维度分组编码，保留原始向量作为事实来源。
//! 布局头和行均使用小端字段与 CRC；调用方提供行缓冲，编码工作区大小固定。
//!
//! ```
//! use triviumdb::index::quiver::cold::{Int8Layout, Int8Query};
//!
//! let layout = Int8Layout::new(4, 64)?;
//! let mut bytes = vec![0; layout.row_stride_bytes()];
//! layout.encode(&[1f32, -1., 1., -1.], &mut bytes)?;
//! let row = layout.row(&bytes)?;
//! assert_eq!(Int8Query::new(&[1f32, -1., 1., -1.])?.score(row)?, 1.);
//! # Ok::<(), triviumdb::TriviumError>(())
//! ```

use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::index::bq::MAX_BQ_DIM;
use crate::vector::{is_binary_vector, validate_query_vector};
use std::ops::Range;

#[path = "cold/rerank.rs"]
mod rerank;
pub use rerank::{ColdLayout, ColdRow, QueryPrecision, RerankEncoding, RerankQuery};

/// 每个完整量化组的维度数；末组使用实际剩余维度。
pub const INT8_GROUP_DIM: usize = 128;
/// 布局头的固定字节数，供容器目录读取和验证。
pub const INT8_LAYOUT_HEADER_BYTES: usize = 40;

const MAGIC: &[u8; 8] = b"QVCOLD01";
const VERSION: u32 = 1;
// codec 1 固定归一化、group128 仿射 INT8、重建范数校正和量化余弦语义。
const CODEC: u32 = 1;
const MAX_GROUPS: usize = MAX_BQ_DIM.div_ceil(INT8_GROUP_DIM);

/// 冷行几何；有效载荷含行 CRC，行步长另含零填充。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Int8Layout {
    dimension: usize,
    payload_bytes: usize,
    row_stride_bytes: usize,
}

impl Int8Layout {
    /// 根据维度及二次幂行对齐生成布局；物理 I/O 对齐由读取器管理。
    pub fn new(dimension: usize, row_alignment: usize) -> Result<Self> {
        check_dimension(dimension)?;
        if !row_alignment.is_power_of_two() {
            return Err(
                invalid("行对齐必须为二次幂 (Row alignment must be a power of two)")
                    .at_field("row_alignment"),
            );
        }
        let payload_bytes = dimension + dimension.div_ceil(INT8_GROUP_DIM) * 8 + 8;
        let row_stride_bytes = payload_bytes
            .checked_add(row_alignment - 1)
            .map(|length| length & !(row_alignment - 1))
            .filter(|&length| u32::try_from(length).is_ok())
            .ok_or_else(|| {
                invalid("冷行步长超出格式范围 (Cold row stride exceeds the format range)")
                    .at_field("row_alignment")
            })?;
        Ok(Self {
            dimension,
            payload_bytes,
            row_stride_bytes,
        })
    }

    pub fn dimension(self) -> usize {
        self.dimension
    }

    pub fn groups(self) -> usize {
        self.dimension.div_ceil(INT8_GROUP_DIM)
    }

    /// 返回编码、参数、范数校正及 CRC 的总字节数。
    pub fn payload_bytes(self) -> usize {
        self.payload_bytes
    }

    pub fn row_stride_bytes(self) -> usize {
        self.row_stride_bytes
    }

    /// 返回相对于冷行区起点的总长度，计数溢出先于文件访问被拒绝。
    pub fn rows_bytes(self, rows: u64) -> Result<u64> {
        rows.checked_mul(self.row_stride_bytes as u64)
            .ok_or_else(|| {
                invalid("冷行区长度溢出 (Cold row section length overflow)").at_field("rows")
            })
    }

    /// 返回行区内的字节范围；容器目录另行绑定节点、slot 和事实代际。
    pub fn row_range(self, row: u64, rows: u64) -> Result<Range<u64>> {
        self.rows_bytes(rows)?;
        if row >= rows {
            return Err(invalid("冷行编号越界 (Cold row ordinal out of bounds)").at_field("row"));
        }
        let start = row * self.row_stride_bytes as u64;
        Ok(start..start + self.row_stride_bytes as u64)
    }

    /// 写出显式布局头；字段顺序独立于 Rust 结构布局。
    pub fn to_header(self) -> [u8; INT8_LAYOUT_HEADER_BYTES] {
        let mut header = [0u8; INT8_LAYOUT_HEADER_BYTES];
        header[..8].copy_from_slice(MAGIC);
        let fields = [
            VERSION,
            CODEC,
            self.dimension as u32,
            INT8_GROUP_DIM as u32,
            self.payload_bytes as u32,
            self.row_stride_bytes as u32,
            0,
        ];
        for (bytes, value) in header[8..36].as_chunks_mut::<4>().0.iter_mut().zip(fields) {
            bytes.copy_from_slice(&value.to_le_bytes());
        }
        let checksum = crc32fast::hash(&header[..36]);
        header[36..].copy_from_slice(&checksum.to_le_bytes());
        header
    }

    /// 校验布局头后恢复几何，整个过程使用固定大小工作区。
    pub fn from_header(header: &[u8]) -> Result<Self> {
        if header.len() != INT8_LAYOUT_HEADER_BYTES {
            return Err(corrupt(
                "冷布局头长度无效 (Invalid cold layout header length)",
            ));
        }
        if &header[..8] != MAGIC || crc32fast::hash(&header[..36]) != read_u32(header, 36) {
            return Err(corrupt(
                "冷布局头身份或CRC无效 (Invalid cold layout identity or CRC)",
            ));
        }
        if read_u32(header, 8) != VERSION
            || read_u32(header, 12) != CODEC
            || read_u32(header, 20) != INT8_GROUP_DIM as u32
            || read_u32(header, 32) != 0
        {
            return Err(corrupt(
                "冷布局版本或编码参数无效 (Invalid cold layout version or codec parameters)",
            ));
        }
        let dimension = read_u32(header, 16) as usize;
        if !(1..=MAX_BQ_DIM).contains(&dimension) {
            return Err(corrupt("冷布局维度无效 (Invalid cold layout dimension)"));
        }
        let mut layout = Self::new(dimension, 1)?;
        let stride = read_u32(header, 28) as usize;
        if read_u32(header, 24) as usize != layout.payload_bytes || stride < layout.payload_bytes {
            return Err(corrupt("冷布局行几何无效 (Invalid cold row geometry)"));
        }
        layout.row_stride_bytes = stride;
        Ok(layout)
    }

    /// 编码一条浮点向量；参数拒绝发生在修改调用方缓冲之前。
    pub fn encode<T: VectorType>(self, vector: &[T], output: &mut [u8]) -> Result<()> {
        check_vector(vector, self.dimension)?;
        if output.len() != self.row_stride_bytes {
            return Err(
                invalid("冷行输出长度不匹配 (Cold row output length mismatch)").at_field("output"),
            );
        }
        let mut normalized = [0f32; MAX_BQ_DIM];
        let squared_norm = vector
            .iter()
            .map(|value| f64::from(value.to_f32()).powi(2))
            .sum::<f64>();
        let norm = if squared_norm == 0. {
            1.
        } else {
            squared_norm.sqrt()
        };
        for (target, value) in normalized.iter_mut().zip(vector) {
            *target = (f64::from(value.to_f32()) / norm) as f32;
        }
        output.fill(0);
        for (group, values) in normalized[..self.dimension]
            .chunks(INT8_GROUP_DIM)
            .enumerate()
        {
            let low = values.iter().copied().fold(f32::INFINITY, f32::min);
            let high = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let bias = ((f64::from(low) + f64::from(high)) * 0.5) as f32;
            let scale = if low == high {
                1.
            } else {
                (((f64::from(high) - f64::from(low)) / 254.) as f32).max(f32::from_bits(1))
            };
            let metadata = self.dimension + group * 8;
            output[metadata..metadata + 4].copy_from_slice(&scale.to_le_bytes());
            output[metadata + 4..metadata + 8].copy_from_slice(&bias.to_le_bytes());
            for (code, &value) in output[group * INT8_GROUP_DIM..][..values.len()]
                .iter_mut()
                .zip(values)
            {
                *code = ((f64::from(value) - f64::from(bias)) / f64::from(scale))
                    .round_ties_even()
                    .clamp(-127., 127.) as i8 as u8;
            }
        }
        let factor = reconstruction_factor(self, output);
        let factor_offset = self.payload_bytes - 8;
        output[factor_offset..factor_offset + 4].copy_from_slice(&factor.to_le_bytes());
        let checksum = crc32fast::hash(&output[..self.payload_bytes - 4]);
        output[self.payload_bytes - 4..self.payload_bytes].copy_from_slice(&checksum.to_le_bytes());
        Ok(())
    }

    /// 验证冷行并借用其字节；校验包括 CRC、参数、规范范数与零填充。
    pub fn row(self, bytes: &[u8]) -> Result<Int8Row<'_>> {
        if bytes.len() != self.row_stride_bytes {
            return Err(corrupt("冷行长度不匹配 (Cold row length mismatch)"));
        }
        if crc32fast::hash(&bytes[..self.payload_bytes - 4])
            != read_u32(bytes, self.payload_bytes - 4)
            || bytes[self.payload_bytes..].iter().any(|&byte| byte != 0)
            || bytes[..self.dimension].contains(&128)
        {
            return Err(corrupt(
                "冷行CRC、编码或填充无效 (Invalid cold row CRC, codes, or padding)",
            ));
        }
        for group in 0..self.groups() {
            let offset = self.dimension + group * 8;
            let scale = read_f32(bytes, offset);
            let bias = read_f32(bytes, offset + 4);
            if !scale.is_finite() || scale <= 0. || !bias.is_finite() {
                return Err(corrupt(
                    "冷行仿射参数无效 (Invalid cold row affine parameters)",
                ));
            }
        }
        let factor = read_f32(bytes, self.payload_bytes - 8);
        if !factor.is_finite()
            || factor <= 0.
            || factor.to_bits() != reconstruction_factor(self, bytes).to_bits()
        {
            return Err(corrupt(
                "冷行范数校正无效 (Invalid cold row norm correction)",
            ));
        }
        Ok(Int8Row {
            layout: self,
            bytes,
        })
    }
}

/// 经过完整校验的冷行视图，生命周期由调用方缓冲或缓存租约限定。
#[derive(Debug, Clone, Copy)]
pub struct Int8Row<'a> {
    layout: Int8Layout,
    bytes: &'a [u8],
}

impl Int8Row<'_> {
    pub fn layout(self) -> Int8Layout {
        self.layout
    }

    /// 返回范数校正后的近似单位向量，供诊断及编码契约验证。
    pub fn decode_into(self, output: &mut [f32]) -> Result<()> {
        if output.len() != self.layout.dimension {
            return Err(TriviumError::DimensionMismatch {
                expected: self.layout.dimension,
                got: output.len(),
            });
        }
        let factor = f64::from(read_f32(self.bytes, self.layout.payload_bytes - 8));
        for (group, values) in output.chunks_mut(INT8_GROUP_DIM).enumerate() {
            let offset = self.layout.dimension + group * 8;
            let scale = f64::from(read_f32(self.bytes, offset));
            let bias = f64::from(read_f32(self.bytes, offset + 4));
            for (value, &code) in values.iter_mut().zip(&self.bytes[group * INT8_GROUP_DIM..]) {
                *value = ((f64::from(code as i8) * scale + bias) * factor) as f32;
            }
        }
        Ok(())
    }
}

/// 全局对称量化查询；固定工作区避免依赖冷行数量分配内存。
pub struct Int8Query {
    dimension: usize,
    codes: [i8; MAX_BQ_DIM],
    group_sums: [i32; MAX_GROUPS],
    inverse_code_norm: f64,
}

impl Int8Query {
    pub fn new<T: VectorType>(query: &[T]) -> Result<Self> {
        check_dimension(query.len())?;
        check_vector(query, query.len())?;
        let maximum = query
            .iter()
            .map(|value| f64::from(value.to_f32()).abs())
            .fold(0f64, f64::max);
        let denominator = if maximum == 0. { 1. } else { maximum };
        let mut result = Self {
            dimension: query.len(),
            codes: [0; MAX_BQ_DIM],
            group_sums: [0; MAX_GROUPS],
            inverse_code_norm: 0.,
        };
        for (code, value) in result.codes.iter_mut().zip(query) {
            *code = (f64::from(value.to_f32()) / denominator * 127.)
                .round_ties_even()
                .clamp(-127., 127.) as i8;
        }
        for (sum, group) in result
            .group_sums
            .iter_mut()
            .zip(result.codes[..query.len()].chunks(INT8_GROUP_DIM))
        {
            *sum = group.iter().map(|&code| i32::from(code)).sum();
        }
        let squared_norm = result.codes[..query.len()]
            .iter()
            .map(|&code| f64::from(code).powi(2))
            .sum::<f64>();
        if squared_norm > 0. {
            result.inverse_code_norm = 1. / squared_norm.sqrt();
        }
        Ok(result)
    }

    /// 按有效维度执行分组 i32 点积，再以 f64 恢复仿射项和量化余弦。
    pub fn score(&self, row: Int8Row<'_>) -> Result<f32> {
        if self.dimension != row.layout.dimension {
            return Err(TriviumError::DimensionMismatch {
                expected: self.dimension,
                got: row.layout.dimension,
            });
        }
        let mut score = 0f64;
        for (group, query) in self.codes[..self.dimension]
            .chunks(INT8_GROUP_DIM)
            .enumerate()
        {
            let offset = self.dimension + group * 8;
            let codes = &row.bytes[group * INT8_GROUP_DIM..][..query.len()];
            // 每组最多128项，127*127*128处于i32范围内，尾组只累加真实分量。
            let dot = query
                .iter()
                .zip(codes)
                .map(|(&q, &x)| i32::from(q) * i32::from(x as i8))
                .sum::<i32>();
            score += f64::from(dot) * f64::from(read_f32(row.bytes, offset))
                + f64::from(self.group_sums[group]) * f64::from(read_f32(row.bytes, offset + 4));
        }
        let factor = f64::from(read_f32(row.bytes, row.layout.payload_bytes - 8));
        Ok((score * factor * self.inverse_code_norm).clamp(-1., 1.) as f32)
    }
}

fn check_dimension(dimension: usize) -> Result<()> {
    if !(1..=MAX_BQ_DIM).contains(&dimension) {
        return Err(invalid(
            "冷向量维度必须在1至3072范围内 (Cold vector dimension must be within 1 to 3072)",
        )
        .at_field("dimension"));
    }
    Ok(())
}

fn check_vector<T: VectorType>(vector: &[T], dimension: usize) -> Result<()> {
    if is_binary_vector::<T>() {
        return Err(invalid(
            "INT8冷编码适用于浮点向量 (INT8 cold encoding requires floating-point vectors)",
        )
        .at_field("dtype"));
    }
    validate_query_vector(vector, dimension)
}

fn reconstruction_factor(layout: Int8Layout, bytes: &[u8]) -> f32 {
    let mut squared_norm = 0f64;
    for (group, codes) in bytes[..layout.dimension].chunks(INT8_GROUP_DIM).enumerate() {
        let offset = layout.dimension + group * 8;
        let scale = f64::from(read_f32(bytes, offset));
        let bias = f64::from(read_f32(bytes, offset + 4));
        for &code in codes {
            squared_norm += (f64::from(code as i8) * scale + bias).powi(2);
        }
    }
    if squared_norm == 0. {
        1.
    } else {
        (1. / squared_norm.sqrt()) as f32
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_f32(bytes: &[u8], offset: usize) -> f32 {
    f32::from_bits(read_u32(bytes, offset))
}

fn invalid(message: &str) -> TriviumError {
    TriviumError::InvalidInput(message.into())
}

fn corrupt(message: &str) -> TriviumError {
    TriviumError::CorruptedFile(message.into())
}

#[cfg(test)]
#[path = "cold/tests.rs"]
mod tests;
