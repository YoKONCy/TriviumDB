//! 文档来源直接调用 Core 的同视图检索，返回证据与节点身份供统一管线使用。

use super::pipeline::{
    BindingValue, NodeRow, NodeSet, PipelineContext, PipelineOperator, ScoreKind, ScoreValue,
};
use super::tql_ast::TqlJson;
use crate::database::document_query::{search_locked, vector::Recall};
use crate::document::{
    DocumentHybridOptions, DocumentSearchQuery, DocumentVectorOptions, DocumentVectorRerank,
};
use crate::{Result, TriviumError, VectorType};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    deny_unknown_fields,
    bound = ""
)]
pub(crate) enum DocumentRequest<T: VectorType> {
    Search {
        query: DocumentSearchQuery,
    },
    SearchVector {
        query: DocumentSearchQuery,
        #[serde(with = "crate::document::vector_values")]
        vector: Vec<T>,
        #[serde(default)]
        options: DocumentVectorOptions,
    },
    SearchHybrid {
        query: DocumentSearchQuery,
        #[serde(with = "crate::document::vector_values")]
        vector: Vec<T>,
        #[serde(default)]
        options: DocumentHybridOptions,
    },
}

impl<T: VectorType> DocumentRequest<T> {
    pub(crate) fn validate(&self, mt: &crate::storage::memtable::MemTable<T>) -> Result<()> {
        crate::database::document_query::validate_search(mt, self.query(), self.recall().as_ref())
    }
    fn recall(&self) -> Option<Recall<'_, T>> {
        match self {
            Self::Search { .. } => None,
            Self::SearchVector {
                vector, options, ..
            } => Some(Recall {
                vector,
                options,
                text_weight: None,
            }),
            Self::SearchHybrid {
                vector, options, ..
            } => Some(Recall {
                vector,
                options: &options.vector,
                text_weight: Some(options.text_weight),
            }),
        }
    }
    pub(crate) fn parse(request: &TqlJson) -> Result<Self> {
        serde_json::from_value(request.value()?).map_err(|error| {
            crate::document::invalid(
                "documents",
                &format!("文档来源配置无效 (Invalid document source configuration): {error}"),
            )
        })
    }
    pub(crate) fn query(&self) -> &DocumentSearchQuery {
        match self {
            Self::Search { query }
            | Self::SearchVector { query, .. }
            | Self::SearchHybrid { query, .. } => query,
        }
    }
    pub(crate) fn vector_options(&self) -> Option<&DocumentVectorOptions> {
        match self {
            Self::Search { .. } => None,
            Self::SearchVector { options, .. } => Some(options),
            Self::SearchHybrid { options, .. } => Some(&options.vector),
        }
    }
    pub(crate) fn vector(&self) -> Option<&[T]> {
        match self {
            Self::Search { .. } => None,
            Self::SearchVector { vector, .. } | Self::SearchHybrid { vector, .. } => Some(vector),
        }
    }
    pub(crate) fn compressed(&self) -> bool {
        self.vector_options()
            .is_some_and(|options| options.rerank == DocumentVectorRerank::Compressed)
    }
}

pub(crate) struct DocumentSource<T: VectorType> {
    pub request: DocumentRequest<T>,
}

impl<T: VectorType> PipelineOperator<T> for DocumentSource<T> {
    fn name(&self) -> &'static str {
        "document_search"
    }
    fn apply(&self, _input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let recall = self.request.recall();
        let mut query = self.request.query().clone();
        query.limits.max_vector_scores = query
            .limits
            .max_vector_scores
            .min(context.remaining_vectors());
        let result = search_locked(
            context.memtable,
            &query,
            recall,
            context.document_memory_limit()?,
        )?;
        if let Some(retrieval) = &result.retrieval {
            context.charge_vectors(retrieval.vector_scores)?;
        }
        let count = result
            .groups
            .iter()
            .map(|group| group.hits.len())
            .sum::<usize>();
        if count > context.budget.max_nodes {
            return Err(TriviumError::QueryRowBudgetExceeded {
                budget: context.budget.max_nodes,
            });
        }
        let bytes = crate::storage::payload_store::serialized_json_len(&result)?
            .saturating_mul(8)
            .saturating_add(count.saturating_mul(1024));
        context.reserve_row_workspace(bytes)?;
        let retrieval = serde_json::to_value(&result.retrieval)
            .map_err(|error| crate::document::invalid("retrieval", &error.to_string()))?;
        let mut rows = Vec::with_capacity(count);
        for (group_index, group) in result.groups.into_iter().enumerate() {
            for (hit_index, hit) in group.hits.into_iter().enumerate() {
                crate::request::check_current()?;
                let mut row = NodeRow::new(hit.node_id);
                row.document_score = Some(ScoreValue {
                    value: hit.score,
                    kind: if result
                        .retrieval
                        .as_ref()
                        .is_some_and(|info| info.approximate)
                    {
                        ScoreKind::Approximate
                    } else {
                        ScoreKind::Exact
                    },
                });
                row.similarity = hit
                    .scores
                    .as_ref()
                    .and_then(|scores| scores.vector_score)
                    .map(|value| ScoreValue {
                        value,
                        kind: row
                            .document_score
                            .map_or(ScoreKind::Exact, |score| score.kind),
                    });
                row.text_score = hit
                    .scores
                    .as_ref()
                    .and_then(|scores| scores.text_score)
                    .map(|value| ScoreValue {
                        value,
                        kind: ScoreKind::Exact,
                    });
                if matches!(self.request, DocumentRequest::Search { .. }) {
                    row.text_score = Some(ScoreValue {
                        value: hit.score,
                        kind: ScoreKind::Exact,
                    });
                }
                let metadata = serde_json::json!({
                    "document_id": group.document_id.to_string(), "revision": group.revision,
                    "source_id": group.source_id, "source_revision": group.source_revision,
                    "group_score": group.score, "score": hit.score,
                    "group_rank": group_index + 1, "hit_rank": hit_index + 1,
                    "block_key": hit.block_key, "kind": hit.kind,
                    "evidence": hit.evidence, "matches": hit.matches, "scores": hit.scores, "retrieval": retrieval
                });
                row.bindings = Some(Arc::new(BTreeMap::from([
                    ("_".into(), BindingValue::Node(hit.node_id)),
                    ("document".into(), BindingValue::Json(metadata)),
                ])));
                rows.push(row);
            }
        }
        Ok(NodeSet::from_rows(rows))
    }
}
