//! QuIVer — QUantized Index for Vector Retrieval
//!
//! BQ-native Vamana 图索引，论文核心实现。
//! 支持 rayon 并行批量构建（batch_build）。
//!
//! 核心架构：
//!   - 2-bit Sign-Magnitude 量化 (pos + strong)
//!   - Vamana α-diversity 剪枝构图
//!   - Symmetric 寻路 (XOR+Popcount) + f32 精排
//!
//! 设计要点：
//!   - Bitset visited: 可复用位向量，2.5KB/20K 节点
//!   - Flat 邻接表: 连续内存，固定步长
//!   - BQ 签名连续存储: SoA 布局，预取友好

use crate::index::bq::{Bq2Signature, Bq2Store};
use crate::vector::cosine_similarity_f32;
use rayon::prelude::*;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Copy, Clone, PartialEq)]
pub struct NonNanF32(pub f32);

impl Eq for NonNanF32 {}
impl PartialOrd for NonNanF32 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for NonNanF32 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .partial_cmp(&other.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}


// ── Bitset ──

struct Bitset {
    data: Vec<u64>,
    len: usize,
}

impl Bitset {
    fn new(n: usize) -> Self {
        Self {
            data: vec![0u64; n.div_ceil(64)],
            len: n,
        }
    }
    #[inline(always)]
    fn set(&mut self, i: usize) {
        self.data[i >> 6] |= 1u64 << (i & 63);
    }
    #[inline(always)]
    fn test(&self, i: usize) -> bool {
        (self.data[i >> 6] >> (i & 63)) & 1 != 0
    }
    fn clear(&mut self) {
        self.data.iter_mut().for_each(|x| *x = 0);
    }
    fn grow(&mut self, new_n: usize) {
        let need = new_n.div_ceil(64);
        if need > self.data.len() {
            self.data.resize(need, 0);
        }
        self.len = new_n;
    }
}

// ── Flat neighbor list ──
// Layer 0: 每节点最多 m0 个邻居，存为 [degree, n0, n1, ..., n_{m0-1}] stride = m0+1
// Upper layers: 用 Vec<Vec<u32>>（节点少，性能不敏感）

const EMPTY_NB: u32 = u32::MAX;

struct FlatAdj {
    data: Vec<u32>, // n * stride 个 u32
    stride: usize,  // m0 + 1 (第一个元素是度数)
}

struct SpinLockGuard<'a> {
    lock: &'a AtomicBool,
}

impl Drop for SpinLockGuard<'_> {
    fn drop(&mut self) {
        self.lock.store(false, Ordering::Release);
    }
}

#[repr(transparent)]
struct NodeGuard<'a>((u32, SpinLockGuard<'a>));

impl NodeGuard<'_> {
    #[inline(always)]
    fn node(&self) -> u32 {
        self.0 .0
    }
}

struct StripedSpinLocks {
    locks: Vec<AtomicBool>,
    mask: usize,
}

impl StripedSpinLocks {
    fn new(nodes: usize, m: usize) -> Self {
        // 96核时需要足够的条纹数减少争用；每核至少 256 条纹
        let desired = nodes.clamp(256, 65536).max(m * 64);
        let count = desired.next_power_of_two();
        let locks = (0..count).map(|_| AtomicBool::new(false)).collect();
        Self {
            locks,
            mask: count - 1,
        }
    }

    #[inline(always)]
    fn stripe(&self, node: u32) -> usize {
        const FIB: usize = 0x9E3779B97F4A7C15usize;
        (node as usize).wrapping_mul(FIB) & self.mask
    }

    #[inline(always)]
    fn lock(&self, node: u32) -> NodeGuard<'_> {
        let lock = &self.locks[self.stripe(node)];
        while lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        NodeGuard((node, SpinLockGuard { lock }))
    }
}

impl FlatAdj {
    fn new(stride: usize) -> Self {
        Self {
            data: Vec::new(),
            stride,
        }
    }

    /// 为新节点追加空邻居列表
    fn push_empty(&mut self) {
        self.data.push(0); // degree = 0
        for _ in 1..self.stride {
            self.data.push(EMPTY_NB);
        }
    }

    #[inline(always)]
    fn degree(&self, node: u32) -> usize {
        self.data[node as usize * self.stride] as usize
    }

    #[inline(always)]
    fn neighbors(&self, node: u32) -> &[u32] {
        let base = node as usize * self.stride;
        let deg = self.data[base] as usize;
        &self.data[base + 1..base + 1 + deg]
    }

    /// 追加一条边（不检查重复，调用方保证）
    fn push_neighbor(&mut self, node: u32, nb: u32) {
        let base = node as usize * self.stride;
        let deg = self.data[base] as usize;
        if deg + 1 < self.stride {
            self.data[base + 1 + deg] = nb;
            self.data[base] = (deg + 1) as u32;
        }
    }

    /// 替换整个邻居列表
    fn set_neighbors(&mut self, node: u32, nbs: &[u32]) {
        let base = node as usize * self.stride;
        let count = nbs.len().min(self.stride - 1);
        self.data[base] = count as u32;
        for i in 0..count {
            self.data[base + 1 + i] = nbs[i];
        }
        for i in count..(self.stride - 1) {
            self.data[base + 1 + i] = EMPTY_NB;
        }
    }

    fn contains(&self, node: u32, nb: u32) -> bool {
        self.neighbors(node).contains(&nb)
    }

    fn reset_full(&mut self, nodes: usize) {
        self.data.clear();
        self.data.resize(nodes * self.stride, EMPTY_NB);
        for node in 0..nodes {
            self.data[node * self.stride] = 0;
        }
    }
}

// ── BQ-Vamana 图 ──

struct ConcurrentFlatAdj {
    data: Box<[UnsafeCell<u32>]>,
    stride: usize,
    nodes: usize,
}

unsafe impl Sync for ConcurrentFlatAdj {}

impl ConcurrentFlatAdj {
    fn new(nodes: usize, stride: usize) -> Self {
        let len = nodes.checked_mul(stride).expect("并发邻接表容量溢出");
        let mut data = Vec::with_capacity(len);
        for i in 0..len {
            let value = if i % stride == 0 { 0 } else { EMPTY_NB };
            data.push(UnsafeCell::new(value));
        }
        Self {
            data: data.into_boxed_slice(),
            stride,
            nodes,
        }
    }

    #[inline(always)]
    fn check_node(&self, node: u32) {
        debug_assert!((node as usize) < self.nodes);
        debug_assert!((node as usize + 1) * self.stride <= self.data.len());
    }

    fn neighbors_locked(&self, node: u32, locks: &StripedSpinLocks) -> Vec<u32> {
        let guard = locks.lock(node);
        self.neighbors_with_guard(&guard)
    }

