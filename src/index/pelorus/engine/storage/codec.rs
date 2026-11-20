//! 重排向量的实际位编码、解码与 FP64 距离计算。

use super::*;
use crate::index::pelorus::engine::metric::{Metric, norm_squared_f64};
use half::{bf16, f16};

pub(crate) struct ColdStore {
    pub(crate) config: RerankConfig,
    pub(crate) dimensions: usize,
    pub(crate) center: Vec<f32>,
    pub(crate) bytes: Buffer<u8>,
    pub(crate) stride: usize,
    pub(crate) prepared: Vec<PreparedRow>,
}

#[derive(Clone, Copy)]
pub(crate) struct PreparedRow {
    scale: f64,
    multiplier: f64,
}

/// 每次公开查询独立创建或从线程局部缓冲复用，查询范数与 SSD 批重排切片均保持稳态零分配。
#[derive(Default)]
pub(crate) struct RerankScratch {
    pub(crate) query_norm: Option<f64>,
}

impl RerankScratch {
    pub(crate) fn reset_query(&mut self) {
        self.query_norm = None;
    }
}

impl ColdStore {
    pub(crate) fn build(
        config: RerankConfig,
        dimensions: usize,
        vectors: &Buffer<f32>,
        active: &[bool],
        metric: Metric,
    ) -> Result<Self> {
        config.validate()?;
        let mut store = Self {
            config,
            dimensions,
            center: Vec::new(),
            bytes: Buffer::owned(Vec::new()),
            stride: 0,
            prepared: Vec::new(),
        };
        if config.scalar_quantization == ScalarQuantization::Centered {
            let mut sums = vec![0.0f64; dimensions];
            let mut count = 0usize;
            for (slot, &live) in active.iter().enumerate() {
                if !live {
                    continue;
                }
                let vector = vectors.row(slot, dimensions);
                let radius = norm_squared_f64(vector).sqrt();
                for (sum, &value) in sums.iter_mut().zip(vector) {
                    let value =
                        if config.representation == RerankRepresentation::Polar && radius > 0.0 {
                            (f64::from(value) / radius) as f32
                        } else {
                            value
                        };
                    *sum += f64::from(value);
                }
                count += 1;
            }
            store.center = sums
                .into_iter()
                .map(|sum| (sum / count.max(1) as f64) as f32)
                .collect();
        }
        store.stride = store.record_bytes()?;
        store.bytes.reserve(
            active
                .len()
                .checked_mul(store.stride)
                .ok_or(Error::CapacityExceeded)?,
        )?;
        store
            .prepared
            .try_reserve_exact(active.len())
            .map_err(|_| Error::CapacityExceeded)?;
        let empty = vec![0.0; dimensions];
        for (slot, &live) in active.iter().enumerate() {
            let vector = if live {
                vectors.row(slot, dimensions)
            } else {
                &empty
            };
            let (row, prepared) = store.encode(vector, metric, slot)?;
            store.bytes.append(&row);
            store.prepared.push(prepared);
        }
        Ok(store)
    }

    pub(crate) fn record_bytes(&self) -> Result<usize> {
        let dim = self.dimensions;
        let codes = match self.config.precision {
            RerankPrecision::Fp32 => dim.checked_mul(4),
            RerankPrecision::Fp16 | RerankPrecision::Bf16 => dim.checked_mul(2),
            RerankPrecision::Sq8 => Some(dim),
            RerankPrecision::Sq4 => Some(dim.div_ceil(2)),
        }
        .ok_or(Error::CapacityExceeded)?;
        let parameters = if self.is_scalar() {
            match self.config.scalar_quantization {
                ScalarQuantization::Affine { group_size } => dim
                    .div_ceil(group_size)
                    .checked_mul(8)
                    .ok_or(Error::CapacityExceeded)?,
                _ => 4,
            }
        } else {
            0
        };
        codes
            .checked_add(parameters)
            .and_then(|size| size.checked_add(if self.is_polar() { 8 } else { 0 }))
            .and_then(|size| size.checked_add(if self.config.norm_correction { 4 } else { 0 }))
            .ok_or(Error::CapacityExceeded)
    }

    fn is_scalar(&self) -> bool {
        matches!(
            self.config.precision,
            RerankPrecision::Sq8 | RerankPrecision::Sq4
        )
    }
    fn is_polar(&self) -> bool {
        self.config.representation == RerankRepresentation::Polar
    }

