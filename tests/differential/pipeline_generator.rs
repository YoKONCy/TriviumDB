//! 类型正确的 TQL Pipeline case 生成、reference 执行、缩减和 replay。
//!
//! 本模块生成声明式阶段而非随机字符串；打印出的 TQL 必须经过正式 Parser 与 Executor。
//! Reference 使用测试侧图模型和穷举路径，不调用生产 Pipeline 或图算法。

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use triviumdb::Database;
use triviumdb::query::tql_executor::TqlValue;

#[derive(Debug, Clone, Serialize)]
enum PipelineStageCase {
    Union(Vec<u64>),
    Intersect(Vec<u64>),
    Except(Vec<u64>),
    ShortestPath {
        target: u64,
        label: Option<String>,
    },
    WeightedPath {
        target: u64,
        label: Option<String>,
    },
    YenPaths {
        target: u64,
        k: usize,
        label: Option<String>,
    },
    NodeSimilarity {
        top_k: usize,
        cutoff: f64,
        label: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
struct PipelineCase {
    seed_ids: Vec<u64>,
    stage: PipelineStageCase,
}

#[derive(Debug, Clone)]
struct Edge {
    source: u64,
    target: u64,
    label: String,
    weight: f64,
}

fn generated_cases(seed: u64, count: usize) -> Vec<PipelineCase> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut output = Vec::with_capacity(count);
    for _ in 0..count {
        let mut seed_ids = (1..=8).filter(|_| rng.gen_bool(0.35)).collect::<Vec<_>>();
        if seed_ids.is_empty() {
            seed_ids.push(rng.gen_range(1..=8));
        }
        let ids = (1..=8).filter(|_| rng.gen_bool(0.5)).collect::<Vec<_>>();
        let label = rng.gen_bool(0.5).then(|| "road".to_owned());
        let stage = match rng.gen_range(0..7) {
            0 => PipelineStageCase::Union(ids),
            1 => PipelineStageCase::Intersect(ids),
            2 => PipelineStageCase::Except(ids),
            3 => PipelineStageCase::ShortestPath {
                target: rng.gen_range(1..=8),
                label,
            },
            4 => PipelineStageCase::WeightedPath {
                target: rng.gen_range(1..=8),
                label,
            },
            5 => PipelineStageCase::YenPaths {
                target: rng.gen_range(1..=8),
                k: rng.gen_range(1..=4),
                label,
            },
            _ => PipelineStageCase::NodeSimilarity {
                top_k: rng.gen_range(1..=8),
                cutoff: [0.0, 0.25, 0.5][rng.gen_range(0..3)],
                label,
            },
        };
        output.push(PipelineCase { seed_ids, stage });
    }
    output
}

fn ids(ids: &[u64]) -> String {
    ids.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn label_clause(label: &Option<String>) -> String {
    label
        .as_ref()
        .map_or_else(String::new, |label| format!(" LABEL {label}"))
}

fn tql(case: &PipelineCase) -> String {
    let prefix = format!(
        "FIND {{id: {{$in: [{}]}}}} AS seed WITH seed ",
        ids(&case.seed_ids)
    );
    match &case.stage {
        PipelineStageCase::Union(other) => format!(
            "{prefix}UNION seed IDS [{}] AS output WITH output RETURN output",
            ids(other)
        ),
        PipelineStageCase::Intersect(other) => format!(
            "{prefix}INTERSECT seed IDS [{}] AS output WITH output RETURN output",
            ids(other)
        ),
        PipelineStageCase::Except(other) => format!(
            "{prefix}EXCEPT seed IDS [{}] AS output WITH output RETURN output",
            ids(other)
        ),
        PipelineStageCase::ShortestPath { target, label } => format!(
            "{prefix}SHORTEST_PATHS seed TO [{target}]{} AS output WITH output RETURN path(output) AS path",
            label_clause(label)
        ),
        PipelineStageCase::WeightedPath { target, label } => format!(
            "{prefix}WEIGHTED_PATHS seed TO [{target}]{} AS output WITH output RETURN path(output) AS path, weighted_distance(output) AS score",
            label_clause(label)
        ),
        PipelineStageCase::YenPaths { target, k, label } => format!(
            "{prefix}YEN_PATHS seed TO [{target}] K {k}{} AS output WITH output RETURN path(output) AS path, weighted_distance(output) AS score, path_rank(output) AS rank",
            label_clause(label)
        ),
        PipelineStageCase::NodeSimilarity {
            top_k,
            cutoff,
            label,
        } => format!(
            "{prefix}NODE_SIMILARITY seed TOP {top_k} CUTOFF {cutoff}{} AS output WITH output RETURN pair_left(output) AS left, pair_right(output) AS right, node_similarity(output) AS score",
            label_clause(label)
        ),
    }
}

fn neighbors(edges: &[Edge], node: u64, label: Option<&str>, directed: bool) -> BTreeSet<u64> {
    let mut output = BTreeSet::new();
    for edge in edges {
        if label.is_some_and(|label| label != edge.label) {
            continue;
        }
        if edge.source == node {
            output.insert(edge.target);
        }
        if !directed && edge.target == node {
            output.insert(edge.source);
        }
    }
    output
}

fn shortest_unweighted(
    edges: &[Edge],
    source: u64,
    target: u64,
    label: Option<&str>,
) -> Option<Vec<u64>> {
    let mut queue = VecDeque::from([vec![source]]);
    let mut visited = BTreeSet::from([source]);
    while let Some(path) = queue.pop_front() {
        let node = *path.last()?;
        if node == target {
            return Some(path);
        }
        for next in neighbors(edges, node, label, true) {
            if visited.insert(next) {
                let mut candidate = path.clone();
                candidate.push(next);
                queue.push_back(candidate);
            }
        }
    }
    None
}

fn simple_paths(
    edges: &[Edge],
    source: u64,
    target: u64,
    label: Option<&str>,
) -> Vec<(f64, Vec<u64>)> {
    fn visit(
        edges: &[Edge],
        node: u64,
        target: u64,
        label: Option<&str>,
        path: &mut Vec<u64>,
        cost: f64,
        output: &mut Vec<(f64, Vec<u64>)>,
    ) {
        if node == target {
            output.push((cost, path.clone()));
            return;
        }
        let mut outgoing = edges
            .iter()
            .filter(|edge| edge.source == node && label.is_none_or(|label| label == edge.label))
            .collect::<Vec<_>>();
        outgoing.sort_by_key(|edge| edge.target);
        for edge in outgoing {
            if path.contains(&edge.target) {
                continue;
            }
            path.push(edge.target);
            visit(
                edges,
                edge.target,
                target,
                label,
                path,
                cost + edge.weight,
                output,
            );
            path.pop();
        }
    }
    let mut output = Vec::new();
    visit(
        edges,
        source,
        target,
        label,
        &mut vec![source],
        0.0,
        &mut output,
    );
    output.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    output
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum RefCell {
    Node(u64),
    Path(Vec<u64>),
    Int(i64),
    Float(u64),
}

type RefRow = BTreeMap<String, RefCell>;

fn reference(case: &PipelineCase, edges: &[Edge]) -> Vec<RefRow> {
    match &case.stage {
        PipelineStageCase::Union(other) => case
            .seed_ids
            .iter()
            .chain(other)
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|id| BTreeMap::from([("output".into(), RefCell::Node(id))]))
            .collect(),
        PipelineStageCase::Intersect(other) => case
            .seed_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .intersection(&other.iter().copied().collect())
            .copied()
            .map(|id| BTreeMap::from([("output".into(), RefCell::Node(id))]))
            .collect(),
        PipelineStageCase::Except(other) => case
            .seed_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .difference(&other.iter().copied().collect())
            .copied()
            .map(|id| BTreeMap::from([("output".into(), RefCell::Node(id))]))
            .collect(),
        PipelineStageCase::ShortestPath { target, label } => {
            let path = case
                .seed_ids
                .iter()
                .filter_map(|&source| shortest_unweighted(edges, source, *target, label.as_deref()))
                .min_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));
            path.into_iter()
                .map(|path| BTreeMap::from([("path".into(), RefCell::Path(path))]))
                .collect()
        }
        PipelineStageCase::WeightedPath { target, label } => {
            let selected = case
                .seed_ids
                .iter()
                .filter_map(|&source| {
                    simple_paths(edges, source, *target, label.as_deref())
                        .into_iter()
                        .next()
                })
                .reduce(|left, right| {
                    if right.0 < left.0 || right.0 == left.0 && right.1 < left.1 {
                        right
                    } else {
                        left
                    }
                });
            selected
                .into_iter()
                .map(|(cost, path)| {
                    BTreeMap::from([
                        ("path".into(), RefCell::Path(path)),
                        (
                            "score".into(),
                            RefCell::Float(f64::from(cost as f32).to_bits()),
                        ),
                    ])
                })
                .collect()
        }
        PipelineStageCase::YenPaths { target, k, label } => {
            let mut output = Vec::new();
            for &source in &case.seed_ids {
                for (index, (cost, path)) in simple_paths(edges, source, *target, label.as_deref())
                    .into_iter()
                    .take(*k)
                    .enumerate()
                {
                    output.push(BTreeMap::from([
                        ("path".into(), RefCell::Path(path)),
                        (
                            "score".into(),
                            RefCell::Float(f64::from(cost as f32).to_bits()),
                        ),
                        ("rank".into(), RefCell::Int(index as i64 + 1)),
                    ]));
                }
            }
            output
        }
        PipelineStageCase::NodeSimilarity {
            top_k,
            cutoff,
            label,
        } => {
            let seed_set = case.seed_ids.iter().copied().collect::<BTreeSet<_>>();
            let induced_edges = edges
                .iter()
                .filter(|edge| seed_set.contains(&edge.source) && seed_set.contains(&edge.target))
                .cloned()
                .collect::<Vec<_>>();
            let mut pairs = Vec::new();
            for (index, &left) in case.seed_ids.iter().enumerate() {
                for &right in &case.seed_ids[index + 1..] {
                    let a = neighbors(&induced_edges, left, label.as_deref(), false);
                    let b = neighbors(&induced_edges, right, label.as_deref(), false);
                    let union = a.union(&b).count();
                    let score = if union == 0 {
                        0.0
                    } else {
                        a.intersection(&b).count() as f64 / union as f64
                    };
                    if score >= *cutoff {
                        pairs.push((score, left, right));
                    }
                }
            }
            pairs.sort_by(|a, b| {
                b.0.total_cmp(&a.0)
                    .then_with(|| a.1.cmp(&b.1))
                    .then_with(|| a.2.cmp(&b.2))
            });
            pairs.truncate(*top_k);
            pairs
                .into_iter()
                .map(|(score, left, right)| {
                    BTreeMap::from([
                        ("left".into(), RefCell::Int(left as i64)),
                        ("right".into(), RefCell::Int(right as i64)),
                        (
                            "score".into(),
                            RefCell::Float(f64::from(score as f32).to_bits()),
                        ),
                    ])
                })
                .collect()
        }
    }
}

