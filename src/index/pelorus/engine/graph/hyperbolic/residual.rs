//! 纯度门控切锥残差编码。

use super::*;
/// 常驻热路径的球面切锥正交残差主轴字典（256 轴 × 512D 仅占 512KB，常驻 L2 缓存）。
///
/// 对高浓度聚簇流形学习单位主轴 `mu_c ∈ S^{D-1}`，将节点方向正交分解为
/// `u_v = alpha_v * mu_c + sqrt(1 - alpha_v^2) * r_v`（其中 `<r_v, mu_c> = 0`），
/// 使 2-bit SM 量化比特全部聚焦于切平面局部残差 `r_v`，在同簇深半径共形放大区将角度噪声压缩一个量级以上；
/// 对各向同性均匀分布自动经支持度检验退化为空（零内存、零运行期开销）。
pub(crate) struct ResidualConeDict {
    pub(crate) num_axes: usize,
    pub(crate) dim: usize,
    /// 展平存储的单位主轴向量：`axes[c * dim .. (c + 1) * dim]`
    pub(crate) axes: Vec<f32>,
}
/// 构图期单节点 32B 缓存行对齐的切锥残差元数据（融合锥号、预计算正交正弦与 SM 重构参数，消除多数组跨页访存与重复开方）。
#[repr(C, align(32))]
#[derive(Clone, Copy, Default)]
pub(crate) struct BuildResNodeMeta {
    pub(crate) cone_id: u16,
    pub(crate) has_valid_res: u16,
    pub(crate) alpha: f32,
    pub(crate) beta: f32,
    pub(crate) sm: SmCodeParams,
}
const _: () = assert!(std::mem::size_of::<BuildResNodeMeta>() == 32);
/// 构图期临时持有的同锥残差签名与重构参数（构图冻结并回写主存储后立即释放，零常驻内存膨胀）。
pub(crate) struct BuildResidualHelper {
    pub(crate) meta: Vec<BuildResNodeMeta>,
    pub(crate) res_store: Bq2Store,
}
impl BuildResidualHelper {
    /// 预取构图期节点的 32B 残差元数据与 64B 对齐交织残差签名。
    #[inline(always)]
    pub(crate) fn prefetch(&self, node: usize) {
        if node < self.meta.len() {
            #[cfg(target_arch = "x86_64")]
            unsafe {
                use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
                _mm_prefetch(self.meta.as_ptr().add(node).cast::<i8>(), _MM_HINT_T0);
            }
            self.res_store.prefetch_sig(node);
        }
    }

    /// 计算属于同一切锥主轴的两个节点的高精度正交残差重构余弦。
    #[inline(always)]
    pub(crate) fn intra_cone_cosine(&self, a: usize, b: usize, dim: usize) -> Option<f64> {
        let ma = *self.meta.get(a)?;
        if ma.cone_id == NO_CONE_ID {
            return None;
        }
        let mb = *self.meta.get(b)?;
        if ma.cone_id != mb.cone_id {
            return None;
        }
        let aa = f64::from(ma.alpha);
        let ab = f64::from(mb.alpha);
        if ma.has_valid_res == 0 || mb.has_valid_res == 0 {
            return Some((aa * ab).clamp(-1.0, 1.0));
        }
        let prod_beta = f64::from(ma.beta) * f64::from(mb.beta);
        let counts = self.res_store.sm_counts(a, b, dim);
        let cos_res = sm_calibrated_cosine_fast(ma.sm, mb.sm, counts, dim);
        Some((aa * ab + prod_beta * cos_res).clamp(-1.0, 1.0))
    }
}
impl ResidualConeDict {
    /// 节点归入紧致切锥主轴的最小余弦门槛（保证正交方差因子 `1 - alpha^2 <= 0.116`，至少压缩 8.6 倍角度噪声）。
    pub(crate) const ALPHA_MIN: f32 = 0.94;
    /// 候选球面 Leader 初筛分离余弦阈值（略低于 `ALPHA_MIN` 以归并同簇样本，防止单簇分裂）。
    pub(crate) const TAU_LEADER: f32 = 0.91;
    /// 精炼后单锥内样本的平均余弦纯度下限（抑制连续宽模态锥边缘伪切分）。
    pub(crate) const CONE_MEAN_ALPHA_MIN: f64 = 0.955;
    /// 启用切锥残差所需的样本 p90 无量纲测地深度下限（浅球心流形无共形边界放大，无需多锥重编码）。
    pub(crate) const MIN_CONFORMAL_DEPTH_P90: f32 = 1.0;
    /// 候选池容量上限（覆盖最大 512 个真实聚簇主轴并限制各向同性分布上的初筛扫描开销）。
    pub(crate) const CAND_AXIS_CAP: usize = 1024;

    #[inline]
    pub(crate) fn axis(&self, cone_id: usize) -> &[f32] {
        &self.axes[cone_id * self.dim..(cone_id + 1) * self.dim]
    }

    #[inline]
    pub(crate) fn hot_bytes(&self) -> usize {
        self.axes.len() * std::mem::size_of::<f32>()
    }

