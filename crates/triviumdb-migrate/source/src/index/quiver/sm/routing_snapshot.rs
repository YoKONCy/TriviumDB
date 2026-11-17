//! 默认SM路由状态的紧凑快照，随QuIVer索引共同绑定原始向量事实。
use super::*;

const HEADER_BYTES: usize = 24;
const ROW_METADATA_BYTES: usize = 20;
const MAX_ROW_BYTES: usize = crate::index::bq::MAX_BQ_DIM.div_ceil(64) * 16 + ROW_METADATA_BYTES;

fn geometry(dim: usize, nodes: usize) -> io::Result<(usize, usize)> {
    dynamic::check_dimension(dim)?;
    let row_bytes = dim.div_ceil(64) * 16 + ROW_METADATA_BYTES;
    let total = nodes
        .checked_mul(row_bytes)
        .and_then(|length| length.checked_add(HEADER_BYTES + 4))
        .ok_or_else(|| corrupt("SM快照长度溢出 (SM snapshot length overflow)"))?;
    Ok((row_bytes, total))
}

impl SmIndex {
    pub(crate) fn routing_snapshot_len(&self) -> io::Result<usize> {
        geometry(self.dimension(), self.len()).map(|(_, length)| length)
    }

    pub(crate) fn write_routing_snapshot(
        &self,
        writer: &mut impl Write,
        ids: &[u64],
        tombstones: &[bool],
    ) -> io::Result<()> {
        let dim = self.dimension();
        let nodes = self.len();
        if self.scoring != SmScoring::ReconstructedL2
            || tombstones.len() != nodes
            || self.exact_scales.len() != nodes
            || self.ids != ids
        {
            return Err(invalid(
                "默认SM快照状态不完整 (Incomplete default SM snapshot state)",
            ));
        }
        let (row_bytes, _) = geometry(dim, nodes)?;
        let words = dim.div_ceil(64);
        let planes_end = words * 16;
        let mut header = [0u8; HEADER_BYTES];
        header[..4].copy_from_slice(b"QDSM");
        header[4..8].copy_from_slice(&1u32.to_le_bytes());
        header[8..12].copy_from_slice(&(dim as u32).to_le_bytes());
        header[12..20].copy_from_slice(&(nodes as u64).to_le_bytes());
        header[20..24].copy_from_slice(&(row_bytes as u32).to_le_bytes());
        writer.write_all(&header)?;
        let mut checksum = crc32fast::Hasher::new();
        checksum.update(&header);
        let mut buffer = [0u8; MAX_ROW_BYTES];
        for (index, &deleted) in tombstones.iter().enumerate() {
            let row = &mut buffer[..row_bytes];
            row.fill(0);
            // 历史恢复把墓碑表示为零向量；沿用该导航状态，避免读取已复用的slot。
            row[planes_end + 16..].copy_from_slice(&1f32.to_le_bytes());
            if !deleted {
                let (pos, strong, weak, delta, norm, count) = if let Some(model) = &self.dynamic {
                    let encoded = model.row(index);
                    let (weak, delta, _, norm, count) = model.metadata(index);
                    (
                        &encoded[..words],
                        &encoded[words..words * 2],
                        weak,
                        delta,
                        norm,
                        count,
                    )
                } else {
                    let encoded = &self.rows[index];
                    (
                        &encoded.pos[..],
                        &encoded.strong[..],
                        encoded.weak,
                        encoded.delta,
                        encoded.reconstructed_norm,
                        u32::from(encoded.strong_count),
                    )
                };
                for (word, encoded) in pos
                    .iter()
                    .chain(strong)
                    .zip(row[..planes_end].as_chunks_mut::<8>().0)
                {
                    encoded.copy_from_slice(&word.to_le_bytes());
                }
                let metadata = [
                    weak.to_bits(),
                    delta.to_bits(),
                    norm.to_bits(),
                    count,
                    self.exact_scales[index].to_bits(),
                ];
                for (word, encoded) in metadata
                    .into_iter()
                    .zip(row[planes_end..].as_chunks_mut::<4>().0)
                {
                    encoded.copy_from_slice(&word.to_le_bytes());
                }
            }
            writer.write_all(row)?;
            checksum.update(row);
        }
        writer.write_all(&checksum.finalize().to_le_bytes())
    }

