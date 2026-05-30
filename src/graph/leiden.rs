use crate::node::NodeId;
use std::collections::{HashMap, HashSet};

// ============================================================================
// Leiden 社区发现 (标签传播近似)
//
// 设计原则:
// 1. 不持有 MemTable 锁 — 调用方先 snapshot 邻接表再传入
// 2. 质心由调用方在外部计算 — 聚类函数只做图划分
// 3. 结果写回 Payload — 利用 TriviumDB 现有的 fast_tags 布隆过滤加速
// ============================================================================

/// 聚类配置 (全部可选，有合理默认值)
#[derive(Debug, Clone)]
pub struct LeidenConfig {
    /// 最小社区大小 (节点数 < 此值的碎片簇被丢弃)
    pub min_community_size: usize,
    /// 最大迭代轮次 (标签传播收敛上限)
    pub max_iterations: usize,
    /// 是否计算质心 (需要提供向量数据)
    pub compute_centroids: bool,
}

impl Default for LeidenConfig {
    fn default() -> Self {
        Self {
            min_community_size: 3,
            max_iterations: 15,
            compute_centroids: true,
        }
    }
}

/// 聚类结果
#[derive(Debug, Clone)]
pub struct LeidenResult {
    /// 节点 → 簇 ID 映射 (仅包含被分配到有效簇的节点)
    pub node_to_cluster: HashMap<NodeId, u32>,
    /// 簇 ID → 簇内节点数
    pub cluster_sizes: HashMap<u32, usize>,
    /// 簇 ID → 质心向量 (仅当 compute_centroids=true 且提供了向量数据时)
    pub centroids: HashMap<u32, Vec<f32>>,
    /// 发现的社区总数
    pub num_clusters: u32,
}

/// 邻接表快照 (无锁, 从 MemTable 浅拷贝)
pub struct AdjacencySnapshot {
    /// 节点 → [(目标节点, 边权重)]
    pub edges: HashMap<NodeId, Vec<(NodeId, f32)>>,
    /// 所有活跃节点 ID
    pub node_ids: Vec<NodeId>,
}

/// 执行 Leiden/Louvain 标签传播聚类 (无锁, 纯计算)
///
/// 调用方负责:
/// 1. 获取 MemTable 锁 → snapshot 邻接表 → 释放锁
/// 2. 调用本函数 (不持有任何锁)
/// 3. 将结果写回 Payload (可选)
pub fn run_leiden(adj: &AdjacencySnapshot, config: &LeidenConfig) -> LeidenResult {
    let nodes = &adj.node_ids;

    if nodes.is_empty() {
        return LeidenResult {
            node_to_cluster: HashMap::new(),
            cluster_sizes: HashMap::new(),
            centroids: HashMap::new(),
            num_clusters: 0,
        };
    }

    // 初始化: 每个节点自成一簇 (用 NodeId 作为初始簇 ID)
    let mut node_to_cluster: HashMap<NodeId, u32> = HashMap::with_capacity(nodes.len());
    for (i, &n) in nodes.iter().enumerate() {
        node_to_cluster.insert(n, i as u32);
    }

    // 标签传播主循环
    let mut changed = true;
    let mut iters_left = config.max_iterations;

    while changed && iters_left > 0 {
        changed = false;
        iters_left -= 1;

        for &n in nodes {
            let current_c = match node_to_cluster.get(&n) {
                Some(&c) => c,
                None => continue,
            };

            // 统计邻居簇的加权投票
            if let Some(neighbors) = adj.edges.get(&n) {
                let mut votes: HashMap<u32, f32> = HashMap::new();
                for &(target, weight) in neighbors {
                    if let Some(&nc) = node_to_cluster.get(&target) {
                        *votes.entry(nc).or_insert(0.0) += weight;
                    }
                }

                // 选最高票簇
                if let Some((&best_c, _)) = votes
                    .iter()
                    .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                    && best_c != current_c
                {
                    node_to_cluster.insert(n, best_c);
                    changed = true;
                }
            }
        }
    }

    // 统计每簇大小
    let mut cluster_counts: HashMap<u32, usize> = HashMap::new();
    for &c in node_to_cluster.values() {
        *cluster_counts.entry(c).or_insert(0) += 1;
    }

    // 过滤碎片簇 + 连续编号 (1, 2, 3...)
    let valid_clusters: HashSet<u32> = cluster_counts
        .iter()
        .filter(|(_, count)| **count >= config.min_community_size)
        .map(|(&c, _)| c)
        .collect();

    let mut remap: HashMap<u32, u32> = HashMap::new();
    // 排序确保确定性映射
    let mut sorted_valid: Vec<u32> = valid_clusters.into_iter().collect();
    sorted_valid.sort_unstable();
    for (new_id, c) in (1u32..).zip(sorted_valid) {
        remap.insert(c, new_id);
    }

    let mut final_map: HashMap<NodeId, u32> = HashMap::new();
    for (&n, &c) in &node_to_cluster {
        if let Some(&nc) = remap.get(&c) {
            final_map.insert(n, nc);
        }
    }

    // 重建簇大小统计
    let mut cluster_sizes: HashMap<u32, usize> = HashMap::new();
    for &c in final_map.values() {
        *cluster_sizes.entry(c).or_insert(0) += 1;
    }

    let num_clusters = cluster_sizes.len() as u32;

    LeidenResult {
        node_to_cluster: final_map,
        cluster_sizes,
        centroids: HashMap::new(), // 质心由调用方负责计算
        num_clusters,
    }
}