fn actual(database: &Database<f32>, query: &str) -> Vec<RefRow> {
    let mut output = database
        .tql_values(query)
        .unwrap_or_else(|error| panic!("TQL 执行失败: {query}\n{error}"))
        .into_iter()
        .map(|row| {
            row.into_iter()
                .filter_map(|(key, value)| {
                    let value = match value {
                        TqlValue::Node(node) => RefCell::Node(node.id),
                        TqlValue::Path(path) => RefCell::Path(path),
                        TqlValue::Int(value) => RefCell::Int(value),
                        TqlValue::Float(value) => RefCell::Float(value.to_bits()),
                        _ => return None,
                    };
                    Some((key, value))
                })
                .collect()
        })
        .collect::<Vec<_>>();
    output.sort();
    output
}

fn shrink(case: &PipelineCase) -> Vec<PipelineCase> {
    let mut output = Vec::new();
    if case.seed_ids.len() > 1 {
        output.push(PipelineCase {
            seed_ids: case.seed_ids[..case.seed_ids.len() / 2].to_vec(),
            stage: case.stage.clone(),
        });
    }
    match &case.stage {
        PipelineStageCase::YenPaths { target, k, label } if *k > 1 => output.push(PipelineCase {
            seed_ids: case.seed_ids.clone(),
            stage: PipelineStageCase::YenPaths {
                target: *target,
                k: 1,
                label: label.clone(),
            },
        }),
        PipelineStageCase::NodeSimilarity {
            top_k,
            cutoff: _,
            label,
        } => output.push(PipelineCase {
            seed_ids: case.seed_ids.clone(),
            stage: PipelineStageCase::NodeSimilarity {
                top_k: 1.min(*top_k),
                cutoff: 0.0,
                label: label.clone(),
            },
        }),
        _ => {}
    }
    output
}

