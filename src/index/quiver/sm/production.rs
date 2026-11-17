//! 默认余弦查询的流式SM状态；只保存紧凑位码与幅值，不复制全库冷向量。
use super::*;

impl SmIndex {
    pub(crate) fn empty_routing(dim: usize) -> io::Result<Self> {
        dynamic::check_dimension(dim)?;
        Ok(Self {
            rows: Vec::new(),
            graph: FlatAdj::new(1),
            ids: Vec::new(),
            entry: 0,
            anchor: 0,
            base_crc: 0,
            exact_scales: Vec::new(),
            scoring: SmScoring::ReconstructedL2,
            dynamic: if dim == DIM {
                None
            } else {
                Some(DynamicSm::empty(dim, 0)?)
            },
        })
    }

    pub(crate) fn reserve_routing(&mut self, n: usize) {
        self.ids.reserve(n);
        self.exact_scales.reserve(n);
        if let Some(model) = &mut self.dynamic {
            model.reserve_rows(n);
        } else {
            self.rows.reserve(n);
        }
    }

    /// 每次只转换一条向量；非单位输入按旧余弦语义归一化，零向量保持零。
    pub(crate) fn push_routing_vector(&mut self, vector: &[f32], id: u64) -> io::Result<()> {
        let dim = self.dimension();
        if vector.len() != dim || vector.iter().any(|x| !x.is_finite()) {
            return Err(invalid("默认SM向量维度或数值无效"));
        }
        let norm = vector.iter().map(|&x| (x as f64).powi(2)).sum::<f64>();
        let scale = if norm == 0. || (norm - 1.).abs() <= 1e-5 {
            1.
        } else {
            (1. / norm.sqrt()) as f32
        };
        let normalized;
        let x = if scale == 1. {
            vector
        } else {
            let inverse = 1. / norm.sqrt();
            normalized = vector
                .iter()
                .map(|&v| {
                    if extreme_scale(scale) {
                        (v as f64 * inverse) as f32
                    } else {
                        v * scale
                    }
                })
                .collect::<Vec<_>>();
            &normalized
        };
        if let Some(model) = &mut self.dynamic {
            model.push_vector(x)?;
        } else {
            let code = Bq2Signature::from_vector(x);
            let mut sums = [0f64; 2];
            let mut counts = [0usize; 2];
            for (j, &v) in x.iter().enumerate() {
                let group = ((code.strong[j / 64] >> (j % 64)) & 1) as usize;
                sums[group] += v.abs() as f64;
                counts[group] += 1;
            }
            let weak = if counts[0] == 0 {
                0.
            } else {
                (sums[0] / counts[0] as f64) as f32
            };
            let high = if counts[1] == 0 {
                0.
            } else {
                (sums[1] / counts[1] as f64) as f32
            };
            let delta = high - weak;
            let reconstructed_high = weak + delta;
            self.rows.push(SmRow {
                pos: code.pos[..12].try_into().unwrap(),
                strong: code.strong[..12].try_into().unwrap(),
                weak,
                delta,
                coefficient: 0.,
                reconstructed_norm: counts[0] as f32 * weak * weak
                    + counts[1] as f32 * reconstructed_high * reconstructed_high,
                strong_count: counts[1] as u16,
            });
        }
        self.ids.push(id);
        self.exact_scales.push(scale);
        Ok(())
    }

    pub(crate) fn set_routing_entry(&mut self, entry: u32) {
        self.entry = entry;
    }
}

/// 极端幅值先在已读取的单条冷向量上归一化，避免FP32点积上溢或下溢。
pub(super) fn extreme_scale(scale: f32) -> bool {
    !scale.is_finite() || !(1e-10..=1e10).contains(&scale)
}

pub(super) fn prepare_exact_buffer(buffer: &mut [f32], scale: f32) {
    if extreme_scale(scale) {
        let norm = buffer
            .iter()
            .map(|&x| (x as f64).powi(2))
            .sum::<f64>()
            .sqrt();
        if norm > 0. {
            for value in buffer {
                *value = (*value as f64 / norm) as f32;
            }
        }
    }
}

pub(super) fn exact_scale(scale: f32) -> f32 {
    if extreme_scale(scale) { 1. } else { scale }
}

impl QuIVer {
    /// 旧BQ文件不含SM幅值；数据库发布索引前从实际slot流式恢复默认查询状态。
    pub(crate) fn prepare_default_sm<F>(&mut self, mut fill: F) -> io::Result<()>
    where
        F: FnMut(usize, &mut Vec<f32>) -> bool,
    {
        let mut model = SmIndex::empty_routing(self.dim)?;
        model.reserve_routing(self.n);
        let mut buffer = Vec::with_capacity(self.dim);
        for (internal, &slot) in self.slot_indices.iter().enumerate() {
            buffer.clear();
            if self.tombstones[internal] {
                // 墓碑只参与图导航，不能从已复用slot读取新节点作为旧节点的向量。
                buffer.resize(self.dim, 0.);
            } else if !fill(slot, &mut buffer) {
                return Err(invalid("默认SM恢复时无法读取活跃节点向量"));
            }
            model.push_routing_vector(&buffer, self.ids[internal])?;
        }
        model.set_routing_entry(self.entry_point);
        self.default_sm = Some(model);
        Ok(())
    }

    pub(crate) fn set_default_sm(&mut self, mut model: SmIndex) {
        assert_eq!(model.dimension(), self.dim);
        assert_eq!(model.len(), self.n);
        model.set_routing_entry(self.entry_point);
        self.default_sm = Some(model);
    }

    pub(crate) fn rebuild_default_sm(&mut self, vectors: &[f32]) {
        // 批量追加入口沿用旧节点的紧凑行，新全量构建入口则从空模型开始。
        let mut model = self
            .default_sm
            .take()
            .unwrap_or_else(|| SmIndex::empty_routing(self.dim).expect("合法SM维度"));
        let start = self.n - vectors.len() / self.dim;
        if model.len() != start {
            model = SmIndex::empty_routing(self.dim).expect("合法SM维度");
            assert_eq!(start, 0, "追加构图缺少旧节点SM状态");
        }
        model.reserve_routing(self.n - start);
        for (i, vector) in vectors.chunks_exact(self.dim).enumerate() {
            model
                .push_routing_vector(vector, self.ids[start + i])
                .expect("构图向量必须合法");
        }
        self.set_default_sm(model);
    }

    /// 当前查询默认使用无训练SM-L2；校正和原生欧氏仅由显式安装覆盖。
    pub fn default_sm_l2_enabled(&self) -> bool {
        self.corrected_sm.is_none() && self.default_sm_ready()
    }

    pub(crate) fn default_sm_ready(&self) -> bool {
        self.default_sm.as_ref().is_some_and(|m| m.len() == self.n)
    }
}
