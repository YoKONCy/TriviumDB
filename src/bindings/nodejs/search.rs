//! 向量、图优先、混合及高级检索入口。

use super::arguments::{
    optional_number, parse_search_usize, parse_u64_vector, read_array, read_id, read_json,
    read_string, read_vector,
};
use super::database::{DbBackend, TriviumDB};
use super::errors::to_napi_error;
use super::options::{parse_payload_filter, parse_search_config};
use super::types::{
    JsGroupedSearchResult, JsHookContext, JsSearchHit, JsSearchWithContextResult, search_hit_to_js,
};
use napi::bindgen_prelude::Either;
use napi::{Env, JsUnknown};
use napi_derive::napi;

#[napi]
impl TriviumDB {
    /// 带 Hook 上下文的检索：返回 { hits, context }
    ///
    /// 除了检索结果外，同时返回管线各阶段的计时统计和 Hook 注入的自定义数据。
    ///
    /// ```js
    /// const { hits, context } = db.searchWithContext(queryVec, { topK: 10 })
    /// console.log(context.timings)     // { hook_pre_search: 0.1, graph_expand: 2.3 }
    /// console.log(context.customData)  // Hook 注入的自定义数据
    /// ```
    #[napi]
    pub fn search_with_context(
        &self,
        env: Env,
        query_vector: JsUnknown,
        config: Option<JsUnknown>,
    ) -> napi::Result<JsSearchWithContextResult> {
        let query_vector = read_vector(env, query_vector, "queryVector", &self.dtype)?;
        let (core_config, query_text) = parse_search_config(env, config, false)?;

        let q_text = query_text.as_deref();

        let (results, hook_ctx) = match &self.inner {
            DbBackend::F32(db) => {
                let v: Vec<f32> = query_vector.iter().map(|&x| x as f32).collect();
                db.search_hybrid_with_context(q_text, Some(&v), &core_config)
            }
            DbBackend::F16(db) => {
                let v: Vec<half::f16> = query_vector
                    .iter()
                    .map(|&x| half::f16::from_f64(x))
                    .collect();
                db.search_hybrid_with_context(q_text, Some(&v), &core_config)
            }
            DbBackend::U64(db) => {
                let v: Vec<u64> = parse_u64_vector(env, &query_vector)?;
                db.search_hybrid_with_context(q_text, Some(&v), &core_config)
            }
        }
        .map_err(|error| to_napi_error(env, error))?;

        // 转换 hits
        let hits: Vec<JsSearchHit> = results
            .into_iter()
            .map(|value| search_hit_to_js(env, value))
            .collect::<napi::Result<_>>()?;

        // 转换 HookContext → JsHookContext
        let mut timings_map = serde_json::Map::new();
        for (stage, dur) in &hook_ctx.stage_timings {
            timings_map.insert(
                stage.clone(),
                serde_json::json!(dur.as_secs_f64() * 1000.0), // 转为毫秒
            );
        }

        let counts = hook_ctx
            .stage_counts
            .iter()
            .map(|(stage, count)| (stage.clone(), serde_json::json!(count)))
            .collect();
        let context = JsHookContext {
            timings: serde_json::Value::Object(timings_map).into(),
            counts: serde_json::Value::Object(counts).into(),
            custom_data: hook_ctx.custom_data.into(),
            observations: serde_json::Value::Object(
                hook_ctx
                    .observations
                    .iter()
                    .map(|(name, value)| (name.clone(), serde_json::json!(value)))
                    .collect(),
            )
            .into(),
            aborted: hook_ctx.abort,
        };

        Ok(JsSearchWithContextResult { hits, context })
    }

    #[napi]
    pub fn search_graph_first(
        &self,
        env: Env,
        query_vector: JsUnknown,
        anchor_ids: JsUnknown,
        top_k: Either<f64, JsUnknown>,
        max_anchor_nodes: Option<Either<f64, JsUnknown>>,
    ) -> napi::Result<Vec<JsSearchHit>> {
        let query_vector = read_vector(env, query_vector, "queryVector", &self.dtype)?;
        let top_k = parse_search_usize(env, Some(top_k), 0.0, "topK", true)?;
        let anchors = read_array(env, anchor_ids, "anchorIds", read_id)?;
        let max_anchor_nodes =
            parse_search_usize(env, max_anchor_nodes, 100_000.0, "maxAnchorNodes", false)?;
        let hits = match &self.inner {
            DbBackend::F32(db) => {
                let query: Vec<f32> = query_vector.iter().map(|value| *value as f32).collect();
                db.search_graph_first(&query, &anchors, top_k, max_anchor_nodes)
            }
            DbBackend::F16(db) => {
                let query: Vec<half::f16> = query_vector
                    .iter()
                    .map(|value| half::f16::from_f64(*value))
                    .collect();
                db.search_graph_first(&query, &anchors, top_k, max_anchor_nodes)
            }
            DbBackend::U64(db) => {
                let query: Vec<u64> = parse_u64_vector(env, &query_vector)?;
                db.search_graph_first(&query, &anchors, top_k, max_anchor_nodes)
            }
        }
        .map_err(|error| to_napi_error(env, error))?;
        hits.into_iter()
            .map(|value| search_hit_to_js(env, value))
            .collect::<napi::Result<_>>()
    }

