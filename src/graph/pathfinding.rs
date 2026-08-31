//! 图谱路径算法模块
//!
//! 提供 BFS 最短路径、可变长路径遍历、全路径枚举等图查询算法。
//! 所有算法接收 `&MemTable<T>` 只读引用，不修改状态。

use crate::VectorType;
use crate::error::Result;
use crate::graph::budget::{BudgetDimension, TraversalBudget, TraversalMetrics, exhausted};
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

/// 路径查询结果：一条从起点到终点的完整节点链
pub type Path = Vec<NodeId>;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ShortestPathOutput {
    pub path: Option<Path>,
    pub metrics: TraversalMetrics,
    pub truncated: bool,
    pub bidirectional: bool,
}

pub fn shortest_path_bidirectional<T: VectorType>(
    mt: &MemTable<T>,
    src: NodeId,
    dst: NodeId,
    label_filter: Option<&str>,
    budget: &TraversalBudget,
) -> Result<ShortestPathOutput> {
    budget.validate()?;
    if !mt.contains(src) {
        return Err(crate::error::TriviumError::NodeNotFound(src));
    }
    if !mt.contains(dst) {
        return Err(crate::error::TriviumError::NodeNotFound(dst));
    }
    if src == dst {
        return Ok(ShortestPathOutput {
            path: Some(vec![src]),
            metrics: TraversalMetrics {
                visited_nodes: 1,
                peak_frontier_size: 1,
                ..Default::default()
            },
            truncated: false,
            bidirectional: true,
        });
    }

    let mut forward = HashMap::from([(src, (0usize, None))]);
    let mut backward = HashMap::from([(dst, (0usize, None))]);
    let mut forward_frontier = vec![src];
    let mut backward_frontier = vec![dst];
    let mut metrics = TraversalMetrics {
        visited_nodes: 2,
        peak_frontier_size: 2,
        ..Default::default()
    };
    let mut truncated = false;
    let mut best_meeting: Option<(usize, NodeId)> = None;

    while !forward_frontier.is_empty() && !backward_frontier.is_empty() {
        let combined_depth = forward[&forward_frontier[0]].0 + backward[&backward_frontier[0]].0;
        if best_meeting.is_some_and(|(distance, _)| combined_depth >= distance) {
            break;
        }
        if combined_depth >= budget.max_depth {
            if exhausted(budget, BudgetDimension::Depth, metrics)? {
                truncated = true;
            }
            break;
        }
        let expand_forward = forward_frontier.len() <= backward_frontier.len();
        let frontier = if expand_forward {
            &forward_frontier
        } else {
            &backward_frontier
        };
        let mut next_frontier = Vec::new();
        let mut meeting = Vec::new();
        for &current in frontier {
            let current_depth = if expand_forward {
                forward[&current].0
            } else {
                backward[&current].0
            };
            let mut neighbors: Vec<NodeId> = if expand_forward {
                mt.get_edges(current)
                    .unwrap_or(&[])
                    .iter()
                    .filter(|edge| label_filter.is_none_or(|label| edge.label == label))
                    .map(|edge| edge.target_id)
                    .collect()
            } else {
                mt.get_incoming_edges(current, label_filter)
                    .into_iter()
                    .map(|edge| edge.source_id)
                    .collect()
            };
            neighbors.sort_unstable();
            neighbors.dedup();
            for neighbor in neighbors {
                if metrics.examined_edges >= budget.max_examined_edges {
                    if exhausted(budget, BudgetDimension::ExaminedEdges, metrics)? {
                        truncated = true;
                    }
                    break;
                }
                metrics.examined_edges += 1;
                let own = if expand_forward {
                    &mut forward
                } else {
                    &mut backward
                };
                if let std::collections::hash_map::Entry::Vacant(entry) = own.entry(neighbor) {
                    if metrics.visited_nodes >= budget.max_visited_nodes {
                        if exhausted(budget, BudgetDimension::VisitedNodes, metrics)? {
                            truncated = true;
                        }
                        break;
                    }
                    entry.insert((current_depth + 1, Some(current)));
                    metrics.visited_nodes += 1;
                    metrics.depth_reached = metrics.depth_reached.max(current_depth + 1);
                    next_frontier.push(neighbor);
                }
                let other = if expand_forward { &backward } else { &forward };
                if other.contains_key(&neighbor) {
                    meeting.push(neighbor);
                }
            }
            if truncated {
                break;
            }
        }
        if truncated {
            break;
        }
        if !meeting.is_empty() {
            for node in meeting {
                let candidate = (forward[&node].0 + backward[&node].0, node);
                if best_meeting.is_none_or(|best| candidate < best) {
                    best_meeting = Some(candidate);
                }
            }
        }
        next_frontier.sort_unstable();
        next_frontier.dedup();
        let next_size = next_frontier.len()
            + if expand_forward {
                backward_frontier.len()
            } else {
                forward_frontier.len()
            };
        metrics.peak_frontier_size = metrics.peak_frontier_size.max(next_size);
        if next_size > budget.max_frontier_size {
            if exhausted(budget, BudgetDimension::FrontierSize, metrics)? {
                truncated = true;
            }
            break;
        }
        if expand_forward {
            forward_frontier = next_frontier;
        } else {
            backward_frontier = next_frontier;
        }
    }

    Ok(ShortestPathOutput {
        path: best_meeting
            .map(|(_, meeting)| reconstruct_bidirectional(meeting, &forward, &backward)),
        metrics,
        truncated,
        bidirectional: true,
    })
}

