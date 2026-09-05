//! 面向 TQL NodeSet 的确定性、有预算图结构分析算法。
//!
//! 所有算法只读取调用方给定的诱导子图；节点与邻居均按 NodeId 排序。构图阶段同时
//! 计算边访问量和临时内存，任一预算不足都会在算法工作区分配前失败。

use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct GraphWorkspace {
    pub directed: BTreeMap<NodeId, Vec<NodeId>>,
    pub reverse: BTreeMap<NodeId, Vec<NodeId>>,
    pub undirected: BTreeMap<NodeId, Vec<NodeId>>,
    pub weighted: BTreeMap<NodeId, Vec<(NodeId, f64)>>,
    pub examined_edges: usize,
}

pub fn build_workspace<T: VectorType>(
    mt: &MemTable<T>,
    nodes: &BTreeSet<NodeId>,
    label_filter: Option<&str>,
    max_examined_edges: usize,
    max_bytes: usize,
) -> Result<GraphWorkspace> {
    let base_bytes = nodes
        .len()
        .checked_mul(std::mem::size_of::<NodeId>() * 10)
        .ok_or_else(|| TriviumError::QueryExecution("图算法工作区大小溢出".into()))?;
    if base_bytes > max_bytes {
        return Err(TriviumError::QueryExecution(
            "图算法工作区超过临时内存预算 (Graph workspace exceeds temporary memory budget)".into(),
        ));
    }
    let mut examined_edges = 0usize;
    let mut retained_edges = 0usize;
    for &source in nodes {
        for edge in mt.get_edges(source).unwrap_or_default() {
            examined_edges = examined_edges.saturating_add(1);
            if examined_edges > max_examined_edges {
                return Err(TriviumError::QueryExecution(
                    "图算法超过边访问预算 (Graph algorithm exceeds edge examination budget)".into(),
                ));
            }
            if nodes.contains(&edge.target_id)
                && label_filter.is_none_or(|label| edge.label == label)
            {
                retained_edges = retained_edges.saturating_add(1);
            }
        }
    }
    let edge_bytes = retained_edges
        .checked_mul(std::mem::size_of::<NodeId>() * 6)
        .ok_or_else(|| TriviumError::QueryExecution("图算法邻接工作区大小溢出".into()))?;
    if base_bytes.saturating_add(edge_bytes) > max_bytes {
        return Err(TriviumError::QueryExecution(
            "图算法邻接表超过临时内存预算 (Graph adjacency exceeds temporary memory budget)".into(),
        ));
    }
    let mut directed = BTreeMap::from_iter(nodes.iter().map(|&id| (id, Vec::new())));
    let mut reverse = directed.clone();
    let mut weighted = BTreeMap::from_iter(nodes.iter().map(|&id| (id, Vec::new())));
    for &source in nodes {
        for edge in mt.get_edges(source).unwrap_or_default() {
            if nodes.contains(&edge.target_id)
                && label_filter.is_none_or(|label| edge.label == label)
            {
                directed.entry(source).or_default().push(edge.target_id);
                reverse.entry(edge.target_id).or_default().push(source);
                weighted
                    .entry(source)
                    .or_default()
                    .push((edge.target_id, f64::from(edge.weight)));
            }
        }
    }
    for targets in directed.values_mut() {
        targets.sort_unstable();
        targets.dedup();
    }
    for sources in reverse.values_mut() {
        sources.sort_unstable();
        sources.dedup();
    }
    for targets in weighted.values_mut() {
        targets.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.total_cmp(&right.1))
        });
        let mut deduplicated = Vec::<(NodeId, f64)>::new();
        for &(target, weight) in targets.iter() {
            if let Some((last_target, last_weight)) = deduplicated.last_mut()
                && *last_target == target
            {
                *last_weight = last_weight.min(weight);
            } else {
                deduplicated.push((target, weight));
            }
        }
        *targets = deduplicated;
    }
    let mut undirected = BTreeMap::from_iter(nodes.iter().map(|&id| (id, Vec::new())));
    for (&source, targets) in &directed {
        for &target in targets {
            if source != target {
                undirected.entry(source).or_default().push(target);
                undirected.entry(target).or_default().push(source);
            }
        }
    }
    for neighbors in undirected.values_mut() {
        neighbors.sort_unstable();
        neighbors.dedup();
    }
    Ok(GraphWorkspace {
        directed,
        reverse,
        undirected,
        weighted,
        examined_edges,
    })
}

