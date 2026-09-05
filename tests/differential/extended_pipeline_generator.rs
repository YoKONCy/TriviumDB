//! EXPAND、结构图算法、聚合和多阶段 WITH 的类型正确生成式差分。
//!
//! 所有 reference 都直接扫描测试边集合；不调用生产遍历、图算法或 Pipeline。

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use triviumdb::Database;
use triviumdb::query::tql_executor::TqlValue;

#[derive(Debug, Clone, Copy, Serialize)]
enum GraphCaseKind {
    Scc,
    KCore,
    Articulation,
    Triangle,
    Harmonic,
}

#[derive(Debug, Clone, Serialize)]
struct GraphPipelineCase {
    seed_ids: Vec<u64>,
    min_depth: usize,
    max_depth: usize,
    label: Option<String>,
    algorithm: GraphCaseKind,
    aggregate: bool,
}

#[derive(Debug, Clone)]
struct Edge {
    source: u64,
    target: u64,
    label: String,
    weight: f64,
}

fn cases(seed: u64, count: usize) -> Vec<GraphPipelineCase> {
    let mut rng = StdRng::seed_from_u64(seed);
    (0..count)
        .map(|_| {
            let mut seed_ids = (1..=9).filter(|_| rng.gen_bool(0.28)).collect::<Vec<_>>();
            if seed_ids.is_empty() {
                seed_ids.push(rng.gen_range(1..=9));
            }
            let min_depth = rng.gen_range(0..=1);
            let max_depth = rng.gen_range(min_depth.max(1)..=3);
            GraphPipelineCase {
                seed_ids,
                min_depth,
                max_depth,
                label: Some("road".into()),
                algorithm: match rng.gen_range(0..5) {
                    0 => GraphCaseKind::Scc,
                    1 => GraphCaseKind::KCore,
                    2 => GraphCaseKind::Articulation,
                    3 => GraphCaseKind::Triangle,
                    _ => GraphCaseKind::Harmonic,
                },
                aggregate: rng.gen_bool(0.5),
            }
        })
        .collect()
}

fn list(ids: &[u64]) -> String {
    ids.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn scalar(kind: GraphCaseKind) -> &'static str {
    match kind {
        GraphCaseKind::Scc => "community(scored)",
        GraphCaseKind::KCore => "core_number(scored)",
        GraphCaseKind::Articulation => "scored.id",
        GraphCaseKind::Triangle => "triangle_count(scored)",
        GraphCaseKind::Harmonic => "harmonic_centrality(scored)",
    }
}

fn algorithm(kind: GraphCaseKind) -> &'static str {
    match kind {
        GraphCaseKind::Scc => "scc",
        GraphCaseKind::KCore => "k_core",
        GraphCaseKind::Articulation => "articulation_points",
        GraphCaseKind::Triangle => "triangle_count",
        GraphCaseKind::Harmonic => "harmonic_centrality",
    }
}

fn tql(case: &GraphPipelineCase) -> String {
    let relation = case.label.as_ref().map_or_else(
        || format!("[*{}..{}]", case.min_depth, case.max_depth),
        |label| format!("[:{label}*{}..{}]", case.min_depth, case.max_depth),
    );
    let prefix = format!(
        "FIND {{id: {{$in: [{}]}}}} AS seed WITH seed EXPAND seed {relation} AS reached WITH reached {} reached{} AS scored WITH scored ",
        list(&case.seed_ids),
        algorithm(case.algorithm),
        case.label
            .as_ref()
            .map_or_else(String::new, |label| format!(" LABEL {label}")),
    );
    if case.aggregate {
        format!(
            "{prefix}RETURN count(*) AS total, sum({}) AS sum_value, min({}) AS min_value, max({}) AS max_value, collect(scored.id) AS ids",
            scalar(case.algorithm),
            scalar(case.algorithm),
            scalar(case.algorithm),
        )
    } else {
        format!(
            "{prefix}RETURN scored, {} AS value ORDER BY scored.id ASC",
            scalar(case.algorithm)
        )
    }
}

