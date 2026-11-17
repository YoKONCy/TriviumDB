//! SoA 向量池上的精确 BruteForce Top-K 基线。
//!
//! 实现对活跃 slot 并行评分并用有界堆控制额外内存，是小规模默认路径、ANN fallback
//! 以及质量 ground truth。最终按 score 降序、NodeId 升序稳定排序，确保线程数不改结果。

use crate::VectorType;
use crate::node::{NodeId, SearchHit};
use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

#[derive(Clone, Copy)]
struct Candidate {
    id: NodeId,
    score: f32,
}
impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then_with(|| self.id.cmp(&other.id))
    }
}
fn retain_best(heap: &mut BinaryHeap<Candidate>, candidate: Candidate, k: usize) {
    if heap.len() < k {
        heap.push(candidate);
    } else if heap.peek().is_some_and(|worst| candidate < *worst) {
        heap.pop();
        heap.push(candidate);
    }
}
fn finish(heap: BinaryHeap<Candidate>) -> Vec<SearchHit> {
    let mut hits = heap
        .into_iter()
        .map(|c| SearchHit {
            id: c.id,
            score: c.score,
            payload: serde_json::Value::Null,
        })
        .collect::<Vec<_>>();
    hits.sort_unstable_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    hits
}

/// 并行分片只保留各自的 Top-K，合并时继续使用同一 NodeId 同分规则。
pub fn search<T: VectorType>(
    query: &[T],
    flat_db_vectors: &[T],
    dim: usize,
    top_k: usize,
    min_score: f32,
    id_mapper: impl Fn(usize) -> NodeId + Sync,
) -> Vec<SearchHit> {
    search_filter_map_segments(query, flat_db_vectors, &[], dim, top_k, min_score, |slot| {
        Some(id_mapper(slot))
    })
}

pub(crate) fn search_filter_map<T: VectorType>(
    query: &[T],
    flat_db_vectors: &[T],
    dim: usize,
    top_k: usize,
    min_score: f32,
    id_filter_map: impl Fn(usize) -> Option<NodeId> + Sync,
) -> Vec<SearchHit> {
    search_filter_map_segments(
        query,
        flat_db_vectors,
        &[],
        dim,
        top_k,
        min_score,
        id_filter_map,
    )
}

/// 分段扫描共享全局 slot 顺序，避免把 mmap 与增量层复制为连续数组。
#[allow(clippy::too_many_arguments)]
pub(crate) fn search_filter_map_segments<T: VectorType>(
    query: &[T],
    base: &[T],
    delta: &[T],
    dim: usize,
    top_k: usize,
    min_score: f32,
    id_filter_map: impl Fn(usize) -> Option<NodeId> + Sync,
) -> Vec<SearchHit> {
    if dim == 0
        || top_k == 0
        || query.len() != dim
        || !base.len().is_multiple_of(dim)
        || !delta.len().is_multiple_of(dim)
        || query.iter().any(|v| !v.to_f32().is_finite())
    {
        return Vec::new();
    }
    let heap = base
        .par_chunks(dim)
        .chain(delta.par_chunks(dim))
        .enumerate()
        .filter_map(|(i, vector)| {
            let id = id_filter_map(i)?;
            let score = T::similarity(query, vector);
            (score.is_finite() && score >= min_score).then_some(Candidate { id, score })
        })
        .fold(BinaryHeap::new, |mut heap, candidate| {
            retain_best(&mut heap, candidate, top_k);
            heap
        })
        .reduce(BinaryHeap::new, |mut left, right| {
            for candidate in right {
                retain_best(&mut left, candidate, top_k);
            }
            left
        });
    finish(heap)
}
