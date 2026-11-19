//! 双曲候选束扩展。

use super::*;
impl QuIVer {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn native_beam(
        &self,
        space: &NativeSpace<'_>,
        query: &[f32],
        factor: f64,
        entry: u32,
        layer: usize,
        ef: usize,
        include_deleted: bool,
        quantized: bool,
    ) -> Vec<Candidate> {
        #[cfg(feature = "research")]
        let strategy = hyperbolic_strategy();
        #[cfg(feature = "research")]
        let comp = geometry_components();
        #[cfg(feature = "research")]
        let speed_cfg = execution_config();
        #[cfg(feature = "research")]
        let trace_slow =
            TRACE_ACTIVE.load(AtomicOrdering::Relaxed) || !speed_cfg.zero_trace_overhead;
        #[cfg(not(feature = "research"))]
        let trace_slow = false;

        #[cfg(feature = "research")]
        let two_level_cones = speed_cfg.two_level_cones;
        #[cfg(not(feature = "research"))]
        let two_level_cones = true;

        #[cfg(feature = "research")]
        let rolling_prefetch = speed_cfg.rolling_prefetch;
        #[cfg(not(feature = "research"))]
        let rolling_prefetch = true;

        #[cfg(feature = "research")]
        let fast_kernels = speed_cfg.fast_kernels;
        #[cfg(not(feature = "research"))]
        let fast_kernels = true;

        #[cfg(feature = "research")]
        let traversal = speed_cfg.traversal;
        #[cfg(not(feature = "research"))]
        let traversal = TraversalProfile::Standard;

        #[cfg(feature = "research")]
        let use_sign_magnitude = matches!(
            strategy,
            HyperbolicStrategy::Full
                | HyperbolicStrategy::FixedShellCompass
                | HyperbolicStrategy::ScalarQuantizedDirection
                | HyperbolicStrategy::FullPrecisionDirection
        );
        #[cfg(not(feature = "research"))]
        let use_sign_magnitude = true;

        #[cfg(feature = "research")]
        let use_asym_gate = matches!(
            strategy,
            HyperbolicStrategy::Full
                | HyperbolicStrategy::ScalarQuantizedDirection
                | HyperbolicStrategy::FullPrecisionDirection
        ) && comp.asymmetric_gate;
        #[cfg(not(feature = "research"))]
        let use_asym_gate = true;

        // ═══════════════════════════════════════════════════════════════════════
        //  第三/四代黎曼 QuIVer（A7 / A8: SM-Poincaré）热路径（Layer 0）
        // ═══════════════════════════════════════════════════════════════════════
        if use_sign_magnitude && layer == 0 && self.n > 1 {
            #[cfg(feature = "research")]
            let stage_prof = STAGE_PROFILE_ACTIVE.load(AtomicOrdering::Relaxed);
            #[cfg(feature = "research")]
            let t_router_start = stage_prof.then(std::time::Instant::now);

            #[cfg(feature = "research")]
            let variance_stabilized_sketch = recall_config().variance_stabilized_sketch;
            #[cfg(not(feature = "research"))]
            let variance_stabilized_sketch = false;

            let residual_dict = self
                .polar_router
                .as_ref()
                .and_then(|r| r.residual_dict.as_ref());

            let query_sig = Bq2Signature::from_vector(query);
            let (q_sm, q_radius) = SmCodeParams::from_vector_and_sig(query, &query_sig);
            let query_depth = dimensionless_depth(factor);
            let horiz_gate = self
                .polar_router
                .as_ref()
                .filter(|r| r.adaptive_shells)
                .map_or(0.85, |r| (f64::from(r.min_depth) + 0.15).min(0.85));

            return SCRATCH.with(|cell| {
                let mut scratch = cell.borrow_mut();
                let scratch = &mut *scratch;
                scratch.visited.grow(self.n);
                scratch.last_query_radius = q_radius;
                scratch
                    .cone_cache
                    .begin_query(query, q_radius, residual_dict.is_some());

                // 分支 A：GeometryBq2 量化导航 —— 主循环 5-popcount 对称遍历 + Stage-0.5 非对称校准过滤器精筛
                if quantized {
                    #[cfg(feature = "research")]
                    let is_a8_budget = matches!(
                        strategy,
                        HyperbolicStrategy::Full
                            | HyperbolicStrategy::ScalarQuantizedDirection
                            | HyperbolicStrategy::FullPrecisionDirection
                    );
                    #[cfg(not(feature = "research"))]
                    let is_a8_budget = true;

                    #[cfg(feature = "research")]
                    let radial_bound_on = comp.radial_bound;
                    #[cfg(not(feature = "research"))]
                    let radial_bound_on = true;

                    #[cfg(feature = "research")]
                    let hot_cfg = storage_config();
                    #[cfg(feature = "research")]
                    let active_colocated: Option<&HotSmStore> = if hot_cfg.colocated_store
                        && self.dirty_count == 0
                        && !matches!(
                            strategy,
                            HyperbolicStrategy::ScalarQuantizedDirection
                                | HyperbolicStrategy::FullPrecisionDirection
                        ) {
                        self.polar_router
                            .as_ref()
                            .and_then(|r| r.colocated_store.as_deref())
                    } else {
                        None
                    };
                    #[cfg(not(feature = "research"))]
                    let active_colocated: Option<&HotSmStore> = if self.dirty_count == 0 {
                        self.polar_router
                            .as_ref()
                            .and_then(|r| r.colocated_store.as_deref())
                    } else {
                        None
                    };

                    #[cfg(feature = "research")]
                    let early_gate = if active_colocated.is_some() {
                        hot_cfg.early_gate
                    } else {
                        SketchFilter::Off
                    };
                    #[cfg(not(feature = "research"))]
                    let early_gate = SketchFilter::Off;

                    #[cfg(feature = "research")]
                    let pf_hdr = usize::from(hot_cfg.header_prefetch_window.clamp(1, 32));
                    #[cfg(not(feature = "research"))]
                    let pf_hdr = 8usize;

                    #[cfg(feature = "research")]
                    let eager_prefetch =
                        active_colocated.is_some() && hot_cfg.eager_prefetch_overlap;
                    #[cfg(not(feature = "research"))]
                    let eager_prefetch = active_colocated.is_some();

                    // 预取指定节点的 L0 扁平邻接表（两行共 128B，覆盖度数 + 31 个邻居，消除 132MB L0 数组 DRAM 同步停顿）
                    let prefetch_l0_adj = |node: u32| {
                        #[cfg(target_arch = "x86_64")]
                        {
                            let base = (node as usize) * self.layer0.stride;
                            if base < self.layer0.data.len() {
                                unsafe {
                                    use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
                                    let ptr = self.layer0.data.as_ptr().add(base) as *const i8;
                                    _mm_prefetch(ptr, _MM_HINT_T0);
                                    if self.layer0.stride > 16 {
                                        _mm_prefetch(ptr.add(64), _MM_HINT_T0);
                                    }
                                }
                            }
                        }
                        #[cfg(not(target_arch = "x86_64"))]
                        let _ = node;
                    };

                    let q_sym_sketch = if matches!(early_gate, SketchFilter::SymSketch256 { .. }) {
                        pack_strided_sketch256(query)
                    } else {
                        [0u64; 4]
                    };
                    if matches!(early_gate, SketchFilter::AsymSketch256 { .. }) {
                        scratch.asym_sketch256.encode_into(query);
                    }

                    // 先提取极坐标路由种子与方向锥最大对齐分，用于自适应识别高维各向同性退化查询
                    let mut max_cone_score = 32768i32;
                    if let Some(router) = &self.polar_router {
                        if factor <= 1.0 + 1e-12 {
                            scratch.router_seeds.clear();
                            scratch.router_seeds.extend_from_slice(&router.origin_hubs);
                        } else {
                            let probe_max = if router.adaptive_shells && router.min_depth > 0.30 {
                                80
                            } else {
                                64
                            };
                            let probe_cones = (ef / 2).clamp(24, probe_max);
                            router.collect_focused_query_seeds(
                                &query_sig,
                                q_sm,
                                query_depth,
                                self.dim,
                                ef,
                                probe_cones,
                                two_level_cones,
                                traversal,
                                &mut scratch.cone_buf,
                                &mut scratch.router_seeds,
                            );
                            if let Some(&(best_s, _)) = scratch.cone_buf.first() {
                                max_cone_score = best_s;
                            }
                        }
                    }
                    // 仅当流形整体处于各向同性退化态且当前查询最大锥余弦落入近正交区时启用各向同性保护，真实/聚簇流形零误触发
                    let is_iso_manifold = self
                        .polar_router
                        .as_ref()
                        .is_some_and(|r| r.is_isotropic_manifold);
                    let iso_cone_thresh = ((5.2 / (self.dim.max(64) as f64).sqrt())
                        .clamp(0.16, 0.26)
                        * 32768.0) as i32;
                    let isotropic_query = variance_stabilized_sketch
                        && is_iso_manifold
                        && max_cone_score < iso_cone_thresh;

                    let ef_nav = if is_a8_budget {
                        match traversal {
                            TraversalProfile::Standard => (ef + (ef / 4).clamp(20, 128)).max(56),
                            TraversalProfile::Balanced => (ef + (ef / 6).clamp(12, 96)).max(48),
                            TraversalProfile::Lean => (ef + (ef / 8).clamp(8, 64)).max(40),
                        }
                    } else {
                        (ef * 5 / 4).max(56)
                    };
                    let horiz_max_steps = if isotropic_query {
                        (ef / 2).clamp(32, 640)
                    } else {
                        match traversal {
                            TraversalProfile::Standard => 24usize,
                            TraversalProfile::Balanced => 10usize,
                            TraversalProfile::Lean => 4usize,
                        }
                    };
                    let track_horiz = query_depth > horiz_gate;
                    let horiz_cap = if isotropic_query {
                        (ef / 3).clamp(24, 256)
                    } else {
                        16usize
                    };
                    let keep_live_heap =
                        !use_asym_gate || (!include_deleted && self.dirty_count > 0);

                    scratch.visited.begin();
                    scratch.candidates.clear();
                    scratch.results.clear();
                    scratch.apex_results.clear();
                    scratch.horiz_candidates.clear();
                    scratch.horiz_results.clear();

                    // 统一入堆更新逻辑（同时服务常规全量评估与头行早筛幸存节点体评估）
                    let push_scored_quant =
                        |scratch: &mut Scratch, node: u32, proxy: f64, cos_hat: f64, d_v: f32| {
                            let idx = node as usize;
                            let cand = (Distance(proxy), node);
                            if scratch.apex_results.len() < ef_nav
                                || cand < *scratch.apex_results.peek().unwrap()
                            {
                                if eager_prefetch
                                    && scratch.candidates.peek().is_none_or(|top| cand < top.0)
                                {
                                    prefetch_l0_adj(node);
                                }
                                scratch.candidates.push(Reverse(cand));
                                scratch.apex_results.push(cand);
                                if scratch.apex_results.len() > ef_nav {
                                    scratch.apex_results.pop();
                                }
                            }
                            if keep_live_heap
                                && (include_deleted || !self.tombstones[idx])
                                && (scratch.results.len() < ef
                                    || cand < *scratch.results.peek().unwrap())
                            {
                                scratch.results.push(cand);
                                if scratch.results.len() > ef {
                                    scratch.results.pop();
                                }
                            }
                            if track_horiz {
                                let ang_loss = cosine_to_angular_loss(cos_hat);
                                let shell_pen =
                                    (0.20 * ANGULAR_SCALE * (query_depth - f64::from(d_v)).abs())
                                        .round() as u32;
                                let h_loss = ang_loss.saturating_add(shell_pen);
                                if scratch.horiz_results.len() < horiz_cap
                                    || h_loss < scratch.horiz_results.peek().unwrap().0
                                {
                                    scratch.horiz_candidates.push(Reverse((h_loss, node)));
                                    scratch.horiz_results.push((h_loss, node));
                                    if scratch.horiz_results.len() > horiz_cap {
                                        scratch.horiz_results.pop();
                                    }
                                }
                            }
                        };

                    // 已通过 visited 去重的节点直接评分并入堆（消除二次 SparseBitset 位查询）
                    let eval_and_push_quant = |scratch: &mut Scratch, node: u32| {
                        let (proxy, cos_hat, d_v) = if let Some(coloc) = active_colocated {
                            #[cfg(feature = "research")]
                            if trace_slow {
                                trace_query(|t| {
                                    t.scored.insert(node);
                                    t.approximate_evals += 1;
                                });
                            }
                            let rec = coloc.get(node as usize);
                            let meta = *rec.meta;
                            let cos_v = scratch.cone_cache.eval_sym_cosine(
                                residual_dict,
                                &query_sig,
                                q_sm,
                                rec.pos.as_ptr(),
                                rec.strong.as_ptr(),
                                meta,
                                self.dim,
                            );
                            (
                                meta.sm_poincare_proxy(q_radius, factor, cos_v),
                                cos_v,
                                meta.depth,
                            )
                        } else {
                            self.eval_sm_node_fast(
                                space,
                                query,
                                &query_sig,
                                q_sm,
                                q_radius,
                                factor,
                                node,
                                Some(&mut scratch.cone_cache),
                                #[cfg(feature = "research")]
                                strategy,
                                trace_slow,
                            )
                        };
                        push_scored_quant(scratch, node, proxy, cos_hat, d_v);
                    };

                    // 处理单轮展开去重后的邻居批次：自动分派单页合表 + 头行两级早筛流水线或经典基线路径
                    let eval_neighbor_batch = |scratch: &mut Scratch, unv: &[u32]| {
                        let cnt = unv.len();
                        if cnt == 0 {
                            return;
                        }
                        if let Some(coloc) = active_colocated {
                            let gate_ready = !matches!(early_gate, SketchFilter::Off)
                                && scratch.apex_results.len() >= ef_nav
                                && (!keep_live_heap || scratch.results.len() >= ef)
                                && (!track_horiz || scratch.horiz_results.len() >= horiz_cap);
                            if gate_ready {
                                // ── Stage A：单缓存行头预取（1 LFB/节点，高 MLP）+ Cache Line 0 早筛 ──
                                let apex_worst = scratch.apex_results.peek().unwrap().0.0;
                                let live_worst = if keep_live_heap {
                                    scratch.results.peek().unwrap().0.0
                                } else {
                                    0.0
                                };
                                let horiz_worst = if track_horiz {
                                    scratch.horiz_results.peek().unwrap().0
                                } else {
                                    0
                                };
                                if !eager_prefetch {
                                    let init_pf = pf_hdr.min(cnt);
                                    for i in 0..init_pf {
                                        coloc.prefetch_header(unv[i] as usize);
                                    }
                                }
                                let mut pass_buf = [0u32; 64];
                                let mut pass_cnt = 0usize;
                                for i in 0..cnt {
                                    if i + pf_hdr < cnt {
                                        coloc.prefetch_header(unv[i + pf_hdr] as usize);
                                    }
                                    let nb = unv[i];
                                    let hdr = coloc.get(nb as usize);
                                    let meta = hdr.meta;
                                    let rejected = match early_gate {
                                        SketchFilter::Off => false,
                                        SketchFilter::LosslessRadial => {
                                            let lb = meta.radial_lower_bound_fast(
                                                q_radius,
                                                factor,
                                                radial_bound_on,
                                            );
                                            let rej_apex = lb > apex_worst;
                                            let rej_live = !keep_live_heap || lb > live_worst;
                                            let rej_horiz = if track_horiz {
                                                let min_hl = (0.20
                                                    * ANGULAR_SCALE
                                                    * (query_depth - f64::from(meta.depth)).abs())
                                                .round()
                                                    as u32;
                                                min_hl > horiz_worst
                                            } else {
                                                true
                                            };
                                            rej_apex && rej_live && rej_horiz
                                        }
                                        SketchFilter::SymSketch256 { margin } => {
                                            let h = (hdr.sketch256[0] ^ q_sym_sketch[0])
                                                .count_ones()
                                                + (hdr.sketch256[1] ^ q_sym_sketch[1]).count_ones()
                                                + (hdr.sketch256[2] ^ q_sym_sketch[2]).count_ones()
                                                + (hdr.sketch256[3] ^ q_sym_sketch[3]).count_ones();
                                            let cos_256 = f64::from(unsafe {
                                                *COS_PI_256.get_unchecked(h as usize)
                                            });
                                            let eff_margin = if isotropic_query {
                                                margin
                                                    * unsafe {
                                                        *SKETCH256_STD_RATIO
                                                            .get_unchecked(h as usize)
                                                    }
                                            } else {
                                                margin
                                            };
                                            let cos_opt =
                                                (cos_256 + f64::from(eff_margin)).min(1.0);
                                            let proxy_opt =
                                                meta.sm_poincare_proxy(q_radius, factor, cos_opt);
                                            let rej_apex = proxy_opt > apex_worst;
                                            let rej_live =
                                                !keep_live_heap || proxy_opt > live_worst;
                                            let rej_horiz = if track_horiz {
                                                let ang_opt = cosine_to_angular_loss(cos_opt);
                                                let shell_pen = (0.20
                                                    * ANGULAR_SCALE
                                                    * (query_depth - f64::from(meta.depth)).abs())
                                                .round()
                                                    as u32;
                                                ang_opt.saturating_add(shell_pen) > horiz_worst
                                            } else {
                                                true
                                            };
                                            rej_apex && rej_live && rej_horiz
                                        }
                                        SketchFilter::AsymSketch256 { margin } => {
                                            let cos_256 =
                                                scratch.asym_sketch256.cosine_hat(hdr.sketch256);
                                            let eff_margin = if isotropic_query {
                                                let std_scale =
                                                    (((1.0 - cos_256 * cos_256).max(0.0).sqrt()
                                                        / 0.555)
                                                        .clamp(1.0, 1.80))
                                                        as f32;
                                                margin * std_scale
                                            } else {
                                                margin
                                            };
                                            let cos_opt =
                                                (cos_256 + f64::from(eff_margin)).min(1.0);
                                            let proxy_opt =
                                                meta.sm_poincare_proxy(q_radius, factor, cos_opt);
                                            let rej_apex = proxy_opt > apex_worst;
                                            let rej_live =
                                                !keep_live_heap || proxy_opt > live_worst;
                                            let rej_horiz = if track_horiz {
                                                let ang_opt = cosine_to_angular_loss(cos_opt);
                                                let shell_pen = (0.20
                                                    * ANGULAR_SCALE
                                                    * (query_depth - f64::from(meta.depth)).abs())
                                                .round()
                                                    as u32;
                                                ang_opt.saturating_add(shell_pen) > horiz_worst
                                            } else {
                                                true
                                            };
                                            rej_apex && rej_live && rej_horiz
                                        }
                                    };
                                    #[cfg(feature = "research")]
                                    if trace_slow {
                                        trace_query(|t| {
                                            t.scored.insert(nb);
                                            t.header_checks += 1;
                                            if rejected {
                                                t.header_rejections += 1;
                                            }
                                        });
                                    }
                                    if !rejected {
                                        coloc.prefetch_body(nb as usize);
                                        pass_buf[pass_cnt] = nb;
                                        pass_cnt += 1;
                                    }
                                }
                                if eager_prefetch
                                    && let Some(&Reverse((_, next_l0))) = scratch.candidates.peek()
                                {
                                    prefetch_l0_adj(next_l0);
                                }
                                // ── Stage B：仅对幸存节点执行完整 5-popcount（含正交切锥残差重构）评估并入堆 ──
                                for j in 0..pass_cnt {
                                    let nb = pass_buf[j];
                                    #[cfg(feature = "research")]
                                    if trace_slow {
                                        trace_query(|t| {
                                            t.approximate_evals += 1;
                                        });
                                    }
                                    let rec = coloc.get(nb as usize);
                                    let meta = *rec.meta;
                                    let cos_hat = scratch.cone_cache.eval_sym_cosine(
                                        residual_dict,
                                        &query_sig,
                                        q_sm,
                                        rec.pos.as_ptr(),
                                        rec.strong.as_ptr(),
                                        meta,
                                        self.dim,
                                    );
                                    let proxy = meta.sm_poincare_proxy(q_radius, factor, cos_hat);
                                    push_scored_quant(scratch, nb, proxy, cos_hat, meta.depth);
                                }
                                return;
                            }

                            // 纯 Opt 1 单页合表全量滚动预取路径（基线模式下结果 100% 逐位一致）
                            const PF_COLOC: usize = 4;
                            if !eager_prefetch {
                                let init_pf = PF_COLOC.min(cnt);
                                for i in 0..init_pf {
                                    coloc.prefetch_full(unv[i] as usize);
                                }
                            } else if let Some(&Reverse((_, next_l0))) = scratch.candidates.peek() {
                                prefetch_l0_adj(next_l0);
                            }
                            for i in 0..cnt {
                                if i + PF_COLOC < cnt {
                                    coloc.prefetch_full(unv[i + PF_COLOC] as usize);
                                }
                                eval_and_push_quant(scratch, unv[i]);
                            }
                            return;
                        }

                        const PF_NB: usize = 3;
                        let init_pf = PF_NB.min(cnt);
                        for i in 0..init_pf {
                            self.prefetch_hot_sm_node(space, unv[i] as usize);
                        }
                        for i in 0..cnt {
                            if i + PF_NB < cnt {
                                self.prefetch_hot_sm_node(space, unv[i + PF_NB] as usize);
                            }
                            eval_and_push_quant(scratch, unv[i]);
                        }
                    };

                    if !space.visit() {
                        return Vec::new();
                    }
                    scratch.visited.set(entry as usize);
                    eval_and_push_quant(scratch, entry);

                    if self.polar_router.is_some() {
                        if rolling_prefetch {
                            const PF_SEED: usize = 4;
                            scratch.unvisited_buf.clear();
                            for &seed in &scratch.router_seeds {
                                let idx = seed as usize;
                                if !scratch.visited.test(idx) {
                                    if !space.visit() {
                                        return Vec::new();
                                    }
                                    scratch.visited.set(idx);
                                    scratch.unvisited_buf.push(seed);
                                }
                            }
                            let seed_cnt = scratch.unvisited_buf.len();
                            let init_pf = PF_SEED.min(seed_cnt);
                            for i in 0..init_pf {
                                let s_idx = scratch.unvisited_buf[i] as usize;
                                if let Some(coloc) = active_colocated {
                                    coloc.prefetch_full(s_idx);
                                } else {
                                    self.prefetch_hot_sm_node(space, s_idx);
                                }
                            }
                            for i in 0..seed_cnt {
                                if i + PF_SEED < seed_cnt {
                                    let ahead = scratch.unvisited_buf[i + PF_SEED] as usize;
                                    if let Some(coloc) = active_colocated {
                                        coloc.prefetch_full(ahead);
                                    } else {
                                        self.prefetch_hot_sm_node(space, ahead);
                                    }
                                }
                                let seed = scratch.unvisited_buf[i];
                                eval_and_push_quant(scratch, seed);
                            }
                        } else {
                            for i in 0..scratch.router_seeds.len() {
                                let seed = scratch.router_seeds[i];
                                if !scratch.visited.test(seed as usize) {
                                    self.prefetch_native_direction(space, seed as usize);
                                }
                            }
                            for i in 0..scratch.router_seeds.len() {
                                let seed = scratch.router_seeds[i];
                                let idx = seed as usize;
                                if !scratch.visited.test(idx) {
                                    if !space.visit() {
                                        return Vec::new();
                                    }
                                    scratch.visited.set(idx);
                                    eval_and_push_quant(scratch, seed);
                                }
                            }
                        }
                    }

                    #[cfg(feature = "research")]
                    let t_beam_start = if let Some(t0) = t_router_start {
                        let now = std::time::Instant::now();
                        STAGE_PROFILE
                            .with(|p| p.borrow_mut().router_seed_ns = (now - t0).as_nanos() as u64);
                        Some(now)
                    } else {
                        None
                    };

                    #[cfg(feature = "research")]
                    let mut main_expansions = 0usize;
                    #[cfg(feature = "research")]
                    let mut horizon_expansions = 0usize;
                    while let Some(Reverse(cand)) = scratch.candidates.pop() {
                        if scratch.apex_results.len() >= ef_nav
                            && cand > *scratch.apex_results.peek().unwrap()
                        {
                            break;
                        }
                        #[cfg(feature = "research")]
                        if stage_prof {
                            main_expansions += 1;
                        }
                        let neighbors = self.native_layer0_neighbors_fast(cand.1, trace_slow);
                        if rolling_prefetch && neighbors.len() <= 64 {
                            let mut unv = [0u32; 64];
                            let mut cnt = 0usize;
                            let gate_ready_now = eager_prefetch
                                && !matches!(early_gate, SketchFilter::Off)
                                && scratch.apex_results.len() >= ef_nav
                                && (!keep_live_heap || scratch.results.len() >= ef)
                                && (!track_horiz || scratch.horiz_results.len() >= horiz_cap);
                            for &neighbor in neighbors {
                                let idx = neighbor as usize;
                                if !scratch.visited.test(idx) {
                                    if !space.visit() {
                                        return Vec::new();
                                    }
                                    scratch.visited.set(idx);
                                    if eager_prefetch && let Some(coloc) = active_colocated {
                                        if gate_ready_now {
                                            if cnt < pf_hdr {
                                                coloc.prefetch_header(idx);
                                            }
                                        } else if cnt < 4 {
                                            coloc.prefetch_full(idx);
                                        }
                                    }
                                    unv[cnt] = neighbor;
                                    cnt += 1;
                                }
                            }
                            eval_neighbor_batch(scratch, &unv[..cnt]);
                        } else {
                            for &neighbor in neighbors {
                                if !scratch.visited.test(neighbor as usize) {
                                    self.prefetch_native_direction(space, neighbor as usize);
                                }
                            }
                            for &neighbor in neighbors {
                                let idx = neighbor as usize;
                                if !scratch.visited.test(idx) {
                                    if !space.visit() {
                                        return Vec::new();
                                    }
                                    scratch.visited.set(idx);
                                    eval_and_push_quant(scratch, neighbor);
                                }
                            }
                        }
                    }

                    // 对外壳层高深度查询补充极少量同视界锚点邻居扩展（复用同一 visited 位图，零重复评估）
                    if track_horiz && horiz_max_steps > 0 {
                        let mut horiz_steps = 0usize;
                        while let Some(Reverse((hl, h_node))) = scratch.horiz_candidates.pop() {
                            if horiz_steps >= horiz_max_steps
                                || (scratch.horiz_results.len() >= horiz_cap
                                    && hl > scratch.horiz_results.peek().unwrap().0)
                            {
                                break;
                            }
                            horiz_steps += 1;
                            #[cfg(feature = "research")]
                            if stage_prof {
                                horizon_expansions += 1;
                            }
                            let neighbors = self.native_layer0_neighbors_fast(h_node, trace_slow);
                            if rolling_prefetch && neighbors.len() <= 64 {
                                let mut unv = [0u32; 64];
                                let mut cnt = 0usize;
                                let gate_ready_now = eager_prefetch
                                    && !matches!(early_gate, SketchFilter::Off)
                                    && scratch.apex_results.len() >= ef_nav
                                    && (!keep_live_heap || scratch.results.len() >= ef)
                                    && (!track_horiz || scratch.horiz_results.len() >= horiz_cap);
                                for &neighbor in neighbors {
                                    let idx = neighbor as usize;
                                    if !scratch.visited.test(idx) {
                                        if !space.visit() {
                                            return Vec::new();
                                        }
                                        scratch.visited.set(idx);
                                        if eager_prefetch && let Some(coloc) = active_colocated {
                                            if gate_ready_now {
                                                if cnt < pf_hdr {
                                                    coloc.prefetch_header(idx);
                                                }
                                            } else if cnt < 4 {
                                                coloc.prefetch_full(idx);
                                            }
                                        }
                                        unv[cnt] = neighbor;
                                        cnt += 1;
                                    }
                                }
                                eval_neighbor_batch(scratch, &unv[..cnt]);
                            } else {
                                for &neighbor in neighbors {
                                    if !scratch.visited.test(neighbor as usize) {
                                        self.prefetch_native_direction(space, neighbor as usize);
                                    }
                                }
                                for &neighbor in neighbors {
                                    let idx = neighbor as usize;
                                    if !scratch.visited.test(idx) {
                                        if !space.visit() {
                                            return Vec::new();
                                        }
                                        scratch.visited.set(idx);
                                        eval_and_push_quant(scratch, neighbor);
                                    }
                                }
                            }
                        }
                    }

                    #[cfg(feature = "research")]
                    let t_gate_start = if let Some(t1) = t_beam_start {
                        let now = std::time::Instant::now();
                        STAGE_PROFILE.with(|p| {
                            let mut p = p.borrow_mut();
                            p.beam_nav_ns = (now - t1).as_nanos() as u64;
                            p.navigation_passes += 1;
                            p.main_expansion_calls = main_expansions;
                            p.horizon_expansion_calls = horizon_expansions;
                            p.ef_nav = ef_nav;
                            p.router_seeds = scratch.router_seeds.len();
                        });
                        Some(now)
                    } else {
                        None
                    };

                    // Stage-0.5：非对称校准过滤器精筛（仅重排有界候选池，记录实际冷存读取次数）
                    if use_asym_gate {
                        scratch.asym_query.encode_into(query, q_radius);
                        scratch.visited.begin();
                        scratch.gate_pool.clear();
                        if rolling_prefetch {
                            scratch.unvisited_buf.clear();
                            let mut collect_unique = |node: u32| {
                                let idx = node as usize;
                                if !scratch.visited.test(idx)
                                    && (include_deleted || !self.tombstones[idx])
                                {
                                    if !space.visit() {
                                        return;
                                    }
                                    scratch.visited.set(idx);
                                    scratch.unvisited_buf.push(node);
                                }
                            };
                            for &(_, node) in scratch.apex_results.iter() {
                                collect_unique(node);
                            }
                            for &(_, node) in scratch.horiz_results.iter() {
                                collect_unique(node);
                            }
                            if keep_live_heap {
                                for &(_, node) in scratch.results.iter() {
                                    collect_unique(node);
                                }
                            }
                            let cnt = scratch.unvisited_buf.len();
                            const PF_GATE: usize = 3;
                            let init_pf = PF_GATE.min(cnt);
                            for i in 0..init_pf {
                                let g_idx = scratch.unvisited_buf[i] as usize;
                                if let Some(coloc) = active_colocated {
                                    coloc.prefetch_full(g_idx);
                                } else {
                                    self.prefetch_hot_sm_node(space, g_idx);
                                }
                            }
                            for i in 0..cnt {
                                if i + PF_GATE < cnt {
                                    let ahead = scratch.unvisited_buf[i + PF_GATE] as usize;
                                    if let Some(coloc) = active_colocated {
                                        coloc.prefetch_full(ahead);
                                    } else {
                                        self.prefetch_hot_sm_node(space, ahead);
                                    }
                                }
                                let node = scratch.unvisited_buf[i];
                                let proxy = if let Some(coloc) = active_colocated {
                                    #[cfg(feature = "research")]
                                    if trace_slow {
                                        trace_query(|t| {
                                            t.scored.insert(node);
                                            t.approximate_evals += 1;
                                        });
                                    }
                                    let rec = coloc.get(node as usize);
                                    let meta = *rec.meta;
                                    let cos_hat = scratch.cone_cache.eval_asym_cosine(
                                        residual_dict,
                                        &scratch.asym_query,
                                        rec.pos,
                                        rec.strong,
                                        meta,
                                    );
                                    meta.sm_poincare_proxy(q_radius, factor, cos_hat)
                                } else {
                                    self.eval_asym_sm_proxy_fast(
                                        space,
                                        query,
                                        &scratch.asym_query,
                                        q_radius,
                                        factor,
                                        node,
                                        &mut scratch.cone_cache,
                                        #[cfg(feature = "research")]
                                        strategy,
                                        trace_slow,
                                    )
                                };
                                scratch.gate_pool.push((Distance(proxy), node));
                            }
                        } else {
                            let push_gate =
                                |node: u32,
                                 visited: &mut SparseBitset,
                                 asym_q: &AsymQuerySm,
                                 cone_cache: &mut QueryConeCache,
                                 pool: &mut Vec<Candidate>| {
                                    let idx = node as usize;
                                    if visited.test(idx)
                                        || (!include_deleted && self.tombstones[idx])
                                    {
                                        return;
                                    }
                                    if !space.visit() {
                                        return;
                                    }
                                    visited.set(idx);
                                    let proxy = self.eval_asym_sm_proxy_fast(
                                        space,
                                        query,
                                        asym_q,
                                        q_radius,
                                        factor,
                                        node,
                                        cone_cache,
                                        #[cfg(feature = "research")]
                                        strategy,
                                        trace_slow,
                                    );
                                    pool.push((Distance(proxy), node));
                                };
                            for &(_, node) in scratch.apex_results.iter() {
                                push_gate(
                                    node,
                                    &mut scratch.visited,
                                    &scratch.asym_query,
                                    &mut scratch.cone_cache,
                                    &mut scratch.gate_pool,
                                );
                            }
                            for &(_, node) in scratch.horiz_results.iter() {
                                push_gate(
                                    node,
                                    &mut scratch.visited,
                                    &scratch.asym_query,
                                    &mut scratch.cone_cache,
                                    &mut scratch.gate_pool,
                                );
                            }
                            if keep_live_heap {
                                for &(_, node) in scratch.results.iter() {
                                    push_gate(
                                        node,
                                        &mut scratch.visited,
                                        &scratch.asym_query,
                                        &mut scratch.cone_cache,
                                        &mut scratch.gate_pool,
                                    );
                                }
                            }
                        }
                        #[cfg(feature = "research")]
                        if stage_prof {
                            STAGE_PROFILE.with(|p| {
                                p.borrow_mut().stage05_candidates = scratch.gate_pool.len();
                            });
                        }
                        if scratch.gate_pool.len() > ef {
                            scratch.gate_pool.select_nth_unstable(ef);
                            scratch.gate_pool.truncate(ef);
                        }
                        scratch.gate_pool.sort_unstable();
                        #[cfg(feature = "research")]
                        if let Some(t2) = t_gate_start {
                            STAGE_PROFILE.with(|p| {
                                p.borrow_mut().asym_gate_ns = t2.elapsed().as_nanos() as u64
                            });
                        }
                        if fast_kernels {
                            return std::mem::take(&mut scratch.gate_pool);
                        }
                        return scratch.gate_pool.clone();
                    }

                    let mut results = scratch.results.iter().copied().collect::<Vec<_>>();
                    results.sort_unstable();
                    return results;
                }

                // 分支 B：Exact 精确导航 —— Stage-0 单遍对称遍历 + Stage-0.5 非对称门户重排 + Stage-1 径向下界预滤
                scratch.apex_portals.clear();
                scratch.horiz_portals.clear();
                if factor <= 1.0 + 1e-12 {
                    if let Some(router) = &self.polar_router {
                        for &hub in &router.origin_hubs {
                            scratch.apex_portals.push((Distance(0.0), hub));
                        }
                    }
                } else {
                    let ef_sm = (ef * 5 / 4).clamp(64, 224);
                    let track_horiz = query_depth > horiz_gate;
                    let horiz_cap = 20usize;

                    scratch.visited.begin();
                    scratch.apex_candidates.clear();
                    scratch.apex_results.clear();
                    scratch.horiz_candidates.clear();
                    scratch.horiz_results.clear();

                    let visit_sm = |scratch: &mut Scratch, node: u32| {
                        let idx = node as usize;
                        if scratch.visited.test(idx) {
                            return;
                        }
                        if !space.visit() {
                            return;
                        }
                        scratch.visited.set(idx);
                        let (proxy, cos_hat, d_v) = self.eval_sm_node_fast(
                            space,
                            query,
                            &query_sig,
                            q_sm,
                            q_radius,
                            factor,
                            node,
                            Some(&mut scratch.cone_cache),
                            #[cfg(feature = "research")]
                            strategy,
                            trace_slow,
                        );
                        let cand = (Distance(proxy), node);
                        if scratch.apex_results.len() < ef_sm
                            || cand < *scratch.apex_results.peek().unwrap()
                        {
                            scratch.apex_candidates.push(Reverse(cand));
                            scratch.apex_results.push(cand);
                            if scratch.apex_results.len() > ef_sm {
                                scratch.apex_results.pop();
                            }
                        }
                        if track_horiz {
                            let ang_loss = cosine_to_angular_loss(cos_hat);
                            let shell_pen =
                                (0.20 * ANGULAR_SCALE * (query_depth - f64::from(d_v)).abs())
                                    .round() as u32;
                            let h_loss = ang_loss.saturating_add(shell_pen);
                            if scratch.horiz_results.len() < horiz_cap
                                || h_loss < scratch.horiz_results.peek().unwrap().0
                            {
                                scratch.horiz_candidates.push(Reverse((h_loss, node)));
                                scratch.horiz_results.push((h_loss, node));
                                if scratch.horiz_results.len() > horiz_cap {
                                    scratch.horiz_results.pop();
                                }
                            }
                        }
                    };

                    visit_sm(scratch, entry);
                    if let Some(router) = &self.polar_router {
                        let probe_max = if router.adaptive_shells && router.min_depth > 0.30 {
                            80
                        } else {
                            64
                        };
                        let probe_cones = (ef / 2).clamp(24, probe_max);
                        router.collect_focused_query_seeds(
                            &query_sig,
                            q_sm,
                            query_depth,
                            self.dim,
                            ef,
                            probe_cones,
                            two_level_cones,
                            traversal,
                            &mut scratch.cone_buf,
                            &mut scratch.router_seeds,
                        );
                        for i in 0..scratch.router_seeds.len() {
                            let seed = scratch.router_seeds[i];
                            if !scratch.visited.test(seed as usize) {
                                self.prefetch_native_direction(space, seed as usize);
                            }
                        }
                        for i in 0..scratch.router_seeds.len() {
                            let seed = scratch.router_seeds[i];
                            visit_sm(scratch, seed);
                        }
                    }

                    while let Some(Reverse(cand)) = scratch.apex_candidates.pop() {
                        if scratch.apex_results.len() >= ef_sm
                            && cand > *scratch.apex_results.peek().unwrap()
                        {
                            break;
                        }
                        let neighbors = self.native_layer0_neighbors_fast(cand.1, trace_slow);
                        for &neighbor in neighbors {
                            if !scratch.visited.test(neighbor as usize) {
                                self.prefetch_native_direction(space, neighbor as usize);
                            }
                        }
                        for &neighbor in neighbors {
                            visit_sm(scratch, neighbor);
                        }
                    }

                    if track_horiz {
                        let mut horiz_steps = 0usize;
                        while let Some(Reverse((hl, h_node))) = scratch.horiz_candidates.pop() {
                            if horiz_steps >= 28
                                || (scratch.horiz_results.len() >= horiz_cap
                                    && hl > scratch.horiz_results.peek().unwrap().0)
                            {
                                break;
                            }
                            horiz_steps += 1;
                            let neighbors = self.native_layer0_neighbors_fast(h_node, trace_slow);
                            for &neighbor in neighbors {
                                if !scratch.visited.test(neighbor as usize) {
                                    self.prefetch_native_direction(space, neighbor as usize);
                                }
                            }
                            for &neighbor in neighbors {
                                visit_sm(scratch, neighbor);
                            }
                        }
                        scratch
                            .horiz_portals
                            .extend(scratch.horiz_results.iter().copied());
                        scratch.horiz_portals.sort_unstable();
                    }

                    if use_asym_gate {
                        scratch.asym_query.encode_into(query, q_radius);
                        scratch.visited.begin();
                        for &(_, node) in scratch.apex_results.iter() {
                            let idx = node as usize;
                            if !scratch.visited.test(idx) {
                                if !space.visit() {
                                    return Vec::new();
                                }
                                scratch.visited.set(idx);
                                let proxy = self.eval_asym_sm_proxy_fast(
                                    space,
                                    query,
                                    &scratch.asym_query,
                                    q_radius,
                                    factor,
                                    node,
                                    &mut scratch.cone_cache,
                                    #[cfg(feature = "research")]
                                    strategy,
                                    trace_slow,
                                );
                                scratch.apex_portals.push((Distance(proxy), node));
                            }
                        }
                        for &(_, node) in scratch.horiz_results.iter() {
                            let idx = node as usize;
                            if !scratch.visited.test(idx) {
                                if !space.visit() {
                                    return Vec::new();
                                }
                                scratch.visited.set(idx);
                                let proxy = self.eval_asym_sm_proxy_fast(
                                    space,
                                    query,
                                    &scratch.asym_query,
                                    q_radius,
                                    factor,
                                    node,
                                    &mut scratch.cone_cache,
                                    #[cfg(feature = "research")]
                                    strategy,
                                    trace_slow,
                                );
                                scratch.apex_portals.push((Distance(proxy), node));
                            }
                        }
                    } else {
                        scratch
                            .apex_portals
                            .extend(scratch.apex_results.iter().copied());
                    }
                    scratch.apex_portals.sort_unstable();
                }

                // Stage 1：精炼门户注入 + 保守径向径向 Cauchy-Schwarz 下界预滤 + 批量冷存预取
                scratch.visited.begin();
                scratch.candidates.clear();
                scratch.results.clear();

                let router = self.polar_router.as_ref();

                let push_exact = |scratch: &mut Scratch, node: u32| {
                    let slot = self.slot_indices[node as usize];
                    let next = (Distance(space.proxy(query, factor, slot)), node);
                    if scratch.results.len() < ef || next < *scratch.results.peek().unwrap() {
                        scratch.candidates.push(Reverse(next));
                        if include_deleted || !self.tombstones[node as usize] {
                            scratch.results.push(next);
                            if scratch.results.len() > ef {
                                scratch.results.pop();
                            }
                        }
                    }
                };

                if !space.visit() {
                    return Vec::new();
                }
                scratch.visited.set(entry as usize);
                push_exact(scratch, entry);

                let apex_inject = (ef * 3 / 4).clamp(32, 96).min(scratch.apex_portals.len());
                let apex_hop = (if a8_search_budget() { 3usize } else { 2usize }).min(apex_inject);
                let horiz_inject = (ef / 4).clamp(8, 24).min(scratch.horiz_portals.len());
                scratch.survivors.clear();
                for i in 0..apex_inject {
                    let p = scratch.apex_portals[i].1;
                    if !scratch.visited.test(p as usize) {
                        if !space.visit() {
                            return Vec::new();
                        }
                        scratch.visited.set(p as usize);
                        scratch.survivors.push(p);
                    }
                    if i < apex_hop {
                        let neighbors = self.native_neighbors(p, 0);
                        for &nb in neighbors.iter().take(8) {
                            if !scratch.visited.test(nb as usize) {
                                if !space.visit() {
                                    return Vec::new();
                                }
                                scratch.visited.set(nb as usize);
                                scratch.survivors.push(nb);
                            }
                        }
                    }
                }
                for i in 0..horiz_inject {
                    let p = scratch.horiz_portals[i].1;
                    if !scratch.visited.test(p as usize) {
                        if !space.visit() {
                            return Vec::new();
                        }
                        scratch.visited.set(p as usize);
                        scratch.survivors.push(p);
                    }
                }
                for i in 0..scratch.survivors.len() {
                    let p = scratch.survivors[i];
                    if scratch.results.len() >= ef
                        && let Some(meta) = router.and_then(|r| r.node_meta(p as usize))
                        && Distance(meta.radial_lower_bound(q_radius, factor))
                            > scratch.results.peek().unwrap().0
                    {
                        continue;
                    }
                    if i + 2 < scratch.survivors.len() {
                        let ahead = scratch.survivors[i + 2];
                        space.prefetch_vector(self.slot_indices[ahead as usize]);
                    }
                    push_exact(scratch, p);
                }

                while let Some(Reverse(candidate)) = scratch.candidates.pop() {
                    if scratch.results.len() >= ef && candidate > *scratch.results.peek().unwrap() {
                        break;
                    }
                    let worst_ef = if scratch.results.len() >= ef {
                        Some(scratch.results.peek().unwrap().0)
                    } else {
                        None
                    };
                    let neighbors = self.native_neighbors(candidate.1, 0);
                    scratch.survivors.clear();
                    for &nb in neighbors {
                        let idx = nb as usize;
                        if scratch.visited.test(idx) {
                            continue;
                        }
                        if !space.visit() {
                            return Vec::new();
                        }
                        scratch.visited.set(idx);
                        if let Some(bound) = worst_ef
                            && let Some(meta) = router.and_then(|r| r.node_meta(idx))
                            && Distance(meta.radial_lower_bound(q_radius, factor)) > bound
                        {
                            continue;
                        }
                        space.prefetch_vector(self.slot_indices[idx]);
                        scratch.survivors.push(nb);
                    }
                    for i in 0..scratch.survivors.len() {
                        let nb = scratch.survivors[i];
                        push_exact(scratch, nb);
                    }
                }

                let mut results = scratch.results.iter().copied().collect::<Vec<_>>();
                results.sort_unstable();
                results
            });
        }

        // ═══════════════════════════════════════════════════════════════════════
        //  高层图搜索（layer > 0）及历史代际消融复现路径（A0..A6）
        // ═══════════════════════════════════════════════════════════════════════
        let query_sig = Bq2Signature::from_vector(query);
        let q_inv_qnorm = bq2_inv_qnorm(self.dim, query_sig.strong_ones());
        let query_depth = dimensionless_depth(factor);
        let cosh_dq = query_depth.cosh();
        let sinh_dq = query_depth.sinh();
        let encoded = quantized.then(|| {
            let code = GeometryCode::new(query, factor, &query_sig);
            (query_sig.clone(), code)
        });
        let score = |node| self.native_query_proxy(space, query, factor, encoded.as_ref(), node);

        #[cfg(feature = "research")]
        let enable_stage0 = !matches!(
            strategy,
            HyperbolicStrategy::GeodesicPruning
                | HyperbolicStrategy::GeodesicPruningWithFill
                | HyperbolicStrategy::DualFiber
        );
        #[cfg(not(feature = "research"))]
        let enable_stage0 = true;

        #[cfg(feature = "research")]
        let use_calibrated_apex = matches!(strategy, HyperbolicStrategy::CalibratedCompass);
        #[cfg(not(feature = "research"))]
        let use_calibrated_apex = false;

        #[cfg(feature = "research")]
        let use_router = matches!(
            strategy,
            HyperbolicStrategy::CalibratedCompass | HyperbolicStrategy::PolarRouting
        );
        #[cfg(not(feature = "research"))]
        let use_router = true;

        let angular_eval = |node: u32| -> (u32, f64) {
            #[cfg(feature = "research")]
            trace_query(|t| {
                t.scored.insert(node);
                t.approximate_evals += 1;
            });
            if !use_calibrated_apex {
                let raw = self
                    .bq_store
                    .distance_to_sig(node as usize, &query_sig, self.dim);
                return (raw, 0.0);
            }
            let bq_d = self
                .bq_store
                .distance_to_sig(node as usize, &query_sig, self.dim);
            let node_inv = self
                .polar_router
                .as_ref()
                .and_then(|r| {
                    r.node_meta(node as usize)
                        .map(|m| m.legacy_inv_qnorm(self.dim))
                })
                .unwrap_or_else(|| {
                    bq2_inv_qnorm(self.dim, self.bq_store.strong_ones(node as usize))
                });
            let cos_hat = calibrated_cosine_from_bq(bq_d, self.dim, q_inv_qnorm, node_inv);
            (cosine_to_angular_loss(cos_hat), cos_hat)
        };

        let polar_apex_proxy = |node: u32, cos_hat: f64| -> f64 {
            let d_v = if let Some(router) = &self.polar_router
                && let Some(meta) = router.node_meta(node as usize)
            {
                f64::from(meta.depth)
            } else {
                dimensionless_depth(space.boundary_factors[self.slot_indices[node as usize]])
            };
            let sv = d_v.sinh();
            let cv = (1.0 + sv * sv).sqrt();
            (cosh_dq * cv - sinh_dq * sv * cos_hat).max(1.0)
        };

        SCRATCH.with(|cell| {
            let mut scratch = cell.borrow_mut();
            let scratch = &mut *scratch;
            scratch.visited.grow(self.n);

            scratch.horiz_portals.clear();
            scratch.apex_portals.clear();
            if enable_stage0 && layer == 0 && self.n > 1 {
                if factor <= 1.0 + 1e-12 {
                    if use_router && let Some(router) = &self.polar_router {
                        for &hub in &router.origin_hubs {
                            scratch.horiz_portals.push((0, hub));
                        }
                    }
                } else if use_calibrated_apex {
                    scratch.router_seeds.clear();
                    if use_router && let Some(router) = &self.polar_router {
                        let probe_cones = (ef / 2).clamp(20, 48);
                        router.collect_query_seeds(
                            &query_sig,
                            q_inv_qnorm,
                            self.dim,
                            probe_cones,
                            &mut scratch.cone_buf,
                            &mut scratch.router_seeds,
                        );
                    }

                    let ef_apex = ef.clamp(48, 160);
                    scratch.visited.begin();
                    scratch.apex_candidates.clear();
                    scratch.apex_results.clear();

                    let visit_apex = |scratch: &mut Scratch, node: u32| {
                        if scratch.visited.test(node as usize) {
                            return;
                        }
                        if !space.visit() {
                            return;
                        }
                        scratch.visited.set(node as usize);
                        let (_, cos_hat) = angular_eval(node);
                        let apex_cand = (Distance(polar_apex_proxy(node, cos_hat)), node);
                        if scratch.apex_results.len() < ef_apex
                            || apex_cand < *scratch.apex_results.peek().unwrap()
                        {
                            scratch.apex_candidates.push(Reverse(apex_cand));
                            scratch.apex_results.push(apex_cand);
                            if scratch.apex_results.len() > ef_apex {
                                scratch.apex_results.pop();
                            }
                        }
                    };

                    visit_apex(scratch, entry);
                    for i in 0..scratch.router_seeds.len() {
                        let seed = scratch.router_seeds[i];
                        if !scratch.visited.test(seed as usize) {
                            self.prefetch_native_direction(space, seed as usize);
                        }
                    }
                    for i in 0..scratch.router_seeds.len() {
                        let seed = scratch.router_seeds[i];
                        visit_apex(scratch, seed);
                    }

                    while let Some(Reverse(cand)) = scratch.apex_candidates.pop() {
                        if scratch.apex_results.len() >= ef_apex
                            && cand > *scratch.apex_results.peek().unwrap()
                        {
                            break;
                        }
                        let neighbors = self.native_neighbors(cand.1, 0);
                        for &neighbor in neighbors {
                            if !scratch.visited.test(neighbor as usize) {
                                self.prefetch_native_direction(space, neighbor as usize);
                            }
                        }
                        for &neighbor in neighbors {
                            visit_apex(scratch, neighbor);
                        }
                    }
                    scratch
                        .apex_portals
                        .extend(scratch.apex_results.iter().copied());
                    scratch.apex_portals.sort_unstable();

                    if query_depth > 0.85 {
                        let ef_horiz = (ef / 2).clamp(20, 64);
                        scratch.visited.begin();
                        scratch.horiz_candidates.clear();
                        scratch.horiz_results.clear();

                        let visit_horiz = |scratch: &mut Scratch, node: u32| {
                            if scratch.visited.test(node as usize) {
                                return;
                            }
                            if !space.visit() {
                                return;
                            }
                            scratch.visited.set(node as usize);
                            let (ang_loss, _) = angular_eval(node);
                            let shell_pen = if let Some(router) = &self.polar_router
                                && let Some(meta) = router.node_meta(node as usize)
                            {
                                (0.20 * ANGULAR_SCALE * (query_depth - f64::from(meta.depth)).abs())
                                    .round() as u32
                            } else {
                                0
                            };
                            let loss = ang_loss.saturating_add(shell_pen);
                            if scratch.horiz_results.len() < ef_horiz
                                || loss < scratch.horiz_results.peek().unwrap().0
                            {
                                scratch.horiz_candidates.push(Reverse((loss, node)));
                                scratch.horiz_results.push((loss, node));
                                if scratch.horiz_results.len() > ef_horiz {
                                    scratch.horiz_results.pop();
                                }
                            }
                        };

                        visit_horiz(scratch, entry);
                        for i in 0..scratch.router_seeds.len() {
                            let seed = scratch.router_seeds[i];
                            visit_horiz(scratch, seed);
                        }
                        while let Some(Reverse((cd, cur))) = scratch.horiz_candidates.pop() {
                            if scratch.horiz_results.len() >= ef_horiz
                                && cd > scratch.horiz_results.peek().unwrap().0
                            {
                                break;
                            }
                            let neighbors = self.native_neighbors(cur, 0);
                            for &neighbor in neighbors {
                                if !scratch.visited.test(neighbor as usize) {
                                    self.prefetch_native_direction(space, neighbor as usize);
                                }
                            }
                            for &neighbor in neighbors {
                                visit_horiz(scratch, neighbor);
                            }
                        }
                        scratch
                            .horiz_portals
                            .extend(scratch.horiz_results.iter().copied());
                        scratch.horiz_portals.sort_unstable();
                    }
                } else {
                    let ef_horiz = (ef / 2).clamp(24, 96);
                    scratch.visited.begin();
                    scratch.horiz_candidates.clear();
                    scratch.horiz_results.clear();

                    let visit_stage0 = |scratch: &mut Scratch, node: u32| {
                        if scratch.visited.test(node as usize) {
                            return;
                        }
                        if !space.visit() {
                            return;
                        }
                        scratch.visited.set(node as usize);
                        let (ang_loss, _) = angular_eval(node);
                        if scratch.horiz_results.len() < ef_horiz
                            || ang_loss < scratch.horiz_results.peek().unwrap().0
                        {
                            scratch.horiz_candidates.push(Reverse((ang_loss, node)));
                            scratch.horiz_results.push((ang_loss, node));
                            if scratch.horiz_results.len() > ef_horiz {
                                scratch.horiz_results.pop();
                            }
                        }
                    };

                    visit_stage0(scratch, entry);
                    if use_router && let Some(router) = &self.polar_router {
                        let probe_cones = (ef / 4).clamp(12, 32);
                        router.collect_query_seeds(
                            &query_sig,
                            q_inv_qnorm,
                            self.dim,
                            probe_cones,
                            &mut scratch.cone_buf,
                            &mut scratch.router_seeds,
                        );
                        for i in 0..scratch.router_seeds.len() {
                            let seed = scratch.router_seeds[i];
                            if !scratch.visited.test(seed as usize) {
                                self.prefetch_native_direction(space, seed as usize);
                            }
                        }
                        for i in 0..scratch.router_seeds.len() {
                            let seed = scratch.router_seeds[i];
                            visit_stage0(scratch, seed);
                        }
                    }

                    while let Some(Reverse((cd, cur))) = scratch.horiz_candidates.pop() {
                        if scratch.horiz_results.len() >= ef_horiz
                            && cd > scratch.horiz_results.peek().unwrap().0
                        {
                            break;
                        }
                        let neighbors = self.native_neighbors(cur, 0);
                        for &neighbor in neighbors {
                            if !scratch.visited.test(neighbor as usize) {
                                self.prefetch_native_direction(space, neighbor as usize);
                            }
                        }
                        for &neighbor in neighbors {
                            visit_stage0(scratch, neighbor);
                        }
                    }

                    scratch
                        .horiz_portals
                        .extend(scratch.horiz_results.iter().copied());
                    scratch.horiz_portals.sort_unstable();
                }
            }

            scratch.visited.begin();
            scratch.candidates.clear();
            scratch.results.clear();

            let consider = |scratch: &mut Scratch, node: u32| {
                if scratch.visited.test(node as usize) {
                    return;
                }
                if !space.visit() {
                    return;
                }
                scratch.visited.set(node as usize);
                let next = (Distance(score(node)), node);
                if scratch.results.len() < ef || next < *scratch.results.peek().unwrap() {
                    scratch.candidates.push(Reverse(next));
                    if include_deleted || !self.tombstones[node as usize] {
                        scratch.results.push(next);
                        if scratch.results.len() > ef {
                            scratch.results.pop();
                        }
                    }
                }
            };

            consider(scratch, entry);

            if use_calibrated_apex {
                let apex_inject = (ef * 3 / 4).clamp(24, 72).min(scratch.apex_portals.len());
                let apex_hop = 2usize.min(apex_inject);
                for i in 0..apex_inject {
                    let portal = scratch.apex_portals[i].1;
                    consider(scratch, portal);
                    if i < apex_hop {
                        let neighbors = self.native_neighbors(portal, 0);
                        let local_take = neighbors.len().min(8);
                        if quantized {
                            for &neighbor in &neighbors[..local_take] {
                                self.prefetch_native_direction(space, neighbor as usize);
                            }
                        }
                        for &neighbor in &neighbors[..local_take] {
                            consider(scratch, neighbor);
                        }
                    }
                }
                let horiz_inject = (ef / 3).clamp(8, 24).min(scratch.horiz_portals.len());
                for i in 0..horiz_inject {
                    let portal = scratch.horiz_portals[i].1;
                    consider(scratch, portal);
                }
            } else if enable_stage0 && layer == 0 {
                #[cfg(feature = "research")]
                let portal_expand = if strategy == HyperbolicStrategy::DualFiberPortals {
                    0
                } else {
                    (ef / 4).clamp(8, 24).min(scratch.horiz_portals.len())
                };
                #[cfg(not(feature = "research"))]
                let portal_expand = (ef / 4).clamp(8, 24).min(scratch.horiz_portals.len());

                for i in 0..portal_expand {
                    let portal = scratch.horiz_portals[i].1;
                    consider(scratch, portal);
                    let neighbors = self.native_neighbors(portal, 0);
                    let geo_take = (neighbors.len() * 5).div_ceil(8).min(neighbors.len());
                    if quantized {
                        for &neighbor in &neighbors[..geo_take] {
                            self.prefetch_native_direction(space, neighbor as usize);
                        }
                    }
                    for &neighbor in &neighbors[..geo_take] {
                        consider(scratch, neighbor);
                    }
                }
                for i in portal_expand..scratch.horiz_portals.len() {
                    consider(scratch, scratch.horiz_portals[i].1);
                }
            }

            while let Some(Reverse(candidate)) = scratch.candidates.pop() {
                if scratch.results.len() >= ef && candidate > *scratch.results.peek().unwrap() {
                    break;
                }
                let neighbors = self.native_neighbors(candidate.1, layer);
                if quantized {
                    for &neighbor in neighbors {
                        self.prefetch_native_direction(space, neighbor as usize);
                    }
                }
                for &neighbor in neighbors {
                    consider(scratch, neighbor);
                }
            }
            let mut results = scratch.results.iter().copied().collect::<Vec<_>>();
            results.sort_unstable();
            results
        })
    }
}
