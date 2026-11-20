//! 并行构图共享上下文、距离评分与邻居剪枝。

use super::*;

pub(crate) struct ParallelBuildContext<'a> {
    pub(crate) adj: &'a ConcurrentFlatAdj,
    pub(crate) n: usize,
    pub(crate) dim: usize,
    pub(crate) m0: usize,
    pub(crate) ef: usize,
    pub(crate) alpha: f32,
    pub(crate) entry_point: u32,
    pub(crate) sigs: &'a Bq2Store,
    pub(crate) locks: &'a StripedSpinLocks,
    pub(crate) profile: &'a BuildProfile,
    /// 反向剪枝模式：true=旧的"每条边全量剪枝"(A/B对照用)，false=惰性剪枝(默认)
    pub(crate) eager: bool,
    pub(crate) quantized_build: bool,
    pub(crate) native_space: Option<&'a NativeSpace<'a>>,
    pub(crate) geometry_codes: Option<&'a [GeometryCode]>,
    pub(crate) polar_router: Option<&'a hyperbolic::PolarRouter>,
    #[cfg(feature = "research")]
    pub(crate) direction_store:
        Option<&'a crate::index::pelorus::engine::direction::DirectionStore>,
    pub(crate) node_top_cones: Option<&'a [[u16; 4]]>,
    pub(crate) build_residual: Option<&'a hyperbolic::BuildResidualHelper>,
}
impl ParallelBuildContext<'_> {
    #[inline]
    pub(crate) fn prefetch_direction(&self, node: usize) {
        #[cfg(feature = "research")]
        if let Some(store) = self.direction_store {
            store.prefetch(
                node,
                self.native_space
                    .expect("方向缓存需要原始向量")
                    .vector(node),
            );
            return;
        }
        if let Some(res) = self.build_residual {
            res.prefetch(node);
            return;
        }
        self.sigs.prefetch_sig(node);
    }

    /// 并行调度、锁与邻接表共享同一实现，距离由索引几何决定。
    #[inline]
    pub(crate) fn distance(&self, left: u32, right: u32) -> hyperbolic::Distance {
        if left == right && self.native_space.is_some() {
            return hyperbolic::Distance(0.0);
        }
        #[cfg(feature = "research")]
        if self.quantized_build
            && let Some(store) = self.direction_store
            && let Some(router) = self.polar_router
        {
            let a = router.node_meta[left as usize];
            let b = router.node_meta[right as usize];
            return hyperbolic::Distance(a.sm_poincare_proxy(
                f64::from(b.radius),
                f64::from(b.boundary_factor),
                store.pair_cosine(
                    left as usize,
                    right as usize,
                    self.native_space.unwrap().vector(left as usize),
                    self.native_space.unwrap().vector(right as usize),
                ),
            ));
        }
        let distance = if self.quantized_build {
            #[cfg(feature = "research")]
            let use_sm = matches!(
                hyperbolic::hyperbolic_strategy(),
                hyperbolic::HyperbolicStrategy::Full
                    | hyperbolic::HyperbolicStrategy::FixedShellCompass
                    | hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                    | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
            );
            #[cfg(not(feature = "research"))]
            let use_sm = true;
            if use_sm {
                if let Some(router) = self.polar_router
                    && let (Some(ml), Some(mr)) = (
                        router.node_meta.get(left as usize),
                        router.node_meta.get(right as usize),
                    )
                {
                    // 同切锥节点对优先采用正交残差量化余弦，异锥节点对无缝回退全局签名
                    let cos_sim = if let Some(res) = self.build_residual
                        && let Some(cos_intra) =
                            res.intra_cone_cosine(left as usize, right as usize, self.dim)
                    {
                        cos_intra
                    } else {
                        let counts = self.sigs.sm_counts(left as usize, right as usize, self.dim);
                        crate::index::pelorus::engine::geometry::sm_calibrated_cosine_fast(
                            ml.sm_params(),
                            mr.sm_params(),
                            counts,
                            self.dim,
                        )
                    };
                    ml.sm_poincare_proxy(
                        f64::from(mr.radius),
                        f64::from(mr.boundary_factor),
                        cos_sim,
                    )
                } else if let Some(codes) = self.geometry_codes {
                    let counts = self.sigs.sm_counts(left as usize, right as usize, self.dim);
                    codes[left as usize].proxy_sm(codes[right as usize], counts, self.dim)
                } else {
                    0.0
                }
            } else if let Some(codes) = self.geometry_codes {
                codes[left as usize].proxy(
                    codes[right as usize],
                    self.sigs.distance(left as usize, right as usize, self.dim),
                    self.dim,
                )
            } else {
                0.0
            }
        } else if let Some(space) = self.native_space {
            space.proxy(
                space.vector(left as usize),
                space.boundary_factors[left as usize],
                right as usize,
            )
        } else {
            self.sigs.distance(left as usize, right as usize, self.dim) as f64
        };
        hyperbolic::Distance(distance)
    }

    /// 优先从极坐标路由缓存中 O(1) 读取无量纲测地深度 sqrt(c) * R，无需每次重复计算 asinh。
    #[inline]
    pub(crate) fn dimensionless_depth(&self, space: &NativeSpace<'_>, node: u32) -> f64 {
        if let Some(router) = self.polar_router
            && let Some(meta) = router.node_meta.get(node as usize)
        {
            return f64::from(meta.depth);
        }
        hyperbolic::dimensionless_depth(space.boundary_factors[node as usize])
    }

    pub(crate) fn prune(
        &self,
        target: u32,
        candidates: &[(hyperbolic::Distance, u32)],
    ) -> Vec<u32> {
        let Some(space) = self.native_space else {
            let candidates = candidates
                .iter()
                .map(|&(d, id)| (d.0 as u32, id))
                .collect::<Vec<_>>();
            return QuIVer::vamana_select(
                self.sigs,
                target,
                &candidates,
                self.m0,
                self.dim,
                self.alpha,
            );
        };
        #[cfg(feature = "research")]
        let strategy = hyperbolic::hyperbolic_strategy();
        #[cfg(feature = "research")]
        if matches!(
            strategy,
            hyperbolic::HyperbolicStrategy::GeodesicPruning
                | hyperbolic::HyperbolicStrategy::GeodesicPruningWithFill
                | hyperbolic::HyperbolicStrategy::GeodesicPortals
        ) {
            let mut selected = Vec::with_capacity(self.m0);
            for &(distance, candidate) in candidates {
                if candidate == target {
                    continue;
                }
                if selected.len() >= self.m0 {
                    break;
                }
                let target_distance = crate::index::pelorus::engine::metric::poincare_distance(
                    distance.0,
                    space.curvature,
                );
                let dominated = selected.iter().any(|&neighbor| {
                    f64::from(self.alpha)
                        * crate::index::pelorus::engine::metric::poincare_distance(
                            self.distance(candidate, neighbor).0,
                            space.curvature,
                        )
                        <= target_distance
                });
                if !dominated {
                    selected.push(candidate);
                }
            }
            if strategy != hyperbolic::HyperbolicStrategy::GeodesicPruning
                && selected.len() < self.m0
            {
                for &(_, candidate) in candidates {
                    if selected.len() >= self.m0 {
                        break;
                    }
                    if candidate != target && !selected.contains(&candidate) {
                        selected.push(candidate);
                    }
                }
            }
            return selected;
        }

        // 消融对比分支：保留第一代双纤维剪枝（A2/A3/A4/A5）以精确隔离第二代四向罗盘与校准度量的增益
        #[cfg(feature = "research")]
        let use_compass_pruning = matches!(
            strategy,
            hyperbolic::HyperbolicStrategy::Full
                | hyperbolic::HyperbolicStrategy::CalibratedCompass
                | hyperbolic::HyperbolicStrategy::FixedShellCompass
                | hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
        );
        #[cfg(not(feature = "research"))]
        let use_compass_pruning = true;

        #[cfg(feature = "research")]
        let use_sign_magnitude = matches!(
            strategy,
            hyperbolic::HyperbolicStrategy::Full
                | hyperbolic::HyperbolicStrategy::FixedShellCompass
                | hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
        );
        #[cfg(not(feature = "research"))]
        let use_sign_magnitude = true;

        #[cfg(feature = "research")]
        let use_compass_pruning = use_compass_pruning && hyperbolic::geometry_components().compass;
        if !use_compass_pruning {
            let scale_c = space.curvature.sqrt();
            let geo_limit = ((self.m0 * 5) / 8).max(1);
            let r_target = self.dimensionless_depth(space, target);
            let mut selected: Vec<(u32, f64)> = Vec::with_capacity(self.m0);
            for &(distance, candidate) in candidates {
                if candidate == target || selected.len() >= geo_limit {
                    continue;
                }
                let r_cand = self.dimensionless_depth(space, candidate);
                let target_distance = crate::index::pelorus::engine::metric::poincare_distance(
                    distance.0,
                    space.curvature,
                ) * scale_c;
                let bq_target_cand =
                    self.sigs
                        .distance(target as usize, candidate as usize, self.dim);
                let dominated = selected.iter().any(|&(neighbor, r_nb)| {
                    let bq_nb_cand =
                        self.sigs
                            .distance(candidate as usize, neighbor as usize, self.dim);
                    if bq_nb_cand > bq_target_cand {
                        return false;
                    }
                    let stem_excess = ((r_target - r_cand).abs() - (r_nb - r_cand).abs()).max(0.0);
                    let effective_target = (target_distance - stem_excess).max(0.0);
                    let alpha = f64::from(self.alpha);
                    if alpha * (r_nb - r_cand).abs() > effective_target {
                        return false;
                    }
                    alpha
                        * crate::index::pelorus::engine::metric::poincare_distance(
                            self.distance(candidate, neighbor).0,
                            space.curvature,
                        )
                        * scale_c
                        <= effective_target
                });
                if !dominated {
                    selected.push((candidate, r_cand));
                }
            }
            if selected.len() < self.m0 {
                let mut horiz_candidates: Vec<(u32, hyperbolic::Distance, u32, f64)> =
                    Vec::with_capacity(candidates.len());
                for &(distance, candidate) in candidates {
                    if candidate == target || selected.iter().any(|&(s, _)| s == candidate) {
                        continue;
                    }
                    let r_cand = self.dimensionless_depth(space, candidate);
                    let bq_d = self
                        .sigs
                        .distance(target as usize, candidate as usize, self.dim);
                    let shell_penalty =
                        (0.25 * self.dim as f64 * (r_target - r_cand).abs()).round() as u32;
                    horiz_candidates.push((
                        bq_d.saturating_add(shell_penalty),
                        distance,
                        candidate,
                        r_cand,
                    ));
                }
                horiz_candidates.sort_unstable_by(|a, b| {
                    a.0.cmp(&b.0)
                        .then_with(|| a.1.cmp(&b.1))
                        .then_with(|| a.2.cmp(&b.2))
                });
                for &(_, _, candidate, r_cand) in &horiz_candidates {
                    if selected.len() >= self.m0 {
                        break;
                    }
                    let bq_target_cand =
                        self.sigs
                            .distance(target as usize, candidate as usize, self.dim);
                    let dominated = selected.iter().any(|&(neighbor, r_nb)| {
                        if (r_nb - r_cand).abs() > 1.0 {
                            return false;
                        }
                        let bq_nb_cand =
                            self.sigs
                                .distance(candidate as usize, neighbor as usize, self.dim);
                        bq_nb_cand > 0 && self.alpha * (bq_nb_cand as f32) <= bq_target_cand as f32
                    });
                    if !dominated {
                        selected.push((candidate, r_cand));
                    }
                }
                if selected.len() < self.m0 {
                    for &(_, _, candidate, r_cand) in &horiz_candidates {
                        if selected.len() >= self.m0 {
                            break;
                        }
                        if !selected.iter().any(|&(s, _)| s == candidate) {
                            selected.push((candidate, r_cand));
                        }
                    }
                }
            }
            return selected.into_iter().map(|(id, _)| id).collect();
        }

        // 校准球面角度距离：A7 采用 SM 校准球面余弦，A6 保留固定系数 Hermite 校准余弦
        let ang_dist = |a: u32, b: u32| -> u32 {
            if a == b {
                return 0;
            }
            #[cfg(feature = "research")]
            if matches!(
                strategy,
                hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                    | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
            ) {
                return hyperbolic::cosine_to_angular_loss(
                    self.direction_store
                        .as_ref()
                        .expect("方向对照必须预编码")
                        .pair_cosine(
                            a as usize,
                            b as usize,
                            space.vector(a as usize),
                            space.vector(b as usize),
                        ),
                );
            }
            if use_sign_magnitude {
                if let Some(res) = self.build_residual
                    && let Some(cos_intra) = res.intra_cone_cosine(a as usize, b as usize, self.dim)
                {
                    return hyperbolic::cosine_to_angular_loss(cos_intra);
                }
                let counts = self.sigs.sm_counts(a as usize, b as usize, self.dim);
                let sm_a = if let Some(router) = self.polar_router
                    && let Some(m) = router.node_meta.get(a as usize)
                {
                    m.sm_params()
                } else if let Some(codes) = self.geometry_codes
                    && let Some(c) = codes.get(a as usize)
                {
                    c.sm
                } else {
                    crate::index::pelorus::engine::quantization::SmCodeParams::from_vector_and_store(
                        space.vector(a as usize),
                        self.sigs,
                        a as usize,
                    )
                    .0
                };
                let sm_b = if let Some(router) = self.polar_router
                    && let Some(m) = router.node_meta.get(b as usize)
                {
                    m.sm_params()
                } else if let Some(codes) = self.geometry_codes
                    && let Some(c) = codes.get(b as usize)
                {
                    c.sm
                } else {
                    crate::index::pelorus::engine::quantization::SmCodeParams::from_vector_and_store(
                        space.vector(b as usize),
                        self.sigs,
                        b as usize,
                    )
                    .0
                };
                return hyperbolic::cosine_to_angular_loss(
                    crate::index::pelorus::engine::geometry::sm_calibrated_cosine_fast(
                        sm_a, sm_b, counts, self.dim,
                    ),
                );
            }
            let bq_d = self.sigs.distance(a as usize, b as usize, self.dim);
            let inv_a = self
                .polar_router
                .and_then(|r| {
                    r.node_meta
                        .get(a as usize)
                        .map(|m| m.legacy_inv_qnorm(self.dim))
                })
                .unwrap_or_else(|| {
                    hyperbolic::bq2_inv_qnorm(self.dim, self.sigs.strong_ones(a as usize))
                });
            let inv_b = self
                .polar_router
                .and_then(|r| {
                    r.node_meta
                        .get(b as usize)
                        .map(|m| m.legacy_inv_qnorm(self.dim))
                })
                .unwrap_or_else(|| {
                    hyperbolic::bq2_inv_qnorm(self.dim, self.sigs.strong_ones(b as usize))
                });
            hyperbolic::cosine_to_angular_loss(hyperbolic::calibrated_cosine_from_bq(
                bq_d, self.dim, inv_a, inv_b,
            ))
        };

        let scale_c = space.curvature.sqrt();
        let r_target = self.dimensionless_depth(space, target);
        // 保持经过全尺度验证的四向罗盘黄金配额（3/8 测地主干 + 4/8 等深球面环 + 1/8 双向测地升降机）
        let (geo_limit, iso_tol, elev_step, elev_min, elev_dom, div_tol) =
            (((self.m0 * 3) / 8).max(1), 0.60, 0.65, 0.18, 0.10, 1.0);
        let mut selected: Vec<(u32, f64)> = Vec::with_capacity(self.m0);

        // 罗盘象限 1：平行移动测地双曲纤维（兼顾局部精确测地近邻与跨壳层主干连通）
        for &(distance, candidate) in candidates {
            if candidate == target || selected.len() >= geo_limit {
                continue;
            }
            let r_cand = self.dimensionless_depth(space, candidate);
            let target_distance = crate::index::pelorus::engine::metric::poincare_distance(
                distance.0,
                space.curvature,
            ) * scale_c;
            let ang_target_cand = ang_dist(target, candidate);
            let dominated = selected.iter().any(|&(neighbor, r_nb)| {
                let ang_nb_cand = ang_dist(candidate, neighbor);
                if ang_nb_cand > ang_target_cand {
                    return false;
                }
                let stem_excess = ((r_target - r_cand).abs() - (r_nb - r_cand).abs()).max(0.0);
                let effective_target = (target_distance - stem_excess).max(0.0);
                let alpha = f64::from(self.alpha);
                if alpha * (r_nb - r_cand).abs() > effective_target {
                    return false;
                }
                alpha
                    * crate::index::pelorus::engine::metric::poincare_distance(
                        self.distance(candidate, neighbor).0,
                        space.curvature,
                    )
                    * scale_c
                    <= effective_target
            });
            if !dominated {
                selected.push((candidate, r_cand));
            }
        }

        if selected.len() < self.m0 {
            // 预存 (线性预计算排序键, 角度损失, 双曲代理距离, 候选节点ID, 测地深度, 已选中标记)，
            // 消除 O(K log K) 比较闭包中的重复浮点乘法/取整与重复线性判重。
            let mut pool: Vec<(u32, u32, hyperbolic::Distance, u32, f64, bool)> =
                Vec::with_capacity(candidates.len());
            for &(distance, candidate) in candidates {
                if candidate == target || selected.iter().any(|&(s, _)| s == candidate) {
                    continue;
                }
                let r_cand = self.dimensionless_depth(space, candidate);
                let ang_d = ang_dist(target, candidate);
                let iso_key = ang_d.saturating_add(
                    (0.85 * hyperbolic::ANGULAR_SCALE * (r_target - r_cand).abs()).round() as u32,
                );
                pool.push((iso_key, ang_d, distance, candidate, r_cand, false));
            }

            // 罗盘象限 2：等深极限球面环（强径向护盾保护同壳层横向小世界边，抵御外层反向边冲刷）
            let iso_target = ((self.m0 * 7) / 8).max(selected.len()).min(self.m0);
            pool.sort_unstable_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| a.2.cmp(&b.2))
                    .then_with(|| a.3.cmp(&b.3))
            });
            for item in &mut pool {
                if selected.len() >= iso_target {
                    break;
                }
                let ang_target_cand = item.1;
                let candidate = item.3;
                let r_cand = item.4;
                let dominated = selected.iter().any(|&(neighbor, r_nb)| {
                    if (r_nb - r_cand).abs() > iso_tol {
                        return false;
                    }
                    let ang_nb_cand = ang_dist(candidate, neighbor);
                    self.alpha * (ang_nb_cand as f32) <= ang_target_cand as f32
                });
                if !dominated {
                    item.5 = true;
                    selected.push((candidate, r_cand));
                }
            }

            // 罗盘象限 3：向内测地升降机与向外测地升降机
            let rem = self.m0.saturating_sub(selected.len());
            let inward_target = selected.len() + rem / 2;
            for item in &mut pool {
                let step = ((r_target - item.4) - elev_step).abs();
                item.0 = item
                    .1
                    .saturating_add((0.15 * hyperbolic::ANGULAR_SCALE * step).round() as u32);
            }
            pool.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
            for item in &mut pool {
                if selected.len() >= inward_target {
                    break;
                }
                if item.5 || item.4 >= r_target - elev_min {
                    continue;
                }
                let ang_target_cand = item.1;
                let candidate = item.3;
                let r_cand = item.4;
                let dominated = selected.iter().any(|&(neighbor, r_nb)| {
                    r_nb < r_target - elev_dom
                        && self.alpha * (ang_dist(candidate, neighbor) as f32)
                            <= ang_target_cand as f32
                });
                if !dominated {
                    item.5 = true;
                    selected.push((candidate, r_cand));
                }
            }

            for item in &mut pool {
                let step = ((item.4 - r_target) - elev_step).abs();
                item.0 = item
                    .1
                    .saturating_add((0.15 * hyperbolic::ANGULAR_SCALE * step).round() as u32);
            }
            pool.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
            for item in &mut pool {
                if selected.len() >= self.m0 {
                    break;
                }
                if item.5 || item.4 <= r_target + elev_min {
                    continue;
                }
                let ang_target_cand = item.1;
                let candidate = item.3;
                let r_cand = item.4;
                let dominated = selected.iter().any(|&(neighbor, r_nb)| {
                    r_nb > r_target + elev_dom
                        && self.alpha * (ang_dist(candidate, neighbor) as f32)
                            <= ang_target_cand as f32
                });
                if !dominated {
                    item.5 = true;
                    selected.push((candidate, r_cand));
                }
            }

            // 罗盘象限 4：跨壳层角度多样性补充与保底填满 m0
            if selected.len() < self.m0 {
                for item in &mut pool {
                    item.0 = item.1.saturating_add(
                        (0.25 * hyperbolic::ANGULAR_SCALE * (r_target - item.4).abs()).round()
                            as u32,
                    );
                }
                pool.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
                for item in &mut pool {
                    if selected.len() >= self.m0 {
                        break;
                    }
                    if item.5 {
                        continue;
                    }
                    let ang_target_cand = item.1;
                    let candidate = item.3;
                    let r_cand = item.4;
                    let dominated = selected.iter().any(|&(neighbor, r_nb)| {
                        (r_nb - r_cand).abs() <= div_tol
                            && self.alpha * (ang_dist(candidate, neighbor) as f32)
                                <= ang_target_cand as f32
                    });
                    if !dominated {
                        item.5 = true;
                        selected.push((candidate, r_cand));
                    }
                }
                for item in &pool {
                    if selected.len() >= self.m0 {
                        break;
                    }
                    if !item.5 {
                        selected.push((item.3, item.4));
                    }
                }
            }
        }

        selected.into_iter().map(|(id, _)| id).collect()
    }

    pub(crate) fn beam_search_l0_locked<V: SearchVisited>(
        &self,
        query: u32,
        entry: u32,
        ef: usize,
        visited: &mut V,
    ) -> Vec<(hyperbolic::Distance, u32)> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        #[cfg(feature = "research")]
        let strategy = hyperbolic::hyperbolic_strategy();
        #[cfg(feature = "research")]
        let build_use_horiz = !matches!(
            strategy,
            hyperbolic::HyperbolicStrategy::GeodesicPruning
                | hyperbolic::HyperbolicStrategy::GeodesicPruningWithFill
                | hyperbolic::HyperbolicStrategy::GeodesicPortals
        );
        #[cfg(not(feature = "research"))]
        let build_use_horiz = true;

        #[cfg(feature = "research")]
        let use_calibrated_build = matches!(
            strategy,
            hyperbolic::HyperbolicStrategy::Full
                | hyperbolic::HyperbolicStrategy::CalibratedCompass
                | hyperbolic::HyperbolicStrategy::FixedShellCompass
                | hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
        );
        #[cfg(not(feature = "research"))]
        let use_calibrated_build = true;

        #[cfg(feature = "research")]
        let use_sign_magnitude = matches!(
            strategy,
            hyperbolic::HyperbolicStrategy::Full
                | hyperbolic::HyperbolicStrategy::FixedShellCompass
                | hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
        );
        #[cfg(not(feature = "research"))]
        let use_sign_magnitude = true;

        // 双曲模式下先通过球面纤维 S^{d-1}（并结合极坐标路由表种子）跨越原点引力势垒
        let mut horiz_seeds: Vec<u32> = Vec::new();
        if build_use_horiz && let Some(space) = self.native_space {
            let r_query = if use_calibrated_build {
                self.dimensionless_depth(space, query)
            } else {
                0.0
            };
            let sm_query = if use_sign_magnitude {
                if let Some(router) = self.polar_router
                    && let Some(m) = router.node_meta.get(query as usize)
                {
                    m.sm_params()
                } else if let Some(codes) = self.geometry_codes
                    && let Some(c) = codes.get(query as usize)
                {
                    c.sm
                } else {
                    crate::index::pelorus::engine::quantization::SmCodeParams::from_vector_and_store(
                        space.vector(query as usize),
                        self.sigs,
                        query as usize,
                    )
                    .0
                }
            } else {
                crate::index::pelorus::engine::quantization::SmCodeParams::default()
            };
            let dir_dist = |b: u32| -> u32 {
                if query == b {
                    return 0;
                }
                let shell_pen = if use_calibrated_build {
                    (0.30
                        * hyperbolic::ANGULAR_SCALE
                        * (r_query - self.dimensionless_depth(space, b)).abs())
                    .round() as u32
                } else {
                    0
                };
                #[cfg(feature = "research")]
                if matches!(
                    strategy,
                    hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                        | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
                ) {
                    return hyperbolic::cosine_to_angular_loss(
                        self.direction_store
                            .as_ref()
                            .expect("方向对照必须预编码")
                            .pair_cosine(
                                query as usize,
                                b as usize,
                                space.vector(query as usize),
                                space.vector(b as usize),
                            ),
                    )
                    .saturating_add(shell_pen);
                }
                if use_sign_magnitude {
                    if let Some(res) = self.build_residual
                        && let Some(cos_intra) =
                            res.intra_cone_cosine(query as usize, b as usize, self.dim)
                    {
                        return hyperbolic::cosine_to_angular_loss(cos_intra)
                            .saturating_add(shell_pen);
                    }
                    let counts = self.sigs.sm_counts(query as usize, b as usize, self.dim);
                    let sm_b = if let Some(router) = self.polar_router
                        && let Some(m) = router.node_meta.get(b as usize)
                    {
                        m.sm_params()
                    } else if let Some(codes) = self.geometry_codes
                        && let Some(c) = codes.get(b as usize)
                    {
                        c.sm
                    } else {
                        crate::index::pelorus::engine::quantization::SmCodeParams::from_vector_and_store(
                            space.vector(b as usize),
                            self.sigs,
                            b as usize,
                        )
                        .0
                    };
                    return hyperbolic::cosine_to_angular_loss(
                        crate::index::pelorus::engine::geometry::sm_calibrated_cosine_fast(
                            sm_query, sm_b, counts, self.dim,
                        ),
                    )
                    .saturating_add(shell_pen);
                }
                let _ = space;
                let bq_d = self.sigs.distance(query as usize, b as usize, self.dim);
                if !use_calibrated_build {
                    return bq_d;
                }
                let inv_a = self
                    .polar_router
                    .and_then(|r| {
                        r.node_meta
                            .get(query as usize)
                            .map(|m| m.legacy_inv_qnorm(self.dim))
                    })
                    .unwrap_or_else(|| {
                        hyperbolic::bq2_inv_qnorm(self.dim, self.sigs.strong_ones(query as usize))
                    });
                let inv_b = self
                    .polar_router
                    .and_then(|r| {
                        r.node_meta
                            .get(b as usize)
                            .map(|m| m.legacy_inv_qnorm(self.dim))
                    })
                    .unwrap_or_else(|| {
                        hyperbolic::bq2_inv_qnorm(self.dim, self.sigs.strong_ones(b as usize))
                    });
                hyperbolic::cosine_to_angular_loss(hyperbolic::calibrated_cosine_from_bq(
                    bq_d, self.dim, inv_a, inv_b,
                ))
                .saturating_add(shell_pen)
            };
            let ef_horiz = if use_sign_magnitude {
                (ef / 2).clamp(24, 72)
            } else if use_calibrated_build {
                (ef * 3 / 4).clamp(24, 96)
            } else {
                (ef / 2).clamp(16, 64)
            };
            visited.begin();
            let mut bq_cand: BinaryHeap<Reverse<(u32, u32)>> =
                BinaryHeap::with_capacity(ef_horiz * 2);
            let mut bq_res: BinaryHeap<(u32, u32)> = BinaryHeap::with_capacity(ef_horiz + 1);

            let push_horiz = |visited: &mut V,
                              bq_cand: &mut BinaryHeap<Reverse<(u32, u32)>>,
                              bq_res: &mut BinaryHeap<(u32, u32)>,
                              id: u32| {
                if id == query || visited.test(id as usize) {
                    return;
                }
                visited.set(id as usize);
                let d = dir_dist(id);
                if bq_res.len() < ef_horiz || d < bq_res.peek().unwrap().0 {
                    bq_cand.push(Reverse((d, id)));
                    bq_res.push((d, id));
                    if bq_res.len() > ef_horiz {
                        bq_res.pop();
                    }
                }
            };

            visited.set(query as usize);
            push_horiz(visited, &mut bq_cand, &mut bq_res, entry);
            if let (Some(router), Some(top_cones)) = (self.polar_router, self.node_top_cones) {
                if use_sign_magnitude {
                    let node_cones = top_cones.get(query as usize).copied().unwrap_or([0; 4]);
                    let mut focused = Vec::with_capacity(80);
                    router.collect_focused_build_seeds(
                        query,
                        r_query as f32,
                        &node_cones,
                        &mut focused,
                    );
                    for seed in focused {
                        push_horiz(visited, &mut bq_cand, &mut bq_res, seed);
                    }
                } else {
                    for &hub in &router.origin_hubs {
                        push_horiz(visited, &mut bq_cand, &mut bq_res, hub);
                    }
                    if router.num_cones > 0 {
                        let take_cones = if use_calibrated_build { 3 } else { 2 };
                        if let Some(cones) = top_cones.get(query as usize) {
                            for &cone_u16 in &cones[..take_cones] {
                                let base = cone_u16 as usize * hyperbolic::NUM_RADIAL_BANDS;
                                for band in 0..hyperbolic::NUM_RADIAL_BANDS {
                                    for &rep in &router.cells[base + band] {
                                        if rep == EMPTY_NB {
                                            break;
                                        }
                                        push_horiz(visited, &mut bq_cand, &mut bq_res, rep);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            while let Some(Reverse((cd, cur))) = bq_cand.pop() {
                if bq_res.len() >= ef_horiz && cd > bq_res.peek().unwrap().0 {
                    break;
                }
                let nbs = self.adj.neighbors_locked(cur, self.locks);
                for &nb in &nbs {
                    if !visited.test(nb as usize) {
                        self.prefetch_direction(nb as usize);
                    }
                }
                for nb in nbs {
                    push_horiz(visited, &mut bq_cand, &mut bq_res, nb);
                }
            }
            horiz_seeds = bq_res.into_iter().map(|(_, id)| id).collect();
        }

        visited.begin();

        let mut candidates: BinaryHeap<Reverse<(hyperbolic::Distance, u32)>> =
            BinaryHeap::with_capacity(ef * 2);
        let mut results: BinaryHeap<(hyperbolic::Distance, u32)> =
            BinaryHeap::with_capacity(ef + 1);

        let d = self.distance(query, entry);
        visited.set(entry as usize);
        candidates.push(Reverse((d, entry)));
        results.push((d, entry));

        let mut evaluated_horiz: Vec<(hyperbolic::Distance, u32)> =
            Vec::with_capacity(horiz_seeds.len());
        for &seed in &horiz_seeds {
            if seed == entry {
                evaluated_horiz.push((d, seed));
            } else if !visited.test(seed as usize) {
                visited.set(seed as usize);
                let sd = self.distance(query, seed);
                evaluated_horiz.push((sd, seed));
                candidates.push(Reverse((sd, seed)));
                results.push((sd, seed));
                if results.len() > ef {
                    results.pop();
                }
            }
        }

        while let Some(Reverse((cd, cur))) = candidates.pop() {
            if results.len() >= ef && cd > results.peek().unwrap().0 {
                break;
            }

            let nbs = self.adj.neighbors_locked(cur, self.locks);

            // 批量预取未访问邻居的签名或冷存向量
            if self.native_space.is_none() || self.quantized_build {
                for &nb in &nbs {
                    if !visited.test(nb as usize) {
                        self.prefetch_direction(nb as usize);
                    }
                }
            } else if let Some(space) = self.native_space {
                for &nb in &nbs {
                    if !visited.test(nb as usize) {
                        space.prefetch_vector(nb as usize);
                    }
                }
            }

            for nb in nbs {
                if visited.test(nb as usize) {
                    continue;
                }
                visited.set(nb as usize);

                let nd = self.distance(query, nb);
                if results.len() < ef || nd < results.peek().unwrap().0 {
                    candidates.push(Reverse((nd, nb)));
                    results.push((nd, nb));
                    if results.len() > ef {
                        results.pop();
                    }
                }
            }
        }

        let mut res: Vec<(hyperbolic::Distance, u32)> = results.into_vec();
        // 直接复用已评估的球面纤维方向种子距离，消除重复冷存计算
        res.extend_from_slice(&evaluated_horiz);
        res.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        res
    }

    pub(crate) fn connect_node_checked(&self, idx: u32, visited: &mut impl SearchVisited) {
        if self.native_space.is_none() {
            if idx == 0 {
                return;
            }
        } else if self.n <= 1 || (self.polar_router.is_none() && idx == 0) {
            return;
        }

        #[cfg(feature = "research")]
        let use_sign_magnitude = matches!(
            hyperbolic::hyperbolic_strategy(),
            hyperbolic::HyperbolicStrategy::Full
                | hyperbolic::HyperbolicStrategy::FixedShellCompass
                | hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
        );
        #[cfg(not(feature = "research"))]
        let use_sign_magnitude = true;

        let candidates_sym = self.beam_search_l0_locked(idx, self.entry_point, self.ef, visited);
        let mut candidates: Vec<(hyperbolic::Distance, u32)> = if self.native_space.is_some() {
            candidates_sym
                .into_iter()
                .filter(|&(_, id)| id != idx)
                .collect()
        } else {
            candidates_sym
                .into_iter()
                .filter(|&(_, id)| id != idx)
                .map(|(_, id)| (self.distance(idx, id), id))
                .collect()
        };

        if let (Some(router), Some(top_cones)) = (self.polar_router, self.node_top_cones) {
            let mut router_seeds = Vec::with_capacity(128);
            let node_cones = top_cones.get(idx as usize).copied().unwrap_or([0; 4]);
            if use_sign_magnitude {
                let d_self = router.node_meta.get(idx as usize).map_or(0.0, |m| m.depth);
                router.collect_focused_build_seeds(idx, d_self, &node_cones, &mut router_seeds);
            } else {
                router.collect_build_seeds(idx, &node_cones, &mut router_seeds);
            }
            for seed in router_seeds {
                candidates.push((self.distance(idx, seed), seed));
            }
        }

        let samples = if self.polar_router.is_some() {
            if use_sign_magnitude { 16 } else { 32 }
        } else {
            self.ef.min(128).max(self.m0 * 2)
        };
        let mut state = (idx as usize)
            .wrapping_mul(0x9E3779B97F4A7C15usize)
            .wrapping_add(0xBF58476D1CE4E5B9usize);
        for _ in 0..samples {
            state ^= state >> 30;
            state = state.wrapping_mul(0xBF58476D1CE4E5B9usize);
            state ^= state >> 27;
            state = state.wrapping_mul(0x94D049BB133111EBusize);
            state ^= state >> 31;
            let id = (state % self.n) as u32;
            if id != idx {
                candidates.push((self.distance(idx, id), id));
            }
        }

        candidates.sort_unstable_by_key(|&(_, id)| id);
        candidates.dedup_by_key(|&mut (_, id)| id);
        candidates.sort_unstable_by_key(|&(d, _)| d);

        if candidates.is_empty() {
            let fallback = if idx == self.entry_point {
                (idx + 1) % (self.n as u32)
            } else {
                self.entry_point
            };
            candidates.push((self.distance(idx, fallback), fallback));
        }

        let selected = self.prune(idx, &candidates);
        self.adj.set_neighbors_locked(idx, &selected, self.locks);

        for &nb in &selected {
            let guard = self.locks.lock(nb);
            let mut current = self.adj.neighbors_with_guard(&guard);
            if !current.contains(&idx) {
                current.push(idx);
            }
            let mut nb_candidates: Vec<(hyperbolic::Distance, u32)> = current
                .into_iter()
                .filter(|&n| n != nb)
                .map(|n| (self.distance(nb, n), n))
                .collect();
            nb_candidates.sort_unstable_by_key(|&(_, id)| id);
            nb_candidates.dedup_by_key(|&mut (_, id)| id);
            nb_candidates.sort_unstable_by_key(|&(d, _)| d);
            let pruned = self.prune(nb, &nb_candidates);
            self.adj.set_neighbors_with_guard(&guard, &pruned);
        }
    }

    pub(crate) fn connect_node_fast(&self, idx: u32, visited: &mut impl SearchVisited) {
        if self.native_space.is_none() {
            if idx == 0 {
                return;
            }
        } else if self.n <= 1 || (self.polar_router.is_none() && idx == 0) {
            return;
        }

        #[cfg(feature = "research")]
        let use_sign_magnitude = matches!(
            hyperbolic::hyperbolic_strategy(),
            hyperbolic::HyperbolicStrategy::Full
                | hyperbolic::HyperbolicStrategy::FixedShellCompass
                | hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
        );
        #[cfg(not(feature = "research"))]
        let use_sign_magnitude = true;

        let prof = self.profile.enabled;
        let t_beam = if prof { Some(Instant::now()) } else { None };
        let candidates_sym = self.beam_search_l0_locked(idx, self.entry_point, self.ef, visited);
        if let Some(t) = t_beam {
            self.profile
                .add(&self.profile.beam_ns, t.elapsed().as_nanos() as u64);
        }
        let t_fwd = if prof { Some(Instant::now()) } else { None };
        let mut candidates: Vec<(hyperbolic::Distance, u32)> = if self.native_space.is_some() {
            candidates_sym
                .into_iter()
                .filter(|&(_, id)| id != idx)
                .collect()
        } else {
            candidates_sym
                .into_iter()
                .filter(|&(_, id)| id != idx)
                .map(|(_, id)| (self.distance(idx, id), id))
                .collect()
        };

        if let (Some(router), Some(top_cones)) = (self.polar_router, self.node_top_cones) {
            let mut router_seeds = Vec::with_capacity(128);
            let node_cones = top_cones.get(idx as usize).copied().unwrap_or([0; 4]);
            if use_sign_magnitude {
                let d_self = router.node_meta.get(idx as usize).map_or(0.0, |m| m.depth);
                router.collect_focused_build_seeds(idx, d_self, &node_cones, &mut router_seeds);
            } else {
                router.collect_build_seeds(idx, &node_cones, &mut router_seeds);
            }
            for seed in router_seeds {
                candidates.push((self.distance(idx, seed), seed));
            }
        }

        let samples = if self.polar_router.is_some() {
            if use_sign_magnitude { 16 } else { 32 }
        } else {
            self.ef.min(128).max(self.m0 * 2)
        };
        let mut state = (idx as usize)
            .wrapping_mul(0x9E3779B97F4A7C15usize)
            .wrapping_add(0xBF58476D1CE4E5B9usize);
        for _ in 0..samples {
            state ^= state >> 30;
            state = state.wrapping_mul(0xBF58476D1CE4E5B9usize);
            state ^= state >> 27;
            state = state.wrapping_mul(0x94D049BB133111EBusize);
            state ^= state >> 31;
            let id = (state % self.n) as u32;
            if id != idx {
                candidates.push((self.distance(idx, id), id));
            }
        }

        candidates.sort_unstable_by_key(|&(_, id)| id);
        candidates.dedup_by_key(|&mut (_, id)| id);
        candidates.sort_unstable_by_key(|&(d, _)| d);

        if candidates.is_empty() {
            let fallback = if idx == self.entry_point {
                (idx + 1) % (self.n as u32)
            } else {
                self.entry_point
            };
            candidates.push((self.distance(idx, fallback), fallback));
        }

        let selected = self.prune(idx, &candidates);
        self.adj
            .set_neighbors_locked_fast(idx, &selected, self.locks);
        if let Some(t) = t_fwd {
            self.profile
                .add(&self.profile.fwd_ns, t.elapsed().as_nanos() as u64);
        }

        let cap = self.adj.capacity();
        for &nb in &selected {
            let (_guard, wait_ns) = self.locks.lock_timed(nb);
            self.profile.add(&self.profile.rev_wait_ns, wait_ns);
            let t_work = if prof { Some(Instant::now()) } else { None };

            // 惰性剪枝：邻居表未满则直接追加(O(deg))，仅当容量用尽才触发一次完整剪枝。
            // 邻接表 stride=m0*2+1 留有 2× headroom，使昂贵的 vamana_select(O(deg²))
            // 从"每条反向边都跑"降为"约每 m0 条才跑一次"，砍掉构图主成本。
            // eager=true 时恢复旧的"每条边全量剪枝"行为，用于 recall A/B 对照。
            if self.eager {
                let mut current = self.adj.neighbors_raw(nb);
                if !current.contains(&idx) {
                    current.push(idx);
                }
                let mut nb_candidates: Vec<(hyperbolic::Distance, u32)> = current
                    .into_iter()
                    .filter(|&n| n != nb)
                    .map(|n| (self.distance(nb, n), n))
                    .collect();
                nb_candidates.sort_unstable_by_key(|&(_, id)| id);
                nb_candidates.dedup_by_key(|&mut (_, id)| id);
                nb_candidates.sort_unstable_by_key(|&(d, _)| d);
                let pruned = self.prune(nb, &nb_candidates);
                self.adj.set_neighbors_raw(nb, &pruned);
            } else if !self.adj.contains_raw(nb, idx) {
                if self.adj.degree_raw(nb) < cap {
                    self.adj.push_neighbor_raw(nb, idx);
                } else {
                    let mut current = self.adj.neighbors_raw(nb);
                    current.push(idx);
                    let mut nb_candidates: Vec<(hyperbolic::Distance, u32)> = current
                        .into_iter()
                        .filter(|&n| n != nb)
                        .map(|n| (self.distance(nb, n), n))
                        .collect();
                    nb_candidates.sort_unstable_by_key(|&(_, id)| id);
                    nb_candidates.dedup_by_key(|&mut (_, id)| id);
                    nb_candidates.sort_unstable_by_key(|&(d, _)| d);
                    let pruned = self.prune(nb, &nb_candidates);
                    self.adj.set_neighbors_raw(nb, &pruned);
                }
            }

            if let Some(t) = t_work {
                self.profile
                    .add(&self.profile.rev_work_ns, t.elapsed().as_nanos() as u64);
            }
        }
        self.profile.nodes.fetch_add(1, Ordering::Relaxed);
    }

    /// 构图后 final-prune：把惰性追加导致 deg>m0 的节点并行收敛到 m0。
    /// 每节点最多剪一次，O(n) 单次剪枝，相对原"每边重剪"成本可忽略。
    pub(crate) fn final_prune(&self, node: u32) {
        let deg = self.adj.degree_raw(node);
        if deg <= self.m0 {
            return;
        }
        let current = self.adj.neighbors_raw(node);
        let mut cand: Vec<(hyperbolic::Distance, u32)> = current
            .into_iter()
            .filter(|&n| n != node)
            .map(|n| (self.distance(node, n), n))
            .collect();
        cand.sort_unstable_by_key(|&(_, id)| id);
        cand.dedup_by_key(|&mut (_, id)| id);
        cand.sort_unstable_by_key(|&(d, _)| d);
        let pruned = self.prune(node, &cand);
        self.adj.set_neighbors_raw(node, &pruned);
    }
}
