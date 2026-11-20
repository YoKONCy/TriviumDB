//! 并行构图调度、邻居选择与连通修复。

use super::*;
impl QuIVer {
    /// ═══ 并行批量构建（rayon 加速）═══
    ///
    /// 利用 rayon 并行计算 BQ 签名，预分配所有数据结构，
    /// 然后顺序插入建图。相比逐个 insert 减少了：
    ///   - BQ 签名计算时间（并行化）
    ///   - 动态扩容开销（一次性预分配）
    ///   - Visited bitset 重复分配（复用）
    pub(crate) fn batch_build(&mut self, vectors: &[f32], ids: &[u64], slot_idxs: &[usize]) {
        let n = ids.len();
        let dim = self.dim;
        assert_eq!(vectors.len(), n * dim);
        assert_eq!(slot_idxs.len(), n);

        // ── Phase 1: 并行计算 BQ 签名 ──
        let sigs: Vec<Bq2Signature> = vectors
            .par_chunks(dim)
            .map(Bq2Signature::from_vector)
            .collect();

        // ── Phase 2: 预分配所有数据结构 ──
        self.bq_store.reserve(n);
        self.ids.reserve(n);
        self.slot_indices.reserve(n);
        self.tombstones.reserve(n);
        self.node_max_layer.reserve(n);

        // 预先计算所有层级
        let mut lcg: u64 = 12345;
        let mut levels = Vec::with_capacity(n);
        for _ in 0..n {
            levels.push(self.random_level(&mut lcg));
        }
        let _max_level = *levels.iter().max().unwrap_or(&0);

        // 预分配 Layer 0
        self.layer0.data.reserve(n * self.layer0.stride);

        // ── Phase 3: 逐个插入建图 ──
        // 使用预计算的签名，避免重复计算
        let mut visited = Bitset::new(0);
        for i in 0..n {
            let _v = &vectors[i * dim..(i + 1) * dim];
            let idx = self.n as u32;

            // 直接使用预计算的签名
            self.bq_store.push_sig(&sigs[i]);
            self.ids.push(ids[i]);
            self.slot_indices.push(slot_idxs[i]);
            self.id_to_internal.insert(ids[i], idx);
            self.tombstones.push(false);

            let level = levels[i];
            self.node_max_layer.push(level as u8);

            self.layer0.push_empty();
            while self.upper_layers.len() < level {
                self.upper_layers.push(vec![Vec::new(); self.n]);
            }
            for ul in self.upper_layers.iter_mut() {
                ul.push(Vec::new());
            }

            self.n += 1;
            visited.grow(self.n);

            if self.n == 1 {
                self.entry_point = 0;
                self.max_level = level;
                continue;
            }

            let my_sig = self.bq_store.get_sig(idx as usize);
            let mut cur_node = self.entry_point;

            // 高层贪心下降（BQ 距离）
            for l in ((level + 1)..=self.max_level).rev() {
                let ul_idx = l - 1;
                if ul_idx < self.upper_layers.len() {
                    loop {
                        let mut changed = false;
                        let cur_d = self.bq_store.distance(idx as usize, cur_node as usize, dim);
                        let mut best_d = cur_d;
                        for &nb in &self.upper_layers[ul_idx][cur_node as usize] {
                            let nd = self.bq_store.distance(idx as usize, nb as usize, dim);
                            if nd < best_d {
                                cur_node = nb;
                                best_d = nd;
                                changed = true;
                            }
                        }
                        if !changed {
                            break;
                        }
                    }
                }
            }

            // 高层搜索
            for l in (level..=self.max_level).rev() {
                let res = self.beam_search_upper(&my_sig, l, cur_node, 1, &mut visited);
                if !res.is_empty() {
                    cur_node = res[0].1;
                }
            }

            // 各层插入
            for l in (0..=level).rev() {
                let ef = self.ef_construction;
                let candidates_sym = if l == 0 {
                    self.beam_search_l0(&my_sig, cur_node, ef, &mut visited)
                } else {
                    self.beam_search_upper(&my_sig, l, cur_node, ef, &mut visited)
                };
                let candidates: Vec<(u32, u32)> = candidates_sym
                    .into_iter()
                    .map(|(_, id)| (self.bq_store.distance(idx as usize, id as usize, dim), id))
                    .collect();

                let max_nb = if l == 0 { self.m0 } else { self.m };
                let selected = self.select_neighbors(idx, &candidates, max_nb);

                if l == 0 {
                    for &nb in &selected {
                        if !self.layer0.contains(idx, nb) {
                            self.layer0.push_neighbor(idx, nb);
                        }
                    }
                    // Vamana 双向剪枝
                    for &nb in &selected {
                        let mut nb_candidates: Vec<(u32, u32)> = self
                            .layer0
                            .neighbors(nb)
                            .iter()
                            .map(|&n| (self.bq_store.distance(nb as usize, n as usize, dim), n))
                            .collect();
                        if !self.layer0.contains(nb, idx) {
                            let d = self.bq_store.distance(nb as usize, idx as usize, dim);
                            nb_candidates.push((d, idx));
                        }
                        nb_candidates.sort_unstable_by_key(|&(d, _)| d);
                        let pruned = Self::vamana_select(
                            &self.bq_store,
                            nb,
                            &nb_candidates,
                            max_nb,
                            dim,
                            self.alpha,
                        );
                        self.layer0.set_neighbors(nb, &pruned);
                    }
                } else {
                    let ul = l - 1;
                    for &nb in &selected {
                        if !self.upper_layers[ul][idx as usize].contains(&nb) {
                            self.upper_layers[ul][idx as usize].push(nb);
                        }
                        if !self.upper_layers[ul][nb as usize].contains(&idx) {
                            self.upper_layers[ul][nb as usize].push(idx);
                        }
                    }
                }

                if !candidates.is_empty() {
                    cur_node = candidates[0].1;
                }
            }

            if level > self.max_level {
                self.entry_point = idx;
                self.max_level = level;
            }
        }
    }

