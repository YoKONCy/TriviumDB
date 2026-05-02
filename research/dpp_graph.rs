//! DPP Graph Index — 基于行列式点过程多样性选边的单层 ANN 图
//!
//! # 核心思想
//!
//! 传统图 ANN（HNSW/NSG）的邻居选择策略是"选最近的 k 个"，
//! 导致邻居之间高度相似（聚团），需要多层随机跳链来修补全局可达性。
//!
//! DPP Graph 用 DPP 贪心采样替代最近邻选边：
//! - 每个节点从候选池中选 k 个邻居
//! - 选择标准：**质量（与目标相关）× 多样性（邻居之间互不相似）**
//! - 数学上等价于最大化 det(L_S)，L 为质量-多样性核矩阵
//!
//! 结果：单层图天然具有扩展图性质（无需层次结构），贪心路由即可全局可达。

/// DPP 图索引
pub struct DppGraphIndex {
    dim: usize,
    n: usize,
    /// 向量数据（连续存储）
    vectors: Vec<f32>,
    /// 原始 ID
    ids: Vec<u64>,
    /// 邻接表：adj[i] = 节点 i 的邻居索引列表
    adj: Vec<Vec<u32>>,
    /// 入口点集合（多个，分散在空间中）
    entry_points: Vec<u32>,
}

/// 构建配置
pub struct DppBuildConfig {
    /// 每个节点的邻居数（图的度数）
    pub degree: usize,
    /// 候选池大小（从中 DPP 采样 degree 个）
    pub candidate_pool: usize,
    /// 入口点数量
    pub num_entry_points: usize,
    /// 随机采样大小（0 = 全量扫描，>0 = 随机采样加速构建）
    /// 建议值：500-2000。使构建从 O(N²d) 降为 O(N × sample × d)
    pub sample_size: usize,
}

impl Default for DppBuildConfig {
    fn default() -> Self {
        Self {
            degree: 16,
            candidate_pool: 64,
            num_entry_points: 8,
            sample_size: 0,
        }
    }
}

/// 查询配置
pub struct DppSearchConfig {
    pub top_k: usize,
    /// beam 宽度（搜索时维护的候选集大小）
    pub ef_search: usize,
}

impl DppGraphIndex {
    /// 构建 DPP 图索引
    pub fn build(
        vectors: &[f32],
        ids: &[u64],
        dim: usize,
        config: &DppBuildConfig,
    ) -> Self {
        let n = ids.len();
        assert_eq!(vectors.len(), n * dim);
        let degree = config.degree;
        let pool_size = config.candidate_pool;

        // 1. 为每个节点选择邻居
        let mut adj: Vec<Vec<u32>> = vec![Vec::new(); n];

        // 用于随机采样的简易 LCG 随机数生成器（无需外部依赖）
        let mut lcg_state: u64 = 42;
        let lcg_next = |state: &mut u64| -> usize {
            *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (*state >> 33) as usize
        };

        let use_sampling = config.sample_size > 0 && config.sample_size < n;

        for i in 0..n {
            let vi = &vectors[i * dim..(i + 1) * dim];

            // 找候选：全量扫描或随机采样
            let mut dists: Vec<(f32, u32)> = if use_sampling {
                // 随机采样 sample_size 个节点
                let mut sampled = Vec::with_capacity(config.sample_size);
                for _ in 0..config.sample_size {
                    let j = lcg_next(&mut lcg_state) % n;
                    if j != i {
                        let vj = &vectors[j * dim..(j + 1) * dim];
                        sampled.push((cosine_sim(vi, vj), j as u32));
                    }
                }
                sampled
            } else {
                (0..n)
                    .filter(|&j| j != i)
                    .map(|j| {
                        let vj = &vectors[j * dim..(j + 1) * dim];
                        (cosine_sim(vi, vj), j as u32)
                    })
                    .collect()
            };
            dists.sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            dists.truncate(pool_size);

            // 从候选池中 DPP 贪心选择 degree 个多样性邻居
            let selected = dpp_greedy_select(
                &dists,
                vectors,
                dim,
                degree,
            );

            adj[i] = selected;
        }

        // 2. 双向化：如果 i→j，也加 j→i
        for i in 0..n {
            let neighbors = adj[i].clone();
            for &j in &neighbors {
                let ju = j as usize;
                if !adj[ju].contains(&(i as u32)) {
                    adj[ju].push(i as u32);
                }
            }
        }

        // 3. 选择入口点：取空间中分散的点
        let entry_points = select_entry_points(vectors, dim, n, config.num_entry_points);

        DppGraphIndex {
            dim,
            n,
            vectors: vectors.to_vec(),
            ids: ids.to_vec(),
            adj,
            entry_points,
        }
    }

