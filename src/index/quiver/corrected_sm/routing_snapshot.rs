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

impl CorrectedSmIndex {
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

#[cfg(test)]
mod tests {
    use super::*;

    // 独立协议样例对应一维单位向量，字段按QDSM v1的固定布局排列。
    fn unit_snapshot() -> Vec<u8> {
        let mut bytes = b"QDSM".to_vec();
        for word in [1u32, 1, 1, 0, 36] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend(1u64.to_le_bytes());
        bytes.extend(0u64.to_le_bytes());
        for word in [
            1f32.to_bits(),
            (-1f32).to_bits(),
            1f32.to_bits(),
            0,
            1f32.to_bits(),
        ] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend(crc32fast::hash(&bytes).to_le_bytes());
        bytes
    }

    #[test]
    fn 单位向量编码及解码符合独立协议样例() {
        let expected = unit_snapshot();
        assert_eq!(expected.len(), 64);
        let mut model = CorrectedSmIndex::empty_routing(1).unwrap();
        model.push_routing_vector(&[1.], 7).unwrap();
        let mut bytes = Vec::new();
        model
            .write_routing_snapshot(&mut bytes, &[7], &[false])
            .unwrap();
        assert_eq!(bytes, expected);
        let loaded = CorrectedSmIndex::from_routing_snapshot(&expected, 1, &[7], 0).unwrap();
        let dynamic = loaded.dynamic.as_ref().unwrap();
        assert_eq!(dynamic.row(0)[..2], [1, 0]);
        assert_eq!(dynamic.metadata(0), (1., -1., 0., 1., 0));
        assert_eq!(loaded.exact_scales, [1.]);
        assert_eq!(loaded.ids, [7]);
        assert_eq!(loaded.entry, 0);
        assert_eq!(loaded.scoring, SmScoring::ReconstructedL2);
    }

    #[test]
    fn 全部维度路径保留位平面幅值和极端精算比例() {
        for dim in [1, 65, 768, 1536, 3072] {
            let mut source = CorrectedSmIndex::empty_routing(dim).unwrap();
            let magnitudes = [0., 3., f32::MAX, f32::from_bits(1)];
            for (index, magnitude) in magnitudes.into_iter().enumerate() {
                let vector = (0..dim)
                    .map(|axis| if axis % 2 == 0 { magnitude } else { -magnitude })
                    .collect::<Vec<_>>();
                source
                    .push_routing_vector(&vector, index as u64 + 7)
                    .unwrap();
            }
            let mut bytes = Vec::new();
            source
                .write_routing_snapshot(&mut bytes, &[7, 8, 9, 10], &[false; 4])
                .unwrap();
            let restored =
                CorrectedSmIndex::from_routing_snapshot(&bytes, dim, &[7, 8, 9, 10], 2).unwrap();
            assert_eq!(restored.ids, [7, 8, 9, 10]);
            assert_eq!(restored.entry, 2);
            assert_eq!(
                restored
                    .exact_scales
                    .iter()
                    .map(|scale| scale.to_bits())
                    .collect::<Vec<_>>(),
                source
                    .exact_scales
                    .iter()
                    .map(|scale| scale.to_bits())
                    .collect::<Vec<_>>()
            );
            assert!(restored.exact_scales[3].is_infinite());
            let mut encoded_again = Vec::new();
            restored
                .write_routing_snapshot(&mut encoded_again, &[7, 8, 9, 10], &[false; 4])
                .unwrap();
            assert_eq!(encoded_again, bytes, "维度{dim}的完整路由行发生变化");
        }
    }

    #[test]
    fn 墓碑使用历史零向量状态且拒绝身份错配() {
        let mut source = CorrectedSmIndex::empty_routing(65).unwrap();
        source.push_routing_vector(&[1.; 65], 7).unwrap();
        source.push_routing_vector(&[2.; 65], 11).unwrap();
        let mut bytes = Vec::new();
        source
            .write_routing_snapshot(&mut bytes, &[7, 11], &[false, true])
            .unwrap();
        let loaded = CorrectedSmIndex::from_routing_snapshot(&bytes, 65, &[7, 11], 0).unwrap();
        let dynamic = loaded.dynamic.as_ref().unwrap();
        assert!(dynamic.row(1).iter().all(|&word| word == 0));
        assert_eq!(dynamic.metadata(1), (0., 0., 0., 0., 0));
        assert_eq!(loaded.exact_scales[1], 1.);
        assert!(
            source
                .write_routing_snapshot(&mut Vec::new(), &[11, 7], &[false; 2])
                .is_err()
        );
    }

    #[test]
    fn 截断损坏及重算crc后的非法字段均被拒绝() {
        let original = unit_snapshot();
        for length in 0..original.len() {
            assert!(
                CorrectedSmIndex::from_routing_snapshot(&original[..length], 1, &[7], 0).is_err(),
                "截断长度{length}"
            );
        }
        for index in 0..original.len() {
            let mut bytes = original.clone();
            bytes[index] ^= 1;
            assert!(
                CorrectedSmIndex::from_routing_snapshot(&bytes, 1, &[7], 0).is_err(),
                "损坏偏移{index}"
            );
        }
        for (offset, word) in [
            (4, 99u32),
            (8, 0),
            (12, u32::MAX),
            (20, u32::MAX),
            (24, 2),
            (40, f32::NAN.to_bits()),
            (44, f32::INFINITY.to_bits()),
            (48, 0),
            (52, 2),
            (56, 0),
            (56, (-1f32).to_bits()),
        ] {
            let mut bytes = original.clone();
            bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
            let end = bytes.len() - 4;
            let checksum = crc32fast::hash(&bytes[..end]);
            bytes[end..].copy_from_slice(&checksum.to_le_bytes());
            assert!(
                CorrectedSmIndex::from_routing_snapshot(&bytes, 1, &[7], 0).is_err(),
                "非法字段偏移{offset}"
            );
        }
        assert!(CorrectedSmIndex::from_routing_snapshot(&original, 1, &[7], 1).is_err());
        let mut trailing = original;
        trailing.push(0);
        assert!(CorrectedSmIndex::from_routing_snapshot(&trailing, 1, &[7], 0).is_err());
    }
}
