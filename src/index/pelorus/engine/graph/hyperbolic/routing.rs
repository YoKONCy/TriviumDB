//! 方向锥与径向壳层路由。

use super::*;
/// 常驻内存的极坐标锥-壳路由图谱（L1/L2 友好的 S^{d-1} × R_{>=0} 直接跳转索引）。
///
/// 派生缓存兼容现有持久化格式，在构图阶段生成，服务跨壳层选边与查询期切锥路由。
pub(crate) struct PolarRouter {
    pub(crate) num_cones: usize,
    pub(crate) cone_store: Bq2Store,
    pub(crate) cone_sm: Vec<SmCodeParams>,
    pub(crate) cone_inv_qnorm: Vec<f32>,
    /// 自适应或固定无量纲测地深度切分边界（7 个内部分位数阈值将径向支撑划分为 8 个壳层）。
    pub(crate) band_edges: [f32; NUM_RADIAL_BANDS - 1],
    /// 数据集正测地深度支撑下界（用于凸函数投影顶点截断）。
    pub(crate) min_depth: f32,
    /// 数据集测地深度支撑上界。
    pub(crate) max_depth: f32,
    /// 径向主支撑跨度（p95 - p05），用于四向罗盘自适应尺度校准。
    pub(crate) shell_span: f32,
    /// 是否启用分布自适应壳层与测地凸支撑投影。
    pub(crate) adaptive_shells: bool,
    /// 展平二维网格：`cells[cone * NUM_RADIAL_BANDS + band]` 存储最多 POLAR_CELL_CAP 个代表节点。
    pub(crate) cells: Vec<[u32; POLAR_CELL_CAP]>,
    /// 靠近双曲球心（R → 0）的核心枢纽节点，用于极小半径查询与双曲中枢入口。
    pub(crate) origin_hubs: Vec<u32>,
    /// 每节点 32B 缓存行对齐的径向与 SM 元数据表。
    pub(crate) node_meta: Vec<PolarNodeMeta>,
    /// 查询期两级粗到细 Tammes 超锥数量（`num_cones > 256` 时为 64，否则为 0）。
    pub(crate) query_coarse_count: usize,
    /// 每个粗地标持久化的有序细锥短表容量（默认 128，总计仅 16KB 常驻 L1/L2）。
    pub(crate) query_coarse_cap: usize,
    /// 扁平化粗到细短表：`query_coarse_cones[g * query_coarse_cap .. (g + 1) * query_coarse_cap]`。
    pub(crate) query_coarse_cones: Vec<u16>,
    /// 可选的 768D 单页对齐 256B 合表热节点存储（消除双数组 DTLB 二次翻译并支撑头行早筛）。
    pub(crate) colocated_store: Option<Arc<HotSmStore>>,
    /// 可选的球面切锥正交残差主轴字典（启用方案 B 且检测到聚簇结构时常驻）。
    pub(crate) residual_dict: Option<ResidualConeDict>,
    /// 流形级各向同性退化标记（仅在样本最大粗锥对齐余弦中位数落入高维随机正交区时为真）。
    pub(crate) is_isotropic_manifold: bool,
}
impl PolarRouter {
    #[inline]
    pub(crate) fn band_of(&self, depth: f32) -> usize {
        radial_band_with_edges(depth, &self.band_edges)
    }

