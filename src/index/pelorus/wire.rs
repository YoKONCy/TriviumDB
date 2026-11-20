//! 绑定与 TQL 共用的只读参数表示。

use super::PelorusSearchOptions;
use crate::{Filter, Result, TriviumError, VectorType};
use serde::Deserialize;
use serde_json::{Number, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WireSearch {
    pub vector: Vec<Number>,
    #[serde(default)]
    pub options: WireSearchOptions,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct WireSearchOptions {
    pub top_k: Option<usize>,
    ef_search: Option<usize>,
    rerank_limit: Option<usize>,
    max_distance: Option<f64>,
    pub exact: bool,
    payload_filter: Option<Value>,
}

impl WireSearchOptions {
    pub fn into_core(self) -> Result<PelorusSearchOptions> {
        let payload_filter = self
            .payload_filter
            .map(|value| Filter::from_json(&value))
            .transpose()
            .map_err(|message| {
                TriviumError::InvalidInput(message).at_field("options.payload_filter")
            })?;
        Ok(PelorusSearchOptions {
            top_k: self
                .top_k
                .unwrap_or_else(|| PelorusSearchOptions::default().top_k),
            ef_search: self.ef_search,
            rerank_limit: self.rerank_limit,
            max_distance: self.max_distance,
            exact: self.exact,
            payload_filter,
        })
    }
}

pub(crate) fn convert_query<T: VectorType>(vector: &[Number]) -> Result<Vec<T>> {
    vector
        .iter()
        .enumerate()
        .map(|(i, value)| {
            T::from_json_number(value).map_err(|e| e.at_field(&format!("vector[{i}]")))
        })
        .collect()
}
