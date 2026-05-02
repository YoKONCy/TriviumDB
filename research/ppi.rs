//! Permutation Proximity Index (PPI)
//!
//! 基于排列嵌入的 cache-friendly ANN 索引。
//!
//! # 数学基础
//!
//! 将 ℝ^d 的度量空间等距嵌入到对称群 S_K 的离散度量空间中：
//! 1. 生成 K 个数据无关的高斯随机锚点（Anchors）
//! 2. 每个向量按到锚点的距离排序 → 得到 1..K 的排列（Permutation）
//! 3. 排列编码为 u64 整数（K=16 时：16 × 4bit = 64bit）
//! 4. 按 u64 整数排序 → 隐式层次化 Voronoi 空间划分
//!
//! # 性质
//!
//! - 锚点为纯高斯随机，完全数据无关
//! - 排列相似度（Kendall-Tau / Spearman Footrule）与原始距离强相关
//! - u64 排序后的数组 = 隐式 K 叉空间划分树，无需指针
//! - 查询 = 二分定位 + 线性扫描，纯连续内存访问

/// 排列近邻索引
pub struct PpiIndex {
    dim: usize,
    n: usize,
    num_anchors: usize,

    // ═══ 连续存储 ═══
    /// 重排后的向量，物理连续
    vectors: Vec<f32>,
    /// 原始 ID（与 vectors 同序）
    ids: Vec<u64>,
    /// 排列签名（已排序），与 vectors 同序
    signatures: Vec<u64>,

    // ═══ 锚点 ═══
    /// K 个随机锚点，每个 dim 维
    anchors: Vec<f32>,
}

/// 查询配置
pub struct PpiQuery {
    pub top_k: usize,
    /// 线性扫描窗口大小（着陆点两侧各扫 window_size/2）
    pub window_size: usize,
}

impl PpiIndex {
    /// 构建索引
    ///
    /// - `vectors`: N × dim 连续 f32
    /// - `ids`: N 个 ID
    /// - `dim`: 维度
    /// - `anchors`: K × dim 的锚点数组（调用方用高斯随机生成）
    pub fn build(
        vectors: &[f32],
        ids: &[u64],
        dim: usize,
        anchors: &[f32],
    ) -> Self {
        let n = ids.len();
        assert_eq!(vectors.len(), n * dim);
        let num_anchors = anchors.len() / dim;
        assert!(num_anchors <= 16, "当前实现限制 K ≤ 16（u64 编码）");
        assert_eq!(anchors.len(), num_anchors * dim);

        // 计算每个向量的排列签名
        let mut entries: Vec<(u64, usize)> = (0..n)
            .map(|i| {
                let v = &vectors[i * dim..(i + 1) * dim];
                let sig = encode_permutation(v, anchors, dim, num_anchors);
                (sig, i)
            })
            .collect();

        // 按签名整数排序
        entries.sort_unstable_by_key(|e| e.0);

        // 构建连续存储
        let mut sorted_vecs = Vec::with_capacity(n * dim);
        let mut sorted_ids = Vec::with_capacity(n);
        let mut sorted_sigs = Vec::with_capacity(n);

        for &(sig, orig_idx) in &entries {
            sorted_vecs.extend_from_slice(&vectors[orig_idx * dim..(orig_idx + 1) * dim]);
            sorted_ids.push(ids[orig_idx]);
            sorted_sigs.push(sig);
        }

        PpiIndex {
            dim,
            n,
            num_anchors,
            vectors: sorted_vecs,
            ids: sorted_ids,
            signatures: sorted_sigs,
            anchors: anchors.to_vec(),
        }
    }

    /// 查询
    pub fn search(&self, query: &[f32], config: &PpiQuery) -> Vec<(u64, f32)> {
        assert_eq!(query.len(), self.dim);

        // 计算查询的排列签名
        let q_sig = encode_permutation(query, &self.anchors, self.dim, self.num_anchors);

        // 二分定位
        let landing = self.signatures.partition_point(|&s| s < q_sig);

        // 线性扫描窗口
        let half = config.window_size / 2;
        let lo = landing.saturating_sub(half);
        let hi = (landing + half).min(self.n);

        let mut candidates: Vec<(u64, f32)> = Vec::with_capacity(hi - lo);
        for i in lo..hi {
            let v = &self.vectors[i * self.dim..(i + 1) * self.dim];
            let score = cosine_sim(query, v);
            candidates.push((self.ids[i], score));
        }

        candidates.sort_unstable_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(config.top_k);
        candidates
    }

    /// 索引额外开销（不含向量数据本身）
    pub fn overhead_bytes(&self) -> usize {
        self.signatures.len() * 8 + self.anchors.len() * 4
    }

    /// 总内存
    pub fn memory_bytes(&self) -> usize {
        self.vectors.len() * 4 + self.ids.len() * 8 + self.overhead_bytes()
    }

    pub fn node_count(&self) -> usize {
        self.n
    }
}

// ═══════ 核心编码函数 ═══════

/// 计算向量到 K 个锚点的距离排序 → 编码为 u64
///
/// 编码格式：最高 4bit = 最近锚点 index，次高 4bit = 次近，...
/// K ≤ 16 时恰好填满 u64
fn encode_permutation(v: &[f32], anchors: &[f32], dim: usize, k: usize) -> u64 {
    // 计算到每个锚点的欧氏距离平方（省去 sqrt，不影响排序）
    let mut dists: Vec<(u32, usize)> = (0..k)
        .map(|j| {
            let anchor = &anchors[j * dim..(j + 1) * dim];
            let d2: f32 = v.iter().zip(anchor).map(|(a, b)| (a - b) * (a - b)).sum();
            // 用 u32 bit 表示 f32 以获得确定性排序（正数域 f32 的 bit 是单调的）
            (d2.to_bits(), j)
        })
        .collect();

    dists.sort_unstable_by_key(|x| x.0);

    // 编码排列：从最高位开始，每个锚点 index 占 4 bit
    let mut sig: u64 = 0;
    for (rank, &(_, anchor_idx)) in dists.iter().enumerate() {
        let shift = (15 - rank) * 4; // 最近锚点放最高位
        sig |= (anchor_idx as u64) << shift;
    }
    sig
}

// ═══════ 数学辅助 ═══════

#[inline]
fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let ab: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    let denom = na * nb;
    if denom < 1e-30 { 0.0 } else { ab / denom }
}
