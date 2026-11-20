//! 由 Core 发布和持有的有界 Pelorus 图映像。

use super::*;
use crate::index::pelorus::engine::storage::binary::{Reader, put, put_usize, put_vec};
use crate::index::pelorus::engine::{
    RerankConfig, RerankPrecision, RerankRepresentation, ScalarQuantization, StorageMode,
};
use memmap2::Mmap;
use std::fs::File;
use std::io::Write;
use std::io::{Seek, SeekFrom};
use std::sync::Arc;

const IMAGE_VERSION: u32 = 4;
const IMAGE_MAGIC: &[u8; 8] = b"QUIVERIX";
const IMAGE_HEADER: usize = 8 + 4 + 8 + 4;

impl Index {
    // Core 将映像作为统一索引容器的一个有界组件发布。
    pub(crate) fn write_image(&self, file: &mut File) -> Result<()> {
        let metadata = self.snapshot_metadata();
        let topology = self.graph.snapshot_bytes();
        file.write_all(&[0u8; IMAGE_HEADER])?;
        let (length, checksum) = {
            let mut writer = HashWriter {
                file,
                hasher: crc32fast::Hasher::new(),
                length: 0,
            };
            writer.write_all(&(metadata.len() as u64).to_le_bytes())?;
            writer.write_all(&metadata)?;
            let absolute = IMAGE_HEADER + writer.length;
            let padding = (64 - absolute % 64) % 64;
            writer.write_all(&[0u8; 64][..padding])?;
            for values in [self.vectors.slices().0, self.vectors.slices().1] {
                write_floats(&mut writer, values)?;
            }
            if let Some(store) = self.snapshot_rerank() {
                let (base, delta) = store.bytes.slices();
                writer.write_all(base)?;
                writer.write_all(delta)?;
            }
            writer.write_all(&(topology.len() as u64).to_le_bytes())?;
            writer.write_all(&topology)?;
            (writer.length, writer.hasher.finalize())
        };
        file.seek(SeekFrom::Start(0))?;
        file.write_all(IMAGE_MAGIC)?;
        file.write_all(&IMAGE_VERSION.to_le_bytes())?;
        file.write_all(&(length as u64).to_le_bytes())?;
        file.write_all(&checksum.to_le_bytes())?;
        Ok(())
    }

    pub(crate) fn open_image(
        file: &File,
        offset: usize,
        mapping: Arc<Mmap>,
        mode: StorageMode,
    ) -> Result<Self> {
        decode_image(file, offset, mapping, mode)
    }

    /// 默认FP32直接使用原始段，运行时缓存句柄通过重新打开配置。
    fn snapshot_rerank(&self) -> Option<&ColdStore> {
        self.rerank
            .as_ref()
            .filter(|store| store.config != RerankConfig::default())
    }

    fn snapshot_metadata(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_usize(&mut out, self.dimensions);
        put_usize(&mut out, self.ids.len());
        put_usize(&mut out, self.config.m);
        put_usize(&mut out, self.config.ef_construction);
        put(&mut out, self.config.alpha);
        let (tag, curvature) = match self.metric {
            Metric::Cosine => (0u32, 0.0),
            Metric::Poincare { curvature } => (1, curvature),
        };
        put(&mut out, tag);
        put(&mut out, curvature);
        put(
            &mut out,
            match self.navigation {
                PoincareNavigation::Exact => 0u32,
                PoincareNavigation::GeometryBq2 => 1,
            },
        );
        put(&mut out, self.next_seed);
        put_vec(&mut out, &self.ids);
        out.extend(self.active.iter().map(|&value| u8::from(value)));
        put_vec(&mut out, &self.boundary_factors);
        put(&mut out, u32::from(self.snapshot_rerank().is_some()));
        if let Some(store) = self.snapshot_rerank() {
            put(
                &mut out,
                match store.config.precision {
                    RerankPrecision::Fp32 => 0u32,
                    RerankPrecision::Fp16 => 1,
                    RerankPrecision::Bf16 => 2,
                    RerankPrecision::Sq8 => 3,
                    RerankPrecision::Sq4 => 4,
                },
            );
            put(
                &mut out,
                u32::from(store.config.representation == RerankRepresentation::Polar),
            );
            let (quant, group) = match store.config.scalar_quantization {
                ScalarQuantization::Symmetric => (0u32, 0),
                ScalarQuantization::Centered => (1, 0),
                ScalarQuantization::Affine { group_size } => (2, group_size),
            };
            put(&mut out, quant);
            put_usize(&mut out, group);
            put(&mut out, u32::from(store.config.norm_correction));
            put_vec(&mut out, &store.center);
            put_usize(&mut out, store.stride);
        }
        out
    }
}

