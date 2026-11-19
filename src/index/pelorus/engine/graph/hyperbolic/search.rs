//! 双曲导航评分与精确精排。

use super::*;
impl QuIVer {
    pub(crate) fn native_greedy(
        &self,
        space: &NativeSpace<'_>,
        query: &[f32],
        factor: f64,
        mut entry: u32,
        layer: usize,
        quantized: bool,
    ) -> u32 {
        let encoded = quantized.then(|| {
            let signature = Bq2Signature::from_vector(query);
            let code = GeometryCode::new(query, factor, &signature);
            (signature, code)
        });
        let score = |node| self.native_query_proxy(space, query, factor, encoded.as_ref(), node);
        if !space.visit() {
            return entry;
        }
        let mut best = score(entry);
        loop {
            let previous = entry;
            for &neighbor in self.native_neighbors(previous, layer) {
                if !space.visit() {
                    return entry;
                }
                let distance = score(neighbor);
                if distance < best {
                    best = distance;
                    entry = neighbor;
                }
            }
            if entry == previous {
                return entry;
            }
        }
    }

    /// 第三代 SM-Poincaré 快速单节点代理评估：返回 `(sm_poincare_proxy, cos_hat, node_depth)`。
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub(crate) fn eval_sm_node_fast(
        &self,
        space: &NativeSpace<'_>,
        query: &[f32],
        q_sig: &Bq2Signature,
        q_sm: SmCodeParams,
        q_radius: f64,
        q_factor: f64,
        node: u32,
        cone_cache: Option<&mut QueryConeCache>,
        #[cfg(feature = "research")] strategy: HyperbolicStrategy,
        trace_slow: bool,
    ) -> (f64, f64, f32) {
        #[cfg(feature = "research")]
        if trace_slow {
            trace_query(|t| {
                t.scored.insert(node);
                t.approximate_evals += 1;
            });
            if (EXECUTION_BITS.load(AtomicOrdering::Relaxed) & 1) == 0 {
                std::hint::black_box(hyperbolic_strategy());
            }
        }
        #[cfg(not(feature = "research"))]
        let _ = trace_slow;

        let idx = node as usize;
        if let Some(router) = &self.polar_router
            && let Some(&meta) = router.node_meta(idx)
        {
            #[cfg(feature = "research")]
            if matches!(
                strategy,
                HyperbolicStrategy::ScalarQuantizedDirection
                    | HyperbolicStrategy::FullPrecisionDirection
            ) {
                let cos_v = if self.dirty_count == 0
                    && let Some(store) = &self.direction_store
                {
                    store.query_cosine(idx, space.vector(self.slot_indices[idx]))
                } else {
                    directional_cosine_slices(query, space.vector(self.slot_indices[idx]), strategy)
                };
                return (
                    meta.sm_poincare_proxy(q_radius, q_factor, cos_v),
                    cos_v,
                    meta.depth,
                );
            }
            let _ = space;
            let cos_hat = if let Some(cache) = cone_cache {
                cache.eval_sym_cosine(
                    router.residual_dict.as_ref(),
                    q_sig,
                    q_sm,
                    self.bq_store.pos_slice(idx).as_ptr(),
                    self.bq_store.strong_slice(idx).as_ptr(),
                    meta,
                    self.dim,
                )
            } else if meta.has_residual_cone()
                && let Some(dict) = router.residual_dict.as_ref()
                && (meta.cone_id() as usize) < dict.num_axes
                && q_radius > 1e-12
            {
                // 高层贪心无 Scratch 缓存回退：直接按正交分解评估
                let axis = dict.axis(meta.cone_id() as usize);
                let alpha_q = (dot_f32_f64(query, axis) / q_radius).clamp(-1.0, 1.0);
                let alpha_v = f64::from(meta.cone_alpha);
                let beta_v = (1.0 - alpha_v * alpha_v).max(0.0).sqrt();
                let counts = self.bq_store.sm_counts_to_sig(idx, q_sig, self.dim);
                let cos_qr = sm_calibrated_cosine_fast(q_sm, meta.sm_params(), counts, self.dim);
                (alpha_q * alpha_v + beta_v * cos_qr).clamp(-1.0, 1.0)
            } else {
                let counts = self.bq_store.sm_counts_to_sig(idx, q_sig, self.dim);
                sm_calibrated_cosine_fast(q_sm, meta.sm_params(), counts, self.dim)
            };
            return (
                meta.sm_poincare_proxy(q_radius, q_factor, cos_hat),
                cos_hat,
                meta.depth,
            );
        }
        // 无路由表小图或反序列化裸图回退路径
        let slot = self.slot_indices[idx];
        let factor_v = space.boundary_factors[slot];
        let depth_v = dimensionless_depth(factor_v) as f32;
        #[cfg(feature = "research")]
        if self.dirty_count == 0
            && let Some(store) = &self.direction_store
        {
            let radius = norm_squared_f64(space.vector(slot)).sqrt();
            let cosine = store.query_cosine(idx, space.vector(slot));
            let delta = q_radius - radius;
            return (
                (delta * delta + 2.0 * q_radius * radius * (1.0 - cosine).max(0.0))
                    * q_factor
                    * factor_v,
                cosine,
                depth_v,
            );
        }
        if let Some(&code) = self.geometry_codes.get(idx) {
            let counts = self.bq_store.sm_counts_to_sig(idx, q_sig, self.dim);
            let cos_hat = sm_calibrated_cosine_fast(q_sm, code.sm, counts, self.dim);
            let dr = q_radius - code.radius;
            let angular = (q_radius * code.radius * (1.0 - cos_hat)).max(0.0);
            let proxy = (dr * dr + 2.0 * angular) * q_factor * code.boundary_factor;
            return (proxy, cos_hat, depth_v);
        }
        let (sm_v, r_v) =
            SmCodeParams::from_vector_and_store(space.vector(slot), &self.bq_store, idx);
        let counts = self.bq_store.sm_counts_to_sig(idx, q_sig, self.dim);
        let cos_hat = sm_calibrated_cosine_fast(q_sm, sm_v, counts, self.dim);
        let dr = q_radius - r_v;
        let angular = (q_radius * r_v * (1.0 - cos_hat)).max(0.0);
        (
            (dr * dr + 2.0 * angular) * q_factor * factor_v,
            cos_hat,
            depth_v,
        )
    }

