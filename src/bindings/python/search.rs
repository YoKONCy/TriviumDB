//! 向量、图优先、混合及高级检索入口。

use super::arguments::{
    boolean_argument, extract_argument, float_argument, integer_argument, read_float, read_id,
    read_sequence, read_string, string_argument,
};
use super::database::{DbBackend, PyTriviumDB, database_read};
use super::errors::to_py_error;
use super::json::json_to_pyobject;
use super::options::{mapping_to_filter, parse_edge_direction};
use super::types::{PyGroupedSearchResult, PyHookContext, PySearchHit, search_hit_to_python};
use pyo3::prelude::*;
use pyo3::types::PyDict;

#[pymethods]
impl PyTriviumDB {
    /// 带 Hook 上下文的检索：返回 (hits, context)
    ///
    /// 除了返回检索结果外，同时返回 HookContext 对象，
    /// 其中包含管线各阶段的计时统计和 Hook 注入的自定义数据。
    ///
    /// 示例：
    /// ```python
    /// hits, ctx = db.search_with_context(query_vec, top_k=10)
    /// print(ctx.timings)   # {'hook_pre_search': 0.1, 'graph_expand': 2.3, ...}
    /// print(ctx.custom_data)  # Hook 注入的自定义数据
    /// ```
    #[pyo3(signature = (query_vector, top_k=integer_argument(5), recall_k=integer_argument(0), rerank_k=integer_argument(0), expand_depth=integer_argument(2), min_score=float_argument(0.1), payload_filter=None), text_signature="($self, query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=2, min_score=0.1, payload_filter=None)")]
    fn search_with_context(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query_vector: Bound<'_, PyAny>,
        top_k: Py<PyAny>,
        recall_k: Py<PyAny>,
        rerank_k: Py<PyAny>,
        expand_depth: Py<PyAny>,
        min_score: Py<PyAny>,
        payload_filter: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<(Vec<PySearchHit>, PyHookContext)> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let min_score = read_float(min_score.bind(py), "min_score")?;
        let top_k = extract_argument(py, top_k.bind(py), "top_k", None)?;
        let recall_k = extract_argument(py, recall_k.bind(py), "recall_k", None)?;
        let rerank_k = extract_argument(py, rerank_k.bind(py), "rerank_k", None)?;
        let expand_depth = extract_argument(py, expand_depth.bind(py), "expand_depth", None)?;
        let rust_filter = match payload_filter {
            Some(dict) => Some(mapping_to_filter(py, dict)?),
            None => None,
        };
        let config = crate::database::SearchConfig {
            top_k,
            recall_k,
            rerank_k,
            expand_depth,
            min_score,
            payload_filter: rust_filter,
            ..Default::default()
        };

        let (results, hook_ctx) = match &this.inner {
            DbBackend::F32(db) => {
                let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                py.allow_threads(|| db.search_hybrid_with_context(None, Some(&vec), &config))
            }
            DbBackend::F16(db) => {
                let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                py.allow_threads(|| db.search_hybrid_with_context(None, Some(&vec16), &config))
            }
            DbBackend::U64(db) => {
                let vec: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                py.allow_threads(|| db.search_hybrid_with_context(None, Some(&vec), &config))
            }
        }
        .map_err(to_py_error)?;

        // 转换搜索结果
        let hits: Vec<PySearchHit> = results
            .into_iter()
            .map(|h| {
                Ok(PySearchHit {
                    id: h.id,
                    score: h.score,
                    payload: json_to_pyobject(py, &h.payload)?,
                })
            })
            .collect::<PyResult<Vec<_>>>()?;

        // 转换 HookContext → PyHookContext
        let timings_dict = PyDict::new(py);
        for (stage, dur) in &hook_ctx.stage_timings {
            let _ = timings_dict.set_item(stage, dur.as_secs_f64() * 1000.0); // 转为毫秒
        }
        let counts_dict = PyDict::new(py);
        for (stage, count) in &hook_ctx.stage_counts {
            let _ = counts_dict.set_item(stage, count);
        }
        let ctx = PyHookContext {
            timings: timings_dict.into_any().unbind(),
            counts: counts_dict.into_any().unbind(),
            custom_data: json_to_pyobject(py, &hook_ctx.custom_data)?,
            observations: hook_ctx
                .observations
                .iter()
                .map(|(name, value)| (name.clone(), value))
                .collect::<std::collections::HashMap<_, _>>()
                .into_pyobject(py)
                .unwrap()
                .into_any()
                .unbind(),
            aborted: hook_ctx.abort,
        };

        Ok((hits, ctx))
    }

