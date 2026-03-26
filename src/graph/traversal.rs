use crate::node::{NodeId, SearchHit};
use crate::storage::memtable::MemTable;
use std::collections::{HashMap, VecDeque};

/// 执行以最初通过向量检索到的“锚点”（Seeds）向外基于权重的图发散
pub fn expand_graph<T: crate::VectorType>(
    db: &MemTable<T>,
    seeds: Vec<SearchHit>,
    max_depth: usize,
) -> Vec<SearchHit> {
    if max_depth == 0 {
        return seeds;
    }

    let mut visited = HashMap::<NodeId, f32>::new();
    let mut queue = VecDeque::new();

    for seed in &seeds {
        visited.insert(seed.id, seed.score);
        queue.push_back((seed.id, seed.score, 0)); // ID, 传播分数, 当前深度
    }

    while let Some((curr_id, curr_score, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }

        if let Some(edges) = db.get_edges(curr_id) {
            for edge in edges {
                // 最简单的 Spreading Activation：上一层特征分 × 边权重
                let new_score = curr_score * edge.weight;
                let target = edge.target_id;

                let old_score = visited.entry(target).or_insert(0.0);
                // 仅保留能传递更大分数的路径
                if new_score > *old_score {
                    *old_score = new_score;
                    queue.push_back((target, new_score, depth + 1));
                }
            }
        }
    }

    // 将扩散出的一整张子网的所有分数按高低返回
    let mut expanded_results = Vec::new();
    for (id, score) in visited {
        if let Some(payload) = db.get_payload(id) {
            expanded_results.push(SearchHit {
                id,
                score,
                payload: payload.clone(),
            });
        }
    }

    expanded_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    expanded_results
}
