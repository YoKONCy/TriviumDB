//! G1～G4 图算法的独立朴素 reference 与固定种子差分。
//!
//! Reference 仅使用测试侧边集合和穷举算法，不调用生产图算法、Pipeline 或邻接构建逻辑。

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{BTreeMap, BTreeSet};
use triviumdb::graph::analytics::{
    GraphWorkspace, articulation_points, harmonic_centrality, hits, k_core, node_similarity,
    strongly_connected_components, triangle_metrics, weighted_dijkstra, yen_k_shortest_paths,
};

#[derive(Debug, Clone)]
struct RefGraph {
    nodes: BTreeSet<u64>,
    edges: BTreeMap<(u64, u64), f64>,
}

impl RefGraph {
    fn random(seed: u64, node_count: u64) -> Self {
        let mut rng = StdRng::seed_from_u64(seed);
        let nodes = (1..=node_count).collect();
        let mut edges = BTreeMap::new();
        for source in 1..=node_count {
            for target in 1..=node_count {
                if source != target && rng.gen_bool(0.24) {
                    edges.insert((source, target), rng.gen_range(1..=8) as f64 / 2.0);
                }
            }
        }
        Self { nodes, edges }
    }

    fn outgoing(&self, node: u64) -> BTreeSet<u64> {
        self.edges
            .keys()
            .filter_map(|&(source, target)| (source == node).then_some(target))
            .collect()
    }

    fn incoming(&self, node: u64) -> BTreeSet<u64> {
        self.edges
            .keys()
            .filter_map(|&(source, target)| (target == node).then_some(source))
            .collect()
    }

    fn neighbors(&self, node: u64) -> BTreeSet<u64> {
        self.outgoing(node)
            .union(&self.incoming(node))
            .copied()
            .filter(|target| *target != node)
            .collect()
    }

    fn workspace(&self) -> GraphWorkspace {
        let directed = self
            .nodes
            .iter()
            .map(|&node| (node, self.outgoing(node).into_iter().collect()))
            .collect();
        let reverse = self
            .nodes
            .iter()
            .map(|&node| (node, self.incoming(node).into_iter().collect()))
            .collect();
        let undirected = self
            .nodes
            .iter()
            .map(|&node| (node, self.neighbors(node).into_iter().collect()))
            .collect();
        let weighted = self
            .nodes
            .iter()
            .map(|&node| {
                let targets = self
                    .edges
                    .iter()
                    .filter_map(|(&(source, target), &weight)| {
                        (source == node).then_some((target, weight))
                    })
                    .collect();
                (node, targets)
            })
            .collect();
        GraphWorkspace {
            directed,
            reverse,
            undirected,
            weighted,
            examined_edges: self.edges.len(),
        }
    }
}

fn reachable(graph: &RefGraph, source: u64) -> BTreeSet<u64> {
    let mut reached = BTreeSet::from([source]);
    let mut frontier = vec![source];
    while let Some(node) = frontier.pop() {
        for target in graph.outgoing(node) {
            if reached.insert(target) {
                frontier.push(target);
            }
        }
    }
    reached
}

fn reference_scc(graph: &RefGraph) -> BTreeMap<u64, u64> {
    let reach = graph
        .nodes
        .iter()
        .map(|&node| (node, reachable(graph, node)))
        .collect::<BTreeMap<_, _>>();
    graph
        .nodes
        .iter()
        .map(|&node| {
            let component = graph
                .nodes
                .iter()
                .filter(|&&other| reach[&node].contains(&other) && reach[&other].contains(&node))
                .copied()
                .min()
                .unwrap_or(node);
            (node, component)
        })
        .collect()
}

fn reference_k_core(graph: &RefGraph) -> BTreeMap<u64, u64> {
    let mut output = BTreeMap::new();
    for &node in &graph.nodes {
        let mut best = 0u64;
        for k in 1..=graph.nodes.len() as u64 {
            let mut remaining = graph.nodes.clone();
            loop {
                let remove = remaining
                    .iter()
                    .filter(|&&candidate| {
                        graph.neighbors(candidate).intersection(&remaining).count() < k as usize
                    })
                    .copied()
                    .collect::<Vec<_>>();
                if remove.is_empty() {
                    break;
                }
                for candidate in remove {
                    remaining.remove(&candidate);
                }
            }
            if remaining.contains(&node) {
                best = k;
            }
        }
        output.insert(node, best);
    }
    output
}

fn component_count(graph: &RefGraph, omitted: Option<u64>) -> usize {
    let mut unseen = graph
        .nodes
        .iter()
        .copied()
        .filter(|node| Some(*node) != omitted)
        .collect::<BTreeSet<_>>();
    let mut count = 0;
    while let Some(start) = unseen.pop_first() {
        count += 1;
        let mut frontier = vec![start];
        while let Some(node) = frontier.pop() {
            for next in graph.neighbors(node) {
                if Some(next) != omitted && unseen.remove(&next) {
                    frontier.push(next);
                }
            }
        }
    }
    count
}

