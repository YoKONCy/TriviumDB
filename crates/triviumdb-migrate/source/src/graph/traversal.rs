//! 面向检索的有限深度 SA-PPR 图扩散。
//!
//! 算法从向量/文本锚点传播相关性能量，并支持方向、Label、弱边过滤、高出度截断、
//! 入度抑制和不应期。它不会迭代到 PageRank 收敛：每层都按绝对边权归一化并注入
//! restart 能量，因此深度和扫描成本可预测；排序以 score 和 NodeId 稳定归并。

use crate::graph::view::GraphRead;
use crate::node::{NodeId, SearchHit};
use crate::storage::memtable::MemTable;
use std::collections::BTreeMap;

use crate::database::EdgeDirection;
use crate::graph::budget::TraversalMetrics;

struct ExpansionEdge<'a> {
    target_id: NodeId,
    label: &'a str,
    weight: f32,
}

/// 执行有限深度的 SA-PPR（Spreading Activation with Personalized Restart）。
///
/// 该算法保留有限深度 spreading activation，不迭代到 PageRank 收敛：
/// - 每层将 `restart_alpha` 比例的能量按初始种子分布重新注入；
/// - 剩余能量按有效出边权重的绝对值归一化，保证单层传播能量不被出度放大；
/// - `inhibition` 边携带负能量，但与普通边共享同一绝对能量预算；
/// - CCSA、入度抑制和疲劳都在归一化前进入有效边权重。
pub fn expand_graph<T: crate::VectorType>(
    db: &MemTable<T>,
    seeds: Vec<SearchHit>,
    max_depth: usize,
    restart_alpha: f32,
    enable_inverse_inhibition: bool,
    lateral_inhibition_threshold: usize,
    enable_refractory_fatigue: bool,
    diffusion_bias: Option<&[f32]>,
) -> Vec<SearchHit> {
    expand_graph_view(
        db,
        seeds,
        max_depth,
        restart_alpha,
        enable_inverse_inhibition,
        lateral_inhibition_threshold,
        enable_refractory_fatigue,
        diffusion_bias,
    )
}

pub(crate) fn expand_graph_view<T: crate::VectorType>(
    db: &impl GraphRead<T>,
    seeds: Vec<SearchHit>,
    max_depth: usize,
    restart_alpha: f32,
    enable_inverse_inhibition: bool,
    lateral_inhibition_threshold: usize,
    enable_refractory_fatigue: bool,
    diffusion_bias: Option<&[f32]>,
) -> Vec<SearchHit> {
    expand_graph_with_labels_view(
        db,
        seeds,
        max_depth,
        restart_alpha,
        enable_inverse_inhibition,
        lateral_inhibition_threshold,
        enable_refractory_fatigue,
        diffusion_bias,
        None,
        0,
        0.0,
        EdgeDirection::Outgoing,
    )
}

/// 执行可按标签限制的有限深度 SA-PPR。
pub fn expand_graph_with_labels<T: crate::VectorType>(
    db: &MemTable<T>,
    seeds: Vec<SearchHit>,
    max_depth: usize,
    restart_alpha: f32,
    enable_inverse_inhibition: bool,
    lateral_inhibition_threshold: usize,
    enable_refractory_fatigue: bool,
    diffusion_bias: Option<&[f32]>,
    expand_labels: Option<&[String]>,
    max_edges_per_node: usize,
    min_edge_weight: f32,
    edge_direction: EdgeDirection,
) -> Vec<SearchHit> {
    expand_graph_with_labels_view(
        db,
        seeds,
        max_depth,
        restart_alpha,
        enable_inverse_inhibition,
        lateral_inhibition_threshold,
        enable_refractory_fatigue,
        diffusion_bias,
        expand_labels,
        max_edges_per_node,
        min_edge_weight,
        edge_direction,
    )
}

