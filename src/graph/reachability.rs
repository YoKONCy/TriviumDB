use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
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
    pub truncated: bool,
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
    let mut truncated = false;
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
                truncated = true;
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
                truncated = true;
                break 'walk;
            }
            best_depth.insert(step.to, next_depth);
            let mut path = state.path.clone();
            path.push(step.to);
            let mut steps = state.steps.clone();
            steps.push(step.clone());
            if next_depth >= config.min_depth {
                if results.len() >= config.max_results {
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
            queue.push_back(PathState {
                node: step.to,
                path,
                steps,
            });
        }
    }
    results.sort_by(|a, b| (a.depth, a.target_id, &a.path).cmp(&(b.depth, b.target_id, &b.path)));
    Ok(ReachabilityOutput {
        results,
        visited_nodes: best_depth.len(),
        traversed_edges,
        truncated,
    })
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
            db.get_payload(id)
                .cloned()
                .map(|payload| SubgraphNode { id, payload })
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
    if c.max_visited_nodes == 0 || c.max_results == 0 || c.max_edges == 0 {
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
    if c.direction != ReachabilityDirection::Incoming {
        if let Some(edges) = db.get_edges(current) {
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
