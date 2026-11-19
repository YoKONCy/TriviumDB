//! 邻接表、访问位图与并行构图锁。

use super::*;
// ── Bitset ──

pub(crate) struct Bitset {
    pub(crate) data: Vec<u64>,
    pub(crate) len: usize,
}
impl Bitset {
    pub(crate) fn new(n: usize) -> Self {
        Self {
            data: vec![0u64; n.div_ceil(64)],
            len: n,
        }
    }
    #[inline(always)]
    pub(crate) fn set(&mut self, i: usize) {
        self.data[i >> 6] |= 1u64 << (i & 63);
    }
    #[inline(always)]
    pub(crate) fn test(&self, i: usize) -> bool {
        (self.data[i >> 6] >> (i & 63)) & 1 != 0
    }
    pub(crate) fn clear(&mut self) {
        self.data.iter_mut().for_each(|x| *x = 0);
    }
    pub(crate) fn grow(&mut self, new_n: usize) {
        let need = new_n.div_ceil(64);
        if need > self.data.len() {
            self.data.resize(need, 0);
        }
        self.len = new_n;
    }
}
pub(crate) struct SparseBitset {
    pub(crate) data: Vec<u64>,
    pub(crate) touched_words: Vec<usize>,
}
pub(crate) struct SearchScratch384 {
    pub(crate) visited: SparseBitset,
    pub(crate) candidates: BinaryHeap<Reverse<(NonNanF32, u32)>>,
    pub(crate) results: BinaryHeap<(NonNanF32, u32)>,
    pub(crate) vector: Vec<f32>,
    pub(crate) scored: Vec<(f32, u32)>,
}
impl SearchScratch384 {
    pub(crate) fn new() -> Self {
        Self {
            visited: SparseBitset::new(0),
            candidates: BinaryHeap::new(),
            results: BinaryHeap::new(),
            vector: Vec::with_capacity(384),
            scored: Vec::new(),
        }
    }
}
impl SparseBitset {
    pub(crate) fn new(n: usize) -> Self {
        Self {
            data: vec![0; n.div_ceil(64)],
            touched_words: Vec::new(),
        }
    }

    pub(crate) fn begin(&mut self) {
        for word in self.touched_words.drain(..) {
            self.data[word] = 0;
        }
    }

    #[inline(always)]
    pub(crate) fn set(&mut self, index: usize) {
        let word = index >> 6;
        if self.data[word] == 0 {
            self.touched_words.push(word);
        }
        self.data[word] |= 1u64 << (index & 63);
    }

    #[inline(always)]
    pub(crate) fn test(&self, index: usize) -> bool {
        (self.data[index >> 6] >> (index & 63)) & 1 != 0
    }

    pub(crate) fn grow(&mut self, new_n: usize) {
        let words = new_n.div_ceil(64);
        if words > self.data.len() {
            self.data.resize(words, 0);
        }
    }
}
pub(crate) trait SearchVisited {
    fn begin(&mut self);
    fn set(&mut self, index: usize);
    fn test(&self, index: usize) -> bool;
}
impl SearchVisited for Bitset {
    fn begin(&mut self) {
        self.clear();
    }

    fn set(&mut self, index: usize) {
        self.set(index);
    }

    fn test(&self, index: usize) -> bool {
        self.test(index)
    }
}
impl SearchVisited for SparseBitset {
    fn begin(&mut self) {
        self.begin();
    }

    fn set(&mut self, index: usize) {
        self.set(index);
    }

    fn test(&self, index: usize) -> bool {
        self.test(index)
    }
}
// ── Flat neighbor list ──
// Layer 0: 每节点最多 m0 个邻居，存为 [degree, n0, n1, ..., n_{m0-1}] stride = m0+1
// Upper layers: 用 Vec<Vec<u32>>（节点少，性能不敏感）

pub(crate) const EMPTY_NB: u32 = u32::MAX;
pub(crate) struct FlatAdj {
    pub(crate) data: Vec<u32>, // n * stride 个 u32
    pub(crate) stride: usize,  // m0 + 1 (第一个元素是度数)
}
pub(crate) struct SpinLockGuard<'a> {
    pub(crate) lock: &'a AtomicBool,
}
impl Drop for SpinLockGuard<'_> {
    fn drop(&mut self) {
        self.lock.store(false, Ordering::Release);
    }
}
#[repr(transparent)]
pub(crate) struct NodeGuard<'a>(pub(crate) (u32, SpinLockGuard<'a>));
impl NodeGuard<'_> {
    #[inline(always)]
    pub(crate) fn node(&self) -> u32 {
        self.0.0
    }
}
pub(crate) struct StripedSpinLocks {
    pub(crate) locks: Vec<AtomicBool>,
    pub(crate) mask: usize,
}
impl StripedSpinLocks {
    pub(crate) fn new(nodes: usize, m: usize) -> Self {
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
    pub(crate) fn stripe(&self, node: u32) -> usize {
        const FIB: usize = 0x9E3779B97F4A7C15usize;
        (node as usize).wrapping_mul(FIB) & self.mask
    }

    #[inline(always)]
    pub(crate) fn lock(&self, node: u32) -> NodeGuard<'_> {
        let lock = &self.locks[self.stripe(node)];
        while lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        NodeGuard((node, SpinLockGuard { lock }))
    }