    fn neighbors_raw(&self, node: u32) -> Vec<u32> {
        self.check_node(node);
        unsafe {
            let base = node as usize * self.stride;
            let deg = *self.data[base].get() as usize;
            debug_assert!(deg < self.stride);
            let mut out = Vec::with_capacity(deg);
            for i in 0..deg.min(self.stride - 1) {
                let nb = *self.data[base + 1 + i].get();
                if nb != EMPTY_NB && (nb as usize) < self.nodes {
                    out.push(nb);
                }
            }
            out
        }
    }

    fn neighbors_with_guard(&self, guard: &NodeGuard<'_>) -> Vec<u32> {
        self.neighbors_raw(guard.node())
    }

    fn set_neighbors_raw(&self, node: u32, nbs: &[u32]) {
        self.check_node(node);
        unsafe {
            let base = node as usize * self.stride;
            let count = nbs.len().min(self.stride - 1);
            *self.data[base].get() = count as u32;
            for i in 0..count {
                debug_assert!((nbs[i] as usize) < self.nodes);
                *self.data[base + 1 + i].get() = nbs[i];
            }
            for i in count..(self.stride - 1) {
                *self.data[base + 1 + i].get() = EMPTY_NB;
            }
        }
    }

    fn set_neighbors_with_guard(&self, guard: &NodeGuard<'_>, nbs: &[u32]) {
        self.set_neighbors_raw(guard.node(), nbs);
    }

    fn set_neighbors_locked(&self, node: u32, nbs: &[u32], locks: &StripedSpinLocks) {
        let guard = locks.lock(node);
        self.set_neighbors_with_guard(&guard, nbs);
    }

    fn set_neighbors_locked_fast(&self, node: u32, nbs: &[u32], locks: &StripedSpinLocks) {
        let _guard = locks.lock(node);
        self.set_neighbors_raw(node, nbs);
    }

    fn freeze_into_flat(self, dst: &mut FlatAdj) {
        dst.data.clear();
        dst.data.reserve(self.data.len());
        for cell in self.data.iter() {
            unsafe {
                dst.data.push(*cell.get());
            }
        }
        dst.stride = self.stride;
    }
}

struct ExperimentalBuildView<'a> {
    adj: &'a ConcurrentFlatAdj,
    n: usize,
    dim: usize,
    m0: usize,
    ef: usize,
    alpha: f32,
    sigs: &'a Bq2Store,
    locks: &'a StripedSpinLocks,
}

impl ExperimentalBuildView<'_> {
    fn beam_search_l0_locked(
        &self,
        q_sig: &Bq2Signature,
        entry: u32,
        ef: usize,
        visited: &mut Bitset,
    ) -> Vec<(NonNanF32, u32)> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        visited.clear();

        let mut candidates: BinaryHeap<Reverse<(NonNanF32, u32)>> = BinaryHeap::with_capacity(ef * 2);
        let mut results: BinaryHeap<(NonNanF32, u32)> = BinaryHeap::with_capacity(ef + 1);

        let d = NonNanF32(self.sigs.distance_to_sig(entry as usize, q_sig, self.dim) as f32);
        visited.set(entry as usize);
        candidates.push(Reverse((d, entry)));
        results.push((d, entry));

        while let Some(Reverse((cd, cur))) = candidates.pop() {
            if results.len() >= ef && cd > results.peek().unwrap().0 {
                break;
            }

            let nbs = self.adj.neighbors_locked(cur, self.locks);

            // 批量预取未访问邻居的 BQ 签名
            for &nb in &nbs {
                if !visited.test(nb as usize) {
                    self.sigs.prefetch_sig(nb as usize);
                }
            }

            for nb in nbs {
                if visited.test(nb as usize) {
                    continue;
                }
                visited.set(nb as usize);

                let nd = NonNanF32(self.sigs.distance_to_sig(nb as usize, q_sig, self.dim) as f32);
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

    fn connect_node_checked(&self, idx: u32, visited: &mut Bitset) {
        if idx == 0 {
            return;
        }

        let my_sig = self.sigs.get_sig(idx as usize);
        let candidates_sym = self.beam_search_l0_locked(&my_sig, 0, self.ef, visited);
        let mut candidates: Vec<(u32, u32)> = candidates_sym
            .into_iter()
            .filter(|&(_, id)| id != idx)
            .map(|(_, id)| (self.sigs.distance(idx as usize, id as usize, self.dim), id))
            .collect();

        let samples = self.ef.min(128).max(self.m0 * 2);
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
                candidates.push((self.sigs.distance(idx as usize, id as usize, self.dim), id));
            }
        }

        candidates.sort_unstable_by_key(|&(_, id)| id);
        candidates.dedup_by_key(|&mut (_, id)| id);
        candidates.sort_unstable_by_key(|&(d, _)| d);

        if candidates.is_empty() {
            candidates.push((self.sigs.distance(idx as usize, 0, self.dim), 0));
        }

        let selected =
            QuIVer::vamana_select(self.sigs, idx, &candidates, self.m0, self.dim, self.alpha);
        self.adj.set_neighbors_locked(idx, &selected, self.locks);

        for &nb in &selected {
            let guard = self.locks.lock(nb);
            let mut current = self.adj.neighbors_with_guard(&guard);
            if !current.contains(&idx) {
                current.push(idx);
            }
            let mut nb_candidates: Vec<(u32, u32)> = current
                .into_iter()
                .filter(|&n| n != nb)
                .map(|n| {
                    (
                        self.sigs.distance(nb as usize, n as usize, self.dim),
                        n,
                    )
                })
                .collect();
            nb_candidates.sort_unstable_by_key(|&(_, id)| id);
            nb_candidates.dedup_by_key(|&mut (_, id)| id);
            nb_candidates.sort_unstable_by_key(|&(d, _)| d);
            let pruned =
                QuIVer::vamana_select(self.sigs, nb, &nb_candidates, self.m0, self.dim, self.alpha);
            self.adj.set_neighbors_with_guard(&guard, &pruned);
        }
    }

    fn connect_node_fast(&self, idx: u32, visited: &mut Bitset) {
        if idx == 0 {
            return;
        }

        let my_sig = self.sigs.get_sig(idx as usize);
        let candidates_sym = self.beam_search_l0_locked(&my_sig, 0, self.ef, visited);
        let mut candidates: Vec<(u32, u32)> = candidates_sym
            .into_iter()
            .filter(|&(_, id)| id != idx)
            .map(|(_, id)| (self.sigs.distance(idx as usize, id as usize, self.dim), id))
            .collect();

        let samples = self.ef.min(128).max(self.m0 * 2);
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
                candidates.push((self.sigs.distance(idx as usize, id as usize, self.dim), id));
            }
        }

        candidates.sort_unstable_by_key(|&(_, id)| id);
        candidates.dedup_by_key(|&mut (_, id)| id);
        candidates.sort_unstable_by_key(|&(d, _)| d);

        if candidates.is_empty() {
            candidates.push((self.sigs.distance(idx as usize, 0, self.dim), 0));
        }

        let selected =
            QuIVer::vamana_select(self.sigs, idx, &candidates, self.m0, self.dim, self.alpha);
        self.adj
            .set_neighbors_locked_fast(idx, &selected, self.locks);

        for &nb in &selected {
            let _guard = self.locks.lock(nb);
            let mut current = self.adj.neighbors_raw(nb);
            if !current.contains(&idx) {
                current.push(idx);
            }
            let mut nb_candidates: Vec<(u32, u32)> = current
                .into_iter()
                .filter(|&n| n != nb)
                .map(|n| {
                    (
                        self.sigs.distance(nb as usize, n as usize, self.dim),
                        n,
                    )
                })
                .collect();
            nb_candidates.sort_unstable_by_key(|&(_, id)| id);
            nb_candidates.dedup_by_key(|&mut (_, id)| id);
            nb_candidates.sort_unstable_by_key(|&(d, _)| d);
            let pruned =
                QuIVer::vamana_select(self.sigs, nb, &nb_candidates, self.m0, self.dim, self.alpha);
            self.adj.set_neighbors_raw(nb, &pruned);
        }
    }
}