fn outgoing(edges: &[Edge], node: u64, label: Option<&str>) -> BTreeSet<u64> {
    edges
        .iter()
        .filter(|edge| edge.source == node && label.is_none_or(|label| label == edge.label))
        .map(|edge| edge.target)
        .collect()
}

fn expand(case: &GraphPipelineCase, edges: &[Edge]) -> BTreeSet<u64> {
    let mut output = BTreeSet::new();
    for &seed in &case.seed_ids {
        let mut best_depth = BTreeMap::from([(seed, 0usize)]);
        let mut queue = VecDeque::from([(seed, 0usize)]);
        while let Some((node, depth)) = queue.pop_front() {
            if depth >= case.min_depth {
                output.insert(node);
            }
            if depth == case.max_depth {
                continue;
            }
            for next in outgoing(edges, node, case.label.as_deref()) {
                if best_depth.get(&next).is_none_or(|known| depth + 1 < *known) {
                    best_depth.insert(next, depth + 1);
                    queue.push_back((next, depth + 1));
                }
            }
        }
    }
    output
}

fn induced_neighbors(nodes: &BTreeSet<u64>, edges: &[Edge], node: u64) -> BTreeSet<u64> {
    let mut output = BTreeSet::new();
    for edge in edges {
        if nodes.contains(&edge.source) && nodes.contains(&edge.target) && edge.label == "road" {
            if edge.source == node && edge.target != node {
                output.insert(edge.target);
            }
            if edge.target == node && edge.source != node {
                output.insert(edge.source);
            }
        }
    }
    output
}

fn directed_reach(nodes: &BTreeSet<u64>, edges: &[Edge], source: u64) -> BTreeSet<u64> {
    let mut reached = BTreeSet::from([source]);
    let mut stack = vec![source];
    while let Some(node) = stack.pop() {
        for edge in edges.iter().filter(|edge| {
            edge.source == node && nodes.contains(&edge.source) && nodes.contains(&edge.target)
        }) {
            if reached.insert(edge.target) {
                stack.push(edge.target);
            }
        }
    }
    reached
}

fn scc(nodes: &BTreeSet<u64>, edges: &[Edge]) -> BTreeMap<u64, f64> {
    let reach = nodes
        .iter()
        .map(|&node| (node, directed_reach(nodes, edges, node)))
        .collect::<BTreeMap<_, _>>();
    nodes
        .iter()
        .map(|&node| {
            let id = nodes
                .iter()
                .filter(|&&other| reach[&node].contains(&other) && reach[&other].contains(&node))
                .copied()
                .min()
                .unwrap_or(node);
            (node, id as f64)
        })
        .collect()
}

fn k_core(nodes: &BTreeSet<u64>, edges: &[Edge]) -> BTreeMap<u64, f64> {
    nodes
        .iter()
        .map(|&node| {
            let mut best = 0;
            for k in 1..=nodes.len() {
                let mut remaining = nodes.clone();
                loop {
                    let remove = remaining
                        .iter()
                        .filter(|&&candidate| {
                            induced_neighbors(nodes, edges, candidate)
                                .intersection(&remaining)
                                .count()
                                < k
                        })
                        .copied()
                        .collect::<Vec<_>>();
                    if remove.is_empty() {
                        break;
                    }
                    for id in remove {
                        remaining.remove(&id);
                    }
                }
                if remaining.contains(&node) {
                    best = k;
                }
            }
            (node, best as f64)
        })
        .collect()
}

fn component_count(nodes: &BTreeSet<u64>, edges: &[Edge], omitted: Option<u64>) -> usize {
    let mut unseen = nodes
        .iter()
        .copied()
        .filter(|node| Some(*node) != omitted)
        .collect::<BTreeSet<_>>();
    let mut components = 0;
    while let Some(start) = unseen.pop_first() {
        components += 1;
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            for next in induced_neighbors(nodes, edges, node) {
                if Some(next) != omitted && unseen.remove(&next) {
                    stack.push(next);
                }
            }
        }
    }
    components
}