    pub(crate) fn build_parallel_checked(
        &mut self,
        vectors: &[f32],
        ids: &[u64],
        slot_idxs: &[usize],
    ) {
        self.build_parallel_impl(vectors, ids, slot_idxs, true, None, false, false);
    }

    pub(crate) fn build_parallel(&mut self, vectors: &[f32], ids: &[u64], slot_idxs: &[usize]) {
        self.build_parallel_impl(vectors, ids, slot_idxs, false, None, false, false);
    }

    pub(crate) fn batch_build_native(
        &mut self,
        space: &NativeSpace<'_>,
        ids: &[u64],
        slots: &[usize],
        quantized: bool,
    ) {
        self.batch_build_native_with_residual(space, ids, slots, quantized, false);
    }

    pub(crate) fn batch_build_native_with_residual(
        &mut self,
        space: &NativeSpace<'_>,
        ids: &[u64],
        slots: &[usize],
        quantized: bool,
        force_residual: bool,
    ) {
        self.build_parallel_impl(
            space.vectors,
            ids,
            slots,
            false,
            Some(space),
            quantized,
            force_residual,
        );
    }

    /// 使用外部流式编码得到的 BQ2 签名构建图，不保留全量 FP32 向量副本。
    pub(crate) fn batch_build_from_store(
        &mut self,
        ids: &[u64],
        slot_idxs: &[usize],
        store: Bq2Store,
    ) {
        let n = ids.len();
        let dim = self.dim;
        assert_eq!(slot_idxs.len(), n);
        assert_eq!(store.len(), n, "Bq2Store 长度必须与 ids 长度一致");
        if n == 0 {
            return;
        }

        // 直接使用外部 store，不做内部编码
        let mut lcg: u64 = 12345;
        let mut levels = Vec::with_capacity(n);
        for _ in 0..n {
            levels.push(self.random_level(&mut lcg) as u8);
        }

        self.n = n;
        self.bq_store = store;
        self.quantized_native = false;
        self.geometry_codes.clear();
        #[cfg(feature = "research")]
        {
            self.direction_store = None;
        }
        self.polar_router = None;
        self.ids.clear();
        self.ids.extend_from_slice(ids);
        self.slot_indices.clear();
        self.slot_indices.extend_from_slice(slot_idxs);
        self.id_to_internal.clear();
        for (i, &id) in ids.iter().enumerate() {
            self.id_to_internal.insert(id, i as u32);
        }
        self.tombstones = vec![false; n];
        self.dirty_count = 0;
        self.node_max_layer = levels;
        self.entry_point = 0;
        self.max_level = 0;
        self.upper_layers.clear();
        self.visited = Bitset::new(n);
        self.layer0.reset_full(n);

        let locks = StripedSpinLocks::new(n, self.m0);
        let concurrent_adj = ConcurrentFlatAdj::new(n, self.layer0.stride);
        let profile = BuildProfile::new();
        let view = ParallelBuildContext {
            adj: &concurrent_adj,
            n,
            dim,
            m0: self.m0,
            ef: self.ef_construction,
            alpha: self.alpha,
            entry_point: 0,
            sigs: &self.bq_store,
            locks: &locks,
            profile: &profile,
            eager: crate::index::pelorus::engine::config::environment_flag(
                "QUIVER_EAGER_PRUNE",
                "TRIVIUM_EAGER_PRUNE",
            ),
            quantized_build: false,
            native_space: None,
            geometry_codes: None,
            polar_router: None,
            #[cfg(feature = "research")]
            direction_store: None,
            node_top_cones: None,
            build_residual: None,
        };

        let t_build = Instant::now();
        let chunk = 256usize;
        let rounds = n.div_ceil(chunk);
        for round in 0..rounds {
            let start = round * chunk;
            let end = ((round + 1) * chunk).min(n);
            (start..end).into_par_iter().for_each(|i| {
                let prof = profile.enabled;
                let t_bs = if prof { Some(Instant::now()) } else { None };
                let mut visited = Bitset::new(n);
                if let Some(t) = t_bs {
                    profile.add(&profile.bitset_ns, t.elapsed().as_nanos() as u64);
                }
                view.connect_node_fast(i as u32, &mut visited);
            });
        }
        // final-prune：把惰性追加导致 deg>m0 的节点并行收敛到 m0
        (0..n as u32)
            .into_par_iter()
            .for_each(|node| view.final_prune(node));
        profile.report(
            t_build.elapsed().as_secs_f64(),
            rayon::current_num_threads(),
        );

        concurrent_adj.freeze_into_flat(&mut self.layer0);
    }