fn decode_image(
    file: &File,
    offset: usize,
    mapping: Arc<Mmap>,
    mode: StorageMode,
) -> Result<Index> {
    let mut r = Reader::new(&mapping);
    if r.take(8)? != IMAGE_MAGIC {
        return Err(Error::CorruptIndex("快照图像标识无效"));
    }
    let version = r.get::<u32>()?;
    if version != IMAGE_VERSION {
        return Err(Error::UnsupportedVersion(version));
    }
    let payload_len = r.usize()?;
    let checksum = r.get::<u32>()?;
    let payload = r.take(payload_len)?;
    r.finish()?;
    if crc32fast::hash(payload) != checksum {
        return Err(Error::CorruptIndex("快照图像校验失败"));
    }
    let mut body = Reader::new(payload);
    let meta_len = body.usize()?;
    let mut meta = Reader::new(body.take(meta_len)?);
    let dimensions = meta.usize()?;
    let count = meta.usize()?;
    if count > u32::MAX as usize {
        return Err(Error::CapacityExceeded);
    }
    let config = BuildConfig {
        m: meta.usize()?,
        ef_construction: meta.usize()?,
        alpha: meta.get()?,
    };
    validate_build_config(dimensions, &config)?;
    let tag = meta.get::<u32>()?;
    let curvature = meta.get::<f64>()?;
    let metric = match tag {
        0 if curvature == 0.0 => Metric::Cosine,
        1 => Metric::Poincare { curvature },
        _ => return Err(Error::CorruptIndex("快照几何类型无效")),
    };
    metric.validate()?;
    let navigation = match meta.get::<u32>()? {
        0 => PoincareNavigation::Exact,
        1 => PoincareNavigation::GeometryBq2,
        _ => return Err(Error::CorruptIndex("快照导航模式无效")),
    };
    let next_seed = meta.get()?;
    let ids = meta.vector::<u64>()?;
    if ids.len() != count {
        return Err(Error::CorruptIndex("向量标识数量错误"));
    }
    let active = meta
        .take(count)?
        .iter()
        .map(|&value| match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::CorruptIndex("活跃状态无效")),
        })
        .collect::<Result<Vec<_>>>()?;
    let boundary_factors = meta.vector::<f64>()?;
    let expected = if matches!(metric, Metric::Poincare { .. }) {
        count
    } else {
        0
    };
    if boundary_factors.len() != expected
        || boundary_factors
            .iter()
            .any(|factor| !factor.is_finite() || *factor < 1.0)
    {
        return Err(Error::CorruptIndex("原始边界因子目录无效"));
    }
    let mut rerank = if meta.boolean()? {
        Some(read_codec(&mut meta, dimensions)?)
    } else {
        None
    };
    meta.finish()?;
    let absolute = IMAGE_HEADER + body.position;
    let padding = (64 - absolute % 64) % 64;
    if body.take(padding)?.iter().any(|&byte| byte != 0) {
        return Err(Error::CorruptIndex("向量对齐填充无效"));
    }
    let values = count
        .checked_mul(dimensions)
        .ok_or(Error::CapacityExceeded)?;
    let length = values.checked_mul(4).ok_or(Error::CapacityExceeded)?;
    let start = IMAGE_HEADER + body.position;
    let raw_bytes = body.take(length)?;
    let vectors = match mode {
        StorageMode::Memory if rerank.is_none() => Buffer::owned(
            raw_bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes))
                .collect(),
        ),
        _ => Buffer::map_file(
            file,
            offset.checked_add(start).ok_or(Error::CapacityExceeded)?,
            length,
        )?,
    };
    if let Some(store) = &mut rerank {
        let length = count
            .checked_mul(store.stride)
            .ok_or(Error::CapacityExceeded)?;
        let start = IMAGE_HEADER + body.position;
        let bytes = body.take(length)?;
        store.bytes = match mode {
            StorageMode::Memory => Buffer::owned(bytes.to_vec()),
            StorageMode::Mmap => Buffer::mapped(Arc::clone(&mapping), start..start + length)?,
        };
        store.validate_records(metric)?;
        if mode == StorageMode::Mmap {
            store.bytes = Buffer::map_file(
                file,
                offset.checked_add(start).ok_or(Error::CapacityExceeded)?,
                length,
            )?;
        }
    }
    let graph_length = body.usize()?;
    let graph =
        QuIVer::from_snapshot(body.take(graph_length)?, dimensions, &config, &ids, &active)?;
    body.finish()?;
    let mut id_to_slot = HashMap::new();
    for (slot, (&id, &live)) in ids.iter().zip(&active).enumerate() {
        if live && id_to_slot.insert(id, slot).is_some() {
            return Err(Error::DuplicateId(id));
        }
    }
    // 目录与坐标在返回句柄前核对，避免损坏数据进入普通查询和维护入口。
    let all_vectors = bytemuck::try_cast_slice::<u8, f32>(raw_bytes)
        .map_err(|_| Error::CorruptIndex("原始向量区间对齐无效"))?;
    for slot in 0..count {
        let vector = &all_vectors[slot * dimensions..(slot + 1) * dimensions];
        validate_vector(vector, dimensions)?;
        if let Metric::Poincare { curvature } = metric {
            let scaled =
                curvature * crate::index::pelorus::engine::metric::norm_squared_f64(vector);
            let stored_boundary = boundary_factors[slot].recip();
            let tolerance = 8.0 * f64::EPSILON * dimensions as f64 * scaled.abs().max(1.0);
            if !scaled.is_finite() || ((1.0 - scaled) - stored_boundary).abs() > tolerance {
                return Err(Error::CorruptIndex("原始坐标与边界因子不一致"));
            }
        }
    }
    Ok(Index {
        graph,
        vectors,
        rerank,
        storage_mode: mode,
        ids,
        active,
        id_to_slot,
        dimensions,
        config,
        next_seed,
        metric,
        boundary_factors,
        navigation,
    })
}