pub fn strongly_connected_components(graph: &GraphWorkspace) -> BTreeMap<NodeId, NodeId> {
    fn finish_order(
        start: NodeId,
        adjacency: &BTreeMap<NodeId, Vec<NodeId>>,
        visited: &mut BTreeSet<NodeId>,
        order: &mut Vec<NodeId>,
    ) {
        let mut stack = vec![(start, false)];
        while let Some((node, expanded)) = stack.pop() {
            if expanded {
                order.push(node);
                continue;
            }
            if !visited.insert(node) {
                continue;
            }
            stack.push((node, true));
            if let Some(neighbors) = adjacency.get(&node) {
                for &neighbor in neighbors.iter().rev() {
                    if !visited.contains(&neighbor) {
                        stack.push((neighbor, false));
                    }
                }
            }
        }
    }
    let mut visited = BTreeSet::new();
    let mut order = Vec::new();
    for &node in graph.directed.keys() {
        if !visited.contains(&node) {
            finish_order(node, &graph.directed, &mut visited, &mut order);
        }
    }
    let mut result = BTreeMap::new();
    visited.clear();
    for &start in order.iter().rev() {
        if visited.contains(&start) {
            continue;
        }
        let mut stack = vec![start];
        let mut component = Vec::new();
        visited.insert(start);
        while let Some(node) = stack.pop() {
            component.push(node);
            if let Some(neighbors) = graph.reverse.get(&node) {
                for &neighbor in neighbors.iter().rev() {
                    if visited.insert(neighbor) {
                        stack.push(neighbor);
                    }
                }
            }
        }
        let component_id = component.iter().copied().min().unwrap_or(start);
        for node in component {
            result.insert(node, component_id);
        }
    }
    result
}

pub fn k_core(graph: &GraphWorkspace) -> BTreeMap<NodeId, u64> {
    let mut degree = BTreeMap::from_iter(
        graph
            .undirected
            .iter()
            .map(|(&id, neighbors)| (id, neighbors.len())),
    );
    let mut remaining = graph.undirected.keys().copied().collect::<BTreeSet<_>>();
    let mut output = BTreeMap::new();
    let mut core = 0usize;
    while let Some((&node, &node_degree)) = remaining
        .iter()
        .map(|id| (id, degree.get(id).unwrap_or(&0)))
        .min_by_key(|(id, degree)| (**degree, **id))
    {
        remaining.remove(&node);
        core = core.max(node_degree);
        output.insert(node, core as u64);
        for neighbor in graph.undirected.get(&node).into_iter().flatten() {
            if remaining.contains(neighbor) {
                degree
                    .entry(*neighbor)
                    .and_modify(|value| *value = value.saturating_sub(1));
            }
        }
    }
    output
}

pub fn articulation_points(graph: &GraphWorkspace) -> BTreeSet<NodeId> {
    let mut discovery = BTreeMap::<NodeId, usize>::new();
    let mut low = BTreeMap::<NodeId, usize>::new();
    let mut parent = BTreeMap::<NodeId, NodeId>::new();
    let mut points = BTreeSet::new();
    let mut time = 0usize;
    for &root in graph.undirected.keys() {
        if discovery.contains_key(&root) {
            continue;
        }
        let mut root_children = 0usize;
        let mut stack = vec![(root, 0usize)];
        time += 1;
        discovery.insert(root, time);
        low.insert(root, time);
        while let Some((node, next_index)) = stack.pop() {
            let neighbors = graph
                .undirected
                .get(&node)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if next_index < neighbors.len() {
                stack.push((node, next_index + 1));
                let neighbor = neighbors[next_index];
                match discovery.entry(neighbor) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        parent.insert(neighbor, node);
                        if node == root {
                            root_children += 1;
                        }
                        time += 1;
                        entry.insert(time);
                        low.insert(neighbor, time);
                        stack.push((neighbor, 0));
                    }
                    std::collections::btree_map::Entry::Occupied(entry)
                        if parent.get(&node).copied() != Some(neighbor) =>
                    {
                        let candidate = *entry.get();
                        low.entry(node)
                            .and_modify(|value| *value = (*value).min(candidate));
                    }
                    std::collections::btree_map::Entry::Occupied(_) => {}
                }
            } else if let Some(&parent_node) = parent.get(&node) {
                let Some(&node_low) = low.get(&node) else {
                    continue;
                };
                low.entry(parent_node)
                    .and_modify(|value| *value = (*value).min(node_low));
                if parent_node != root
                    && discovery
                        .get(&parent_node)
                        .is_some_and(|&parent_discovery| node_low >= parent_discovery)
                {
                    points.insert(parent_node);
                }
            }
        }
        if root_children > 1 {
            points.insert(root);
        }
    }
    points
}