    /// 第四代 Stage-0.5 非对称校准 SM-Poincaré 过滤器精筛：仅作用于有界候选池，零冷存读取、零开方。
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub(crate) fn eval_asym_sm_proxy_fast(
        &self,
        space: &NativeSpace<'_>,
        query: &[f32],
        asym_q: &AsymQuerySm,
        q_radius: f64,
        q_factor: f64,
        node: u32,
        cone_cache: &mut QueryConeCache,
        #[cfg(feature = "research")] strategy: HyperbolicStrategy,
        trace_slow: bool,
    ) -> f64 {
        #[cfg(feature = "research")]
        if trace_slow {
            trace_query(|t| {
                t.scored.insert(node);
                t.approximate_evals += 1;
            });
        }
        #[cfg(not(feature = "research"))]
        let _ = trace_slow;

        let idx = node as usize;
        let _ = query;
        #[cfg(feature = "research")]
        if matches!(
            strategy,
            HyperbolicStrategy::ScalarQuantizedDirection
                | HyperbolicStrategy::FullPrecisionDirection
        ) {
            let slot = self.slot_indices[idx];
            let cosine = if self.dirty_count == 0
                && let Some(store) = &self.direction_store
            {
                store.query_cosine(idx, space.vector(slot))
            } else {
                directional_cosine_slices(query, space.vector(slot), strategy)
            };
            if let Some(router) = &self.polar_router
                && let Some(meta) = router.node_meta(idx)
            {
                return meta.sm_poincare_proxy(q_radius, q_factor, cosine);
            }
            let radius = norm_squared_f64(space.vector(slot)).sqrt();
            let delta = q_radius - radius;
            return (delta * delta + 2.0 * q_radius * radius * (1.0 - cosine).max(0.0))
                * q_factor
                * space.boundary_factors[slot];
        }
        if let Some(router) = &self.polar_router
            && let Some(&meta) = router.node_meta(idx)
        {
            let cos_hat = cone_cache.eval_asym_cosine(
                router.residual_dict.as_ref(),
                asym_q,
                self.bq_store.pos_slice(idx),
                self.bq_store.strong_slice(idx),
                meta,
            );
            return meta.sm_poincare_proxy(q_radius, q_factor, cos_hat);
        }
        if let Some(&code) = self.geometry_codes.get(idx) {
            let cos_hat = asym_q.calibrated_cosine(&self.bq_store, idx, code.sm);
            let dr = q_radius - code.radius;
            let angular = (q_radius * code.radius * (1.0 - cos_hat)).max(0.0);
            return (dr * dr + 2.0 * angular) * q_factor * code.boundary_factor;
        }
        let slot = self.slot_indices[idx];
        let factor_v = space.boundary_factors[slot];
        let (sm_v, r_v) =
            SmCodeParams::from_vector_and_store(space.vector(slot), &self.bq_store, idx);
        let cos_hat = asym_q.calibrated_cosine(&self.bq_store, idx, sm_v);
        let dr = q_radius - r_v;
        let angular = (q_radius * r_v * (1.0 - cos_hat)).max(0.0);
        (dr * dr + 2.0 * angular) * q_factor * factor_v
    }