fn articulation(nodes: &BTreeSet<u64>, edges: &[Edge]) -> BTreeMap<u64, f64> {
    let baseline = component_count(nodes, edges, None);
    nodes
        .iter()
        .filter(|&&node| component_count(nodes, edges, Some(node)) > baseline)
        .map(|&node| (node, node as f64))
        .collect()
}

fn triangles(nodes: &BTreeSet<u64>, edges: &[Edge]) -> BTreeMap<u64, f64> {
    let ids = nodes.iter().copied().collect::<Vec<_>>();
    let mut counts = BTreeMap::from_iter(ids.iter().map(|&id| (id, 0.0)));
    for a in 0..ids.len() {
        for b in a + 1..ids.len() {
            for c in b + 1..ids.len() {
                if induced_neighbors(nodes, edges, ids[a]).contains(&ids[b])
                    && induced_neighbors(nodes, edges, ids[a]).contains(&ids[c])
                    && induced_neighbors(nodes, edges, ids[b]).contains(&ids[c])
                {
                    *counts.entry(ids[a]).or_default() += 1.0;
                    *counts.entry(ids[b]).or_default() += 1.0;
                    *counts.entry(ids[c]).or_default() += 1.0;
                }
            }
        }
    }
    counts
}

fn shortest_distances(nodes: &BTreeSet<u64>, edges: &[Edge], source: u64) -> BTreeMap<u64, f64> {
    let mut distances = BTreeMap::from_iter(nodes.iter().map(|&id| (id, f64::INFINITY)));
    distances.insert(source, 0.0);
    let mut settled = BTreeSet::new();
    loop {
        let next = distances
            .iter()
            .filter(|(id, distance)| !settled.contains(*id) && distance.is_finite())
            .min_by(|left, right| left.1.total_cmp(right.1).then_with(|| left.0.cmp(right.0)))
            .map(|(&id, &distance)| (id, distance));
        let Some((node, distance)) = next else { break };
        settled.insert(node);
        for edge in edges.iter().filter(|edge| {
            edge.label == "road"
                && edge.source == node
                && nodes.contains(&edge.source)
                && nodes.contains(&edge.target)
        }) {
            let candidate = distance + edge.weight;
            if candidate < distances[&edge.target] {
                distances.insert(edge.target, candidate);
            }
        }
    }
    distances
}

fn harmonic(nodes: &BTreeSet<u64>, edges: &[Edge]) -> BTreeMap<u64, f64> {
    nodes
        .iter()
        .map(|&source| {
            let score = shortest_distances(nodes, edges, source)
                .into_iter()
                .filter(|&(target, distance)| {
                    target != source && distance.is_finite() && distance > 0.0
                })
                .map(|(_, distance)| 1.0 / distance)
                .sum();
            (source, score)
        })
        .collect()
}

fn reference(case: &GraphPipelineCase, edges: &[Edge]) -> BTreeMap<u64, f64> {
    let reached = expand(case, edges);
    match case.algorithm {
        GraphCaseKind::Scc => scc(&reached, edges),
        GraphCaseKind::KCore => k_core(&reached, edges),
        GraphCaseKind::Articulation => articulation(&reached, edges),
        GraphCaseKind::Triangle => triangles(&reached, edges),
        GraphCaseKind::Harmonic => harmonic(&reached, edges),
    }
}

fn numeric(value: &TqlValue<f32>) -> f64 {
    match value {
        TqlValue::Int(value) => *value as f64,
        TqlValue::Float(value) => *value,
        other => panic!("期待数值，实际为 {other:?}"),
    }
}

