use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use triviumdb::graph::budget::{BudgetExhaustionPolicy, TraversalBudget};
use triviumdb::graph::pathfinding::BoundedPathConfig;
use triviumdb::graph::reachability::ReachabilityDirection;
use triviumdb::graph::subset::{LabelPropagationConfig, SubsetPageRankConfig};
use triviumdb::query::pipeline::{
    BoundedAllPaths, BoundedIterate, DegreeCentralityOperator, ExactRerank, Expand, GraphMetric,
    GraphSubsetMode, LabelPropagationOperator, NodeSet, PageRankOperator, PathStrengthAggregation,
    PipelineBudget, PipelineContext, PipelineOperator, PropertyLookup, SaPprOperator, ScoreKind,
    SetOperation, WccOperator, combine_sets,
};
use triviumdb::storage::memtable::MemTable;

fn graph() -> MemTable<f32> {
    let mut mt = MemTable::new(8);
    for id in 1..=40u64 {
        let mut vector = vec![0.0; 8];
        vector[id as usize % 8] = 1.0;
        mt.insert_with_id(id, &vector, json!({"active": id % 2 == 0, "group": id % 4}))
            .unwrap();
    }
    mt.register_property_index("active");
    for id in 1..40u64 {
        mt.link(id, id + 1, "next".into(), 0.5).unwrap();
        if id + 4 <= 40 {
            mt.link(id, id + 4, "related".into(), 0.25).unwrap();
        }
    }
    mt
}

fn budget() -> PipelineBudget {
    PipelineBudget {
        max_stages: 64,
        max_nodes: 10_000,
        max_node_set_bytes: 32 * 1024 * 1024,
        max_vector_read_bytes: 32 * 1024 * 1024,
        max_payload_lookups: 10_000,
        max_payload_parsed_bytes: 32 * 1024 * 1024,
        traversal: TraversalBudget {
            max_visited_nodes: 10_000,
            max_examined_edges: 100_000,
            max_frontier_size: 10_000,
            max_depth: 16,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        },
        parallelism: Default::default(),
    }
}

fn apply<O: PipelineOperator<f32>>(mt: &MemTable<f32>, input: NodeSet, operator: O) -> NodeSet {
    operator
        .apply(input, &mut PipelineContext::new(mt, budget()))
        .unwrap()
}

fn materialize(set: NodeSet) -> NodeSet {
    NodeSet::from_rows(set.into_rows())
}

fn expand(mt: &MemTable<f32>, input: NodeSet) -> NodeSet {
    apply(
        mt,
        input,
        Expand {
            min_depth: 1,
            max_depth: 2,
            labels: None,
            direction: ReachabilityDirection::Outgoing,
            include_input: true,
        },
    )
}

fn rerank(mt: &MemTable<f32>, input: NodeSet, top_k: usize) -> NodeSet {
    apply(
        mt,
        input,
        ExactRerank {
            query: (1..=8).map(|value| value as f32).collect(),
            top_k: Some(top_k),
        },
    )
}

fn ids(set: &NodeSet) -> Vec<u64> {
    set.rows().iter().map(|row| row.id).collect()
}

// 独立样例：四十个节点各有 +1 和 +4 两种前向边，边权分别为 1/2 与 1/4。
// 二进制精确权重使同强度路径可直接检验 NodeId 字典序，避免乘法舍入改变同分条件。
// 预期只读取这里的数学定义，不读取 MemTable、产品图算法或向量算子。
fn neighbors(id: u64) -> Vec<(u64, f64)> {
    [(id + 1, 0.5), (id + 4, 0.25)]
        .into_iter()
        .filter(|(target, _)| *target <= 40)
        .collect()
}

fn reference_expand(sources: &[u64], depth: usize) -> BTreeSet<u64> {
    let mut reached = BTreeSet::from_iter(sources.iter().copied());
    let mut frontier = sources.to_vec();
    for _ in 0..depth {
        frontier = frontier
            .into_iter()
            .flat_map(|source| neighbors(source).into_iter().map(|(target, _)| target))
            .collect();
        reached.extend(frontier.iter().copied());
    }
    reached
}

