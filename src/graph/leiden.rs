//! 标准、确定性的加权 Leiden 社区发现实现。
//!
//! 输入有向业务边会转换为规范化无向权重，随后执行 local moving、refinement 和
//! aggregation，直到收敛或达到轮次上限。所有节点/社区迭代和 tie-break 都使用稳定
//! 顺序，保证重复运行及并行环境下结果一致；最小社区过滤和可选质心只影响输出整理。

use crate::node::NodeId;
#[cfg(test)]
use std::collections::HashSet;
use std::collections::{BTreeMap, BTreeSet, HashMap};

// ============================================================================
// 标准确定性 Leiden：加权无向 modularity、refinement 与 aggregation
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

/// 执行确定性标准 Leiden 社区发现。
pub fn run_leiden(adj: &AdjacencySnapshot, config: &LeidenConfig) -> LeidenResult {
    let mut nodes = adj.node_ids.clone();
    nodes.sort_unstable();
    nodes.dedup();
    if nodes.is_empty() {
        return empty_result();
    }

    let mut graph = WeightedGraph::from_snapshot(adj, &nodes);
    let mut members = nodes.iter().map(|&node| vec![node]).collect::<Vec<_>>();
    let mut total_iterations = 0usize;
    for _level in 0..config.max_iterations.max(1) {
        let (mut communities, iterations) = local_moving(&graph, config.max_iterations.max(1));
        total_iterations = total_iterations.saturating_add(iterations);
        communities = refine_connected(&graph, &communities);
        let community_count = communities.iter().copied().collect::<BTreeSet<_>>().len();
        if community_count == graph.len() {
            break;
        }
        let (next_graph, remap) = aggregate(&graph, &communities);
        let mut next_members = vec![Vec::new(); next_graph.len()];
        for (node, &community) in remap.iter().enumerate() {
            next_members[community].append(&mut members[node]);
        }
        graph = next_graph;
        members = next_members;
        if community_count <= 1 {
            break;
        }
    }

    let mut final_groups = members
        .into_iter()
        .filter(|community| community.len() >= config.min_community_size)
        .collect::<Vec<_>>();
    for community in &mut final_groups {
        community.sort_unstable();
    }
    final_groups.sort_by_key(|community| community[0]);
    let mut node_to_cluster = HashMap::new();
    let mut cluster_sizes = HashMap::new();
    for (index, community) in final_groups.iter().enumerate() {
        let cluster = index as u32 + 1;
        cluster_sizes.insert(cluster, community.len());
        for &node in community {
            node_to_cluster.insert(node, cluster);
        }
    }
    let _ = total_iterations;
    LeidenResult {
        num_clusters: cluster_sizes.len() as u32,
        node_to_cluster,
        cluster_sizes,
        centroids: HashMap::new(),
    }
}

fn empty_result() -> LeidenResult {
    LeidenResult {
        node_to_cluster: HashMap::new(),
        cluster_sizes: HashMap::new(),
        centroids: HashMap::new(),
        num_clusters: 0,
    }
}

#[derive(Debug, Clone)]
struct WeightedGraph {
    adjacency: Vec<Vec<(usize, f64)>>,
    degree: Vec<f64>,
    total_weight_twice: f64,
}

impl WeightedGraph {
    fn len(&self) -> usize {
        self.adjacency.len()
    }

    fn from_snapshot(snapshot: &AdjacencySnapshot, nodes: &[NodeId]) -> Self {
        let positions = nodes
            .iter()
            .enumerate()
            .map(|(index, &node)| (node, index))
            .collect::<HashMap<_, _>>();
        let mut undirected = BTreeMap::<(usize, usize), f64>::new();
        for (&source, edges) in &snapshot.edges {
            let Some(&left) = positions.get(&source) else {
                continue;
            };
            for &(target, weight) in edges {
                let Some(&right) = positions.get(&target) else {
                    continue;
                };
                if !weight.is_finite() || weight <= 0.0 {
                    continue;
                }
                let pair = if left <= right {
                    (left, right)
                } else {
                    (right, left)
                };
                undirected
                    .entry(pair)
                    .and_modify(|current| *current = current.max(weight as f64))
                    .or_insert(weight as f64);
            }
        }
        let mut adjacency = vec![Vec::new(); nodes.len()];
        for ((left, right), weight) in undirected {
            adjacency[left].push((right, weight));
            if left != right {
                adjacency[right].push((left, weight));
            } else {
                adjacency[left].push((right, weight));
            }
        }
        for neighbors in &mut adjacency {
            neighbors.sort_by_key(|(target, _)| *target);
        }
        Self::new(adjacency)
    }

    fn new(adjacency: Vec<Vec<(usize, f64)>>) -> Self {
        let degree = adjacency
            .iter()
            .map(|neighbors| neighbors.iter().map(|(_, weight)| *weight).sum::<f64>())
            .collect::<Vec<_>>();
        let total_weight_twice = degree.iter().sum();
        Self {
            adjacency,
            degree,
            total_weight_twice,
        }
    }
}