    #[pyo3(signature = (query_vector, top_k=integer_argument(5), recall_k=integer_argument(0), rerank_k=integer_argument(0), expand_depth=integer_argument(0), min_score=float_argument(0.5), payload_filter=None, max_edges_per_node=integer_argument(0), min_edge_weight=float_argument(0.0), edge_direction=string_argument("out")), text_signature="($self, query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=0, min_score=0.5, payload_filter=None, max_edges_per_node=0, min_edge_weight=0.0, edge_direction='out')")]
    fn search(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query_vector: Bound<'_, PyAny>,
        top_k: Py<PyAny>,
        recall_k: Py<PyAny>,
        rerank_k: Py<PyAny>,
        expand_depth: Py<PyAny>,
        min_score: Py<PyAny>,
        payload_filter: Option<&Bound<'_, PyAny>>,
        max_edges_per_node: Py<PyAny>,
        min_edge_weight: Py<PyAny>,
        edge_direction: Py<PyAny>,
    ) -> PyResult<Vec<PySearchHit>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let min_score = read_float(min_score.bind(py), "min_score")?;
        let min_edge_weight = read_float(min_edge_weight.bind(py), "min_edge_weight")?;
        let edge_direction = read_string(edge_direction.bind(py), "edge_direction")?;
        let top_k = extract_argument(py, top_k.bind(py), "top_k", None)?;
        let recall_k = extract_argument(py, recall_k.bind(py), "recall_k", None)?;
        let rerank_k = extract_argument(py, rerank_k.bind(py), "rerank_k", None)?;
        let expand_depth = extract_argument(py, expand_depth.bind(py), "expand_depth", None)?;
        let max_edges_per_node =
            extract_argument(py, max_edges_per_node.bind(py), "max_edges_per_node", None)?;
        let rust_filter = match payload_filter {
            Some(dict) => Some(mapping_to_filter(py, dict)?),
            None => None,
        };

        let config = crate::database::SearchConfig {
            top_k,
            recall_k,
            rerank_k,
            expand_depth,
            min_score,
            enable_advanced_pipeline: false,
            payload_filter: rust_filter,
            max_edges_per_node,
            min_edge_weight,
            edge_direction: parse_edge_direction(&edge_direction)?,
            ..Default::default()
        };

        let results = match &this.inner {
            DbBackend::F32(db) => {
                let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                py.allow_threads(|| db.search_hybrid(None, Some(&vec), &config))
            }
            DbBackend::F16(db) => {
                let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                py.allow_threads(|| db.search_hybrid(None, Some(&vec16), &config))
            }
            DbBackend::U64(db) => {
                let vec: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                py.allow_threads(|| db.search_hybrid(None, Some(&vec), &config))
            }
        }
        .map_err(to_py_error)?;