    pub(crate) fn encode(
        &self,
        vector: &[f32],
        metric: Metric,
        slot: usize,
    ) -> Result<(Vec<u8>, PreparedRow)> {
        let radius_squared = norm_squared_f64(vector);
        let radius = radius_squared.sqrt();
        let mut source = vector.to_vec();
        if self.is_polar() && radius > 0.0 {
            for value in &mut source {
                *value = (f64::from(*value) / radius) as f32;
            }
        }
        if !self.center.is_empty() {
            for (value, &center) in source.iter_mut().zip(&self.center) {
                *value -= center;
            }
        }
        let mut row = Vec::with_capacity(self.stride);
        if self.is_scalar() {
            self.encode_scalar(&source, &mut row)?;
        } else {
            for value in source {
                match self.config.precision {
                    RerankPrecision::Fp32 => row.extend_from_slice(&value.to_le_bytes()),
                    RerankPrecision::Fp16 => {
                        row.extend_from_slice(&f16::from_f32(value).to_bits().to_le_bytes())
                    }
                    RerankPrecision::Bf16 => {
                        row.extend_from_slice(&bf16::from_f32(value).to_bits().to_le_bytes())
                    }
                    _ => unreachable!(),
                }
            }
        }
        if self.is_polar() {
            row.extend_from_slice(&radius_squared.to_le_bytes());
        }
        if self.config.norm_correction {
            row.extend_from_slice(&1.0f32.to_le_bytes());
            let mut decoded = Vec::with_capacity(self.dimensions);
            self.decode_row(&row, &mut decoded);
            let norm = decoded.iter().map(|v| v * v).sum::<f64>().sqrt();
            let correction = if norm > 0.0 {
                (radius / norm) as f32
            } else {
                1.0
            };
            let offset = row.len() - 4;
            row[offset..].copy_from_slice(&correction.to_le_bytes());
        }
        let mut decoded = Vec::with_capacity(self.dimensions);
        let mut prepared = self.validate_record(&row, metric, slot, &mut decoded)?;
        if self.config == RerankConfig::default() && matches!(metric, Metric::Poincare { .. }) {
            prepared.multiplier = metric.boundary_factor(vector, slot)?;
        }
        Ok((row, prepared))
    }

    fn encode_scalar(&self, source: &[f32], row: &mut Vec<u8>) -> Result<()> {
        let four = self.config.precision == RerankPrecision::Sq4;
        let limit = if four { 7.0 } else { 127.0 };
        let mut codes = vec![0i8; self.dimensions];
        let mut parameters = Vec::new();
        let group = match self.config.scalar_quantization {
            ScalarQuantization::Affine { group_size } => group_size,
            _ => self.dimensions,
        };
        for (index, values) in source.chunks(group).enumerate() {
            let (scale, bias) = if matches!(
                self.config.scalar_quantization,
                ScalarQuantization::Affine { .. }
            ) {
                let low = values.iter().copied().fold(f32::INFINITY, f32::min);
                let high = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                ((high - low) / (2.0 * limit), (high + low) * 0.5)
            } else {
                (
                    values
                        .iter()
                        .fold(0.0f32, |largest, v| largest.max(v.abs()))
                        / limit,
                    0.0,
                )
            };
            let scale = if scale == 0.0 { 1.0 } else { scale };
            if !scale.is_finite() || scale <= 0.0 || !bias.is_finite() {
                return Err(Error::InvalidRerankConfig("标量量化尺度或偏置超出有限范围"));
            }
            parameters.extend_from_slice(&scale.to_le_bytes());
            if matches!(
                self.config.scalar_quantization,
                ScalarQuantization::Affine { .. }
            ) {
                parameters.extend_from_slice(&bias.to_le_bytes());
            }
            for (j, &value) in values.iter().enumerate() {
                codes[index * group + j] = ((value - bias) / scale)
                    .round_ties_even()
                    .clamp(-limit, limit) as i8;
            }
        }
        if four {
            for pair in codes.chunks(2) {
                let low = (pair[0] + 7) as u8;
                let high = pair.get(1).map_or(7, |&value| (value + 7) as u8);
                row.push(low | (high << 4));
            }
        } else {
            row.extend(codes.into_iter().map(|value| value as u8));
        }
        row.extend(parameters);
        Ok(())
    }