fn reconstruct_bidirectional(
    meeting: NodeId,
    forward: &HashMap<NodeId, (usize, Option<NodeId>)>,
    backward: &HashMap<NodeId, (usize, Option<NodeId>)>,
) -> Path {
    let mut left = vec![meeting];
    let mut current = meeting;
    while let Some(parent) = forward[&current].1 {
        left.push(parent);
        current = parent;
    }
    left.reverse();
    current = meeting;
    while let Some(parent) = backward[&current].1 {
        left.push(parent);
        current = parent;
    }
    left
}

/// BFS 最短路径：找到从 src 到 dst 的最短节点序列
///
/// - 如果 `label_filter` 为 Some，只沿匹配标签的边行走
/// - `max_depth` 限制最大搜索深度，防止在大图上无限扩展
/// - 返回 None 表示在 max_depth 跳内不可达
///
/// 时间复杂度：O(V + E)，其中 V/E 为 max_depth 范围内的可达子图规模
pub fn shortest_path<T: VectorType>(
    mt: &MemTable<T>,
    src: NodeId,
    dst: NodeId,
    max_depth: usize,
    label_filter: Option<&str>,
) -> Option<Path> {
    if src == dst {
        return Some(vec![src]);
    }
    if max_depth == 0 {
        return None;
    }

    let mut visited: HashSet<NodeId> = HashSet::new();
    visited.insert(src);

    // BFS 队列：(当前节点, 从起点到当前节点的路径)
    let mut queue: VecDeque<(NodeId, Vec<NodeId>)> = VecDeque::new();
    queue.push_back((src, vec![src]));

    while let Some((current, path)) = queue.pop_front() {
        if path.len() > max_depth {
            break;
        }

        if let Some(edges) = mt.get_edges(current) {
            for edge in edges {
                // 标签过滤
                if let Some(lf) = label_filter
                    && edge.label != lf
                {
                    continue;
                }

                let next = edge.target_id;
                if next == dst {
                    let mut result = path.clone();
                    result.push(dst);
                    return Some(result);
                }

                if !visited.contains(&next) && path.len() < max_depth {
                    visited.insert(next);
                    let mut new_path = path.clone();
                    new_path.push(next);
                    queue.push_back((next, new_path));
                }
            }
        }
    }

    None
}

/// 可变长路径遍历：找到从 src 出发、跳数在 [min_depth, max_depth] 范围内的所有可达终点
///
/// - 如果 `label_filter` 为 Some，只沿匹配标签的边行走
/// - 使用 DFS + visited 集合防环
/// - `limit` 限制最大返回路径数，防止组合爆炸
///
/// 返回所有满足深度约束的 (终点 ID, 路径) 对
pub fn variable_length_paths<T: VectorType>(
    mt: &MemTable<T>,
    src: NodeId,
    min_depth: usize,
    max_depth: usize,
    label_filter: Option<&str>,
    limit: usize,
) -> Vec<(NodeId, Path)> {
    let mut results = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(src);

    dfs_variable_length(
        mt,
        src,
        &vec![src],
        min_depth,
        max_depth,
        label_filter,
        &mut visited,
        &mut results,
        limit,
    );

    results
}