    /// 并行构建极坐标锥-壳路由图谱，同时返回构图期复用的每节点 Top-4 方向锥索引及可选的切锥残差构建辅助器。
    pub(crate) fn build(
        space: &NativeSpace<'_>,
        bq_store: &Bq2Store,
        slot_indices: &[usize],
        dim: usize,
        use_sm: bool,
        adaptive_shells: bool,
        force_residual: bool,
    ) -> (Self, Vec<[u16; 4]>, Option<BuildResidualHelper>) {
        let n = slot_indices.len();
        let build_meta = |i: usize| {
            let slot = slot_indices[i];
            PolarNodeMeta::from_node(
                space.vector(slot),
                space.boundary_factors[slot],
                bq_store,
                i,
                dim,
            )
        };
        let node_meta: Vec<PolarNodeMeta> = if n >= 2048 {
            (0..n).into_par_iter().map(build_meta).collect()
        } else {
            (0..n).map(build_meta).collect()
        };

        // 计算径向深度支撑感知壳层边界与 [d_min, d_max] 凸支撑边界
        let (band_edges, min_depth, max_depth, shell_span) = if adaptive_shells && n >= 64 {
            let mut depths: Vec<f32> = node_meta.iter().map(|m| m.depth).collect();
            depths.sort_unstable_by(f32::total_cmp);
            let d_min = depths
                .iter()
                .copied()
                .find(|&d| d > 1e-5)
                .unwrap_or(depths[0]);
            let d_max = *depths.last().unwrap_or(&d_min);
            let p01 = depths[(n - 1) / 100];
            let edges = if p01 > 0.35 {
                // 空心球壳流形（如真实高维嵌入投影）：按内核加密分位数贴合有效径向支撑集
                let quantiles: [usize; NUM_RADIAL_BANDS - 1] = [5, 20, 50, 120, 250, 450, 720];
                let mut e = [0.0f32; NUM_RADIAL_BANDS - 1];
                for (b, &q_permil) in quantiles.iter().enumerate() {
                    let pos = ((n - 1) * q_permil) / 1000;
                    let mut edge = depths[pos];
                    if b > 0 && edge < e[b - 1] {
                        edge = e[b - 1];
                    }
                    e[b] = edge;
                }
                e
            } else {
                // 球心实心流形：保持匹配双曲体积增长率 sinh^{d-1}(r) 的几何测地壳层
                LEGACY_RADIAL_BAND_EDGES
            };
            let p05 = depths[((n - 1) * 5) / 100];
            let p95 = depths[((n - 1) * 95) / 100];
            let span = (p95 - p05).max(0.25);
            (edges, d_min, d_max, span)
        } else {
            (LEGACY_RADIAL_BAND_EDGES, 0.0, 8.0, 4.0)
        };

        // 选取最靠近双曲球心的最多 16 个核心枢纽节点
        let mut depth_order: Vec<u32> = (0..n as u32).collect();
        let hub_take = n.min(16);
        if depth_order.len() > hub_take {
            depth_order.select_nth_unstable_by(hub_take, |&a, &b| {
                node_meta[a as usize]
                    .depth
                    .total_cmp(&node_meta[b as usize].depth)
                    .then_with(|| a.cmp(&b))
            });
            depth_order.truncate(hub_take);
        }
        depth_order.sort_unstable_by(|&a, &b| {
            node_meta[a as usize]
                .depth
                .total_cmp(&node_meta[b as usize].depth)
                .then_with(|| a.cmp(&b))
        });
        let origin_hubs = depth_order;

        // 小图直接通过全连通邻接覆盖，无需建立方向锥网格
        if n < 64 {
            return (
                Self {
                    num_cones: 0,
                    cone_store: Bq2Store::new(dim),
                    cone_sm: Vec::new(),
                    cone_inv_qnorm: Vec::new(),
                    band_edges,
                    min_depth,
                    max_depth,
                    shell_span,
                    adaptive_shells,
                    cells: Vec::new(),
                    origin_hubs,
                    node_meta,
                    query_coarse_count: 0,
                    query_coarse_cap: 0,
                    query_coarse_cones: Vec::new(),
                    colocated_store: None,
                    residual_dict: None,
                    is_isotropic_manifold: false,
                },
                vec![[0u16; 4]; n],
                None,
            );
        }

        let max_cones = if use_sm { 2048 } else { 1024 };
        let num_cones = (n / 100).clamp(64, max_cones).min(n);
        let sample_target = (num_cones * 5).clamp(512, 6144).min(n);
        let mut sample_nodes = Vec::with_capacity(sample_target);
        for s in 0..sample_target {
            let idx = (s * (n - 1)) / sample_target.saturating_sub(1).max(1);
            if node_meta[idx].depth > 1e-4 {
                sample_nodes.push(idx as u32);
            }
        }
        if sample_nodes.is_empty() {
            sample_nodes.extend(0..n.min(sample_target) as u32);
        }

        // 在球面 S^{d-1} 上执行确定性最远点贪心填充（Tammes 球面码近似），确保方向地标均匀铺满各切锥
        let mut chosen_cones: Vec<u32> = Vec::with_capacity(num_cones);
        let mut max_sim = vec![f32::NEG_INFINITY; sample_nodes.len()];
        chosen_cones.push(sample_nodes[0]);
        while chosen_cones.len() < num_cones && chosen_cones.len() < sample_nodes.len() {
            let last_cone = *chosen_cones.last().unwrap() as usize;
            let last_meta = node_meta[last_cone];
            let mut best_pos = 0usize;
            let mut min_max_sim = f32::INFINITY;
            for (pos, &cand_u32) in sample_nodes.iter().enumerate() {
                let cand = cand_u32 as usize;
                let sim = if use_sm {
                    let counts = bq_store.sm_counts(last_cone, cand, dim);
                    sm_calibrated_cosine_fast(
                        last_meta.sm_params(),
                        node_meta[cand].sm_params(),
                        counts,
                        dim,
                    ) as f32
                } else {
                    let bq_d = bq_store.distance(last_cone, cand, dim);
                    let dot = 4.0 * dim as f32 - bq_d as f32;
                    dot * last_meta.legacy_inv_qnorm(dim) * node_meta[cand].legacy_inv_qnorm(dim)
                };
                if sim > max_sim[pos] {
                    max_sim[pos] = sim;
                }
                if max_sim[pos] < min_max_sim {
                    min_max_sim = max_sim[pos];
                    best_pos = pos;
                }
            }
            if min_max_sim >= 0.9999 {
                break;
            }
            max_sim[best_pos] = 2.0;
            chosen_cones.push(sample_nodes[best_pos]);
        }

        let actual_cones = chosen_cones.len();
        let mut cone_store = Bq2Store::new(dim);
        cone_store.reserve(actual_cones);
        let mut cone_sm = Vec::with_capacity(actual_cones);
        let mut cone_inv_qnorm = Vec::with_capacity(actual_cones);
        for &cid in &chosen_cones {
            cone_store.push_sig(&bq_store.get_sig(cid as usize));
            let meta = node_meta[cid as usize];
            cone_sm.push(meta.sm_params());
            cone_inv_qnorm.push(meta.legacy_inv_qnorm(dim));
        }

        // 当细粒度方向锥数 > 256（如 100K/1M 规模）时，使用前 64 个 Tammes 地标作为粗粒度超锥导航目录，
        // 将百万节点的锥分配从 O(N * 2048) 加速 3 倍以上，同时持久化紧凑 16KB 查询期两级细锥短表。
        let coarse_count = if use_sm && actual_cones > 256 { 64 } else { 0 };
        let query_coarse_cap = if coarse_count > 0 {
            128.min(actual_cones)
        } else {
            0
        };
        let mut query_coarse_cones = vec![0u16; coarse_count * query_coarse_cap];
        let coarse_shortlists: Vec<Vec<u16>> = if coarse_count > 0 {
            let shortlist_cap = (actual_cones / 4).max(192).min(actual_cones);
            (0..coarse_count)
                .map(|g| {
                    let mut scored: Vec<(i32, u16)> = (0..actual_cones)
                        .map(|c| {
                            let counts = cone_store.sm_counts(g, c, dim);
                            let cos_v =
                                sm_calibrated_cosine_fast(cone_sm[g], cone_sm[c], counts, dim);
                            ((cos_v * 32768.0).round() as i32, c as u16)
                        })
                        .collect();
                    scored.select_nth_unstable_by(shortlist_cap - 1, |a, b| {
                        b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1))
                    });
                    scored.truncate(shortlist_cap);
                    // 先按余弦得分降序提取查询期 Top-128 近邻细锥，再恢复升序供构图期确定性遍历（确保拓扑 CRC32 100% 不变）
                    scored.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
                    for (rank, &(_, c)) in scored.iter().take(query_coarse_cap).enumerate() {
                        query_coarse_cones[g * query_coarse_cap + rank] = c;
                    }
                    scored.sort_unstable_by(|a, b| a.1.cmp(&b.1));
                    scored.into_iter().map(|(_, c)| c).collect()
                })
                .collect()
        } else {
            Vec::new()
        };

        // 先提取高纯度紧切锥正交残差字典（若存在聚簇紧锥结构，流形必然非全局各向同性均匀分布）
        let (residual_dict, build_residual) =
            if use_sm && (force_residual || tangent_residual_enabled()) {
                match ResidualConeDict::build(space, slot_indices, &node_meta, dim) {
                    Some((dict, helper)) => (Some(dict), Some(helper)),
                    None => (None, None),
                }
            } else {
                (None, None)
            };

        // 流形级各向同性检验：选取非地标独立节点，检验其对全部细粒度 Tammes 地标的最大余弦中位数是否落入
        // O(sqrt(2 ln M / D)) 高维随机球面极值区间，精准区分真正的全局均匀流形与多簇流形（如 256 簇 > 64 粗锥）及真实模态锥。
        let iso_coarse_thresh =
            ((3.8 / (dim.max(64) as f64).sqrt()).clamp(0.11, 0.19) * 32768.0) as i32;
        let iso_fine_thresh =
            ((5.6 / (dim.max(64) as f64).sqrt()).clamp(0.19, 0.30) * 32768.0) as i32;
        let is_isotropic_manifold =
            if residual_dict.is_none() && use_sm && actual_cones >= 64 && n > actual_cones + 64 {
                let mut is_landmark = vec![false; n];
                for &cid in &chosen_cones {
                    is_landmark[cid as usize] = true;
                }
                let target_probes = 256usize;
                let step = (n / (target_probes * 2).max(1)).max(1);
                let mut max_fine_scores = Vec::with_capacity(target_probes);
                let mut cursor = step / 2;
                while cursor < n && max_fine_scores.len() < target_probes {
                    if !is_landmark[cursor] && node_meta[cursor].depth > 1e-4 {
                        let sm_i = node_meta[cursor].sm_params();
                        let mut best_sf = i32::MIN;
                        for c in 0..actual_cones {
                            let counts =
                                bq_store.sm_counts_between_stores(cursor, &cone_store, c, dim);
                            let cos_c = sm_calibrated_cosine_fast(sm_i, cone_sm[c], counts, dim);
                            let sf = (cos_c * 32768.0).round() as i32;
                            if sf > best_sf {
                                best_sf = sf;
                            }
                        }
                        max_fine_scores.push(best_sf);
                    }
                    cursor += step;
                }
                if max_fine_scores.len() >= 32 {
                    let mid = max_fine_scores.len() / 2;
                    let (_, &mut med_sf, _) = max_fine_scores.select_nth_unstable(mid);
                    med_sf < iso_fine_thresh
                } else {
                    false
                }
            } else {
                false
            };

        #[cfg(feature = "research")]
        let iso_router_guard = recall_config().variance_stabilized_sketch && is_isotropic_manifold;
        #[cfg(not(feature = "research"))]
        let iso_router_guard = false;

        // 并行计算每个节点的 Top-4 方向锥（前 2 个用于填充锥-壳网格，全部 4 个用于构图跨壳层种子注入）
        let per_node: Vec<_> = (0..n)
            .into_par_iter()
            .map(|i| {
                let meta_i = node_meta[i];
                let sm_i = meta_i.sm_params();
                let mut best = [(0u16, i32::MIN); 4];
                let mut push_cand = |c: usize, score: i32| {
                    if score > best[3].1 {
                        best[3] = (c as u16, score);
                        let mut k = 3;
                        while k > 0 && best[k].1 > best[k - 1].1 {
                            best.swap(k, k - 1);
                            k -= 1;
                        }
                    }
                };

                if coarse_count > 0 {
                    let mut top_coarse = [(0usize, i32::MIN); 4];
                    for g in 0..coarse_count {
                        let counts = bq_store.sm_counts_between_stores(i, &cone_store, g, dim);
                        let cos_g = sm_calibrated_cosine_fast(sm_i, cone_sm[g], counts, dim);
                        let sg = (cos_g * 32768.0).round() as i32;
                        push_cand(g, sg);
                        if sg > top_coarse[3].1 {
                            top_coarse[3] = (g, sg);
                            let mut k = 3;
                            while k > 0 && top_coarse[k].1 > top_coarse[k - 1].1 {
                                top_coarse.swap(k, k - 1);
                                k -= 1;
                            }
                        }
                    }
                    // 当最高粗锥余弦处于高维各向同性近正交区且启用保护时，补扫剩余细锥消除两级粗锥失锁
                    if iso_router_guard && top_coarse[0].1 < iso_coarse_thresh {
                        for c in coarse_count..actual_cones {
                            let counts = bq_store.sm_counts_between_stores(i, &cone_store, c, dim);
                            let cos_c = sm_calibrated_cosine_fast(sm_i, cone_sm[c], counts, dim);
                            push_cand(c, (cos_c * 32768.0).round() as i32);
                        }
                    } else {
                        let mut tested = [0u64; 32];
                        for g in 0..coarse_count {
                            tested[g / 64] |= 1u64 << (g % 64);
                        }
                        for &(g, sg) in &top_coarse {
                            if sg == i32::MIN {
                                continue;
                            }
                            for &c_u16 in &coarse_shortlists[g] {
                                let c = c_u16 as usize;
                                let word = c / 64;
                                let bit = 1u64 << (c % 64);
                                if (tested[word] & bit) != 0 {
                                    continue;
                                }
                                tested[word] |= bit;
                                let counts =
                                    bq_store.sm_counts_between_stores(i, &cone_store, c, dim);
                                let cos_c =
                                    sm_calibrated_cosine_fast(sm_i, cone_sm[c], counts, dim);
                                push_cand(c, (cos_c * 32768.0).round() as i32);
                            }
                        }
                    }
                } else if use_sm {
                    for c in 0..actual_cones {
                        let counts = bq_store.sm_counts_between_stores(i, &cone_store, c, dim);
                        let cos_c = sm_calibrated_cosine_fast(sm_i, cone_sm[c], counts, dim);
                        push_cand(c, (cos_c * 32768.0).round() as i32);
                    }
                } else {
                    let inv_i = meta_i.legacy_inv_qnorm(dim);
                    for c in 0..actual_cones {
                        let bq_d = bq_store.distance_between_stores(i, &cone_store, c, dim);
                        let dot = 4 * dim as i32 - bq_d as i32;
                        let score =
                            (dot as f32 * inv_i * cone_inv_qnorm[c] * 32768.0).round() as i32;
                        push_cand(c, score);
                    }
                }
                (
                    [best[0].0, best[1].0, best[2].0, best[3].0],
                    [best[0], best[1]],
                )
            })
            .collect();

        // 按节点顺序确定性归并每个（方向锥，测地深度带）单元的 Top-POLAR_CELL_CAP 代表节点，保证与线程数无关
        let total_cells = actual_cones * NUM_RADIAL_BANDS;
        let mut cell_scores = vec![[(i32::MIN, EMPTY_NB); POLAR_CELL_CAP]; total_cells];
        let mut node_top_cones = Vec::with_capacity(n);

        for (i, &(cones4, top2)) in per_node.iter().enumerate() {
            node_top_cones.push(cones4);
            let band = radial_band_with_edges(node_meta[i].depth, &band_edges);
            for &(cone_u16, score) in &top2 {
                if score == i32::MIN {
                    continue;
                }
                let cell_idx = cone_u16 as usize * NUM_RADIAL_BANDS + band;
                let slot_arr = &mut cell_scores[cell_idx];
                if score > slot_arr[POLAR_CELL_CAP - 1].0
                    && !slot_arr.iter().any(|&(_, id)| id == i as u32)
                {
                    slot_arr[POLAR_CELL_CAP - 1] = (score, i as u32);
                    let mut k = POLAR_CELL_CAP - 1;
                    while k > 0 && slot_arr[k].0 > slot_arr[k - 1].0 {
                        slot_arr.swap(k, k - 1);
                        k -= 1;
                    }
                }
            }
        }

        let cells = cell_scores
            .into_iter()
            .map(|arr| arr.map(|(_, id)| id))
            .collect();

        (
            Self {
                num_cones: actual_cones,
                cone_store,
                cone_sm,
                cone_inv_qnorm,
                band_edges,
                min_depth,
                max_depth,
                shell_span,
                adaptive_shells,
                cells,
                origin_hubs,
                node_meta,
                query_coarse_count: coarse_count,
                query_coarse_cap,
                query_coarse_cones,
                colocated_store: None,
                residual_dict,
                is_isotropic_manifold,
            },
            node_top_cones,
            build_residual,
        )
    }

    /// 增量插入新节点时同步更新 32B 极坐标/SM 元数据与锥-壳网格单元。
    pub(crate) fn register_incremental(
        &mut self,
        vector: &[f32],
        bq_store: &Bq2Store,
        dim: usize,
        node: u32,
        boundary_factor: f64,
    ) {
        let idx = node as usize;
        self.restore_split_meta();
        let meta = PolarNodeMeta::from_node(vector, boundary_factor, bq_store, idx, dim);
        let depth = meta.depth;
        if idx == self.node_meta.len() {
            self.node_meta.push(meta);
        } else if idx < self.node_meta.len() {
            self.node_meta[idx] = meta;
        }
        if depth > 1e-5 {
            self.min_depth = self.min_depth.min(depth);
            self.max_depth = self.max_depth.max(depth);
        }
        if self.origin_hubs.len() < 16 {
            self.origin_hubs.push(node);
        } else if let Some((pos, _)) = self.origin_hubs.iter().enumerate().max_by(|a, b| {
            self.node_meta[*a.1 as usize]
                .depth
                .total_cmp(&self.node_meta[*b.1 as usize].depth)
        }) && depth < self.node_meta[self.origin_hubs[pos] as usize].depth
        {
            self.origin_hubs[pos] = node;
        }
        if self.num_cones == 0 {
            return;
        }
        let band = self.band_of(depth);
        let sm_i = meta.sm_params();
        let mut best_cone = 0usize;
        let mut best_score = f64::NEG_INFINITY;
        for c in 0..self.num_cones {
            let counts = bq_store.sm_counts_between_stores(idx, &self.cone_store, c, dim);
            let score = sm_calibrated_cosine_fast(sm_i, self.cone_sm[c], counts, dim);
            if score > best_score {
                best_score = score;
                best_cone = c;
            }
        }
        let cell = &mut self.cells[best_cone * NUM_RADIAL_BANDS + band];
        if let Some(empty_slot) = cell.iter_mut().find(|&&mut id| id == EMPTY_NB) {
            *empty_slot = node;
        }
    }

    /// 按节点编号读取当前布局中的元数据。
    #[inline]
    pub(crate) fn node_meta(&self, idx: usize) -> Option<&PolarNodeMeta> {
        self.node_meta.get(idx).or_else(|| {
            self.colocated_store
                .as_ref()
                .filter(|store| idx < store.len())
                .map(|store| store.get(idx).meta)
        })
    }

    /// 修改路由表前恢复被合表回收的元数据，再撤销只读合表布局。
    pub(crate) fn restore_split_meta(&mut self) {
        if let Some(store) = &self.colocated_store {
            if self.node_meta.is_empty() {
                self.node_meta = (0..store.len()).map(|idx| *store.get(idx).meta).collect();
            }
            self.colocated_store = None;
        }
    }

    /// 选取最靠近双曲中心且具有明确方向的地标节点作为默认双曲图入口。
    pub(crate) fn entry_hub(&self) -> u32 {
        self.origin_hubs
            .iter()
            .copied()
            .find(|&id| {
                self.node_meta(id as usize)
                    .is_some_and(|meta| meta.depth > 1e-4)
            })
            .or_else(|| self.origin_hubs.first().copied())
            .unwrap_or(0)
    }

    /// 统计路由图谱常驻热内存开销（字节）。
    pub(crate) fn hot_bytes(&self) -> usize {
        self.cone_inv_qnorm.len() * std::mem::size_of::<f32>()
            + self.cone_store.hot_bytes()
            + self.cone_sm.len() * std::mem::size_of::<SmCodeParams>()
            + self.cells.len() * std::mem::size_of::<[u32; POLAR_CELL_CAP]>()
            + self.origin_hubs.len() * std::mem::size_of::<u32>()
            + self.node_meta.len() * std::mem::size_of::<PolarNodeMeta>()
            + self.query_coarse_cones.len() * std::mem::size_of::<u16>()
            + self
                .colocated_store
                .as_ref()
                .map_or(0, |store| store.hot_bytes())
            + self
                .residual_dict
                .as_ref()
                .map_or(0, |dict| dict.hot_bytes())
    }

    /// 构图期传统全带提取（保留供 A5/A6 消融复现）。
    pub(crate) fn collect_build_seeds(&self, idx: u32, top_cones: &[u16; 4], out: &mut Vec<u32>) {
        out.clear();
        for &hub in &self.origin_hubs {
            if hub != idx {
                out.push(hub);
            }
        }
        if self.num_cones == 0 {
            return;
        }
        for &cone_u16 in top_cones {
            let base = (cone_u16 as usize) * NUM_RADIAL_BANDS;
            for band in 0..NUM_RADIAL_BANDS {
                for &rep in &self.cells[base + band] {
                    if rep == EMPTY_NB {
                        break;
                    }
                    if rep != idx {
                        out.push(rep);
                    }
                }
            }
        }
    }

    /// 构图期聚焦选种：提取球心枢纽 + 内层测地主干带 [0..=2] + 当前节点同壳层及相邻升降壳层，
    /// 剔除无关远端壳层的冗余冷存评估，同时保证四向罗盘四个象限的候选充裕度。
    pub(crate) fn collect_focused_build_seeds(
        &self,
        idx: u32,
        depth: f32,
        top_cones: &[u16; 4],
        out: &mut Vec<u32>,
    ) {
        out.clear();
        for &hub in &self.origin_hubs {
            if hub != idx {
                out.push(hub);
            }
        }
        if self.num_cones == 0 {
            return;
        }
        let b_self = self.band_of(depth);
        let mut band_mask = 0b0000_0111u8;
        let b_lo = b_self.saturating_sub(1);
        let b_hi = (b_self + 1).min(NUM_RADIAL_BANDS - 1);
        for b in b_lo..=b_hi {
            band_mask |= 1u8 << b;
        }
        for (rank, &cone_u16) in top_cones.iter().take(3).enumerate() {
            let base = (cone_u16 as usize) * NUM_RADIAL_BANDS;
            for band in 0..NUM_RADIAL_BANDS {
                if (band_mask & (1u8 << band)) == 0 {
                    continue;
                }
                let take = if band == b_self && rank < 2 { 6 } else { 4 };
                for &rep in self.cells[base + band].iter().take(take) {
                    if rep == EMPTY_NB {
                        break;
                    }
                    if rep != idx {
                        out.push(rep);
                    }
                }
            }
        }
    }

    /// 传统查询期全带种子提取（保留供 A5/A6 消融复现）。
    pub(crate) fn collect_query_seeds(
        &self,
        q_sig: &Bq2Signature,
        q_inv_qnorm: f32,
        dim: usize,
        probe_cones: usize,
        cone_buf: &mut Vec<(i32, usize)>,
        seed_buf: &mut Vec<u32>,
    ) {
        seed_buf.clear();
        seed_buf.extend_from_slice(&self.origin_hubs);
        if self.num_cones == 0 || probe_cones == 0 {
            return;
        }
        cone_buf.clear();
        for c in 0..self.num_cones {
            let bq_d = self.cone_store.distance_to_sig(c, q_sig, dim);
            let dot = 4.0 * dim as f32 - bq_d as f32;
            let score = (dot * q_inv_qnorm * self.cone_inv_qnorm[c] * 32768.0).round() as i32;
            cone_buf.push((score, c));
        }
        let take = probe_cones.min(cone_buf.len());
        if cone_buf.len() > take {
            cone_buf.select_nth_unstable_by(take, |a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            cone_buf.truncate(take);
        }
        for &(_, cone) in cone_buf.iter() {
            let base = cone * NUM_RADIAL_BANDS;
            for band in 0..NUM_RADIAL_BANDS {
                for &rep in &self.cells[base + band] {
                    if rep == EMPTY_NB {
                        break;
                    }
                    seed_buf.push(rep);
                }
            }
        }
    }

    /// 闭式测地凸支撑投影顶点选种（Convex-Support Apex-Focused Cone-Band Seeding）：
    /// 利用 `g(r) = cosh(d_q)cosh(r) - c * sinh(d_q)sinh(r)` 的严格凸性（`g''(r) = g(r) > 0`），
    /// 其在数据集真实径向支撑 `[d_min, d_max]` 上的唯一全局最优点恒为
    /// `d*_c = clamp(atanh((tanh(d_q) * cos_c)_+), d_min, d_max)`，消除空内壳盲探。
    /// 当 `two_level_cones` 开启且 `query_coarse_count > 0` 时，先评估 64 个 Tammes 粗地标再展开 Top-K 近邻细锥，
    /// 将每查询锥扫描量从 2048 压缩至 ~280（节省 ~86% 路由器算力）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn collect_focused_query_seeds(
        &self,
        q_sig: &Bq2Signature,
        q_sm: SmCodeParams,
        query_depth: f64,
        dim: usize,
        ef: usize,
        probe_cones: usize,
        two_level_cones: bool,
        traversal: TraversalProfile,
        cone_buf: &mut Vec<(i32, usize)>,
        seed_buf: &mut Vec<u32>,
    ) {
        #[cfg(feature = "research")]
        let iso_router_guard = recall_config().variance_stabilized_sketch;
        #[cfg(not(feature = "research"))]
        let iso_router_guard = false;

        seed_buf.clear();
        let hollow_shell = self.adaptive_shells && self.min_depth > 0.30;
        let hub_take = if hollow_shell && ef <= 64 {
            self.origin_hubs.len().min(48)
        } else {
            self.origin_hubs.len()
        };
        seed_buf.extend_from_slice(&self.origin_hubs[..hub_take]);
        if self.num_cones == 0 || probe_cones == 0 {
            return;
        }
        cone_buf.clear();

        if two_level_cones
            && self.query_coarse_count > 0
            && self.query_coarse_cap > 0
            && self.num_cones <= 2048
        {
            let coarse_k = if ef <= 64 {
                8usize
            } else if ef <= 256 {
                10usize
            } else {
                12usize
            };
            let per_coarse_limit = if ef <= 64 {
                88.min(self.query_coarse_cap)
            } else if ef <= 256 {
                112.min(self.query_coarse_cap)
            } else {
                self.query_coarse_cap
            };
            let mut top_coarse = [(0usize, i32::MIN); 12];
            let g_cnt = self.query_coarse_count;
            for g in 0..g_cnt {
                if g + 2 < g_cnt {
                    self.cone_store.prefetch_node_exact(g + 2);
                }
                let counts = self.cone_store.sm_counts_to_sig(g, q_sig, dim);
                let cos_g = sm_calibrated_cosine_fast(q_sm, self.cone_sm[g], counts, dim);
                let sg = (cos_g * 32768.0).round() as i32;
                cone_buf.push((sg, g));
                if sg > top_coarse[coarse_k - 1].1 {
                    top_coarse[coarse_k - 1] = (g, sg);
                    let mut k = coarse_k - 1;
                    while k > 0 && top_coarse[k].1 > top_coarse[k - 1].1 {
                        top_coarse.swap(k, k - 1);
                        k -= 1;
                    }
                }
            }
            // 各向同性正交失锁检测：若查询与全部 64 个粗地标的最大余弦 < 0.20，直接补扫剩余细锥（真实流形最大余弦 > 0.45，零触发）
            if iso_router_guard && top_coarse[0].1 < 6554 {
                for c in g_cnt..self.num_cones {
                    if c + 3 < self.num_cones {
                        self.cone_store.prefetch_node_exact(c + 3);
                    }
                    let counts = self.cone_store.sm_counts_to_sig(c, q_sig, dim);
                    let cos_c = sm_calibrated_cosine_fast(q_sm, self.cone_sm[c], counts, dim);
                    cone_buf.push(((cos_c * 32768.0).round() as i32, c));
                }
            } else {
                let mut tested = [0u64; 32];
                for g in 0..g_cnt {
                    tested[g >> 6] |= 1u64 << (g & 63);
                }
                let mut fine_list = [0u16; 1536];
                let mut fine_len = 0usize;
                for &(g, sg) in top_coarse.iter().take(coarse_k) {
                    if sg == i32::MIN {
                        continue;
                    }
                    let base = g * self.query_coarse_cap;
                    for &c_u16 in &self.query_coarse_cones[base..base + per_coarse_limit] {
                        let c = c_u16 as usize;
                        let w = c >> 6;
                        let b = 1u64 << (c & 63);
                        if (tested[w] & b) == 0 {
                            tested[w] |= b;
                            fine_list[fine_len] = c_u16;
                            fine_len += 1;
                        }
                    }
                }
                let pf_init = 3.min(fine_len);
                for i in 0..pf_init {
                    self.cone_store.prefetch_node_exact(fine_list[i] as usize);
                }
                for i in 0..fine_len {
                    if i + 3 < fine_len {
                        self.cone_store
                            .prefetch_node_exact(fine_list[i + 3] as usize);
                    }
                    let c = fine_list[i] as usize;
                    let counts = self.cone_store.sm_counts_to_sig(c, q_sig, dim);
                    let cos_c = sm_calibrated_cosine_fast(q_sm, self.cone_sm[c], counts, dim);
                    cone_buf.push(((cos_c * 32768.0).round() as i32, c));
                }
            }
        } else {
            for c in 0..self.num_cones {
                let counts = self.cone_store.sm_counts_to_sig(c, q_sig, dim);
                let cos_c = sm_calibrated_cosine_fast(q_sm, self.cone_sm[c], counts, dim);
                let score = (cos_c * 32768.0).round() as i32;
                cone_buf.push((score, c));
            }
        }
        let max_cone_score = cone_buf.iter().map(|&(s, _)| s).max().unwrap_or(32768);
        let isotropic_query = iso_router_guard && max_cone_score < 9175;
        let seed_budget = if hollow_shell {
            if isotropic_query && ef > 128 {
                (ef * 4).clamp(560, 2048)
            } else {
                match traversal {
                    TraversalProfile::Standard => (ef * 4).clamp(176, 560),
                    TraversalProfile::Balanced => (ef * 3).clamp(128, 420),
                    TraversalProfile::Lean => (ef * 2 + 16).clamp(88, 300),
                }
            }
        } else {
            usize::MAX
        };
        let scale_bonus = (self.num_cones / 18).min(48);
        let max_take = if isotropic_query && ef > 128 {
            (ef / 2 + 96).clamp(160, 512)
        } else if hollow_shell && ef > 128 {
            144
        } else {
            112
        };
        let eff_probe = if isotropic_query && ef > 128 {
            (ef / 2).clamp(probe_cones, 512)
        } else {
            probe_cones
        };
        let take = (eff_probe + scale_bonus).min(max_take).min(cone_buf.len());
        if cone_buf.len() > take {
            cone_buf.select_nth_unstable_by(take, |a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            cone_buf.truncate(take);
        }
        cone_buf.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

        let tanh_dq = query_depth.tanh();
        let b_query = self.band_of(query_depth as f32);
        let horiz_gate = if hollow_shell {
            (f64::from(self.min_depth) + 0.15).min(0.85)
        } else {
            0.85
        };

        for (rank, &(score, cone)) in cone_buf.iter().enumerate() {
            if seed_buf.len() >= seed_budget {
                break;
            }
            let cos_pos = (f64::from(score) / 32768.0).clamp(0.0, 0.9999);
            let raw_apex = (tanh_dq * cos_pos).clamp(0.0, 0.9999).atanh() as f32;
            let apex_depth = if hollow_shell {
                raw_apex.clamp(self.min_depth, self.max_depth)
            } else {
                raw_apex
            };
            let b_apex = self.band_of(apex_depth);
            let (b_lo, b_hi, rep_cap) = if hollow_shell {
                if isotropic_query && rank < 128 {
                    (
                        b_apex.saturating_sub(1),
                        (b_apex + 2).max(b_query.min(3)).min(NUM_RADIAL_BANDS - 1),
                        if ef <= 64 { 6 } else { POLAR_CELL_CAP },
                    )
                } else if rank < 12 {
                    (
                        b_apex.saturating_sub(1),
                        (b_apex + 1).min(NUM_RADIAL_BANDS - 1),
                        if ef <= 64 { 6 } else { POLAR_CELL_CAP },
                    )
                } else if rank < 32 {
                    (b_apex.saturating_sub(1), b_apex, 4)
                } else {
                    (b_apex, b_apex, 3)
                }
            } else if rank < 16 {
                (
                    b_apex.saturating_sub(1),
                    (b_apex + 2).min(NUM_RADIAL_BANDS - 1),
                    POLAR_CELL_CAP,
                )
            } else if rank < 36 {
                (
                    b_apex.saturating_sub(1),
                    (b_apex + 1).min(NUM_RADIAL_BANDS - 1),
                    5,
                )
            } else {
                (b_apex.saturating_sub(1), b_apex, 3)
            };
            let mut band_mask = 0u8;
            for b in b_lo..=b_hi {
                band_mask |= 1u8 << b;
            }
            let query_band_cones = if hollow_shell && ef <= 64 { 6 } else { 10 };
            // 对最对齐的方向锥同时覆盖查询自身所在壳层邻域，兼顾高深度聚簇语义直达
            if rank < query_band_cones && query_depth > horiz_gate {
                let q_lo = b_query.saturating_sub(1);
                let q_hi = (b_query + 1).min(NUM_RADIAL_BANDS - 1);
                for b in q_lo..=q_hi {
                    band_mask |= 1u8 << b;
                }
            }
            let base = cone * NUM_RADIAL_BANDS;
            for band in 0..NUM_RADIAL_BANDS {
                if (band_mask & (1u8 << band)) == 0 {
                    continue;
                }
                for &rep in self.cells[base + band].iter().take(rep_cap) {
                    if rep == EMPTY_NB || seed_buf.len() >= seed_budget {
                        break;
                    }
                    seed_buf.push(rep);
                }
            }
        }
    }
}
