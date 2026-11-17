//! 当前 QuIVer 快照解码。
use super::*;
impl QuIVer {
    pub fn load_from_bytes(bytes: &[u8]) -> std::io::Result<Self> {
        fn invalid(message: impl Into<String>) -> std::io::Error {
            std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
        }

        fn checked_end(
            offset: usize,
            count: usize,
            width: usize,
            name: &str,
        ) -> std::io::Result<usize> {
            count
                .checked_mul(width)
                .and_then(|bytes| offset.checked_add(bytes))
                .ok_or_else(|| invalid(format!("{name} 长度溢出")))
        }

        if bytes.len() < 52 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "QuIVer 文件太小 (QuIVer file too small)",
            ));
        }

        // Magic
        if &bytes[0..4] != Self::QUIVER_MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "无效的 QuIVer magic (Invalid QuIVer magic): {:?}",
                    &bytes[0..4]
                ),
            ));
        }

        // Version
        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if version != Self::QUIVER_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "不支持的 QuIVer 版本 (Unsupported QuIVer version): {}",
                    version
                ),
            ));
        }

        // Header
        let mut off = 8;
        let dim = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        if dim == 0 || dim > crate::index::bq::MAX_BQ_DIM {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "QuIVer 维度 {} 超出安全范围 1..={} ",
                    dim,
                    crate::index::bq::MAX_BQ_DIM
                ),
            ));
        }
        let n = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        let m = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        let m0 = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        let ef_construction = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        let alpha = f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        off += 4;
        let entry_point = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        off += 4;
        let max_level = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        let dirty_count = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        off += 4; // 保留字段

        if n == 0 || m == 0 || m0 == 0 || !alpha.is_finite() || alpha <= 0.0 {
            return Err(invalid("QuIVer 头部参数非法"));
        }
        if entry_point as usize >= n || max_level > n {
            return Err(invalid("QuIVer 入口或层级超出节点范围"));
        }

        // BQ Signatures（紧凑格式）
        let chunks = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        let expected_chunks = dim.div_ceil(64);
        if chunks != expected_chunks {
            return Err(invalid(format!(
                "QuIVer BQ chunks 不匹配: 期望 {expected_chunks}，实际 {chunks}"
            )));
        }
        let pos_end = checked_end(
            off,
            n,
            chunks
                .checked_mul(8)
                .ok_or_else(|| invalid("BQ 单签名长度溢出"))?,
            "BQ pos",
        )?;
        let strong_end = checked_end(
            pos_end,
            n,
            chunks
                .checked_mul(8)
                .ok_or_else(|| invalid("BQ 单签名长度溢出"))?,
            "BQ strong",
        )?;
        if strong_end > bytes.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "BQ 签名数据不完整 (BQ signature data incomplete)",
            ));
        }
        let pos_data: Vec<u64> = bytes[off..pos_end]
            .as_chunks::<8>()
            .0
            .iter()
            .map(|chunk| u64::from_le_bytes(*chunk))
            .collect();
        let strong_data: Vec<u64> = bytes[pos_end..strong_end]
            .as_chunks::<8>()
            .0
            .iter()
            .map(|chunk| u64::from_le_bytes(*chunk))
            .collect();
        let bq_store = Bq2Store::from_raw(pos_data, strong_data, chunks)
            .map_err(|error| invalid(error.to_string()))?;
        off = strong_end;

        // Layer0 FlatAdj
        let stride = m0
            .checked_mul(2)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| invalid("Layer0 stride 溢出"))?;
        let l0_end = checked_end(
            off,
            n,
            stride
                .checked_mul(4)
                .ok_or_else(|| invalid("Layer0 单节点长度溢出"))?,
            "Layer0",
        )?;
        if l0_end > bytes.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Layer0 数据不完整 (Layer0 data incomplete)",
            ));
        }
        let l0_data: Vec<u32> = bytes[off..l0_end]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| u32::from_le_bytes(*chunk))
            .collect();
        off = l0_end;

        // Tombstones
        let tomb_end = checked_end(off, n, 1, "Tombstone")?;
        if tomb_end > bytes.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Tombstone 数据不完整 (Tombstone data incomplete)",
            ));
        }
        if bytes[off..tomb_end].iter().any(|&byte| byte > 1) {
            return Err(invalid(
                "QuIVer墓碑编码无效 (Invalid QuIVer tombstone encoding)",
            ));
        }
        let tombstones: Vec<bool> = bytes[off..tomb_end].iter().map(|&b| b != 0).collect();
        off = tomb_end;

        // IDs
        let ids_end = checked_end(off, n, 8, "IDs")?;
        if ids_end > bytes.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "IDs 数据不完整 (IDs data incomplete)",
            ));
        }
        let ids: Vec<u64> = bytes[off..ids_end]
            .as_chunks::<8>()
            .0
            .iter()
            .map(|chunk| u64::from_le_bytes(*chunk))
            .collect();
        off = ids_end;

        // SlotIndices
        let si_end = checked_end(off, n, 8, "SlotIndices")?;
        if si_end > bytes.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "SlotIndices 数据不完整 (SlotIndices data incomplete)",
            ));
        }
        let slot_indices: Vec<usize> = bytes[off..si_end]
            .as_chunks::<8>()
            .0
            .iter()
            .map(|chunk| u64::from_le_bytes(*chunk) as usize)
            .collect();
        off = si_end;

        // NodeMaxLayer
        let nml_end = checked_end(off, n, 1, "NodeMaxLayer")?;
        if nml_end > bytes.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "NodeMaxLayer 数据不完整 (NodeMaxLayer data incomplete)",
            ));
        }
        let node_max_layer: Vec<u8> = bytes[off..nml_end].to_vec();
        off = nml_end;

        // Upper Layers
        if off + 4 > bytes.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Upper Layers header 不完整 (Upper Layers header incomplete)",
            ));
        }
        let num_upper = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        if num_upper != max_level {
            return Err(invalid("QuIVer upper layer 数量与 max_level 不一致"));
        }

        let mut upper_layers = Vec::with_capacity(num_upper);
        for _ in 0..num_upper {
            if off + 4 > bytes.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Upper layer 节点数不完整 (Upper layer node count incomplete)",
                ));
            }
            let layer_nodes = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
            off += 4;
            if layer_nodes != n {
                return Err(invalid("QuIVer upper layer 节点数与索引节点数不一致"));
            }

            let mut layer = Vec::with_capacity(layer_nodes);
            for _ in 0..layer_nodes {
                if off + 2 > bytes.len() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Upper adj 度数不完整 (Upper adj degree incomplete)",
                    ));
                }
                let deg = u16::from_le_bytes(bytes[off..off + 2].try_into().unwrap()) as usize;
                off += 2;

                let neighbors_end = checked_end(off, deg, 4, "Upper adj")?;
                if neighbors_end > bytes.len() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Upper adj 邻居列表不完整 (Upper adj neighbor list incomplete)",
                    ));
                }
                let mut adj = Vec::with_capacity(deg);
                for _ in 0..deg {
                    let neighbor = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
                    if neighbor as usize >= n {
                        return Err(invalid("QuIVer upper layer 邻居越界"));
                    }
                    adj.push(neighbor);
                    off += 4;
                }
                layer.push(adj);
            }
            upper_layers.push(layer);
        }

        // 构建反向映射
        let mut id_to_internal = std::collections::HashMap::with_capacity(n);
        for (i, &id) in ids.iter().enumerate() {
            if id == 0 {
                return Err(invalid("QuIVer 节点 ID 为 0 (QuIVer node ID is zero)"));
            }
            if let Some(&previous) = id_to_internal.get(&id) {
                // 向量更新保留旧墓碑行；同一业务ID仅允许一个活跃表示。
                if !tombstones[previous as usize] && !tombstones[i] {
                    return Err(invalid(
                        "QuIVer 活跃节点 ID 重复 (Duplicate active QuIVer node ID)",
                    ));
                }
                if !tombstones[previous as usize] {
                    continue;
                }
            }
            id_to_internal.insert(id, i as u32);
        }

        for node in 0..n {
            let base = node * stride;
            let degree = l0_data[base] as usize;
            if degree > stride - 1 {
                return Err(invalid("QuIVer Layer0 degree 越界"));
            }
            for &neighbor in &l0_data[base + 1..base + 1 + degree] {
                if neighbor as usize >= n {
                    return Err(invalid("QuIVer Layer0 邻居越界"));
                }
            }
        }

        let mut index = Self {
            dim,
            n,
            routing_identity: std::sync::Arc::new(()),
            m,
            m0,
            ef_construction,
            ml: 1.0 / (m as f64).ln(),
            alpha,
            bq_store,
            corrected_sm: None,
            default_sm: None,
            layer0: FlatAdj {
                data: l0_data,
                stride,
            },
            upper_bytes: upper_layers
                .iter()
                .flat_map(|layer| layer.iter())
                .map(|row| row.len() * 4 + std::mem::size_of::<Vec<u32>>())
                .sum(),
            upper_layers,
            node_max_layer,
            ids,
            slot_indices,
            id_to_internal,
            entry_point,
            max_level,
            tombstones,
            dirty_count,
            prefilter_slot_cache: Mutex::new(PrefilterSlotCacheState::default()),
            visited: Bitset::new(n),
            connectivity_parents: Vec::new(),
        };
        {
            let length_end = off
                .checked_add(8)
                .ok_or_else(|| invalid("SM长度偏移溢出 (SM length offset overflow)"))?;
            let length_bytes = bytes
                .get(off..length_end)
                .ok_or_else(|| invalid("SM长度字段截断 (Truncated SM length field)"))?;
            let length = u64::from_le_bytes(length_bytes.as_chunks::<8>().0[0]);
            let length = usize::try_from(length)
                .map_err(|_| invalid("SM长度超出平台范围 (SM length exceeds platform range)"))?;
            let end = length_end
                .checked_add(length)
                .ok_or_else(|| invalid("SM快照长度溢出 (SM snapshot length overflow)"))?;
            if end != bytes.len() {
                return Err(invalid("SM快照范围不匹配 (SM snapshot range mismatch)"));
            }
            if length > 0 {
                index.default_sm = Some(sm::SmIndex::from_routing_snapshot(
                    &bytes[length_end..end],
                    dim,
                    &index.ids,
                    entry_point,
                )?);
            }
        }
        index.restore_navigation_tree()?;
        tracing::info!(
            "QuIVer 索引从磁盘加载完成：{} 个节点，dim={}，max_level={}，tombstone={}",
            n,
            dim,
            max_level,
            index.tombstones.iter().filter(|&&t| t).count()
        );
        Ok(index)
    }
}
