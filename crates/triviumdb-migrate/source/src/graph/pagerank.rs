//! PageRank 算法
//!
//! 迭代式 PageRank，支持阻尼因子、收敛检测、悬挂节点处理、边标签过滤。
//!
//! - 时间复杂度：O(V · iterations)，通常 20-50 次迭代收敛
//! - 用途：实体重要性排名、热度传播分析

use crate::VectorType;
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use std::collections::HashMap;

/// PageRank 算法配置
pub struct PageRankConfig {
    /// 阻尼因子（默认 0.85）
    pub damping: f64,
    /// 最大迭代次数（默认 100）
    pub max_iterations: usize,
    /// 收敛阈值（L1 范数差值低于此值则提前停止，默认 1e-6）
    pub tolerance: f64,
}

impl Default for PageRankConfig {
    fn default() -> Self {
        Self {
            damping: 0.85,
            max_iterations: 100,
            tolerance: 1e-6,
        }
    }
}

/// 计算所有节点的 PageRank 分数
///
/// - 如果 `label_filter` 为 Some，只沿匹配标签的边传播权重
/// - 返回 `Vec<(NodeId, f64)>`，按分数降序排列
pub fn pagerank<T: VectorType>(
    mt: &MemTable<T>,
    config: &PageRankConfig,
    label_filter: Option<&str>,
) -> Vec<(NodeId, f64)> {
    let all_ids = mt.all_node_ids();
    let n = all_ids.len();
    if n == 0 {
        return Vec::new();
    }

    let init_val = 1.0 / n as f64;
    let mut scores: HashMap<NodeId, f64> = all_ids.iter().map(|&id| (id, init_val)).collect();

    // 预计算每个节点的有效出度（考虑标签过滤）
    let out_degrees: HashMap<NodeId, usize> = all_ids
        .iter()
        .map(|&id| {
            let deg = mt
                .get_edges(id)
                .map(|edges| match label_filter {
                    None => edges.len(),
                    Some(lf) => edges.iter().filter(|e| e.label == lf).count(),
                })
                .unwrap_or(0);
            (id, deg)
        })
        .collect();

    let teleport = (1.0 - config.damping) / n as f64;

    for _iter in 0..config.max_iterations {
        let mut new_scores: HashMap<NodeId, f64> = all_ids.iter().map(|&id| (id, 0.0)).collect();

        // 悬挂节点（出度为 0）的权重均匀分配
        let dangling_sum: f64 = all_ids
            .iter()
            .filter(|&&id| out_degrees[&id] == 0)
            .map(|&id| scores[&id])
            .sum();
        let dangling_contrib = config.damping * dangling_sum / n as f64;

        // 正常传播
        for &src in &all_ids {
            let src_score = scores[&src];
            let src_out = out_degrees[&src];
            if src_out == 0 {
                continue;
            }

            if let Some(edges) = mt.get_edges(src) {
                for edge in edges {
                    if let Some(lf) = label_filter
                        && edge.label != lf
                    {
                        continue;
                    }
                    let dst = edge.target_id;
                    if let Some(val) = new_scores.get_mut(&dst) {
                        *val += config.damping * src_score / src_out as f64;
                    }
                }
            }
        }

        // 加上 teleport 和悬挂贡献
        let mut diff = 0.0_f64;
        for &id in &all_ids {
            let new_val = new_scores[&id] + teleport + dangling_contrib;
            diff += (new_val - scores[&id]).abs();
            new_scores.insert(id, new_val);
        }

        scores = new_scores;

        if diff < config.tolerance {
            break;
        }
    }

    let mut result: Vec<(NodeId, f64)> = scores.into_iter().collect();
    result.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    result
}
