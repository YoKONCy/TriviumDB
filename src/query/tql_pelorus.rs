//! 显式的双曲向量来源，和普通 SEARCH 共享 NodeSet 与图作用域。

use super::pipeline::{NodeRow, NodeSet, PipelineContext, PipelineOperator, ScoreKind, ScoreValue};
use super::tql_ast::TqlJson;
use crate::graph::view::GraphRead;
use crate::index::pelorus::{self, PelorusSearchOptions, SearchWork, WireSearch, convert_query};
use crate::{Result, TriviumError, VectorType};
use serde_json::Number;

pub(crate) fn parse(request: &TqlJson) -> Result<WireSearch> {
    serde_json::from_value(request.value()?).map_err(|error| {
        TriviumError::InvalidInput(format!(
            "Pelorus 来源无效 (Invalid Pelorus source): {error}"
        ))
        .at_field("pelorus")
    })
}

#[derive(Clone)]
pub(crate) struct PelorusSource<T> {
    pub vector: Vec<T>,
    pub vector_literals: Vec<Number>,
    pub options: PelorusSearchOptions,
}

impl<T: VectorType> PelorusSource<T> {
    pub fn new(request: &TqlJson) -> Result<Self> {
        let request = parse(request)?;
        Ok(Self {
            vector: convert_query(&request.vector)?,
            vector_literals: request.vector,
            options: request.options.into_core()?,
        })
    }
}

impl<T: VectorType> PipelineOperator<T> for PelorusSource<T> {
    fn name(&self) -> &'static str {
        "pelorus_vector_search"
    }

    fn apply(&self, _input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let mt = context.memtable;
        let graph = context.graph();
        let allow_fallback = mt.pelorus_policy != crate::database::MissingIndexPolicy::Error;
        let meter = SearchWork::new(usize::MAX, context.remaining_vectors());
        let hits = pelorus::search_bounded(
            mt,
            &self.vector,
            &self.options,
            allow_fallback,
            true,
            &meter,
            |id| {
                if let Some(control) = &context.query_control {
                    control.check()?;
                }
                Ok(graph.contains(id)
                    && self.options.payload_filter.as_ref().is_none_or(|filter| {
                        mt.get_payload(id)
                            .is_some_and(|payload| filter.matches(&payload))
                    }))
            },
        )?;
        context.charge_vectors(meter.counts().1)?;
        let score_kind = if self.options.exact
            || mt.pelorus.as_ref().is_none_or(|state| {
                state.index.is_none() || state.config.rerank == Default::default()
            }) {
            ScoreKind::Exact
        } else {
            ScoreKind::Approximate
        };
        Ok(NodeSet::from_rows(
            hits.into_iter()
                .map(|(id, distance)| {
                    let mut row = NodeRow::new(id);
                    row.similarity = Some(ScoreValue {
                        kind: score_kind,
                        value: -distance,
                    });
                    row.provenance.source_ids.push(id);
                    row
                })
                .collect(),
        ))
    }
}