fn reference_rank(candidates: &BTreeSet<u64>, top_k: usize) -> Vec<u64> {
    // 查询为 [1,2,...,8]，平方范数为 204；第 id%8 个单位基向量的余弦可解析求出。
    let mut ranked = candidates.iter().copied().collect::<Vec<_>>();
    ranked.sort_by_key(|id| (std::cmp::Reverse(id % 8), *id));
    ranked.truncate(top_k);
    ranked
}

fn close_score(actual: impl Into<f64>, expected: f64) {
    let actual = actual.into();
    assert!(
        (actual - expected).abs() < 2e-6,
        "分数偏差超限：实际={actual}，预期={expected}"
    );
}

// 直接求解 (I - 0.85 P^T)x = 0.15/n，独立于产品的迭代收敛实现。
// 悬挂节点向全集均匀转移；容差覆盖产品 1e-6 的 L1 收敛条件与 f32 返回值。
fn reference_pagerank(universe: &BTreeSet<u64>) -> BTreeMap<u64, f64> {
    let ids = universe.iter().copied().collect::<Vec<_>>();
    let n = ids.len();
    let mut matrix = vec![vec![0.0; n + 1]; n];
    for (index, row) in matrix.iter_mut().enumerate() {
        row[index] = 1.0;
        row[n] = 0.15 / n as f64;
    }
    for (column, &source) in ids.iter().enumerate() {
        let outgoing = neighbors(source)
            .into_iter()
            .map(|(target, _)| target)
            .filter(|id| universe.contains(id))
            .collect::<BTreeSet<_>>();
        for (row, target) in ids.iter().enumerate() {
            let probability = if outgoing.is_empty() {
                1.0 / n as f64
            } else if outgoing.contains(target) {
                1.0 / outgoing.len() as f64
            } else {
                0.0
            };
            matrix[row][column] -= 0.85 * probability;
        }
    }
    for column in 0..n {
        let pivot = matrix[column][column];
        assert!(pivot.abs() > 1e-12, "独立 PageRank 方程应有唯一解");
        for value in &mut matrix[column][column..=n] {
            *value /= pivot;
        }
        let pivot_row = matrix[column].clone();
        for (index, row) in matrix.iter_mut().enumerate() {
            if index == column {
                continue;
            }
            let scale = row[column];
            for (value, pivot_value) in row[column..].iter_mut().zip(&pivot_row[column..]) {
                *value -= scale * pivot_value;
            }
        }
    }
    ids.into_iter()
        .enumerate()
        .map(|(index, id)| (id, matrix[index][n]))
        .collect()
}

fn reference_labels(universe: &BTreeSet<u64>) -> BTreeMap<u64, u64> {
    // 无向加权邻接矩阵，十六轮同步投票；最大票数同分选择最小原始标签。
    let ids = universe.iter().copied().collect::<Vec<_>>();
    let n = ids.len();
    let mut weights = vec![vec![0.0; n]; n];
    for (i, &source) in ids.iter().enumerate() {
        for (target, weight) in neighbors(source) {
            if let Some(j) = ids.iter().position(|id| *id == target) {
                weights[i][j] += weight;
                weights[j][i] += weight;
            }
        }
    }
    let mut labels = (0..n).collect::<Vec<_>>();
    for _ in 0..16 {
        labels = (0..n)
            .map(|node| {
                let mut votes = vec![0.0; n];
                for neighbor in 0..n {
                    votes[labels[neighbor]] += weights[node][neighbor];
                }
                (0..n)
                    .max_by(|&left, &right| {
                        votes[left]
                            .total_cmp(&votes[right])
                            .then_with(|| right.cmp(&left))
                    })
                    .unwrap()
            })
            .collect();
    }
    let unique = labels
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    ids.into_iter()
        .zip(labels)
        .map(|(id, label)| {
            (
                id,
                unique.iter().position(|value| *value == label).unwrap() as u64 + 1,
            )
        })
        .collect()
}

