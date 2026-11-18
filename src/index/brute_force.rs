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

#[cfg(test)]
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
    search_segments_controlled(
        query,
        base,
        delta,
        dim,
        top_k,
        min_score,
        id_filter_map,
        None,
    )
    .expect("未传入取消控制器时扫描无需执行取消检查")
}

/// 并行分片共享控制器，在批次检查点停止评分并传播请求错误。
#[allow(clippy::too_many_arguments)]
pub(crate) fn search_segments_controlled<T: VectorType>(
    query: &[T],
    base: &[T],
    delta: &[T],
    dim: usize,
    top_k: usize,
    min_score: f32,
    id_filter_map: impl Fn(usize) -> Option<NodeId> + Sync,
    control: Option<&crate::query::tql_executor::QueryControl>,
) -> crate::Result<Vec<SearchHit>> {
    if let Some(control) = control {
        control.check()?;
    }
    if dim == 0
        || top_k == 0
        || query.len() != dim
        || !base.len().is_multiple_of(dim)
        || !delta.len().is_multiple_of(dim)
        || query.iter().any(|v| !v.to_f32().is_finite())
    {
        return Ok(Vec::new());
    }
    let heap = base
        .par_chunks(dim)
        .chain(delta.par_chunks(dim))
        .enumerate()
        .try_fold(BinaryHeap::new, |mut heap, (slot, vector)| {
            if slot % 64 == 0
                && let Some(control) = control
            {
                control.check()?;
            }
            if let Some(id) = id_filter_map(slot) {
                let score = T::similarity(query, vector);
                if score.is_finite() && score >= min_score {
                    retain_best(&mut heap, Candidate { id, score }, top_k);
                }
            }
            Ok(heap)
        })
        .try_reduce(BinaryHeap::new, |mut left, right| {
            for candidate in right {
                retain_best(&mut left, candidate, top_k);
            }
            Ok::<_, crate::TriviumError>(left)
        })?;
    if let Some(control) = control {
        control.check()?;
    }
    Ok(finish(heap))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 分片线程数和top_k边界遵守独立同分次序() {
        // 257 跨越并行分片边界；映射倒序让 slot 次序与 ID 次序相反。
        let flat = [1f32, 0.].repeat(257);
        for threads in [1, 2, 4] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            for split in [0, 1, 128, 257] {
                for k in [0, 1, 17, 256, 257, usize::MAX] {
                    let hits = pool.install(|| {
                        search_filter_map_segments(
                            &[1., 0.],
                            &flat[..split * 2],
                            &flat[split * 2..],
                            2,
                            k,
                            0.,
                            |i| (i != 128).then_some(257 - i as u64),
                        )
                    });
                    let expected = (1..=257)
                        .filter(|id| *id != 129)
                        .take(k)
                        .collect::<Vec<_>>();
                    assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), expected);
                    assert!(hits.iter().all(|h| h.score == 1.));
                }
            }
        }
        for (query, base, dim) in [
            (vec![1.], vec![1., 0.], 2),
            (vec![1., 0.], vec![1.], 2),
            (vec![], vec![], 0),
            (vec![f32::NAN, 0.], vec![1., 0.], 2),
        ] {
            assert!(search(&query, &base, dim, 1, 0., |_| 1).is_empty());
        }
    }

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

    fn check_segments<T: VectorType>() {
        for dim in [3, 65, 768] {
            let query = vec![T::from_f32(1.); dim];
            let flat: Vec<T> = (0..80 * dim)
                .map(|i| T::from_f32(((i / dim % 11 + i % dim * 3) % 17) as f32))
                .collect();
            let mut reference: Vec<_> = flat
                .chunks(dim)
                .enumerate()
                .filter(|(i, _)| i % 7 != 0)
                .map(|(i, v)| (i as u64, T::similarity(&query, v)))
                .filter(|(_, s)| *s >= -1.)
                .collect();
            reference.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            reference.truncate(13);
            for split in [0, 1, 37, 80] {
                let hits = search_filter_map_segments(
                    &query,
                    &flat[..split * dim],
                    &flat[split * dim..],
                    dim,
                    13,
                    -1.,
                    |i| (i % 7 != 0).then_some(i as u64),
                );
                assert_eq!(
                    hits.iter().map(|h| (h.id, h.score)).collect::<Vec<_>>(),
                    reference,
                    "维度 {dim}，分界 {split}"
                );
            }
        }
    }

    #[test]
    fn 分段精确扫描与串行reference一致_包含同分过滤和尾维() {
        check_segments::<f32>();
        check_segments::<half::f16>();
        check_segments::<u64>();
    }
}
