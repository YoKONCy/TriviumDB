//! MATCH 的显式栈遍历：按物理边保留行重数，并在节点、路径和邻边扫描处检查预算。
use super::{
    EdgeDirection, QueryControl, TqlPattern, TqlResult, TriviumError, VectorType,
    eval_predicate_env, matches_filter_with_id,
};
use crate::node::{Edge, NodeId};
use crate::storage::memtable::MemTable;
use std::collections::{HashMap, HashSet};

pub(super) struct WorkBudget<'a> {
    used: usize,
    limit: usize,
    control: Option<&'a QueryControl>,
}

impl<'a> WorkBudget<'a> {
    pub(super) fn new(limit: usize, control: Option<&'a QueryControl>) -> Self {
        Self {
            used: 0,
            limit,
            control,
        }
    }

    fn step(&mut self) -> Result<(), TriviumError> {
        if let Some(control) = self.control {
            control.check()?;
        }
        if self.used >= self.limit {
            return Err(TriviumError::QueryExecution(format!(
                "MATCH 遍历超过 {} 步预算 (MATCH traversal exceeded its {}-step budget)",
                self.limit, self.limit
            )));
        }
        self.used += 1;
        Ok(())
    }
}

/// 入边目录只枚举来源，随后扫描实际边；不同标签的平行边各产生一次结果。
pub(super) struct Neighbors<'a> {
    current: NodeId,
    direction: EdgeDirection,
    outgoing: std::slice::Iter<'a, Edge>,
    sources: std::slice::Iter<'a, NodeId>,
    incoming: std::slice::Iter<'a, Edge>,
    source: NodeId,
}

impl<'a> Neighbors<'a> {
    pub(super) fn new<T: VectorType>(
        mt: &'a MemTable<T>,
        current: NodeId,
        direction: EdgeDirection,
    ) -> Self {
        Self {
            current,
            direction,
            outgoing: if direction == EdgeDirection::Backward {
                &[]
            } else {
                mt.get_edges(current).unwrap_or(&[])
            }
            .iter(),
            sources: if direction == EdgeDirection::Forward {
                &[]
            } else {
                mt.get_incoming_sources(current)
            }
            .iter(),
            incoming: [].iter(),
            source: current,
        }
    }

    pub(super) fn next<T: VectorType>(
        &mut self,
        mt: &'a MemTable<T>,
        labels: &[String],
        budget: &mut WorkBudget<'_>,
    ) -> Result<Option<(NodeId, &'a Edge)>, TriviumError> {
        for edge in self.outgoing.by_ref() {
            budget.step()?;
            if labels.is_empty() || labels.contains(&edge.label) {
                return Ok(Some((self.current, edge)));
            }
        }
        loop {
            for edge in self.incoming.by_ref() {
                budget.step()?;
                if edge.target_id == self.current
                    && (labels.is_empty() || labels.contains(&edge.label))
                {
                    return Ok(Some((self.source, edge)));
                }
            }
            let Some(&source) = self.sources.next() else {
                return Ok(None);
            };
            budget.step()?;
            // 双向枚举中的自环已经从出边产生；反向单独查询仍保留该物理边。
            if source == self.current && self.direction == EdgeDirection::Both {
                continue;
            }
            self.source = source;
            self.incoming = mt.get_edges(source).unwrap_or(&[]).iter();
        }
    }
}

struct HopFrame<'a> {
    node: NodeId,
    emit: bool,
    neighbors: Neighbors<'a>,
}

struct PathWalker<'a> {
    frames: Vec<HopFrame<'a>>,
    visited: HashSet<NodeId>,
    min: usize,
    max: usize,
}

impl<'a> PathWalker<'a> {
    fn new<T: VectorType>(mt: &'a MemTable<T>, node: NodeId, edge: &super::TqlEdgePattern) -> Self {
        let range = edge.hop_range.unwrap_or(super::HopRange { min: 1, max: 1 });
        Self {
            frames: vec![HopFrame {
                node,
                emit: true,
                neighbors: Neighbors::new(mt, node, edge.direction),
            }],
            visited: HashSet::from([node]),
            min: range.min,
            max: range.max,
        }
    }

    fn next<T: VectorType>(
        &mut self,
        mt: &'a MemTable<T>,
        edge: &super::TqlEdgePattern,
        budget: &mut WorkBudget<'_>,
    ) -> Result<Option<NodeId>, TriviumError> {
        loop {
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::TqlPathStep);
            budget.step()?;
            let depth = self.frames.len().saturating_sub(1);
            let Some(frame) = self.frames.last_mut() else {
                return Ok(None);
            };
            if frame.emit {
                frame.emit = false;
                if depth >= self.min {
                    return Ok(Some(frame.node));
                }
            }
            if depth < self.max
                && let Some((source, next)) = frame.neighbors.next(mt, &edge.labels, budget)?
            {
                let next = if source == frame.node {
                    next.target_id
                } else {
                    source
                };
                if self.visited.insert(next) {
                    self.frames.push(HopFrame {
                        node: next,
                        emit: true,
                        neighbors: Neighbors::new(mt, next, edge.direction),
                    });
                }
                continue;
            }
            if let Some(frame) = self.frames.pop() {
                self.visited.remove(&frame.node);
            }
        }
    }
}