/// 使用向量数据为聚类结果补充质心 (无锁, 纯计算)
///
/// vectors: NodeId → 向量 (f32 切片)
pub fn compute_centroids(
    result: &mut LeidenResult,
    vectors: &HashMap<NodeId, Vec<f32>>,
    dim: usize,
) {
    // 按簇聚合
    let mut cluster_sums: HashMap<u32, Vec<f64>> = HashMap::new();
    let mut cluster_counts: HashMap<u32, usize> = HashMap::new();

    for (&node_id, &cluster_id) in &result.node_to_cluster {
        if let Some(vec) = vectors.get(&node_id) {
            let sum = cluster_sums
                .entry(cluster_id)
                .or_insert_with(|| vec![0.0f64; dim]);
            for i in 0..dim.min(vec.len()) {
                sum[i] += vec[i] as f64;
            }
            *cluster_counts.entry(cluster_id).or_insert(0) += 1;
        }
    }

    // 平均化
    for (&c, sum) in &cluster_sums {
        let count = cluster_counts.get(&c).copied().unwrap_or(1) as f64;
        let centroid: Vec<f32> = sum.iter().map(|&s| (s / count) as f32).collect();
        result.centroids.insert(c, centroid);
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snapshot(edges: Vec<(NodeId, NodeId, f32)>) -> AdjacencySnapshot {
        let mut adj: HashMap<NodeId, Vec<(NodeId, f32)>> = HashMap::new();
        let mut all_ids: HashSet<NodeId> = HashSet::new();
        for (src, dst, w) in edges {
            adj.entry(src).or_default().push((dst, w));
            adj.entry(dst).or_default().push((src, w)); // 无向
            all_ids.insert(src);
            all_ids.insert(dst);
        }
        AdjacencySnapshot {
            edges: adj,
            node_ids: all_ids.into_iter().collect(),
        }
    }

    #[test]
    fn test_empty_graph() {
        let snap = AdjacencySnapshot {
            edges: HashMap::new(),
            node_ids: vec![],
        };
        let result = run_leiden(&snap, &LeidenConfig::default());
        assert_eq!(result.num_clusters, 0);
    }

    #[test]
    fn test_two_cliques() {
        // 两个完全子图: {1,2,3} 和 {4,5,6}
        let snap = make_snapshot(vec![
            (1, 2, 1.0),
            (1, 3, 1.0),
            (2, 3, 1.0), // 团 A
            (4, 5, 1.0),
            (4, 6, 1.0),
            (5, 6, 1.0), // 团 B
        ]);
        let result = run_leiden(
            &snap,
            &LeidenConfig {
                min_community_size: 3,
                ..Default::default()
            },
        );
        assert_eq!(result.num_clusters, 2, "应发现 2 个社区");
        // 同团节点应属于同一簇
        assert_eq!(result.node_to_cluster[&1], result.node_to_cluster[&2]);
        assert_eq!(result.node_to_cluster[&4], result.node_to_cluster[&5]);
        // 不同团应属于不同簇
        assert_ne!(result.node_to_cluster[&1], result.node_to_cluster[&4]);
    }

    #[test]
    fn test_fragment_filtering() {
        // {1,2,3} 是团, {4,5} 是碎片 (< min_community_size=3)
        let snap = make_snapshot(vec![(1, 2, 1.0), (1, 3, 1.0), (2, 3, 1.0), (4, 5, 1.0)]);
        let result = run_leiden(
            &snap,
            &LeidenConfig {
                min_community_size: 3,
                ..Default::default()
            },
        );
        assert_eq!(result.num_clusters, 1, "碎片簇应被过滤");
        assert!(result.node_to_cluster.contains_key(&1));
        assert!(!result.node_to_cluster.contains_key(&4), "碎片节点不应出现");
    }

    #[test]
    fn test_centroid_computation() {
        let snap = make_snapshot(vec![(1, 2, 1.0), (1, 3, 1.0), (2, 3, 1.0)]);
        let mut result = run_leiden(
            &snap,
            &LeidenConfig {
                min_community_size: 3,
                ..Default::default()
            },
        );

        let mut vectors = HashMap::new();
        vectors.insert(1u64, vec![1.0f32, 0.0, 0.0]);
        vectors.insert(2, vec![0.0, 1.0, 0.0]);
        vectors.insert(3, vec![0.0, 0.0, 1.0]);

        compute_centroids(&mut result, &vectors, 3);
        assert_eq!(result.centroids.len(), 1);
        let c = result.centroids.values().next().unwrap();
        // 质心应约为 (1/3, 1/3, 1/3)
        assert!((c[0] - 1.0 / 3.0).abs() < 0.01);
    }
}