    #[inline]
    pub(crate) fn native_query_proxy(
        &self,
        space: &NativeSpace<'_>,
        query: &[f32],
        factor: f64,
        encoded: Option<&(Bq2Signature, GeometryCode)>,
        node: u32,
    ) -> f64 {
        #[cfg(feature = "research")]
        trace_query(|t| {
            t.scored.insert(node);
        });
        match encoded {
            Some((signature, code)) => {
                if let Some(&node_code) = self.geometry_codes.get(node as usize) {
                    #[cfg(feature = "research")]
                    trace_query(|t| t.approximate_evals += 1);
                    #[cfg(feature = "research")]
                    if !matches!(
                        hyperbolic_strategy(),
                        HyperbolicStrategy::Full
                            | HyperbolicStrategy::FixedShellCompass
                            | HyperbolicStrategy::ScalarQuantizedDirection
                            | HyperbolicStrategy::FullPrecisionDirection
                    ) {
                        return code.proxy(
                            node_code,
                            self.bq_store
                                .distance_to_sig(node as usize, signature, self.dim),
                            self.dim,
                        );
                    }
                    return code.proxy_sm(
                        node_code,
                        self.bq_store
                            .sm_counts_to_sig(node as usize, signature, self.dim),
                        self.dim,
                    );
                }
                #[cfg(feature = "research")]
                let trace_slow = TRACE_ACTIVE.load(AtomicOrdering::Relaxed)
                    || !execution_config().zero_trace_overhead;
                #[cfg(not(feature = "research"))]
                let trace_slow = false;
                self.eval_sm_node_fast(
                    space,
                    query,
                    signature,
                    code.sm,
                    code.radius,
                    factor,
                    node,
                    None,
                    #[cfg(feature = "research")]
                    hyperbolic_strategy(),
                    trace_slow,
                )
                .0
            }
            None => space.proxy(query, factor, self.slot_indices[node as usize]),
        }
    }