fn reference_articulation(graph: &RefGraph) -> BTreeSet<u64> {
    let baseline = component_count(graph, None);
    graph
        .nodes
        .iter()
        .filter(|&&node| component_count(graph, Some(node)) > baseline)
        .copied()
        .collect()
}

fn reference_triangles(graph: &RefGraph) -> BTreeMap<u64, (u64, f64)> {
    let ids = graph.nodes.iter().copied().collect::<Vec<_>>();
    let mut counts = BTreeMap::from_iter(ids.iter().map(|&id| (id, 0u64)));
    for a in 0..ids.len() {
        for b in a + 1..ids.len() {
            for c in b + 1..ids.len() {
                if graph.neighbors(ids[a]).contains(&ids[b])
                    && graph.neighbors(ids[a]).contains(&ids[c])
                    && graph.neighbors(ids[b]).contains(&ids[c])
                {
                    *counts.entry(ids[a]).or_default() += 1;
                    *counts.entry(ids[b]).or_default() += 1;
                    *counts.entry(ids[c]).or_default() += 1;
                }
            }
        }
    }
    counts
        .into_iter()
        .map(|(node, count)| {
            let degree = graph.neighbors(node).len();
            let coefficient = if degree < 2 {
                0.0
            } else {
                2.0 * count as f64 / (degree * (degree - 1)) as f64
            };
            (node, (count, coefficient))
        })
        .collect()
}

fn all_simple_paths(graph: &RefGraph, source: u64, target: u64) -> Vec<(f64, Vec<u64>)> {
    fn visit(
        graph: &RefGraph,
        node: u64,
        target: u64,
        path: &mut Vec<u64>,
        cost: f64,
        output: &mut Vec<(f64, Vec<u64>)>,
    ) {
        if node == target {
            output.push((cost, path.clone()));
            return;
        }
        for next in graph.outgoing(node) {
            if path.contains(&next) {
                continue;
            }
            let Some(weight) = graph.edges.get(&(node, next)).copied() else {
                continue;
            };
            path.push(next);
            visit(graph, next, target, path, cost + weight, output);
            path.pop();
        }
    }
    let mut output = Vec::new();
    if graph.nodes.contains(&source) && graph.nodes.contains(&target) {
        visit(graph, source, target, &mut vec![source], 0.0, &mut output);
    }
    output.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    output
}

fn reference_harmonic(graph: &RefGraph) -> BTreeMap<u64, f64> {
    graph
        .nodes
        .iter()
        .map(|&source| {
            let score = graph
                .nodes
                .iter()
                .filter(|&&target| target != source)
                .filter_map(|&target| all_simple_paths(graph, source, target).first().map(|p| p.0))
                .filter(|distance| *distance > 0.0)
                .map(|distance| 1.0 / distance)
                .sum();
            (source, score)
        })
        .collect()
}

fn reference_similarity(graph: &RefGraph) -> BTreeMap<(u64, u64), f64> {
    let ids = graph.nodes.iter().copied().collect::<Vec<_>>();
    let mut output = BTreeMap::new();
    for (index, &left) in ids.iter().enumerate() {
        for &right in &ids[index + 1..] {
            let a = graph.neighbors(left);
            let b = graph.neighbors(right);
            let union = a.union(&b).count();
            let score = if union == 0 {
                0.0
            } else {
                a.intersection(&b).count() as f64 / union as f64
            };
            output.insert((left, right), score);
        }
    }
    output
}

fn reference_hits(
    graph: &RefGraph,
    iterations: usize,
    tolerance: f64,
) -> BTreeMap<u64, (f64, f64)> {
    let initial = 1.0 / (graph.nodes.len() as f64).sqrt();
    let mut authority = BTreeMap::from_iter(graph.nodes.iter().map(|&id| (id, initial)));
    let mut hub = authority.clone();
    for _ in 0..iterations {
        let mut next_authority = BTreeMap::new();
        for &id in &graph.nodes {
            next_authority.insert(id, graph.incoming(id).iter().map(|n| hub[n]).sum::<f64>());
        }
        normalize(&mut next_authority);
        let mut next_hub = BTreeMap::new();
        for &id in &graph.nodes {
            next_hub.insert(
                id,
                graph
                    .outgoing(id)
                    .iter()
                    .map(|n| next_authority[n])
                    .sum::<f64>(),
            );
        }
        normalize(&mut next_hub);
        let delta = graph
            .nodes
            .iter()
            .map(|id| (next_authority[id] - authority[id]).abs() + (next_hub[id] - hub[id]).abs())
            .sum::<f64>();
        authority = next_authority;
        hub = next_hub;
        if delta <= tolerance {
            break;
        }
    }
    graph
        .nodes
        .iter()
        .map(|&id| (id, (authority[&id], hub[&id])))
        .collect()
}

