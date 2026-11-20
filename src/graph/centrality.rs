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
use std::collections::{HashMap, VecDeque};

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

    results.sort_by(|a, b| {
        b.total_degree
            .cmp(&a.total_degree)
            .then_with(|| a.id.cmp(&b.id))
    });
    results
}

// ═══════════════════════════════════════════════════════════════════════
//  Betweenness 中心性 (Brandes 算法)
// ═══════════════════════════════════════════════════════════════════════

/// 计算所有节点的 Betweenness 中心性（Brandes 算法）
///
/// - 如果 `label_filter` 为 Some，只沿匹配标签的边计算
/// - 如果 `sample_size` 为 Some(k)，只从 k 个源节点出发（近似计算，降低大图开销）
/// - 零采样时所有节点的贡献为零。
/// - 返回 `HashMap<NodeId, f64>`，值为归一化后的中心性分数
/// - 用途：发现信息瓶颈节点——移除该节点后最大程度破坏图连通性
pub fn betweenness_centrality<T: VectorType>(
    mt: &MemTable<T>,
    label_filter: Option<&str>,
    sample_size: Option<usize>,
) -> HashMap<NodeId, f64> {
    let mut ids = mt.all_node_ids();
    ids.sort_unstable();
    let n = ids.len();
    let sources = sample_size.unwrap_or(n).min(n);
    let mut scores: HashMap<_, _> = ids.iter().map(|&id| (id, 0.0)).collect();
    if sources == 0 || n < 3 {
        return scores;
    }
    let adjacency = ids
        .iter()
        .map(|&id| {
            let neighbors = mt
                .get_edges(id)
                .unwrap_or_default()
                .iter()
                .filter(|edge| label_filter.is_none_or(|label| edge.label == label))
                .filter(|edge| mt.contains(edge.target_id))
                .map(|edge| edge.target_id)
                .collect::<Vec<_>>();
            (id, neighbors)
        })
        .collect();
    for &source in &ids[..sources] {
        let partial = brandes_source(
            &adjacency,
            source,
            |_| Ok::<_, std::convert::Infallible>(()),
        )
        .unwrap_or_else(|never| match never {});
        for (id, value) in partial {
            if let Some(score) = scores.get_mut(&id) {
                *score += value;
            }
        }
    }
    for value in scores.values_mut() {
        *value = normalize(*value, n, sources);
    }
    scores
}

pub(super) fn normalize(value: f64, nodes: usize, sources: usize) -> f64 {
    if nodes < 3 || sources == 0 {
        return 0.0;
    }
    value / ((nodes - 1) as f64 * (nodes - 2) as f64) * (nodes as f64 / sources as f64)
}

#[derive(Default)]
struct Visit {
    depth: usize,
    log_paths: f64,
    predecessors: Vec<NodeId>,
    dependency: f64,
}

/// 在对数域累积最短路径条数，回溯只需要前驱与目标的条数比值。
pub(super) fn brandes_source<E>(
    adjacency: &HashMap<NodeId, Vec<NodeId>>,
    source: NodeId,
    mut admit: impl FnMut(usize) -> std::result::Result<(), E>,
) -> std::result::Result<Vec<(NodeId, f64)>, E> {
    use std::collections::hash_map::Entry;
    let mut visits = HashMap::from([(source, Visit::default())]);
    let mut stack = Vec::new();
    let mut queue = VecDeque::from([source]);
    while let Some(id) = queue.pop_front() {
        stack.push(id);
        let depth = visits[&id].depth + 1;
        let log_paths = visits[&id].log_paths;
        let neighbors = &adjacency[&id];
        admit(neighbors.len())?;
        for &target in neighbors {
            admit(0)?;
            match visits.entry(target) {
                Entry::Vacant(entry) => {
                    entry.insert(Visit {
                        depth,
                        log_paths,
                        predecessors: vec![id],
                        dependency: 0.0,
                    });
                    queue.push_back(target);
                }
                Entry::Occupied(mut entry) if entry.get().depth == depth => {
                    let target = entry.get_mut();
                    let previous = target.log_paths;
                    target.log_paths =
                        previous.max(log_paths) + (-(previous - log_paths).abs()).exp().ln_1p();
                    target.predecessors.push(id);
                }
                _ => {}
            }
        }
    }
    let mut partial = Vec::new();
    while let Some(id) = stack.pop() {
        admit(0)?;
        let visit = visits
            .remove(&id)
            .expect("每个入栈节点在逆序回溯时恰好出栈一次");
        for predecessor in visit.predecessors {
            admit(0)?;
            if let Some(parent) = visits.get_mut(&predecessor) {
                parent.dependency +=
                    (parent.log_paths - visit.log_paths).exp() * (1.0 + visit.dependency);
            }
        }
        if id != source && visit.dependency != 0.0 {
            partial.push((id, visit.dependency));
        }
    }
    Ok(partial)
}