    pub(crate) fn search_native(
        &self,
        space: &NativeSpace<'_>,
        query: &[f32],
        factor: f64,
        config: &SearchConfig,
        quantized: bool,
        eligible: &mut dyn FnMut(u64) -> bool,
        ssd: Option<&crate::index::pelorus::io::PelorusSsd>,
    ) -> crate::index::pelorus::engine::Result<Vec<(u64, f64)>> {
        #[cfg(feature = "research")]
        let speed_cfg = execution_config();
        #[cfg(feature = "research")]
        let trace_slow =
            TRACE_ACTIVE.load(AtomicOrdering::Relaxed) || !speed_cfg.zero_trace_overhead;
        #[cfg(not(feature = "research"))]
        let trace_slow = false;
        #[cfg(feature = "research")]
        let fast_kernels = speed_cfg.fast_kernels;
        #[cfg(not(feature = "research"))]
        let fast_kernels = true;
        #[cfg(feature = "research")]
        let radial_bound_on = geometry_components().radial_bound;
        #[cfg(not(feature = "research"))]
        let radial_bound_on = true;

        #[cfg(feature = "research")]
        if self.dirty_count == 0
            && let Some(store) = &self.direction_store
        {
            store.prepare_query(query);
        }
        if fast_kernels && quantized {
            SCRATCH.with(|c| c.borrow_mut().last_query_radius = -1.0);
        }
        #[cfg(feature = "research")]
        let stage_prof = STAGE_PROFILE_ACTIVE.load(AtomicOrdering::Relaxed);
        #[cfg(feature = "research")]
        let greedy_start = stage_prof.then(std::time::Instant::now);
        let mut entry = self.entry_point;
        for layer in (1..=self.max_level).rev() {
            entry = self.native_greedy(space, query, factor, entry, layer, quantized);
            space.checkpoint()?;
        }
        #[cfg(feature = "research")]
        if let Some(start) = greedy_start {
            STAGE_PROFILE.with(|p| {
                p.borrow_mut().upper_greedy_ns = start.elapsed().as_nanos() as u64;
            });
        }
        let mut candidates = self.native_beam(
            space,
            query,
            factor,
            entry,
            0,
            config.ef_search,
            false,
            quantized,
        );
        space.checkpoint()?;
        #[cfg(feature = "research")]
        let select_start = stage_prof.then(std::time::Instant::now);
        if !quantized {
            candidates.sort_unstable_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| self.ids[a.1 as usize].cmp(&self.ids[b.1 as usize]))
            });
        }
        // 不符合谓词的节点仍可用于导航，但不能占用精排名额。
        candidates.retain(|&(_, node)| eligible(self.ids[node as usize]));
        // 方案 A：共形雅可比量化不确定度门控（Conformal Jacobian Quantization-Uncertainty Gate）
        // 基于非对称 2-bit SM 余弦估计标准差 σ_asym(D) = sqrt(0.221 / D) 与庞加莱共形雅可比
        // |∂P/∂c| = 2 * r_q * r_v * f_q * f_v，闭式导出第 k 名的量化代理扰动标准差 σ_P；
        // 当节点归属切锥正交残差字典时，按查询-主轴投影精确传播一阶误差系数 β_{q,c} * β_v（远端未物化切锥为 β_v）。
        // 仅当第 B 名与第 k 名的代理差距未越过 z * σ_P 时按置信带扩容，正常流形与收敛残差锥零冗余精排。
        let base_limit = candidates.len().min(config.rerank_limit());
        #[cfg(feature = "research")]
        let eff_limit = {
            let recall_options = recall_config();
            if quantized
                && recall_options.adaptive_rerank
                && config.rerank_limit.is_some()
                && base_limit > 0
                && base_limit < candidates.len()
            {
                let k_ref = config.top_k.saturating_sub(1).min(base_limit - 1);
                let ref_node = candidates[k_ref].1 as usize;
                let p_ref = candidates[k_ref].0.0;
                let p_base = candidates[base_limit - 1].0.0;
                let sigma_asym = (0.221 / (self.dim.max(1) as f64)).sqrt();
                let sigma_p = if let Some(router) = self.polar_router.as_ref()
                    && let Some(&meta) = router.node_meta(ref_node)
                {
                    let (q_rad, res_atten) = SCRATCH.with(|c| {
                        let s = c.borrow();
                        let q_r = if s.last_query_radius >= 0.0 {
                            s.last_query_radius
                        } else {
                            norm_squared_f64(query).sqrt()
                        };
                        let atten = s.cone_cache.query_residual_atten(
                            router.residual_dict.as_ref(),
                            query,
                            q_r,
                            meta,
                        );
                        (q_r, atten)
                    });
                    2.0 * q_rad
                        * f64::from(meta.radius)
                        * factor
                        * f64::from(meta.boundary_factor)
                        * sigma_asym
                        * res_atten
                } else {
                    p_ref.max(1e-9) * f64::from(recall_options.plateau_rel_gap)
                };
                let z_gate =
                    (f64::from(recall_options.plateau_rel_gap) * (2.0 / 0.22)).clamp(0.5, 4.5);
                if (p_base - p_ref) <= z_gate * sigma_p {
                    let hard_cap = candidates
                        .len()
                        .min(usize::from(recall_options.adaptive_rerank_cap).max(base_limit));
                    let bound_hi = p_ref + (z_gate + 0.8) * sigma_p;
                    let mut lim = base_limit;
                    while lim < hard_cap && candidates[lim].0.0 <= bound_hi {
                        lim += 1;
                    }
                    lim
                } else {
                    base_limit
                }
            } else {
                base_limit
            }
        };
        #[cfg(not(feature = "research"))]
        let eff_limit = base_limit;

        #[cfg(feature = "research")]
        if let Some(start) = select_start {
            STAGE_PROFILE.with(|p| {
                let mut p = p.borrow_mut();
                p.candidate_select_ns = start.elapsed().as_nanos() as u64;
                p.rerank_selected_count = eff_limit;
            });
        }

        #[cfg(feature = "research")]
        if trace_slow {
            trace_query(|t| {
                t.beam.extend(candidates.iter().map(|c| c.1));
                t.selected
                    .extend(candidates.iter().take(eff_limit).map(|c| c.1));
            });
        }
        if !quantized && space.rerank.is_none() {
            return candidates
                .into_iter()
                .take(eff_limit)
                .take(config.top_k)
                .map(|(distance, id)| {
                    space.charge_score()?;
                    Ok((
                        self.ids[id as usize],
                        poincare_distance(distance.0, space.curvature),
                    ))
                })
                .collect();
        }
        // 量化导航精排：优先使用 PolarRouter 32B 热元数据（或 GeometryCode）执行精确径向 Cauchy-Schwarz 下界预滤，
        // 并批量预取通过径向检验的冷存向量。
        #[cfg(feature = "research")]
        let t_rerank_start = stage_prof.then(std::time::Instant::now);
        #[cfg(feature = "research")]
        let mut score_ns = 0u64;
        #[cfg(feature = "research")]
        let mut scored = 0usize;
        #[cfg(feature = "research")]
        let mut radial_rejections = 0usize;
        let query_radius = if fast_kernels {
            let cached = SCRATCH.with(|c| c.borrow().last_query_radius);
            if cached >= 0.0 {
                cached
            } else {
                norm_squared_f64(query).sqrt()
            }
        } else {
            norm_squared_f64(query).sqrt()
        };
        let router = self.polar_router.as_ref();
        let mut top_heap: BinaryHeap<(Distance, u64, u32)> =
            BinaryHeap::with_capacity(config.top_k + 1);
        let rerank_slice = &candidates[..eff_limit];
        let use_radial_bound = space.rerank.is_none_or(|store| {
            store.config == crate::index::pelorus::engine::RerankConfig::default()
                || store.config.representation
                    == crate::index::pelorus::engine::RerankRepresentation::Polar
        });
        let mut decode_buffer = SCRATCH.with(|cell| std::mem::take(&mut cell.borrow_mut().rerank));
        let ctx = RerankCtx {
            space,
            query,
            factor,
            query_radius,
            radial_bound_on,
            use_radial_bound,
            top_k: config.top_k,
            router,
            geometry_codes: &self.geometry_codes,
            slot_indices: &self.slot_indices,
            ids: &self.ids,
            rerank_slice,
            trace_slow,
            #[cfg(feature = "research")]
            stage_prof,
        };
        let mut state = RerankState {
            top_heap: &mut top_heap,
            decode_buffer: &mut decode_buffer,
            #[cfg(feature = "research")]
            score_ns: &mut score_ns,
            #[cfg(feature = "research")]
            scored: &mut scored,
            #[cfg(feature = "research")]
            radial_rejections: &mut radial_rejections,
        };
        if let Some(ssd) = ssd {
            let store = space
                .rerank
                .ok_or(crate::index::pelorus::engine::Error::CorruptIndex(
                    "SSD 缺少精排记录",
                ))?;
            ctx.rerank_shared_io(&mut state, store, ssd)?;
        } else {
            ctx.rerank_sequential(&mut state, 2)?;
        }
        decode_buffer.reset_query();
        SCRATCH.with(|cell| cell.borrow_mut().rerank = decode_buffer);
        if fast_kernels && candidates.capacity() > 0 {
            candidates.clear();
            SCRATCH.with(|cell| {
                let mut s = cell.borrow_mut();
                if candidates.capacity() > s.gate_pool.capacity() {
                    s.gate_pool = candidates;
                }
            });
        }
        let mut results = top_heap.into_vec();
        results.sort_unstable();
        let out: Vec<(u64, f64)> = results
            .into_iter()
            .map(|(distance, _, id)| {
                (
                    self.ids[id as usize],
                    poincare_distance(distance.0, space.curvature),
                )
            })
            .collect();
        #[cfg(feature = "research")]
        if let Some(t3) = t_rerank_start {
            STAGE_PROFILE.with(|p| {
                let mut p = p.borrow_mut();
                p.exact_rerank_ns = t3.elapsed().as_nanos() as u64;
                p.rerank_score_ns = score_ns;
                p.reranked_count = scored;
                p.rerank_radial_rejections = radial_rejections;
            });
        }
        Ok(out)
    }
}

