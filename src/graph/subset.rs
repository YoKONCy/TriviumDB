//! 面向查询管线的确定性子集图算法。
//!
//! 这些实现只在调用方显式提供的节点集合诱导子图上运行，不会静默扫描集合外节点。

use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphAlgorithmQuality {
    Exact,
    DeterministicApproximate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphAlgorithmSemantics {
    pub name: &'static str,
    pub quality: GraphAlgorithmQuality,
    pub directed: bool,
    pub weighted: bool,
    pub induced_subgraph: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SubsetPageRankConfig {
    pub damping: f64,
    pub max_iterations: usize,
    pub tolerance: f64,
}

impl Default for SubsetPageRankConfig {
    fn default() -> Self {
        Self {
            damping: 0.85,
            max_iterations: 100,
            tolerance: 1e-6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubsetPageRankResult {
    pub scores: Vec<(NodeId, f64)>,
    pub iterations: usize,
    pub residual_l1: f64,
    pub converged: bool,
    pub examined_edges: usize,
}

pub fn subset_pagerank<T: VectorType>(
    mt: &MemTable<T>,
    nodes: &BTreeSet<NodeId>,
    config: SubsetPageRankConfig,
    label_filter: Option<&str>,
    max_examined_edges: usize,
) -> Result<SubsetPageRankResult> {
    if !(0.0..1.0).contains(&config.damping)
        || !config.tolerance.is_finite()
        || config.tolerance <= 0.0
        || config.max_iterations == 0
    {
        return Err(TriviumError::QueryExecution(
            "PageRank 配置非法：damping 必须位于 [0,1)，迭代次数和 tolerance 必须为正 (Invalid PageRank configuration)".into(),
        ));
    }
    if nodes.is_empty() {
        return Ok(SubsetPageRankResult {
            scores: Vec::new(),
            iterations: 0,
            residual_l1: 0.0,
            converged: true,
            examined_edges: 0,
        });
    }

    let ids = nodes.iter().copied().collect::<Vec<_>>();
    let mut outgoing = BTreeMap::<NodeId, Vec<NodeId>>::new();
    let mut examined_edges = 0usize;
    for &source in &ids {
        let mut targets = Vec::new();
        for edge in mt.get_edges(source).unwrap_or_default() {
            examined_edges = examined_edges.saturating_add(1);
            if examined_edges > max_examined_edges {
                return Err(TriviumError::QueryExecution(
                    "PageRank 超过边访问预算 (PageRank exceeds edge examination budget)".into(),
                ));
            }
            if nodes.contains(&edge.target_id)
                && label_filter.is_none_or(|label| edge.label == label)
            {
                targets.push(edge.target_id);
            }
        }
        targets.sort_unstable();
        targets.dedup();
        outgoing.insert(source, targets);
    }

    let n = ids.len();
    let initial = 1.0 / n as f64;
    let mut scores = BTreeMap::from_iter(ids.iter().map(|&id| (id, initial)));
    let teleport = (1.0 - config.damping) / n as f64;
    let mut residual_l1 = 0.0;
    let mut iterations = 0usize;
    let mut converged = false;
    for iteration in 1..=config.max_iterations {
        let dangling = ids
            .iter()
            .filter(|id| outgoing[id].is_empty())
            .map(|id| scores[id])
            .sum::<f64>();
        let base = teleport + config.damping * dangling / n as f64;
        let mut next = BTreeMap::from_iter(ids.iter().map(|&id| (id, base)));
        for &source in &ids {
            let targets = &outgoing[&source];
            if targets.is_empty() {
                continue;
            }
            let contribution = config.damping * scores[&source] / targets.len() as f64;
            for target in targets {
                *next.get_mut(target).expect("目标属于诱导子图") += contribution;
            }
        }
        residual_l1 = ids.iter().map(|id| (next[id] - scores[id]).abs()).sum();
        scores = next;
        iterations = iteration;
        if residual_l1 < config.tolerance {
            converged = true;
            break;
        }
    }
    let mut scores = scores.into_iter().collect::<Vec<_>>();
    scores.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    Ok(SubsetPageRankResult {
        scores,
        iterations,
        residual_l1,
        converged,
        examined_edges,
    })
}

pub fn subset_pagerank_parallel<T: VectorType>(
    mt: &MemTable<T>,
    nodes: &BTreeSet<NodeId>,
    config: SubsetPageRankConfig,
    label_filter: Option<&str>,
    max_examined_edges: usize,
) -> Result<SubsetPageRankResult> {
    use rayon::prelude::*;
    if !(0.0..1.0).contains(&config.damping)
        || !config.tolerance.is_finite()
        || config.tolerance <= 0.0
        || config.max_iterations == 0
    {
        return Err(TriviumError::QueryExecution(
            "PageRank 配置非法：damping 必须位于 [0,1)，迭代次数和 tolerance 必须为正 (Invalid PageRank configuration)".into(),
        ));
    }
    let ids = nodes.iter().copied().collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(SubsetPageRankResult {
            scores: Vec::new(),
            iterations: 0,
            residual_l1: 0.0,
            converged: true,
            examined_edges: 0,
        });
    }
    let slots = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect::<HashMap<_, _>>();
    let scanned = ids
        .par_iter()
        .map(|&source| {
            let edges = mt.get_edges(source).unwrap_or_default();
            let mut targets = edges
                .iter()
                .filter_map(|edge| {
                    (slots.contains_key(&edge.target_id)
                        && label_filter.is_none_or(|label| edge.label == label))
                    .then_some(slots[&edge.target_id])
                })
                .collect::<Vec<_>>();
            targets.sort_unstable();
            targets.dedup();
            (targets, edges.len())
        })
        .collect::<Vec<_>>();
    let examined_edges = scanned.iter().map(|(_, n)| *n).sum::<usize>();
    if examined_edges > max_examined_edges {
        return Err(TriviumError::QueryExecution(
            "PageRank 超过边访问预算 (PageRank exceeds edge examination budget)".into(),
        ));
    }
    let n = ids.len();
    let mut incoming = vec![Vec::<usize>::new(); n];
    for (source, (targets, _)) in scanned.iter().enumerate() {
        for &target in targets {
            incoming[target].push(source);
        }
    }
    let initial = 1.0 / n as f64;
    let teleport = (1.0 - config.damping) / n as f64;
    let mut scores = vec![initial; n];
    let mut residual_l1 = 0.0;
    let mut iterations = 0;
    let mut converged = false;
    for iteration in 1..=config.max_iterations {
        let dangling = (0..n)
            .filter(|&i| scanned[i].0.is_empty())
            .map(|i| scores[i])
            .sum::<f64>();
        let base = teleport + config.damping * dangling / n as f64;
        let next = incoming
            .par_iter()
            .map(|sources| {
                sources.iter().fold(base, |sum, &source| {
                    sum + config.damping * scores[source] / scanned[source].0.len() as f64
                })
            })
            .collect::<Vec<_>>();
        residual_l1 = next
            .par_iter()
            .zip(&scores)
            .map(|(a, b)| (a - b).abs())
            .sum();
        scores = next;
        iterations = iteration;
        if residual_l1 < config.tolerance {
            converged = true;
            break;
        }
    }
    let mut output = ids.into_iter().zip(scores).collect::<Vec<_>>();
    output.par_sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(SubsetPageRankResult {
        scores: output,
        iterations,
        residual_l1,
        converged,
        examined_edges,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubsetDegreeCentrality {
    pub id: NodeId,
    pub out_degree: usize,
    pub in_degree: usize,
    pub total_degree: usize,
    pub normalized: f64,
}

pub fn subset_degree_centrality<T: VectorType>(
    mt: &MemTable<T>,
    nodes: &BTreeSet<NodeId>,
    label_filter: Option<&str>,
    max_examined_edges: usize,
) -> Result<(Vec<SubsetDegreeCentrality>, usize)> {
    let mut in_degrees = BTreeMap::from_iter(nodes.iter().map(|&id| (id, 0usize)));
    let mut out_degrees = BTreeMap::from_iter(nodes.iter().map(|&id| (id, 0usize)));
    let mut examined = 0usize;
    for &source in nodes {
        let mut targets = BTreeSet::new();
        for edge in mt.get_edges(source).unwrap_or_default() {
            examined = examined.saturating_add(1);
            if examined > max_examined_edges {
                return Err(TriviumError::QueryExecution(
                    "度中心性超过边访问预算 (Degree centrality exceeds edge examination budget)"
                        .into(),
                ));
            }
            if nodes.contains(&edge.target_id)
                && label_filter.is_none_or(|label| edge.label == label)
            {
                targets.insert(edge.target_id);
            }
        }
        out_degrees.insert(source, targets.len());
        for target in targets {
            *in_degrees.get_mut(&target).expect("目标属于诱导子图") += 1;
        }
    }
    let denominator = nodes.len().saturating_sub(1).saturating_mul(2).max(1) as f64;
    let mut output = nodes
        .iter()
        .map(|&id| {
            let out_degree = out_degrees[&id];
            let in_degree = in_degrees[&id];
            let total_degree = out_degree + in_degree;
            SubsetDegreeCentrality {
                id,
                out_degree,
                in_degree,
                total_degree,
                normalized: total_degree as f64 / denominator,
            }
        })
        .collect::<Vec<_>>();
    output.sort_by(|left, right| {
        right
            .total_degree
            .cmp(&left.total_degree)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok((output, examined))
}

pub fn subset_degree_centrality_parallel<T: VectorType>(
    mt: &MemTable<T>,
    nodes: &BTreeSet<NodeId>,
    label_filter: Option<&str>,
    max_examined_edges: usize,
) -> Result<(Vec<SubsetDegreeCentrality>, usize)> {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let ids = nodes.iter().copied().collect::<Vec<_>>();
    let slots = ids
        .iter()
        .enumerate()
        .map(|(slot, &id)| (id, slot))
        .collect::<HashMap<_, _>>();
    let in_degrees = (0..ids.len())
        .map(|_| AtomicUsize::new(0))
        .collect::<Vec<_>>();
    let scanned = ids
        .par_iter()
        .map(|&source| {
            let edges = mt.get_edges(source).unwrap_or_default();
            let mut targets = edges
                .iter()
                .filter(|edge| {
                    slots.contains_key(&edge.target_id)
                        && label_filter.is_none_or(|label| edge.label == label)
                })
                .map(|edge| edge.target_id)
                .collect::<Vec<_>>();
            targets.sort_unstable();
            targets.dedup();
            for target in &targets {
                in_degrees[slots[target]].fetch_add(1, Ordering::Relaxed);
            }
            (targets.len(), edges.len())
        })
        .collect::<Vec<_>>();
    let examined = scanned
        .iter()
        .fold(0usize, |total, (_, count)| total.saturating_add(*count));
    if examined > max_examined_edges {
        return Err(TriviumError::QueryExecution(
            "度中心性超过边访问预算 (Degree centrality exceeds edge examination budget)".into(),
        ));
    }
    let denominator = nodes.len().saturating_sub(1).saturating_mul(2).max(1) as f64;
    let mut output = ids
        .into_par_iter()
        .enumerate()
        .map(|(slot, id)| {
            let out_degree = scanned[slot].0;
            let in_degree = in_degrees[slot].load(Ordering::Relaxed);
            let total_degree = out_degree + in_degree;
            SubsetDegreeCentrality {
                id,
                out_degree,
                in_degree,
                total_degree,
                normalized: total_degree as f64 / denominator,
            }
        })
        .collect::<Vec<_>>();
    output.par_sort_unstable_by(|left, right| {
        right
            .total_degree
            .cmp(&left.total_degree)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok((output, examined))
}

pub fn subset_wcc<T: VectorType>(
    mt: &MemTable<T>,
    nodes: &BTreeSet<NodeId>,
    label_filter: Option<&str>,
    max_examined_edges: usize,
) -> Result<(Vec<Vec<NodeId>>, usize)> {
    let mut adjacency = BTreeMap::from_iter(nodes.iter().map(|&id| (id, BTreeSet::new())));
    let mut examined = 0usize;
    for &source in nodes {
        for edge in mt.get_edges(source).unwrap_or_default() {
            examined = examined.saturating_add(1);
            if examined > max_examined_edges {
                return Err(TriviumError::QueryExecution(
                    "WCC 超过边访问预算 (WCC exceeds edge examination budget)".into(),
                ));
            }
            if nodes.contains(&edge.target_id)
                && label_filter.is_none_or(|label| edge.label == label)
            {
                adjacency
                    .get_mut(&source)
                    .expect("源属于诱导子图")
                    .insert(edge.target_id);
                adjacency
                    .get_mut(&edge.target_id)
                    .expect("目标属于诱导子图")
                    .insert(source);
            }
        }
    }
    let mut unseen = nodes.clone();
    let mut components = Vec::new();
    while let Some(&start) = unseen.first() {
        unseen.remove(&start);
        let mut queue = VecDeque::from([start]);
        let mut component = Vec::new();
        while let Some(current) = queue.pop_front() {
            component.push(current);
            for &neighbor in &adjacency[&current] {
                if unseen.remove(&neighbor) {
                    queue.push_back(neighbor);
                }
            }
        }
        component.sort_unstable();
        components.push(component);
    }
    components.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    Ok((components, examined))
}

pub fn subset_wcc_parallel<T: VectorType>(
    mt: &MemTable<T>,
    nodes: &BTreeSet<NodeId>,
    label_filter: Option<&str>,
    max_examined_edges: usize,
) -> Result<(Vec<Vec<NodeId>>, usize)> {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let ids = nodes.iter().copied().collect::<Vec<_>>();
    let slots = ids
        .iter()
        .enumerate()
        .map(|(slot, &id)| (id, slot))
        .collect::<HashMap<_, _>>();
    let scanned = ids
        .par_iter()
        .map(|&source| {
            let edges = mt.get_edges(source).unwrap_or_default();
            let source_slot = slots[&source];
            let mut targets = edges
                .iter()
                .filter_map(|edge| {
                    (label_filter.is_none_or(|label| edge.label == label))
                        .then(|| slots.get(&edge.target_id).copied())
                        .flatten()
                })
                .collect::<Vec<_>>();
            targets.sort_unstable();
            targets.dedup();
            (source_slot, targets, edges.len())
        })
        .collect::<Vec<_>>();
    let examined = scanned.iter().map(|(_, _, count)| *count).sum::<usize>();
    if examined > max_examined_edges {
        return Err(TriviumError::QueryExecution(
            "WCC 超过边访问预算 (WCC exceeds edge examination budget)".into(),
        ));
    }

    let parents = (0..ids.len()).map(AtomicUsize::new).collect::<Vec<_>>();
    let find_root = |mut slot: usize| {
        loop {
            let parent = parents[slot].load(Ordering::Acquire);
            if parent == slot {
                return slot;
            }
            slot = parent;
        }
    };
    scanned.par_iter().for_each(|(source, targets, _)| {
        for &target in targets {
            loop {
                let source_root = find_root(*source);
                let target_root = find_root(target);
                if source_root == target_root {
                    break;
                }
                let (lower, higher) = if source_root < target_root {
                    (source_root, target_root)
                } else {
                    (target_root, source_root)
                };
                if parents[higher]
                    .compare_exchange(higher, lower, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    break;
                }
            }
        }
    });

    // 最小 slot 总是根，因而线程调度不会改变 component id 或输出顺序。
    let mut grouped = BTreeMap::<usize, Vec<NodeId>>::new();
    for (slot, id) in ids.into_iter().enumerate() {
        grouped.entry(find_root(slot)).or_default().push(id);
    }
    let mut components = grouped.into_values().collect::<Vec<_>>();
    components.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    Ok((components, examined))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubsetBetweennessResult {
    pub scores: Vec<(NodeId, f64)>,
    pub sampled_sources: usize,
    pub exact: bool,
    pub examined_edges: usize,
}

pub fn subset_betweenness<T: VectorType>(
    mt: &MemTable<T>,
    nodes: &BTreeSet<NodeId>,
    label_filter: Option<&str>,
    sample_size: Option<usize>,
    max_examined_edges: usize,
) -> Result<SubsetBetweennessResult> {
    let ids = nodes.iter().copied().collect::<Vec<_>>();
    let source_count = sample_size.unwrap_or(ids.len()).min(ids.len());
    if sample_size == Some(0) {
        return Err(TriviumError::QueryExecution(
            "中介中心性采样数必须大于 0 (Betweenness sample size must be positive)".into(),
        ));
    }
    let mut adjacency = BTreeMap::<NodeId, Vec<NodeId>>::new();
    let mut scanned_edges = 0usize;
    for &source in &ids {
        let mut targets = BTreeSet::new();
        for edge in mt.get_edges(source).unwrap_or_default() {
            scanned_edges = scanned_edges.saturating_add(1);
            if nodes.contains(&edge.target_id)
                && label_filter.is_none_or(|label| edge.label == label)
            {
                targets.insert(edge.target_id);
            }
        }
        adjacency.insert(source, targets.into_iter().collect());
    }
    let mut examined = scanned_edges;
    if examined > max_examined_edges {
        return Err(TriviumError::QueryExecution(
            "中介中心性超过边访问预算 (Betweenness exceeds edge examination budget)".into(),
        ));
    }
    let mut centrality = BTreeMap::from_iter(ids.iter().map(|&id| (id, 0.0f64)));
    for &source in ids.iter().take(source_count) {
        let mut stack = Vec::new();
        let mut predecessors = BTreeMap::<NodeId, Vec<NodeId>>::new();
        let mut sigma = BTreeMap::from_iter(ids.iter().map(|&id| (id, 0.0f64)));
        let mut distance = BTreeMap::from_iter(ids.iter().map(|&id| (id, -1isize)));
        sigma.insert(source, 1.0);
        distance.insert(source, 0);
        let mut queue = VecDeque::from([source]);
        while let Some(current) = queue.pop_front() {
            stack.push(current);
            for &target in &adjacency[&current] {
                examined = examined.saturating_add(1);
                if examined > max_examined_edges {
                    return Err(TriviumError::QueryExecution(
                        "中介中心性超过边访问预算 (Betweenness exceeds edge examination budget)"
                            .into(),
                    ));
                }
                if distance[&target] < 0 {
                    distance.insert(target, distance[&current] + 1);
                    queue.push_back(target);
                }
                if distance[&target] == distance[&current] + 1 {
                    sigma.insert(target, sigma[&target] + sigma[&current]);
                    predecessors.entry(target).or_default().push(current);
                }
            }
        }
        let mut delta = BTreeMap::from_iter(ids.iter().map(|&id| (id, 0.0f64)));
        while let Some(target) = stack.pop() {
            if sigma[&target] > 0.0 {
                for &predecessor in predecessors.get(&target).unwrap_or(&Vec::new()) {
                    let contribution =
                        sigma[&predecessor] / sigma[&target] * (1.0 + delta[&target]);
                    *delta.get_mut(&predecessor).expect("前驱属于子图") += contribution;
                }
            }
            if target != source {
                *centrality.get_mut(&target).expect("目标属于子图") += delta[&target];
            }
        }
    }
    let n = ids.len();
    let normalization = n.saturating_sub(1).saturating_mul(n.saturating_sub(2)) as f64;
    let sample_scale = if source_count > 0 {
        n as f64 / source_count as f64
    } else {
        1.0
    };
    let mut scores = centrality
        .into_iter()
        .map(|(id, value)| {
            let normalized = if normalization > 0.0 {
                value / normalization
            } else {
                0.0
            };
            (id, normalized * sample_scale)
        })
        .collect::<Vec<_>>();
    scores.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    Ok(SubsetBetweennessResult {
        scores,
        sampled_sources: source_count,
        exact: source_count == n,
        examined_edges: examined,
    })
}

pub fn subset_betweenness_parallel<T: VectorType>(
    mt: &MemTable<T>,
    nodes: &BTreeSet<NodeId>,
    label_filter: Option<&str>,
    sample_size: Option<usize>,
    max_examined_edges: usize,
) -> Result<SubsetBetweennessResult> {
    use rayon::prelude::*;
    let ids = nodes.iter().copied().collect::<Vec<_>>();
    let source_count = sample_size.unwrap_or(ids.len()).min(ids.len());
    if sample_size == Some(0) {
        return Err(TriviumError::QueryExecution(
            "中介中心性采样数必须大于 0 (Betweenness sample size must be positive)".into(),
        ));
    }
    let scanned = ids
        .par_iter()
        .map(|&source| {
            let edges = mt.get_edges(source).unwrap_or_default();
            let mut targets = edges
                .iter()
                .filter_map(|edge| {
                    (nodes.contains(&edge.target_id)
                        && label_filter.is_none_or(|label| edge.label == label))
                    .then_some(edge.target_id)
                })
                .collect::<Vec<_>>();
            targets.sort_unstable();
            targets.dedup();
            (source, targets, edges.len())
        })
        .collect::<Vec<_>>();
    let adjacency = scanned
        .iter()
        .map(|(id, targets, _)| (*id, targets.clone()))
        .collect::<BTreeMap<_, _>>();
    let scanned_edges = scanned.iter().map(|(_, _, n)| *n).sum::<usize>();
    let per_source_edges = adjacency.values().map(Vec::len).sum::<usize>();
    let examined = scanned_edges.saturating_add(per_source_edges.saturating_mul(source_count));
    if examined > max_examined_edges {
        return Err(TriviumError::QueryExecution(
            "中介中心性超过边访问预算 (Betweenness exceeds edge examination budget)".into(),
        ));
    }
    let partials = ids[..source_count]
        .par_iter()
        .map(|&source| {
            let mut stack = Vec::new();
            let mut predecessors = BTreeMap::<NodeId, Vec<NodeId>>::new();
            let mut sigma = BTreeMap::from_iter(ids.iter().map(|&id| (id, 0.0f64)));
            let mut distance = BTreeMap::from_iter(ids.iter().map(|&id| (id, -1isize)));
            sigma.insert(source, 1.0);
            distance.insert(source, 0);
            let mut queue = VecDeque::from([source]);
            while let Some(current) = queue.pop_front() {
                stack.push(current);
                for &target in &adjacency[&current] {
                    if distance[&target] < 0 {
                        distance.insert(target, distance[&current] + 1);
                        queue.push_back(target);
                    }
                    if distance[&target] == distance[&current] + 1 {
                        sigma.insert(target, sigma[&target] + sigma[&current]);
                        predecessors.entry(target).or_default().push(current);
                    }
                }
            }
            let mut delta = BTreeMap::from_iter(ids.iter().map(|&id| (id, 0.0f64)));
            while let Some(target) = stack.pop() {
                if sigma[&target] > 0.0 {
                    for &predecessor in predecessors.get(&target).map(Vec::as_slice).unwrap_or(&[])
                    {
                        let contribution =
                            sigma[&predecessor] / sigma[&target] * (1.0 + delta[&target]);
                        *delta.get_mut(&predecessor).expect("前驱属于子图") += contribution;
                    }
                }
            }
            delta.insert(source, 0.0);
            delta
        })
        .collect::<Vec<_>>();
    let mut centrality = BTreeMap::from_iter(ids.iter().map(|&id| (id, 0.0f64)));
    for partial in partials {
        for (&id, &value) in &partial {
            *centrality.get_mut(&id).expect("节点属于子图") += value;
        }
    }
    let n = ids.len();
    let normalization = n.saturating_sub(1).saturating_mul(n.saturating_sub(2)) as f64;
    let sample_scale = if source_count > 0 {
        n as f64 / source_count as f64
    } else {
        1.0
    };
    let mut scores = centrality
        .into_iter()
        .map(|(id, value)| {
            (
                id,
                if normalization > 0.0 {
                    value / normalization * sample_scale
                } else {
                    0.0
                },
            )
        })
        .collect::<Vec<_>>();
    scores.par_sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(SubsetBetweennessResult {
        scores,
        sampled_sources: source_count,
        exact: source_count == n,
        examined_edges: examined,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LabelPropagationConfig {
    pub max_iterations: usize,
    pub min_community_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LabelPropagationResult {
    pub node_to_community: BTreeMap<NodeId, u64>,
    pub iterations: usize,
    pub converged: bool,
    pub examined_edges: usize,
}

pub fn deterministic_label_propagation<T: VectorType>(
    mt: &MemTable<T>,
    nodes: &BTreeSet<NodeId>,
    config: LabelPropagationConfig,
    label_filter: Option<&str>,
    max_examined_edges: usize,
) -> Result<LabelPropagationResult> {
    if config.max_iterations == 0 || config.min_community_size == 0 {
        return Err(TriviumError::QueryExecution(
            "标签传播迭代次数和最小社区大小必须大于 0 (Label propagation limits must be positive)"
                .into(),
        ));
    }
    let mut adjacency = BTreeMap::<NodeId, Vec<(NodeId, f64)>>::new();
    let mut examined = 0usize;
    for &source in nodes {
        let mut targets = BTreeMap::<NodeId, f64>::new();
        for edge in mt.get_edges(source).unwrap_or_default() {
            examined = examined.saturating_add(1);
            if examined > max_examined_edges {
                return Err(TriviumError::QueryExecution(
                    "标签传播超过边访问预算 (Label propagation exceeds edge examination budget)"
                        .into(),
                ));
            }
            if nodes.contains(&edge.target_id)
                && edge.weight.is_finite()
                && edge.weight > 0.0
                && label_filter.is_none_or(|label| edge.label == label)
            {
                *targets.entry(edge.target_id).or_insert(0.0) += edge.weight as f64;
            }
        }
        adjacency.insert(source, targets.into_iter().collect());
    }
    // 社区检测使用无向投影；有向边的两个端点互为邻居，相反方向的平行边权会累加。
    let directed = adjacency.clone();
    for (&source, targets) in &directed {
        for &(target, weight) in targets {
            let reverse = adjacency.entry(target).or_default();
            if let Some((_, existing)) =
                reverse.iter_mut().find(|(neighbor, _)| *neighbor == source)
            {
                *existing += weight;
            } else {
                reverse.push((source, weight));
            }
        }
    }
    for targets in adjacency.values_mut() {
        targets.sort_by_key(|(target, _)| *target);
    }
    let mut labels = BTreeMap::from_iter(nodes.iter().map(|&id| (id, id)));
    let mut iterations = 0usize;
    let mut converged = false;
    for iteration in 1..=config.max_iterations {
        let mut next = labels.clone();
        let mut changed = false;
        for &node in nodes {
            let mut votes = BTreeMap::<NodeId, f64>::new();
            for &(neighbor, weight) in &adjacency[&node] {
                *votes.entry(labels[&neighbor]).or_insert(0.0) += weight;
            }
            let best = votes.into_iter().max_by(|left, right| {
                left.1
                    .total_cmp(&right.1)
                    .then_with(|| right.0.cmp(&left.0))
            });
            if let Some((label, _)) = best
                && label != labels[&node]
            {
                next.insert(node, label);
                changed = true;
            }
        }
        labels = next;
        iterations = iteration;
        if !changed {
            converged = true;
            break;
        }
    }
    let mut sizes = BTreeMap::<NodeId, usize>::new();
    for &label in labels.values() {
        *sizes.entry(label).or_insert(0) += 1;
    }
    labels.retain(|_, label| sizes[label] >= config.min_community_size);
    let valid = labels.values().copied().collect::<BTreeSet<_>>();
    let remap = valid
        .into_iter()
        .enumerate()
        .map(|(index, label)| (label, index as u64 + 1))
        .collect::<BTreeMap<_, _>>();
    let node_to_community = labels
        .into_iter()
        .map(|(node, label)| (node, remap[&label]))
        .collect();
    Ok(LabelPropagationResult {
        node_to_community,
        iterations,
        converged,
        examined_edges: examined,
    })
}

pub fn deterministic_label_propagation_parallel<T: VectorType>(
    mt: &MemTable<T>,
    nodes: &BTreeSet<NodeId>,
    config: LabelPropagationConfig,
    label_filter: Option<&str>,
    max_examined_edges: usize,
) -> Result<LabelPropagationResult> {
    use rayon::prelude::*;
    if config.max_iterations == 0 || config.min_community_size == 0 {
        return Err(TriviumError::QueryExecution(
            "标签传播迭代次数和最小社区大小必须大于 0 (Label propagation limits must be positive)"
                .into(),
        ));
    }
    let ids = nodes.iter().copied().collect::<Vec<_>>();
    let slots = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect::<HashMap<_, _>>();
    let scanned = ids
        .par_iter()
        .map(|&source| {
            let edges = mt.get_edges(source).unwrap_or_default();
            let mut targets = BTreeMap::<usize, f64>::new();
            for edge in edges {
                if let Some(&target) = slots.get(&edge.target_id)
                    && edge.weight.is_finite()
                    && edge.weight > 0.0
                    && label_filter.is_none_or(|label| edge.label == label)
                {
                    *targets.entry(target).or_insert(0.0) += edge.weight as f64;
                }
            }
            (targets, edges.len())
        })
        .collect::<Vec<_>>();
    let examined = scanned.iter().map(|(_, n)| *n).sum::<usize>();
    if examined > max_examined_edges {
        return Err(TriviumError::QueryExecution(
            "标签传播超过边访问预算 (Label propagation exceeds edge examination budget)".into(),
        ));
    }
    let mut adjacency = scanned
        .iter()
        .map(|(targets, _)| targets.clone())
        .collect::<Vec<_>>();
    for (source, (targets, _)) in scanned.iter().enumerate() {
        for (&target, &weight) in targets {
            *adjacency[target].entry(source).or_insert(0.0) += weight;
        }
    }
    let mut labels = ids.clone();
    let mut iterations = 0;
    let mut converged = false;
    for iteration in 1..=config.max_iterations {
        let next = (0..ids.len())
            .into_par_iter()
            .map(|node| {
                let mut votes = BTreeMap::<NodeId, f64>::new();
                for (&neighbor, &weight) in &adjacency[node] {
                    *votes.entry(labels[neighbor]).or_insert(0.0) += weight;
                }
                votes
                    .into_iter()
                    .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                    .map_or(labels[node], |(label, _)| label)
            })
            .collect::<Vec<_>>();
        let changed = next.par_iter().zip(&labels).any(|(a, b)| a != b);
        labels = next;
        iterations = iteration;
        if !changed {
            converged = true;
            break;
        }
    }
    let mut sizes = BTreeMap::<NodeId, usize>::new();
    for &label in &labels {
        *sizes.entry(label).or_insert(0) += 1;
    }
    let valid = labels
        .iter()
        .copied()
        .filter(|label| sizes[label] >= config.min_community_size)
        .collect::<BTreeSet<_>>();
    let remap = valid
        .into_iter()
        .enumerate()
        .map(|(i, label)| (label, i as u64 + 1))
        .collect::<BTreeMap<_, _>>();
    let node_to_community = ids
        .into_iter()
        .zip(labels)
        .filter_map(|(node, label)| remap.get(&label).map(|&community| (node, community)))
        .collect();
    Ok(LabelPropagationResult {
        node_to_community,
        iterations,
        converged,
        examined_edges: examined,
    })
}