pub struct QuIVer {
    dim: usize,
    n: usize,
    m: usize,
    m0: usize,
    ef_construction: usize,
    ml: f64,
    alpha: f32,

    // Hot（常驻内存）
    bq_store: Bq2Store,
    layer0: FlatAdj,
    upper_layers: Vec<Vec<Vec<u32>>>,
    node_max_layer: Vec<u8>,

    // ID 映射
    ids: Vec<u64>,                             // internal_index → NodeId
    slot_indices: Vec<usize>,                  // internal_index → MemTable slot_index（精排用）
    id_to_internal: std::collections::HashMap<u64, u32>, // NodeId → internal_index（增量操作用）

    entry_point: u32,
    max_level: usize,

    // 增量管理
    tombstones: Vec<bool>,                     // internal_index → 是否已删除
    dirty_count: usize,                        // 增量变更计数（tombstone + 追加节点）

    visited: Bitset, // 建图期间复用，搜索时每次新建
}

pub struct QuIVerConfig {
    pub m: usize,
    pub ef_construction: usize,
    /// Vamana α 参数：1.0=严格剪枝，1.2=推荐值，越大图越密
    pub alpha: f32,
}

impl Default for QuIVerConfig {
    fn default() -> Self {
        Self {
            m: 16,
            ef_construction: 128,
            alpha: 1.2,
        }
    }
}

pub struct QuIVerSearchConfig {
    pub top_k: usize,
    pub ef_search: usize,
    pub rerank_limit: Option<usize>,
}

impl QuIVerSearchConfig {
    #[inline]
    fn rerank_limit(&self) -> usize {
        self.rerank_limit.unwrap_or(self.ef_search).max(self.top_k)
    }
}

impl QuIVer {
    pub fn new(dim: usize, config: &QuIVerConfig) -> Self {
        let m = config.m;
        let m0 = m * 2;
        Self {
            dim,
            n: 0,
            m,
            m0,
            ef_construction: config.ef_construction,
            ml: 1.0 / (m as f64).ln(),
            alpha: config.alpha,
            bq_store: Bq2Store::new(dim),
            layer0: FlatAdj::new(m0 * 2 + 1),
            upper_layers: Vec::new(),
            node_max_layer: Vec::new(),
            ids: Vec::new(),
            slot_indices: Vec::new(),
            id_to_internal: std::collections::HashMap::new(),
            entry_point: 0,
            max_level: 0,
            tombstones: Vec::new(),
            dirty_count: 0,
            visited: Bitset::new(0),
        }
    }

    fn random_level(&self, lcg: &mut u64) -> usize {
        *lcg = lcg
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let r = ((*lcg >> 33) as f64 / (1u64 << 31) as f64).max(1e-15);
        (-r.ln() * self.ml).floor() as usize
    }