/// 精排阶段不可变查询上下文（避免跨多种重排路径传递十余个参数）。
struct RerankCtx<'a> {
    space: &'a NativeSpace<'a>,
    query: &'a [f32],
    factor: f64,
    query_radius: f64,
    radial_bound_on: bool,
    use_radial_bound: bool,
    top_k: usize,
    router: Option<&'a PolarRouter>,
    geometry_codes: &'a [GeometryCode],
    slot_indices: &'a [usize],
    ids: &'a [u64],
    rerank_slice: &'a [Candidate],
    trace_slow: bool,
    #[cfg(feature = "research")]
    stage_prof: bool,
}

/// 精排阶段可变结果堆、缓冲与研究计数状态。
struct RerankState<'a> {
    top_heap: &'a mut BinaryHeap<(Distance, u64, u32)>,
    decode_buffer: &'a mut crate::index::pelorus::engine::storage::RerankScratch,
    #[cfg(feature = "research")]
    score_ns: &'a mut u64,
    #[cfg(feature = "research")]
    scored: &'a mut usize,
    #[cfg(feature = "research")]
    radial_rejections: &'a mut usize,
}

impl RerankCtx<'_> {
    fn rerank_shared_io(
        &self,
        state: &mut RerankState<'_>,
        store: &crate::index::pelorus::engine::storage::ColdStore,
        ssd: &crate::index::pelorus::io::PelorusSsd,
    ) -> crate::index::pelorus::engine::Result<()> {
        let metric = crate::index::pelorus::engine::Metric::Poincare {
            curvature: self.space.curvature,
        };
        let mut nodes = Vec::with_capacity(ssd.batch_rows());
        let mut slots = Vec::with_capacity(ssd.batch_rows());
        for batch in self.rerank_slice.chunks(ssd.batch_rows()) {
            nodes.clear();
            slots.clear();
            for &(_, node) in batch {
                if !self.pruned(state, node) {
                    self.space.charge_score()?;
                    nodes.push(node);
                    slots.push(self.slot_indices[node as usize]);
                }
            }
            ssd.read_rows(&slots, |position, row| {
                let node = nodes[position];
                let proxy = store.score_row(
                    self.query,
                    self.factor,
                    slots[position],
                    row,
                    metric,
                    &mut state.decode_buffer.query_norm,
                );
                self.push_score(state, node, proxy);
            })
            .map_err(|error| {
                crate::index::pelorus::engine::Error::Host(std::sync::Arc::new(error))
            })?;
        }
        Ok(())
    }
    #[inline]
    fn pruned(&self, state: &mut RerankState<'_>, node: u32) -> bool {
        if !self.use_radial_bound || state.top_heap.len() < self.top_k {
            return false;
        }
        let bound = state.top_heap.peek().unwrap().0;
        if let Some(meta) = self.router.and_then(|r| r.node_meta(node as usize)) {
            if Distance(meta.radial_lower_bound_fast(
                self.query_radius,
                self.factor,
                self.radial_bound_on,
            )) > bound
            {
                #[cfg(feature = "research")]
                if self.stage_prof {
                    *state.radial_rejections += 1;
                }
                #[cfg(feature = "research")]
                if self.trace_slow {
                    trace_query(|t| t.bound_rejections += 1);
                }
                return true;
            }
        } else if let Some(&code) = self.geometry_codes.get(node as usize) {
            let radial_lb = if self.radial_bound_on {
                radial_bound_core(
                    self.query_radius,
                    code.radius,
                    self.factor,
                    code.boundary_factor,
                )
            } else {
                1.0
            };
            if Distance(radial_lb) > bound {
                #[cfg(feature = "research")]
                if self.stage_prof {
                    *state.radial_rejections += 1;
                }
                return true;
            }
        }
        false
    }

    #[inline]
    fn score_and_push(
        &self,
        state: &mut RerankState<'_>,
        node: u32,
    ) -> crate::index::pelorus::engine::Result<f64> {
        self.space.charge_score()?;
        let slot = self.slot_indices[node as usize];
        #[cfg(feature = "research")]
        let score_start = self.stage_prof.then(std::time::Instant::now);
        let distance = self.space.rerank_proxy(
            self.query,
            self.factor,
            slot,
            self.trace_slow,
            state.decode_buffer,
        )?;
        #[cfg(feature = "research")]
        if let Some(start) = score_start {
            *state.score_ns += start.elapsed().as_nanos() as u64;
            *state.scored += 1;
        }
        self.push_score(state, node, distance);
        Ok(distance)
    }

    fn push_score(&self, state: &mut RerankState<'_>, node: u32, distance: f64) {
        let exact = (Distance(distance), self.ids[node as usize], node);
        if state.top_heap.len() < self.top_k {
            state.top_heap.push(exact);
        } else if exact < *state.top_heap.peek().unwrap() {
            state.top_heap.pop();
            state.top_heap.push(exact);
        }
    }

    /// 常规滑窗预取重排路径（内存冷存、非直接读或禁用预取窗口时使用）。
    fn rerank_sequential(
        &self,
        state: &mut RerankState<'_>,
        prefetch_window: usize,
    ) -> crate::index::pelorus::engine::Result<()> {
        if prefetch_window > 0 {
            for &(_, node) in self.rerank_slice.iter().take(prefetch_window) {
                self.space.prefetch_rerank(self.slot_indices[node as usize]);
            }
        }
        for (idx_pos, &(_, node)) in self.rerank_slice.iter().enumerate() {
            if prefetch_window > 0 && idx_pos + prefetch_window < self.rerank_slice.len() {
                let ahead = self.rerank_slice[idx_pos + prefetch_window].1 as usize;
                self.space.prefetch_rerank(self.slot_indices[ahead]);
            }
            if !self.pruned(state, node) {
                self.score_and_push(state, node)?;
            }
        }
        Ok(())
    }
}