        results
            .into_iter()
            .map(|h| {
                Ok(PySearchHit {
                    id: h.id,
                    score: h.score,
                    payload: json_to_pyobject(py, &h.payload)?,
                })
            })
            .collect::<PyResult<Vec<_>>>()
    }

    #[pyo3(signature = (query_vector, top_k=integer_argument(5), recall_k=integer_argument(0), rerank_k=integer_argument(0), expand_depth=integer_argument(2), min_score=float_argument(0.1), payload_filter=None, max_edges_per_node=integer_argument(0), min_edge_weight=float_argument(0.0), edge_direction=string_argument("out")), text_signature="($self, query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=2, min_score=0.1, payload_filter=None, max_edges_per_node=0, min_edge_weight=0.0, edge_direction='out')")]
    fn search_grouped(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query_vector: Bound<'_, PyAny>,
        top_k: Py<PyAny>,
        recall_k: Py<PyAny>,
        rerank_k: Py<PyAny>,
        expand_depth: Py<PyAny>,
        min_score: Py<PyAny>,
        payload_filter: Option<&Bound<'_, PyAny>>,
        max_edges_per_node: Py<PyAny>,
        min_edge_weight: Py<PyAny>,
        edge_direction: Py<PyAny>,
    ) -> PyResult<PyGroupedSearchResult> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let min_score = read_float(min_score.bind(py), "min_score")?;
        let min_edge_weight = read_float(min_edge_weight.bind(py), "min_edge_weight")?;
        let edge_direction = read_string(edge_direction.bind(py), "edge_direction")?;
        let top_k = extract_argument(py, top_k.bind(py), "top_k", None)?;
        let recall_k = extract_argument(py, recall_k.bind(py), "recall_k", None)?;
        let rerank_k = extract_argument(py, rerank_k.bind(py), "rerank_k", None)?;
        let expand_depth = extract_argument(py, expand_depth.bind(py), "expand_depth", None)?;
        let max_edges_per_node =
            extract_argument(py, max_edges_per_node.bind(py), "max_edges_per_node", None)?;
        let rust_filter = payload_filter
            .map(|dict| mapping_to_filter(py, dict))
            .transpose()?;
        let config = crate::database::SearchConfig {
            top_k,
            recall_k,
            rerank_k,
            expand_depth,
            min_score,
            payload_filter: rust_filter,
            max_edges_per_node,
            min_edge_weight,
            edge_direction: parse_edge_direction(&edge_direction)?,
            ..Default::default()
        };
        let result = match &this.inner {
            DbBackend::F32(db) => {
                let vector: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                py.allow_threads(|| db.search_hybrid_grouped(None, Some(&vector), &config))
            }
            DbBackend::F16(db) => {
                let vector: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                let vector: Vec<half::f16> = vector.into_iter().map(half::f16::from_f32).collect();
                py.allow_threads(|| db.search_hybrid_grouped(None, Some(&vector), &config))
            }
            DbBackend::U64(db) => {
                let vector: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                py.allow_threads(|| db.search_hybrid_grouped(None, Some(&vector), &config))
            }
        }
        .map_err(to_py_error)?;
        Ok(PyGroupedSearchResult {
            semantic_hits: result
                .semantic_hits
                .into_iter()
                .map(|hit| search_hit_to_python(py, hit))
                .collect::<PyResult<Vec<_>>>()?
                .into_pyobject(py)
                .unwrap()
                .into_any()
                .unbind(),
            graph_hits: result
                .graph_hits
                .into_iter()
                .map(|hit| search_hit_to_python(py, hit))
                .collect::<PyResult<Vec<_>>>()?
                .into_pyobject(py)
                .unwrap()
                .into_any()
                .unbind(),
        })
    }

    #[pyo3(signature = (query_vectors, top_k=integer_argument(5), recall_k=integer_argument(0), rerank_k=integer_argument(0), expand_depth=integer_argument(0), min_score=float_argument(0.5), parallelism=integer_argument(0)), text_signature="($self, query_vectors, top_k=5, recall_k=0, rerank_k=0, expand_depth=0, min_score=0.5, parallelism=0)")]
    fn search_batch(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query_vectors: Bound<'_, PyAny>,
        top_k: Py<PyAny>,
        recall_k: Py<PyAny>,
        rerank_k: Py<PyAny>,
        expand_depth: Py<PyAny>,
        min_score: Py<PyAny>,
        parallelism: Py<PyAny>,
    ) -> PyResult<Vec<Vec<PySearchHit>>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let min_score = read_float(min_score.bind(py), "min_score")?;
        let top_k = extract_argument(py, top_k.bind(py), "top_k", None)?;
        let recall_k = extract_argument(py, recall_k.bind(py), "recall_k", None)?;
        let rerank_k = extract_argument(py, rerank_k.bind(py), "rerank_k", None)?;
        let expand_depth = extract_argument(py, expand_depth.bind(py), "expand_depth", None)?;
        let parallelism = extract_argument(py, parallelism.bind(py), "parallelism", None)?;
        let config = crate::database::SearchConfig {
            top_k,
            recall_k,
            rerank_k,
            expand_depth,
            min_score,
            enable_advanced_pipeline: false,
            ..Default::default()
        };
        let batch_config = crate::database::BatchSearchConfig { parallelism };
        let results = match &this.inner {
            DbBackend::F32(db) => {
                let queries: Vec<Vec<f32>> =
                    read_sequence(&query_vectors, "query_vectors", |value, field| {
                        read_sequence(value, field, read_float)
                    })?;
                py.allow_threads(|| db.search_batch(&queries, &config, &batch_config))
            }
            DbBackend::F16(db) => {
                let queries: Vec<Vec<f32>> =
                    read_sequence(&query_vectors, "query_vectors", |value, field| {
                        read_sequence(value, field, read_float)
                    })?;
                let queries: Vec<Vec<half::f16>> = queries
                    .into_iter()
                    .map(|query| query.into_iter().map(half::f16::from_f32).collect())
                    .collect();
                py.allow_threads(|| db.search_batch(&queries, &config, &batch_config))
            }
            DbBackend::U64(db) => {
                let queries: Vec<Vec<u64>> =
                    read_sequence(&query_vectors, "query_vectors", |value, field| {
                        read_sequence(value, field, read_id)
                    })?;
                py.allow_threads(|| db.search_batch(&queries, &config, &batch_config))
            }
        }
        .map_err(to_py_error)?;

        results
            .into_iter()
            .map(|hits| {
                hits.into_iter()
                    .map(|hit| {
                        Ok(PySearchHit {
                            id: hit.id,
                            score: hit.score,
                            payload: json_to_pyobject(py, &hit.payload)?,
                        })
                    })
                    .collect::<PyResult<Vec<_>>>()
            })
            .collect::<PyResult<Vec<_>>>()
    }

    #[pyo3(signature = (
            query_vector,
            top_k=integer_argument(5),
            recall_k=integer_argument(0),
            rerank_k=integer_argument(0),
            expand_depth=integer_argument(2),
            min_score=float_argument(0.1),
            teleport_alpha=float_argument(0.0),
            enable_advanced_pipeline=boolean_argument(true),
            enable_sparse_residual=boolean_argument(false),
            fista_lambda=float_argument(0.1),
            fista_threshold=float_argument(0.3),
            enable_dpp=boolean_argument(false),
            dpp_quality_weight=float_argument(1.0),
            enable_refractory_fatigue=boolean_argument(false),
            enable_text_hybrid_search=boolean_argument(false),
            text_boost=float_argument(1.5),
            custom_query_text=None,
            payload_filter=None,
            force_brute_force=boolean_argument(false),
            expand_labels=None,
            max_edges_per_node=integer_argument(0),
            min_edge_weight=float_argument(0.0),
            edge_direction=string_argument("out")
        ), text_signature="($self, query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=2, min_score=0.1, teleport_alpha=0.0, enable_advanced_pipeline=True, enable_sparse_residual=False, fista_lambda=0.1, fista_threshold=0.3, enable_dpp=False, dpp_quality_weight=1.0, enable_refractory_fatigue=False, enable_text_hybrid_search=False, text_boost=1.5, custom_query_text=None, payload_filter=None, force_brute_force=False, expand_labels=None, max_edges_per_node=0, min_edge_weight=0.0, edge_direction='out')")]
    fn search_advanced(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query_vector: Bound<'_, PyAny>,
        top_k: Py<PyAny>,
        recall_k: Py<PyAny>,
        rerank_k: Py<PyAny>,
        expand_depth: Py<PyAny>,
        min_score: Py<PyAny>,
        teleport_alpha: Py<PyAny>,
        enable_advanced_pipeline: Py<PyAny>,
        enable_sparse_residual: Py<PyAny>,
        fista_lambda: Py<PyAny>,
        fista_threshold: Py<PyAny>,
        enable_dpp: Py<PyAny>,
        dpp_quality_weight: Py<PyAny>,
        enable_refractory_fatigue: Py<PyAny>,
        enable_text_hybrid_search: Py<PyAny>,
        text_boost: Py<PyAny>,
        custom_query_text: Option<Bound<'_, PyAny>>,
        payload_filter: Option<&Bound<'_, PyAny>>,
        force_brute_force: Py<PyAny>,
        expand_labels: Option<Bound<'_, PyAny>>,
        max_edges_per_node: Py<PyAny>,
        min_edge_weight: Py<PyAny>,
        edge_direction: Py<PyAny>,
    ) -> PyResult<Vec<PySearchHit>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let min_score = read_float(min_score.bind(py), "min_score")?;
        let teleport_alpha = read_float(teleport_alpha.bind(py), "teleport_alpha")?;
        let fista_lambda = read_float(fista_lambda.bind(py), "fista_lambda")?;
        let fista_threshold = read_float(fista_threshold.bind(py), "fista_threshold")?;
        let dpp_quality_weight = read_float(dpp_quality_weight.bind(py), "dpp_quality_weight")?;
        let text_boost = read_float(text_boost.bind(py), "text_boost")?;
        let min_edge_weight = read_float(min_edge_weight.bind(py), "min_edge_weight")?;
        let enable_advanced_pipeline = extract_argument(
            py,
            enable_advanced_pipeline.bind(py),
            "enable_advanced_pipeline",
            None,
        )?;
        let enable_sparse_residual = extract_argument(
            py,
            enable_sparse_residual.bind(py),
            "enable_sparse_residual",
            None,
        )?;
        let enable_dpp = extract_argument(py, enable_dpp.bind(py), "enable_dpp", None)?;
        let enable_refractory_fatigue = extract_argument(
            py,
            enable_refractory_fatigue.bind(py),
            "enable_refractory_fatigue",
            None,
        )?;
        let enable_text_hybrid_search = extract_argument(
            py,
            enable_text_hybrid_search.bind(py),
            "enable_text_hybrid_search",
            None,
        )?;
        let force_brute_force =
            extract_argument(py, force_brute_force.bind(py), "force_brute_force", None)?;
        let edge_direction = read_string(edge_direction.bind(py), "edge_direction")?;
        let custom_query_text = custom_query_text
            .map(|value| read_string(&value, "custom_query_text"))
            .transpose()?;
        let expand_labels = expand_labels
            .map(|value| read_sequence(&value, "expand_labels", read_string))
            .transpose()?;
        let top_k = extract_argument(py, top_k.bind(py), "top_k", None)?;
        let recall_k = extract_argument(py, recall_k.bind(py), "recall_k", None)?;
        let rerank_k = extract_argument(py, rerank_k.bind(py), "rerank_k", None)?;
        let expand_depth = extract_argument(py, expand_depth.bind(py), "expand_depth", None)?;
        let max_edges_per_node =
            extract_argument(py, max_edges_per_node.bind(py), "max_edges_per_node", None)?;
        // 解析 payload_filter（类 MongoDB 语法的 dict -> Rust Filter）
        let rust_filter = match payload_filter {
            Some(dict) => Some(mapping_to_filter(py, dict)?),
            None => None,
        };

        let config = crate::database::SearchConfig {
            top_k,
            recall_k,
            rerank_k,
            expand_depth,
            min_score,
            teleport_alpha,
            enable_advanced_pipeline,
            enable_sparse_residual,
            fista_lambda,
            fista_threshold,
            enable_dpp,
            dpp_quality_weight,
            enable_refractory_fatigue,
            enable_text_hybrid_search,
            text_boost,
            force_brute_force,
            expand_labels,
            max_edges_per_node,
            min_edge_weight,
            edge_direction: parse_edge_direction(&edge_direction)?,
            payload_filter: rust_filter,
            ..Default::default()
        };

        let q_text = custom_query_text.as_deref();

        let results = match &this.inner {
            DbBackend::F32(db) => {
                let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                py.allow_threads(|| db.search_hybrid(q_text, Some(&vec), &config))
            }
            DbBackend::F16(db) => {
                let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                py.allow_threads(|| db.search_hybrid(q_text, Some(&vec16), &config))
            }
            DbBackend::U64(db) => {
                let vec: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                py.allow_threads(|| db.search_hybrid(q_text, Some(&vec), &config))
            }
        }
        .map_err(to_py_error)?;

        results
            .into_iter()
            .map(|h| {
                Ok(PySearchHit {
                    id: h.id,
                    score: h.score,
                    payload: json_to_pyobject(py, &h.payload)?,
                })
            })
            .collect::<PyResult<Vec<_>>>()
    }

    #[pyo3(signature = (query_vector, query_text, top_k=integer_argument(5), expand_depth=integer_argument(2), min_score=float_argument(0.1), hybrid_alpha=float_argument(0.7), payload_filter=None), text_signature="($self, query_vector, query_text, top_k=5, expand_depth=2, min_score=0.1, hybrid_alpha=0.7, payload_filter=None)")]
    fn search_hybrid(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query_vector: Bound<'_, PyAny>,
        query_text: Bound<'_, PyAny>,
        top_k: Py<PyAny>,
        expand_depth: Py<PyAny>,
        min_score: Py<PyAny>,
        hybrid_alpha: Py<PyAny>,
        payload_filter: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Vec<PySearchHit>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let min_score = read_float(min_score.bind(py), "min_score")?;
        let hybrid_alpha = read_float(hybrid_alpha.bind(py), "hybrid_alpha")?;
        let query_text: String = extract_argument(py, &query_text, "query_text", None)?;
        let top_k = extract_argument(py, top_k.bind(py), "top_k", None)?;
        let expand_depth = extract_argument(py, expand_depth.bind(py), "expand_depth", None)?;
        let rust_filter = match payload_filter {
            Some(dict) => Some(mapping_to_filter(py, dict)?),
            None => None,
        };

        // hybrid_alpha 越大，向量分数占比越高。
        // TriviumDB 底层使用 text_boost = (1.0 - alpha) * 2.5 作为启发式倍率
        let boost = crate::sdk::hybrid_text_boost(hybrid_alpha).map_err(to_py_error)?;
        let config = crate::database::SearchConfig {
            top_k,
            expand_depth,
            min_score,
            enable_text_hybrid_search: true,
            text_boost: boost,
            payload_filter: rust_filter,
            ..Default::default()
        };
        let query_text = query_text.to_owned();
        let results = match &this.inner {
            DbBackend::F32(db) => {
                let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                py.allow_threads(|| db.search_hybrid(Some(&query_text), Some(&vec), &config))
            }
            DbBackend::F16(db) => {
                let vec: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                py.allow_threads(|| db.search_hybrid(Some(&query_text), Some(&vec16), &config))
            }
            DbBackend::U64(db) => {
                let vec: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                py.allow_threads(|| db.search_hybrid(Some(&query_text), Some(&vec), &config))
            }
        }
        .map_err(to_py_error)?;
        results
            .into_iter()
            .map(|h| {
                Ok(PySearchHit {
                    id: h.id,
                    score: h.score,
                    payload: json_to_pyobject(py, &h.payload)?,
                })
            })
            .collect::<PyResult<Vec<_>>>()
    }

    #[pyo3(signature = (query_vector, anchor_ids, top_k, max_anchor_nodes=integer_argument(100_000)), text_signature="($self, query_vector, anchor_ids, top_k, max_anchor_nodes=100000)")]
    fn search_graph_first(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query_vector: Bound<'_, PyAny>,
        anchor_ids: Bound<'_, PyAny>,
        top_k: Bound<'_, PyAny>,
        max_anchor_nodes: Py<PyAny>,
    ) -> PyResult<Vec<PySearchHit>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;

        let anchor_ids = read_sequence(&anchor_ids, "anchor_ids", read_id)?;
        let top_k = extract_argument(py, &top_k, "top_k", None)?;
        let max_anchor_nodes =
            extract_argument(py, max_anchor_nodes.bind(py), "max_anchor_nodes", None)?;
        let hits = match &this.inner {
            DbBackend::F32(db) => {
                let query: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                db.search_graph_first(&query, &anchor_ids, top_k, max_anchor_nodes)
            }
            DbBackend::F16(db) => {
                let query: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                let query: Vec<half::f16> = query.into_iter().map(half::f16::from_f32).collect();
                db.search_graph_first(&query, &anchor_ids, top_k, max_anchor_nodes)
            }
            DbBackend::U64(db) => {
                let query: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                db.search_graph_first(&query, &anchor_ids, top_k, max_anchor_nodes)
            }
        }
        .map_err(to_py_error)?;
        hits.into_iter()
            .map(|hit| {
                Ok(PySearchHit {
                    id: hit.id,
                    score: hit.score,
                    payload: json_to_pyobject(py, &hit.payload)?,
                })
            })
            .collect::<PyResult<Vec<_>>>()
    }

    #[pyo3(signature = (query_vector, top_k))]
    fn search_exact(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query_vector: Bound<'_, PyAny>,
        top_k: Bound<'_, PyAny>,
    ) -> PyResult<Vec<PySearchHit>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;

        let top_k = extract_argument(py, &top_k, "top_k", None)?;
        let hits = match &this.inner {
            DbBackend::F32(db) => {
                let query: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                db.search_exact(&query, top_k)
            }
            DbBackend::F16(db) => {
                let query: Vec<f32> = read_sequence(&query_vector, "query_vector", read_float)?;
                let query: Vec<half::f16> = query.into_iter().map(half::f16::from_f32).collect();
                db.search_exact(&query, top_k)
            }
            DbBackend::U64(db) => {
                let query: Vec<u64> = read_sequence(&query_vector, "query_vector", read_id)?;
                db.search_exact(&query, top_k)
            }
        }
        .map_err(to_py_error)?;
        hits.into_iter()
            .map(|hit| {
                Ok(PySearchHit {
                    id: hit.id,
                    score: hit.score,
                    payload: json_to_pyobject(py, &hit.payload)?,
                })
            })
            .collect::<PyResult<Vec<_>>>()
    }
}
