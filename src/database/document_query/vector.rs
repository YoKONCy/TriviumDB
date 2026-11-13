//! 文档候选约束下的原生向量评分、分组感知ANN扩展及既有RRF融合。
use super::*;
use crate::index::quiver::{NavigationScorer, QuIVerSearchConfig};
use std::cell::RefCell;

pub(super) struct Recall<'a, T: VectorType> {
    pub vector: &'a [T],
    pub options: &'a DocumentVectorOptions,
    pub text_weight: Option<f32>,
}

impl<T: VectorType> Recall<'_, T> {
    pub fn validate(&self, query: &DocumentSearchQuery, mt: &MemTable<T>) -> Result<()> {
        crate::vector::validate_query_vector(self.vector, mt.dim())
            .map_err(|error| error.at_field("vector"))?;
        let prefix = if self.text_weight.is_some() {
            "options.vector"
        } else {
            "options"
        };
        for (field, value) in [
            ("initial_candidates", self.options.initial_candidates),
            ("candidate_limit", self.options.candidate_limit),
            ("ef_search", self.options.ef_search),
        ] {
            if value == 0 {
                return Err(invalid(
                    &format!("{prefix}.{field}"),
                    "ANN候选参数须为正数 (ANN candidate parameters must be positive)",
                ));
            }
        }
        if self.options.initial_candidates > self.options.candidate_limit {
            return Err(invalid(
                &format!("{prefix}.initial_candidates"),
                "初始候选数超出候选上限 (Initial candidates exceed the candidate limit)",
            ));
        }
        if self
            .options
            .min_score
            .is_some_and(|score| !score.is_finite())
        {
            return Err(invalid(
                &format!("{prefix}.min_score"),
                "向量分数阈值须为有限数值 (Vector score threshold must be finite)",
            ));
        }
        if self.options.strategy == DocumentVectorStrategy::Ann
            && (crate::vector::is_binary_vector::<T>() || mt.quiver().is_none())
        {
            return Err(invalid(
                &format!("{prefix}.strategy"),
                "ANN要求已就绪的浮点QuIVer索引 (ANN requires a ready floating-point QuIVer index)",
            ));
        }
        if let Some(weight) = self.text_weight {
            if !weight.is_finite() || weight <= 0.0 {
                return Err(invalid(
                    "options.text_weight",
                    "文本融合权重须为有限正数 (Text fusion weight must be finite and positive)",
                ));
            }
        } else {
            for (field, present) in [
                ("text", !query.text.trim().is_empty()),
                ("fields", !query.fields.is_empty()),
                ("include_matches", query.include_matches),
                ("match_mode", query.match_mode != DocumentMatchMode::Any),
            ] {
                if present {
                    return Err(invalid(
                        &format!("query.{field}"),
                        "该文本参数适用于文本或混合检索 (This text parameter applies to text or hybrid retrieval)",
                    ));
                }
            }
        }
        Ok(())
    }
}

pub(super) struct Output {
    pub scores: BTreeMap<NodeId, f32>,
    pub components: BTreeMap<NodeId, DocumentMatchScores>,
    pub info: DocumentRetrievalInfo,
}

struct Control<'a> {
    limits: &'a DocumentQueryLimits,
    info: DocumentRetrievalInfo,
    failure: Option<TriviumError>,
}

impl Control<'_> {
    fn score(&mut self) -> Result<()> {
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::DocumentVectorStep);
        crate::request::check_current()?;
        self.info.vector_scores = self.info.vector_scores.saturating_add(1);
        budget(
            "document_vector_scores",
            self.info.vector_scores,
            self.limits.max_vector_scores,
        )
    }

    fn navigation(&mut self) -> Result<()> {
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::DocumentAnnStep);
        crate::request::check_current()?;
        self.info.ann_visits = self.info.ann_visits.saturating_add(1);
        budget(
            "document_ann_visits",
            self.info.ann_visits,
            self.limits.max_ann_visits,
        )
    }
}

struct Scorer<'a, 'b> {
    eligible: &'a BTreeSet<NodeId>,
    control: &'a RefCell<Control<'b>>,
}

impl NavigationScorer for Scorer<'_, '_> {
    fn score(&mut self, _id: NodeId, distance: u32) -> u32 {
        distance
    }

    fn checkpoint(&mut self) -> bool {
        let mut control = self.control.borrow_mut();
        if control.failure.is_some() {
            return false;
        }
        match control.navigation() {
            Ok(()) => true,
            Err(error) => {
                control.failure = Some(error);
                false
            }
        }
    }