fn read_codec(meta: &mut Reader<'_>, dimensions: usize) -> Result<ColdStore> {
    let precision = match meta.get::<u32>()? {
        0 => RerankPrecision::Fp32,
        1 => RerankPrecision::Fp16,
        2 => RerankPrecision::Bf16,
        3 => RerankPrecision::Sq8,
        4 => RerankPrecision::Sq4,
        _ => return Err(Error::CorruptIndex("重排精度无效")),
    };
    let representation = match meta.get::<u32>()? {
        0 => RerankRepresentation::Cartesian,
        1 => RerankRepresentation::Polar,
        _ => return Err(Error::CorruptIndex("重排坐标表示无效")),
    };
    let quant = meta.get::<u32>()?;
    let group_size = meta.usize()?;
    let scalar_quantization = match quant {
        0 if group_size == 0 => ScalarQuantization::Symmetric,
        1 if group_size == 0 => ScalarQuantization::Centered,
        2 => ScalarQuantization::Affine { group_size },
        _ => return Err(Error::CorruptIndex("标量量化配置无效")),
    };
    let config = RerankConfig {
        precision,
        representation,
        scalar_quantization,
        norm_correction: meta.boolean()?,
    };
    config.validate()?;
    let center = meta.vector::<f32>()?;
    let stride = meta.usize()?;
    let expected_center = if scalar_quantization == ScalarQuantization::Centered {
        dimensions
    } else {
        0
    };
    if center.len() != expected_center || center.iter().any(|value| !value.is_finite()) {
        return Err(Error::CorruptIndex("量化中心目录无效"));
    }
    let store = ColdStore {
        config,
        dimensions,
        center,
        bytes: Buffer::owned(Vec::new()),
        stride,
        prepared: Vec::new(),
    };
    if stride != store.record_bytes()? {
        return Err(Error::CorruptIndex("重排记录步长错误"));
    }
    Ok(store)
}

struct HashWriter<'a> {
    file: &'a mut File,
    hasher: crc32fast::Hasher,
    length: usize,
}
impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = self.file.write(bytes)?;
        self.hasher.update(&bytes[..count]);
        self.length += count;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn write_floats(writer: &mut impl Write, values: &[f32]) -> Result<()> {
    #[cfg(target_endian = "little")]
    writer.write_all(bytemuck::cast_slice(values))?;
    #[cfg(target_endian = "big")]
    for &value in values {
        writer.write_all(&value.to_le_bytes())?;
    }
    Ok(())
}
