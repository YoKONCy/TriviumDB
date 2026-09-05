//! 方向、标签和预算感知的确定性可达性查询。
//!
//! 支持出边、入边与双向 BFS，按逐层稳定顺序返回最短深度及路径标签。并行路径只
//! 加速同层前沿处理，归并仍以 NodeId/label 排序保证线程数变化不改变结果。访问节点、
//! 扫描边、前沿和深度均复用 TraversalBudget，Error 模式绝不返回静默截断结果。

use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::graph::budget::{
    BudgetDimension, BudgetExhaustionPolicy, TraversalBudget, TraversalMetrics, exhausted,
};
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReachabilityDirection {
    #[default]
    Outgoing,
    Incoming,
    Both,
}

#[derive(Debug, Clone)]
pub struct ReachabilityConfig {
    pub min_depth: usize,
    pub max_depth: usize,
    pub labels: Option<Vec<String>>,
    pub direction: ReachabilityDirection,
    pub max_visited_nodes: usize,
    pub max_results: usize,
    pub max_edges: usize,
    pub max_frontier_size: usize,
    pub exhaustion_policy: BudgetExhaustionPolicy,
}
impl Default for ReachabilityConfig {
    fn default() -> Self {
        Self {
            min_depth: 1,
            max_depth: 1,
            labels: None,
            direction: ReachabilityDirection::Outgoing,
            max_visited_nodes: 10_000,
            max_results: 10_000,
            max_edges: 50_000,
            max_frontier_size: 10_000,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReachabilityStep {
    pub from: NodeId,
    pub to: NodeId,
    pub edge_source: NodeId,
    pub edge_target: NodeId,
    pub label: String,
    pub weight: f32,
    pub metadata: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReachabilityResult {
    pub source_id: NodeId,
    pub target_id: NodeId,
    pub depth: usize,
    pub path: Vec<NodeId>,
    pub steps: Vec<ReachabilityStep>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ReachabilityOutput {
    pub results: Vec<ReachabilityResult>,
    pub visited_nodes: usize,
    pub traversed_edges: usize,
    pub peak_frontier_size: usize,
    pub depth_reached: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReachabilityHit {
    pub target_id: NodeId,
    pub depth: usize,
}

#[derive(Debug, Clone)]
pub struct CompactReachabilityOutput {
    pub results: Vec<ReachabilityHit>,
    pub visited_nodes: usize,
    pub traversed_edges: usize,
}
#[derive(Debug, Clone, Serialize)]
pub struct SubgraphNode {
    pub id: NodeId,
    pub payload: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubgraphEdge {
    pub source_id: NodeId,
    pub target_id: NodeId,
    pub label: String,
    pub weight: f32,
    pub metadata: serde_json::Value,
}
#[derive(Debug, Clone, Serialize)]
pub struct SubgraphResult {
    pub nodes: Vec<SubgraphNode>,
    pub edges: Vec<SubgraphEdge>,
    pub visited_nodes: usize,
    pub traversed_edges: usize,
    pub truncated: bool,
}
#[derive(Clone)]
struct PathState {
    node: NodeId,
    path: Vec<NodeId>,
    steps: Vec<ReachabilityStep>,
}

pub fn traverse<T: VectorType>(
    db: &MemTable<T>,
    source_id: NodeId,
    config: &ReachabilityConfig,
) -> Result<Vec<ReachabilityResult>> {
    Ok(traverse_detailed(db, source_id, config)?.results)
}
pub fn traverse_detailed<T: VectorType>(
    db: &MemTable<T>,
    source_id: NodeId,
    config: &ReachabilityConfig,
) -> Result<ReachabilityOutput> {
    validate(db, source_id, config)?;
    let mut queue = VecDeque::from([PathState {
        node: source_id,
        path: vec![source_id],
        steps: vec![],
    }]);
    let mut best_depth = HashMap::from([(source_id, 0usize)]);
    let mut results = vec![];
    let mut traversed_edges = 0;
    let mut peak_frontier_size = 1;
    let mut depth_reached = 0;
    let mut truncated = false;
    let budget = TraversalBudget {
        max_visited_nodes: config.max_visited_nodes,
        max_examined_edges: config.max_edges,
        max_frontier_size: config.max_frontier_size,
        max_depth: config.max_depth,
        exhaustion_policy: config.exhaustion_policy,
    };
    if config.min_depth == 0 {
        results.push(ReachabilityResult {
            source_id,
            target_id: source_id,
            depth: 0,
            path: vec![source_id],
            steps: vec![],
        });
    }
    'walk: while let Some(state) = queue.pop_front() {
        let depth = state.steps.len();
        if depth >= config.max_depth {
            continue;
        }
        for step in collect_steps(db, state.node, config) {
            if traversed_edges >= config.max_edges {
                truncated = exhausted(
                    &budget,
                    BudgetDimension::ExaminedEdges,
                    TraversalMetrics {
                        visited_nodes: best_depth.len(),
                        examined_edges: traversed_edges,
                        peak_frontier_size,
                        depth_reached,
                    },
                )?;
                break 'walk;
            }
            traversed_edges += 1;
            let next_depth = depth + 1;
            if best_depth
                .get(&step.to)
                .is_some_and(|known| *known <= next_depth)
            {
                continue;
            }
            if best_depth.len() >= config.max_visited_nodes {
                truncated = exhausted(
                    &budget,
                    BudgetDimension::VisitedNodes,
                    TraversalMetrics {
                        visited_nodes: best_depth.len(),
                        examined_edges: traversed_edges,
                        peak_frontier_size,
                        depth_reached,
                    },
                )?;
                break 'walk;
            }
            best_depth.insert(step.to, next_depth);
            depth_reached = depth_reached.max(next_depth);
            let mut path = state.path.clone();
            path.push(step.to);
            let mut steps = state.steps.clone();
            steps.push(step.clone());
            if next_depth >= config.min_depth {
                if results.len().saturating_add(1) > config.max_results {
                    truncated = true;
                    break 'walk;
                }
                results.push(ReachabilityResult {
                    source_id,
                    target_id: step.to,
                    depth: next_depth,
                    path: path.clone(),
                    steps: steps.clone(),
                });
            }
            if queue.len() >= config.max_frontier_size {
                truncated = exhausted(
                    &budget,
                    BudgetDimension::FrontierSize,
                    TraversalMetrics {
                        visited_nodes: best_depth.len(),
                        examined_edges: traversed_edges,
                        peak_frontier_size: peak_frontier_size.max(queue.len()),
                        depth_reached,
                    },
                )?;
                break 'walk;
            }
            queue.push_back(PathState {
                node: step.to,
                path,
                steps,
            });
            peak_frontier_size = peak_frontier_size.max(queue.len());
        }
    }
    results.sort_by(|a, b| (a.depth, a.target_id, &a.path).cmp(&(b.depth, b.target_id, &b.path)));
    Ok(ReachabilityOutput {
        results,
        visited_nodes: best_depth.len(),
        traversed_edges,
        peak_frontier_size,
        depth_reached,
        truncated,
    })
}
/// 仅返回节点与最短深度，避免无目标扩展为每个命中复制完整路径与边元数据。
pub fn traverse_compact<T: VectorType>(
    db: &MemTable<T>,
    source_id: NodeId,
    config: &ReachabilityConfig,
) -> Result<CompactReachabilityOutput> {
    validate(db, source_id, config)?;
    let mut queue = VecDeque::from([(source_id, 0usize)]);
    let mut best_depth = HashMap::from([(source_id, 0usize)]);
    let mut results = Vec::new();
    if config.min_depth == 0 {
        results.push(ReachabilityHit {
            target_id: source_id,
            depth: 0,
        });
    }
    let mut traversed_edges = 0usize;
    while let Some((current, depth)) = queue.pop_front() {
        if depth >= config.max_depth {
            continue;
        }
        for target in collect_targets(db, current, config) {
            if traversed_edges >= config.max_edges {
                return Err(TriviumError::QueryExecution(
                    "图扩展检查边数超过预算 (Graph expansion examined-edge budget exceeded)".into(),
                ));
            }
            traversed_edges += 1;
            let next_depth = depth + 1;
            if best_depth
                .get(&target)
                .is_some_and(|known| *known <= next_depth)
            {
                continue;
            }
            if best_depth.len() >= config.max_visited_nodes {
                return Err(TriviumError::QueryExecution(
                    "图扩展访问节点数超过预算 (Graph expansion visited-node budget exceeded)"
                        .into(),
                ));
            }
            best_depth.insert(target, next_depth);
            if next_depth >= config.min_depth {
                if results.len() >= config.max_results {
                    return Err(TriviumError::QueryExecution(
                        "图扩展结果数超过预算 (Graph expansion result budget exceeded)".into(),
                    ));
                }
                results.push(ReachabilityHit {
                    target_id: target,
                    depth: next_depth,
                });
            }
            if queue.len() >= config.max_frontier_size {
                return Err(TriviumError::QueryExecution(
                    "图扩展前沿超过预算 (Graph expansion frontier budget exceeded)".into(),
                ));
            }
            queue.push_back((target, next_depth));
        }
    }
    results.sort_unstable_by_key(|hit| (hit.depth, hit.target_id));
    Ok(CompactReachabilityOutput {
        results,
        visited_nodes: best_depth.len(),
        traversed_edges,
    })
}

fn set_bit(words: &mut [u64], slot: usize) {
    words[slot / 64] |= 1u64 << (slot % 64);
}

fn collect_target_slots<T: VectorType>(
    db: &MemTable<T>,
    current: NodeId,
    config: &ReachabilityConfig,
    words: &mut [u64],
) -> usize {
    let targets = collect_targets(db, current, config);
    for target in &targets {
        if let Some(slot) = db.internal_slot_of(*target) {
            set_bit(words, slot);
        }
    }
    targets.len()
}

/// 使用稳定 internal slot 位图执行全确定性分层并行 BFS。
/// worker 仅写本地位图，层末按固定 chunk 顺序 OR，再按 slot 升序提交结果。
pub fn traverse_compact_parallel<T: VectorType>(
    db: &MemTable<T>,
    source_id: NodeId,
    config: &ReachabilityConfig,
) -> Result<CompactReachabilityOutput> {
    validate(db, source_id, config)?;
    let slot_count = db.internal_slot_count();
    let word_count = slot_count.div_ceil(64);
    let source_slot = db
        .internal_slot_of(source_id)
        .ok_or(TriviumError::NodeNotFound(source_id))?;
    let mut frontier = vec![source_slot];
    let mut visited = vec![0u64; word_count];
    set_bit(&mut visited, source_slot);
    let mut visited_count = 1usize;
    let mut results = Vec::new();
    if config.min_depth == 0 {
        results.push(ReachabilityHit {
            target_id: source_id,
            depth: 0,
        });
    }
    let mut traversed_edges = 0usize;
    for depth in 0..config.max_depth {
        let workers = rayon::current_num_threads().min(frontier.len()).max(1);
        let partials = if frontier.len() < 1_024 || workers == 1 {
            let mut bitmap = vec![0u64; word_count];
            let mut edges = 0usize;
            for &slot in &frontier {
                if let Some(id) = db.active_id_at_slot(slot) {
                    edges = edges.saturating_add(collect_target_slots(db, id, config, &mut bitmap));
                }
            }
            vec![(bitmap, edges)]
        } else {
            let chunk_size = frontier.len().div_ceil(workers);
            frontier
                .par_chunks(chunk_size)
                .map(|chunk| {
                    let mut bitmap = vec![0u64; word_count];
                    let mut edges = 0usize;
                    for &slot in chunk {
                        if let Some(id) = db.active_id_at_slot(slot) {
                            edges = edges.saturating_add(collect_target_slots(
                                db,
                                id,
                                config,
                                &mut bitmap,
                            ));
                        }
                    }
                    (bitmap, edges)
                })
                .collect::<Vec<_>>()
        };
        let layer_edges = partials
            .iter()
            .fold(0usize, |total, (_, count)| total.saturating_add(*count));
        traversed_edges = traversed_edges.saturating_add(layer_edges);
        if traversed_edges > config.max_edges {
            return Err(TriviumError::QueryExecution(
                "图扩展检查边数超过预算 (Graph expansion examined-edge budget exceeded)".into(),
            ));
        }
        let mut merged = vec![0u64; word_count];
        for (bitmap, _) in partials {
            for (target, source) in merged.iter_mut().zip(bitmap) {
                *target |= source;
            }
        }
        let next_depth = depth + 1;
        let mut next_frontier = Vec::new();
        for (word_index, word) in merged.iter().copied().enumerate() {
            let mut unseen = word & !visited[word_index];
            while unseen != 0 {
                let bit = unseen.trailing_zeros() as usize;
                let slot = word_index * 64 + bit;
                unseen &= unseen - 1;
                if slot >= slot_count || db.active_id_at_slot(slot).is_none() {
                    continue;
                }
                if next_frontier.len() >= config.max_frontier_size {
                    return Err(TriviumError::QueryExecution(
                        "图扩展前沿超过预算 (Graph expansion frontier budget exceeded)".into(),
                    ));
                }
                if visited_count + next_frontier.len() >= config.max_visited_nodes {
                    return Err(TriviumError::QueryExecution(
                        "图扩展访问节点数超过预算 (Graph expansion visited-node budget exceeded)"
                            .into(),
                    ));
                }
                next_frontier.push(slot);
            }
        }
        visited_count = visited_count.saturating_add(next_frontier.len());
        for &slot in &next_frontier {
            set_bit(&mut visited, slot);
            if next_depth >= config.min_depth {
                if results.len() >= config.max_results {
                    return Err(TriviumError::QueryExecution(
                        "图扩展结果数超过预算 (Graph expansion result budget exceeded)".into(),
                    ));
                }
                results.push(ReachabilityHit {
                    target_id: db.active_id_at_slot(slot).expect("前沿槽位必须活跃"),
                    depth: next_depth,
                });
            }
        }
        if next_frontier.is_empty() {
            break;
        }
        frontier = next_frontier;
    }
    results.sort_unstable_by_key(|hit| (hit.depth, hit.target_id));
    Ok(CompactReachabilityOutput {
        results,
        visited_nodes: visited_count,
        traversed_edges,
    })
}

fn collect_targets<T: VectorType>(
    db: &MemTable<T>,
    current: NodeId,
    config: &ReachabilityConfig,
) -> Vec<NodeId> {
    let mut targets = Vec::new();
    if config.direction != ReachabilityDirection::Incoming
        && let Some(edges) = db.get_edges(current)
    {
        targets.extend(
            edges
                .iter()
                .filter(|edge| allowed(&edge.label, config.labels.as_deref()))
                .map(|edge| edge.target_id),
        );
    }
    if config.direction != ReachabilityDirection::Outgoing {
        targets.extend(
            db.get_incoming_edges(current, None)
                .into_iter()
                .filter(|edge| allowed(&edge.label, config.labels.as_deref()))
                .map(|edge| edge.source_id),
        );
    }
    targets.sort_unstable();
    targets.dedup();
    targets
}

pub fn query_subgraph<T: VectorType>(
    db: &MemTable<T>,
    source_id: NodeId,
    config: &ReachabilityConfig,
) -> Result<SubgraphResult> {
    let output = traverse_detailed(db, source_id, config)?;
    let mut ids = BTreeSet::from([source_id]);
    let mut edges = BTreeMap::new();
    for result in &output.results {
        ids.extend(result.path.iter().copied());
        for step in &result.steps {
            edges
                .entry((step.edge_source, step.edge_target, step.label.clone()))
                .or_insert(SubgraphEdge {
                    source_id: step.edge_source,
                    target_id: step.edge_target,
                    label: step.label.clone(),
                    weight: step.weight,
                    metadata: step.metadata.clone(),
                });
        }
    }
    let nodes = ids
        .into_iter()
        .filter_map(|id| {
            db.get_payload(id).map(|payload| SubgraphNode {
                id,
                payload: (*payload).clone(),
            })
        })
        .collect();
    Ok(SubgraphResult {
        nodes,
        edges: edges.into_values().collect(),
        visited_nodes: output.visited_nodes,
        traversed_edges: output.traversed_edges,
        truncated: output.truncated,
    })
}
fn validate<T: VectorType>(
    db: &MemTable<T>,
    source_id: NodeId,
    c: &ReachabilityConfig,
) -> Result<()> {
    if !db.contains(source_id) {
        return Err(TriviumError::NodeNotFound(source_id));
    }
    if c.min_depth > c.max_depth {
        return Err(TriviumError::InvalidInput(
            "Reachability min_depth 不能大于 max_depth".into(),
        ));
    }
    if c.max_visited_nodes == 0
        || c.max_results == 0
        || c.max_edges == 0
        || c.max_frontier_size == 0
    {
        return Err(TriviumError::InvalidInput(
            "Reachability 查询预算必须大于 0".into(),
        ));
    }
    Ok(())
}
fn collect_steps<T: VectorType>(
    db: &MemTable<T>,
    current: NodeId,
    c: &ReachabilityConfig,
) -> Vec<ReachabilityStep> {
    let mut steps = vec![];
    if c.direction != ReachabilityDirection::Incoming
        && let Some(edges) = db.get_edges(current)
    {
        for edge in edges {
            if allowed(&edge.label, c.labels.as_deref()) {
                steps.push(ReachabilityStep {
                    from: current,
                    to: edge.target_id,
                    edge_source: current,
                    edge_target: edge.target_id,
                    label: edge.label.clone(),
                    weight: edge.weight,
                    metadata: edge.metadata.clone(),
                })
            }
        }
    }
    if c.direction != ReachabilityDirection::Outgoing {
        for edge in db.get_incoming_edges(current, None) {
            if allowed(&edge.label, c.labels.as_deref()) {
                steps.push(ReachabilityStep {
                    from: current,
                    to: edge.source_id,
                    edge_source: edge.source_id,
                    edge_target: current,
                    label: edge.label,
                    weight: edge.weight,
                    metadata: edge.metadata,
                })
            }
        }
    }
    steps.sort_by(|a, b| {
        (a.to, a.label.as_str(), a.edge_source, a.edge_target).cmp(&(
            b.to,
            b.label.as_str(),
            b.edge_source,
            b.edge_target,
        ))
    });
    steps.dedup_by(|a, b| {
        a.to == b.to
            && a.label == b.label
            && a.edge_source == b.edge_source
            && a.edge_target == b.edge_target
    });
    steps
}
fn allowed(label: &str, labels: Option<&[String]>) -> bool {
    labels.is_none_or(|items| items.iter().any(|item| item == label))
}