fn fixture() -> (Database<f32>, Vec<Edge>, String) {
    let root = std::env::temp_dir().join("triviumdb_pipeline_generator");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("database.tdb").to_string_lossy().to_string();
    super::matrix::cleanup(&path);
    let mut database = Database::<f32>::open(&path, 2).unwrap();
    for id in 1..=8 {
        database
            .insert_with_id(id, &[id as f32, 1.0], serde_json::json!({"id": id}))
            .unwrap();
    }
    let edges = vec![
        Edge {
            source: 1,
            target: 2,
            label: "road".into(),
            weight: 1.0,
        },
        Edge {
            source: 1,
            target: 3,
            label: "road".into(),
            weight: 1.0,
        },
        Edge {
            source: 2,
            target: 4,
            label: "road".into(),
            weight: 1.0,
        },
        Edge {
            source: 3,
            target: 4,
            label: "road".into(),
            weight: 1.0,
        },
        Edge {
            source: 2,
            target: 3,
            label: "alt".into(),
            weight: 0.5,
        },
        Edge {
            source: 4,
            target: 5,
            label: "road".into(),
            weight: 2.0,
        },
        Edge {
            source: 6,
            target: 2,
            label: "road".into(),
            weight: 1.0,
        },
        Edge {
            source: 7,
            target: 2,
            label: "road".into(),
            weight: 1.0,
        },
        Edge {
            source: 7,
            target: 3,
            label: "road".into(),
            weight: 1.0,
        },
    ];
    for edge in &edges {
        database
            .link(edge.source, edge.target, &edge.label, edge.weight as f32)
            .unwrap();
    }
    (database, edges, path)
}

#[test]
fn 类型正确_pipeline_generator_与独立reference逐案一致() {
    let (database, edges, path) = fixture();
    for (index, case) in generated_cases(0x5451_4C32, 160).into_iter().enumerate() {
        let query = tql(&case);
        let mut expected = reference(&case, &edges);
        expected.sort();
        let observed = actual(&database, &query);
        assert_eq!(
            observed,
            expected,
            "case={index}\nquery={query}\nreplay={}",
            serde_json::to_string(&case).unwrap()
        );
    }
    drop(database);
    super::matrix::cleanup(&path);
}

#[test]
fn pipeline_case_打印稳定_缩减单调且replay完整() {
    let cases = generated_cases(7, 32);
    for case in &cases {
        let first = tql(case);
        let second = tql(case);
        assert_eq!(first, second);
        let replay = serde_json::to_string(case).unwrap();
        assert!(replay.contains("seed_ids"));
        for smaller in shrink(case) {
            assert!(smaller.seed_ids.len() <= case.seed_ids.len());
            assert!(!tql(&smaller).is_empty());
        }
    }
}