pub(crate) fn expand_graph_with_labels_view<T: crate::VectorType>(
    db: &impl GraphRead<T>,
    seeds: Vec<SearchHit>,
    max_depth: usize,
    restart_alpha: f32,
    enable_inverse_inhibition: bool,
    lateral_inhibition_threshold: usize,
    enable_refractory_fatigue: bool,
    diffusion_bias: Option<&[f32]>,
    expand_labels: Option<&[String]>,
    max_edges_per_node: usize,
    min_edge_weight: f32,
    edge_direction: EdgeDirection,
) -> Vec<SearchHit> {
    if max_depth == 0 || seeds.is_empty() {
        return seeds;
    }
    // 兼容入口使用不可失败的准入器；受预算约束的查询共享同一评分循环。
    let scores = match expand_graph_checked(
        db,
        seeds,
        max_depth,
        restart_alpha,
        enable_inverse_inhibition,
        lateral_inhibition_threshold,
        enable_refractory_fatigue,
        diffusion_bias,
        expand_labels,
        max_edges_per_node,
        min_edge_weight,
        edge_direction,
        |_| Ok::<(), std::convert::Infallible>(()),
    ) {
        Ok(scores) => scores,
        Err(never) => match never {},
    };
    scores
        .into_iter()
        .filter_map(|(id, score)| {
            db.facts().get_payload(id).map(|payload| SearchHit {
                id,
                score,
                payload: (*payload).clone(),
            })
        })
        .collect()
}

fn activate<E>(
    id: NodeId,
    energy: f32,
    total: &mut BTreeMap<NodeId, f32>,
    tier: &mut BTreeMap<NodeId, f32>,
    metrics: &mut TraversalMetrics,
    admit: &mut impl FnMut(TraversalMetrics) -> Result<(), E>,
) -> Result<(), E> {
    if energy == 0.0 {
        return Ok(());
    }
    let projected = TraversalMetrics {
        visited_nodes: total
            .len()
            .saturating_add(usize::from(!total.contains_key(&id))),
        peak_frontier_size: metrics.peak_frontier_size.max(
            tier.len()
                .saturating_add(usize::from(!tier.contains_key(&id))),
        ),
        ..*metrics
    };
    admit(projected)?;
    *metrics = projected;
    *tier.entry(id).or_insert(0.0) += energy;
    *total.entry(id).or_insert(0.0) += energy;
    Ok(())
}