pub fn triangle_metrics(
    graph: &GraphWorkspace,
    max_work: usize,
) -> Result<BTreeMap<NodeId, (u64, f64)>> {
    let mut counts = BTreeMap::from_iter(graph.undirected.keys().map(|&id| (id, 0u64)));
    let mut work = 0usize;
    for (&a, neighbors) in &graph.undirected {
        for &b in neighbors.iter().filter(|&&neighbor| neighbor > a) {
            let Some(b_neighbors) = graph.undirected.get(&b) else {
                continue;
            };
            for &c in b_neighbors.iter().filter(|&&neighbor| neighbor > b) {
                work = work.saturating_add(1);
                if work > max_work {
                    return Err(TriviumError::QueryExecution(
                        "三角计数超过交集工作预算 (Triangle count exceeds intersection work budget)".into(),
                    ));
                }
                if neighbors.contains(&c) {
                    *counts.entry(a).or_default() += 1;
                    *counts.entry(b).or_default() += 1;
                    *counts.entry(c).or_default() += 1;
                }
            }
        }
    }
    Ok(counts
        .into_iter()
        .map(|(id, count)| {
            let degree = graph.undirected.get(&id).map_or(0, Vec::len);
            let coefficient = if degree >= 2 {
                2.0 * count as f64 / (degree * (degree - 1)) as f64
            } else {
                0.0
            };
            (id, (count, coefficient))
        })
        .collect())
}

pub fn hits(
    graph: &GraphWorkspace,
    max_iterations: usize,
    tolerance: f64,
    max_work: usize,
) -> Result<BTreeMap<NodeId, (f64, f64)>> {
    if max_iterations == 0 || !tolerance.is_finite() || tolerance <= 0.0 {
        return Err(TriviumError::InvalidInput(
            "HITS 参数无效 (Invalid HITS parameters)".into(),
        ));
    }
    let ids = graph.directed.keys().copied().collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let per_iteration = graph
        .directed
        .values()
        .map(Vec::len)
        .sum::<usize>()
        .saturating_mul(2);
    let initial = 1.0 / (ids.len() as f64).sqrt();
    let mut hubs = BTreeMap::from_iter(ids.iter().map(|&id| (id, initial)));
    let mut authorities = hubs.clone();
    let mut work = 0usize;
    for _ in 0..max_iterations {
        work = work
            .checked_add(per_iteration)
            .ok_or_else(|| TriviumError::QueryExecution("HITS 工作预算溢出".into()))?;
        if work > max_work {
            return Err(TriviumError::QueryExecution(
                "HITS 超过迭代边预算 (HITS exceeds iterative edge budget)".into(),
            ));
        }
        let mut next_authorities = BTreeMap::new();
        for &id in &ids {
            let score = graph
                .reverse
                .get(&id)
                .into_iter()
                .flatten()
                .filter_map(|source| hubs.get(source))
                .sum::<f64>();
            next_authorities.insert(id, score);
        }
        normalize(&mut next_authorities);
        let mut next_hubs = BTreeMap::new();
        for &id in &ids {
            let score = graph
                .directed
                .get(&id)
                .into_iter()
                .flatten()
                .filter_map(|target| next_authorities.get(target))
                .sum::<f64>();
            next_hubs.insert(id, score);
        }
        normalize(&mut next_hubs);
        let delta = ids
            .iter()
            .map(|id| {
                let next_hub = next_hubs.get(id).copied().unwrap_or_default();
                let hub = hubs.get(id).copied().unwrap_or_default();
                let next_authority = next_authorities.get(id).copied().unwrap_or_default();
                let authority = authorities.get(id).copied().unwrap_or_default();
                (next_hub - hub).abs() + (next_authority - authority).abs()
            })
            .sum::<f64>();
        hubs = next_hubs;
        authorities = next_authorities;
        if delta <= tolerance {
            break;
        }
    }
    Ok(ids
        .into_iter()
        .map(|id| {
            (
                id,
                (
                    authorities.get(&id).copied().unwrap_or_default(),
                    hubs.get(&id).copied().unwrap_or_default(),
                ),
            )
        })
        .collect())
}