    /// 计时版加锁：返回自旋等待耗时(ns)，用于构图性能探针
    #[inline(always)]
    pub(crate) fn lock_timed(&self, node: u32) -> (NodeGuard<'_>, u64) {
        let lock = &self.locks[self.stripe(node)];
        let t0 = Instant::now();
        let mut spun = false;
        while lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spun = true;
            std::hint::spin_loop();
        }
        // 仅在确实自旋等待时计入耗时，避免无争用时 Instant 噪声主导
        let wait = if spun {
            t0.elapsed().as_nanos() as u64
        } else {
            0
        };
        (NodeGuard((node, SpinLockGuard { lock })), wait)
    }
}
/// 构图性能探针：通过环境变量 QUIVER_PROFILE_BUILD=1 启用，默认零开销。
/// 分段累计纳秒，定位 spin-lock 弱扩展的真正主因。
pub(crate) struct BuildProfile {
    pub(crate) enabled: bool,
    pub(crate) bitset_ns: AtomicU64,   // 每节点 Bitset 分配耗时
    pub(crate) beam_ns: AtomicU64,     // beam search 寻路耗时
    pub(crate) fwd_ns: AtomicU64,      // 前向候选准备+vamana_select+写自身边
    pub(crate) rev_wait_ns: AtomicU64, // 反向剪枝的自旋锁等待耗时
    pub(crate) rev_work_ns: AtomicU64, // 反向剪枝的锁内做活耗时(去掉等待)
    pub(crate) nodes: AtomicU64,       // 处理节点数
}
impl BuildProfile {
    pub(crate) fn new() -> Self {
        let enabled = crate::index::pelorus::engine::config::environment_flag(
            "QUIVER_PROFILE_BUILD",
            "TRIVIUM_BUILD_PROFILE",
        );
        Self {
            enabled,
            bitset_ns: AtomicU64::new(0),
            beam_ns: AtomicU64::new(0),
            fwd_ns: AtomicU64::new(0),
            rev_wait_ns: AtomicU64::new(0),
            rev_work_ns: AtomicU64::new(0),
            nodes: AtomicU64::new(0),
        }
    }

    #[inline(always)]
    pub(crate) fn add(&self, counter: &AtomicU64, ns: u64) {
        if self.enabled {
            counter.fetch_add(ns, Ordering::Relaxed);
        }
    }

    pub(crate) fn report(&self, wall_secs: f64, threads: usize) {
        if !self.enabled {
            return;
        }
        let nodes = self.nodes.load(Ordering::Relaxed).max(1);
        let ms = |c: &AtomicU64| c.load(Ordering::Relaxed) as f64 / 1e6;
        let per = |c: &AtomicU64| c.load(Ordering::Relaxed) as f64 / nodes as f64;
        let total_cpu_ms = ms(&self.bitset_ns)
            + ms(&self.beam_ns)
            + ms(&self.fwd_ns)
            + ms(&self.rev_wait_ns)
            + ms(&self.rev_work_ns);
        tracing::info!(
            nodes,
            threads,
            wall_seconds = wall_secs,
            total_cpu_ms,
            "构图性能统计完成"
        );
        let line = |name: &str, c: &AtomicU64| {
            let cpu_ms = ms(c);
            let pct = if total_cpu_ms > 0.0 {
                cpu_ms / total_cpu_ms * 100.0
            } else {
                0.0
            };
            tracing::info!(
                stage = name,
                cpu_ms,
                percent = pct,
                per_node_ns = per(c),
                "构图阶段性能"
            );
        };
        line("Bitset分配/allocate", &self.bitset_ns);
        line("束搜索寻路/beam search", &self.beam_ns);
        line("前向选边/forward select", &self.fwd_ns);
        line("反向剪枝锁等待/reverse wait", &self.rev_wait_ns);
        line("反向剪枝工作/reverse work", &self.rev_work_ns);
    }
}
impl FlatAdj {
    pub(crate) fn new(stride: usize) -> Self {
        Self {
            data: Vec::new(),
            stride,
        }
    }

    /// 为新节点追加空邻居列表
    pub(crate) fn push_empty(&mut self) {
        self.data.push(0); // degree = 0
        for _ in 1..self.stride {
            self.data.push(EMPTY_NB);
        }
    }