    #[napi]
    pub fn search_exact(
        &self,
        env: Env,
        query_vector: JsUnknown,
        top_k: Either<f64, JsUnknown>,
    ) -> napi::Result<Vec<JsSearchHit>> {
        let query_vector = read_vector(env, query_vector, "queryVector", &self.dtype)?;
        let top_k = parse_search_usize(env, Some(top_k), 0.0, "topK", true)?;
        let hits = match &self.inner {
            DbBackend::F32(db) => {
                let query: Vec<f32> = query_vector.iter().map(|value| *value as f32).collect();
                db.search_exact(&query, top_k)
            }
            DbBackend::F16(db) => {
                let query: Vec<half::f16> = query_vector
                    .iter()
                    .map(|value| half::f16::from_f64(*value))
                    .collect();
                db.search_exact(&query, top_k)
            }
            DbBackend::U64(db) => {
                let query: Vec<u64> = parse_u64_vector(env, &query_vector)?;
                db.search_exact(&query, top_k)
            }
        }
        .map_err(|error| to_napi_error(env, error))?;
        hits.into_iter()
            .map(|value| search_hit_to_js(env, value))
            .collect::<napi::Result<_>>()
    }

    // ── 向量检索 ──

    /// 混合检索：向量锚定 + 图谱扩散
    #[napi]
    pub fn search(
        &self,
        env: Env,
        query_vector: JsUnknown,
        top_k: Option<Either<f64, JsUnknown>>,
        expand_depth: Option<Either<f64, JsUnknown>>,
        min_score: Option<JsUnknown>,
        payload_filter: Option<JsUnknown>,
    ) -> napi::Result<Vec<JsSearchHit>> {
        let query_vector = read_vector(env, query_vector, "queryVector", &self.dtype)?;
        let payload_filter = payload_filter
            .map(|value| read_json(env, value, "payloadFilter"))
            .transpose()?;
        let top_k = parse_search_usize(env, top_k, 5.0, "topK", true)?;

        let expand_depth = parse_search_usize(env, expand_depth, 0.0, "expandDepth", false)?;
        let min_score = optional_number(env, min_score, 0.5, "minScore")? as f32;
        let payload_filter = parse_payload_filter(env, payload_filter)?;

        let search_config = crate::database::SearchConfig {
            top_k,
            expand_depth,
            min_score,
            enable_advanced_pipeline: false,
            payload_filter,
            ..Default::default()
        };

        let hits = match &self.inner {
            DbBackend::F32(db) => {
                let v: Vec<f32> = query_vector.iter().map(|&x| x as f32).collect();
                db.search_hybrid(None, Some(&v), &search_config)
            }
            DbBackend::F16(db) => {
                let v: Vec<half::f16> = query_vector
                    .iter()
                    .map(|&x| half::f16::from_f64(x))
                    .collect();
                db.search_hybrid(None, Some(&v), &search_config)
            }
            DbBackend::U64(db) => {
                let v: Vec<u64> = parse_u64_vector(env, &query_vector)?;
                db.search_hybrid(None, Some(&v), &search_config)
            }
        }
        .map_err(|error| to_napi_error(env, error))?;

        hits.into_iter()
            .map(|value| search_hit_to_js(env, value))
            .collect::<napi::Result<_>>()
    }

    #[napi]
    pub fn search_grouped(
        &self,
        env: Env,
        query_vector: JsUnknown,
        top_k: Option<Either<f64, JsUnknown>>,
        expand_depth: Option<Either<f64, JsUnknown>>,
        min_score: Option<JsUnknown>,
        payload_filter: Option<JsUnknown>,
    ) -> napi::Result<JsGroupedSearchResult> {
        let query_vector = read_vector(env, query_vector, "queryVector", &self.dtype)?;
        let payload_filter = payload_filter
            .map(|value| read_json(env, value, "payloadFilter"))
            .transpose()?;
        let top_k = parse_search_usize(env, top_k, 5.0, "topK", true)?;
        let payload_filter = parse_payload_filter(env, payload_filter)?;
        let config = crate::database::SearchConfig {
            top_k,
            expand_depth: parse_search_usize(env, expand_depth, 2.0, "expandDepth", false)?,
            min_score: optional_number(env, min_score, 0.1, "minScore")? as f32,
            payload_filter,
            ..Default::default()
        };
        let result = match &self.inner {
            DbBackend::F32(db) => {
                let vector: Vec<f32> = query_vector.iter().map(|&value| value as f32).collect();
                db.search_hybrid_grouped(None, Some(&vector), &config)
            }
            DbBackend::F16(db) => {
                let vector: Vec<half::f16> = query_vector
                    .iter()
                    .map(|&value| half::f16::from_f64(value))
                    .collect();
                db.search_hybrid_grouped(None, Some(&vector), &config)
            }
            DbBackend::U64(db) => {
                let vector: Vec<u64> = parse_u64_vector(env, &query_vector)?;
                db.search_hybrid_grouped(None, Some(&vector), &config)
            }
        }
        .map_err(|error| to_napi_error(env, error))?;
        Ok(JsGroupedSearchResult {
            semantic_hits: result
                .semantic_hits
                .into_iter()
                .map(|value| search_hit_to_js(env, value))
                .collect::<napi::Result<_>>()?,
            graph_hits: result
                .graph_hits
                .into_iter()
                .map(|value| search_hit_to_js(env, value))
                .collect::<napi::Result<_>>()?,
        })
    }