    /// Beam Search 查询
    pub fn search(&self, query: &[f32], config: &DppSearchConfig) -> Vec<(u64, f32)> {
        assert_eq!(query.len(), self.dim);

        let ef = config.ef_search;
        let mut visited = vec![false; self.n];
        // (负相似度用于 min-heap, 节点索引)
        let mut candidates = std::collections::BinaryHeap::<std::cmp::Reverse<(OrdF32, u32)>>::new();
        let mut results = std::collections::BinaryHeap::<(OrdF32, u32)>::new();

        // 从所有入口点开始
        for &ep in &self.entry_points {
            let epu = ep as usize;
            if !visited[epu] {
                visited[epu] = true;
                let sim = cosine_sim(query, &self.vectors[epu * self.dim..(epu + 1) * self.dim]);
                candidates.push(std::cmp::Reverse((OrdF32(-sim), ep)));
                results.push((OrdF32(-sim), ep)); // 用负值使 max-heap 变 min-heap
            }
        }

        // Beam search
        while let Some(std::cmp::Reverse((OrdF32(neg_sim), node))) = candidates.pop() {
            let current_sim = -neg_sim;

            // 如果当前候选比结果集中最差的还差，且结果集已满，停止
            if results.len() >= ef {
                let worst = -results.peek().unwrap().0 .0;
                if current_sim < worst {
                    break;
                }
            }

            // 展开邻居
            for &neighbor in &self.adj[node as usize] {
                let nu = neighbor as usize;
                if visited[nu] {
                    continue;
                }
                visited[nu] = true;

                let sim = cosine_sim(
                    query,
                    &self.vectors[nu * self.dim..(nu + 1) * self.dim],
                );

                candidates.push(std::cmp::Reverse((OrdF32(-sim), neighbor)));

                if results.len() < ef {
                    results.push((OrdF32(-sim), neighbor));
                } else if let Some(&(OrdF32(worst_neg), _)) = results.peek() {
                    if -sim < worst_neg {
                        // sim > worst_sim，替换
                        results.pop();
                        results.push((OrdF32(-sim), neighbor));
                    }
                }
            }
        }

        // 提取结果
        let mut out: Vec<(u64, f32)> = results
            .into_iter()
            .map(|(OrdF32(neg_sim), idx)| (self.ids[idx as usize], -neg_sim))
            .collect();
        out.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        out.truncate(config.top_k);
        out
    }

    pub fn node_count(&self) -> usize { self.n }
    pub fn avg_degree(&self) -> f64 {
        self.adj.iter().map(|a| a.len()).sum::<usize>() as f64 / self.n as f64
    }
    pub fn memory_bytes(&self) -> usize {
        self.vectors.len() * 4
            + self.ids.len() * 8
            + self.adj.iter().map(|a| a.len() * 4 + 24).sum::<usize>()
    }
}

// ═══════ DPP 贪心选择 ═══════

/// 从候选池中贪心选择 k 个多样性邻居
///
/// 贪心策略：每一步选择使 quality × min_diversity 最大的候选
/// - quality = 与目标节点的余弦相似度
/// - diversity = 与已选邻居的最小距离（1 - max_sim）
fn dpp_greedy_select(
    candidates: &[(f32, u32)], // (similarity_to_target, node_index)
    vectors: &[f32],
    dim: usize,
    k: usize,
) -> Vec<u32> {
    if candidates.len() <= k {
        return candidates.iter().map(|c| c.1).collect();
    }

    let mut selected: Vec<u32> = Vec::with_capacity(k);
    let mut used = vec![false; candidates.len()];

    // 第一个：选质量最高的
    selected.push(candidates[0].1);
    used[0] = true;

    // 后续：贪心选 quality × diversity 最大的
    for _ in 1..k {
        let mut best_score = f32::NEG_INFINITY;
        let mut best_idx = 0;

        for (ci, &(quality, cand_node)) in candidates.iter().enumerate() {
            if used[ci] {
                continue;
            }

            let vc = &vectors[cand_node as usize * dim..(cand_node as usize + 1) * dim];

            // diversity = 与已选节点的最小距离
            let max_sim_to_selected = selected
                .iter()
                .map(|&s| {
                    let vs = &vectors[s as usize * dim..(s as usize + 1) * dim];
                    cosine_sim(vc, vs)
                })
                .fold(f32::NEG_INFINITY, f32::max);

            let diversity = 1.0 - max_sim_to_selected;
            let score = quality * diversity;

            if score > best_score {
                best_score = score;
                best_idx = ci;
            }
        }

        used[best_idx] = true;
        selected.push(candidates[best_idx].1);
    }

    selected
}

/// 选择分散的入口点
fn select_entry_points(vectors: &[f32], dim: usize, n: usize, count: usize) -> Vec<u32> {
    if n <= count {
        return (0..n as u32).collect();
    }

    let mut entries: Vec<u32> = Vec::with_capacity(count);

    // 第一个：选离原点最远的（通常在空间边缘）
    let mut best_norm = 0.0f32;
    let mut best_idx = 0u32;
    for i in 0..n {
        let v = &vectors[i * dim..(i + 1) * dim];
        let norm: f32 = v.iter().map(|x| x * x).sum();
        if norm > best_norm {
            best_norm = norm;
            best_idx = i as u32;
        }
    }
    entries.push(best_idx);

    // 后续：贪心选离已选点最远的
    for _ in 1..count {
        let mut max_min_dist = f32::NEG_INFINITY;
        let mut best = 0u32;
        for i in 0..n {
            let vi = &vectors[i * dim..(i + 1) * dim];
            let min_sim = entries
                .iter()
                .map(|&e| cosine_sim(vi, &vectors[e as usize * dim..(e as usize + 1) * dim]))
                .fold(f32::INFINITY, f32::min);
            let dist = 1.0 - min_sim;
            if dist > max_min_dist {
                max_min_dist = dist;
                best = i as u32;
            }
        }
        entries.push(best);
    }

    entries
}

// ═══════ 辅助类型 ═══════

/// 可排序的 f32 包装
#[derive(Clone, Copy)]
struct OrdF32(f32);

impl PartialEq for OrdF32 {
    fn eq(&self, other: &Self) -> bool { self.0 == other.0 }
}
impl Eq for OrdF32 {}
impl PartialOrd for OrdF32 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(other)) }
}
impl Ord for OrdF32 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.partial_cmp(&other.0).unwrap_or(std::cmp::Ordering::Equal)
    }
}

#[inline]
fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let ab: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    let d = na * nb;
    if d < 1e-30 { 0.0 } else { ab / d }
}
