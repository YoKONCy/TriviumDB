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
struct PathState {
    node: NodeId,
    path: Vec<NodeId>,
    steps: Vec<ReachabilityStep>,
    heap_bytes: usize,
    _workspace: crate::resources::WorkspaceReservation,
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
    Ok(traverse_leased(db, source_id, config)?.0)
}

fn traverse_leased<T: VectorType>(
    db: &MemTable<T>,
    source_id: NodeId,
    config: &ReachabilityConfig,
) -> Result<(
    ReachabilityOutput,
    Vec<crate::resources::WorkspaceReservation>,
    crate::resources::WorkspaceReservation,
)> {
    validate(db, source_id, config)?;
    let mut base = db
        .resource_runtime()
        .reserve_workspace(state_bytes(1, true, 0, 1)?)?;
    let mut result_leases = Vec::new();
    let mut queue = VecDeque::from([PathState {
        node: source_id,
        path: vec![source_id],
        steps: vec![],
        heap_bytes: 16,
        _workspace: db.resource_runtime().reserve_workspace(16)?,
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
        result_leases.push(db.resource_runtime().reserve_workspace(16)?);
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
        let (neighbors, _neighbor_lease) = collect_steps(db, state.node, config)?;
        for step in neighbors {
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
            base.resize(state_bytes(best_depth.len().saturating_add(1), true, 0, 1)?)?;
            best_depth.insert(step.to, next_depth);
            depth_reached = depth_reached.max(next_depth);
            let added = std::mem::size_of::<NodeId>()
                .saturating_add(std::mem::size_of::<ReachabilityStep>())
                .saturating_add(step.edge.label.len())
                .saturating_add(crate::storage::payload_store::query_clone_peak(
                    &step.edge.metadata,
                ));
            let heap_bytes = state
                .heap_bytes
                .checked_add(added.checked_mul(2).ok_or_else(workspace_overflow)?)
                .ok_or_else(workspace_overflow)?;
            let workspace = db.resource_runtime().reserve_workspace(heap_bytes)?;
            let mut path = state.path.clone();
            path.push(step.to);
            let mut steps = state.steps.clone();
            steps.push(ReachabilityStep {
                from: step.from,
                to: step.to,
                edge_source: step.edge_source,
                edge_target: step.edge.target_id,
                label: step.edge.label.clone(),
                weight: step.edge.weight,
                metadata: step.edge.metadata.clone(),
            });
            if next_depth >= config.min_depth {
                if results.len().saturating_add(1) > config.max_results {
                    truncated = true;
                    break 'walk;
                }
                result_leases.push(db.resource_runtime().reserve_workspace(heap_bytes)?);
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
                heap_bytes,
                _workspace: workspace,
            });
            peak_frontier_size = peak_frontier_size.max(queue.len());
        }
    }
    results.sort_by(|a, b| (a.depth, a.target_id, &a.path).cmp(&(b.depth, b.target_id, &b.path)));
    Ok((
        ReachabilityOutput {
            results,
            visited_nodes: best_depth.len(),
            traversed_edges,
            peak_frontier_size,
            depth_reached,
            truncated,
        },
        result_leases,
        base,
    ))
}
/// 仅返回节点与最短深度，避免无目标扩展为每个命中复制完整路径与边元数据。
pub fn traverse_compact<T: VectorType>(
    db: &MemTable<T>,
    source_id: NodeId,
    config: &ReachabilityConfig,
) -> Result<CompactReachabilityOutput> {
    validate(db, source_id, config)?;
    let mut workspace = db
        .resource_runtime()
        .reserve_workspace(state_bytes(1, false, 0, 1)?)?;
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
        let (targets, _targets_lease) = collect_targets(db, current, config)?;
        for target in targets {
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
            workspace.resize(state_bytes(
                best_depth.len().saturating_add(1),
                false,
                0,
                1,
            )?)?;
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
) -> Result<usize> {
    let (targets, _targets_lease) = collect_targets(db, current, config)?;
    for target in &targets {
        if let Some(slot) = db.internal_slot_of(*target) {
            set_bit(words, slot);
        }
    }
    Ok(targets.len())
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
    let mut workspace = db
        .resource_runtime()
        .reserve_workspace(state_bytes(1, false, word_count, 1)?)?;
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
        workspace.resize(state_bytes(visited_count, false, word_count, workers)?)?;
        let partials = if frontier.len() < 1_024 || workers == 1 {
            let mut bitmap = vec![0u64; word_count];
            let mut edges = 0usize;
            for &slot in &frontier {
                if let Some(id) = db.active_id_at_slot(slot) {
                    edges =
                        edges.saturating_add(collect_target_slots(db, id, config, &mut bitmap)?);
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
                            )?);
                        }
                    }
                    Ok((bitmap, edges))
                })
                .collect::<Result<Vec<_>>>()?
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
                workspace.resize(state_bytes(
                    visited_count
                        .saturating_add(next_frontier.len())
                        .saturating_add(1),
                    false,
                    word_count,
                    workers,
                )?)?;
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
) -> Result<(Vec<NodeId>, crate::resources::WorkspaceReservation)> {
    let (steps, mut lease) = collect_steps(db, current, config)?;
    let bytes = steps
        .len()
        .max(4)
        .checked_mul((std::mem::size_of::<BorrowedStep<'_>>() + std::mem::size_of::<NodeId>()) * 2)
        .ok_or_else(workspace_overflow)?;
    lease.resize(bytes)?;
    let mut targets = steps.iter().map(|step| step.to).collect::<Vec<_>>();
    targets.sort_unstable();
    targets.dedup();
    Ok((targets, lease))
}

pub fn query_subgraph<T: VectorType>(
    db: &MemTable<T>,
    source_id: NodeId,
    config: &ReachabilityConfig,
) -> Result<SubgraphResult> {
    let (output, _result_leases, _base) = traverse_leased(db, source_id, config)?;
    let mut ids = BTreeSet::from([source_id]);
    let mut edges = BTreeMap::new();
    for result in &output.results {
        ids.extend(result.path.iter().copied());
        for step in &result.steps {
            edges
                .entry((step.edge_source, step.edge_target, step.label.as_str()))
                .or_insert(step);
        }
    }
    let bytes = ids.iter().try_fold(0usize, |sum, &id| {
        sum.checked_add(db.payload_query_peak(id)?)
            .ok_or_else(workspace_overflow)
    })?;
    let bytes = edges.values().try_fold(bytes, |sum, step| {
        sum.checked_add(step.label.len())
            .and_then(|bytes| {
                bytes.checked_add(crate::storage::payload_store::query_clone_peak(
                    &step.metadata,
                ))
            })
            .ok_or_else(workspace_overflow)
    })?;
    let _materialization = db.resource_runtime().reserve_workspace(bytes)?;
    let nodes = ids
        .into_iter()
        .map(|id| {
            let payload = db
                .try_get_payload(id)?
                .ok_or(TriviumError::NodeNotFound(id))?;
            Ok(SubgraphNode {
                id,
                payload: (*payload).clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let edges = edges
        .into_values()
        .map(|step| SubgraphEdge {
            source_id: step.edge_source,
            target_id: step.edge_target,
            label: step.label.clone(),
            weight: step.weight,
            metadata: step.metadata.clone(),
        })
        .collect();
    Ok(SubgraphResult {
        nodes,
        edges,
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
    db.ensure_readable()?;
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
struct BorrowedStep<'a> {
    from: NodeId,
    to: NodeId,
    edge_source: NodeId,
    edge: &'a crate::node::Edge,
}

fn workspace_overflow() -> TriviumError {
    TriviumError::CapacityAllocationFailed {
        reason: "图查询工作区估算溢出 (Graph query workspace estimate overflowed)".into(),
    }
}

fn state_bytes(nodes: usize, detailed: bool, bitmap_words: usize, workers: usize) -> Result<usize> {
    // 状态按实际探索量增长，容器初次分配和几何扩容预留余量；JSON 与路径单独记账。
    let bytes = nodes
        .max(4)
        .checked_mul(if detailed { 512 } else { 192 })
        .and_then(|bytes| bytes.checked_add(256))
        .ok_or_else(workspace_overflow)?;
    let bitmaps = bitmap_words
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_mul(workers.saturating_add(2)))
        .ok_or_else(workspace_overflow)?;
    bytes.checked_add(bitmaps).ok_or_else(workspace_overflow)
}

fn collect_steps<'a, T: VectorType>(
    db: &'a MemTable<T>,
    current: NodeId,
    c: &ReachabilityConfig,
) -> Result<(
    Vec<BorrowedStep<'a>>,
    crate::resources::WorkspaceReservation,
)> {
    let mut steps = Vec::new();
    let mut lease = db.resource_runtime().reserve_workspace(0)?;
    let mut add = |source, edge: &'a crate::node::Edge| -> Result<()> {
        if !allowed(&edge.label, c.labels.as_deref()) {
            return Ok(());
        }
        let bytes = steps
            .len()
            .checked_add(1)
            .and_then(|len| {
                len.max(4)
                    .checked_mul(std::mem::size_of::<BorrowedStep<'_>>() * 2)
            })
            .ok_or_else(workspace_overflow)?;
        lease.resize(bytes)?;
        steps.push(BorrowedStep {
            from: current,
            to: if source == current {
                edge.target_id
            } else {
                source
            },
            edge_source: source,
            edge,
        });
        Ok(())
    };
    if c.direction != ReachabilityDirection::Incoming {
        for edge in db.get_edges(current).unwrap_or(&[]) {
            add(current, edge)?;
        }
    }
    if c.direction != ReachabilityDirection::Outgoing {
        for &source in db.get_incoming_sources(current) {
            for edge in db.get_edges(source).unwrap_or(&[]) {
                if edge.target_id == current {
                    add(source, edge)?;
                }
            }
        }
    }
    steps.sort_unstable_by(|a, b| {
        (a.to, a.edge.label.as_str(), a.edge_source, a.edge.target_id).cmp(&(
            b.to,
            b.edge.label.as_str(),
            b.edge_source,
            b.edge.target_id,
        ))
    });
    steps.dedup_by(|a, b| {
        a.to == b.to
            && a.edge.label == b.edge.label
            && a.edge_source == b.edge_source
            && a.edge.target_id == b.edge.target_id
    });
    Ok((steps, lease))
}
fn allowed(label: &str, labels: Option<&[String]>) -> bool {
    labels.is_none_or(|items| items.iter().any(|item| item == label))
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use crate::{ResourceLimits, ResourceRuntime};
    use serde_json::json;

    #[test]
    fn 所有可达性物理入口拒绝零共享余额并准确归还工作区() {
        let runtime = ResourceRuntime::new(ResourceLimits {
            memory_limit: 16 * 1024 * 1024,
            maintenance_memory_limit: 2 * 1024 * 1024,
            max_threads: 1,
            ..Default::default()
        })
        .unwrap();
        let mut db = MemTable::<f32>::new(1);
        for id in 1..=2048 {
            db.insert_with_id(id, &[1.], json!({})).unwrap();
        }
        db.upsert_edge(1, 2, "rel".into(), 1., json!({"data":"x".repeat(64*1024)}))
            .unwrap();
        db.attach_resources(
            runtime.clone(),
            runtime.reserve_database(14 * 1024 * 1024).unwrap(),
        );
        let config = ReachabilityConfig::default();
        for result in [
            traverse_compact(&db, 1, &config),
            traverse_compact_parallel(&db, 1, &config),
        ] {
            assert!(matches!(
                result,
                Err(TriviumError::ResourceBudgetExceeded { .. })
            ));
        }
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        db.resize_resource_reservation(2 * 1024 * 1024).unwrap();
        for result in [
            traverse_compact(&db, 1, &config),
            traverse_compact_parallel(&db, 1, &config),
        ] {
            assert_eq!(
                result.unwrap().results,
                [ReachabilityHit {
                    target_id: 2,
                    depth: 1
                }]
            );
            assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        }
        // 余额足以开始 BFS，但不足以复制 Metadata；返回部分结果策略也要传播内存拒绝。
        db.resize_resource_reservation(14 * 1024 * 1024 - 8192)
            .unwrap();
        let partial = ReachabilityConfig {
            exhaustion_policy: BudgetExhaustionPolicy::Partial,
            ..config
        };
        assert!(matches!(
            traverse_detailed(&db, 1, &partial),
            Err(TriviumError::ResourceBudgetExceeded { .. })
        ));
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        db.resize_resource_reservation(2 * 1024 * 1024).unwrap();
        let output = query_subgraph(&db, 1, &partial).unwrap();
        assert_eq!(
            output.nodes.iter().map(|node| node.id).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(output.edges.len(), 1);
        assert_eq!(
            output.edges[0].metadata,
            json!({"data":"x".repeat(64*1024)})
        );
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }
}