    fn decode_row(&self, row: &[u8], output: &mut Vec<f64>) -> f64 {
        output.clear();
        let dim = self.dimensions;
        let scalar = self.is_scalar();
        let code_bytes = match self.config.precision {
            RerankPrecision::Fp32 => dim * 4,
            RerankPrecision::Fp16 | RerankPrecision::Bf16 => dim * 2,
            RerankPrecision::Sq8 => dim,
            RerankPrecision::Sq4 => dim.div_ceil(2),
        };
        let affine = matches!(
            self.config.scalar_quantization,
            ScalarQuantization::Affine { .. }
        );
        let group = match self.config.scalar_quantization {
            ScalarQuantization::Affine { group_size } => group_size,
            _ => dim,
        };
        for index in 0..dim {
            let mut value = if scalar {
                let code = if self.config.precision == RerankPrecision::Sq4 {
                    ((row[index / 2] >> ((index % 2) * 4)) & 15) as i8 - 7
                } else {
                    row[index] as i8
                };
                let offset = code_bytes + index / group * if affine { 8 } else { 4 };
                let scale = number(row, offset);
                let bias = if affine { number(row, offset + 4) } else { 0.0 };
                f32::from(code) * scale + bias
            } else {
                match self.config.precision {
                    RerankPrecision::Fp32 => number(row, index * 4),
                    RerankPrecision::Fp16 => f16::from_bits(u16::from_le_bytes(
                        row[index * 2..index * 2 + 2].try_into().unwrap(),
                    ))
                    .to_f32(),
                    RerankPrecision::Bf16 => bf16::from_bits(u16::from_le_bytes(
                        row[index * 2..index * 2 + 2].try_into().unwrap(),
                    ))
                    .to_f32(),
                    _ => unreachable!(),
                }
            };
            if !self.center.is_empty() {
                value += self.center[index];
            }
            output.push(f64::from(value));
        }
        let mut correction = 1.0;
        if self.is_polar() {
            let radius_squared = f64::from_le_bytes(row[row.len() - 8..].try_into().unwrap());
            let norm = output.iter().map(|value| value * value).sum::<f64>().sqrt();
            let factor = if norm > 0.0 {
                radius_squared.sqrt() / norm
            } else {
                0.0
            };
            for value in output.iter_mut() {
                *value *= factor;
            }
            correction = factor;
        }
        if self.config.norm_correction {
            let factor = f64::from(number(row, row.len() - 4));
            for value in output.iter_mut() {
                *value *= factor;
            }
            correction *= factor;
        }
        correction
    }

    fn validate_decoded(
        &self,
        values: &[f64],
        metric: Metric,
        slot: usize,
        radial_squared: Option<f64>,
    ) -> Result<()> {
        if values.iter().any(|value| !value.is_finite()) {
            return Err(Error::NonFiniteVector { index: slot });
        }
        let norm = values.iter().map(|value| value * value).sum::<f64>();
        let norm = radial_squared.unwrap_or(norm);
        if let Metric::Poincare { curvature } = metric
            && (!norm.is_finite() || 1.0 - curvature * norm <= 0.0)
        {
            return Err(Error::OutsideManifold { index: slot });
        }
        Ok(())
    }

    fn validate_record(
        &self,
        row: &[u8],
        metric: Metric,
        slot: usize,
        output: &mut Vec<f64>,
    ) -> Result<PreparedRow> {
        if row.len() != self.stride {
            return Err(Error::CorruptIndex("冷向量记录长度错误"));
        }
        if self.is_scalar() {
            let four = self.config.precision == RerankPrecision::Sq4;
            let codes = if four {
                self.dimensions.div_ceil(2)
            } else {
                self.dimensions
            };
            if four {
                if row[..codes]
                    .iter()
                    .any(|&value| value & 15 == 15 || value >> 4 == 15)
                    || (self.dimensions % 2 == 1 && row[codes - 1] >> 4 != 7)
                {
                    return Err(Error::CorruptIndex("四位量化码字或尾部填充无效"));
                }
            } else if row[..codes].contains(&128) {
                return Err(Error::CorruptIndex("八位量化码字超出对称范围"));
            }
            let (groups, parameter_size) = match self.config.scalar_quantization {
                ScalarQuantization::Affine { group_size } => {
                    (self.dimensions.div_ceil(group_size), 8)
                }
                _ => (1, 4),
            };
            for group in 0..groups {
                let offset = codes + group * parameter_size;
                let scale = number(row, offset);
                if !scale.is_finite()
                    || scale <= 0.0
                    || (parameter_size == 8 && !number(row, offset + 4).is_finite())
                {
                    return Err(Error::CorruptIndex("冷向量尺度或偏置无效"));
                }
            }
        }
        if self.config.norm_correction {
            let correction = number(row, row.len() - 4);
            if !correction.is_finite() || correction < 0.0 {
                return Err(Error::CorruptIndex("范数恢复系数无效"));
            }
        }
        let radial_squared = if self.is_polar() {
            let value = f64::from_le_bytes(row[row.len() - 8..].try_into().unwrap());
            if !value.is_finite() || value < 0.0 {
                return Err(Error::CorruptIndex("冷向量半径平方无效"));
            }
            Some(value)
        } else {
            None
        };
        let scale = self.decode_row(row, output);
        self.validate_decoded(output, metric, slot, radial_squared)?;
        let norm = output.iter().map(|value| value * value).sum::<f64>();
        let multiplier = match metric {
            Metric::Poincare { curvature } => {
                (1.0 - curvature * radial_squared.unwrap_or(norm)).recip()
            }
            Metric::Cosine => {
                if norm > 0.0 {
                    norm.sqrt().recip()
                } else {
                    0.0
                }
            }
        };
        Ok(PreparedRow { scale, multiplier })
    }