    fn accept_result(&mut self, id: NodeId) -> bool {
        self.eligible.contains(&id)
    }
}

fn ranked(scores: BTreeMap<NodeId, f32>) -> Vec<(NodeId, f32)> {
    let mut rows = scores.into_iter().collect::<Vec<_>>();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    rows
}

fn insert(
    scores: &mut BTreeMap<NodeId, f32>,
    id: NodeId,
    score: f32,
    query: &DocumentSearchQuery,
    options: &DocumentVectorOptions,
) -> Result<()> {
    if !score.is_finite() {
        return Err(stored_error(
            "文档向量评分无效 (Invalid document vector score)",
        ));
    }
    if options.min_score.is_some_and(|minimum| score < minimum) {
        return Ok(());
    }
    budget(
        "document_candidates",
        scores.len().saturating_add(1),
        query.limits.max_candidates,
    )?;
    scores.insert(id, score);
    Ok(())
}

fn exact<T: VectorType>(
    mt: &MemTable<T>,
    query: &DocumentSearchQuery,
    recall: &Recall<'_, T>,
    eligible: &BTreeSet<NodeId>,
    control: &mut Control<'_>,
) -> Result<Vec<(NodeId, f32)>> {
    let mut scores = BTreeMap::new();
    for &id in eligible {
        control.score()?;
        let vector = mt.get_vector(id).ok_or_else(|| {
            stored_error("文档表示缺少向量 (Document representation has no vector)")
        })?;
        insert(
            &mut scores,
            id,
            T::similarity(recall.vector, vector),
            query,
            recall.options,
        )?;
    }
    Ok(ranked(scores))
}

fn ann<'a, T: VectorType>(
    work: &mut SearchWorkspace<'_, T>,
    index: &DocumentTextIndex,
    query: &DocumentSearchQuery,
    recall: &Recall<'_, T>,
    eligible: &BTreeSet<NodeId>,
    control: Control<'a>,
    base: usize,
) -> Result<(Vec<(NodeId, f32)>, Control<'a>)> {
    let quiver = work.mt.quiver().ok_or_else(|| {
        invalid(
            "options.strategy",
            "QuIVer索引尚未就绪 (QuIVer index is not ready)",
        )
    })?;
    let limit = recall.options.candidate_limit.min(quiver.total_count());
    let scratch = quiver
        .total_count()
        .div_ceil(64)
        .saturating_mul(32)
        .saturating_add(
            limit
                .max(recall.options.ef_search)
                .min(quiver.total_count())
                .saturating_mul(256),
        )
        .saturating_add(work.mt.dim().saturating_mul(16));
    reserve(
        work.mt,
        work.memory_limit,
        work.workspace,
        base.saturating_add(scratch),
    )?;
    let vector = recall
        .vector
        .iter()
        .map(|value| value.to_f32())
        .collect::<Vec<_>>();
    let target = query.max_documents.min(
        eligible
            .iter()
            .map(|id| index.units[id].document_id)
            .collect::<BTreeSet<_>>()
            .len(),
    );
    let mut width = recall
        .options
        .initial_candidates
        .max(query.max_documents.saturating_mul(query.hits_per_document))
        .min(limit);
    let control = RefCell::new(control);
    if eligible.is_empty() || limit == 0 {
        control.borrow_mut().info.stop = DocumentRecallStop::IndexExhausted;
        return Ok((Vec::new(), control.into_inner()));
    }
    loop {
        crate::request::check_current()?;
        control.borrow_mut().info.ann_rounds += 1;
        let mut scorer = Scorer {
            eligible,
            control: &control,
        };
        let raw = quiver.search_with_scorer_local(
            &vector,
            |slot, buffer| {
                let mut state = control.borrow_mut();
                if state.failure.is_some() {
                    return false;
                }
                let result = (|| {
                    let (id, stored) = work.mt.active_vector_at_slot(slot)?.ok_or_else(|| {
                        stored_error("ANN候选向量缺失 (ANN candidate vector is missing)")
                    })?;
                    if !eligible.contains(&id) {
                        return Ok(false);
                    }
                    state.score()?;
                    buffer.clear();
                    buffer.extend(stored.iter().map(|value| value.to_f32()));
                    if buffer.iter().any(|value| !value.is_finite()) {
                        return Err(stored_error(
                            "ANN候选向量无效 (Invalid ANN candidate vector)",
                        ));
                    }
                    Ok(true)
                })();
                match result {
                    Ok(value) => value,
                    Err(error) => {
                        state.failure = Some(error);
                        false
                    }
                }
            },
            &QuIVerSearchConfig {
                top_k: width,
                ef_search: recall.options.ef_search.max(width),
                rerank_limit: None,
            },
            &mut scorer,
        );
        if let Some(error) = control.borrow_mut().failure.take() {
            return Err(error);
        }
        crate::request::check_current()?;
        let mut scores = BTreeMap::new();
        for (id, score) in raw {
            if !eligible.contains(&id) {
                return Err(stored_error(
                    "ANN结果超出文档候选集 (ANN result exceeds the document candidate set)",
                ));
            }
            insert(&mut scores, id, score, query, recall.options)?;
        }
        let groups = scores
            .keys()
            .map(|id| index.units[id].document_id)
            .collect::<BTreeSet<_>>()
            .len();
        let stop = if groups >= target {
            Some(DocumentRecallStop::GroupsSatisfied)
        } else if width >= quiver.total_count() {
            Some(DocumentRecallStop::IndexExhausted)
        } else if width >= limit {
            Some(DocumentRecallStop::CandidateLimit)
        } else {
            None
        };
        if let Some(stop) = stop {
            control.borrow_mut().info.stop = stop;
            return Ok((ranked(scores), control.into_inner()));
        }
        width = width.saturating_mul(2).min(limit);
    }
}

