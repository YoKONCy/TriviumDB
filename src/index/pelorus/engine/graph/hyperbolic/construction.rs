//! 双曲剪枝、插入与连通维护。

use super::*;
impl QuIVer {
    pub(crate) fn native_node_proxy(&self, space: &NativeSpace<'_>, left: u32, right: u32) -> f64 {
        if left == right {
            return 0.0;
        }
        if !self.geometry_codes.is_empty() {
            #[cfg(feature = "research")]
            if !matches!(
                hyperbolic_strategy(),
                HyperbolicStrategy::Full
                    | HyperbolicStrategy::FixedShellCompass
                    | HyperbolicStrategy::ScalarQuantizedDirection
                    | HyperbolicStrategy::FullPrecisionDirection
            ) {
                return self.geometry_codes[left as usize].proxy(
                    self.geometry_codes[right as usize],
                    self.bq_store
                        .distance(left as usize, right as usize, self.dim),
                    self.dim,
                );
            }
            return self.geometry_codes[left as usize].proxy_sm(
                self.geometry_codes[right as usize],
                self.bq_store
                    .sm_counts(left as usize, right as usize, self.dim),
                self.dim,
            );
        }
        if self.quantized_native
            && let Some(router) = &self.polar_router
            && let (Some(&ml), Some(&mr)) = (
                router.node_meta(left as usize),
                router.node_meta(right as usize),
            )
        {
            let cos_hat = if !ml.has_residual_cone() && !mr.has_residual_cone() {
                let counts = self
                    .bq_store
                    .sm_counts(left as usize, right as usize, self.dim);
                sm_calibrated_cosine_fast(ml.sm_params(), mr.sm_params(), counts, self.dim)
            } else if ml.has_residual_cone()
                && mr.has_residual_cone()
                && ml.cone_id() == mr.cone_id()
            {
                // 同切锥节点：利用正交残差恒等式高精度重构单位余弦
                let al = f64::from(ml.cone_alpha);
                let ar = f64::from(mr.cone_alpha);
                let bl = (1.0 - al * al).max(0.0).sqrt();
                let br = (1.0 - ar * ar).max(0.0).sqrt();
                let cos_res =
                    if ml.sm_sigma <= 0.0 || mr.sm_sigma <= 0.0 || bl <= 1e-8 || br <= 1e-8 {
                        0.0
                    } else {
                        let counts =
                            self.bq_store
                                .sm_counts(left as usize, right as usize, self.dim);
                        sm_calibrated_cosine_fast(ml.sm_params(), mr.sm_params(), counts, self.dim)
                    };
                (al * ar + bl * br * cos_res).clamp(-1.0, 1.0)
            } else {
                // 增量插入跨锥剪枝安全回退：从原始向量计算精确单位余弦，防止混用不同锥正交基
                let vl = space.vector(self.slot_indices[left as usize]);
                let vr = space.vector(self.slot_indices[right as usize]);
                let rl = f64::from(ml.radius);
                let rr = f64::from(mr.radius);
                if rl > 1e-12 && rr > 1e-12 {
                    (dot_f32_f64(vl, vr) / (rl * rr)).clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            };
            return ml.sm_poincare_proxy(
                f64::from(mr.radius),
                f64::from(mr.boundary_factor),
                cos_hat,
            );
        }
        let slot = self.slot_indices[left as usize];
        space.proxy(
            space.vector(slot),
            space.boundary_factors[slot],
            self.slot_indices[right as usize],
        )
    }

    #[inline]
    pub(crate) fn native_dimensionless_depth(&self, space: &NativeSpace<'_>, node: u32) -> f64 {
        if let Some(router) = &self.polar_router
            && let Some(meta) = router.node_meta(node as usize)
        {
            return f64::from(meta.depth);
        }
        dimensionless_depth(space.boundary_factors[self.slot_indices[node as usize]])
    }

    #[inline]
    pub(crate) fn native_angular_loss(
        &self,
        space: &NativeSpace<'_>,
        left: u32,
        right: u32,
    ) -> u32 {
        if left == right {
            return 0;
        }
        #[cfg(feature = "research")]
        {
            let mode = hyperbolic_strategy();
            if matches!(
                mode,
                HyperbolicStrategy::ScalarQuantizedDirection
                    | HyperbolicStrategy::FullPrecisionDirection
            ) {
                return directional_distance_slices(
                    space.vector(self.slot_indices[left as usize]),
                    space.vector(self.slot_indices[right as usize]),
                    mode,
                );
            }
            if mode == HyperbolicStrategy::CalibratedCompass {
                let bq_d = self
                    .bq_store
                    .distance(left as usize, right as usize, self.dim);
                let inv_l = self
                    .polar_router
                    .as_ref()
                    .and_then(|r| {
                        r.node_meta(left as usize)
                            .map(|m| m.legacy_inv_qnorm(self.dim))
                    })
                    .unwrap_or_else(|| {
                        bq2_inv_qnorm(self.dim, self.bq_store.strong_ones(left as usize))
                    });
                let inv_r = self
                    .polar_router
                    .as_ref()
                    .and_then(|r| {
                        r.node_meta(right as usize)
                            .map(|m| m.legacy_inv_qnorm(self.dim))
                    })
                    .unwrap_or_else(|| {
                        bq2_inv_qnorm(self.dim, self.bq_store.strong_ones(right as usize))
                    });
                return cosine_to_angular_loss(calibrated_cosine_from_bq(
                    bq_d, self.dim, inv_l, inv_r,
                ));
            }
        }
        if let Some(router) = &self.polar_router
            && let (Some(&ml), Some(&mr)) = (
                router.node_meta(left as usize),
                router.node_meta(right as usize),
            )
            && (ml.has_residual_cone() || mr.has_residual_cone())
        {
            let cos_hat = if ml.has_residual_cone()
                && mr.has_residual_cone()
                && ml.cone_id() == mr.cone_id()
            {
                let al = f64::from(ml.cone_alpha);
                let ar = f64::from(mr.cone_alpha);
                let bl = (1.0 - al * al).max(0.0).sqrt();
                let br = (1.0 - ar * ar).max(0.0).sqrt();
                let cos_res =
                    if ml.sm_sigma <= 0.0 || mr.sm_sigma <= 0.0 || bl <= 1e-8 || br <= 1e-8 {
                        0.0
                    } else {
                        let counts =
                            self.bq_store
                                .sm_counts(left as usize, right as usize, self.dim);
                        sm_calibrated_cosine_fast(ml.sm_params(), mr.sm_params(), counts, self.dim)
                    };
                (al * ar + bl * br * cos_res).clamp(-1.0, 1.0)
            } else {
                let vl = space.vector(self.slot_indices[left as usize]);
                let vr = space.vector(self.slot_indices[right as usize]);
                let rl = f64::from(ml.radius);
                let rr = f64::from(mr.radius);
                if rl > 1e-12 && rr > 1e-12 {
                    (dot_f32_f64(vl, vr) / (rl * rr)).clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            };
            return cosine_to_angular_loss(cos_hat);
        }
        let counts = self
            .bq_store
            .sm_counts(left as usize, right as usize, self.dim);
        let sm_l = if let Some(router) = &self.polar_router
            && let Some(m) = router.node_meta(left as usize)
        {
            m.sm_params()
        } else if let Some(c) = self.geometry_codes.get(left as usize) {
            c.sm
        } else {
            SmCodeParams::from_vector_and_store(
                space.vector(self.slot_indices[left as usize]),
                &self.bq_store,
                left as usize,
            )
            .0
        };
        let sm_r = if let Some(router) = &self.polar_router
            && let Some(m) = router.node_meta(right as usize)
        {
            m.sm_params()
        } else if let Some(c) = self.geometry_codes.get(right as usize) {
            c.sm
        } else {
            SmCodeParams::from_vector_and_store(
                space.vector(self.slot_indices[right as usize]),
                &self.bq_store,
                right as usize,
            )
            .0
        };
        cosine_to_angular_loss(sm_calibrated_cosine_fast(sm_l, sm_r, counts, self.dim))
    }

    /// 深度与壳厚自适应四向黎曼罗盘剪枝（增量插入路径复用）：
    /// 协同分配平行移动测地纤维、等深极限球面环、向内测地升降与向外测地升降槽位。
    pub(crate) fn native_prune(
        &self,
        space: &NativeSpace<'_>,
        target: u32,
        candidates: &[Candidate],
        limit: usize,
    ) -> Vec<u32> {
        let scale_c = space.curvature.sqrt();
        let r_target = self.native_dimensionless_depth(space, target);
        let (geo_limit, iso_tol, elev_step, elev_min, elev_dom, div_tol) =
            (((limit * 3) / 8).max(1), 0.60, 0.65, 0.18, 0.10, 1.0);
        let mut selected: Vec<(u32, f64)> = Vec::with_capacity(limit);

        // 1. 平行移动测地纤维配额
        for &(distance, candidate) in candidates {
            if candidate == target || selected.len() >= geo_limit {
                continue;
            }
            let r_cand = self.native_dimensionless_depth(space, candidate);
            let target_distance = poincare_distance(distance.0, space.curvature) * scale_c;
            let ang_target_cand = self.native_angular_loss(space, target, candidate);
            let dominated = selected.iter().any(|&(neighbor, r_nb)| {
                let ang_nb_cand = self.native_angular_loss(space, candidate, neighbor);
                if ang_nb_cand > ang_target_cand {
                    return false;
                }
                let stem_excess = ((r_target - r_cand).abs() - (r_nb - r_cand).abs()).max(0.0);
                let effective_target = (target_distance - stem_excess).max(0.0);
                let alpha = f64::from(self.alpha);
                if alpha * (r_nb - r_cand).abs() > effective_target {
                    return false;
                }
                let proxy = self.native_node_proxy(space, candidate, neighbor);
                alpha * poincare_distance(proxy, space.curvature) * scale_c <= effective_target
            });
            if !dominated {
                selected.push((candidate, r_cand));
            }
        }

        if selected.len() < limit {
            let mut pool: Vec<(u32, Distance, u32, f64)> = Vec::with_capacity(candidates.len());
            for &(distance, candidate) in candidates {
                if candidate == target || selected.iter().any(|&(s, _)| s == candidate) {
                    continue;
                }
                let r_cand = self.native_dimensionless_depth(space, candidate);
                let ang_d = self.native_angular_loss(space, target, candidate);
                pool.push((ang_d, distance, candidate, r_cand));
            }

            // 2. 等深极限球面环配额
            let iso_target = ((limit * 7) / 8).max(selected.len()).min(limit);
            pool.sort_unstable_by_key(|a| {
                let sa = a
                    .0
                    .saturating_add((0.85 * ANGULAR_SCALE * (r_target - a.3).abs()).round() as u32);
                (sa, a.1, a.2)
            });
            for &(ang_target_cand, _, candidate, r_cand) in &pool {
                if selected.len() >= iso_target {
                    break;
                }
                let dominated = selected.iter().any(|&(neighbor, r_nb)| {
                    if (r_nb - r_cand).abs() > iso_tol {
                        return false;
                    }
                    let ang_nb_cand = self.native_angular_loss(space, candidate, neighbor);
                    self.alpha * (ang_nb_cand as f32) <= ang_target_cand as f32
                });
                if !dominated {
                    selected.push((candidate, r_cand));
                }
            }

            // 3. 向内测地升降机与向外测地升降机
            let rem = limit.saturating_sub(selected.len());
            let inward_target = selected.len() + rem / 2;
            pool.sort_unstable_by_key(|a| {
                let step_a = ((r_target - a.3) - elev_step).abs();
                let sa =
                    a.0.saturating_add((0.15 * ANGULAR_SCALE * step_a).round() as u32);
                (sa, a.1)
            });
            for &(ang_target_cand, _, candidate, r_cand) in &pool {
                if selected.len() >= inward_target {
                    break;
                }
                if r_cand >= r_target - elev_min || selected.iter().any(|&(s, _)| s == candidate) {
                    continue;
                }
                let dominated = selected.iter().any(|&(neighbor, r_nb)| {
                    r_nb < r_target - elev_dom
                        && self.alpha
                            * (self.native_angular_loss(space, candidate, neighbor) as f32)
                            <= ang_target_cand as f32
                });
                if !dominated {
                    selected.push((candidate, r_cand));
                }
            }

            pool.sort_unstable_by_key(|a| {
                let step_a = ((a.3 - r_target) - elev_step).abs();
                let sa =
                    a.0.saturating_add((0.15 * ANGULAR_SCALE * step_a).round() as u32);
                (sa, a.1)
            });
            for &(ang_target_cand, _, candidate, r_cand) in &pool {
                if selected.len() >= limit {
                    break;
                }
                if r_cand <= r_target + elev_min || selected.iter().any(|&(s, _)| s == candidate) {
                    continue;
                }
                let dominated = selected.iter().any(|&(neighbor, r_nb)| {
                    r_nb > r_target + elev_dom
                        && self.alpha
                            * (self.native_angular_loss(space, candidate, neighbor) as f32)
                            <= ang_target_cand as f32
                });
                if !dominated {
                    selected.push((candidate, r_cand));
                }
            }

            // 4. 角度多样性与保底补满至 limit
            if selected.len() < limit {
                pool.sort_unstable_by_key(|a| {
                    let sa = a.0.saturating_add(
                        (0.25 * ANGULAR_SCALE * (r_target - a.3).abs()).round() as u32,
                    );
                    (sa, a.1)
                });
                for &(ang_target_cand, _, candidate, r_cand) in &pool {
                    if selected.len() >= limit {
                        break;
                    }
                    if selected.iter().any(|&(s, _)| s == candidate) {
                        continue;
                    }
                    let dominated = selected.iter().any(|&(neighbor, r_nb)| {
                        (r_nb - r_cand).abs() <= div_tol
                            && self.alpha
                                * (self.native_angular_loss(space, candidate, neighbor) as f32)
                                <= ang_target_cand as f32
                    });
                    if !dominated {
                        selected.push((candidate, r_cand));
                    }
                }
                for &(_, _, candidate, r_cand) in &pool {
                    if selected.len() >= limit {
                        break;
                    }
                    if !selected.iter().any(|&(s, _)| s == candidate) {
                        selected.push((candidate, r_cand));
                    }
                }
            }
        }
        selected.into_iter().map(|(id, _)| id).collect()
    }

    pub(crate) fn insert_native(
        &mut self,
        space: &NativeSpace<'_>,
        id: u64,
        slot: usize,
        seed: &mut u64,
        quantized: bool,
    ) {
        #[cfg(feature = "research")]
        {
            self.direction_store = None;
        }
        self.quantized_native = quantized;
        let node = self.n as u32;
        let query = space.vector(slot);
        let factor = space.boundary_factors[slot];
        let level = self.random_level(seed);
        let signature = Bq2Signature::from_vector(query);
        if quantized && (self.polar_router.is_none() || !self.geometry_codes.is_empty()) {
            self.geometry_codes
                .push(GeometryCode::new(query, factor, &signature));
        }
        self.bq_store.push_sig(&signature);
        self.ids.push(id);
        self.slot_indices.push(slot);
        self.id_to_internal.insert(id, node);
        self.tombstones.push(false);
        self.node_max_layer.push(level as u8);
        self.layer0.push_empty();
        while self.upper_layers.len() < level {
            self.upper_layers.push(vec![Vec::new(); self.n]);
        }
        for upper in &mut self.upper_layers {
            upper.push(Vec::new());
        }
        self.n += 1;
        if let Some(router) = &mut self.polar_router {
            router.register_incremental(query, &self.bq_store, self.dim, node, factor);
        }
        if node == 0 {
            self.entry_point = node;
            self.max_level = level;
            return;
        }
        let mut entry = self.entry_point;
        for layer in ((level + 1)..=self.max_level).rev() {
            entry = self.native_greedy(space, query, factor, entry, layer, quantized);
        }
        for layer in (0..=level.min(self.max_level)).rev() {
            let candidates = self.native_beam(
                space,
                query,
                factor,
                entry,
                layer,
                self.ef_construction,
                true,
                quantized,
            );
            let limit = if layer == 0 { self.m0 } else { self.m };
            let selected = self.native_prune(space, node, &candidates, limit);
            self.set_native_neighbors(node, layer, &selected);
            for neighbor in selected {
                let mut reverse = self.native_neighbors(neighbor, layer).to_vec();
                if !reverse.contains(&node) {
                    reverse.push(node);
                }
                if reverse.len() <= limit {
                    self.set_native_neighbors(neighbor, layer, &reverse);
                    continue;
                }
                let mut reverse = reverse
                    .into_iter()
                    .map(|candidate| {
                        (
                            Distance(self.native_node_proxy(space, neighbor, candidate)),
                            candidate,
                        )
                    })
                    .collect::<Vec<_>>();
                reverse.sort_unstable();
                let pruned = self.native_prune(space, neighbor, &reverse, limit);
                self.set_native_neighbors(neighbor, layer, &pruned);
            }
            if let Some(first) = candidates.first() {
                entry = first.1;
            }
        }
        if level > self.max_level {
            self.entry_point = node;
            self.max_level = level;
        }
    }
}