fn reference_sa_ppr() -> BTreeMap<u64, f64> {
    // 三层线性传播，两个种子初始能量各一；每层注入 0.15 restart，余量按边权分配。
    let mut transfer = vec![vec![0.0; 40]; 40];
    for source in 1..=40 {
        let outgoing = neighbors(source);
        let sum = outgoing.iter().map(|(_, weight)| weight).sum::<f64>();
        transfer[0][(source - 1) as usize] = 0.075;
        transfer[3][(source - 1) as usize] = 0.075;
        if outgoing.is_empty() {
            transfer[0][(source - 1) as usize] += 0.425;
            transfer[3][(source - 1) as usize] += 0.425;
        } else {
            for (target, weight) in outgoing {
                transfer[(target - 1) as usize][(source - 1) as usize] += 0.85 * weight / sum;
            }
        }
    }
    let mut tier = vec![0.0; 40];
    tier[0] = 1.0;
    tier[3] = 1.0;
    let mut total = tier.clone();
    for _ in 0..3 {
        tier = transfer
            .iter()
            .map(|row| row.iter().zip(&tier).map(|(a, b)| a * b).sum())
            .collect();
        for (sum, value) in total.iter_mut().zip(&tier) {
            *sum += value;
        }
    }
    total
        .into_iter()
        .enumerate()
        .filter_map(|(index, score)| {
            let id = index as u64 + 1;
            (id.is_multiple_of(2) && score > 0.0).then_some((id, score))
        })
        .collect()
}

/// 返回直接流水计划与每阶段显式物化计划；两者均执行真实产品算子。
fn execute_family(mt: &MemTable<f32>, family: usize, staged: bool) -> NodeSet {
    let barrier = |set| if staged { materialize(set) } else { set };
    let seeds = NodeSet::from_ids([1, 4]);
    match family {
        1 => expand(mt, seeds),
        2 => rerank(mt, barrier(expand(mt, seeds)), 12),
        3 => rerank(mt, barrier(expand(mt, seeds)), 6),
        4 => {
            let ranked = rerank(mt, NodeSet::from_ids(mt.all_node_ids()), 40);
            let active = apply(
                mt,
                NodeSet::empty(),
                PropertyLookup {
                    field: "active".into(),
                    value: json!(true),
                },
            );
            combine_sets(barrier(ranked), active, SetOperation::Intersect)
        }
        5 => {
            let active = apply(
                mt,
                NodeSet::empty(),
                PropertyLookup {
                    field: "active".into(),
                    value: json!(true),
                },
            );
            rerank(mt, barrier(expand(mt, barrier(active))), 10)
        }
        6 => {
            let expanded = expand(mt, seeds);
            let ranked = apply(
                mt,
                barrier(expanded),
                PageRankOperator {
                    mode: GraphSubsetMode::Induced,
                    config: SubsetPageRankConfig {
                        damping: 0.85,
                        max_iterations: 100,
                        tolerance: 1e-6,
                    },
                    label_filter: None,
                },
            );
            rerank(mt, barrier(ranked), 8)
        }
        7 => {
            let communities = apply(
                mt,
                barrier(expand(mt, seeds)),
                LabelPropagationOperator {
                    mode: GraphSubsetMode::Induced,
                    config: LabelPropagationConfig {
                        max_iterations: 16,
                        min_community_size: 1,
                    },
                    label_filter: None,
                },
            );
            apply(
                mt,
                barrier(communities),
                DegreeCentralityOperator {
                    mode: GraphSubsetMode::Induced,
                    label_filter: None,
                },
            )
        }
        8 => {
            let left = expand(mt, NodeSet::from_ids([1]));
            let right = expand(mt, NodeSet::from_ids([4]));
            rerank(
                mt,
                combine_sets(barrier(left), barrier(right), SetOperation::Intersect),
                8,
            )
        }
        9 => apply(
            mt,
            NodeSet::from_ids([1]),
            BoundedAllPaths {
                targets: vec![8],
                config: BoundedPathConfig {
                    max_depth: 8,
                    max_paths: 32,
                    label_sequence: None,
                    forbidden_nodes: Default::default(),
                },
                aggregation: PathStrengthAggregation::MaxProduct,
            },
        ),
        10 => {
            let expanded = expand(mt, seeds);
            let active = apply(
                mt,
                NodeSet::empty(),
                PropertyLookup {
                    field: "active".into(),
                    value: json!(true),
                },
            );
            let selected = combine_sets(barrier(expanded), active, SetOperation::Intersect);
            apply(
                mt,
                barrier(selected),
                PageRankOperator {
                    mode: GraphSubsetMode::Induced,
                    config: SubsetPageRankConfig {
                        damping: 0.85,
                        max_iterations: 100,
                        tolerance: 1e-6,
                    },
                    label_filter: None,
                },
            )
        }
        11 => {
            let connected = apply(
                mt,
                barrier(expand(mt, seeds)),
                WccOperator {
                    mode: GraphSubsetMode::Induced,
                    label_filter: None,
                },
            );
            rerank(mt, barrier(connected), 8)
        }
        12 => {
            let propagated = apply(
                mt,
                seeds,
                SaPprOperator {
                    max_depth: 3,
                    restart_alpha: 0.15,
                    labels: None,
                    max_edges_per_node: 16,
                    min_edge_weight: 0.0,
                },
            );
            let active = apply(
                mt,
                NodeSet::empty(),
                PropertyLookup {
                    field: "active".into(),
                    value: json!(true),
                },
            );
            combine_sets(barrier(propagated), active, SetOperation::Intersect)
        }
        13 => {
            let active = apply(
                mt,
                NodeSet::empty(),
                PropertyLookup {
                    field: "active".into(),
                    value: json!(true),
                },
            );
            let communities = apply(
                mt,
                barrier(active),
                LabelPropagationOperator {
                    mode: GraphSubsetMode::Induced,
                    config: LabelPropagationConfig {
                        max_iterations: 8,
                        min_community_size: 1,
                    },
                    label_filter: None,
                },
            );
            expand(mt, barrier(communities))
        }
        14 => apply(
            mt,
            NodeSet::from_ids([1]),
            BoundedIterate {
                operators: vec![Box::new(Expand {
                    min_depth: 1,
                    max_depth: 1,
                    labels: None,
                    direction: ReachabilityDirection::Outgoing,
                    include_input: true,
                })],
                max_iterations: 4,
                stop_on_fixed_point: true,
            },
        ),
        _ => unreachable!(),
    }
}

