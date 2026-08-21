use crate::node::{NodeId, SearchHit};
use crate::storage::memtable::MemTable;
use std::collections::HashMap;

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
    expand_graph_with_labels(
        db,
        seeds,
        max_depth,
        restart_alpha,
        enable_inverse_inhibition,
        lateral_inhibition_threshold,
        enable_refractory_fatigue,
        diffusion_bias,
        None,
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
) -> Vec<SearchHit> {
    if max_depth == 0 || seeds.is_empty() {
        return seeds;
    }

    let alpha = restart_alpha.clamp(0.0, 1.0);
    let mut seed_distribution = HashMap::<NodeId, f32>::new();
    let seed_mass: f32 = seeds.iter().map(|seed| seed.score.max(0.0)).sum();
    if seed_mass > 0.0 {
        for seed in &seeds {
            seed_distribution.insert(seed.id, seed.score.max(0.0) / seed_mass);
        }
    } else {
        let uniform = 1.0 / seeds.len() as f32;
        for seed in &seeds {
            seed_distribution.insert(seed.id, uniform);
        }
    }

    let mut total_activation = HashMap::<NodeId, f32>::new();
    let mut current_tier = HashMap::<NodeId, f32>::new();
    let mut active_fatigue = Vec::new();
    for seed in &seeds {
        let fatigue_discount = if enable_refractory_fatigue && db.get_fatigue(seed.id) > 0 {
            active_fatigue.push(seed.id);
            0.15
        } else {
            1.0
        };
        let energy = seed.score * fatigue_discount;
        *total_activation.entry(seed.id).or_insert(0.0) += energy;
        *current_tier.entry(seed.id).or_insert(0.0) += energy;
    }

    let bias_scale = diffusion_bias
        .filter(|bias| !bias.is_empty())
        .map(|bias| 1.0 / (bias.len() as f32).sqrt());

    for _ in 0..max_depth {
        let mut next_tier = HashMap::<NodeId, f32>::new();

        for (curr_id, curr_energy) in current_tier {
            if curr_energy <= 0.0 {
                continue;
            }

            let restart_energy = curr_energy * alpha;
            for (&seed_id, &share) in &seed_distribution {
                let injected = restart_energy * share;
                *next_tier.entry(seed_id).or_insert(0.0) += injected;
                *total_activation.entry(seed_id).or_insert(0.0) += injected;
            }

            let spread_budget = curr_energy * (1.0 - alpha);
            if spread_budget <= 0.0 {
                continue;
            }
            let Some(edges) = db.get_edges(curr_id) else {
                // 悬挂节点没有出边，其传播预算回注到个性化种子，避免能量凭空消失。
                for (&seed_id, &share) in &seed_distribution {
                    let injected = spread_budget * share;
                    *next_tier.entry(seed_id).or_insert(0.0) += injected;
                    *total_activation.entry(seed_id).or_insert(0.0) += injected;
                }
                continue;
            };

            let mut weighted_edges = Vec::with_capacity(edges.len());
            let mut normalizer = 0.0f32;
            for edge in edges {
                if expand_labels
                    .is_some_and(|labels| !labels.iter().any(|label| label == &edge.label))
                {
                    continue;
                }
                if !edge.weight.is_finite() || edge.weight == 0.0 {
                    continue;
                }
                let inhibition_factor = if enable_inverse_inhibition {
                    let in_degree = db.get_in_degree(edge.target_id).max(1) as f32;
                    1.0 / in_degree.powf(0.55)
                } else {
                    1.0
                };
                let fatigue_discount = if enable_refractory_fatigue {
                    if db.get_fatigue(edge.target_id) > 0 {
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
                    db.get_vector(edge.target_id)
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
                let raw_magnitude = edge.weight.abs();
                let magnitude =
                    raw_magnitude * inhibition_factor * fatigue_discount * attention_gate;
                if magnitude > 0.0 && magnitude.is_finite() {
                    // 原始边权决定预算份额；门控项仅衰减已分配能量，不能在分母中自我抵消。
                    normalizer += raw_magnitude;
                    weighted_edges.push((edge.target_id, sign, magnitude));
                }
            }

            if normalizer <= 0.0 {
                for (&seed_id, &share) in &seed_distribution {
                    let injected = spread_budget * share;
                    *next_tier.entry(seed_id).or_insert(0.0) += injected;
                    *total_activation.entry(seed_id).or_insert(0.0) += injected;
                }
                continue;
            }

            for (target_id, sign, magnitude) in weighted_edges {
                let transmitted = spread_budget * (magnitude / normalizer) * sign;
                *next_tier.entry(target_id).or_insert(0.0) += transmitted;
                *total_activation.entry(target_id).or_insert(0.0) += transmitted;
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
        if let Some(payload) = db.get_payload(id) {
            expanded_results.push(SearchHit {
                id,
                score,
                payload: payload.clone(),
            });
        }
    }
    expanded_results.sort_by(|a, b| b.score.total_cmp(&a.score));

    if enable_refractory_fatigue {
        active_fatigue.sort_unstable();
        active_fatigue.dedup();
        db.consume_fatigue_batch(&active_fatigue);
        let top_ids: Vec<NodeId> = expanded_results
            .iter()
            .take(15)
            .map(|hit| hit.id)
            .filter(|id| !active_fatigue.contains(id))
            .collect();
        if !top_ids.is_empty() {
            db.mark_fatigued(&top_ids);
        }
    }

    expanded_results
}