    pub(crate) fn from_routing_snapshot(
        bytes: &[u8],
        dim: usize,
        ids: &[u64],
        entry: u32,
    ) -> io::Result<Self> {
        let nodes = ids.len();
        let (row_bytes, expected_length) = geometry(dim, nodes)?;
        if bytes.len() != expected_length {
            return Err(corrupt("SM快照长度不匹配 (SM snapshot length mismatch)"));
        }
        let header = bytes[..HEADER_BYTES].as_chunks::<4>().0;
        let stored_nodes = u64::from(u32::from_le_bytes(header[3]))
            | (u64::from(u32::from_le_bytes(header[4])) << 32);
        let payload_end = bytes.len() - 4;
        let footer = bytes[payload_end..].as_chunks::<4>().0[0];
        if &bytes[..4] != b"QDSM"
            || u32::from_le_bytes(header[1]) != 1
            || u32::from_le_bytes(header[2]) as usize != dim
            || stored_nodes != nodes as u64
            || u32::from_le_bytes(header[5]) as usize != row_bytes
            || crc32fast::hash(&bytes[..payload_end]) != u32::from_le_bytes(footer)
            || entry as usize >= nodes
        {
            return Err(corrupt(
                "SM快照身份或CRC无效 (Invalid SM snapshot identity or CRC)",
            ));
        }

        // 完整几何与CRC校验先于分配；只物化紧凑路由行，原始向量继续按需读取。
        let mut model = Self::empty_routing(dim)?;
        let allocation = || {
            io::Error::new(
                io::ErrorKind::OutOfMemory,
                "SM快照分配失败 (SM snapshot allocation failed)",
            )
        };
        model
            .ids
            .try_reserve_exact(nodes)
            .map_err(|_| allocation())?;
        model
            .exact_scales
            .try_reserve_exact(nodes)
            .map_err(|_| allocation())?;
        if dim == DIM {
            model
                .rows
                .try_reserve_exact(nodes)
                .map_err(|_| allocation())?;
        } else {
            model.dynamic = Some(DynamicSm::empty(dim, nodes)?);
        }
        let words = dim.div_ceil(64);
        let planes_end = words * 16;
        let tail = dim % 64;
        let invalid_tail = if tail == 0 { 0 } else { !((1u64 << tail) - 1) };
        for (index, row) in bytes[HEADER_BYTES..payload_end]
            .chunks_exact(row_bytes)
            .enumerate()
        {
            let encoded = row[..planes_end].as_chunks::<8>().0;
            let metadata = row[planes_end..].as_chunks::<4>().0;
            let weak = f32::from_le_bytes(metadata[0]);
            let delta = f32::from_le_bytes(metadata[1]);
            let norm = f32::from_le_bytes(metadata[2]);
            let count = u32::from_le_bytes(metadata[3]);
            let scale = f32::from_le_bytes(metadata[4]);
            let actual_count = encoded[words..]
                .iter()
                .map(|word| u64::from_le_bytes(*word).count_ones())
                .sum::<u32>();
            let high = weak + delta;
            let expected_norm = dim.saturating_sub(count as usize) as f32 * weak * weak
                + count as f32 * high * high;
            if !weak.is_finite()
                || weak < 0.
                || !delta.is_finite()
                || !(weak + delta).is_finite()
                || weak + delta < 0.
                || !norm.is_finite()
                || norm < 0.
                || norm.to_bits() != expected_norm.to_bits()
                || scale.is_nan()
                || scale <= 0.
                || count != actual_count
                || count as usize > dim
                || ((u64::from_le_bytes(encoded[words - 1])
                    | u64::from_le_bytes(encoded[words * 2 - 1]))
                    & invalid_tail)
                    != 0
            {
                return Err(corrupt("SM快照行无效 (Invalid SM snapshot row)"));
            }
            if let Some(dynamic) = &mut model.dynamic {
                for (target, word) in dynamic.row_mut(index)[..words * 2].iter_mut().zip(encoded) {
                    *target = u64::from_le_bytes(*word);
                }
                dynamic.write_metadata(index, weak, delta, 0., norm, count);
            } else {
                model.rows.push(SmRow {
                    pos: std::array::from_fn(|word| u64::from_le_bytes(encoded[word])),
                    strong: std::array::from_fn(|word| u64::from_le_bytes(encoded[words + word])),
                    weak,
                    delta,
                    coefficient: 0.,
                    reconstructed_norm: norm,
                    strong_count: count as u16,
                });
            }
            model.exact_scales.push(scale);
        }
        model.ids.extend_from_slice(ids);
        model.set_routing_entry(entry);
        Ok(model)
    }
}