pub(super) fn recall<T: VectorType>(
    work: &mut SearchWorkspace<'_, T>,
    index: &DocumentTextIndex,
    query: &DocumentSearchQuery,
    recall: &Recall<'_, T>,
    eligible: &BTreeSet<NodeId>,
    sparse: BTreeMap<NodeId, f32>,
    base: usize,
) -> Result<Output> {
    let use_ann = match recall.options.strategy {
        DocumentVectorStrategy::Auto => {
            !crate::vector::is_binary_vector::<T>() && work.mt.quiver().is_some()
        }
        DocumentVectorStrategy::Exact => false,
        DocumentVectorStrategy::Ann => true,
    };
    let mut control = Control {
        limits: &query.limits,
        failure: None,
        info: DocumentRetrievalInfo {
            hybrid: recall.text_weight.is_some(),
            strategy: if use_ann {
                DocumentVectorStrategy::Ann
            } else {
                DocumentVectorStrategy::Exact
            },
            approximate: use_ann,
            eligible_units: eligible.len(),
            vector_candidates: 0,
            vector_scores: 0,
            ann_visits: 0,
            ann_rounds: 0,
            stop: DocumentRecallStop::ExactComplete,
        },
    };
    let dense = if use_ann {
        let (rows, state) = ann(work, index, query, recall, eligible, control, base)?;
        control = state;
        rows
    } else {
        exact(work.mt, query, recall, eligible, &mut control)?
    };
    let sparse = ranked(sparse);
    let mut components = BTreeMap::new();
    for (rank, &(id, score)) in dense.iter().enumerate() {
        components.insert(
            id,
            DocumentMatchScores {
                vector_score: Some(score),
                vector_rank: Some(rank + 1),
                text_score: None,
                text_rank: None,
            },
        );
    }
    for (rank, &(id, score)) in sparse.iter().enumerate() {
        if !components.contains_key(&id) {
            budget(
                "document_candidates",
                components.len().saturating_add(1),
                query.limits.max_candidates,
            )?;
        }
        let entry = components.entry(id).or_insert(DocumentMatchScores {
            vector_score: None,
            vector_rank: None,
            text_score: None,
            text_rank: None,
        });
        entry.text_score = Some(score);
        entry.text_rank = Some(rank + 1);
    }
    let scores: BTreeMap<NodeId, f32> = if let Some(weight) = recall.text_weight {
        let mut fused = HashMap::new();
        super::super::pipeline::fuse_rankings_rrf(&mut fused, &dense, &sparse, weight);
        fused.into_iter().collect()
    } else {
        dense.iter().copied().collect()
    };
    if scores.values().any(|score| !score.is_finite()) {
        return Err(invalid(
            "options.text_weight",
            "融合分数溢出 (Fusion score overflow)",
        ));
    }
    control.info.vector_candidates = dense.len();
    Ok(Output {
        scores,
        components,
        info: control.info,
    })
}
