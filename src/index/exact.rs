//! 受候选集合约束的精确向量 Top-K 执行器。
//!
//! 与全库 BruteForce 不同，本模块只对 Planner/GraphFirst 产生的合法 NodeId 集合打分。
//! 使用有界最小堆控制内存，并以 score 降序、NodeId 升序作为最终稳定顺序；非有限
//! 分数、墓碑和不存在节点不会进入结果，预算在并行分片前完成检查。

use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::node::{NodeId, SearchHit};
use crate::storage::memtable::MemTable;
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
        self.id == other.id && self.score.to_bits() == other.score.to_bits()
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

fn insert_candidate(heap: &mut BinaryHeap<Candidate>, candidate: Candidate, top_k: usize) {
    if heap.len() < top_k {
        heap.push(candidate);
        return;
    }
    let should_replace = heap.peek().is_some_and(|worst| {
        candidate.score.total_cmp(&worst.score) == Ordering::Greater
            || (candidate.score.total_cmp(&worst.score) == Ordering::Equal
                && candidate.id < worst.id)
    });
    if should_replace {
        heap.pop();
        heap.push(candidate);
    }
}

pub fn search<T: VectorType>(
    db: &MemTable<T>,
    query: &[T],
    top_k: usize,
) -> Result<Vec<SearchHit>> {
    if query.len() != db.dim() {
        return Err(TriviumError::DimensionMismatch {
            expected: db.dim(),
            got: query.len(),
        });
    }
    if top_k == 0 {
        return Err(TriviumError::InvalidInput(
            "search_exact 的 top_k 必须大于 0".into(),
        ));
    }

    let top_k = top_k.min(db.internal_slot_count());
    if top_k == 0 {
        return Ok(Vec::new());
    }

    let candidates = (0..db.internal_slot_count())
        .into_par_iter()
        .fold(
            || BinaryHeap::with_capacity(top_k),
            |mut heap, slot| {
                if let Some((id, vector)) = db.active_vector_at_slot(slot) {
                    let score = T::similarity(query, vector);
                    if score.is_finite() {
                        insert_candidate(&mut heap, Candidate { id, score }, top_k);
                    }
                }
                heap
            },
        )
        .reduce(
            || BinaryHeap::with_capacity(top_k),
            |mut left, right| {
                for candidate in right {
                    insert_candidate(&mut left, candidate, top_k);
                }
                left
            },
        );

    let mut candidates = candidates.into_vec();
    candidates.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(candidates
        .into_iter()
        .filter_map(|candidate| {
            db.get_payload(candidate.id).map(|payload| SearchHit {
                id: candidate.id,
                score: candidate.score,
                payload: (*payload).clone(),
            })
        })
        .collect())
}
