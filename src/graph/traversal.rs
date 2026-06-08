use crate::node::{NodeId, SearchHit};
use crate::storage::memtable::MemTable;
use std::collections::HashMap;

/// 执行以最初通过向量检索到的“锚点”（Seeds）向外基于权重的图发散
///
/// # CCSA (Context-Conditioned Spreading Activation)
///
/// 当提供 `diffusion_bias` 时，在每条边的能量传导中施加 attention gate：
/// `gate_j = σ(bias · v_j / √dim)`
/// 扩散优先沿着与 bias 向量语义相近的节点方向传播。
/// 不提供时完全退化为标准 PPR（零开销）。
pub fn expand_graph<T: crate::VectorType>(
    db: &MemTable<T>,
    seeds: Vec<SearchHit>,
    max_depth: usize,
    teleport_alpha: f32,                 // PPR 阻尼因子/回家概率
    enable_inverse_inhibition: bool,     // 是否启用的入度惩罚
    lateral_inhibition_threshold: usize, // 侧向截断阈值，若是 0 则无限制
    enable_refractory_fatigue: bool,     // 是否启用生物不应期（疲劳冷却）
    diffusion_bias: Option<&[f32]>,      // CCSA: 扩散方向偏置向量
) -> Vec<SearchHit> {
    if max_depth == 0 {
        return seeds;
    }

    // `total_activation` 用于沉淀所有节点最终累积到的能量总和
    let mut total_activation = HashMap::<NodeId, f32>::new();

    // `current_tier` 用于保存当前轮次正在向外辐射边界节点的增量能量
    let mut current_tier = HashMap::<NodeId, f32>::new();

    // CCSA: 预计算 bias 维度的平方根，用于缩放点积防止饱和
    let bias_scale = diffusion_bias.map(|b| 1.0 / (b.len() as f32).sqrt());

    // 用于收集在漫游期间真实遇到过并生效削弱的疲劳节点
    let mut active_fatigue = Vec::new();

    for seed in &seeds {
        total_activation.insert(seed.id, seed.score);
        current_tier.insert(seed.id, seed.score);
    }

    // 传播阈值：被强抑制的节点（得分 <= 0.0）会严格切断物理传播路径
    let propagation_threshold = 0.0;

    for _ in 0..max_depth {
        let mut next_tier = HashMap::<NodeId, f32>::new();

        for (curr_id, curr_energy) in current_tier {
            if let Some(edges) = db.get_edges(curr_id) {
                // PPR 阻尼机制：留下一部分能量不传导（或者说是回跳），剩下部分顺着边流淌
                let spread_energy = curr_energy * (1.0 - teleport_alpha).max(0.0);
                if spread_energy <= propagation_threshold {
                    continue;
                }

                for edge in edges {
                    // 反向抑制（边特异性）：基于目标节点的入度构建阻力
                    let inhibition_factor = if enable_inverse_inhibition {
                        let in_degree = db.get_in_degree(edge.target_id).max(1) as f32;
                        // powf(0.55) 代替原有的缓慢 log10，既有效压制黑洞节点，也不会让真重要节点彻底失联
                        1.0 / in_degree.powf(0.55)
                    } else {
                        1.0
                    };

                    // 不应期判定（疲劳状态）：若目标节点处于疲劳期，本次能量传导严重衰减
                    let fatigue_discount = if enable_refractory_fatigue {
                        let target_fatigue = db.get_fatigue(edge.target_id);
                        if target_fatigue > 0 {
                            active_fatigue.push(edge.target_id);
                            0.15 // 遭遇疲劳节点，接下来的单次能量传导衰减 85%
                        } else {
                            1.0
                        }
                    } else {
                        1.0
                    };

                    // CCSA: 计算 attention gate（当提供 diffusion_bias 时）
                    let attention_gate = if let (Some(bias), Some(scale)) = (diffusion_bias, bias_scale) {
                        // 获取目标节点的 embedding，计算 gate = σ(bias · v_j / √dim)
                        if let Some(target_vec) = db.get_vector(edge.target_id) {
                            let dot: f32 = bias.iter()
                                .zip(target_vec.iter())
                                .map(|(b, v)| *b * v.to_f32())
                                .sum();
                            // sigmoid 激活: 将缩放后的点积压缩到 (0, 1)
                            1.0 / (1.0 + (-dot * scale).exp())
                        } else {
                            1.0 // 无法获取向量时不施加门控
                        }
                    } else {
                        1.0 // 未提供 bias 时 gate = 1.0，退化为标准 PPR
                    };

                    // 发散传播的能量片段（含 CCSA attention gate）
                    let transmitted = if edge.label == "inhibition" {
                        // 负面边不仅不贡献，还会扣除能量
                        -(spread_energy * edge.weight * inhibition_factor * fatigue_discount * attention_gate)
                    } else {
                        spread_energy * edge.weight * inhibition_factor * fatigue_discount * attention_gate
                    };

                    // 1. 将收到的片段累加到下一轮发射台
                    *next_tier.entry(edge.target_id).or_insert(0.0) += transmitted;

                    // 2. 将收到的片段沉淀到该节点的最终总得分池里
                    *total_activation.entry(edge.target_id).or_insert(0.0) += transmitted;
                }
            }
        }

        // 阈值守护（The Gatekeeper）：截断被强抑制或自然衰减掉的节点
        next_tier.retain(|_, energy| *energy > propagation_threshold);

        // ===== 侧向抑制 (Lateral Inhibition / Top-K Cutoff) =====
        if lateral_inhibition_threshold > 0 && next_tier.len() > lateral_inhibition_threshold {
            let mut sorted_tier: Vec<(NodeId, f32)> = next_tier.into_iter().collect();
            // 在 Rust 中对于 f32 使用 partial_cmp 排降序
            sorted_tier.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            sorted_tier.truncate(lateral_inhibition_threshold);
            next_tier = sorted_tier.into_iter().collect();
        }

        if next_tier.is_empty() {
            break; // 能量完全衰竭，提前终止图谱漫游
        }
        current_tier = next_tier;
    }

    // 将散发出的一整张子网通过最终沉淀出的得分转化为 SearchHit 返回
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

    // 依总能量从高到低排序返回
    expanded_results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // === 不应期（疲劳）的更新与结算机制 ===
    if enable_refractory_fatigue {
        // 1. 消耗本轮真实触发削弱了的疲劳节点（解除标记）
        //    这些节点已经在本轮受到了 85% 的能量衰减惩罚，不应期债务已偿还。
        if !active_fatigue.is_empty() {
            active_fatigue.sort_unstable();
            active_fatigue.dedup();
            db.consume_fatigue_batch(&active_fatigue);
        }

        // 2. 将本次漫游排位最高的赢家节点（Top 15）打入物理疲劳冷却期
        //    但排除本轮刚消耗掉疲劳的节点——它们已经承受过一轮惩罚，
        //    需要给予一轮"恢复窗口"，否则疲劳将变成永久性的。
        if !expanded_results.is_empty() {
            let top_ids: Vec<NodeId> = expanded_results
                .iter()
                .take(15)
                .map(|h| h.id)
                .filter(|id| !active_fatigue.contains(id))
                .collect();
            if !top_ids.is_empty() {
                db.mark_fatigued(&top_ids);
            }
        }
    }

    expanded_results
}