fn dfs_variable_length<T: VectorType>(
    mt: &MemTable<T>,
    current: NodeId,
    path: &Vec<NodeId>,
    min_depth: usize,
    max_depth: usize,
    label_filter: Option<&str>,
    visited: &mut HashSet<NodeId>,
    results: &mut Vec<(NodeId, Path)>,
    limit: usize,
) {
    let depth = path.len() - 1; // 路径中的边数 = 节点数 - 1

    // 当前深度在有效范围内，收集结果
    if depth >= min_depth {
        results.push((current, path.clone()));
        if results.len() >= limit {
            return;
        }
    }

    // 已达最大深度，不再展开
    if depth >= max_depth {
        return;
    }

    if let Some(edges) = mt.get_edges(current) {
        for edge in edges {
            // 标签过滤
            if let Some(lf) = label_filter
                && edge.label != lf
            {
                continue;
            }

            let next = edge.target_id;
            if visited.contains(&next) {
                continue; // 防环
            }

            visited.insert(next);
            let mut new_path = path.clone();
            new_path.push(next);

            dfs_variable_length(
                mt,
                next,
                &new_path,
                min_depth,
                max_depth,
                label_filter,
                visited,
                results,
                limit,
            );

            if results.len() >= limit {
                visited.remove(&next);
                return;
            }

            visited.remove(&next); // 回溯
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BoundedPath {
    pub nodes: Path,
    pub edge_weights: Vec<f32>,
    pub labels: Vec<String>,
    pub strength_product: f32,
    pub strength_average: f32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BoundedPathsOutput {
    pub paths: Vec<BoundedPath>,
    pub metrics: TraversalMetrics,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedPathConfig {
    pub max_depth: usize,
    pub max_paths: usize,
    pub label_sequence: Option<Vec<String>>,
    pub forbidden_nodes: BTreeSet<NodeId>,
}

/// 在统一遍历预算内确定性枚举简单路径，并计算边权强度。
pub fn bounded_all_paths<T: VectorType>(
    mt: &MemTable<T>,
    src: NodeId,
    dst: NodeId,
    config: &BoundedPathConfig,
    budget: &TraversalBudget,
) -> Result<BoundedPathsOutput> {
    budget.validate()?;
    if !mt.contains(src) {
        return Err(crate::error::TriviumError::NodeNotFound(src));
    }
    if !mt.contains(dst) {
        return Err(crate::error::TriviumError::NodeNotFound(dst));
    }
    if config.max_paths == 0 {
        return Err(crate::error::TriviumError::InvalidInput(
            "最大路径数必须大于 0 (max_paths must be greater than zero)".into(),
        ));
    }
    let max_depth = config.max_depth.min(budget.max_depth);
    let mut state = BoundedPathState {
        results: Vec::new(),
        visited: HashSet::from([src]),
        nodes: vec![src],
        weights: Vec::new(),
        labels: Vec::new(),
        metrics: TraversalMetrics {
            visited_nodes: 1,
            peak_frontier_size: 1,
            ..Default::default()
        },
        truncated: false,
    };
    bounded_path_dfs(mt, src, dst, config, budget, max_depth, &mut state)?;
    state
        .results
        .sort_by(|left, right| left.nodes.cmp(&right.nodes));
    Ok(BoundedPathsOutput {
        paths: state.results,
        metrics: state.metrics,
        truncated: state.truncated,
    })
}

struct BoundedPathState {
    results: Vec<BoundedPath>,
    visited: HashSet<NodeId>,
    nodes: Vec<NodeId>,
    weights: Vec<f32>,
    labels: Vec<String>,
    metrics: TraversalMetrics,
    truncated: bool,
}

fn bounded_path_dfs<T: VectorType>(
    mt: &MemTable<T>,
    current: NodeId,
    dst: NodeId,
    config: &BoundedPathConfig,
    budget: &TraversalBudget,
    max_depth: usize,
    state: &mut BoundedPathState,
) -> Result<()> {
    if state.truncated {
        return Ok(());
    }
    let depth = state.weights.len();
    state.metrics.depth_reached = state.metrics.depth_reached.max(depth);
    if current == dst {
        let product = state.weights.iter().copied().product::<f32>();
        state.results.push(BoundedPath {
            nodes: state.nodes.clone(),
            edge_weights: state.weights.clone(),
            labels: state.labels.clone(),
            strength_product: product,
            strength_average: if state.weights.is_empty() {
                1.0
            } else {
                state.weights.iter().copied().sum::<f32>() / state.weights.len() as f32
            },
        });
        if state.results.len() >= config.max_paths {
            state.truncated = true;
        }
        return Ok(());
    }
    if depth >= max_depth {
        return Ok(());
    }
    let mut edges = mt
        .get_edges(current)
        .unwrap_or(&[])
        .iter()
        .collect::<Vec<_>>();
    edges.sort_by(|left, right| {
        left.target_id
            .cmp(&right.target_id)
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.weight.total_cmp(&right.weight))
    });
    state.metrics.peak_frontier_size = state.metrics.peak_frontier_size.max(edges.len());
    if edges.len() > budget.max_frontier_size {
        state.truncated = exhausted(budget, BudgetDimension::FrontierSize, state.metrics)?;
        return Ok(());
    }
    for edge in edges {
        if state.metrics.examined_edges >= budget.max_examined_edges {
            state.truncated = exhausted(budget, BudgetDimension::ExaminedEdges, state.metrics)?;
            break;
        }
        state.metrics.examined_edges += 1;
        if config
            .label_sequence
            .as_ref()
            .is_some_and(|sequence| sequence.get(depth) != Some(&edge.label))
            || config.forbidden_nodes.contains(&edge.target_id)
            || state.visited.contains(&edge.target_id)
        {
            continue;
        }
        if state.metrics.visited_nodes >= budget.max_visited_nodes {
            state.truncated = exhausted(budget, BudgetDimension::VisitedNodes, state.metrics)?;
            break;
        }
        state.metrics.visited_nodes += 1;
        state.visited.insert(edge.target_id);
        state.nodes.push(edge.target_id);
        state.weights.push(edge.weight);
        state.labels.push(edge.label.clone());
        bounded_path_dfs(mt, edge.target_id, dst, config, budget, max_depth, state)?;
        state.labels.pop();
        state.weights.pop();
        state.nodes.pop();
        state.visited.remove(&edge.target_id);
        if state.truncated {
            break;
        }
    }
    Ok(())
}

/// 全路径枚举：找到从 src 到 dst 的所有路径（不允许环路）
///
/// - 如果 `label_filter` 为 Some，只沿匹配标签的边行走
/// - `max_depth` 限制最大路径长度
/// - `limit` 限制最大返回路径数（熔断防护）
///
/// 用于溯源研判：列出两个实体之间的所有可能关联链路
pub fn all_paths<T: VectorType>(
    mt: &MemTable<T>,
    src: NodeId,
    dst: NodeId,
    max_depth: usize,
    label_filter: Option<&str>,
    limit: usize,
) -> Vec<Path> {
    let mut results = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(src);

    dfs_all_paths(
        mt,
        src,
        dst,
        &vec![src],
        max_depth,
        label_filter,
        &mut visited,
        &mut results,
        limit,
    );

    results
}

fn dfs_all_paths<T: VectorType>(
    mt: &MemTable<T>,
    current: NodeId,
    dst: NodeId,
    path: &Vec<NodeId>,
    max_depth: usize,
    label_filter: Option<&str>,
    visited: &mut HashSet<NodeId>,
    results: &mut Vec<Path>,
    limit: usize,
) {
    if current == dst && path.len() > 1 {
        results.push(path.clone());
        return;
    }

    let depth = path.len() - 1;
    if depth >= max_depth || results.len() >= limit {
        return;
    }

    if let Some(edges) = mt.get_edges(current) {
        for edge in edges {
            if let Some(lf) = label_filter
                && edge.label != lf
            {
                continue;
            }

            let next = edge.target_id;
            if visited.contains(&next) && next != dst {
                continue;
            }

            // 允许到达 dst（即使 dst 可能在 visited 中不存在，因为我们不提前加入它）
            if next == dst {
                let mut result_path = path.clone();
                result_path.push(dst);
                results.push(result_path);
                if results.len() >= limit {
                    return;
                }
                continue;
            }

            visited.insert(next);
            let mut new_path = path.clone();
            new_path.push(next);

            dfs_all_paths(
                mt,
                next,
                dst,
                &new_path,
                max_depth,
                label_filter,
                visited,
                results,
                limit,
            );

            if results.len() >= limit {
                visited.remove(&next);
                return;
            }

            visited.remove(&next);
        }
    }
}

/// K-hop 邻域：从 src 出发，返回 K 跳范围内的所有可达节点及其最短距离
///
/// 用于影响力范围评估、事件扩散分析等场景。
pub fn k_hop_neighbors<T: VectorType>(
    mt: &MemTable<T>,
    src: NodeId,
    k: usize,
    label_filter: Option<&str>,
) -> HashMap<NodeId, usize> {
    let mut distances: HashMap<NodeId, usize> = HashMap::new();
    distances.insert(src, 0);

    let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();
    queue.push_back((src, 0));

    while let Some((current, depth)) = queue.pop_front() {
        if depth >= k {
            continue;
        }

        if let Some(edges) = mt.get_edges(current) {
            for edge in edges {
                if let Some(lf) = label_filter
                    && edge.label != lf
                {
                    continue;
                }

                let next = edge.target_id;
                if let std::collections::hash_map::Entry::Vacant(e) = distances.entry(next) {
                    e.insert(depth + 1);
                    queue.push_back((next, depth + 1));
                }
            }
        }
    }

    distances
}
