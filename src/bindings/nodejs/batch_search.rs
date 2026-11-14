//! 批量检索的异步任务与请求上下文传播。

use super::arguments::{
    optional_number, parse_search_usize, read_array, read_number, validate_u64_component,
};
use super::database::{DbBackend, TriviumDB};
use super::errors::to_napi_error;
use super::types::{JsSearchHit, search_hit_to_js};
use napi::bindgen_prelude::Either;
use napi::{Env, JsUnknown};
use napi_derive::napi;

#[derive(Clone)]
enum SearchBackendHandle {
    F32(crate::database::SearchHandle<f32>),
    F16(crate::database::SearchHandle<half::f16>),
    U64(crate::database::SearchHandle<u64>),
}

pub struct BatchSearchTask {
    context: Option<crate::sdk::RequestContext>,
    backend: SearchBackendHandle,
    queries: Vec<Vec<f64>>,
    search_config: crate::database::SearchConfig,
    batch_config: crate::database::BatchSearchConfig,
}

impl napi::Task for BatchSearchTask {
    type Output = crate::Result<Vec<Vec<crate::node::SearchHit>>>;
    type JsValue = Vec<Vec<JsSearchHit>>;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        let _scope = match self
            .context
            .as_ref()
            .map(crate::sdk::RequestContext::enter)
            .transpose()
        {
            Ok(scope) => scope,
            Err(error) => return Ok(Err(error)),
        };
        if let Err(error) = crate::request::check_current() {
            return Ok(Err(error));
        }
        Ok(match &self.backend {
            SearchBackendHandle::F32(db) => {
                let queries = self
                    .queries
                    .iter()
                    .map(|query| query.iter().map(|value| *value as f32).collect())
                    .collect::<Vec<Vec<f32>>>();
                db.search_batch(&queries, &self.search_config, &self.batch_config)
            }
            SearchBackendHandle::F16(db) => {
                let queries = self
                    .queries
                    .iter()
                    .map(|query| {
                        query
                            .iter()
                            .map(|value| half::f16::from_f64(*value))
                            .collect()
                    })
                    .collect::<Vec<Vec<half::f16>>>();
                db.search_batch(&queries, &self.search_config, &self.batch_config)
            }
            SearchBackendHandle::U64(db) => {
                let queries = self
                    .queries
                    .iter()
                    .enumerate()
                    .map(|(row, query)| {
                        query
                            .iter()
                            .copied()
                            .enumerate()
                            .map(|(column, value)| {
                                validate_u64_component(value).map_err(|error| {
                                    error.at_field(&format!("/queryVectors/{row}/{column}"))
                                })
                            })
                            .collect::<crate::Result<Vec<_>>>()
                    })
                    .collect::<crate::Result<Vec<_>>>();
                let queries = match queries {
                    Ok(queries) => queries,
                    Err(error) => return Ok(Err(error)),
                };
                db.search_batch(&queries, &self.search_config, &self.batch_config)
            }
        })
    }

    fn resolve(&mut self, env: napi::Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        output
            .map_err(|error| to_napi_error(env, error))?
            .into_iter()
            .map(|hits| {
                hits.into_iter()
                    .map(|value| search_hit_to_js(env, value))
                    .collect::<napi::Result<_>>()
            })
            .collect::<napi::Result<_>>()
    }
}

#[napi]
impl TriviumDB {
    #[napi]
    pub fn search_batch(
        &self,
        env: Env,
        query_vectors: JsUnknown,
        top_k: Option<Either<f64, JsUnknown>>,
        parallelism: Option<Either<f64, JsUnknown>>,
        min_score: Option<JsUnknown>,
    ) -> napi::Result<napi::bindgen_prelude::AsyncTask<BatchSearchTask>> {
        let query_vectors = read_array(env, query_vectors, "queryVectors", |env, value, path| {
            // 保留 u64 数值范围在任务阶段拒绝的 Promise 完成语义。
            read_array(env, value, path, read_number)
        })?;
        let top_k = parse_search_usize(env, top_k, 5.0, "topK", true)?;
        let parallelism = parse_search_usize(env, parallelism, 0.0, "parallelism", false)?;
        let backend = match &self.inner {
            DbBackend::F32(db) => SearchBackendHandle::F32(db.search_handle()),
            DbBackend::F16(db) => SearchBackendHandle::F16(db.search_handle()),
            DbBackend::U64(db) => SearchBackendHandle::U64(db.search_handle()),
        };
        Ok(napi::bindgen_prelude::AsyncTask::new(BatchSearchTask {
            context: crate::request::current(),
            backend,
            queries: query_vectors,
            search_config: crate::database::SearchConfig {
                top_k,
                expand_depth: 0,
                min_score: optional_number(env, min_score, 0.5, "minScore")? as f32,
                enable_advanced_pipeline: false,
                ..Default::default()
            },
            batch_config: crate::database::BatchSearchConfig { parallelism },
        }))
    }
}