    /// 确定性学习球面切锥主轴字典并并行编码全量节点的切空间正交残差。
    pub(crate) fn build(
        space: &NativeSpace<'_>,
        slot_indices: &[usize],
        node_meta: &[PolarNodeMeta],
        dim: usize,
    ) -> Option<(Self, BuildResidualHelper)> {
        let n = slot_indices.len();
        if n < 64 || dim < 8 {
            return None;
        }

        // 1. 确定性均匀抽取具有明确球面方向的代表样本，并检验是否存在深半径共形放大效应
        let sample_target = 8192.min(n);
        let mut sample_depths: Vec<f32> = Vec::with_capacity(sample_target);
        let mut sample_indices: Vec<usize> = Vec::with_capacity(sample_target);
        for s in 0..sample_target {
            let idx = (s * (n - 1)) / sample_target.saturating_sub(1).max(1);
            if node_meta[idx].depth <= 1e-4 || node_meta[idx].radius <= 1e-6 {
                continue;
            }
            sample_depths.push(node_meta[idx].depth);
            sample_indices.push(idx);
        }
        if sample_indices.len() < 16 {
            return None;
        }
        let p90_pos = ((sample_depths.len() - 1) * 90) / 100;
        let (_, &mut p90_depth, _) = sample_depths.select_nth_unstable_by(p90_pos, f32::total_cmp);
        if p90_depth < Self::MIN_CONFORMAL_DEPTH_P90 {
            return None;
        }

        let mut sample_units: Vec<f32> = Vec::with_capacity(sample_indices.len() * dim);
        let mut sample_cnt = 0usize;
        for &idx in &sample_indices {
            let vec = space.vector(slot_indices[idx]);
            let norm_sq = norm_squared_f64(vec);
            if norm_sq <= 1e-16 || !norm_sq.is_finite() {
                continue;
            }
            let inv = (1.0 / norm_sq.sqrt()) as f32;
            for &v in vec {
                sample_units.push(v * inv);
            }
            sample_cnt += 1;
        }
        if sample_cnt < 16 {
            return None;
        }

        // 2. 确定性球面 Leader 初筛：收集互余弦 < TAU_LEADER 的紧致候选方向
        let mut cand_axes: Vec<f32> = Vec::new();
        let mut cand_cnt = 0usize;
        for s in 0..sample_cnt {
            let u = &sample_units[s * dim..(s + 1) * dim];
            let mut max_dot = f32::NEG_INFINITY;
            for c in 0..cand_cnt {
                let axis = &cand_axes[c * dim..(c + 1) * dim];
                let d = dot_f32_fast(u, axis);
                if d > max_dot {
                    max_dot = d;
                }
            }
            if max_dot < Self::TAU_LEADER && cand_cnt < Self::CAND_AXIS_CAP {
                cand_axes.extend_from_slice(u);
                cand_cnt += 1;
            }
        }
        if cand_cnt == 0 {
            return None;
        }

        // 3. 两轮球面均值精炼、最小支持度与簇内紧致度检验（自动零开销排除均匀分布与连续模态锥）
        let min_support = (sample_cnt / 1024).clamp(3, 12);
        let mut axes = cand_axes;
        let mut num_axes = cand_cnt;

        for round in 0..2 {
            let assign_sample = |u: &[f32]| -> Option<(usize, f32)> {
                let mut best_c = 0usize;
                let mut best_dot = f32::NEG_INFINITY;
                for c in 0..num_axes {
                    let axis = &axes[c * dim..(c + 1) * dim];
                    let d = dot_f32_fast(u, axis);
                    if d > best_dot {
                        best_dot = d;
                        best_c = c;
                    }
                }
                (best_dot >= Self::ALPHA_MIN).then_some((best_c, best_dot))
            };

            let matched: Vec<Option<(usize, f32)>> = if sample_cnt * num_axes >= 65_536 {
                sample_units
                    .par_chunks_exact(dim)
                    .map(assign_sample)
                    .collect()
            } else {
                sample_units.chunks_exact(dim).map(assign_sample).collect()
            };

            let mut sums = vec![0.0f64; num_axes * dim];
            let mut counts = vec![0usize; num_axes];
            let mut alpha_sums = vec![0.0f64; num_axes];
            for (s, m) in matched.into_iter().enumerate() {
                if let Some((best_c, best_dot)) = m {
                    counts[best_c] += 1;
                    alpha_sums[best_c] += f64::from(best_dot);
                    let u = &sample_units[s * dim..(s + 1) * dim];
                    let dst = &mut sums[best_c * dim..(best_c + 1) * dim];
                    for j in 0..dim {
                        dst[j] += f64::from(u[j]);
                    }
                }
            }

            let mut kept: Vec<(usize, usize)> = (0..num_axes)
                .filter(|&c| {
                    counts[c] >= min_support
                        && (round == 0
                            || (alpha_sums[c] / (counts[c] as f64)) >= Self::CONE_MEAN_ALPHA_MIN)
                })
                .map(|c| (counts[c], c))
                .collect();
            if kept.is_empty() {
                return None;
            }
            if round == 1 && kept.len() > MAX_RESIDUAL_CONES {
                kept.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
                kept.truncate(MAX_RESIDUAL_CONES);
                kept.sort_unstable_by_key(|&(_, c)| c);
            }

            let mut next_axes = Vec::with_capacity(kept.len() * dim);
            for &(_, old_c) in &kept {
                let src = &sums[old_c * dim..(old_c + 1) * dim];
                let mut nsq = 0.0f64;
                for &v in src {
                    nsq += v * v;
                }
                if nsq <= 1e-18 {
                    continue;
                }
                let inv = 1.0 / nsq.sqrt();
                for &v in src {
                    next_axes.push((v * inv) as f32);
                }
            }
            num_axes = next_axes.len() / dim;
            axes = next_axes;
            if num_axes == 0 {
                return None;
            }
        }

        // 4. 零堆分配并行分配主轴，直接向 64B 对齐交织仓库编码二次 Gram-Schmidt 2-bit SM 正交残差
        let chunks = dim.div_ceil(64);
        let mut res_store = Bq2Store::with_zeroed_len(n, dim);
        let mut meta = vec![
            BuildResNodeMeta {
                cone_id: NO_CONE_ID,
                has_valid_res: 0,
                alpha: 0.0,
                beta: 0.0,
                sm: SmCodeParams::default(),
            };
            n
        ];
        {
            let (interleaved_words, stride) = res_store.interleaved_words_mut();
            interleaved_words
                .par_chunks_exact_mut(stride)
                .zip(meta.par_iter_mut())
                .enumerate()
                .for_each_init(
                    || (vec![0.0f32; dim], vec![0.0f64; dim]),
                    |(unit_buf, res_f64), (i, (sig_dst, meta_dst))| {
                        if node_meta[i].radius <= 1e-6 {
                            return;
                        }
                        let vec = space.vector(slot_indices[i]);
                        let norm_sq = norm_squared_f64(vec);
                        if norm_sq <= 1e-18 || !norm_sq.is_finite() {
                            return;
                        }
                        let inv_r = 1.0 / norm_sq.sqrt();

                        for j in 0..dim {
                            unit_buf[j] = (f64::from(vec[j]) * inv_r) as f32;
                        }
                        let mut best_c = 0usize;
                        let mut best_dot = f32::NEG_INFINITY;
                        for c in 0..num_axes {
                            let axis = &axes[c * dim..(c + 1) * dim];
                            let d = dot_f32_fast(unit_buf, axis);
                            if d > best_dot {
                                best_dot = d;
                                best_c = c;
                            }
                        }
                        if best_dot < Self::ALPHA_MIN {
                            return;
                        }

                        // 二次 Gram-Schmidt 双精度正交化：消除 u_v 与 mu_c 高度对齐时的灾难性抵消误差
                        let axis = &axes[best_c * dim..(best_c + 1) * dim];
                        let mut alpha1 = 0.0f64;
                        for j in 0..dim {
                            let uj = f64::from(vec[j]) * inv_r;
                            alpha1 += uj * f64::from(axis[j]);
                        }
                        let mut alpha2 = 0.0f64;
                        for j in 0..dim {
                            let uj = f64::from(vec[j]) * inv_r;
                            let wj = uj - alpha1 * f64::from(axis[j]);
                            res_f64[j] = wj;
                            alpha2 += wj * f64::from(axis[j]);
                        }
                        let mut res_norm_sq = 0.0f64;
                        for j in 0..dim {
                            let wj = res_f64[j] - alpha2 * f64::from(axis[j]);
                            res_f64[j] = wj;
                            res_norm_sq += wj * wj;
                        }
                        let alpha_exact = (alpha1 + alpha2).clamp(-1.0, 1.0);
                        let beta_exact = (1.0 - alpha_exact * alpha_exact).max(0.0).sqrt() as f32;
                        meta_dst.cone_id = best_c as u16;
                        meta_dst.alpha = alpha_exact as f32;
                        meta_dst.beta = beta_exact;

                        if res_norm_sq <= 1e-12 || beta_exact <= 1e-8 {
                            return;
                        }
                        let inv_res = 1.0 / res_norm_sq.sqrt();
                        for j in 0..dim {
                            unit_buf[j] = (res_f64[j] * inv_res) as f32;
                        }
                        let (p_dst, s_dst) = sig_dst.split_at_mut(chunks);
                        Bq2Signature::encode_f32_slices(unit_buf, p_dst, s_dst);
                        let (sm_params, _) =
                            SmCodeParams::from_vector_and_strong_words(unit_buf, s_dst);
                        meta_dst.sm = sm_params;
                        if sm_params.sigma > 0.0 {
                            meta_dst.has_valid_res = 1;
                        }
                    },
                );
        }

        let assigned_any = meta.iter().any(|m| m.cone_id != NO_CONE_ID);
        if !assigned_any {
            return None;
        }

        Some((
            Self {
                num_axes,
                dim,
                axes,
            },
            BuildResidualHelper { meta, res_store },
        ))
    }
}