    /// 认知检索引擎：完全参数化暴露的高级功能 (FISTA, DPP, SA-PPR)
    #[napi]
    pub fn search_advanced(
        &self,
        env: Env,
        query_vector: JsUnknown,
        config: Option<JsUnknown>,
    ) -> napi::Result<Vec<JsSearchHit>> {
        let query_vector = read_vector(env, query_vector, "queryVector", &self.dtype)?;
        let (core_config, query_text) = parse_search_config(env, config, true)?;

        let q_text = query_text.as_deref();

        let hits = match &self.inner {
            DbBackend::F32(db) => {
                let v: Vec<f32> = query_vector.iter().map(|&x| x as f32).collect();
                db.search_hybrid(q_text, Some(&v), &core_config)
            }
            DbBackend::F16(db) => {
                let v: Vec<half::f16> = query_vector
                    .iter()
                    .map(|&x| half::f16::from_f64(x))
                    .collect();
                db.search_hybrid(q_text, Some(&v), &core_config)
            }
            DbBackend::U64(db) => {
                let v: Vec<u64> = parse_u64_vector(env, &query_vector)?;
                db.search_hybrid(q_text, Some(&v), &core_config)
            }
        }
        .map_err(|error| to_napi_error(env, error))?;

        hits.into_iter()
            .map(|value| search_hit_to_js(env, value))
            .collect::<napi::Result<_>>()
    }

    /// 混合检索增强入口：带图扩散的双路检索
    #[napi]
    pub fn search_hybrid(
        &self,
        env: Env,
        query_vector: JsUnknown,
        query_text: JsUnknown,
        top_k: Option<Either<f64, JsUnknown>>,
        expand_depth: Option<Either<f64, JsUnknown>>,
        min_score: Option<JsUnknown>,
        hybrid_alpha: Option<JsUnknown>,
        payload_filter: Option<JsUnknown>,
    ) -> napi::Result<Vec<JsSearchHit>> {
        let query_vector = read_vector(env, query_vector, "queryVector", &self.dtype)?;
        let payload_filter = payload_filter
            .map(|value| read_json(env, value, "payloadFilter"))
            .transpose()?;
        let query_text = read_string(env, query_text, "queryText")?;
        let top_k = parse_search_usize(env, top_k, 5.0, "topK", true)?;

        let expand_depth = parse_search_usize(env, expand_depth, 2.0, "expandDepth", false)?;
        let min_score = optional_number(env, min_score, 0.1, "minScore")? as f32;
        let alpha = optional_number(env, hybrid_alpha, 0.7, "hybridAlpha")? as f32;
        let payload_filter = parse_payload_filter(env, payload_filter)?;
        // 简单的启发式权重换算
        let boost =
            crate::sdk::hybrid_text_boost(alpha).map_err(|error| to_napi_error(env, error))?;

        let core_config = crate::database::SearchConfig {
            top_k,
            expand_depth,
            min_score,
            enable_text_hybrid_search: true,
            text_boost: boost,
            payload_filter,
            ..Default::default()
        };

        let hits = match &self.inner {
            DbBackend::F32(db) => {
                let v: Vec<f32> = query_vector.iter().map(|&x| x as f32).collect();
                db.search_hybrid(Some(&query_text), Some(&v), &core_config)
            }
            DbBackend::F16(db) => {
                let v: Vec<half::f16> = query_vector
                    .iter()
                    .map(|&x| half::f16::from_f64(x))
                    .collect();
                db.search_hybrid(Some(&query_text), Some(&v), &core_config)
            }
            DbBackend::U64(db) => {
                let v: Vec<u64> = parse_u64_vector(env, &query_vector)?;
                db.search_hybrid(Some(&query_text), Some(&v), &core_config)
            }
        }
        .map_err(|error| to_napi_error(env, error))?;

        hits.into_iter()
            .map(|value| search_hit_to_js(env, value))
            .collect::<napi::Result<_>>()
    }
}