fn reference_degree(universe: &BTreeSet<u64>, id: u64) -> usize {
    universe
        .iter()
        .copied()
        .filter(|&other| neighbors(other).iter().any(|(target, _)| *target == id))
        .count()
        + neighbors(id)
            .iter()
            .filter(|(target, _)| universe.contains(target))
            .count()
}

fn assert_reference(family: usize, actual: &NodeSet) {
    let universe = reference_expand(&[1, 4], 2);
    let active = (2..=40).step_by(2).collect::<Vec<_>>();
    let expected_ids = match family {
        1 => universe.iter().copied().collect(),
        7 => {
            let mut ranked = universe.iter().copied().collect::<Vec<_>>();
            ranked.sort_by_key(|id| (std::cmp::Reverse(reference_degree(&universe, *id)), *id));
            ranked
        }
        2 => reference_rank(&universe, 12),
        3 => reference_rank(&universe, 6),
        4 => active.clone(),
        5 => reference_rank(&reference_expand(&active, 2), 10),
        6 | 11 => reference_rank(&universe, 8),
        8 => reference_rank(
            &reference_expand(&[1], 2)
                .intersection(&reference_expand(&[4], 2))
                .copied()
                .collect(),
            8,
        ),
        9 => vec![8],
        10 => universe.iter().copied().filter(|id| id % 2 == 0).collect(),
        12 => reference_sa_ppr().keys().copied().collect(),
        13 => reference_expand(&active, 2).into_iter().collect(),
        14 => reference_expand(&[1], 4).into_iter().collect(),
        _ => unreachable!(),
    };
    // 先验证完整 ID 集合及顺序，空结果、缺行和重复行都必须失败。
    assert_eq!(
        ids(actual),
        expected_ids,
        "Q{family} 结果集合不符合独立样例"
    );
    if [2, 3, 4, 5, 6, 8, 11].contains(&family) {
        for row in actual.rows() {
            let score = row.similarity.expect("精排结果必须包含相似度");
            assert_eq!(score.kind, ScoreKind::Exact);
            close_score(score.value, (row.id % 8 + 1) as f64 / 204.0_f64.sqrt());
        }
    }
    match family {
        6 | 10 => {
            let selected = if family == 6 {
                universe
            } else {
                universe.into_iter().filter(|id| id % 2 == 0).collect()
            };
            let scores = reference_pagerank(&selected);
            for row in actual.rows() {
                let score = row.graph_score.expect("PageRank 结果必须包含分数");
                assert_eq!(score.kind, ScoreKind::Exact);
                close_score(score.value, scores[&row.id]);
                assert_eq!(row.graph_metrics[&GraphMetric::PageRank], score);
            }
        }
        7 => {
            let labels = reference_labels(&universe);
            for row in actual.rows() {
                assert_eq!(row.community_id, Some(labels[&row.id]));
                let degree = reference_degree(&universe, row.id);
                let score = row.graph_score.expect("度中心性结果必须包含分数");
                assert_eq!(score.kind, ScoreKind::Exact);
                close_score(
                    score.value,
                    degree as f64 / (2 * (universe.len() - 1)) as f64,
                );
            }
        }
        9 => {
            // 从 1 到 8：七条 +1 边，或一条 +4 与三条 +1 的四种排列，共五条路径。
            let row = &actual.rows()[0];
            assert_eq!(row.path_count, Some(5));
            assert_eq!(row.path, Some(vec![1, 2, 3, 4, 8]));
            let score = row.path_strength.expect("路径结果必须包含强度");
            assert_eq!(score.kind, ScoreKind::Exact);
            close_score(score.value, 0.25 * 0.5_f64.powi(3));
            assert_eq!(row.provenance.source_ids, vec![1]);
            assert_eq!(row.provenance.min_depth, Some(4));
        }
        11 => {
            // 此诱导子图所有节点经无向边互相连通，统一归入第一个连通分量。
            for row in actual.rows() {
                assert_eq!(row.community_id, Some(1));
            }
        }
        12 => {
            let scores = reference_sa_ppr();
            for row in actual.rows() {
                let score = row.graph_score.expect("SA-PPR 结果必须包含分数");
                assert_eq!(score.kind, ScoreKind::DepthBounded);
                close_score(score.value, scores[&row.id]);
                assert_eq!(row.graph_metrics[&GraphMetric::SaPpr], score);
                assert_eq!(row.provenance.source_ids, vec![1, 4]);
            }
        }
        _ => {}
    }
}