fn local_moving(graph: &WeightedGraph, max_iterations: usize) -> (Vec<usize>, usize) {
    let mut communities = (0..graph.len()).collect::<Vec<_>>();
    let mut totals = graph.degree.clone();
    if graph.total_weight_twice <= f64::EPSILON {
        return (communities, 0);
    }
    for iteration in 0..max_iterations {
        let mut changed = false;
        for node in 0..graph.len() {
            let current = communities[node];
            let degree = graph.degree[node];
            let mut weights = BTreeMap::<usize, f64>::new();
            for &(neighbor, weight) in &graph.adjacency[node] {
                *weights.entry(communities[neighbor]).or_default() += weight;
            }
            totals[current] -= degree;
            let current_gain = weights.get(&current).copied().unwrap_or_default()
                - degree * totals[current] / graph.total_weight_twice;
            let mut best = current;
            let mut best_gain = current_gain;
            for (community, internal_weight) in weights {
                let gain = internal_weight - degree * totals[community] / graph.total_weight_twice;
                if gain > best_gain + 1e-12 || (gain - best_gain).abs() <= 1e-12 && community < best
                {
                    best = community;
                    best_gain = gain;
                }
            }
            communities[node] = best;
            totals[best] += degree;
            changed |= best != current;
        }
        if !changed {
            return (communities, iteration + 1);
        }
    }
    (communities, max_iterations)
}

fn refine_connected(graph: &WeightedGraph, communities: &[usize]) -> Vec<usize> {
    let mut refined = vec![usize::MAX; graph.len()];
    let mut next = 0usize;
    for start in 0..graph.len() {
        if refined[start] != usize::MAX {
            continue;
        }
        let target_community = communities[start];
        refined[start] = next;
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            for &(neighbor, weight) in &graph.adjacency[node] {
                if weight > 0.0
                    && communities[neighbor] == target_community
                    && refined[neighbor] == usize::MAX
                {
                    refined[neighbor] = next;
                    stack.push(neighbor);
                }
            }
        }
        next += 1;
    }
    refined
}

fn aggregate(graph: &WeightedGraph, communities: &[usize]) -> (WeightedGraph, Vec<usize>) {
    let labels = communities.iter().copied().collect::<BTreeSet<_>>();
    let dense = labels
        .into_iter()
        .enumerate()
        .map(|(index, label)| (label, index))
        .collect::<HashMap<_, _>>();
    let remap = communities
        .iter()
        .map(|community| dense[community])
        .collect::<Vec<_>>();
    let mut edges = BTreeMap::<(usize, usize), f64>::new();
    for source in 0..graph.len() {
        for &(target, weight) in &graph.adjacency[source] {
            if source > target {
                continue;
            }
            let left = remap[source];
            let right = remap[target];
            let pair = if left <= right {
                (left, right)
            } else {
                (right, left)
            };
            *edges.entry(pair).or_default() += weight;
        }
    }
    let mut adjacency = vec![Vec::new(); dense.len()];
    for ((left, right), weight) in edges {
        adjacency[left].push((right, weight));
        if left != right {
            adjacency[right].push((left, weight));
        } else {
            adjacency[left].push((right, weight));
        }
    }
    for neighbors in &mut adjacency {
        neighbors.sort_by_key(|(target, _)| *target);
    }
    (WeightedGraph::new(adjacency), remap)
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
    fn 标准_leiden_弱桥双团稳定分离且结果确定() {
        let mut edges = vec![
            (1, 2, 1.0),
            (1, 3, 1.0),
            (2, 3, 1.0),
            (4, 5, 1.0),
            (4, 6, 1.0),
            (5, 6, 1.0),
            (3, 4, 0.01),
        ];
        let first = run_leiden(
            &make_snapshot(edges.clone()),
            &LeidenConfig {
                min_community_size: 1,
                ..Default::default()
            },
        );
        edges.reverse();
        let second = run_leiden(
            &make_snapshot(edges),
            &LeidenConfig {
                min_community_size: 1,
                ..Default::default()
            },
        );
        assert_eq!(first.node_to_cluster, second.node_to_cluster);
        assert_eq!(first.node_to_cluster[&1], first.node_to_cluster[&3]);
        assert_eq!(first.node_to_cluster[&4], first.node_to_cluster[&6]);
        assert_ne!(first.node_to_cluster[&1], first.node_to_cluster[&4]);
    }

    #[test]
    fn refinement_保证每个输出社区内部连通() {
        let snap = make_snapshot(vec![(1, 2, 1.0), (2, 3, 1.0), (4, 5, 1.0), (5, 6, 1.0)]);
        let result = run_leiden(
            &snap,
            &LeidenConfig {
                min_community_size: 1,
                ..Default::default()
            },
        );
        for cluster in 1..=result.num_clusters {
            let members = result
                .node_to_cluster
                .iter()
                .filter_map(|(&node, &value)| (value == cluster).then_some(node))
                .collect::<HashSet<_>>();
            let start = *members.iter().next().unwrap();
            let mut reached = HashSet::from([start]);
            let mut stack = vec![start];
            while let Some(node) = stack.pop() {
                for &(neighbor, _) in snap.edges.get(&node).into_iter().flatten() {
                    if members.contains(&neighbor) && reached.insert(neighbor) {
                        stack.push(neighbor);
                    }
                }
            }
            assert_eq!(reached, members);
        }
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