fn normalize(values: &mut BTreeMap<u64, f64>) {
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

#[test]
fn 固定种子随机小图_g1_g2_与独立reference一致() {
    for seed in 0..24 {
        let graph = RefGraph::random(seed, 7);
        let workspace = graph.workspace();
        assert_eq!(
            strongly_connected_components(&workspace),
            reference_scc(&graph),
            "seed={seed}"
        );
        assert_eq!(k_core(&workspace), reference_k_core(&graph), "seed={seed}");
        assert_eq!(
            articulation_points(&workspace),
            reference_articulation(&graph),
            "seed={seed}"
        );
        let actual = triangle_metrics(&workspace, 100_000).unwrap();
        let expected = reference_triangles(&graph);
        for id in &graph.nodes {
            assert_eq!(actual[id].0, expected[id].0, "seed={seed}, node={id}");
            assert!((actual[id].1 - expected[id].1).abs() < 1e-12);
        }
        let actual_hits = hits(&workspace, 40, 1e-12, 100_000).unwrap();
        let expected_hits = reference_hits(&graph, 40, 1e-12);
        for id in &graph.nodes {
            assert!((actual_hits[id].0 - expected_hits[id].0).abs() < 1e-12);
            assert!((actual_hits[id].1 - expected_hits[id].1).abs() < 1e-12);
        }
    }
}

#[test]
fn 固定种子随机小图_g3_g4_与穷举reference一致() {
    for seed in 100..116 {
        let graph = RefGraph::random(seed, 6);
        let workspace = graph.workspace();
        for source in 1..=6 {
            for target in 1..=6 {
                let expected = all_simple_paths(&graph, source, target);
                let actual = weighted_dijkstra(&workspace, source, target, 100_000).unwrap();
                assert_eq!(
                    actual.as_ref().map(|p| p.nodes.clone()),
                    expected.first().map(|p| p.1.clone())
                );
                let actual_yen =
                    yen_k_shortest_paths(&workspace, source, target, 4, 100_000, 1_000).unwrap();
                let expected_yen = expected.iter().take(4).collect::<Vec<_>>();
                assert_eq!(actual_yen.len(), expected_yen.len());
                for (actual, expected) in actual_yen.iter().zip(expected_yen) {
                    assert_eq!(actual.nodes, expected.1);
                    assert!((actual.cost - expected.0).abs() < 1e-12);
                }
            }
        }
        let actual_harmonic = harmonic_centrality(&workspace, 100_000).unwrap();
        let expected_harmonic = reference_harmonic(&graph);
        for id in &graph.nodes {
            assert!((actual_harmonic[id] - expected_harmonic[id]).abs() < 1e-12);
        }
        let actual_similarity =
            node_similarity(&workspace, 1_000, 0.0, 100_000, 1_000_000).unwrap();
        let expected_similarity = reference_similarity(&graph);
        assert_eq!(actual_similarity.pairs().len(), expected_similarity.len());
        for pair in actual_similarity.pairs() {
            assert!(
                (pair.similarity - expected_similarity[&(pair.left, pair.right)]).abs() < 1e-12
            );
        }
    }
}

#[test]
fn 图算法_metamorphic_性质保持成立() {
    let graph = RefGraph::random(20260905, 7);
    let workspace = graph.workspace();
    let mut isolated = graph.clone();
    isolated.nodes.insert(99);
    let isolated_workspace = isolated.workspace();
    let original_scc = strongly_connected_components(&workspace);
    let extended_scc = strongly_connected_components(&isolated_workspace);
    assert!(
        original_scc
            .iter()
            .all(|(id, component)| extended_scc[id] == *component)
    );
    assert_eq!(extended_scc[&99], 99);
    assert_eq!(
        articulation_points(&workspace),
        articulation_points(&isolated_workspace)
    );

    let triangles = triangle_metrics(&workspace, 100_000).unwrap();
    let extended_triangles = triangle_metrics(&isolated_workspace, 100_000).unwrap();
    assert!(
        triangles
            .iter()
            .all(|(id, value)| extended_triangles[id] == *value)
    );
    assert_eq!(extended_triangles[&99], (0, 0.0));

    let mut scaled = graph.clone();
    for weight in scaled.edges.values_mut() {
        *weight *= 3.0;
    }
    let scaled_workspace = scaled.workspace();
    for source in 1..=7 {
        for target in 1..=7 {
            let original = weighted_dijkstra(&workspace, source, target, 100_000).unwrap();
            let scaled = weighted_dijkstra(&scaled_workspace, source, target, 100_000).unwrap();
            assert_eq!(
                original.as_ref().map(|path| &path.nodes),
                scaled.as_ref().map(|path| &path.nodes)
            );
            if let (Some(original), Some(scaled)) = (original, scaled) {
                assert!((scaled.cost - original.cost * 3.0).abs() < 1e-12);
            }
        }
    }
}
