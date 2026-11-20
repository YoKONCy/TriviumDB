//! 在已准入的 MemTable 读视图上执行双曲 ANN 与同度量精确扫描。

use super::{PelorusSearchOptions, boundary_factor, engine, map_error};
use crate::storage::memtable::MemTable;
use crate::{NodeId, Result, TriviumError, VectorType};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
};

#[derive(Clone, Copy)]
struct Candidate {
    id: NodeId,
    distance: f64,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
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
        self.distance
            .total_cmp(&other.distance)
            .then_with(|| self.id.cmp(&other.id))
    }
}

pub(crate) struct SearchWork {
    max_visits: usize,
    max_scores: usize,
    visits: AtomicUsize,
    scores: AtomicUsize,
    failed: AtomicBool,
    failure: Mutex<Option<TriviumError>>,
}

impl SearchWork {
    pub fn new(max_visits: usize, max_scores: usize) -> Self {
        Self {
            max_visits,
            max_scores,
            visits: AtomicUsize::default(),
            scores: AtomicUsize::default(),
            failed: AtomicBool::default(),
            failure: Mutex::default(),
        }
    }

    pub fn visit(&self) -> bool {
        if self.failed.load(Relaxed) {
            return false;
        }
        match self.charge(&self.visits, self.max_visits, "pelorus_ann_visits") {
            Ok(()) => true,
            Err(error) => {
                crate::database::lock_or_recover(&self.failure).get_or_insert(error);
                self.failed.store(true, Relaxed);
                false
            }
        }
    }

    pub fn score(&self) -> Result<()> {
        self.charge(&self.scores, self.max_scores, "pelorus_vector_scores")
    }

    pub fn checkpoint(&self) -> Result<()> {
        match crate::database::lock_or_recover(&self.failure).take() {
            Some(error) => Err(error),
            None => crate::request::check_current(),
        }
    }

    pub fn counts(&self) -> (usize, usize) {
        (self.visits.load(Relaxed), self.scores.load(Relaxed))
    }

    fn charge(&self, count: &AtomicUsize, limit: usize, resource: &'static str) -> Result<()> {
        crate::request::check_current()?;
        count
            .try_update(Relaxed, Relaxed, |used| (used < limit).then(|| used + 1))
            .map(|_| ())
            .map_err(|used| TriviumError::ResourceBudgetExceeded {
                resource,
                requested: 1,
                used,
                limit,
            })
    }
}

pub(crate) fn search<T: VectorType>(
    mt: &MemTable<T>,
    query: &[T],
    options: &PelorusSearchOptions,
    allow_exact_fallback: bool,
    eligible: impl FnMut(NodeId) -> Result<bool>,
) -> Result<Vec<(NodeId, f64)>> {
    search_bounded(
        mt,
        query,
        options,
        allow_exact_fallback,
        true,
        &SearchWork::new(usize::MAX, usize::MAX),
        eligible,
    )
}

impl PelorusSearchOptions {
    pub(crate) fn validate(&self) -> Result<usize> {
        let top_k = self.top_k;
        let ef_search = self.ef_search.unwrap_or_else(|| top_k.max(64));
        if top_k == 0 || ef_search < top_k || self.rerank_limit.is_some_and(|k| k < top_k) {
            return Err(TriviumError::InvalidInput(
            "Pelorus 要求 top_k > 0 且搜索与精排宽度至少为 top_k (Pelorus requires top_k > 0 and search and rerank widths at least top_k)".into(),
        )
        .at_field("options"));
        }
        if self.max_distance.is_some_and(|d| !d.is_finite() || d < 0.0) {
            return Err(TriviumError::InvalidInput(
            "最大测地距离须为非负有限数 (Maximum geodesic distance must be finite and nonnegative)"
                .into(),
        )
        .at_field("max_distance"));
        }
        Ok(ef_search)
    }
}