    pub fn insert(&mut self, vector: &[f32], id: u64, slot_index: usize, lcg: &mut u64) {
        assert_eq!(vector.len(), self.dim);
        let idx = self.n as u32;

        let sig = Bq2Signature::from_vector(vector);
        self.bq_store.push_sig(&sig);
        self.ids.push(id);
        self.slot_indices.push(slot_index);
        self.id_to_internal.insert(id, idx);
        self.tombstones.push(false);

        let level = self.random_level(lcg);
        self.node_max_layer.push(level as u8);

        // 扩展 layer0
        self.layer0.push_empty();

        // 扩展 upper layers
        while self.upper_layers.len() < level {
            // upper_layers[0] = layer 1, [1] = layer 2, ...
            self.upper_layers.push(vec![Vec::new(); self.n]);
        }
        for ul in self.upper_layers.iter_mut() {
            ul.push(Vec::new());
        }

        self.n += 1;

        // 确保 visited bitset 够大
        self.visited.grow(self.n);

        if self.n == 1 {
            self.entry_point = 0;
            self.max_level = level;
            return;
        }

        let my_sig = self.bq_store.get_sig(idx as usize);
        let mut cur_node = self.entry_point;

        // ── 高层贪心下降（BQ 距离，不需要 f32 向量） ──
        for l in ((level + 1)..=self.max_level).rev() {
            let ul_idx = l - 1;
            if ul_idx < self.upper_layers.len() {
                loop {
                    let mut changed = false;
                    let cur_d = self.bq_store.distance(idx as usize, cur_node as usize, self.dim);
                    let mut best_d = cur_d;

                    for &nb in &self.upper_layers[ul_idx][cur_node as usize] {
                        let nd = self.bq_store.distance(idx as usize, nb as usize, self.dim);
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

        // 3. 高层搜索
        let mut visited = Bitset::new(self.n);
        for l in (level..=self.max_level).rev() {
            let res = self.beam_search_upper(&my_sig, l, cur_node, 1, &mut visited);
            if !res.is_empty() {
                cur_node = res[0].1;
            }
        }

        // 4. 新节点插入各层
        for l in (0..=level).rev() {
            let ef = self.ef_construction;
            let candidates_sym = if l == 0 {
                self.beam_search_l0(&my_sig, cur_node, ef, &mut visited)
            } else {
                self.beam_search_upper(&my_sig, l, cur_node, ef, &mut visited)
            };
            let candidates: Vec<(u32, u32)> = candidates_sym
                .into_iter()
                .map(|(_, id)| (self.bq_store.distance(idx as usize, id as usize, self.dim), id))
                .collect();

            let max_nb = if l == 0 { self.m0 } else { self.m };
            let selected = self.select_neighbors(idx, &candidates, max_nb);

            if l == 0 {
                // 正向边：idx → selected
                for &nb in &selected {
                    if !self.layer0.contains(idx, nb) {
                        self.layer0.push_neighbor(idx, nb);
                    }
                }

                // ── Vamana 双向剪枝（核心）──
                // 对于每个被选中的邻居 nb，将 idx 加入 nb 的候选集，
                // 然后用 vamana_select 重新决定 nb 应该保留哪些邻居。
                // 这保证了反向边也经过多样性剪枝，而不是盲目堆积。
                for &nb in &selected {
                    // 收集 nb 的当前邻居 + 新候选 idx
                    let mut nb_candidates: Vec<(u32, u32)> = self
                        .layer0
                        .neighbors(nb)
                        .iter()
                        .map(|&n| {
                            (
                                self.bq_store.distance(nb as usize, n as usize, self.dim),
                                n,
                            )
                        })
                        .collect();

                    // 如果 idx 还不在 nb 的邻居中，加入候选
                    if !self.layer0.contains(nb, idx) {
                        let d = self.bq_store.distance(nb as usize, idx as usize, self.dim);
                        nb_candidates.push((d, idx));
                    }

                    // 按距离排序后做 Vamana 剪枝
                    nb_candidates.sort_unstable_by_key(|&(d, _)| d);
                    let pruned = Self::vamana_select(
                        &self.bq_store,
                        nb,
                        &nb_candidates,
                        self.m0,
                        self.dim,
                        self.alpha,
                    );
                    self.layer0.set_neighbors(nb, &pruned);
                }
            } else {
                let ul = l - 1;
                // 正向边
                for &nb in &selected {
                    if !self.upper_layers[ul][idx as usize].contains(&nb) {
                        self.upper_layers[ul][idx as usize].push(nb);
                    }
                }

                // 双向剪枝（上层）
                for &nb in &selected {
                    let mut nb_candidates: Vec<(u32, u32)> = self.upper_layers[ul][nb as usize]
                        .iter()
                        .map(|&n| {
                            (
                                self.bq_store.distance(nb as usize, n as usize, self.dim),
                                n,
                            )
                        })
                        .collect();

                    if !self.upper_layers[ul][nb as usize].contains(&idx) {
                        let d = self.bq_store.distance(nb as usize, idx as usize, self.dim);
                        nb_candidates.push((d, idx));
                    }

                    nb_candidates.sort_unstable_by_key(|&(d, _)| d);
                    let pruned = Self::vamana_select(
                        &self.bq_store,
                        nb,
                        &nb_candidates,
                        self.m,
                        self.dim,
                        self.alpha,
                    );
                    self.upper_layers[ul][nb as usize] = pruned;
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

    /// Vamana 选边
    fn select_neighbors(&self, target: u32, candidates: &[(u32, u32)], max_k: usize) -> Vec<u32> {
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
    fn beam_search_l0(
        &self,
        q_sig: &Bq2Signature,
        entry: u32,
        ef: usize,
        visited: &mut Bitset,
    ) -> Vec<(NonNanF32, u32)> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        visited.clear();

        let mut candidates: BinaryHeap<Reverse<(NonNanF32, u32)>> = BinaryHeap::with_capacity(ef * 2);
        let mut results: BinaryHeap<(NonNanF32, u32)> = BinaryHeap::with_capacity(ef + 1);

        let d = NonNanF32(self.bq_store.distance_to_sig_cheap(entry as usize, q_sig, self.dim) as f32);
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

                let nd = NonNanF32(self.bq_store.distance_to_sig_cheap(nb as usize, q_sig, self.dim) as f32);
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
            item.0 = NonNanF32(self.bq_store.distance_to_sig(item.1 as usize, q_sig, self.dim) as f32);
        }
        res.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        res
    }

    /// Upper layer Symmetric beam search
    fn beam_search_upper(
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

        let d = NonNanF32(self.bq_store.distance_to_sig(entry as usize, q_sig, self.dim) as f32);
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
                let nd = NonNanF32(self.bq_store.distance_to_sig(nb as usize, q_sig, self.dim) as f32);
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
    fn vamana_select(
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
    pub fn search<F>(
        &self,
        query: &[f32],
        mut get_vec_f32: F,
        config: &QuIVerSearchConfig,
    ) -> Vec<(u64, f32)>
    where
        F: FnMut(usize, &mut Vec<f32>) -> bool,
    {
        if self.n == 0 {
            return Vec::new();
        }
        assert_eq!(query.len(), self.dim);
        let dim = self.dim;

        let q_sig = Bq2Signature::from_vector(query);
        let mut cur_node = self.entry_point;

        // 高层贪心下降
        for l in (1..=self.max_level).rev() {
            let ul = l - 1;
            if ul < self.upper_layers.len() {
                loop {
                    let mut changed = false;
                    let cur_d = self.bq_store.distance_to_sig(cur_node as usize, &q_sig, dim);
                    let mut best_d = cur_d;
                    for &nb in &self.upper_layers[ul][cur_node as usize] {
                        let nd = self.bq_store.distance_to_sig(nb as usize, &q_sig, dim);
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

        // Stage 1: Symmetric BQ beam search
        // 使用 thread_local Bitset 复用，避免每次查询 heap alloc
        thread_local! {
            static VISITED: std::cell::RefCell<Bitset> = std::cell::RefCell::new(Bitset::new(0));
        }
        let bq_candidates = VISITED.with(|cell| {
            let mut visited = cell.borrow_mut();
            if visited.len < self.n {
                visited.grow(self.n);
            }
            self.beam_search_l0(&q_sig, cur_node, config.ef_search, &mut visited)
        });

        // Stage 2: f32 按需精排（跳过 tombstone，通过 slot_indices 映射到冷存储）
        // 复用单个缓冲区，避免每个候选 heap alloc；冷向量按需取回仅触达 ~ef 个候选。
        let rerank_limit = config.rerank_limit().min(bq_candidates.len());
        let mut buf: Vec<f32> = Vec::with_capacity(dim);
        let mut scored: Vec<(f32, u32)> = Vec::with_capacity(rerank_limit);
        for &(_, nid) in bq_candidates.iter().take(rerank_limit) {
            if self.tombstones[nid as usize] {
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

    /// 便捷封装：从连续 f32 数组（按 slot 偏移）精排。
    ///
    /// 主要用于纯内存场景与单元测试。生产检索路径应直接调用 `search`
    /// 并传入按需读取 mmap 冷存储的回调，以保持冷热分离。
    pub fn search_flat(
        &self,
        query: &[f32],
        ext_vectors: &[f32],
        config: &QuIVerSearchConfig,
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

    /// ═══ 并行批量构建（rayon 加速）═══
    ///
    /// 利用 rayon 并行计算 BQ 签名，预分配所有数据结构，
    /// 然后顺序插入建图。相比逐个 insert 减少了：
    ///   - BQ 签名计算时间（并行化）
    ///   - 动态扩容开销（一次性预分配）
    ///   - Visited bitset 重复分配（复用）
    pub fn batch_build(&mut self, vectors: &[f32], ids: &[u64], slot_idxs: &[usize]) {
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
                            .map(|&n| {
                                (
                                    self.bq_store.distance(nb as usize, n as usize, dim),
                                    n,
                                )
                            })
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

    pub fn batch_build_experimental_v2_checked(&mut self, vectors: &[f32], ids: &[u64], slot_idxs: &[usize]) {
        self.batch_build_experimental_v2_impl(vectors, ids, slot_idxs, true);
    }

    pub fn batch_build_experimental_v2(&mut self, vectors: &[f32], ids: &[u64], slot_idxs: &[usize]) {
        self.batch_build_experimental_v2_impl(vectors, ids, slot_idxs, false);
    }

    // ── 实验代码路径 (feature = "ablation") ────────────────────────
    // 以下方法仅用于论文 encoding ablation 实验，不属于生产 API。
    // 正常构建不会编译此段代码。
    // 对应 bench: benches/bench_encoding_ablation.rs
    // ──────────────────────────────────────────────────────────────

    /// 使用外部预构建的 Bq2Store 构建图（**仅限实验**）。
    ///
    /// 与 `batch_build_experimental_v2` 逻辑完全相同，但跳过内部
    /// `push_from_vector` 编码步骤，直接使用传入的 `store`。
    /// 用于对比不同编码方案（1-bit sign / 2-bit SM）对图拓扑的影响。
    ///
    /// 调用方需保证 `store.len() == ids.len()`。
    #[cfg(feature = "ablation")]
    pub fn batch_build_with_store(&mut self, vectors: &[f32], ids: &[u64], slot_idxs: &[usize], store: Bq2Store) {
        let n = ids.len();
        let dim = self.dim;
        assert_eq!(vectors.len(), n * dim);
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
        let view = ExperimentalBuildView {
            adj: &concurrent_adj,
            n,
            dim,
            m0: self.m0,
            ef: self.ef_construction,
            alpha: self.alpha,
            sigs: &self.bq_store,
            locks: &locks,
        };

        let chunk = 256usize;
        let rounds = n.div_ceil(chunk);
        for round in 0..rounds {
            let start = round * chunk;
            let end = ((round + 1) * chunk).min(n);
            (start..end).into_par_iter().for_each(|i| {
                let mut visited = Bitset::new(n);
                view.connect_node_fast(i as u32, &mut visited);
            });
        }

        concurrent_adj.freeze_into_flat(&mut self.layer0);
    }

    /// 获取 Layer 0 节点的邻居列表（**仅限实验**）。
    #[cfg(feature = "ablation")]
    pub fn layer0_neighbors(&self, node: u32) -> &[u32] {
        self.layer0.neighbors(node)
    }

    /// 获取图的入口节点索引（**仅限实验**）。
    #[cfg(feature = "ablation")]
    pub fn ablation_entry_point(&self) -> u32 {
        self.entry_point as u32
    }

    fn batch_build_experimental_v2_impl(&mut self, vectors: &[f32], ids: &[u64], slot_idxs: &[usize], checked: bool) {
        let n = ids.len();
        let dim = self.dim;
        assert_eq!(vectors.len(), n * dim);
        assert_eq!(slot_idxs.len(), n);
        if n == 0 {
            return;
        }

        let mut store = Bq2Store::new(dim);
        store.reserve(n);
        for chunk in vectors.chunks(dim) {
            store.push_from_vector(chunk);
        }

        let mut lcg: u64 = 12345;
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

        let locks = StripedSpinLocks::new(n, self.m0);
        let concurrent_adj = ConcurrentFlatAdj::new(n, self.layer0.stride);
        let view = ExperimentalBuildView {
            adj: &concurrent_adj,
            n,
            dim,
            m0: self.m0,
            ef: self.ef_construction,
            alpha: self.alpha,
            sigs: &self.bq_store,
            locks: &locks,
        };

        let chunk = 256usize;
        let rounds = n.div_ceil(chunk);
        for round in 0..rounds {
            let start = round * chunk;
            let end = ((round + 1) * chunk).min(n);
            (start..end).into_par_iter().for_each(|i| {
                let mut visited = Bitset::new(n);
                if checked {
                    view.connect_node_checked(i as u32, &mut visited);
                } else {
                    view.connect_node_fast(i as u32, &mut visited);
                }
            });
        }

        concurrent_adj.freeze_into_flat(&mut self.layer0);
        if cfg!(debug_assertions)
            || std::env::var("TRIVIUM_BQ_VAMANA_VALIDATE").as_deref() == Ok("1")
        {
            self.validate_layer0();
        }
    }

    pub fn batch_build_experimental(&mut self, vectors: &[f32], ids: &[u64], slot_idxs: &[usize]) {
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
                            .map(|&n| {
                                (
                                    this.bq_store.distance(nb as usize, n as usize, dim),
                                    n,
                                )
                            })
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

    fn validate_layer0(&self) {
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

    pub fn stats(&self) -> QuIVerStats {
        let hot_bq = self.bq_store.hot_bytes();
        let hot_l0 = self.layer0.data.len() * 4;
        let hot_upper: usize = self
            .upper_layers
            .iter()
            .map(|l| l.iter().map(|adj| adj.len() * 4 + 24).sum::<usize>())
            .sum();
        let tombstone_count = self.tombstones.iter().filter(|&&t| t).count();

        QuIVerStats {
            n: self.n,
            max_level: self.max_level,
            hot_bytes: hot_bq + hot_l0 + hot_upper,
            tombstone_count,
            dirty_count: self.dirty_count,
            avg_degree_l0: if self.n > 0 {
                (0..self.n)
                    .map(|i| self.layer0.degree(i as u32))
                    .sum::<usize>() as f64
                    / self.n as f64
            } else {
                0.0
            },
        }
    }

    /// 图连通性诊断
    pub fn debug_connectivity(&self) {
        // 度数分布
        let mut deg0 = 0usize;
        let mut min_deg = usize::MAX;
        let mut max_deg = 0usize;
        for i in 0..self.n {
            let d = self.layer0.degree(i as u32);
            if d == 0 {
                deg0 += 1;
            }
            min_deg = min_deg.min(d);
            max_deg = max_deg.max(d);
        }
        eprintln!(
            "      [debug] L0 度数: min={} max={} 孤立节点={}/{}",
            min_deg, max_deg, deg0, self.n
        );

        // BFS 从入口点测可达性
        let mut visited = vec![false; self.n];
        let mut queue = std::collections::VecDeque::new();
        visited[self.entry_point as usize] = true;
        queue.push_back(self.entry_point);
        let mut reached = 1usize;
        while let Some(cur) = queue.pop_front() {
            for &nb in self.layer0.neighbors(cur) {
                if !visited[nb as usize] {
                    visited[nb as usize] = true;
                    queue.push_back(nb);
                    reached += 1;
                }
            }
        }
        eprintln!(
            "      [debug] BFS 从入口点可达: {}/{} ({:.1}%)",
            reached,
            self.n,
            100.0 * reached as f64 / self.n as f64
        );

        // 入口点邻居数
        eprintln!(
            "      [debug] 入口点={} 度数={}",
            self.entry_point,
            self.layer0.degree(self.entry_point)
        );
    }

    // ── 增量管理接口 ──

    /// 软删除：标记 tombstone，搜索时跳过（节点仍参与图遍历作为中转）
    pub fn soft_delete(&mut self, id: u64) -> bool {
        if let Some(&idx) = self.id_to_internal.get(&id) {
            if !self.tombstones[idx as usize] {
                self.tombstones[idx as usize] = true;
                self.dirty_count += 1;
            }
            true
        } else {
            false
        }
    }

    /// 是否需要全量重建（增量变更超过 25%）
    #[inline]
    pub fn needs_rebuild(&self) -> bool {
        self.n > 0 && self.dirty_count * 4 > self.n
    }

    /// 活跃节点数（总数 - tombstone 数）
    #[inline]
    pub fn active_count(&self) -> usize {
        self.n - self.tombstones.iter().filter(|&&t| t).count()
    }

    /// 节点总数（含 tombstone）
    #[inline]
    pub fn total_count(&self) -> usize {
        self.n
    }

    /// 递增增量变更计数（增量追加节点时调用）
    #[inline]
    pub fn dirty_count_inc(&mut self) {
        self.dirty_count += 1;
    }

    // ── 持久化 ──

    const QUIVER_MAGIC: &'static [u8; 4] = b"QUIV";
    const QUIVER_VERSION: u32 = 1;

    /// 将 QuIVer 索引保存到文件（POD memcpy 格式）
    ///
    /// 文件结构：
    /// ```text
    /// [Magic "QUIV" 4B] [Version 4B] [Header 40B]
    /// [BQ Signatures: n × 128B]
    /// [Layer0 FlatAdj: n × stride × 4B]
    /// [Tombstones: n × 1B]
    /// [IDs: n × 8B]
    /// [SlotIndices: n × 8B]
    /// [NodeMaxLayer: n × 1B]
    /// [Upper Layers: 变长编码]
    /// ```
    pub fn save_to_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::{BufWriter, Write};

        let tmp_path = path.with_extension("quiver.tmp");
        let file = std::fs::File::create(&tmp_path)?;
        let mut w = BufWriter::new(file);

        // Magic + Version
        w.write_all(Self::QUIVER_MAGIC)?;
        w.write_all(&Self::QUIVER_VERSION.to_le_bytes())?;

        // Header (40 bytes)
        w.write_all(&(self.dim as u32).to_le_bytes())?;
        w.write_all(&(self.n as u32).to_le_bytes())?;
        w.write_all(&(self.m as u32).to_le_bytes())?;
        w.write_all(&(self.m0 as u32).to_le_bytes())?;
        w.write_all(&(self.ef_construction as u32).to_le_bytes())?;
        w.write_all(&self.alpha.to_le_bytes())?;
        w.write_all(&self.entry_point.to_le_bytes())?;
        w.write_all(&(self.max_level as u32).to_le_bytes())?;
        w.write_all(&(self.dirty_count as u32).to_le_bytes())?;
        w.write_all(&[0u8; 4])?; // 保留字段

        // BQ Signatures（紧凑格式：chunks + pos + strong）
        let chunks = self.bq_store.chunks();
        w.write_all(&(chunks as u32).to_le_bytes())?;
        w.write_all(bytemuck::cast_slice(self.bq_store.pos_data()))?;
        w.write_all(bytemuck::cast_slice(self.bq_store.strong_data()))?;

        // Layer0 FlatAdj (POD memcpy)
        w.write_all(bytemuck::cast_slice(&self.layer0.data))?;

        // Tombstones
        let tomb_bytes: Vec<u8> = self.tombstones.iter().map(|&t| if t { 1 } else { 0 }).collect();
        w.write_all(&tomb_bytes)?;

        // IDs
        for &id in &self.ids {
            w.write_all(&id.to_le_bytes())?;
        }

        // SlotIndices
        for &si in &self.slot_indices {
            w.write_all(&(si as u64).to_le_bytes())?;
        }

        // NodeMaxLayer
        w.write_all(&self.node_max_layer)?;

        // Upper Layers (变长编码)
        w.write_all(&(self.upper_layers.len() as u32).to_le_bytes())?;
        for layer in &self.upper_layers {
            w.write_all(&(layer.len() as u32).to_le_bytes())?;
            for adj in layer {
                w.write_all(&(adj.len() as u16).to_le_bytes())?;
                for &nb in adj {
                    w.write_all(&nb.to_le_bytes())?;
                }
            }
        }

        w.flush()?;
        let file = w.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);

        // 原子替换
        #[cfg(windows)]
        {
            // Windows: 需要先删除目标文件
            if path.exists() {
                std::fs::remove_file(path)?;
            }
        }
        std::fs::rename(&tmp_path, path)?;

        tracing::info!(
            "QuIVer 索引已持久化 (QuIVer index persisted)：{} 个节点，dim={}，max_level={}",
            self.n, self.dim, self.max_level
        );
        Ok(())
    }

    /// 从文件加载 QuIVer 索引
    pub fn load_from_file(path: &std::path::Path) -> std::io::Result<Self> {
        let data = std::fs::read(path)?;
        let bytes = &data[..];

        if bytes.len() < 48 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "QuIVer 文件太小 (QuIVer file too small)",
            ));
        }

        // Magic
        if &bytes[0..4] != Self::QUIVER_MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("无效的 QuIVer magic (Invalid QuIVer magic): {:?}", &bytes[0..4]),
            ));
        }

        // Version
        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if version != Self::QUIVER_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("不支持的 QuIVer 版本 (Unsupported QuIVer version): {}", version),
            ));
        }

        // Header
        let mut off = 8;
        let dim = u32::from_le_bytes(bytes[off..off+4].try_into().unwrap()) as usize; off += 4;
        let n = u32::from_le_bytes(bytes[off..off+4].try_into().unwrap()) as usize; off += 4;
        let m = u32::from_le_bytes(bytes[off..off+4].try_into().unwrap()) as usize; off += 4;
        let m0 = u32::from_le_bytes(bytes[off..off+4].try_into().unwrap()) as usize; off += 4;
        let ef_construction = u32::from_le_bytes(bytes[off..off+4].try_into().unwrap()) as usize; off += 4;
        let alpha = f32::from_le_bytes(bytes[off..off+4].try_into().unwrap()); off += 4;
        let entry_point = u32::from_le_bytes(bytes[off..off+4].try_into().unwrap()); off += 4;
        let max_level = u32::from_le_bytes(bytes[off..off+4].try_into().unwrap()) as usize; off += 4;
        let dirty_count = u32::from_le_bytes(bytes[off..off+4].try_into().unwrap()) as usize; off += 4;
        off += 4; // 保留字段

        // BQ Signatures（紧凑格式）
        let chunks = u32::from_le_bytes(bytes[off..off+4].try_into().unwrap()) as usize; off += 4;
        let bq_u64_count = n * chunks;
        let bq_bytes_per_array = bq_u64_count * 8;
        let pos_end = off + bq_bytes_per_array;
        let strong_end = pos_end + bq_bytes_per_array;
        if strong_end > bytes.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "BQ 签名数据不完整 (BQ signature data incomplete)"));
        }
        let pos_data: Vec<u64> = bytes[off..pos_end]
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let strong_data: Vec<u64> = bytes[pos_end..strong_end]
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let bq_store = Bq2Store::from_raw(pos_data, strong_data, chunks);
        off = strong_end;

        // Layer0 FlatAdj
        let stride = m0 * 2 + 1;
        let l0_size = n * stride * 4;
        let l0_end = off + l0_size;
        if l0_end > bytes.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Layer0 数据不完整 (Layer0 data incomplete)"));
        }
        let l0_data: Vec<u32> = bytes[off..l0_end]
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        off = l0_end;

        // Tombstones
        let tomb_end = off + n;
        if tomb_end > bytes.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Tombstone 数据不完整 (Tombstone data incomplete)"));
        }
        let tombstones: Vec<bool> = bytes[off..tomb_end].iter().map(|&b| b != 0).collect();
        off = tomb_end;

        // IDs
        let ids_end = off + n * 8;
        if ids_end > bytes.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "IDs 数据不完整 (IDs data incomplete)"));
        }
        let ids: Vec<u64> = bytes[off..ids_end]
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        off = ids_end;

        // SlotIndices
        let si_end = off + n * 8;
        if si_end > bytes.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "SlotIndices 数据不完整 (SlotIndices data incomplete)"));
        }
        let slot_indices: Vec<usize> = bytes[off..si_end]
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()) as usize)
            .collect();
        off = si_end;

        // NodeMaxLayer
        let nml_end = off + n;
        if nml_end > bytes.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "NodeMaxLayer 数据不完整 (NodeMaxLayer data incomplete)"));
        }
        let node_max_layer: Vec<u8> = bytes[off..nml_end].to_vec();
        off = nml_end;

        // Upper Layers
        if off + 4 > bytes.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Upper Layers header 不完整 (Upper Layers header incomplete)"));
        }
        let num_upper = u32::from_le_bytes(bytes[off..off+4].try_into().unwrap()) as usize;
        off += 4;

        let mut upper_layers = Vec::with_capacity(num_upper);
        for _ in 0..num_upper {
            if off + 4 > bytes.len() {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Upper layer 节点数不完整 (Upper layer node count incomplete)"));
            }
            let layer_nodes = u32::from_le_bytes(bytes[off..off+4].try_into().unwrap()) as usize;
            off += 4;

            let mut layer = Vec::with_capacity(layer_nodes);
            for _ in 0..layer_nodes {
                if off + 2 > bytes.len() {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Upper adj 度数不完整 (Upper adj degree incomplete)"));
                }
                let deg = u16::from_le_bytes(bytes[off..off+2].try_into().unwrap()) as usize;
                off += 2;

                if off + deg * 4 > bytes.len() {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Upper adj 邻居列表不完整 (Upper adj neighbor list incomplete)"));
                }
                let mut adj = Vec::with_capacity(deg);
                for _ in 0..deg {
                    adj.push(u32::from_le_bytes(bytes[off..off+4].try_into().unwrap()));
                    off += 4;
                }
                layer.push(adj);
            }
            upper_layers.push(layer);
        }

        // 构建反向映射
        let mut id_to_internal = std::collections::HashMap::with_capacity(n);
        for (i, &id) in ids.iter().enumerate() {
            id_to_internal.insert(id, i as u32);
        }

        tracing::info!(
            "QuIVer 索引从磁盘加载完成 (QuIVer index loaded from disk)：{} 个节点，dim={}，max_level={}，tombstone={}",
            n, dim, max_level, tombstones.iter().filter(|&&t| t).count()
        );

        Ok(Self {
            dim,
            n,
            m,
            m0,
            ef_construction,
            ml: 1.0 / (m as f64).ln(),
            alpha,
            bq_store,
            layer0: FlatAdj { data: l0_data, stride },
            upper_layers,
            node_max_layer,
            ids,
            slot_indices,
            id_to_internal,
            entry_point,
            max_level,
            tombstones,
            dirty_count,
            visited: Bitset::new(n),
        })
    }
}