fn check_family(family: usize) {
    let mt = graph();
    let pipelined = execute_family(&mt, family, false);
    let materialized = execute_family(&mt, family, true);
    assert_reference(family, &pipelined);
    assert_reference(family, &materialized);
    assert_eq!(pipelined, materialized, "Q{family} 两个物理计划不一致");
}

macro_rules! family_test {
    ($name:ident, $family:literal) => {
        #[test]
        fn $name() {
            check_family($family);
        }
    };
}

family_test!(q1_两层扩展符合独立可达集合, 1);
family_test!(q2_扩展后精排符合解析余弦与同分顺序, 2);
family_test!(q3_精排截断符合完整TopK集合, 3);
family_test!(q4_精排与属性交集符合样例, 4);
family_test!(q5_属性过滤扩展后精排符合样例, 5);
family_test!(q6_PageRank后精排保留独立方程分数, 6);
family_test!(q7_标签传播与度中心性符合独立矩阵, 7);
family_test!(q8_双路扩展交集精排符合样例, 8);
family_test!(q9_全路径数量强度及路径内容符合枚举, 9);
family_test!(q10_属性交集PageRank符合独立方程, 10);
family_test!(q11_弱连通分量精排返回完整集合与归属, 11);
family_test!(q12_SA_PPR属性交集符合三层能量传播, 12);
family_test!(q13_社区结果扩展符合独立可达集合, 13);
family_test!(q14_四轮定点扩展符合独立可达集合, 14);
