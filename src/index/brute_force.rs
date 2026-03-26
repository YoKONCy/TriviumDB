use crate::node::{NodeId, SearchHit};
use crate::VectorType;
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
    let mut hits: Vec<SearchHit> = flat_db_vectors
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

    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    hits.truncate(top_k);
    hits
}
