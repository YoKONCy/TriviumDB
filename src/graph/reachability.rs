use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use std::collections::{HashMap, VecDeque};

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
}

impl Default for ReachabilityConfig {
    fn default() -> Self {
        Self {
            min_depth: 1,
            max_depth: 1,
            labels: None,
            direction: ReachabilityDirection::Outgoing,
            max_visited_nodes: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachabilityStep {
    pub from: NodeId,
    pub to: NodeId,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachabilityResult {
    pub source_id: NodeId,
    pub target_id: NodeId,
    pub depth: usize,
    pub path: Vec<NodeId>,
    pub steps: Vec<ReachabilityStep>,
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
    validate(db, source_id, config)?;
    let mut queue = VecDeque::from([PathState {
        node: source_id,
        path: vec![source_id],
        steps: Vec::new(),
    }]);
    let mut best_depth = HashMap::from([(source_id, 0usize)]);
    let mut results = Vec::new();

    if config.min_depth == 0 {
        results.push(ReachabilityResult {
            source_id,
            target_id: source_id,
            depth: 0,
            path: vec![source_id],
            steps: Vec::new(),
        });
    }

    while let Some(state) = queue.pop_front() {
        let depth = state.steps.len();
        if depth >= config.max_depth {
            continue;
        }
        for step in collect_steps(db, state.node, config) {
            let next_depth = depth + 1;
            if best_depth
                .get(&step.to)
                .is_some_and(|known| *known <= next_depth)
            {
                continue;
            }
            if best_depth.len() >= config.max_visited_nodes {
                return Err(TriviumError::QueryExecution(format!(
                    "Reachability 超过最大访问节点预算 {}",
                    config.max_visited_nodes
                )));
            }
            best_depth.insert(step.to, next_depth);
            let mut path = state.path.clone();
            path.push(step.to);
            let mut steps = state.steps.clone();
            steps.push(step.clone());
            if next_depth >= config.min_depth {
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
    results.sort_by(|left, right| {
        (left.depth, left.target_id, &left.path).cmp(&(right.depth, right.target_id, &right.path))
    });
    Ok(results)
}

fn validate<T: VectorType>(
    db: &MemTable<T>,
    source_id: NodeId,
    config: &ReachabilityConfig,
) -> Result<()> {
    if !db.contains(source_id) {
        return Err(TriviumError::NodeNotFound(source_id));
    }
    if config.min_depth > config.max_depth {
        return Err(TriviumError::InvalidInput(
            "Reachability min_depth 不能大于 max_depth".into(),
        ));
    }
    if config.max_visited_nodes == 0 {
        return Err(TriviumError::InvalidInput(
            "Reachability max_visited_nodes 必须大于 0".into(),
        ));
    }
    Ok(())
}

fn collect_steps<T: VectorType>(
    db: &MemTable<T>,
    current: NodeId,
    config: &ReachabilityConfig,
) -> Vec<ReachabilityStep> {
    let mut steps = Vec::new();
    if config.direction != ReachabilityDirection::Incoming
        && let Some(edges) = db.get_edges(current)
    {
        for edge in edges {
            if label_allowed(&edge.label, config.labels.as_deref()) {
                steps.push(ReachabilityStep {
                    from: current,
                    to: edge.target_id,
                    label: edge.label.clone(),
                });
            }
        }
    }
    if config.direction != ReachabilityDirection::Outgoing {
        for edge in db.get_incoming_edges(current, None) {
            if label_allowed(&edge.label, config.labels.as_deref()) {
                steps.push(ReachabilityStep {
                    from: current,
                    to: edge.source_id,
                    label: edge.label,
                });
            }
        }
    }
    steps.sort_by(|left, right| {
        (left.to, left.label.as_str(), left.from).cmp(&(right.to, right.label.as_str(), right.from))
    });
    steps.dedup_by(|left, right| left.to == right.to && left.label == right.label);
    steps
}

fn label_allowed(label: &str, labels: Option<&[String]>) -> bool {
    labels.is_none_or(|allowed| allowed.iter().any(|item| item == label))
}
