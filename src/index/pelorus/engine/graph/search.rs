//! 余弦导航、候选过滤与精排。

use super::*;
impl QuIVer {
    /// Vamana 选边
    pub(crate) fn select_neighbors(
        &self,
        target: u32,
        candidates: &[(u32, u32)],
        max_k: usize,
    ) -> Vec<u32> {
        Self::vamana_select(
            &self.bq_store,
            target,
            candidates,
            max_k,
            self.dim,
            self.alpha,
        )
    }

    /// Layer 0 搜索：cheap BQ 导航后用完整 2-bit BQ 距离重排候选
    pub(crate) fn beam_search_l0<V: SearchVisited>(
        &self,
        q_sig: &Bq2Signature,
        entry: u32,
        ef: usize,
        visited: &mut V,
    ) -> Vec<(NonNanF32, u32)> {
        let mut scorer = BqNavigationScorer;
        self.beam_search_l0_with_scorer(q_sig, entry, ef, visited, &mut scorer)
    }

    pub(crate) fn beam_search_l0_with_scorer<V: SearchVisited, S: NavigationScorer>(
        &self,
        q_sig: &Bq2Signature,
        entry: u32,
        ef: usize,
        visited: &mut V,
        scorer: &mut S,
    ) -> Vec<(NonNanF32, u32)> {
        #[cfg(target_arch = "x86_64")]
        if self.dim == 384
            && !*crate::index::pelorus::engine::quantization::FORCE_NO_384_KERNEL
            && is_x86_feature_detected!("popcnt")
        {
            return self.beam_search_l0_impl::<true, V, S>(q_sig, entry, ef, visited, scorer);
        }
        self.beam_search_l0_impl::<false, V, S>(q_sig, entry, ef, visited, scorer)
    }

    pub(crate) fn beam_search_l0_impl<
        const DIM_384: bool,
        V: SearchVisited,
        S: NavigationScorer,
    >(
        &self,
        q_sig: &Bq2Signature,
        entry: u32,
        ef: usize,
        visited: &mut V,
        scorer: &mut S,
    ) -> Vec<(NonNanF32, u32)> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        visited.begin();

        let mut candidates: BinaryHeap<Reverse<(NonNanF32, u32)>> =
            BinaryHeap::with_capacity(ef * 2);
        let mut results: BinaryHeap<(NonNanF32, u32)> = BinaryHeap::with_capacity(ef + 1);

        let cheap_distance = |node: u32| -> u32 {
            #[cfg(target_arch = "x86_64")]
            if DIM_384 {
                return self
                    .bq_store
                    .distance_to_sig_cheap_384(node as usize, q_sig);
            }
            self.bq_store
                .distance_to_sig_cheap(node as usize, q_sig, self.dim)
        };
        let d = NonNanF32(scorer.score(self.ids[entry as usize], cheap_distance(entry)) as f32);
        visited.set(entry as usize);
        candidates.push(Reverse((d, entry)));
        results.push((d, entry));

        while let Some(Reverse((cd, cur))) = candidates.pop() {
            if results.len() >= ef && cd > results.peek().unwrap().0 {
                break;
            }

            let nbs = self.layer0.neighbors(cur);

            for &nb in nbs {
                if !visited.test(nb as usize) {
                    self.bq_store.prefetch_sig(nb as usize);
                }
            }

            for &nb in nbs {
                if visited.test(nb as usize) {
                    continue;
                }
                visited.set(nb as usize);

                let nd = NonNanF32(scorer.score(self.ids[nb as usize], cheap_distance(nb)) as f32);
                if results.len() < ef || nd < results.peek().unwrap().0 {
                    candidates.push(Reverse((nd, nb)));
                    results.push((nd, nb));
                    if results.len() > ef {
                        results.pop();
                    }
                }
            }
        }