    /// 使用外部预构建签名的实验兼容入口。
    #[cfg(feature = "research")]
    pub fn batch_build_with_store(
        &mut self,
        _vectors: &[f32],
        ids: &[u64],
        slot_idxs: &[usize],
        store: Bq2Store,
    ) {
        self.batch_build_from_store(ids, slot_idxs, store);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_parallel_impl(
        &mut self,
        vectors: &[f32],
        ids: &[u64],
        slot_idxs: &[usize],
        checked: bool,
        native_space: Option<&NativeSpace<'_>>,
        quantized: bool,
        force_residual: bool,
    ) {
        let n = ids.len();
        let dim = self.dim;
        assert_eq!(vectors.len(), n * dim);
        assert_eq!(slot_idxs.len(), n);
        self.quantized_native = quantized;
        if n == 0 {
            return;
        }

        // 大规模数据集上零拷贝并行打包 BQ2 位平面
        let store = Bq2Store::from_vectors_parallel(vectors, dim);

        #[cfg(feature = "research")]
        let strategy = hyperbolic::hyperbolic_strategy();
        #[cfg(feature = "research")]
        let enable_polar_router = native_space.is_some()
            && matches!(
                strategy,
                hyperbolic::HyperbolicStrategy::Full
                    | hyperbolic::HyperbolicStrategy::PolarRouting
                    | hyperbolic::HyperbolicStrategy::CalibratedCompass
                    | hyperbolic::HyperbolicStrategy::FixedShellCompass
                    | hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                    | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
            );
        #[cfg(not(feature = "research"))]
        let enable_polar_router = native_space.is_some();

        #[cfg(feature = "research")]
        let use_sm_router = matches!(
            strategy,
            hyperbolic::HyperbolicStrategy::Full
                | hyperbolic::HyperbolicStrategy::FixedShellCompass
                | hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
        );
        #[cfg(not(feature = "research"))]
        let use_sm_router = true;

        #[cfg(feature = "research")]
        let adaptive_shells = matches!(
            strategy,
            hyperbolic::HyperbolicStrategy::Full
                | hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
        );
        #[cfg(not(feature = "research"))]
        let adaptive_shells = true;

        #[cfg(feature = "research")]
        let adaptive_shells = adaptive_shells && hyperbolic::geometry_components().adaptive_shells;
        // A8 启用自适应极坐标路由与 SM 参数后，PolarNodeMeta 已承载完整径向与 SM 元数据，
        // 不再重复分配 32 B/vec 的 GeometryCode（在百万级数据集上直接节省 32 MB 常驻热内存）。
        let need_geometry_codes = quantized && !(enable_polar_router && use_sm_router);
        self.geometry_codes = if need_geometry_codes {
            let space = native_space.expect("几何量化需要双曲空间");
            if n >= 2048 {
                vectors
                    .par_chunks_exact(dim)
                    .enumerate()
                    .map(|(slot, vector)| {
                        GeometryCode::from_store(vector, space.boundary_factors[slot], &store, slot)
                    })
                    .collect()
            } else {
                vectors
                    .chunks_exact(dim)
                    .enumerate()
                    .map(|(slot, vector)| {
                        GeometryCode::from_store(vector, space.boundary_factors[slot], &store, slot)
                    })
                    .collect()
            }
        } else {
            Vec::new()
        };

        let mut lcg: u64 = 12345;
        #[cfg(feature = "research")]
        {
            lcg = std::env::var("QUIVER_BUILD_SEED")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(lcg);
        }
        let mut levels = Vec::with_capacity(n);
        for _ in 0..n {
            levels.push(self.random_level(&mut lcg) as u8);
        }

        self.n = n;
        self.bq_store = store;
        self.ids.clear();
        self.ids.extend_from_slice(ids);
        self.slot_indices.clear();
        self.slot_indices.extend_from_slice(slot_idxs);
        self.id_to_internal.clear();
        for (i, &id) in ids.iter().enumerate() {
            self.id_to_internal.insert(id, i as u32);
        }
        self.tombstones = vec![false; n];
        self.dirty_count = 0;
        self.node_max_layer = levels;
        self.entry_point = 0;
        self.max_level = 0;
        self.upper_layers.clear();
        self.visited = Bitset::new(n);
        self.layer0.reset_full(n);

        #[cfg(feature = "research")]
        {
            self.direction_store = if matches!(
                strategy,
                hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection
                    | hyperbolic::HyperbolicStrategy::FullPrecisionDirection
            ) {
                Some(
                    crate::index::pelorus::engine::direction::DirectionStore::build(
                        vectors,
                        dim,
                        strategy == hyperbolic::HyperbolicStrategy::ScalarQuantizedDirection,
                    ),
                )
            } else {
                None
            };
        }
        let router_started = Instant::now();
        let (node_top_cones, build_residual) = if enable_polar_router {
            let space = native_space.unwrap();
            let (router, cones, build_res) = hyperbolic::PolarRouter::build(
                space,
                &self.bq_store,
                &self.slot_indices,
                dim,
                use_sm_router,
                adaptive_shells,
                quantized && force_residual,
            );
            self.entry_point = router.entry_hub();
            self.polar_router = Some(router);
            (Some(cones), build_res)
        } else {
            self.polar_router = None;
            (None, None)
        };

        let router_seconds = router_started.elapsed().as_secs_f64();
        // 双曲极坐标模式下按测地半径由内向外调度并行构图：先织密球心测地主干，再向外层语义叶子自然生长
        let build_order: Option<Vec<u32>> = if let Some(router) = &self.polar_router {
            let mut order: Vec<u32> = (0..n as u32).collect();
            order.sort_unstable_by(|&a, &b| {
                router.node_meta[a as usize]
                    .depth
                    .total_cmp(&router.node_meta[b as usize].depth)
                    .then_with(|| a.cmp(&b))
            });
            Some(order)
        } else {
            None
        };

        let locks = StripedSpinLocks::new(n, self.m0);
        let concurrent_adj = ConcurrentFlatAdj::new(n, self.layer0.stride);
        let profile = BuildProfile::new();
        let view = ParallelBuildContext {
            adj: &concurrent_adj,
            n,
            dim,
            m0: self.m0,
            ef: self.ef_construction,
            alpha: self.alpha,
            entry_point: self.entry_point,
            sigs: &self.bq_store,
            locks: &locks,
            profile: &profile,
            eager: crate::index::pelorus::engine::config::environment_flag(
                "QUIVER_EAGER_PRUNE",
                "TRIVIUM_EAGER_PRUNE",
            ),
            quantized_build: quantized,
            native_space,
            geometry_codes: (!self.geometry_codes.is_empty())
                .then_some(self.geometry_codes.as_slice()),
            polar_router: self.polar_router.as_ref(),
            #[cfg(feature = "research")]
            direction_store: self.direction_store.as_ref(),
            node_top_cones: node_top_cones.as_deref(),
            build_residual: build_residual.as_ref(),
        };

        let t_build = Instant::now();
        let chunk = 256usize;
        let rounds = n.div_ceil(chunk);
        for round in 0..rounds {
            let start = round * chunk;
            let end = ((round + 1) * chunk).min(n);
            (start..end).into_par_iter().for_each(|pos| {
                let node_idx = build_order
                    .as_ref()
                    .map_or(pos as u32, |ord| ord[pos]);
                let prof = profile.enabled;
                let t_bs = if prof { Some(Instant::now()) } else { None };
                if native_space.is_some() {
                    // 原生距离分支复用工作线程的稀疏访问位图，避免每节点清空百万位。
                    thread_local! {
                        static BUILD_VISITED: std::cell::RefCell<SparseBitset> = std::cell::RefCell::new(SparseBitset::new(0));
                    }
                    BUILD_VISITED.with(|cell| {
                        let mut visited = cell.borrow_mut();
                        visited.grow(n);
                        if let Some(t) = t_bs {
                            profile.add(&profile.bitset_ns, t.elapsed().as_nanos() as u64);
                        }
                        view.connect_node_fast(node_idx, &mut *visited);
                    });
                } else {
                    let mut visited = Bitset::new(n);
                    if let Some(t) = t_bs {
                        profile.add(&profile.bitset_ns, t.elapsed().as_nanos() as u64);
                    }
                    if checked {
                        view.connect_node_checked(node_idx, &mut visited);
                    } else {
                        view.connect_node_fast(node_idx, &mut visited);
                    }
                }
            });
        }
        let insertion_seconds = t_build.elapsed().as_secs_f64();
        let prune_started = Instant::now();
        // final-prune：fast 路径惰性追加后收敛到 m0；checked 路径 deg≤m0 时为空操作
        (0..n as u32)
            .into_par_iter()
            .for_each(|node| view.final_prune(node));
        profile.report(
            t_build.elapsed().as_secs_f64(),
            rayon::current_num_threads(),
        );

        let graph_seconds = t_build.elapsed().as_secs_f64();
        let finalize_started = Instant::now();
        concurrent_adj.freeze_into_flat(&mut self.layer0);
        if enable_polar_router
            && use_sm_router
            && n > 1
            && hyperbolic::connectivity_repair_enabled()
            && {
                #[cfg(feature = "research")]
                {
                    strategy != hyperbolic::HyperbolicStrategy::FixedShellCompass
                }
                #[cfg(not(feature = "research"))]
                {
                    true
                }
            }
            && let Some(order) = build_order.as_deref()
        {
            self.wire_unreachable_native_nodes(order);
        }
        // 构图拓扑冻结完成后，将同锥节点签名原地切换为正交残差签名并释放构图缓冲
        if let Some(helper) = build_residual {
            self.finalize_residual_store(helper);
        }
        if std::env::var_os("QUIVER_BUILD_PHASES").is_some() {
            println!(
                "{{\"构建阶段\":true,\"路由秒\":{router_seconds},\"图插入剪枝秒\":{graph_seconds},\"图插入秒\":{insertion_seconds},\"最终剪枝秒\":{},\"冻结修复秒\":{}}}",
                prune_started.elapsed().as_secs_f64() - finalize_started.elapsed().as_secs_f64(),
                finalize_started.elapsed().as_secs_f64()
            );
        }
        if cfg!(debug_assertions)
            || crate::index::pelorus::engine::config::environment_flag(
                "QUIVER_VERIFY_GRAPH",
                "TRIVIUM_BQ_VAMANA_VALIDATE",
            )
        {
            self.validate_layer0();
        }
    }

    /// 双曲图 BFS 生成树边保护与零入度孤岛救生索：
    /// 在不超出 `m0` 最大出度、不产生自环与重边、不破坏任何已有可达生成树边的前提下，
    /// 尝试将零入度叶子与封闭分量接回入口可达图，最终可达率由图诊断核验。
    pub(crate) fn wire_unreachable_native_nodes(&mut self, build_order: &[u32]) {
        const ROOT_PARENT: u32 = EMPTY_NB - 1;
        let n = self.n;
        if n <= 1 {
            return;
        }
        let mut in_deg = vec![0u32; n];
        for u in 0..n as u32 {
            for &v in self.layer0.neighbors(u) {
                in_deg[v as usize] += 1;
            }
        }
        let mut bfs_parent = vec![EMPTY_NB; n];
        let mut queue = std::collections::VecDeque::with_capacity(1024);
        bfs_parent[self.entry_point as usize] = ROOT_PARENT;
        let mut reached_cnt = 1usize;
        queue.push_back(self.entry_point);
        while let Some(cur) = queue.pop_front() {
            for &nb in self.layer0.neighbors(cur) {
                let idx = nb as usize;
                if bfs_parent[idx] == EMPTY_NB {
                    bfs_parent[idx] = cur;
                    reached_cnt += 1;
                    queue.push_back(nb);
                }
            }
        }
        if reached_cnt == n && in_deg[self.entry_point as usize] > 0 {
            return;
        }

        let pick_host = |layer0: &FlatAdj,
                         m0: usize,
                         bfs_parent: &[u32],
                         in_deg: &[u32],
                         u: u32,
                         cands: &[u32]|
         -> Option<(u32, Option<usize>)> {
            let mut best: Option<((bool, u32, usize), u32, usize)> = None;
            for &p in cands {
                if p == u || bfs_parent[p as usize] == EMPTY_NB || layer0.contains(p, u) {
                    continue;
                }
                let nbs = layer0.neighbors(p);
                if nbs.len() < m0 {
                    return Some((p, None));
                }
                for slot in (0..nbs.len()).rev() {
                    let w = nbs[slot];
                    let deg_w = in_deg[w as usize];
                    if bfs_parent[w as usize] != p && deg_w >= 2 {
                        let key = (slot >= nbs.len() / 2, deg_w, slot);
                        if best.as_ref().is_none_or(|(bk, _, _)| key > *bk) {
                            best = Some((key, p, slot));
                        }
                        break;
                    }
                }
            }
            best.map(|(_, p, slot)| (p, Some(slot)))
        };

        let mut u_nbs_buf: Vec<u32> = Vec::with_capacity(self.m0);
        let fallback_prefix = &build_order[..build_order.len().min(256)];
        for &u in build_order {
            if reached_cnt == n {
                break;
            }
            if bfs_parent[u as usize] != EMPTY_NB {
                continue;
            }
            u_nbs_buf.clear();
            u_nbs_buf.extend_from_slice(self.layer0.neighbors(u));
            let chosen = pick_host(&self.layer0, self.m0, &bfs_parent, &in_deg, u, &u_nbs_buf)
                .or_else(|| {
                    let hubs = self
                        .polar_router
                        .as_ref()
                        .map(|r| r.origin_hubs.as_slice())
                        .unwrap_or(&[]);
                    pick_host(&self.layer0, self.m0, &bfs_parent, &in_deg, u, hubs)
                })
                .or_else(|| {
                    pick_host(
                        &self.layer0,
                        self.m0,
                        &bfs_parent,
                        &in_deg,
                        u,
                        fallback_prefix,
                    )
                });
            if let Some((p, slot_opt)) = chosen {
                match slot_opt {
                    None => {
                        self.layer0.push_neighbor(p, u);
                        in_deg[u as usize] += 1;
                    }
                    Some(slot) => {
                        let base = p as usize * self.layer0.stride;
                        let old_w = self.layer0.data[base + 1 + slot];
                        in_deg[old_w as usize] -= 1;
                        self.layer0.data[base + 1 + slot] = u;
                        in_deg[u as usize] += 1;
                    }
                }
                bfs_parent[u as usize] = p;
                reached_cnt += 1;
                queue.push_back(u);
                while let Some(cur) = queue.pop_front() {
                    for &nb in self.layer0.neighbors(cur) {
                        let idx = nb as usize;
                        if bfs_parent[idx] == EMPTY_NB {
                            bfs_parent[idx] = cur;
                            reached_cnt += 1;
                            queue.push_back(nb);
                        }
                    }
                }
            }
        }

        // 确保入口节点自身也拥有至少一条反向入边，消除入口孤入度
        if in_deg[self.entry_point as usize] == 0 {
            let ep = self.entry_point;
            u_nbs_buf.clear();
            u_nbs_buf.extend_from_slice(self.layer0.neighbors(ep));
            if let Some((p, slot_opt)) =
                pick_host(&self.layer0, self.m0, &bfs_parent, &in_deg, ep, &u_nbs_buf).or_else(
                    || {
                        pick_host(
                            &self.layer0,
                            self.m0,
                            &bfs_parent,
                            &in_deg,
                            ep,
                            fallback_prefix,
                        )
                    },
                )
            {
                match slot_opt {
                    None => {
                        self.layer0.push_neighbor(p, ep);
                    }
                    Some(slot) => {
                        let base = p as usize * self.layer0.stride;
                        self.layer0.data[base + 1 + slot] = ep;
                    }
                }
            }
        }
    }

    pub(crate) fn build_parallel_eager(
        &mut self,
        vectors: &[f32],
        ids: &[u64],
        slot_idxs: &[usize],
    ) {
        let n = ids.len();
        let dim = self.dim;
        assert_eq!(vectors.len(), n * dim);
        assert_eq!(slot_idxs.len(), n);

        let sigs: Vec<Bq2Signature> = vectors
            .par_chunks(dim)
            .map(Bq2Signature::from_vector)
            .collect();

        self.bq_store.reserve(n);
        self.ids.reserve(n);
        self.slot_indices.reserve(n);
        self.tombstones.reserve(n);
        self.node_max_layer.reserve(n);

        let mut lcg: u64 = 12345;
        let mut levels = Vec::with_capacity(n);
        for _ in 0..n {
            levels.push(self.random_level(&mut lcg));
        }

        self.layer0.data.reserve(n * self.layer0.stride);

        let mut visited = Bitset::new(0);
        for i in 0..n {
            let _v = &vectors[i * dim..(i + 1) * dim];
            let idx = self.n as u32;

            self.bq_store.push_sig(&sigs[i]);
            self.ids.push(ids[i]);
            self.slot_indices.push(slot_idxs[i]);
            self.id_to_internal.insert(ids[i], idx);
            self.tombstones.push(false);

            let level = levels[i];
            self.node_max_layer.push(level as u8);

            self.layer0.push_empty();
            while self.upper_layers.len() < level {
                self.upper_layers.push(vec![Vec::new(); self.n]);
            }
            for ul in self.upper_layers.iter_mut() {
                ul.push(Vec::new());
            }

            self.n += 1;
            visited.grow(self.n);

            if self.n == 1 {
                self.entry_point = 0;
                self.max_level = level;
                continue;
            }

            let my_sig = self.bq_store.get_sig(idx as usize);
            let mut cur_node = self.entry_point;

            for l in ((level + 1)..=self.max_level).rev() {
                let ul_idx = l - 1;
                if ul_idx < self.upper_layers.len() {
                    loop {
                        let mut changed = false;
                        let cur_d = self.bq_store.distance(idx as usize, cur_node as usize, dim);
                        let mut best_d = cur_d;
                        for &nb in &self.upper_layers[ul_idx][cur_node as usize] {
                            let nd = self.bq_store.distance(idx as usize, nb as usize, dim);
                            if nd < best_d {
                                cur_node = nb;
                                best_d = nd;
                                changed = true;
                            }
                        }
                        if !changed {
                            break;
                        }
                    }
                }
            }

            for l in (level..=self.max_level).rev() {
                let res = self.beam_search_upper(&my_sig, l, cur_node, 1, &mut visited);
                if !res.is_empty() {
                    cur_node = res[0].1;
                }
            }

            for l in (0..=level).rev() {
                let ef = self.ef_construction;
                let candidates_sym = if l == 0 {
                    self.beam_search_l0(&my_sig, cur_node, ef, &mut visited)
                } else {
                    self.beam_search_upper(&my_sig, l, cur_node, ef, &mut visited)
                };
                let candidates: Vec<(u32, u32)> = candidates_sym
                    .into_iter()
                    .map(|(_, id)| (self.bq_store.distance(idx as usize, id as usize, dim), id))
                    .collect();

                let max_nb = if l == 0 { self.m0 } else { self.m };
                let selected = self.select_neighbors(idx, &candidates, max_nb);

                if l == 0 {
                    for &nb in &selected {
                        if !self.layer0.contains(idx, nb) {
                            self.layer0.push_neighbor(idx, nb);
                        }
                    }

                    let this_addr = self as *mut Self as usize;
                    selected.par_iter().for_each(|&nb| unsafe {
                        let this = &mut *(this_addr as *mut Self);
                        let mut nb_candidates: Vec<(u32, u32)> = this
                            .layer0
                            .neighbors(nb)
                            .iter()
                            .map(|&n| (this.bq_store.distance(nb as usize, n as usize, dim), n))
                            .collect();
                        if !this.layer0.contains(nb, idx) {
                            let d = this.bq_store.distance(nb as usize, idx as usize, dim);
                            nb_candidates.push((d, idx));
                        }
                        nb_candidates.sort_unstable_by_key(|&(d, _)| d);
                        let pruned = Self::vamana_select(
                            &this.bq_store,
                            nb,
                            &nb_candidates,
                            max_nb,
                            dim,
                            this.alpha,
                        );
                        this.layer0.set_neighbors(nb, &pruned);
                    });
                } else {
                    let ul = l - 1;
                    for &nb in &selected {
                        if !self.upper_layers[ul][idx as usize].contains(&nb) {
                            self.upper_layers[ul][idx as usize].push(nb);
                        }
                        if !self.upper_layers[ul][nb as usize].contains(&idx) {
                            self.upper_layers[ul][nb as usize].push(idx);
                        }
                    }
                }

                if !candidates.is_empty() {
                    cur_node = candidates[0].1;
                }
            }

            if level > self.max_level {
                self.entry_point = idx;
                self.max_level = level;
            }
        }
    }

    pub(crate) fn validate_layer0(&self) {
        if self.n == 0 {
            return;
        }
        debug_assert_eq!(self.layer0.data.len(), self.n * self.layer0.stride);
        for node in 0..self.n {
            let base = node * self.layer0.stride;
            let deg = self.layer0.data[base] as usize;
            assert!(
                deg < self.layer0.stride,
                "L0 节点 {} 度数 {} 超过 stride {}",
                node,
                deg,
                self.layer0.stride
            );
            let mut seen = Vec::with_capacity(deg);
            for i in 0..deg {
                let nb = self.layer0.data[base + 1 + i];
                assert!(nb != EMPTY_NB, "L0 节点 {} 有效邻居区出现空槽", node);
                assert!(
                    (nb as usize) < self.n,
                    "L0 节点 {} 邻居 {} 越界，n={}",
                    node,
                    nb,
                    self.n
                );
                assert!(nb as usize != node, "L0 节点 {} 出现自环", node);
                seen.push(nb);
            }
            seen.sort_unstable();
            for pair in seen.windows(2) {
                assert!(
                    pair[0] != pair[1],
                    "L0 节点 {} 出现重复邻居 {}",
                    node,
                    pair[0]
                );
            }
            for i in deg..(self.layer0.stride - 1) {
                assert!(
                    self.layer0.data[base + 1 + i] == EMPTY_NB,
                    "L0 节点 {} 无效邻居区出现非空槽",
                    node
                );
            }
        }
    }
}
