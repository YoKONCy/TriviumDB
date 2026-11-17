//! 中心性分析算法
//!
//! 提供度中心性和 Betweenness 中心性（Brandes 算法）。
//!
//! | 算法 | 复杂度 | 用途 |
//! |------|--------|------|
//! | 度中心性 | O(V) | 找核心枢纽实体 |
//! | Betweenness | O(VE) | 发现信息瓶颈节点 |

use crate::VectorType;
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use std::collections::{HashMap, HashSet, VecDeque};

// ═══════════════════════════════════════════════════════════════════════
//  度中心性 (Degree Centrality)
// ═══════════════════════════════════════════════════════════════════════

/// 单个节点的度中心性指标
#[derive(Debug, Clone)]
pub struct DegreeCentrality {
    pub id: NodeId,
    pub out_degree: usize,
    pub in_degree: usize,
    pub total_degree: usize,
    /// 归一化度中心性 = total_degree / (N - 1)
    pub normalized: f64,
}

/// 计算所有节点的度中心性
///
/// - 返回值按 total_degree 降序排列
/// - 用途：快速识别图谱中的核心枢纽实体
pub fn degree_centrality<T: VectorType>(mt: &MemTable<T>) -> Vec<DegreeCentrality> {
    let all_ids = mt.all_node_ids();
    let n = all_ids.len();
    if n == 0 {
        return Vec::new();
    }
    let denom = if n > 1 { (n - 1) as f64 } else { 1.0 };

    let mut results: Vec<DegreeCentrality> = all_ids
        .iter()
        .map(|&id| {
            let out_degree = mt.get_edges(id).map(|e| e.len()).unwrap_or(0);
            let in_degree = mt.get_in_degree(id);
            let total_degree = out_degree + in_degree;
            DegreeCentrality {
                id,
                out_degree,
                in_degree,
                total_degree,
                normalized: total_degree as f64 / denom,
            }
        })
        .collect();

    results.sort_by(|a, b| b.total_degree.cmp(&a.total_degree));
    results
}

// ═══════════════════════════════════════════════════════════════════════
//  Betweenness 中心性 (Brandes 算法)
// ═══════════════════════════════════════════════════════════════════════

/// 计算所有节点的 Betweenness 中心性（Brandes 算法）
///
/// - 如果 `label_filter` 为 Some，只沿匹配标签的边计算
/// - 如果 `sample_size` 为 Some(k)，只从 k 个源节点出发（近似计算，降低大图开销）
/// - 返回 `HashMap<NodeId, f64>`，值为归一化后的中心性分数
/// - 用途：发现信息瓶颈节点——移除该节点后最大程度破坏图连通性
pub fn betweenness_centrality<T: VectorType>(
    mt: &MemTable<T>,
    label_filter: Option<&str>,
    sample_size: Option<usize>,
) -> HashMap<NodeId, f64> {
    let all_ids = mt.all_node_ids();
    let n = all_ids.len();
    if n < 2 {
        return all_ids.into_iter().map(|id| (id, 0.0)).collect();
    }

    let mut centrality: HashMap<NodeId, f64> = all_ids.iter().map(|&id| (id, 0.0)).collect();

    // 确定源节点集合
    let sources: Vec<NodeId> = if let Some(k) = sample_size {
        let mut sorted = all_ids.clone();
        sorted.sort_unstable();
        sorted.into_iter().take(k).collect()
    } else {
        all_ids.clone()
    };

    let id_set: HashSet<NodeId> = all_ids.iter().copied().collect();

    for &s in &sources {
        // Brandes 单源 BFS
        let mut stack: Vec<NodeId> = Vec::new();
        let mut predecessors: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut sigma: HashMap<NodeId, f64> = HashMap::new();
        let mut dist: HashMap<NodeId, i64> = HashMap::new();

        for &id in &all_ids {
            sigma.insert(id, 0.0);
            dist.insert(id, -1);
        }
        sigma.insert(s, 1.0);
        dist.insert(s, 0);

        let mut queue: VecDeque<NodeId> = VecDeque::new();
        queue.push_back(s);

        while let Some(v) = queue.pop_front() {
            stack.push(v);
            let d_v = dist[&v];

            if let Some(edges) = mt.get_edges(v) {
                for edge in edges {
                    if let Some(lf) = label_filter
                        && edge.label != lf
                    {
                        continue;
                    }
                    let w = edge.target_id;
                    if !id_set.contains(&w) {
                        continue;
                    }

                    if dist[&w] < 0 {
                        dist.insert(w, d_v + 1);
                        queue.push_back(w);
                    }
                    if dist[&w] == d_v + 1 {
                        let sigma_v = sigma[&v];
                        if let Some(s) = sigma.get_mut(&w) {
                            *s += sigma_v;
                        }
                        predecessors.entry(w).or_default().push(v);
                    }
                }
            }
        }

        // 回溯累积
        let mut delta: HashMap<NodeId, f64> = all_ids.iter().map(|&id| (id, 0.0)).collect();
        while let Some(w) = stack.pop() {
            if let Some(preds) = predecessors.get(&w) {
                let sigma_w = sigma[&w];
                if sigma_w > 0.0 {
                    for &v in preds {
                        let coeff = (sigma[&v] / sigma_w) * (1.0 + delta[&w]);
                        if let Some(d) = delta.get_mut(&v) {
                            *d += coeff;
                        }
                    }
                }
            }
            if w != s
                && let Some(c) = centrality.get_mut(&w)
            {
                *c += delta[&w];
            }
        }
    }

    // 归一化：对有向图，分母为 (N-1)(N-2)
    let norm = ((n - 1) * (n - 2)) as f64;
    if norm > 0.0 {
        for val in centrality.values_mut() {
            *val /= norm;
        }
    }

    // 若使用采样，按比例放大
    if let Some(k) = sample_size
        && k < n
    {
        let scale = n as f64 / k as f64;
        for val in centrality.values_mut() {
            *val *= scale;
        }
    }

    centrality
}