        let mut res: Vec<(NonNanF32, u32)> = results.into_vec();
        for item in &mut res {
            item.0 = NonNanF32(
                self.bq_store
                    .distance_to_sig(item.1 as usize, q_sig, self.dim) as f32,
            );
        }
        res.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        res
    }

    pub(crate) fn beam_search_l0_dual<S: NavigationScorer>(
        &self,
        q_sig: &Bq2Signature,
        entry: u32,
        ef: usize,
        scorer: &mut S,
        signal_quota_ppm: u32,
        signal_seed_ids: &[u64],
    ) -> Vec<(NonNanF32, u32)> {
        let mut discovered = Bitset::new(self.n);
        let mut expanded = Bitset::new(self.n);
        let mut vector_queue = BinaryHeap::with_capacity(ef.saturating_mul(2));
        let mut signal_queue = BinaryHeap::with_capacity(ef.saturating_mul(2));
        let mut vector_results = BinaryHeap::with_capacity(ef.saturating_add(1));
        let mut signal_results = BinaryHeap::with_capacity(ef.saturating_add(1));
        let distance = |node: u32| {
            self.bq_store
                .distance_to_sig_cheap(node as usize, q_sig, self.dim)
        };
        let add = |node: u32,
                   discovered: &mut Bitset,
                   vector_queue: &mut BinaryHeap<Reverse<(NonNanF32, u32)>>,
                   signal_queue: &mut BinaryHeap<Reverse<(NonNanF32, u32)>>,
                   vector_results: &mut BinaryHeap<(NonNanF32, u32)>,
                   signal_results: &mut BinaryHeap<(NonNanF32, u32)>,
                   scorer: &mut S| {
            if discovered.test(node as usize) {
                return;
            }
            discovered.set(node as usize);
            let vector_distance = NonNanF32(distance(node) as f32);
            let signal_distance =
                NonNanF32(scorer.score(self.ids[node as usize], vector_distance.0 as u32) as f32);
            vector_queue.push(Reverse((vector_distance, node)));
            signal_queue.push(Reverse((signal_distance, node)));
            vector_results.push((vector_distance, node));
            signal_results.push((signal_distance, node));
            if vector_results.len() > ef {
                vector_results.pop();
            }
            if signal_results.len() > ef {
                signal_results.pop();
            }
        };
        add(
            entry,
            &mut discovered,
            &mut vector_queue,
            &mut signal_queue,
            &mut vector_results,
            &mut signal_results,
            scorer,
        );
        for &id in signal_seed_ids {
            if let Some(&node) = self.id_to_internal.get(&id)
                && !self.tombstones[node as usize]
            {
                add(
                    node,
                    &mut discovered,
                    &mut vector_queue,
                    &mut signal_queue,
                    &mut vector_results,
                    &mut signal_results,
                    scorer,
                );
            }
        }

        let mut quota_accumulator = 0u64;
        loop {
            while vector_queue
                .peek()
                .is_some_and(|Reverse((_, node))| expanded.test(*node as usize))
            {
                vector_queue.pop();
            }
            while signal_queue
                .peek()
                .is_some_and(|Reverse((_, node))| expanded.test(*node as usize))
            {
                signal_queue.pop();
            }
            let vector_available = vector_queue.peek().is_some_and(|Reverse((score, _))| {
                vector_results.len() < ef || *score <= vector_results.peek().unwrap().0
            });
            let signal_available = signal_queue.peek().is_some_and(|Reverse((score, _))| {
                signal_results.len() < ef || *score <= signal_results.peek().unwrap().0
            });
            if !vector_available && !signal_available {
                break;
            }
            quota_accumulator = quota_accumulator.saturating_add(u64::from(signal_quota_ppm));
            let prefer_signal = quota_accumulator >= 1_000_000;
            if prefer_signal {
                quota_accumulator -= 1_000_000;
            }
            let current = if prefer_signal && signal_available || !vector_available {
                signal_queue.pop().map(|Reverse((_, node))| node)
            } else {
                vector_queue.pop().map(|Reverse((_, node))| node)
            };
            let Some(current) = current else {
                break;
            };
            if expanded.test(current as usize) {
                continue;
            }
            expanded.set(current as usize);
            let neighbors = self.layer0.neighbors(current);
            for &neighbor in neighbors {
                if !discovered.test(neighbor as usize) {
                    self.bq_store.prefetch_sig(neighbor as usize);
                }
            }
            for &neighbor in neighbors {
                add(
                    neighbor,
                    &mut discovered,
                    &mut vector_queue,
                    &mut signal_queue,
                    &mut vector_results,
                    &mut signal_results,
                    scorer,
                );
            }
        }

        let mut vector = vector_results.into_vec();
        let mut signal = signal_results.into_vec();
        vector.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        signal.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let mut selected = Bitset::new(self.n);
        let mut candidates = Vec::with_capacity(ef);
        let mut vector_index = 0usize;
        let mut signal_index = 0usize;
        let mut merge_accumulator = 0u64;
        while candidates.len() < ef && (vector_index < vector.len() || signal_index < signal.len())
        {
            merge_accumulator = merge_accumulator.saturating_add(u64::from(signal_quota_ppm));
            let prefer_signal = merge_accumulator >= 1_000_000;
            if prefer_signal {
                merge_accumulator -= 1_000_000;
            }
            let source =
                if prefer_signal && signal_index < signal.len() || vector_index >= vector.len() {
                    let item = signal[signal_index];
                    signal_index += 1;
                    item
                } else {
                    let item = vector[vector_index];
                    vector_index += 1;
                    item
                };
            if selected.test(source.1 as usize) {
                continue;
            }
            selected.set(source.1 as usize);
            candidates.push(source);
        }
        for candidate in &mut candidates {
            candidate.0 = NonNanF32(self.bq_store.distance_to_sig(
                candidate.1 as usize,
                q_sig,
                self.dim,
            ) as f32);
        }
        candidates.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        candidates
    }

    pub(crate) fn beam_search_l0_384_with_scorer<S: NavigationScorer>(
        &self,
        q_sig: &Bq2Signature,
        entry: u32,
        ef: usize,
        scratch: &mut SearchScratch384,
        scorer: &mut S,
    ) -> Vec<(NonNanF32, u32)> {
        scratch.visited.begin();
        scratch.candidates.clear();
        scratch.results.clear();
        scratch.candidates.reserve(ef.saturating_mul(2));
        scratch.results.reserve(ef.saturating_add(1));

        let distance = |scorer: &mut S, node: u32| -> NonNanF32 {
            let bq_distance = self
                .bq_store
                .distance_to_sig_cheap_384(node as usize, q_sig);
            NonNanF32(scorer.score(self.ids[node as usize], bq_distance) as f32)
        };
        let initial_distance = distance(scorer, entry);
        scratch.visited.set(entry as usize);
        scratch.candidates.push(Reverse((initial_distance, entry)));
        scratch.results.push((initial_distance, entry));

        while let Some(Reverse((candidate_distance, current))) = scratch.candidates.pop() {
            if scratch.results.len() >= ef && candidate_distance > scratch.results.peek().unwrap().0
            {
                break;
            }
            let neighbors = self.layer0.neighbors(current);
            for &neighbor in neighbors {
                if !scratch.visited.test(neighbor as usize) {
                    self.bq_store.prefetch_sig(neighbor as usize);
                }
            }
            for &neighbor in neighbors {
                if scratch.visited.test(neighbor as usize) {
                    continue;
                }
                scratch.visited.set(neighbor as usize);
                let neighbor_distance = distance(scorer, neighbor);
                if scratch.results.len() < ef
                    || neighbor_distance < scratch.results.peek().unwrap().0
                {
                    scratch
                        .candidates
                        .push(Reverse((neighbor_distance, neighbor)));
                    scratch.results.push((neighbor_distance, neighbor));
                    if scratch.results.len() > ef {
                        scratch.results.pop();
                    }
                }
            }
        }

        let mut candidates = scratch.results.drain().collect::<Vec<_>>();
        for candidate in &mut candidates {
            candidate.0 = NonNanF32(
                self.bq_store
                    .distance_to_sig(candidate.1 as usize, q_sig, 384) as f32,
            );
        }
        candidates.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        candidates
    }

    /// Upper layer Symmetric beam search
    pub(crate) fn beam_search_upper(
        &self,
        q_sig: &Bq2Signature,
        layer: usize,
        entry: u32,
        ef: usize,
        visited: &mut Bitset,
    ) -> Vec<(NonNanF32, u32)> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        if layer == 0 {
            return Vec::new();
        }
        let ul = layer - 1;
        if ul >= self.upper_layers.len() {
            return Vec::new();
        }

        visited.clear();
        let mut candidates: BinaryHeap<Reverse<(NonNanF32, u32)>> = BinaryHeap::new();
        let mut results: BinaryHeap<(NonNanF32, u32)> = BinaryHeap::with_capacity(ef + 1);

        let d = NonNanF32(
            self.bq_store
                .distance_to_sig(entry as usize, q_sig, self.dim) as f32,
        );
        visited.set(entry as usize);
        candidates.push(Reverse((d, entry)));
        results.push((d, entry));

        while let Some(Reverse((cd, cur))) = candidates.pop() {
            if results.len() >= ef && cd > results.peek().unwrap().0 {
                break;
            }

            let nbs: Vec<u32> = self.upper_layers[ul][cur as usize].clone();
            for nb in nbs {
                if visited.test(nb as usize) {
                    continue;
                }
                visited.set(nb as usize);
                let nd =
                    NonNanF32(self.bq_store.distance_to_sig(nb as usize, q_sig, self.dim) as f32);
                if results.len() < ef || nd < results.peek().unwrap().0 {
                    candidates.push(Reverse((nd, nb)));
                    results.push((nd, nb));
                    if results.len() > ef {
                        results.pop();
                    }
                }
            }
        }

        let mut res: Vec<(NonNanF32, u32)> = results.into_vec();
        res.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        res
    }

    /// Vamana Robust Prune (带有 alpha 放宽因子的 Heuristic)
    pub(crate) fn vamana_select(
        sigs: &Bq2Store,
        target: u32,
        candidates: &[(u32, u32)],
        max_k: usize,
        dim: usize,
        alpha: f32,
    ) -> Vec<u32> {
        let mut selected: Vec<u32> = Vec::with_capacity(max_k);

        for &(dist_to_target, cid) in candidates {
            if cid == target {
                continue;
            }
            if selected.len() >= max_k {
                break;
            }

            // Vamana Prune: dist_to_selected < alpha * dist_to_target
            let dominated = selected.iter().any(|&s| {
                let dist_to_selected = sigs.distance(cid as usize, s as usize, dim);
                (dist_to_selected as f32) < alpha * (dist_to_target as f32)
            });

            if !dominated {
                selected.push(cid);
            }
        }

        if selected.len() < max_k {
            for &(_, cid) in candidates {
                if cid == target {
                    continue;
                }
                if selected.len() >= max_k {
                    break;
                }
                if !selected.contains(&cid) {
                    selected.push(cid);
                }
            }
        }

        selected
    }

    /// 两阶段搜索：Symmetric BQ 寻路 + f32 按需精排（真正的冷热分离）
    ///
    /// # 冷热分离
    /// - **Hot（Stage 1）**：beam search 全程仅访问常驻内存的 2-bit BQ 签名与图拓扑，
    ///   不触碰任何 f32 原始向量。
    /// - **Cold（Stage 2）**：仅对 beam search 召回的 ~ef 个候选，通过 `get_vec_f32`
    ///   回调按 MemTable slot 索引**按需**取回单条 f32 向量做精排。
    ///
    /// `get_vec_f32(slot, buf)`：把 `slot` 对应节点的向量以 f32 写入 `buf`，
    /// 成功返回 `true`。冷数据由调用方按需从 mmap 零拷贝读取，
    /// 不再要求传入全量 f32 数组——这是 QuIVer 冷热分离设计的最后一块拼图。
    pub fn search<F>(&self, query: &[f32], get_vec_f32: F, config: &SearchConfig) -> Vec<(u64, f32)>
    where
        F: FnMut(usize, &mut Vec<f32>) -> bool,
    {
        let mut scorer = BqNavigationScorer;
        self.search_with_scorer(query, get_vec_f32, config, &mut scorer)
    }

    /// 估算指定属性候选在 BQ2 空间中的相对密度。
    ///
    /// 返回“候选集合平均成对距离 / 全局平均成对距离”。小于 1 表示候选更聚集。
    /// 采样按排序后等距抽取，且仅比较相邻样本，保证成本有界与结果确定。
    pub fn vector_density_skew(
        &self,
        candidate_ids: &[u64],
        max_samples: usize,
    ) -> Option<(f64, usize)> {
        fn sample_slots(index: &QuIVer, ids: &[u64], limit: usize) -> Vec<usize> {
            let mut slots = ids
                .iter()
                .filter_map(|id| index.id_to_internal.get(id).copied())
                .map(|slot| slot as usize)
                .filter(|slot| !index.tombstones.get(*slot).copied().unwrap_or(true))
                .collect::<Vec<_>>();
            slots.sort_unstable();
            slots.dedup();
            if slots.len() > limit {
                let last = slots.len() - 1;
                slots = (0..limit)
                    .map(|position| slots[position * last / (limit - 1)])
                    .collect();
            }
            slots
        }

        fn mean_adjacent_distance(index: &QuIVer, slots: &[usize]) -> Option<f64> {
            let (sum, count) = slots.windows(2).fold((0u64, 0usize), |(sum, count), pair| {
                (
                    sum.saturating_add(u64::from(
                        index.bq_store.distance(pair[0], pair[1], index.dim),
                    )),
                    count + 1,
                )
            });
            (count > 0).then_some(sum as f64 / count as f64)
        }

        let limit = max_samples.clamp(2, 256);
        let candidate_slots = sample_slots(self, candidate_ids, limit);
        let all_ids = self
            .ids
            .iter()
            .copied()
            .filter(|id| self.id_to_internal.contains_key(id))
            .collect::<Vec<_>>();
        let global_slots = sample_slots(self, &all_ids, limit);
        let candidate_mean = mean_adjacent_distance(self, &candidate_slots)?;
        let global_mean = mean_adjacent_distance(self, &global_slots)?;
        if global_mean <= f64::EPSILON {
            return Some((1.0, candidate_slots.len()));
        }
        Some((
            (candidate_mean / global_mean).clamp(0.25, 4.0),
            candidate_slots.len(),
        ))
    }

    /// 对指定 NodeId 候选集合执行纯 BQ2 粗排，不访问原始向量。
    ///
    /// 该接口仅复用现有 BQ2 签名和 NodeId 映射，不修改 Vamana 图或建边语义。
    pub fn prefilter_candidates(
        &self,
        query: &[f32],
        candidate_ids: &[u64],
        limit: usize,
    ) -> Vec<(u64, u32)> {
        self.prefilter_candidates_profiled(query, candidate_ids, 0, 0, limit)
            .0
    }

    /// 使用当前 QuIVer 实例范围内的 internal slot posting 缓存执行 BQ2 粗排，
    /// 并返回可供 benchmark 分解的阶段耗时。缓存不落盘，也不参与 Vamana 建图。
    pub fn prefilter_candidates_profiled(
        &self,
        query: &[f32],
        candidate_ids: &[u64],
        posting_identity: u64,
        property_generation: u64,
        limit: usize,
    ) -> (Vec<(u64, u32)>, PrefilterMetrics) {
        if query.len() != self.dim || limit == 0 {
            return (Vec::new(), PrefilterMetrics::default());
        }
        let lookup_started = Instant::now();
        let cache_key = (posting_identity, property_generation);
        let cached = self
            .prefilter_slot_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(cache_key);
        let posting_lookup_ns = elapsed_ns(lookup_started);
        let cache_hit = cached.is_some();
        let mapping_started = Instant::now();
        let slots = cached.unwrap_or_else(|| {
            let mapped = candidate_ids
                .iter()
                .map(|id| {
                    (
                        *id,
                        self.id_to_internal.get(id).copied().unwrap_or(u32::MAX),
                    )
                })
                .collect::<Vec<_>>();
            let mapped = Arc::new(mapped);
            self.prefilter_slot_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(cache_key, Arc::clone(&mapped));
            mapped
        });
        let node_mapping_ns = elapsed_ns(mapping_started);
        let signature = Bq2Signature::from_vector(query);
        let scan_started = Instant::now();
        let mut best = BinaryHeap::with_capacity(limit.min(slots.len()));
        let mut mapped_candidates = 0usize;
        for &(id, internal) in slots.iter() {
            let internal = internal as usize;
            if internal == u32::MAX as usize
                || self.tombstones.get(internal).copied().unwrap_or(true)
            {
                continue;
            }
            mapped_candidates += 1;
            let candidate = (
                self.bq_store
                    .distance_to_sig(internal, &signature, self.dim),
                id,
            );
            if best.len() < limit {
                best.push(candidate);
            } else if best.peek().is_some_and(|worst| candidate < *worst) {
                best.pop();
                best.push(candidate);
            }
        }
        let bq_heap_scan_ns = elapsed_ns(scan_started);
        let sort_started = Instant::now();
        let mut scored = best
            .into_iter()
            .map(|(distance, id)| (id, distance))
            .collect::<Vec<_>>();
        scored.sort_unstable_by(|left, right| {
            left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0))
        });
        let output_sort_ns = elapsed_ns(sort_started);
        (
            scored,
            PrefilterMetrics {
                posting_lookup_ns,
                node_mapping_ns,
                bq_heap_scan_ns,
                output_sort_ns,
                cache_hit,
                mapped_candidates,
            },
        )
    }

    /// 实验性双队列联合导航。纯向量入口不会调用此方法。
    pub fn search_dual_queue<F, S>(
        &self,
        query: &[f32],
        mut get_vec_f32: F,
        config: &SearchConfig,
        scorer: &mut S,
        signal_quota_ppm: u32,
        signal_seed_ids: &[u64],
    ) -> Vec<(u64, f32)>
    where
        F: FnMut(usize, &mut Vec<f32>) -> bool,
        S: NavigationScorer,
    {
        if self.n == 0 {
            return Vec::new();
        }
        assert_eq!(query.len(), self.dim);
        let query_signature = Bq2Signature::from_vector(query);
        let mut entry = self.entry_point;
        for layer in (1..=self.max_level).rev() {
            let upper = layer - 1;
            if upper >= self.upper_layers.len() {
                continue;
            }
            loop {
                let mut changed = false;
                let mut best =
                    self.bq_store
                        .distance_to_sig(entry as usize, &query_signature, self.dim);
                for &neighbor in &self.upper_layers[upper][entry as usize] {
                    let distance = self.bq_store.distance_to_sig(
                        neighbor as usize,
                        &query_signature,
                        self.dim,
                    );
                    if distance < best {
                        entry = neighbor;
                        best = distance;
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
        }
        let candidates = self.beam_search_l0_dual(
            &query_signature,
            entry,
            config.ef_search,
            scorer,
            signal_quota_ppm,
            signal_seed_ids,
        );
        let rerank_limit = config.rerank_limit().min(candidates.len());
        let mut buffer = Vec::with_capacity(self.dim);
        let mut scored = Vec::with_capacity(rerank_limit);
        for &(_, internal) in candidates.iter().take(rerank_limit) {
            if self.tombstones[internal as usize]
                || !scorer.accept_result(self.ids[internal as usize])
            {
                continue;
            }
            let slot = self.slot_indices[internal as usize];
            if get_vec_f32(slot, &mut buffer) && buffer.len() == self.dim {
                scored.push((cosine_similarity_f32(query, &buffer), internal));
            }
        }
        scored.sort_unstable_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.1.cmp(&right.1))
        });
        scored
            .into_iter()
            .take(config.top_k)
            .map(|(score, internal)| (self.ids[internal as usize], score))
            .collect()
    }

    /// 使用自定义查询时评分器搜索。仅显式联合查询应调用此入口。
    pub fn search_with_scorer<F, S>(
        &self,
        query: &[f32],
        mut get_vec_f32: F,
        config: &SearchConfig,
        scorer: &mut S,
    ) -> Vec<(u64, f32)>
    where
        F: FnMut(usize, &mut Vec<f32>) -> bool,
        S: NavigationScorer,
    {
        if self.n == 0 {
            return Vec::new();
        }
        assert_eq!(query.len(), self.dim);
        let dim = self.dim;

        let q_sig = Bq2Signature::from_vector(query);
        let cur_node = self.cosine_entry(&q_sig);

        // Stage 1: Symmetric BQ beam search
        // 使用 thread_local Bitset 复用，避免每次查询 heap alloc
        thread_local! {
            static VISITED: std::cell::RefCell<Bitset> = std::cell::RefCell::new(Bitset::new(0));
            static SCRATCH_384: std::cell::RefCell<SearchScratch384> = std::cell::RefCell::new(SearchScratch384::new());
        }
        let use_384 = dim == 384
            && !*crate::index::pelorus::engine::quantization::FORCE_NO_384_KERNEL
            && cfg!(target_arch = "x86_64");
        if use_384 {
            return SCRATCH_384.with(|cell| {
                let mut scratch = cell.borrow_mut();
                scratch.visited.grow(self.n);
                let bq_candidates = self.beam_search_l0_384_with_scorer(
                    &q_sig,
                    cur_node,
                    config.ef_search,
                    &mut scratch,
                    scorer,
                );
                self.rerank_candidates_384(
                    query,
                    &bq_candidates,
                    config,
                    &mut get_vec_f32,
                    &mut scratch,
                    scorer,
                )
            });
        }
        let bq_candidates = VISITED.with(|cell| {
            let mut visited = cell.borrow_mut();
            if visited.len < self.n {
                visited.grow(self.n);
            }
            self.beam_search_l0_with_scorer(
                &q_sig,
                cur_node,
                config.ef_search,
                &mut *visited,
                scorer,
            )
        });
        // Stage 2: f32 按需精排（跳过 tombstone，通过 slot_indices 映射到冷存储）
        // 复用单个缓冲区，避免每个候选 heap alloc；冷向量按需取回仅触达 ~ef 个候选。
        let rerank_limit = config.rerank_limit().min(bq_candidates.len());
        let mut buf: Vec<f32> = Vec::with_capacity(dim);
        let mut scored: Vec<(f32, u32)> = Vec::with_capacity(rerank_limit);
        for &(_, nid) in bq_candidates.iter().take(rerank_limit) {
            if self.tombstones[nid as usize] || !scorer.accept_result(self.ids[nid as usize]) {
                continue;
            }
            let slot = self.slot_indices[nid as usize];
            if get_vec_f32(slot, &mut buf) && buf.len() == dim {
                scored.push((cosine_similarity_f32(query, &buf), nid));
            }
        }
        scored.sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

        scored
            .iter()
            .take(config.top_k)
            .map(|&(sim, nid)| (self.ids[nid as usize], sim))
            .collect()
    }

    pub(crate) fn rerank_candidates_384<F, S>(
        &self,
        query: &[f32],
        bq_candidates: &[(NonNanF32, u32)],
        config: &SearchConfig,
        get_vec_f32: &mut F,
        scratch: &mut SearchScratch384,
        scorer: &mut S,
    ) -> Vec<(u64, f32)>
    where
        F: FnMut(usize, &mut Vec<f32>) -> bool,
        S: NavigationScorer,
    {
        let rerank_limit = config.rerank_limit().min(bq_candidates.len());
        scratch.scored.clear();
        if scratch.scored.capacity() < rerank_limit {
            scratch
                .scored
                .reserve(rerank_limit - scratch.scored.capacity());
        }
        let query_norm = cosine_query_norm_f32_384(query).unwrap_or_default();
        for &(_, nid) in bq_candidates.iter().take(rerank_limit) {
            if self.tombstones[nid as usize] || !scorer.accept_result(self.ids[nid as usize]) {
                continue;
            }
            let slot = self.slot_indices[nid as usize];
            if get_vec_f32(slot, &mut scratch.vector) && scratch.vector.len() == 384 {
                scratch.scored.push((
                    cosine_similarity_f32_384_with_query_norm(query, &scratch.vector, query_norm),
                    nid,
                ));
            }
        }
        scratch
            .scored
            .sort_unstable_by(|left, right| right.0.partial_cmp(&left.0).unwrap());
        scratch
            .scored
            .iter()
            .take(config.top_k)
            .map(|&(score, nid)| (self.ids[nid as usize], score))
            .collect()
    }

    /// 便捷封装：从连续 f32 数组（按 slot 偏移）精排。
    ///
    /// 主要用于纯内存场景与单元测试。生产检索路径应直接调用 `search`
    /// 并传入按需读取 mmap 冷存储的回调，以保持冷热分离。
    pub fn search_flat(
        &self,
        query: &[f32],
        ext_vectors: &[f32],
        config: &SearchConfig,
    ) -> Vec<(u64, f32)> {
        let dim = self.dim;
        self.search(
            query,
            |slot, buf| {
                let offset = slot * dim;
                if offset + dim <= ext_vectors.len() {
                    buf.clear();
                    buf.extend_from_slice(&ext_vectors[offset..offset + dim]);
                    true
                } else {
                    false // slot 越界（不应发生，防御性跳过）
                }
            },
            config,
        )
    }
}