pub struct QuIVerStats {
    pub n: usize,
    pub max_level: usize,
    pub hot_bytes: usize,
    pub tombstone_count: usize,
    pub dirty_count: usize,
    pub avg_degree_l0: f64,
}

#[inline]
pub fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    dot / (na.sqrt() * nb.sqrt()).max(1e-30)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造 N 个正交基向量（dim 维），每个向量只在第 i%dim 维为 1.0
    #[allow(dead_code)]
    fn make_orthogonal_vectors(n: usize, dim: usize) -> Vec<f32> {
        let mut vecs = vec![0.0f32; n * dim];
        for i in 0..n {
            vecs[i * dim + (i % dim)] = 1.0;
        }
        vecs
    }

    /// 构造随机向量（确定性 LCG）
    fn make_random_vectors(n: usize, dim: usize, seed: u64) -> Vec<f32> {
        let mut vecs = vec![0.0f32; n * dim];
        let mut lcg = seed;
        for v in vecs.iter_mut() {
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1);
            *v = ((lcg >> 33) as f32 / (1u32 << 31) as f32) * 2.0 - 1.0;
        }
        vecs
    }

    #[test]
    fn test_quiver_基础建图与搜索() {
        let config = QuIVerConfig {
            m: 16,
            ef_construction: 32,
            alpha: 1.2,
        };
        let mut quiver = QuIVer::new(4, &config);

        let vectors = vec![
            1.0f32, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];
        let ids = vec![10, 20, 30, 40];
        let slots = vec![0, 1, 2, 3];

        quiver.batch_build_experimental_v2(&vectors, &ids, &slots);

        assert_eq!(quiver.active_count(), 4);
        assert_eq!(quiver.total_count(), 4);

        let query = vec![1.0f32, 0.0, 0.0, 0.0];
        let search_config = QuIVerSearchConfig {
            top_k: 2,
            ef_search: 10,
            rerank_limit: None,
        };
        
        let results = quiver.search_flat(&query, &vectors, &search_config);
        assert!(!results.is_empty());
        assert_eq!(results[0].0, 10);

        // 测试软删除
        quiver.soft_delete(10);
        assert_eq!(quiver.active_count(), 3);
        
        let results_after = quiver.search_flat(&query, &vectors, &search_config);
        if !results_after.is_empty() {
            assert_ne!(results_after[0].0, 10);
        }

        let stats = quiver.stats();
        assert_eq!(stats.n, 4);
        assert_eq!(stats.tombstone_count, 1);
        
        assert!(!quiver.needs_rebuild());
    }

    #[test]
    fn test_quiver_多路径建图与序列化() {
        let config = QuIVerConfig {
            m: 16,
            ef_construction: 32,
            alpha: 1.2,
        };
        let mut quiver = QuIVer::new(4, &config);

        let vectors = vec![
            1.0f32, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];
        let ids = vec![10, 20, 30, 40];
        let slots = vec![0, 1, 2, 3];

        // 测试批量建图
        quiver.batch_build(&vectors, &ids, &slots);
        assert_eq!(quiver.total_count(), 4);

        // 测试追加式批量建图
        quiver.batch_build_experimental(&vectors, &[50, 60, 70, 80], &[4, 5, 6, 7]);
        assert_eq!(quiver.total_count(), 8);

        // 测试带校验的批量建图（会重置图）
        quiver.batch_build_experimental_v2_checked(&vectors, &ids, &slots);
        assert_eq!(quiver.total_count(), 4);

        // 测试单节点增量插入
        let mut lcg = 12345;
        let v5 = vec![0.5, 0.5, 0.0, 0.0];
        quiver.insert(&v5, 50, 4, &mut lcg);
        assert_eq!(quiver.total_count(), 5);

        quiver.dirty_count_inc();
        quiver.dirty_count_inc();
        assert!(quiver.needs_rebuild());

        quiver.debug_connectivity();

        // 测试序列化与反序列化
        let path = std::path::Path::new("test_quiver_ser.quiv");
        quiver.save_to_file(path).unwrap();

        let loaded = QuIVer::load_from_file(path).unwrap();
        assert_eq!(loaded.total_count(), 5);
        assert_eq!(loaded.active_count(), 5);
        assert_eq!(loaded.dim, 4);
        assert_eq!(loaded.entry_point, quiver.entry_point);
        assert_eq!(loaded.ids, vec![10, 20, 30, 40, 50]);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_quiver_中等规模搜索() {
        // 50 个节点，8 维，测试更完整的 beam search 路径
        let dim = 8;
        let n = 50;
        let config = QuIVerConfig {
            m: 8,
            ef_construction: 32,
            alpha: 1.2,
        };
        let mut quiver = QuIVer::new(dim, &config);

        let vectors = make_random_vectors(n, dim, 42);
        let ids: Vec<u64> = (0..n as u64).collect();
        let slots: Vec<usize> = (0..n).collect();

        quiver.batch_build_experimental_v2(&vectors, &ids, &slots);
        assert_eq!(quiver.total_count(), n);

        let stats = quiver.stats();
        assert!(stats.avg_degree_l0 > 0.0);
        assert!(stats.hot_bytes > 0);

        // 用第 0 号向量自身做查询，应该能搜到自己
        let query = vectors[0..dim].to_vec();
        let search_config = QuIVerSearchConfig {
            top_k: 5,
            ef_search: 32,
            rerank_limit: Some(20),
        };
        let results = quiver.search_flat(&query, &vectors, &search_config);
        assert!(!results.is_empty());
        assert_eq!(results[0].0, 0); // 自身应为 Top-1

        // 删除一半节点再搜索
        for i in 0..n/2 {
            quiver.soft_delete(i as u64);
        }
        assert_eq!(quiver.active_count(), n - n/2);
        let results2 = quiver.search_flat(&query, &vectors, &search_config);
        for (id, _) in &results2 {
            assert!(*id >= n as u64 / 2, "已删除节点不应出现在结果中");
        }
    }

    #[test]
    fn test_quiver_空图搜索() {
        let config = QuIVerConfig::default();
        let quiver = QuIVer::new(4, &config);
        assert_eq!(quiver.total_count(), 0);
        assert_eq!(quiver.active_count(), 0);

        let query = vec![1.0f32, 0.0, 0.0, 0.0];
        let search_config = QuIVerSearchConfig {
            top_k: 5,
            ef_search: 10,
            rerank_limit: None,
        };
        let results = quiver.search_flat(&query, &[], &search_config);
        assert!(results.is_empty());
    }

    #[test]
    fn test_quiver_单节点图() {
        let config = QuIVerConfig::default();
        let mut quiver = QuIVer::new(4, &config);

        let vectors = vec![1.0f32, 0.0, 0.0, 0.0];
        quiver.batch_build_experimental_v2(&vectors, &[42], &[0]);
        assert_eq!(quiver.total_count(), 1);

        let search_config = QuIVerSearchConfig {
            top_k: 1,
            ef_search: 10,
            rerank_limit: None,
        };
        let results = quiver.search_flat(&[1.0, 0.0, 0.0, 0.0], &vectors, &search_config);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 42);
    }

    #[test]
    fn test_quiver_soft_delete_不存在的id() {
        let config = QuIVerConfig::default();
        let mut quiver = QuIVer::new(4, &config);
        let vectors = vec![1.0, 0.0, 0.0, 0.0];
        quiver.batch_build_experimental_v2(&vectors, &[1], &[0]);

        // 删除不存在的 ID
        assert!(!quiver.soft_delete(999));
        // 删除存在的 ID
        assert!(quiver.soft_delete(1));
        // 重复删除（已经是 tombstone）
        assert!(quiver.soft_delete(1));
    }

    #[test]
    fn test_quiver_序列化错误路径() {
        // 加载不存在的文件
        let result = QuIVer::load_from_file(std::path::Path::new("nonexistent.quiv"));
        assert!(result.is_err());
    }

    #[test]
    fn test_cosine_sim_函数() {
        let a = [1.0f32, 0.0, 0.0];
        let b = [1.0f32, 0.0, 0.0];
        assert!((cosine_sim(&a, &b) - 1.0).abs() < 1e-5);

        let c = [0.0f32, 1.0, 0.0];
        assert!(cosine_sim(&a, &c).abs() < 1e-5);

        let d = [-1.0f32, 0.0, 0.0];
        assert!((cosine_sim(&a, &d) + 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_quiver_config_default() {
        let config = QuIVerConfig::default();
        assert_eq!(config.m, 16);
        assert_eq!(config.ef_construction, 128);
        assert!((config.alpha - 1.2).abs() < 1e-5);
    }

    #[test]
    fn test_quiver_search_config_rerank_limit() {
        let cfg = QuIVerSearchConfig {
            top_k: 10,
            ef_search: 50,
            rerank_limit: None,
        };
        assert_eq!(cfg.rerank_limit(), 50); // 默认取 ef_search

        let cfg2 = QuIVerSearchConfig {
            top_k: 10,
            ef_search: 50,
            rerank_limit: Some(30),
        };
        assert_eq!(cfg2.rerank_limit(), 30);

        let cfg3 = QuIVerSearchConfig {
            top_k: 100,
            ef_search: 50,
            rerank_limit: Some(30),
        };
        assert_eq!(cfg3.rerank_limit(), 100); // max(rerank_limit, top_k)
    }
}