enum Frame<'a> {
    Visit {
        layer: usize,
        node: NodeId,
    },
    Restore {
        slot: usize,
        previous: Option<NodeId>,
    },
    Single {
        layer: usize,
        current: NodeId,
        neighbors: Neighbors<'a>,
    },
    Variable {
        layer: usize,
        walker: PathWalker<'a>,
    },
}

/// 同时限制模式回溯栈、可变长路径帧和 visited 哈希表的保守峰值；预留后再展开。
pub(super) fn workspace_bytes(pattern: &TqlPattern, nodes: usize) -> Result<usize, TriviumError> {
    let overflow = || TriviumError::CapacityAllocationFailed {
        reason: "MATCH 工作区估算溢出 (MATCH workspace estimate overflowed)".into(),
    };
    let mut bytes = pattern
        .nodes
        .len()
        .checked_mul(std::mem::size_of::<Frame<'_>>() * 4)
        .ok_or_else(overflow)?;
    for edge in &pattern.edges {
        if let Some(range) = edge.hop_range {
            let depth = range
                .max
                .min(nodes.saturating_sub(1))
                .checked_add(1)
                .ok_or_else(overflow)?;
            let extra = depth
                .checked_mul((std::mem::size_of::<HopFrame<'_>>() + 64) * 4)
                .ok_or_else(overflow)?;
            bytes = bytes.checked_add(extra).ok_or_else(overflow)?;
        }
    }
    Ok(bytes)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn walk<T: VectorType>(
    mt: &MemTable<T>,
    pattern: &TqlPattern,
    predicate: Option<&super::Predicate>,
    return_vars: &[String],
    var_map: &HashMap<String, usize>,
    env: &mut [Option<NodeId>],
    start: NodeId,
    results: &mut TqlResult<T>,
    budget: &mut WorkBudget<'_>,
    row_limit: usize,
    memory: &super::QueryMemory,
) -> Result<bool, TriviumError> {
    let mut frames = vec![Frame::Visit {
        layer: 0,
        node: start,
    }];
    while let Some(frame) = frames.pop() {
        budget.step()?;
        match frame {
            Frame::Restore { slot, previous } => env[slot] = previous,
            Frame::Single {
                layer,
                current,
                mut neighbors,
            } => {
                if let Some((source, edge)) =
                    neighbors.next(mt, &pattern.edges[layer].labels, budget)?
                {
                    let next = if source == current {
                        edge.target_id
                    } else {
                        source
                    };
                    frames.push(Frame::Single {
                        layer,
                        current,
                        neighbors,
                    });
                    frames.push(Frame::Visit {
                        layer: layer + 1,
                        node: next,
                    });
                }
            }
            Frame::Variable { layer, mut walker } => {
                if let Some(next) = walker.next(mt, &pattern.edges[layer], budget)? {
                    frames.push(Frame::Variable { layer, walker });
                    frames.push(Frame::Visit {
                        layer: layer + 1,
                        node: next,
                    });
                }
            }
            Frame::Visit { layer, node } => {
                let node_pat = &pattern.nodes[layer];
                if layer > 0
                    && node_pat
                        .filter
                        .as_ref()
                        .is_some_and(|filter| !matches_filter_with_id(filter, node, mt))
                {
                    continue;
                }
                if let Some(var) = &node_pat.var {
                    let slot = var_map[var];
                    let previous = env[slot];
                    if previous.is_some_and(|bound| bound != node) {
                        continue;
                    }
                    env[slot] = Some(node);
                    frames.push(Frame::Restore { slot, previous });
                }
                if layer == pattern.edges.len() {
                    if predicate.is_none_or(|pred| eval_predicate_env(pred, env, var_map, mt)) {
                        let mut row = HashMap::new();
                        for var in return_vars {
                            if let Some(&slot) = var_map.get(var)
                                && let Some(id) = env[slot]
                                && let node = memory.snapshot(id, mt)?
                            {
                                row.insert(var.clone(), node);
                            }
                        }
                        results.push(row);
                        if results.len() >= row_limit {
                            return Ok(false);
                        }
                    }
                } else {
                    let edge = &pattern.edges[layer];
                    frames.push(if edge.hop_range.is_some() {
                        Frame::Variable {
                            layer,
                            walker: PathWalker::new(mt, node, edge),
                        }
                    } else {
                        Frame::Single {
                            layer,
                            current: node,
                            neighbors: Neighbors::new(mt, node, edge.direction),
                        }
                    });
                }
            }
        }
    }
    Ok(true)
}