    #[inline(always)]
    pub(crate) fn degree(&self, node: u32) -> usize {
        self.data[node as usize * self.stride] as usize
    }

    #[inline(always)]
    pub(crate) fn neighbors(&self, node: u32) -> &[u32] {
        let base = node as usize * self.stride;
        let deg = self.data[base] as usize;
        &self.data[base + 1..base + 1 + deg]
    }

    /// 追加一条边（不检查重复，调用方保证）
    pub(crate) fn push_neighbor(&mut self, node: u32, nb: u32) {
        let base = node as usize * self.stride;
        let deg = self.data[base] as usize;
        if deg + 1 < self.stride {
            self.data[base + 1 + deg] = nb;
            self.data[base] = (deg + 1) as u32;
        }
    }

    /// 替换整个邻居列表
    pub(crate) fn set_neighbors(&mut self, node: u32, nbs: &[u32]) {
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

    pub(crate) fn contains(&self, node: u32, nb: u32) -> bool {
        self.neighbors(node).contains(&nb)
    }

    pub(crate) fn reset_full(&mut self, nodes: usize) {
        self.data.clear();
        self.data.resize(nodes * self.stride, EMPTY_NB);
        for node in 0..nodes {
            self.data[node * self.stride] = 0;
        }
    }
}
// ── BQ-Vamana 图 ──

pub(crate) struct ConcurrentFlatAdj {
    pub(crate) data: Box<[UnsafeCell<u32>]>,
    pub(crate) stride: usize,
    pub(crate) nodes: usize,
}
unsafe impl Sync for ConcurrentFlatAdj {}
impl ConcurrentFlatAdj {
    pub(crate) fn new(nodes: usize, stride: usize) -> Self {
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
    pub(crate) fn check_node(&self, node: u32) {
        debug_assert!((node as usize) < self.nodes);
        debug_assert!((node as usize + 1) * self.stride <= self.data.len());
    }

    pub(crate) fn neighbors_locked(&self, node: u32, locks: &StripedSpinLocks) -> Vec<u32> {
        let guard = locks.lock(node);
        self.neighbors_with_guard(&guard)
    }

    pub(crate) fn neighbors_raw(&self, node: u32) -> Vec<u32> {
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

    /// 邻接表单节点容量（不含度数槽）
    #[inline(always)]
    pub(crate) fn capacity(&self) -> usize {
        self.stride - 1
    }

    /// 读取节点当前度数（需持锁调用，惰性剪枝用）
    #[inline(always)]
    pub(crate) fn degree_raw(&self, node: u32) -> usize {
        self.check_node(node);
        unsafe { (*self.data[node as usize * self.stride].get() as usize).min(self.stride - 1) }
    }

    /// 判断节点邻居中是否已含 nb（需持锁调用，惰性剪枝用）
    pub(crate) fn contains_raw(&self, node: u32, nb: u32) -> bool {
        self.check_node(node);
        unsafe {
            let base = node as usize * self.stride;
            let deg = (*self.data[base].get() as usize).min(self.stride - 1);
            for i in 0..deg {
                if *self.data[base + 1 + i].get() == nb {
                    return true;
                }
            }
            false
        }
    }

    /// 追加一条边（不剪枝，调用方保证未满且无重复，需持锁调用）
    #[inline(always)]
    pub(crate) fn push_neighbor_raw(&self, node: u32, nb: u32) {
        self.check_node(node);
        unsafe {
            let base = node as usize * self.stride;
            let deg = *self.data[base].get() as usize;
            if deg + 1 < self.stride {
                *self.data[base + 1 + deg].get() = nb;
                *self.data[base].get() = (deg + 1) as u32;
            }
        }
    }

    pub(crate) fn neighbors_with_guard(&self, guard: &NodeGuard<'_>) -> Vec<u32> {
        self.neighbors_raw(guard.node())
    }

    pub(crate) fn set_neighbors_raw(&self, node: u32, nbs: &[u32]) {
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

    pub(crate) fn set_neighbors_with_guard(&self, guard: &NodeGuard<'_>, nbs: &[u32]) {
        self.set_neighbors_raw(guard.node(), nbs);
    }

    pub(crate) fn set_neighbors_locked(&self, node: u32, nbs: &[u32], locks: &StripedSpinLocks) {
        let guard = locks.lock(node);
        self.set_neighbors_with_guard(&guard, nbs);
    }

    pub(crate) fn set_neighbors_locked_fast(
        &self,
        node: u32,
        nbs: &[u32],
        locks: &StripedSpinLocks,
    ) {
        let _guard = locks.lock(node);
        self.set_neighbors_raw(node, nbs);
    }

    pub(crate) fn freeze_into_flat(self, dst: &mut FlatAdj) {
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