    #[inline]
    pub(crate) fn score_row(
        &self,
        query: &[f32],
        query_factor: f64,
        slot: usize,
        row: &[u8],
        metric: Metric,
        query_norm: &mut Option<f64>,
    ) -> f64 {
        let prepared = self.prepared[slot];
        let dot = metric == Metric::Cosine;
        let sum =
            super::scoring::accumulate(query, row, self.config, &self.center, prepared.scale, dot);
        match metric {
            Metric::Poincare { .. } => sum * query_factor * prepared.multiplier,
            Metric::Cosine => {
                let norm = *query_norm.get_or_insert_with(|| norm_squared_f64(query).sqrt());
                if prepared.multiplier == 0.0 || norm == 0.0 {
                    0.0
                } else {
                    (sum * prepared.multiplier / norm).clamp(-1.0, 1.0)
                }
            }
        }
    }

    #[inline]
    pub(crate) fn score(
        &self,
        query: &[f32],
        query_factor: f64,
        slot: usize,
        metric: Metric,
        scratch: &mut RerankScratch,
    ) -> Result<f64> {
        let RerankScratch { query_norm } = scratch;
        let row = self.bytes.row(slot, self.stride);
        Ok(self.score_row(query, query_factor, slot, row, metric, query_norm))
    }

    pub(crate) fn prefetch(&self, slot: usize) {
        #[cfg(target_arch = "x86_64")]
        {
            use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
            if let Some(prepared) = self.prepared.get(slot) {
                // SAFETY：_mm_prefetch 仅为非阻塞硬件缓存提示，prepared 引用指向有效元素。
                unsafe {
                    _mm_prefetch(std::ptr::from_ref(prepared).cast(), _MM_HINT_T0);
                }
            }
            if self.stride == 0
                || slot
                    .checked_mul(self.stride)
                    .is_none_or(|off| off + self.stride > self.bytes.len())
            {
                return;
            }
            let row = self.bytes.row(slot, self.stride);
            let ptr = row.as_ptr().cast::<i8>();
            let len = row.len();
            // SAFETY：所有偏移均严格位于已验证记录切片内，覆盖首部码字流与尾部量化尺度元数据。
            unsafe {
                _mm_prefetch(ptr, _MM_HINT_T0);
                if len > 64 {
                    _mm_prefetch(ptr.add(64), _MM_HINT_T0);
                }
                if len > 128 {
                    _mm_prefetch(ptr.add(len - 1), _MM_HINT_T0);
                }
                if len > 576 {
                    _mm_prefetch(ptr.add(512), _MM_HINT_T0);
                }
                if len > 1536 {
                    _mm_prefetch(ptr.add(1536), _MM_HINT_T0);
                }
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = slot;
    }

    pub(crate) fn validate_records(&mut self, metric: Metric) -> Result<()> {
        let mut output = Vec::with_capacity(self.dimensions);
        let mut prepared = Vec::new();
        prepared
            .try_reserve_exact(self.bytes.len() / self.stride)
            .map_err(|_| Error::CapacityExceeded)?;
        for slot in 0..self.bytes.len() / self.stride {
            let row = self.bytes.row(slot, self.stride);
            prepared.push(self.validate_record(row, metric, slot, &mut output)?);
        }
        self.prepared = prepared;
        Ok(())
    }

    pub(crate) fn canonical(
        dimensions: usize,
        vectors: &Buffer<f32>,
        metric: Metric,
        boundary_factors: &[f64],
    ) -> Result<Self> {
        let (base, delta) = vectors.slices();
        let mut bytes = if let Some((file, offset, length)) = vectors.file_source() {
            Buffer::map_file(&file, offset, length)?
        } else if base.is_empty() {
            Buffer::owned(Vec::new())
        } else {
            Buffer::owned(bytemuck::cast_slice(base).to_vec())
        };
        bytes.append(bytemuck::cast_slice(delta));
        let mut store = Self {
            config: RerankConfig::default(),
            dimensions,
            center: Vec::new(),
            bytes,
            stride: dimensions * 4,
            prepared: Vec::new(),
        };
        if matches!(metric, Metric::Poincare { .. }) {
            store
                .prepared
                .try_reserve_exact(boundary_factors.len())
                .map_err(|_| Error::CapacityExceeded)?;
            store
                .prepared
                .extend(boundary_factors.iter().map(|&multiplier| PreparedRow {
                    scale: 1.0,
                    multiplier,
                }));
        } else {
            store.validate_records(metric)?;
            store.bytes = store.bytes.file_backed()?;
        }
        Ok(store)
    }
}

fn number(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 准备阶段的范数和缩放与独立逐维解码评分一致() {
        for dim in [3, 17, 64, 129, 512] {
            let data = (0..3 * dim)
                .map(|i| ((i * 73 % 127) as f32 - 63.0) * 0.003 / (dim as f32).sqrt())
                .collect::<Vec<_>>();
            let query = &data[dim..2 * dim];
            for metric in [
                Metric::Cosine,
                Metric::Poincare { curvature: 0.5 },
                Metric::Poincare { curvature: 2.0 },
            ] {
                for precision in [
                    RerankPrecision::Fp32,
                    RerankPrecision::Fp16,
                    RerankPrecision::Bf16,
                    RerankPrecision::Sq8,
                    RerankPrecision::Sq4,
                ] {
                    for representation in
                        [RerankRepresentation::Cartesian, RerankRepresentation::Polar]
                    {
                        for quant in [
                            ScalarQuantization::Symmetric,
                            ScalarQuantization::Centered,
                            ScalarQuantization::Affine { group_size: 3 },
                            ScalarQuantization::Affine { group_size: 64 },
                        ] {
                            for correction in [false, true] {
                                let config = RerankConfig {
                                    precision,
                                    representation,
                                    scalar_quantization: quant,
                                    norm_correction: correction,
                                };
                                if config.validate().is_err() {
                                    continue;
                                }
                                let store = ColdStore::build(
                                    config,
                                    dim,
                                    &Buffer::owned(data.clone()),
                                    &[true; 3],
                                    metric,
                                )
                                .unwrap();
                                let mut decoded = Vec::new();
                                store.decode_row(store.bytes.row(0, store.stride), &mut decoded);
                                let norm = decoded.iter().map(|v| v * v).sum::<f64>();
                                let expected = match metric {
                                    Metric::Cosine => {
                                        let dot = query
                                            .iter()
                                            .zip(&decoded)
                                            .map(|(&q, &v)| f64::from(q) * v)
                                            .sum::<f64>();
                                        if norm == 0.0 {
                                            0.0
                                        } else {
                                            (dot / norm.sqrt() / norm_squared_f64(query).sqrt())
                                                .clamp(-1.0, 1.0)
                                        }
                                    }
                                    Metric::Poincare { curvature } => {
                                        let norm = if representation == RerankRepresentation::Polar
                                        {
                                            norm_squared_f64(&data[..dim])
                                        } else {
                                            norm
                                        };
                                        query
                                            .iter()
                                            .zip(&decoded)
                                            .map(|(&q, &v)| (f64::from(q) - v).powi(2))
                                            .sum::<f64>()
                                            * metric.boundary_factor(query, 0).unwrap()
                                            * (1.0 - curvature * norm).recip()
                                    }
                                };
                                let actual = store
                                    .score(
                                        query,
                                        metric.boundary_factor(query, 0).unwrap(),
                                        0,
                                        metric,
                                        &mut RerankScratch::default(),
                                    )
                                    .unwrap();
                                assert!(
                                    (actual - expected).abs() <= 1e-12 * expected.abs().max(1.0),
                                    "预计算评分不符，维度{dim}，配置{config:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