fn normalize(values: &mut BTreeMap<NodeId, f64>) {
    let norm = values
        .values()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if norm > 0.0 {
        for value in values.values_mut() {
            *value /= norm;
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeightedPath {
    pub nodes: Vec<NodeId>,
    pub cost: f64,
}

type WeightedSearchState = (BTreeMap<NodeId, f64>, BTreeMap<NodeId, Vec<NodeId>>);

fn shortest_weighted_path(
    graph: &GraphWorkspace,
    source: NodeId,
    target: Option<NodeId>,
    banned_nodes: &BTreeSet<NodeId>,
    banned_edges: &BTreeSet<(NodeId, NodeId)>,
    work: &mut usize,
    max_work: usize,
) -> Result<WeightedSearchState> {
    let mut distances =
        BTreeMap::from_iter(graph.weighted.keys().copied().map(|id| (id, f64::INFINITY)));
    let mut paths = BTreeMap::<NodeId, Vec<NodeId>>::new();
    if banned_nodes.contains(&source) || !distances.contains_key(&source) {
        return Ok((distances, paths));
    }
    distances.insert(source, 0.0);
    paths.insert(source, vec![source]);
    let mut settled = BTreeSet::new();
    loop {
        let next = distances
            .iter()
            .filter(|(id, distance)| !settled.contains(*id) && distance.is_finite())
            .min_by(|left, right| {
                left.1
                    .total_cmp(right.1)
                    .then_with(|| paths.get(left.0).cmp(&paths.get(right.0)))
                    .then_with(|| left.0.cmp(right.0))
            })
            .map(|(&id, &distance)| (id, distance));
        let Some((node, distance)) = next else {
            break;
        };
        settled.insert(node);
        if target == Some(node) {
            break;
        }
        let Some(prefix) = paths.get(&node).cloned() else {
            continue;
        };
        for &(neighbor, weight) in graph.weighted.get(&node).into_iter().flatten() {
            *work = work
                .checked_add(1)
                .ok_or_else(|| TriviumError::QueryExecution("加权路径工作预算溢出".into()))?;
            if *work > max_work {
                return Err(TriviumError::QueryExecution(
                    "加权路径超过工作预算 (Weighted path exceeds work budget)".into(),
                ));
            }
            if banned_nodes.contains(&neighbor)
                || banned_edges.contains(&(node, neighbor))
                || settled.contains(&neighbor)
            {
                continue;
            }
            let candidate_distance = distance + weight;
            let mut candidate_path = prefix.clone();
            candidate_path.push(neighbor);
            let current_distance = distances.get(&neighbor).copied().unwrap_or(f64::INFINITY);
            let replace = candidate_distance < current_distance
                || candidate_distance == current_distance
                    && paths
                        .get(&neighbor)
                        .is_none_or(|current| candidate_path < *current);
            if replace {
                distances.insert(neighbor, candidate_distance);
                paths.insert(neighbor, candidate_path);
            }
        }
    }
    Ok((distances, paths))
}

fn validate_weighted_graph(graph: &GraphWorkspace) -> Result<()> {
    if graph
        .weighted
        .values()
        .flatten()
        .any(|(_, weight)| !weight.is_finite() || *weight < 0.0)
    {
        return Err(TriviumError::InvalidInput(
            "加权算法要求非负有限边权 (Weighted algorithms require finite non-negative edge weights)".into(),
        ));
    }
    Ok(())
}

pub fn weighted_dijkstra(
    graph: &GraphWorkspace,
    source: NodeId,
    target: NodeId,
    max_work: usize,
) -> Result<Option<WeightedPath>> {
    validate_weighted_graph(graph)?;
    let mut work = 0usize;
    let (distances, paths) = shortest_weighted_path(
        graph,
        source,
        Some(target),
        &BTreeSet::new(),
        &BTreeSet::new(),
        &mut work,
        max_work,
    )?;
    Ok(paths.get(&target).cloned().map(|nodes| WeightedPath {
        nodes,
        cost: distances.get(&target).copied().unwrap_or(f64::INFINITY),
    }))
}

pub fn harmonic_centrality(
    graph: &GraphWorkspace,
    max_work: usize,
) -> Result<BTreeMap<NodeId, f64>> {
    validate_weighted_graph(graph)?;
    let mut work = 0usize;
    let mut output = BTreeMap::new();
    for &source in graph.weighted.keys() {
        let (distances, _) = shortest_weighted_path(
            graph,
            source,
            None,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &mut work,
            max_work,
        )?;
        let score = distances
            .into_iter()
            .filter(|&(target, distance)| {
                target != source && distance.is_finite() && distance > 0.0
            })
            .map(|(_, distance)| 1.0 / distance)
            .sum();
        output.insert(source, score);
    }
    Ok(output)
}

pub fn yen_k_shortest_paths(
    graph: &GraphWorkspace,
    source: NodeId,
    target: NodeId,
    k: usize,
    max_work: usize,
    max_candidates: usize,
) -> Result<Vec<WeightedPath>> {
    validate_weighted_graph(graph)?;
    if k == 0 || max_candidates == 0 {
        return Err(TriviumError::InvalidInput(
            "Yen 的 K 和候选上限必须大于 0 (Yen K and candidate limit must be positive)".into(),
        ));
    }
    let mut work = 0usize;
    let (distances, paths) = shortest_weighted_path(
        graph,
        source,
        Some(target),
        &BTreeSet::new(),
        &BTreeSet::new(),
        &mut work,
        max_work,
    )?;
    let Some(first_nodes) = paths.get(&target).cloned() else {
        return Ok(Vec::new());
    };
    let mut accepted = vec![WeightedPath {
        nodes: first_nodes,
        cost: distances.get(&target).copied().unwrap_or(f64::INFINITY),
    }];
    let mut candidates = BTreeMap::<Vec<NodeId>, f64>::new();
    while accepted.len() < k {
        let previous = accepted
            .last()
            .cloned()
            .ok_or_else(|| TriviumError::QueryExecution("Yen 已接受路径状态不一致".into()))?;
        for spur_index in 0..previous.nodes.len().saturating_sub(1) {
            let root = &previous.nodes[..=spur_index];
            let spur = previous.nodes[spur_index];
            let banned_nodes = root[..root.len().saturating_sub(1)]
                .iter()
                .copied()
                .collect::<BTreeSet<_>>();
            let banned_edges = accepted
                .iter()
                .filter(|path| path.nodes.len() > spur_index && path.nodes[..=spur_index] == *root)
                .filter_map(|path| {
                    path.nodes
                        .get(spur_index + 1)
                        .copied()
                        .map(|next| (spur, next))
                })
                .collect::<BTreeSet<_>>();
            let (_, spur_paths) = shortest_weighted_path(
                graph,
                spur,
                Some(target),
                &banned_nodes,
                &banned_edges,
                &mut work,
                max_work,
            )?;
            let Some(spur_path) = spur_paths.get(&target) else {
                continue;
            };
            let mut nodes = root[..root.len().saturating_sub(1)].to_vec();
            nodes.extend(spur_path.iter().copied());
            let mut cost = 0.0;
            let mut valid = true;
            for edge in nodes.windows(2) {
                let Some(weight) = graph
                    .weighted
                    .get(&edge[0])
                    .into_iter()
                    .flatten()
                    .find(|(target, _)| *target == edge[1])
                    .map(|(_, weight)| *weight)
                else {
                    valid = false;
                    break;
                };
                cost += weight;
            }
            if valid && !accepted.iter().any(|path| path.nodes == nodes) {
                candidates.entry(nodes).or_insert(cost);
                if candidates.len() > max_candidates {
                    return Err(TriviumError::QueryExecution(
                        "Yen 候选路径超过预算 (Yen candidate paths exceed budget)".into(),
                    ));
                }
            }
        }
        let Some((nodes, cost)) = candidates
            .iter()
            .min_by(|left, right| left.1.total_cmp(right.1).then_with(|| left.0.cmp(right.0)))
            .map(|(nodes, &cost)| (nodes.clone(), cost))
        else {
            break;
        };
        candidates.remove(&nodes);
        accepted.push(WeightedPath { nodes, cost });
    }
    Ok(accepted)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeSimilarityPair {
    pub left: NodeId,
    pub right: NodeId,
    pub similarity: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PairSet {
    pairs: Vec<NodeSimilarityPair>,
}

impl PairSet {
    pub fn pairs(&self) -> &[NodeSimilarityPair] {
        &self.pairs
    }
}

pub fn node_similarity(
    graph: &GraphWorkspace,
    top_k: usize,
    cutoff: f64,
    max_comparisons: usize,
    max_bytes: usize,
) -> Result<PairSet> {
    if top_k == 0 || !cutoff.is_finite() || !(0.0..=1.0).contains(&cutoff) {
        return Err(TriviumError::InvalidInput(
            "节点相似度参数无效 (Invalid node similarity parameters)".into(),
        ));
    }
    let node_count = graph.undirected.len();
    let comparisons = node_count
        .checked_mul(node_count.saturating_sub(1))
        .and_then(|value| value.checked_div(2))
        .ok_or_else(|| TriviumError::QueryExecution("节点对数量溢出".into()))?;
    if comparisons > max_comparisons {
        return Err(TriviumError::QueryExecution(
            "节点相似度超过比较预算 (Node similarity exceeds comparison budget)".into(),
        ));
    }
    let capacity = top_k.min(comparisons);
    let required_bytes = capacity
        .checked_mul(std::mem::size_of::<NodeSimilarityPair>())
        .ok_or_else(|| TriviumError::QueryExecution("PairSet 大小溢出".into()))?;
    if required_bytes > max_bytes {
        return Err(TriviumError::QueryExecution(
            "PairSet 超过临时内存预算 (PairSet exceeds temporary memory budget)".into(),
        ));
    }
    let ids = graph.undirected.keys().copied().collect::<Vec<_>>();
    let mut pairs = Vec::with_capacity(capacity);
    let mut work = 0usize;
    for (index, &left) in ids.iter().enumerate() {
        let left_neighbors = graph
            .undirected
            .get(&left)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for &right in &ids[index + 1..] {
            work = work
                .checked_add(1)
                .ok_or_else(|| TriviumError::QueryExecution("节点相似度工作预算溢出".into()))?;
            if work > max_comparisons {
                return Err(TriviumError::QueryExecution(
                    "节点相似度超过比较预算 (Node similarity exceeds comparison budget)".into(),
                ));
            }
            let right_neighbors = graph
                .undirected
                .get(&right)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut left_index = 0usize;
            let mut right_index = 0usize;
            let mut intersection = 0usize;
            while left_index < left_neighbors.len() && right_index < right_neighbors.len() {
                work = work
                    .checked_add(1)
                    .ok_or_else(|| TriviumError::QueryExecution("节点相似度工作预算溢出".into()))?;
                if work > max_comparisons {
                    return Err(TriviumError::QueryExecution(
                        "节点相似度超过邻居交集预算 (Node similarity exceeds neighbor intersection budget)".into(),
                    ));
                }
                match left_neighbors[left_index].cmp(&right_neighbors[right_index]) {
                    std::cmp::Ordering::Less => left_index += 1,
                    std::cmp::Ordering::Greater => right_index += 1,
                    std::cmp::Ordering::Equal => {
                        intersection += 1;
                        left_index += 1;
                        right_index += 1;
                    }
                }
            }
            let union = left_neighbors
                .len()
                .saturating_add(right_neighbors.len())
                .saturating_sub(intersection);
            let similarity = if union == 0 {
                0.0
            } else {
                intersection as f64 / union as f64
            };
            if similarity < cutoff {
                continue;
            }
            pairs.push(NodeSimilarityPair {
                left,
                right,
                similarity,
            });
            pairs.sort_by(|left, right| {
                right
                    .similarity
                    .total_cmp(&left.similarity)
                    .then_with(|| left.left.cmp(&right.left))
                    .then_with(|| left.right.cmp(&right.right))
            });
            if pairs.len() > top_k {
                pairs.pop();
            }
        }
    }
    Ok(PairSet { pairs })
}