pub(crate) fn expand_graph_checked<T: crate::VectorType, E>(
    db: &impl GraphRead<T>,
    seeds: impl IntoIterator<Item = SearchHit>,
    max_depth: usize,
    restart_alpha: f32,
    enable_inverse_inhibition: bool,
    lateral_inhibition_threshold: usize,
    enable_refractory_fatigue: bool,
    diffusion_bias: Option<&[f32]>,
    expand_labels: Option<&[String]>,
    max_edges_per_node: usize,
    min_edge_weight: f32,
    edge_direction: EdgeDirection,
    mut admit: impl FnMut(TraversalMetrics) -> Result<(), E>,
) -> Result<Vec<(NodeId, f32)>, E> {
    let mut metrics = TraversalMetrics::default();
    admit(metrics)?;

    let alpha = restart_alpha.clamp(0.0, 1.0);
    let mut seed_mass = 0.0f64;
    let mut total_activation = BTreeMap::<NodeId, f32>::new();
    let mut current_tier = BTreeMap::<NodeId, f32>::new();
    let mut seed_distribution = BTreeMap::<NodeId, f64>::new();
    let mut active_fatigue = Vec::new();
    for seed in seeds {
        let projected = TraversalMetrics {
            visited_nodes: total_activation
                .len()
                .saturating_add(usize::from(!total_activation.contains_key(&seed.id))),
            peak_frontier_size: current_tier
                .len()
                .saturating_add(usize::from(!current_tier.contains_key(&seed.id))),
            ..metrics
        };
        admit(projected)?;
        metrics = projected;
        let mass = f64::from(seed.score.max(0.0));
        seed_mass += mass;
        *seed_distribution.entry(seed.id).or_insert(0.0) += mass;
        let fatigue_discount = if enable_refractory_fatigue && db.facts().get_fatigue(seed.id) > 0 {
            active_fatigue.push(seed.id);
            0.15
        } else {
            1.0
        };
        let energy = seed.score * fatigue_discount;
        *total_activation.entry(seed.id).or_insert(0.0) += energy;
        *current_tier.entry(seed.id).or_insert(0.0) += energy;
    }
    let uniform = 1.0 / seed_distribution.len().max(1) as f64;
    for share in seed_distribution.values_mut() {
        *share = if seed_mass > 0.0 {
            *share / seed_mass
        } else {
            uniform
        };
    }

    let bias_scale = diffusion_bias
        .filter(|bias| !bias.is_empty())
        .map(|bias| 1.0 / (bias.len() as f32).sqrt());

    for depth in 0..max_depth {
        if current_tier.is_empty() {
            break;
        }
        metrics.depth_reached = depth.saturating_add(1);
        admit(metrics)?;
        let mut next_tier = BTreeMap::<NodeId, f32>::new();

        for (curr_id, curr_energy) in current_tier {
            admit(metrics)?;
            if curr_energy <= 0.0 {
                continue;
            }

            let restart_energy = curr_energy * alpha;
            for (&seed_id, &share) in &seed_distribution {
                let injected = restart_energy * share as f32;
                activate(
                    seed_id,
                    injected,
                    &mut total_activation,
                    &mut next_tier,
                    &mut metrics,
                    &mut admit,
                )?;
            }

            let spread_budget = curr_energy * (1.0 - alpha);
            if spread_budget <= 0.0 {
                continue;
            }
            let mut edges = Vec::new();
            if matches!(
                edge_direction,
                EdgeDirection::Outgoing | EdgeDirection::Both
            ) {
                for edge in db.get_edges(curr_id).unwrap_or_default() {
                    metrics.examined_edges = metrics.examined_edges.saturating_add(1);
                    admit(metrics)?;
                    edges.push(ExpansionEdge {
                        target_id: edge.target_id,
                        label: &edge.label,
                        weight: edge.weight,
                    });
                }
            }
            if matches!(
                edge_direction,
                EdgeDirection::Incoming | EdgeDirection::Both
            ) {
                for source in db.get_incoming_sources(curr_id) {
                    for edge in db.get_edges(*source).unwrap_or_default() {
                        metrics.examined_edges = metrics.examined_edges.saturating_add(1);
                        admit(metrics)?;
                        if edge.target_id == curr_id {
                            edges.push(ExpansionEdge {
                                target_id: *source,
                                label: &edge.label,
                                weight: edge.weight,
                            });
                        }
                    }
                }
            }
            edges.retain(|edge| {
                edge.weight.is_finite()
                    && edge.weight != 0.0
                    && edge.weight.abs() >= min_edge_weight
                    && expand_labels
                        .is_none_or(|labels| labels.iter().any(|label| label == edge.label))
            });
            edges.sort_by(|left, right| {
                right
                    .weight
                    .abs()
                    .total_cmp(&left.weight.abs())
                    .then_with(|| left.target_id.cmp(&right.target_id))
                    .then_with(|| left.label.cmp(right.label))
            });
            edges.dedup_by(|left, right| {
                left.target_id == right.target_id
                    && left.label == right.label
                    && left.weight.to_bits() == right.weight.to_bits()
            });
            if max_edges_per_node > 0 {
                edges.truncate(max_edges_per_node);
            }
            if edges.is_empty() {
                // 悬挂节点没有出边，其传播预算回注到个性化种子，避免能量凭空消失。
                for (&seed_id, &share) in &seed_distribution {
                    let injected = spread_budget * share as f32;
                    activate(
                        seed_id,
                        injected,
                        &mut total_activation,
                        &mut next_tier,
                        &mut metrics,
                        &mut admit,
                    )?;
                }
                continue;
            }

            let mut weighted_edges = Vec::with_capacity(edges.len());
            let mut normalizer = 0.0f64;
            for edge in edges {
                let inhibition_factor = if enable_inverse_inhibition {
                    let in_degree = db.get_in_degree(edge.target_id).max(1) as f32;
                    1.0 / in_degree.powf(0.55)
                } else {
                    1.0
                };
                let fatigue_discount: f32 = if enable_refractory_fatigue {
                    if db.facts().get_fatigue(edge.target_id) > 0 {
                        active_fatigue.push(edge.target_id);
                        0.15
                    } else {
                        1.0
                    }
                } else {
                    1.0
                };
                let attention_gate = if let (Some(bias), Some(scale)) = (diffusion_bias, bias_scale)
                {
                    db.facts()
                        .get_vector(edge.target_id)
                        .map(|target_vec| {
                            let dot: f32 = bias
                                .iter()
                                .zip(target_vec.iter())
                                .map(|(b, v)| *b * v.to_f32())
                                .sum();
                            1.0 / (1.0 + (-dot * scale).exp())
                        })
                        .unwrap_or(1.0)
                } else {
                    1.0
                };
                let sign = if edge.label == "inhibition" {
                    -1.0
                } else {
                    edge.weight.signum()
                };
                let raw_magnitude = f64::from(edge.weight.abs());
                let magnitude = raw_magnitude
                    * f64::from(inhibition_factor)
                    * f64::from(fatigue_discount)
                    * f64::from(attention_gate);
                if magnitude > 0.0 && magnitude.is_finite() {
                    // 原始边权决定预算份额；门控项仅衰减已分配能量，不能在分母中自我抵消。
                    normalizer += raw_magnitude;
                    weighted_edges.push((edge.target_id, sign, magnitude));
                }
            }

            if normalizer <= 0.0 {
                for (&seed_id, &share) in &seed_distribution {
                    let injected = spread_budget * share as f32;
                    activate(
                        seed_id,
                        injected,
                        &mut total_activation,
                        &mut next_tier,
                        &mut metrics,
                        &mut admit,
                    )?;
                }
                continue;
            }

            for (target_id, sign, magnitude) in weighted_edges {
                let transmitted =
                    (f64::from(spread_budget) * (magnitude / normalizer) * f64::from(sign)) as f32;
                activate(
                    target_id,
                    transmitted,
                    &mut total_activation,
                    &mut next_tier,
                    &mut metrics,
                    &mut admit,
                )?;
            }
        }

        next_tier.retain(|_, energy| *energy > 0.0 && energy.is_finite());
        if lateral_inhibition_threshold > 0 && next_tier.len() > lateral_inhibition_threshold {
            let mut sorted_tier: Vec<(NodeId, f32)> = next_tier.into_iter().collect();
            sorted_tier.sort_by(|a, b| b.1.total_cmp(&a.1));
            sorted_tier.truncate(lateral_inhibition_threshold);
            next_tier = sorted_tier.into_iter().collect();
        }
        if next_tier.is_empty() {
            break;
        }
        current_tier = next_tier;
    }

    let mut expanded_results = Vec::new();
    for (id, score) in total_activation {
        admit(metrics)?;
        if db.contains(id) {
            expanded_results.push((id, score));
        }
    }
    expanded_results.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));

    if enable_refractory_fatigue {
        active_fatigue.sort_unstable();
        active_fatigue.dedup();
        db.facts().consume_fatigue_batch(&active_fatigue);
        let top_ids: Vec<NodeId> = expanded_results
            .iter()
            .take(15)
            .map(|hit| hit.0)
            .filter(|id| !active_fatigue.contains(id))
            .collect();
        if !top_ids.is_empty() {
            db.facts().mark_fatigued(&top_ids);
        }
    }

    Ok(expanded_results)
}