impl QuIVer {
    fn cosine_entry(&self, signature: &Bq2Signature) -> u32 {
        let mut cur_node = self.entry_point;

        for l in (1..=self.max_level).rev() {
            let ul = l - 1;
            if ul < self.upper_layers.len() {
                loop {
                    let mut changed = false;
                    let cur_d =
                        self.bq_store
                            .distance_to_sig(cur_node as usize, signature, self.dim);
                    let mut best_d = cur_d;
                    for &nb in &self.upper_layers[ul][cur_node as usize] {
                        let nd = self
                            .bq_store
                            .distance_to_sig(nb as usize, signature, self.dim);
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

        cur_node
    }

    pub(crate) fn cold_candidate_ids(&self, query: &[f32], config: &SearchConfig) -> Vec<u64> {
        if self.n == 0 {
            return Vec::new();
        }
        let sig = Bq2Signature::from_vector(query);
        let entry = self.cosine_entry(&sig);
        thread_local! { static VISITED: std::cell::RefCell<Bitset> = std::cell::RefCell::new(Bitset::new(0)); }
        let candidates = VISITED.with(|cell| {
            let mut visited = cell.borrow_mut();
            visited.grow(self.n);
            self.beam_search_l0(&sig, entry, config.ef_search, &mut *visited)
        });
        candidates
            .into_iter()
            .filter(|&(_, node)| !self.tombstones[node as usize])
            .take(config.rerank_limit())
            .map(|(_, node)| self.ids[node as usize])
            .collect()
    }
}
