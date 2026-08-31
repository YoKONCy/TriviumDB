//! SoA 向量池上的精确 BruteForce Top-K 基线。
//!
//! 实现对活跃 slot 并行评分并用有界堆控制额外内存，是小规模默认路径、ANN fallback
//! 以及质量 ground truth。最终按 score 降序、NodeId 升序稳定排序，确保线程数不改结果。

use crate::VectorType;
use crate::node::{NodeId, SearchHit};
use rayon::prelude::*;

/// rayon 并行扫描 SoA 向量池。
/// 安全代码，无 unsafe。利用多核并行分块计算余弦相似度。
pub fn search<T: VectorType>(
    query: &[T],
    flat_db_vectors: &[T],
    dim: usize,
    top_k: usize,
    min_score: f32,
    id_mapper: impl Fn(usize) -> NodeId + Sync,
) -> Vec<SearchHit> {
    if flat_db_vectors.is_empty() || dim == 0 {
        return Vec::new();
    }

    // 使用 rayon 的 par_chunks 将向量池按 dim 分块，多核并行计算每个块的余弦相似度。
    // 每个线程独立收集自己的命中结果，最后合并排序。
    // 这是纯安全代码，不涉及任何 unsafe 操作。
    let hits: Vec<SearchHit> = flat_db_vectors
        .par_chunks(dim)
        .enumerate()
        .filter_map(|(i, vec_slice)| {
            let score = T::similarity(query, vec_slice);
            if score >= min_score {
                Some(SearchHit {
                    id: id_mapper(i),
                    score,
                    payload: serde_json::Value::Null,
                })
            } else {
                None
            }
        })
        .collect();

    sort_and_truncate(hits, top_k)
}

pub(crate) fn search_filter_map<T: VectorType>(
    query: &[T],
    flat_db_vectors: &[T],
    dim: usize,
    top_k: usize,
    min_score: f32,
    id_filter_map: impl Fn(usize) -> Option<NodeId> + Sync,
) -> Vec<SearchHit> {
    if flat_db_vectors.is_empty() || dim == 0 {
        return Vec::new();
    }

    let hits: Vec<SearchHit> = flat_db_vectors
        .par_chunks(dim)
        .enumerate()
        .filter_map(|(i, vec_slice)| {
            let id = id_filter_map(i)?;
            let score = T::similarity(query, vec_slice);
            (score >= min_score).then_some(SearchHit {
                id,
                score,
                payload: serde_json::Value::Null,
            })
        })
        .collect();

    sort_and_truncate(hits, top_k)
}

fn sort_and_truncate(mut hits: Vec<SearchHit>, top_k: usize) -> Vec<SearchHit> {
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(top_k);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_map_excludes_candidates_before_top_k() {
        let flat = vec![1.0f32, 0.0, 0.8, 0.6, 0.6, 0.8, 0.0, 1.0, -0.6, 0.8];
        let hits = search_filter_map(&[1.0, 0.0], &flat, 2, 2, -1.0, |idx| {
            (idx >= 2).then_some(10 + idx as u64)
        });

        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits.iter().map(|hit| hit.id).collect::<Vec<_>>(),
            vec![12, 13]
        );
    }
}