pub(crate) fn search_bounded<T: VectorType>(
    mt: &MemTable<T>,
    query: &[T],
    options: &PelorusSearchOptions,
    allow_exact_fallback: bool,
    use_profile_rerank: bool,
    work: &SearchWork,
    mut eligible: impl FnMut(NodeId) -> Result<bool>,
) -> Result<Vec<(NodeId, f64)>> {
    super::validate_dtype::<T>()?;
    mt.ensure_readable()?;
    crate::vector::validate_query_vector(query, mt.dim())?;
    let ef_search = options.validate()?;
    let top_k = options.top_k;
    let state = mt.pelorus.as_ref().ok_or_else(missing_index)?;
    let curvature = state.config.curvature;
    let query_factor = boundary_factor(curvature, query).map_err(|e| e.at_field("query"))?;
    let count = mt.active_vector_count();
    let k = top_k.min(count);
    if k == 0 {
        return Ok(Vec::new());
    }

    let query_bytes = query.len().saturating_mul(64);
    let nav_bytes = state
        .index
        .as_ref()
        .filter(|_| !options.exact)
        .map_or(0, |index| {
            index
                .len()
                .saturating_mul(16)
                .saturating_add(index.len().min(ef_search).saturating_mul(512))
        });
    let _reservation = mt.resource_runtime().reserve_workspace(
        query_bytes
            .saturating_add(k.saturating_mul(64))
            .saturating_add(nav_bytes),
    )?;
    let threshold = options.max_distance.unwrap_or(f64::INFINITY);
    crate::request::check_current()?;
    if !options.exact {
        if let Some(index) = &state.index {
            let search_config = engine::SearchConfig {
                top_k: k.min(index.len()).max(1),
                ef_search: ef_search.min(index.len()).max(1),
                rerank_limit: options.rerank_limit.map(|n| n.min(index.len()).max(1)),
            };
            let mut error = None;
            let hits = index.search_poincare_distances(
                &query.iter().map(|value| value.to_f32()).collect::<Vec<_>>(),
                &search_config,
                &mut |id| {
                    if error.is_some() || state.changed.contains(&id) {
                        return false;
                    }
                    match crate::request::check_current().and_then(|()| eligible(id)) {
                        Ok(allowed) => allowed,
                        Err(failure) => {
                            error = Some(failure);
                            false
                        }
                    }
                },
                state.ssd.as_ref().filter(|_| use_profile_rerank),
                use_profile_rerank,
                work,
            );
            if let Some(error) = error {
                return Err(error);
            }
            crate::request::check_current()?;
            let mut hits = hits.map_err(map_error)?;
            if hits.iter().any(|(_, d)| !d.is_finite() || *d < 0.0) {
                return Err(map_error(engine::Error::DistanceOverflow));
            }
            hits.retain(|&(_, distance)| distance <= threshold);
            for &id in &state.changed {
                crate::request::check_current()?;
                if let Some(vector) = mt.get_vector(id)
                    && eligible(id)?
                {
                    work.score()?;
                    let distance = distance(curvature, query, query_factor, vector)?;
                    if distance <= threshold {
                        hits.push((id, distance));
                    }
                }
            }
            hits.sort_unstable_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            hits.truncate(k);
            if hits.len() >= k || !allow_exact_fallback {
                return Ok(hits);
            }
        } else if !allow_exact_fallback {
            return Err(missing_index());
        }
    }

    let mut heap = BinaryHeap::with_capacity(k);
    for slot in 0..mt.internal_slot_count() {
        crate::request::check_current()?;
        let Some((id, vector)) = mt.active_vector_at_slot(slot)? else {
            continue;
        };
        if !eligible(id)? {
            continue;
        }
        work.score()?;
        let distance = distance(curvature, query, query_factor, vector)?;
        if distance > threshold {
            continue;
        }
        let hit = Candidate { id, distance };
        if heap.len() < k {
            heap.push(hit);
        } else if heap.peek().is_some_and(|worst| hit < *worst) {
            *heap.peek_mut().expect("非空 Top-K") = hit;
        }
    }
    Ok(heap
        .into_sorted_vec()
        .into_iter()
        .map(|hit| (hit.id, hit.distance))
        .collect())
}

pub(crate) fn distance<T: VectorType>(
    curvature: f64,
    query: &[T],
    query_factor: f64,
    vector: &[T],
) -> Result<f64> {
    let delta = query
        .iter()
        .zip(vector)
        .map(|(a, b)| (f64::from(a.to_f32()) - f64::from(b.to_f32())).powi(2))
        .sum::<f64>();
    let vector_factor = boundary_factor(curvature, vector)?;
    let distance =
        engine::metric::poincare_distance(delta * (query_factor * vector_factor), curvature);
    if !distance.is_finite() {
        return Err(map_error(engine::Error::DistanceOverflow));
    }
    Ok(distance)
}

fn missing_index() -> TriviumError {
    TriviumError::ImmutableArtifactInvalid {
        reason: "Pelorus 索引缺失或需要重建 (Pelorus index is missing or requires rebuild)".into(),
    }
}
