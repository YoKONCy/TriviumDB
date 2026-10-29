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
    db.ensure_readable()?;
    crate::vector::validate_query_vector(query, db.dim())?;
    let runtime = db.resource_runtime();
    let estimate = top_k
        .min(db.internal_slot_count())
        .checked_mul(std::mem::size_of::<Candidate>())
        .and_then(|bytes| bytes.checked_mul(runtime.parallelism().saturating_add(1)))
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or_else(|| TriviumError::CapacityAllocationFailed {
            reason: "精确查询工作区估算溢出".into(),
        })?;
    let _workspace = runtime.reserve_workspace(estimate)?;
    runtime
        .pool(runtime.parallelism())?
        .install(|| search_inner(db, query, top_k))
}

fn search_inner<T: VectorType>(
    db: &MemTable<T>,
    query: &[T],
    top_k: usize,
) -> Result<Vec<SearchHit>> {
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
        .try_fold(
            || BinaryHeap::with_capacity(top_k),
            |mut heap, slot| {
                if let Some((id, vector)) = db.active_vector_at_slot(slot)? {
                    let score = T::similarity(query, vector);
                    if score.is_finite() {
                        insert_candidate(&mut heap, Candidate { id, score }, top_k);
                    }
                }
                Ok::<_, TriviumError>(heap)
            },
        )
        .try_reduce(
            || BinaryHeap::with_capacity(top_k),
            |mut left, right| {
                for candidate in right {
                    insert_candidate(&mut left, candidate, top_k);
                }
                Ok(left)
            },
        )?;

    hydrate_candidates(db, candidates)
}

/// GraphFirst 只对去重后的合法 anchor 评分，先保留 Top-K，再读取 Payload。
pub(crate) fn rank_unique_ids<T: VectorType>(
    db: &MemTable<T>,
    query: &[T],
    ids: &[NodeId],
    top_k: usize,
) -> Result<Vec<SearchHit>> {
    let k = top_k.min(ids.len());
    if k == 0 {
        return Ok(Vec::new());
    }
    let bytes = k
        .checked_mul(std::mem::size_of::<Candidate>() * 2)
        .ok_or_else(|| TriviumError::CapacityAllocationFailed {
            reason: "候选堆工作区溢出 (Candidate heap workspace overflowed)".into(),
        })?;
    let _workspace = db.resource_runtime().reserve_workspace(bytes)?;
    let mut heap = BinaryHeap::with_capacity(k);
    for &id in ids {
        if let Some(slot) = db.internal_slot_of(id)
            && let Some((_, vector)) = db.active_vector_at_slot(slot)?
        {
            let score = T::similarity(query, vector);
            if score.is_finite() {
                insert_candidate(&mut heap, Candidate { id, score }, k);
            }
        }
    }
    hydrate_candidates(db, heap)
}

fn hydrate_candidates<T: VectorType>(
    db: &MemTable<T>,
    candidates: BinaryHeap<Candidate>,
) -> Result<Vec<SearchHit>> {
    let mut candidates = candidates.into_vec();
    candidates.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.id.cmp(&right.id))
    });
    let bytes = candidates.iter().try_fold(
        candidates
            .len()
            .saturating_mul(std::mem::size_of::<SearchHit>()),
        |sum, candidate| {
            sum.checked_add(db.payload_query_peak(candidate.id)?)
                .ok_or_else(|| TriviumError::CapacityAllocationFailed {
                    reason: "精确结果工作区溢出 (Exact result workspace overflowed)".into(),
                })
        },
    )?;
    let _payload_workspace = db.resource_runtime().reserve_workspace(bytes)?;
    candidates
        .into_iter()
        .map(|candidate| {
            let payload = db
                .try_get_payload(candidate.id)?
                .ok_or(TriviumError::NodeNotFound(candidate.id))?;
            Ok(SearchHit {
                id: candidate.id,
                score: candidate.score,
                payload: (*payload).clone(),
            })
        })
        .collect()
}