fn fixture() -> (Database<f32>, Vec<Edge>, String) {
    let root = std::env::temp_dir().join("triviumdb_extended_pipeline_generator");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.tdb").to_string_lossy().to_string();
    super::matrix::cleanup(&path);
    let mut database = Database::<f32>::open(&path, 2).unwrap();
    for id in 1..=9 {
        database
            .insert_with_id(
                id,
                &[id as f32, 1.0],
                serde_json::json!({"id": id, "group": id % 3}),
            )
            .unwrap();
    }
    let edges = [
        (1, 2, "road", 1.0),
        (2, 3, "road", 1.0),
        (3, 1, "road", 1.0),
        (3, 4, "road", 2.0),
        (4, 5, "road", 1.0),
        (5, 6, "road", 1.0),
        (6, 4, "road", 1.0),
        (2, 5, "alt", 0.5),
        (7, 8, "road", 1.0),
        (8, 9, "road", 1.0),
        (7, 9, "road", 1.0),
    ]
    .into_iter()
    .map(|(source, target, label, weight)| Edge {
        source,
        target,
        label: label.into(),
        weight,
    })
    .collect::<Vec<_>>();
    for edge in &edges {
        database
            .link(edge.source, edge.target, &edge.label, edge.weight as f32)
            .unwrap();
    }
    (database, edges, path)
}

#[test]
fn expand_图算法_聚合_多阶段with生成差分() {
    let (database, edges, path) = fixture();
    for (index, case) in cases(0x4558_5041, 180).into_iter().enumerate() {
        let query = tql(&case);
        let expected = reference(&case, &edges);
        let rows = database.tql_values(&query).unwrap_or_else(|error| {
            panic!(
                "case={index}\nquery={query}\nerror={error}\nreplay={}",
                serde_json::to_string(&case).unwrap()
            )
        });
        if case.aggregate {
            assert_eq!(rows.len(), 1, "case={index}\n{query}");
            assert_eq!(
                numeric(&rows[0]["total"]),
                expected.len() as f64,
                "case={index}\nquery={query}\nreplay={}\nexpected={expected:?}\nrows={rows:?}",
                serde_json::to_string(&case).unwrap()
            );
            if expected.is_empty() {
                assert!(matches!(rows[0]["min_value"], TqlValue::Null));
                assert!(matches!(rows[0]["max_value"], TqlValue::Null));
            } else {
                let sum = expected.values().sum::<f64>();
                let min = expected.values().copied().min_by(f64::total_cmp).unwrap();
                let max = expected.values().copied().max_by(f64::total_cmp).unwrap();
                assert!((numeric(&rows[0]["sum_value"]) - sum).abs() < 1e-5);
                assert!((numeric(&rows[0]["min_value"]) - min).abs() < 1e-5);
                assert!((numeric(&rows[0]["max_value"]) - max).abs() < 1e-5);
                let TqlValue::List(ids) = &rows[0]["ids"] else {
                    panic!("collect 应返回 List")
                };
                let actual = ids
                    .iter()
                    .map(|id| id.as_u64().unwrap())
                    .collect::<BTreeSet<_>>();
                assert_eq!(actual, expected.keys().copied().collect());
            }
        } else {
            let actual = rows
                .iter()
                .map(|row| {
                    let TqlValue::Node(node) = &row["scored"] else {
                        panic!("scored 应为 Node")
                    };
                    (node.id, numeric(&row["value"]))
                })
                .collect::<BTreeMap<_, _>>();
            assert_eq!(
                actual.keys().copied().collect::<Vec<_>>(),
                expected.keys().copied().collect::<Vec<_>>(),
                "case={index}\n{query}"
            );
            for (id, value) in expected {
                assert!(
                    (actual[&id] - value).abs() < 1e-5,
                    "case={index}, node={id}\n{query}"
                );
            }
        }
    }
    drop(database);
    super::matrix::cleanup(&path);
}

#[test]
fn 扩展case打印稳定且可单调缩减() {
    for case in cases(7, 40) {
        assert_eq!(tql(&case), tql(&case));
        assert!(serde_json::to_string(&case).unwrap().contains("algorithm"));
        if case.max_depth > 1 {
            let mut smaller = case.clone();
            smaller.max_depth -= 1;
            smaller.min_depth = smaller.min_depth.min(smaller.max_depth);
            assert!(tql(&smaller).len() <= tql(&case).len() + 2);
        }
        if case.seed_ids.len() > 1 {
            let mut smaller = case.clone();
            smaller.seed_ids.truncate(1);
            assert_eq!(smaller.seed_ids.len(), 1);
            assert!(!tql(&smaller).is_empty());
        }
    }
}
