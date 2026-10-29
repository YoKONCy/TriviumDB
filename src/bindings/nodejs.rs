//! Node.js napi-rs 公共绑定。
//!
//! 该层只负责 JS 值校验、camelCase API、异步任务和稳定错误码映射，核心语义必须委托
//! Rust Database/TQL 实现。u64 NodeId 在一等查询值中使用字符串，避免超过 JS 安全整数。

#[cfg(feature = "nodejs")]
#[allow(clippy::module_inception)]
pub mod nodejs {
    use crate::bindings::node_json::NodeJson;
    use crate::database::Database as GenericDatabase;
    use napi::bindgen_prelude::{Function, This};
    use napi::{Env, JsObject};
    use napi_derive::napi;

    fn node_json_value(value: impl serde::Serialize) -> serde_json::Result<NodeJson> {
        serde_json::to_value(value).map(NodeJson)
    }

    struct JsNativeHook {
        object: super::super::node_hook_refs::HookReference,
        owner_thread: std::thread::ThreadId,
    }

    impl JsNativeHook {
        fn invoke(
            &self,
            name: &str,
            value: serde_json::Value,
            ctx: &mut crate::hook::HookContext,
        ) -> Option<serde_json::Value> {
            if std::thread::current().id() != self.owner_thread {
                ctx.error = Some(format!(
                    "Node Hook {name} 只能在注册它的 JavaScript 线程同步执行"
                ));
                return None;
            }
            let object: JsObject = match self.object.get() {
                Ok(value) => value,
                Err(error) => {
                    ctx.error = Some(format!("Node Hook {name} 引用无效: {error}"));
                    return None;
                }
            };
            let function: Function<'_, NodeJson, NodeJson> = match object.get_named_property(name) {
                Ok(value) => value,
                Err(_) => return None,
            };
            match function.call(value.into()) {
                Ok(result) => Some(result.0),
                Err(error) => {
                    ctx.error = Some(format!("Node Hook {name} 执行失败: {error}"));
                    None
                }
            }
        }

        fn hits_json(hits: &[crate::node::SearchHit]) -> serde_json::Value {
            serde_json::Value::Array(
                hits.iter()
                    .map(|hit| {
                        serde_json::json!({
                            "id": hit.id.to_string(), "score": hit.score, "payload": hit.payload
                        })
                    })
                    .collect(),
            )
        }

        fn replace_hits(hits: &mut Vec<crate::node::SearchHit>, value: serde_json::Value) {
            let Some(items) = value.as_array() else {
                return;
            };
            *hits = items
                .iter()
                .filter_map(|item| {
                    Some(crate::node::SearchHit {
                        id: item.get("id")?.as_str()?.parse().ok()?,
                        score: item.get("score")?.as_f64()? as f32,
                        payload: item
                            .get("payload")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    })
                })
                .filter(|hit| hit.id != 0 && hit.score.is_finite())
                .collect();
        }
    }

    impl crate::hook::SearchHook for JsNativeHook {
        fn on_pre_search(
            &self,
            query: &mut Vec<f32>,
            _config: &mut crate::database::SearchConfig,
            ctx: &mut crate::hook::HookContext,
        ) {
            if let Some(value) = self.invoke("onPreSearch", serde_json::json!(query), ctx)
                && let Ok(updated) = serde_json::from_value::<Vec<f32>>(value)
            {
                *query = updated;
            }
        }
        fn on_custom_recall(
            &self,
            query: &[f32],
            _config: &crate::database::SearchConfig,
            ctx: &mut crate::hook::HookContext,
        ) -> Option<Vec<crate::node::SearchHit>> {
            let value = self.invoke("onCustomRecall", serde_json::json!(query), ctx)?;
            if value.is_null() {
                return None;
            }
            let mut hits = Vec::new();
            Self::replace_hits(&mut hits, value);
            Some(hits)
        }
        fn on_post_recall(
            &self,
            hits: &mut Vec<crate::node::SearchHit>,
            ctx: &mut crate::hook::HookContext,
        ) {
            if let Some(value) = self.invoke("onPostRecall", Self::hits_json(hits), ctx) {
                Self::replace_hits(hits, value);
            }
        }
        fn on_pre_graph_expand(
            &self,
            hits: &mut Vec<crate::node::SearchHit>,
            ctx: &mut crate::hook::HookContext,
        ) {
            if let Some(value) = self.invoke("onPreGraphExpand", Self::hits_json(hits), ctx) {
                Self::replace_hits(hits, value);
            }
        }
        fn on_rerank(
            &self,
            hits: &mut Vec<crate::node::SearchHit>,
            ctx: &mut crate::hook::HookContext,
        ) -> Option<Vec<crate::node::SearchHit>> {
            let value = self.invoke("onRerank", Self::hits_json(hits), ctx)?;
            let mut updated = Vec::new();
            Self::replace_hits(&mut updated, value);
            Some(updated)
        }
        fn on_post_search(
            &self,
            hits: &mut Vec<crate::node::SearchHit>,
            ctx: &mut crate::hook::HookContext,
        ) {
            if let Some(value) = self.invoke("onPostSearch", Self::hits_json(hits), ctx) {
                Self::replace_hits(hits, value);
            }
        }
    }

    fn parse_runtime(runtime: Option<JsObject>) -> napi::Result<crate::resources::ResourceRuntime> {
        Ok(if let Some(object) = runtime {
            if !object.has_own_property("__tdbResourceRuntime")? {
                return Err(napi::Error::new(
                    napi::Status::InvalidArg,
                    "runtime 必须是有效的 ResourceRuntime 句柄",
                ));
            }
            let handle: napi::bindgen_prelude::External<crate::resources::ResourceRuntime> =
                object.get_named_property("__tdbResourceRuntime")?;
            (*handle).clone()
        } else {
            crate::resources::ResourceRuntime::shared_default()
        })
    }

    fn to_napi_error(env: Env, error: crate::error::TriviumError) -> napi::Error {
        let code = error.code();
        let details = node_tql_scalar_json(error.details());
        let result = env
            .create_error(napi::Error::from_reason(format!("{code}: {error}")))
            .and_then(|mut object| {
                object.set_named_property("code", code)?;
                object.set_named_property("details", details.clone())?;
                if let Some(fields) = details.as_object() {
                    for (name, value) in fields {
                        object.set_named_property(name, value.clone())?;
                    }
                }
                Ok(napi::Error::from(object.into_unknown()))
            });
        match result {
            Ok(error) | Err(error) => error,
        }
    }

    #[napi]
    pub fn verify_snapshot(
        env: Env,
        directory: String,
        runtime: Option<JsObject>,
    ) -> napi::Result<NodeJson> {
        let runtime = parse_runtime(runtime)?;
        let manifest = crate::storage::snapshot::verify_snapshot_with_runtime(&directory, &runtime)
            .map_err(|e| to_napi_error(env, e))?;
        node_json_value(manifest).map_err(|e| napi::Error::from_reason(e.to_string()))
    }

    #[napi]
    pub fn restore_snapshot(
        env: Env,
        source: String,
        destination: String,
        runtime: Option<JsObject>,
    ) -> napi::Result<NodeJson> {
        let runtime = parse_runtime(runtime)?;
        let manifest = crate::storage::snapshot::restore_snapshot_with_runtime(
            &source,
            &destination,
            &runtime,
        )
        .map_err(|e| to_napi_error(env, e))?;
        node_json_value(manifest).map_err(|e| napi::Error::from_reason(e.to_string()))
    }

    fn node_tql_scalar_json(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Number(ref number)
                if number
                    .as_i64()
                    .is_some_and(|n| n.unsigned_abs() > 9_007_199_254_740_991)
                    || number.as_u64().is_some_and(|n| n > 9_007_199_254_740_991) =>
            {
                serde_json::Value::String(number.to_string())
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(node_tql_scalar_json).collect())
            }
            serde_json::Value::Object(values) => serde_json::Value::Object(
                values
                    .into_iter()
                    .map(|(name, value)| (name, node_tql_scalar_json(value)))
                    .collect(),
            ),
            value => value,
        }
    }

    fn node_tql_row_to_json<T: crate::VectorType>(
        row: std::collections::HashMap<String, crate::query::tql_executor::TqlValue<T>>,
    ) -> NodeJson {
        use crate::query::tql_executor::TqlValue;
        let mut obj = serde_json::Map::new();
        for (name, value) in row {
            let value = match value {
                TqlValue::Node(node) => serde_json::json!({
                    "id": node.id.to_string(),
                    "payload": node.payload,
                    "numEdges": node.edges.len(),
                }),
                TqlValue::Edge(edge) => serde_json::json!({
                    "type": "edge",
                    "source": edge.source_id.to_string(),
                    "target": edge.target_id.to_string(),
                    "label": edge.label,
                    "weight": edge.weight,
                    "metadata": edge.metadata,
                }),
                TqlValue::Int(value) => node_tql_scalar_json(serde_json::json!(value)),
                TqlValue::UInt(value) => node_tql_scalar_json(serde_json::json!(value)),
                TqlValue::Float(value) => serde_json::json!(value),
                TqlValue::String(value) => serde_json::json!(value),
                TqlValue::Bool(value) => serde_json::json!(value),
                TqlValue::Path(value) => serde_json::json!(
                    value
                        .into_iter()
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>()
                ),
                TqlValue::List(value) => node_tql_scalar_json(serde_json::Value::Array(value)),
                TqlValue::Object(value) => node_tql_scalar_json(serde_json::Value::Object(value)),
                TqlValue::Null => serde_json::Value::Null,
            };
            obj.insert(name, value);
        }
        serde_json::Value::Object(obj).into()
    }

    #[napi]
    pub struct JsPreparedTql {
        inner: crate::query::tql_prepared::PreparedTql,
    }

    #[napi]
    impl JsPreparedTql {
        #[napi]
        pub fn parameter_names(&self) -> Vec<String> {
            self.inner
                .parameter_names()
                .into_iter()
                .map(str::to_owned)
                .collect()
        }
    }

    // ════════ 后端枚举：封装三种泛型特化 ════════

    enum DbBackend {
        F32(GenericDatabase<f32>),
        F16(GenericDatabase<half::f16>),
        U64(GenericDatabase<u64>),
    }

    #[derive(Clone)]
    enum SearchBackendHandle {
        F32(crate::database::SearchHandle<f32>),
        F16(crate::database::SearchHandle<half::f16>),
        U64(crate::database::SearchHandle<u64>),
    }

    pub struct BatchSearchTask {
        backend: SearchBackendHandle,
        queries: Vec<Vec<f64>>,
        search_config: crate::database::SearchConfig,
        batch_config: crate::database::BatchSearchConfig,
    }

    impl napi::Task for BatchSearchTask {
        type Output = crate::Result<Vec<Vec<crate::node::SearchHit>>>;
        type JsValue = Vec<Vec<JsSearchHit>>;

        fn compute(&mut self) -> napi::Result<Self::Output> {
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
                        .map(|query| {
                            query
                                .iter()
                                .copied()
                                .map(validate_u64_component)
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

    /// 统一分发宏：对三种后端执行相同的表达式
    macro_rules! dispatch {
        ($self:expr, $db:ident => $expr:expr) => {
            match &$self.inner {
                DbBackend::F32($db) => $expr,
                DbBackend::F16($db) => $expr,
                DbBackend::U64($db) => $expr,
            }
        };
        ($self:expr, mut $db:ident => $expr:expr) => {
            match &mut $self.inner {
                DbBackend::F32($db) => $expr,
                DbBackend::F16($db) => $expr,
                DbBackend::U64($db) => $expr,
            }
        };
    }

    // ════════ JS 侧返回结构体 ════════

    /// 向量检索命中结果
    #[napi(object)]
    pub struct JsSearchHit {
        /// 节点 ID（JS Number，安全范围内的 u64）
        pub id: f64,
        /// 相似度得分
        pub score: f64,
        /// 节点元数据（JSON 对象）
        pub payload: NodeJson,
    }

    #[napi(object)]
    pub struct JsGroupedSearchResult {
        pub semantic_hits: Vec<JsSearchHit>,
        pub graph_hits: Vec<JsSearchHit>,
    }

    fn round_api_f32(value: f32) -> f64 {
        ((value as f64) * 1_000_000.0).round() / 1_000_000.0
    }

    #[cfg(test)]
    mod tests {
        use super::round_api_f32;

        #[test]
        fn 错误字段中的大整数保持十进制精度() {
            assert_eq!(
                super::node_tql_scalar_json(
                    serde_json::json!({"id": 18446744073709551614u64, "used": 9007199254740993u64, "limit": 9007199254740991u64})
                ),
                serde_json::json!({"id": "18446744073709551614", "used": "9007199254740993", "limit": 9007199254740991u64})
            );
        }

        #[test]
        fn 边权重_api舍入到六位小数() {
            assert_eq!(round_api_f32(0.9f32), 0.9);
            assert_eq!(round_api_f32(0.12345678f32), 0.123457);
        }
    }

    fn search_hit_to_js(env: Env, hit: crate::node::SearchHit) -> napi::Result<JsSearchHit> {
        Ok(JsSearchHit {
            id: node_id_to_js(env, hit.id)?,
            score: hit.score as f64,
            payload: hit.payload.into(),
        })
    }

    #[napi(object)]
    pub struct JsReachabilityStep {
        pub from: f64,
        pub to: f64,
        pub label: String,
        pub weight: f64,
        pub metadata: NodeJson,
    }

    #[napi(object)]
    pub struct JsReachabilityResult {
        pub source_id: f64,
        pub target_id: f64,
        pub depth: u32,
        pub path: Vec<f64>,
        pub steps: Vec<JsReachabilityStep>,
    }

    #[napi(object)]
    pub struct JsReachabilityOutput {
        pub results: Vec<JsReachabilityResult>,
        pub visited_nodes: f64,
        pub traversed_edges: f64,
        pub truncated: bool,
    }

    #[napi(object)]
    pub struct JsSubgraphNode {
        pub id: f64,
        pub payload: NodeJson,
    }

    #[napi(object)]
    pub struct JsSubgraphEdge {
        pub source_id: f64,
        pub target_id: f64,
        pub label: String,
        pub weight: f64,
        pub metadata: NodeJson,
    }

    #[napi(object)]
    pub struct JsSubgraphResult {
        pub nodes: Vec<JsSubgraphNode>,
        pub edges: Vec<JsSubgraphEdge>,
        pub visited_nodes: f64,
        pub traversed_edges: f64,
        pub truncated: bool,
    }

    #[napi(object)]
    pub struct JsReachabilityOptions {
        pub min_depth: Option<f64>,
        pub max_depth: Option<f64>,
        pub labels: Option<Vec<String>>,
        pub direction: Option<String>,
        pub max_visited_nodes: Option<f64>,
        pub max_results: Option<f64>,
        pub max_edges: Option<f64>,
    }

    /// 多个数据库共享的运行时资源配置，字节统计不等于进程 RSS。
    #[napi(object)]
    pub struct JsResourceLimits {
        pub maintenance_memory_limit_mb: Option<f64>,
        pub memory_limit_mb: Option<f64>,
        pub max_threads: Option<f64>,
        pub max_active_tasks: Option<f64>,
    }

    #[napi(js_name = "ResourceRuntime")]
    pub struct JsResourceRuntime {
        inner: crate::resources::ResourceRuntime,
    }

    #[napi]
    impl JsResourceRuntime {
        #[napi(constructor)]
        pub fn new(
            env: Env,
            mut this: napi::bindgen_prelude::This<JsObject>,
            options: Option<JsResourceLimits>,
        ) -> napi::Result<Self> {
            let options = options.unwrap_or(JsResourceLimits {
                maintenance_memory_limit_mb: None,
                memory_limit_mb: None,
                max_threads: None,
                max_active_tasks: None,
            });
            let memory_limit =
                parse_safe_usize(env, options.memory_limit_mb.unwrap_or(0.0), "memoryLimitMb")?
                    .checked_mul(1024 * 1024)
                    .filter(|bytes| (*bytes as u128) <= 9_007_199_254_740_991)
                    .ok_or_else(|| {
                        napi::Error::from_reason("memoryLimitMb 换算后超过安全整数范围")
                    })?;
            let inner = crate::resources::ResourceRuntime::new(crate::resources::ResourceLimits {
                memory_limit,
                maintenance_memory_limit: parse_safe_usize(
                    env,
                    options.maintenance_memory_limit_mb.unwrap_or(0.0),
                    "maintenanceMemoryLimitMb",
                )?
                .checked_mul(1024 * 1024)
                .filter(|bytes| (*bytes as u128) <= 9_007_199_254_740_991)
                .ok_or_else(|| napi::Error::from_reason("维护额度换算超过安全整数范围"))?,
                max_threads: parse_safe_usize(
                    env,
                    options.max_threads.unwrap_or(0.0),
                    "maxThreads",
                )?,
                max_active_tasks: parse_safe_usize(
                    env,
                    options.max_active_tasks.unwrap_or(0.0),
                    "maxActiveTasks",
                )?,
            })
            .map_err(|error| to_napi_error(env, error))?;
            // 使用带类型标记的外部句柄，避免把其他原生类的指针误解为资源运行时。
            let handle = env.create_external(inner.clone(), None)?;
            this.define_properties(&[
                napi::Property::new("__tdbResourceRuntime")?.with_value(&handle)
            ])?;
            Ok(Self { inner })
        }

        #[napi]
        pub fn stats(&self) -> napi::Result<NodeJson> {
            node_json_value(self.inner.stats())
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }
    }

    /// 完整文本索引重建的节点正文与关键词。
    #[napi(object)]
    pub struct JsTextIndexDocument {
        pub id: f64,
        pub text: String,
        pub keywords: Option<Vec<String>>,
    }

    /// 数据库打开与容量规划配置。
    #[napi(object)]
    pub struct JsDatabaseOptions {
        pub runtime: Option<JsObject>,
        pub dim: Option<f64>,
        pub dtype: Option<String>,
        pub sync_mode: Option<String>,
        pub storage_mode: Option<String>,
        pub auto_build_quiver: Option<bool>,
        pub load_text_index: Option<bool>,
        pub expected_nodes: Option<f64>,
        pub memory_limit_mb: Option<f64>,
        pub payload_cache_mb: Option<f64>,
        pub payload_cache_entry_mb: Option<f64>,
        pub access_mode: Option<String>,
        pub missing_index_policy: Option<String>,
        pub max_query_rows: Option<f64>,
        pub row_overflow: Option<String>,
    }

    /// 高级管线专用配置结构
    #[napi(object)]
    pub struct JsSearchConfig {
        /// 保留原始 JS Number，在转换为整数前检查有限性、整数性和范围。
        pub top_k: Option<f64>,
        pub recall_k: Option<f64>,
        pub rerank_k: Option<f64>,
        pub expand_depth: Option<f64>,
        pub min_score: Option<f64>,
        pub teleport_alpha: Option<f64>,
        pub enable_advanced_pipeline: Option<bool>,
        pub enable_sparse_residual: Option<bool>,
        pub fista_lambda: Option<f64>,
        pub fista_threshold: Option<f64>,
        pub enable_dpp: Option<bool>,
        pub dpp_quality_weight: Option<f64>,
        pub enable_refractory_fatigue: Option<bool>,
        pub enable_text_hybrid_search: Option<bool>,
        pub text_boost: Option<f64>,
        pub force_brute_force: Option<bool>,
        pub custom_query_text: Option<String>,
        /// 类 MongoDB JSON Payload 过滤器，在向量召回阶段生效。
        pub payload_filter: Option<NodeJson>,
        /// CCSA: 扩散方向偏置向量，让图扩散优先沿语义相近的节点方向传播
        pub diffusion_bias: Option<Vec<f64>>,
        /// 图扩散允许的边标签；空数组表示禁止扩散。
        pub expand_labels: Option<Vec<String>>,
        pub max_edges_per_node: Option<f64>,
        pub min_edge_weight: Option<f64>,
        pub edge_direction: Option<String>,
    }

    /// 节点关系边
    #[napi(object)]
    pub struct JsEdge {
        pub target_id: f64,
        pub label: String,
        pub weight: f64,
        pub metadata: NodeJson,
    }

    /// 完整入边视图
    #[napi(object)]
    pub struct JsIncomingEdge {
        pub source_id: f64,
        pub target_id: f64,
        pub label: String,
        pub weight: f64,
        pub metadata: NodeJson,
    }

    /// 节点完整视图
    #[napi(object)]
    pub struct JsNodeView {
        pub id: f64,
        pub vector: Vec<f64>,
        pub payload: NodeJson,
        pub edges: Vec<JsEdge>,
        pub num_edges: u32,
    }

    /// Leiden 聚类结果结构
    #[napi(object)]
    pub struct JsClusterResult {
        /// 平铺数组: [nodeId1, clusterId1, nodeId2, clusterId2, ...]
        pub node_to_cluster: Vec<f64>,
        /// 平铺数组: [clusterId1, "label1", ...]
        pub cluster_labels: Vec<String>,
        /// 平铺首尾连接数组: [clusterId1, vector[0]...vector[dim], clusterId2, ...]
        pub centroids: Vec<f64>,
    }

    /// Leiden 聚类配置 (全部可选)
    #[napi(object)]
    pub struct JsLeidenConfig {
        /// 最小社区大小 (节点数 < 此值的碎片簇被丢弃, 默认 3)
        pub min_community_size: Option<f64>,
        /// 最大迭代轮次 (默认 15)
        pub max_iterations: Option<f64>,
        /// 是否计算质心 (默认 true)
        pub with_centroids: Option<bool>,
    }

    /// Hook 管线执行上下文（包含各阶段计时统计和自定义数据）
    #[napi(object)]
    pub struct JsHookContext {
        /// 各管线阶段的耗时统计（JSON 对象, 单位: 毫秒）
        pub timings: NodeJson,
        /// 每阶段候选数量
        pub counts: NodeJson,
        /// Hook 注入的自定义数据
        pub custom_data: NodeJson,
        pub observations: NodeJson,
        /// 管线是否被 Hook 提前终止
        pub aborted: bool,
    }

    /// 带上下文的检索结果
    #[napi(object)]
    pub struct JsSearchWithContextResult {
        /// 检索结果列表
        pub hits: Vec<JsSearchHit>,
        /// Hook 管线上下文
        pub context: JsHookContext,
    }

    fn parse_node_id(env: Env, value: f64, name: &str) -> napi::Result<u64> {
        Ok(parse_safe_usize(env, value, name)? as u64)
    }

    fn validate_u64_component(value: f64) -> crate::Result<u64> {
        if !value.is_finite()
            || value < 0.0
            || value.fract() != 0.0
            || value >= 18_446_744_073_709_551_616.0
        {
            return Err(crate::TriviumError::InvalidInput("u64 向量分量必须是 [0, 2^64) 内的有限整数 (u64 vector components must be finite integers in [0, 2^64))".into()));
        }
        Ok(value as u64)
    }

    fn parse_u64_component(env: Env, value: f64) -> napi::Result<u64> {
        validate_u64_component(value).map_err(|error| to_napi_error(env, error))
    }

    fn parse_u64_vector(env: Env, values: &[f64]) -> napi::Result<Vec<u64>> {
        values
            .iter()
            .copied()
            .map(|value| parse_u64_component(env, value))
            .collect()
    }

    fn parse_safe_usize(env: Env, value: f64, name: &str) -> napi::Result<usize> {
        if !value.is_finite()
            || value < 0.0
            || value.fract() != 0.0
            || value > 9_007_199_254_740_991.0
            || value > usize::MAX as f64
        {
            return Err(to_napi_error(
                env,
                crate::TriviumError::InvalidInput(format!(
                    "{name} 必须是 JavaScript 安全范围内的非负整数 ({name} must be a non-negative JavaScript safe integer)"
                )),
            ));
        }
        Ok(value as usize)
    }

    fn parse_positive_usize(env: Env, value: f64, name: &str) -> napi::Result<usize> {
        let value = parse_safe_usize(env, value, name)?;
        if value == 0 {
            return Err(to_napi_error(
                env,
                crate::TriviumError::InvalidInput(format!(
                    "{name} 必须大于零 ({name} must be positive)"
                )),
            ));
        }
        Ok(value)
    }

    fn node_id_to_js(env: Env, id: u64) -> napi::Result<f64> {
        if id > 9_007_199_254_740_991 {
            return Err(to_napi_error(env, crate::TriviumError::InvalidInput("节点 ID 超出 JavaScript 安全整数范围，请使用字符串 ID 的 TQL 接口 (Node ID exceeds the JavaScript safe-integer range; use TQL string IDs)".into())));
        }
        Ok(id as f64)
    }

    fn node_ids_to_js(env: Env, ids: impl IntoIterator<Item = u64>) -> napi::Result<Vec<f64>> {
        ids.into_iter()
            .map(|value| node_id_to_js(env, value))
            .collect()
    }

    fn parse_reachability_options(
        env: Env,
        options: Option<JsReachabilityOptions>,
    ) -> napi::Result<crate::graph::reachability::ReachabilityConfig> {
        let options = options.unwrap_or(JsReachabilityOptions {
            min_depth: None,
            max_depth: None,
            labels: None,
            direction: None,
            max_visited_nodes: None,
            max_results: None,
            max_edges: None,
        });
        let direction = match options.direction.as_deref().unwrap_or("outgoing") {
            "outgoing" => crate::graph::reachability::ReachabilityDirection::Outgoing,
            "incoming" => crate::graph::reachability::ReachabilityDirection::Incoming,
            "both" => crate::graph::reachability::ReachabilityDirection::Both,
            _ => {
                return Err(napi::Error::from_reason(
                    "direction 必须是 outgoing / incoming / both",
                ));
            }
        };
        Ok(crate::graph::reachability::ReachabilityConfig {
            min_depth: parse_safe_usize(env, options.min_depth.unwrap_or(1.0), "minDepth")?,
            max_depth: parse_safe_usize(env, options.max_depth.unwrap_or(1.0), "maxDepth")?,
            labels: options.labels,
            direction,
            max_visited_nodes: options
                .max_visited_nodes
                .map(|value| parse_safe_usize(env, value, "maxVisitedNodes"))
                .transpose()?
                .unwrap_or(10_000),
            max_results: options
                .max_results
                .map(|value| parse_safe_usize(env, value, "maxResults"))
                .transpose()?
                .unwrap_or(10_000),
            max_edges: options
                .max_edges
                .map(|value| parse_safe_usize(env, value, "maxEdges"))
                .transpose()?
                .unwrap_or(50_000),
            max_frontier_size: 10_000,
            exhaustion_policy: crate::graph::budget::BudgetExhaustionPolicy::Partial,
        })
    }

    fn build_transaction<T: crate::VectorType>(
        env: Env,
        operations: &[serde_json::Value],
        convert: impl Fn(f64) -> napi::Result<T>,
    ) -> napi::Result<crate::database::TxBuilder<T>> {
        let mut tx = crate::database::TxBuilder::new();
        for operation in operations {
            let kind = operation
                .get("type")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| napi::Error::from_reason("事务操作缺少 type"))?;
            let id = |name: &str| -> napi::Result<u64> {
                let value = operation
                    .get(name)
                    .and_then(serde_json::Value::as_f64)
                    .ok_or_else(|| napi::Error::from_reason(format!("事务操作缺少 {name}")))?;
                Ok(parse_safe_usize(env, value, name)? as u64)
            };
            let vector = || -> napi::Result<Vec<T>> {
                operation
                    .get("vector")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| napi::Error::from_reason("事务操作缺少 vector"))?
                    .iter()
                    .map(|value| {
                        value
                            .as_f64()
                            .ok_or_else(|| napi::Error::from_reason("vector 必须是数字数组"))
                            .and_then(&convert)
                    })
                    .collect()
            };
            match kind {
                "insert" => tx.insert(
                    &vector()?,
                    operation.get("payload").cloned().unwrap_or_default(),
                ),
                "insertWithId" => tx.insert_with_id(
                    id("id")?,
                    &vector()?,
                    operation.get("payload").cloned().unwrap_or_default(),
                ),
                "delete" => tx.delete(id("id")?),
                "updatePayload" => tx.update_payload(
                    id("id")?,
                    operation.get("payload").cloned().unwrap_or_default(),
                ),
                "updateVector" => tx.update_vector(id("id")?, &vector()?),
                "upsertEdge" | "link" => tx.upsert_edge(
                    id("src")?,
                    id("dst")?,
                    operation
                        .get("label")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("related"),
                    operation
                        .get("weight")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(1.0) as f32,
                    operation.get("metadata").cloned().unwrap_or_default(),
                ),
                "unlink" => tx.unlink(id("src")?, id("dst")?),
                "unlinkLabel" => tx.unlink_label(
                    id("src")?,
                    id("dst")?,
                    operation
                        .get("label")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| napi::Error::from_reason("事务操作缺少 label"))?,
                ),
                _ => {
                    return Err(napi::Error::from_reason(format!(
                        "不支持的事务操作类型: {kind}"
                    )));
                }
            }
        }
        Ok(tx)
    }

    fn to_js_reachability(
        env: Env,
        result: crate::graph::reachability::ReachabilityResult,
    ) -> napi::Result<JsReachabilityResult> {
        Ok(JsReachabilityResult {
            source_id: node_id_to_js(env, result.source_id)?,
            target_id: node_id_to_js(env, result.target_id)?,
            depth: result.depth as u32,
            path: result
                .path
                .into_iter()
                .map(|value| node_id_to_js(env, value))
                .collect::<napi::Result<_>>()?,
            steps: result
                .steps
                .into_iter()
                .map(|step| {
                    Ok(JsReachabilityStep {
                        from: node_id_to_js(env, step.from)?,
                        to: node_id_to_js(env, step.to)?,
                        label: step.label,
                        weight: round_api_f32(step.weight),
                        metadata: step.metadata.into(),
                    })
                })
                .collect::<napi::Result<_>>()?,
        })
    }

    fn parse_row_overflow(value: Option<&str>) -> napi::Result<crate::database::RowOverflowPolicy> {
        match value.unwrap_or("throw") {
            "throw" => Ok(crate::database::RowOverflowPolicy::Throw),
            "break" => Ok(crate::database::RowOverflowPolicy::Break),
            _ => Err(napi::Error::from_reason("rowOverflow 必须是 throw / break")),
        }
    }

    fn parse_storage_mode(value: Option<&str>) -> napi::Result<crate::database::StorageMode> {
        match value.unwrap_or("mmap") {
            "mmap" => Ok(crate::database::StorageMode::Mmap),
            "rom" => Ok(crate::database::StorageMode::Rom),
            _ => Err(napi::Error::from_reason("storageMode 必须是 mmap / rom")),
        }
    }

    fn parse_access_mode(value: Option<&str>) -> napi::Result<crate::database::AccessMode> {
        match value.unwrap_or("readWrite") {
            "readWrite" => Ok(crate::database::AccessMode::ReadWrite),
            "readOnly" => Ok(crate::database::AccessMode::ReadOnly),
            "immutable" => Ok(crate::database::AccessMode::Immutable),
            _ => Err(napi::Error::from_reason(
                "accessMode 必须是 readWrite / readOnly / immutable",
            )),
        }
    }

    fn parse_missing_index_policy(
        value: Option<&str>,
    ) -> napi::Result<crate::database::MissingIndexPolicy> {
        match value.unwrap_or("fallback") {
            "fallback" => Ok(crate::database::MissingIndexPolicy::Fallback),
            "buildInMemory" => Ok(crate::database::MissingIndexPolicy::BuildInMemory),
            "error" => Ok(crate::database::MissingIndexPolicy::Error),
            _ => Err(napi::Error::from_reason(
                "missingIndexPolicy 必须是 fallback / buildInMemory / error",
            )),
        }
    }

    fn parse_sync_mode(s: &str) -> napi::Result<crate::storage::wal::SyncMode> {
        crate::storage::wal::SyncMode::parse(s).map_err(napi::Error::from_reason)
    }

    fn parse_payload_filter(
        value: Option<serde_json::Value>,
    ) -> napi::Result<Option<crate::filter::Filter>> {
        value
            .map(|filter| {
                crate::filter::Filter::from_json(&filter).map_err(|error| {
                    napi::Error::from_reason(format!(
                        "payloadFilter 无效 (Invalid payloadFilter): {error}"
                    ))
                })
            })
            .transpose()
    }

    fn parse_edge_direction(value: Option<&str>) -> napi::Result<crate::database::EdgeDirection> {
        match value.unwrap_or("out") {
            "out" | "outgoing" => Ok(crate::database::EdgeDirection::Outgoing),
            "in" | "incoming" => Ok(crate::database::EdgeDirection::Incoming),
            "both" => Ok(crate::database::EdgeDirection::Both),
            _ => Err(napi::Error::from_reason(
                "edgeDirection 必须是 out / in / both",
            )),
        }
    }

    // ════════ TriviumDB 主类 ════════

    #[napi(js_name = "TriviumDB")]
    pub struct TriviumDB {
        inner: DbBackend,
        dtype: String,
    }

    impl TriviumDB {
        fn check_generated_ids(&self, env: Env, count: usize) -> napi::Result<()> {
            if count == 0 {
                return Ok(());
            }
            let next = dispatch!(self,db=>db.next_id_for_binding())
                .map_err(|error| to_napi_error(env, error))?;
            let last = next.checked_add(count as u64 - 1).ok_or_else(|| {
                napi::Error::from_reason("节点 ID 分配溢出 (Node ID allocation overflow)")
            })?;
            node_id_to_js(env, last)?;
            Ok(())
        }

        fn check_transaction_ids(
            &self,
            env: Env,
            operations: &[serde_json::Value],
        ) -> napi::Result<()> {
            let mut next = dispatch!(self,db=>db.next_id_for_binding())
                .map_err(|error| to_napi_error(env, error))?;
            for operation in operations {
                match operation.get("type").and_then(serde_json::Value::as_str) {
                    Some("insert") => {
                        node_id_to_js(env, next)?;
                        next += 1;
                    }
                    Some("insertWithId") => {
                        let id = operation
                            .get("id")
                            .and_then(serde_json::Value::as_f64)
                            .ok_or_else(|| {
                                napi::Error::from_reason("缺少事务 ID (Missing transaction ID)")
                            })?;
                        next = next.max(parse_node_id(env, id, "id")? + 1);
                    }
                    _ => {}
                }
            }
            Ok(())
        }
    }

    #[napi]
    impl TriviumDB {
        /// 打开或创建数据库
        ///
        /// ```js
        /// const db = new TriviumDB("data.tdb", { dim: 1536, dtype: "f32", syncMode: "normal" })
        /// ```
        #[napi(constructor)]
        pub fn new(
            env: Env,
            path: String,
            options: Option<napi::bindgen_prelude::Either<f64, JsDatabaseOptions>>,
        ) -> napi::Result<Self> {
            let options = match options {
                Some(napi::bindgen_prelude::Either::A(_)) => {
                    return Err(napi::Error::from_reason(
                        "TDB_API_MIGRATION_REQUIRED: 数字位置参数已移除，请使用 options 对象 (numeric positional arguments were removed; use an options object)",
                    ));
                }
                Some(napi::bindgen_prelude::Either::B(options)) => options,
                None => JsDatabaseOptions {
                    runtime: None,
                    dim: None,
                    dtype: None,
                    sync_mode: None,
                    storage_mode: None,
                    auto_build_quiver: None,
                    load_text_index: None,
                    expected_nodes: None,
                    memory_limit_mb: None,
                    payload_cache_mb: None,
                    payload_cache_entry_mb: None,
                    access_mode: None,
                    missing_index_policy: None,
                    max_query_rows: None,
                    row_overflow: None,
                },
            };
            let dim = parse_positive_usize(env, options.dim.unwrap_or(1536.0), "dim")?;
            let dtype = options.dtype.unwrap_or_else(|| "f32".into());
            let expected_nodes = options
                .expected_nodes
                .map(|value| parse_safe_usize(env, value, "expectedNodes"))
                .transpose()?;
            let memory_limit_mb = options
                .memory_limit_mb
                .map(|value| parse_safe_usize(env, value, "memoryLimitMb"))
                .transpose()?
                .unwrap_or(0);
            let memory_limit = memory_limit_mb
                .checked_mul(1024 * 1024)
                .ok_or_else(|| napi::Error::from_reason("memoryLimitMb 换算字节时溢出"))?;
            let mb_to_bytes = |value: Option<f64>, name: &str, default_mb: usize| {
                value
                    .map(|value| parse_safe_usize(env, value, name))
                    .transpose()?
                    .unwrap_or(default_mb)
                    .checked_mul(1024 * 1024)
                    .ok_or_else(|| napi::Error::from_reason(format!("{name} 换算字节时溢出")))
            };
            let payload_cache_bytes = mb_to_bytes(options.payload_cache_mb, "payloadCacheMb", 64)?;
            let payload_cache_entry_bytes =
                mb_to_bytes(options.payload_cache_entry_mb, "payloadCacheEntryMb", 8)?;
            let max_query_rows = options
                .max_query_rows
                .map(|value| parse_safe_usize(env, value, "maxQueryRows"))
                .transpose()?;
            let config = crate::database::Config {
                dim,
                sync_mode: parse_sync_mode(options.sync_mode.as_deref().unwrap_or("normal"))?,
                storage_mode: parse_storage_mode(options.storage_mode.as_deref())?,
                auto_build_quiver: options.auto_build_quiver.unwrap_or(true),
                load_text_index: options.load_text_index.unwrap_or(false),
                expected_nodes,
                memory_limit,
                payload_cache_bytes,
                payload_cache_entry_bytes,
                access_mode: parse_access_mode(options.access_mode.as_deref())?,
                missing_index_policy: parse_missing_index_policy(
                    options.missing_index_policy.as_deref(),
                )?,
                max_query_rows,
                row_overflow: parse_row_overflow(options.row_overflow.as_deref())?,
            };
            let runtime = parse_runtime(options.runtime)?;
            let dtype_str = dtype.as_str();

            let inner = match dtype_str {
                "f32" => DbBackend::F32(
                    GenericDatabase::<f32>::open_with_runtime(&path, config, runtime.clone())
                        .map_err(|error| to_napi_error(env, error))?,
                ),
                "f16" => DbBackend::F16(
                    GenericDatabase::<half::f16>::open_with_runtime(&path, config, runtime.clone())
                        .map_err(|error| to_napi_error(env, error))?,
                ),
                "u64" => DbBackend::U64(
                    GenericDatabase::<u64>::open_with_runtime(&path, config, runtime.clone())
                        .map_err(|error| to_napi_error(env, error))?,
                ),
                _ => return Err(napi::Error::from_reason("dtype 必须是 f32 / f16 / u64")),
            };
            Ok(Self {
                inner,
                dtype: dtype_str.to_string(),
            })
        }

        // ── Hook 管理 ──

        #[napi]
        pub fn set_hook(
            &mut self,
            env: Env,
            this: This<JsObject>,
            hook: JsObject,
        ) -> napi::Result<()> {
            dispatch!(self, mut db => db.set_hook_with(
                || Ok(JsNativeHook {
                    object: super::super::node_hook_refs::HookReference::new(env, this, hook)?,
                    owner_thread: std::thread::current().id(),
                }),
                |error| to_napi_error(env, error),
            ))
        }

        /// 加载 C/C++ 动态库作为检索管线 Hook
        ///
        /// 动态库需导出 C ABI 符号（均可选）：
        /// - `trivium_recall`: 自定义召回
        /// - `trivium_rerank`: 自定义重排序
        ///
        /// ```js
        /// db.loadFfiHook('./libmy_plugin.so')
        /// const results = db.search(queryVec)  // 自动经过 C++ Hook
        /// ```
        #[napi]
        pub fn load_ffi_hook(&mut self, env: Env, lib_path: String) -> napi::Result<()> {
            let ffi_hook =
                crate::hook::FfiHook::load(&lib_path).map_err(|error| to_napi_error(env, error))?;
            dispatch!(self, mut db => db.set_hook(ffi_hook))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 清除当前已注册的 Hook，恢复为默认的零开销 NoopHook
        #[napi]
        pub fn clear_hook(&mut self) {
            dispatch!(self, mut db => db.clear_hook());
        }

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
            query_vector: Vec<f64>,
            config: Option<JsSearchConfig>,
        ) -> napi::Result<JsSearchWithContextResult> {
            let mut cfg = config.unwrap_or(JsSearchConfig {
                top_k: None,
                recall_k: None,
                rerank_k: None,
                expand_depth: None,
                min_score: None,
                teleport_alpha: None,
                enable_advanced_pipeline: None,
                enable_sparse_residual: None,
                fista_lambda: None,
                fista_threshold: None,
                enable_dpp: None,
                dpp_quality_weight: None,
                enable_refractory_fatigue: None,
                custom_query_text: None,
                payload_filter: None,
                enable_text_hybrid_search: None,
                text_boost: None,
                force_brute_force: None,
                diffusion_bias: None,
                expand_labels: None,
                max_edges_per_node: None,
                min_edge_weight: None,
                edge_direction: None,
            });

            let top_k = parse_positive_usize(env, cfg.top_k.unwrap_or(5.0), "topK")?;
            let payload_filter =
                parse_payload_filter(cfg.payload_filter.take().map(|value| value.0))?;
            let core_config = crate::database::SearchConfig {
                top_k,
                recall_k: parse_safe_usize(env, cfg.recall_k.unwrap_or(0.0), "recallK")?,
                rerank_k: parse_safe_usize(env, cfg.rerank_k.unwrap_or(0.0), "rerankK")?,
                expand_depth: parse_safe_usize(
                    env,
                    cfg.expand_depth.unwrap_or(2.0),
                    "expandDepth",
                )?,
                min_score: cfg.min_score.unwrap_or(0.1) as f32,
                teleport_alpha: cfg.teleport_alpha.unwrap_or(0.0) as f32,
                enable_advanced_pipeline: cfg.enable_advanced_pipeline.unwrap_or(false),
                enable_sparse_residual: cfg.enable_sparse_residual.unwrap_or(false),
                fista_lambda: cfg.fista_lambda.unwrap_or(0.1) as f32,
                fista_threshold: cfg.fista_threshold.unwrap_or(0.3) as f32,
                enable_dpp: cfg.enable_dpp.unwrap_or(false),
                dpp_quality_weight: cfg.dpp_quality_weight.unwrap_or(1.0) as f32,
                enable_refractory_fatigue: cfg.enable_refractory_fatigue.unwrap_or(false),
                enable_text_hybrid_search: cfg.enable_text_hybrid_search.unwrap_or(false),
                text_boost: cfg.text_boost.unwrap_or(1.5) as f32,
                force_brute_force: cfg.force_brute_force.unwrap_or(false),
                diffusion_bias: cfg
                    .diffusion_bias
                    .map(|v| v.into_iter().map(|x| x as f32).collect()),
                expand_labels: cfg.expand_labels,
                max_edges_per_node: cfg
                    .max_edges_per_node
                    .map(|value| parse_safe_usize(env, value, "maxEdgesPerNode"))
                    .transpose()?
                    .unwrap_or(0),
                min_edge_weight: cfg.min_edge_weight.unwrap_or(0.0) as f32,
                edge_direction: parse_edge_direction(cfg.edge_direction.as_deref())?,
                payload_filter,
                ..Default::default()
            };

            let q_text = cfg.custom_query_text.as_deref();

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

        // ── CRUD ──

        /// 插入节点，返回新节点 ID
        #[napi]
        pub fn insert(
            &mut self,
            env: Env,
            vector: Vec<f64>,
            payload: NodeJson,
        ) -> napi::Result<f64> {
            let payload = payload.0;
            self.check_generated_ids(env, 1)?;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    db.insert(&v, payload)
                        .map_err(|error| to_napi_error(env, error))
                        .and_then(|value| node_id_to_js(env, value))
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> =
                        vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.insert(&v, payload)
                        .map_err(|error| to_napi_error(env, error))
                        .and_then(|value| node_id_to_js(env, value))
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = parse_u64_vector(env, &vector)?;
                    db.insert(&v, payload)
                        .map_err(|error| to_napi_error(env, error))
                        .and_then(|value| node_id_to_js(env, value))
                }
            }
        }

        /// 批量插入节点，返回新分配的 ID 列表
        #[napi]
        pub fn batch_insert(
            &mut self,
            env: Env,
            vectors: Vec<Vec<f64>>,
            payloads: Vec<NodeJson>,
        ) -> napi::Result<Vec<f64>> {
            let payloads = payloads
                .into_iter()
                .map(|value| value.0)
                .collect::<Vec<_>>();
            if vectors.len() != payloads.len() {
                return Err(napi::Error::from_reason("向量列表与负载列表长度不一致"));
            }
            self.check_generated_ids(env, vectors.len())?;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let converted: Vec<Vec<f32>> = vectors
                        .into_iter()
                        .map(|vector| vector.into_iter().map(|value| value as f32).collect())
                        .collect();
                    let mut tx = crate::database::TxBuilder::new();
                    for (vector, payload) in converted.iter().zip(payloads) {
                        tx.insert(vector, payload);
                    }
                    db.commit_tx(tx)
                        .map_err(|error| to_napi_error(env, error))
                        .and_then(|value| node_ids_to_js(env, value))
                }
                DbBackend::F16(db) => {
                    let converted: Vec<Vec<half::f16>> = vectors
                        .into_iter()
                        .map(|vector| vector.into_iter().map(half::f16::from_f64).collect())
                        .collect();
                    let mut tx = crate::database::TxBuilder::new();
                    for (vector, payload) in converted.iter().zip(payloads) {
                        tx.insert(vector, payload);
                    }
                    db.commit_tx(tx)
                        .map_err(|error| to_napi_error(env, error))
                        .and_then(|value| node_ids_to_js(env, value))
                }
                DbBackend::U64(db) => {
                    let converted: Vec<Vec<u64>> = vectors
                        .iter()
                        .map(|vector| parse_u64_vector(env, vector))
                        .collect::<napi::Result<_>>()?;
                    let mut tx = crate::database::TxBuilder::new();
                    for (vector, payload) in converted.iter().zip(payloads) {
                        tx.insert(vector, payload);
                    }
                    db.commit_tx(tx)
                        .map_err(|error| to_napi_error(env, error))
                        .and_then(|value| node_ids_to_js(env, value))
                }
            }
        }

        /// 批量插入指定 ID 的节点
        #[napi]
        pub fn batch_insert_with_ids(
            &mut self,
            env: Env,
            ids: Vec<f64>,
            vectors: Vec<Vec<f64>>,
            payloads: Vec<NodeJson>,
        ) -> napi::Result<()> {
            let payloads = payloads
                .into_iter()
                .map(|value| value.0)
                .collect::<Vec<_>>();
            if ids.len() != vectors.len() || vectors.len() != payloads.len() {
                return Err(napi::Error::from_reason("ID、向量与负载列表长度不一致"));
            }
            let parsed_ids: Vec<u64> = ids
                .into_iter()
                .map(|id| parse_safe_usize(env, id, "id").map(|value| value as u64))
                .collect::<napi::Result<_>>()?;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let converted: Vec<Vec<f32>> = vectors
                        .into_iter()
                        .map(|vector| vector.into_iter().map(|value| value as f32).collect())
                        .collect();
                    let mut tx = crate::database::TxBuilder::new();
                    for ((id, vector), payload) in
                        parsed_ids.iter().zip(converted.iter()).zip(payloads)
                    {
                        tx.insert_with_id(*id, vector, payload);
                    }
                    db.commit_tx(tx)
                        .map(|_| ())
                        .map_err(|error| to_napi_error(env, error))
                }
                DbBackend::F16(db) => {
                    let converted: Vec<Vec<half::f16>> = vectors
                        .into_iter()
                        .map(|vector| vector.into_iter().map(half::f16::from_f64).collect())
                        .collect();
                    let mut tx = crate::database::TxBuilder::new();
                    for ((id, vector), payload) in
                        parsed_ids.iter().zip(converted.iter()).zip(payloads)
                    {
                        tx.insert_with_id(*id, vector, payload);
                    }
                    db.commit_tx(tx)
                        .map(|_| ())
                        .map_err(|error| to_napi_error(env, error))
                }
                DbBackend::U64(db) => {
                    let converted: Vec<Vec<u64>> = vectors
                        .iter()
                        .map(|vector| parse_u64_vector(env, vector))
                        .collect::<napi::Result<_>>()?;
                    let mut tx = crate::database::TxBuilder::new();
                    for ((id, vector), payload) in
                        parsed_ids.iter().zip(converted.iter()).zip(payloads)
                    {
                        tx.insert_with_id(*id, vector, payload);
                    }
                    db.commit_tx(tx)
                        .map(|_| ())
                        .map_err(|error| to_napi_error(env, error))
                }
            }
        }

        /// 带指定 ID 插入节点
        #[napi]
        pub fn insert_with_id(
            &mut self,
            env: Env,
            id: f64,
            vector: Vec<f64>,
            payload: NodeJson,
        ) -> napi::Result<()> {
            let payload = payload.0;
            let id = parse_safe_usize(env, id, "id")? as u64;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    db.insert_with_id(id, &v, payload)
                        .map_err(|error| to_napi_error(env, error))
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> =
                        vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.insert_with_id(id, &v, payload)
                        .map_err(|error| to_napi_error(env, error))
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = parse_u64_vector(env, &vector)?;
                    db.insert_with_id(id, &v, payload)
                        .map_err(|error| to_napi_error(env, error))
                }
            }
        }

        /// 使用自定义 ID 插入或覆盖节点。
        #[napi]
        pub fn upsert_with_id(
            &mut self,
            env: Env,
            id: f64,
            vector: Vec<f64>,
            payload: NodeJson,
        ) -> napi::Result<()> {
            let payload = payload.0;
            let id = parse_safe_usize(env, id, "id")? as u64;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let vector = vector
                        .into_iter()
                        .map(|value| value as f32)
                        .collect::<Vec<_>>();
                    db.upsert_with_id(id, &vector, payload)
                }
                DbBackend::F16(db) => {
                    let vector = vector
                        .into_iter()
                        .map(half::f16::from_f64)
                        .collect::<Vec<_>>();
                    db.upsert_with_id(id, &vector, payload)
                }
                DbBackend::U64(db) => {
                    let vector = parse_u64_vector(env, &vector)?;
                    db.upsert_with_id(id, &vector, payload)
                }
            }
            .map_err(|error| to_napi_error(env, error))
        }

        /// 提交缓冲操作，返回新插入的节点 ID。
        #[napi]
        pub fn commit_transaction(
            &mut self,
            env: Env,
            operations: Vec<NodeJson>,
        ) -> napi::Result<Vec<f64>> {
            let operations = operations
                .into_iter()
                .map(|value| value.0)
                .collect::<Vec<_>>();
            self.check_transaction_ids(env, &operations)?;
            let ids = match &mut self.inner {
                DbBackend::F32(db) => db.commit_tx(build_transaction(env, &operations, |value| {
                    Ok(value as f32)
                })?),
                DbBackend::F16(db) => db.commit_tx(build_transaction(env, &operations, |value| {
                    Ok(half::f16::from_f64(value))
                })?),
                DbBackend::U64(db) => db.commit_tx(build_transaction(env, &operations, |value| {
                    parse_u64_component(env, value)
                })?),
            }
            .map_err(|error| to_napi_error(env, error))?;
            ids.into_iter()
                .map(|value| node_id_to_js(env, value))
                .collect::<napi::Result<_>>()
        }

        #[napi]
        pub fn graph_stats(&self, env: Env) -> napi::Result<NodeJson> {
            node_json_value(
                dispatch!(self, db => db.graph_stats().map_err(|error| to_napi_error(env, error))?),
            )
            .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        #[napi]
        pub fn validate_graph(&self, env: Env) -> napi::Result<NodeJson> {
            node_json_value(dispatch!(self, db => db.validate_graph().map_err(|error| to_napi_error(env, error))?))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        #[napi]
        pub fn repair_graph_indexes(&mut self, env: Env) -> napi::Result<NodeJson> {
            let report = dispatch!(self, mut db => db.repair_graph_indexes())
                .map_err(|error| to_napi_error(env, error))?;
            node_json_value(report).map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        #[napi]
        pub fn get(&self, env: Env, id: f64) -> napi::Result<Option<JsNodeView>> {
            {
                let id = parse_node_id(env, id, "id")?;
                match &self.inner {
                    DbBackend::F32(db) => db
                        .get(id)
                        .map_err(|error| to_napi_error(env, error))?
                        .map(|n| {
                            let num_edges = n.edges.len() as u32;
                            let edges_arr = n
                                .edges
                                .into_iter()
                                .map(|e| {
                                    Ok(JsEdge {
                                        target_id: node_id_to_js(env, e.target_id)?,
                                        label: e.label.clone(),
                                        weight: round_api_f32(e.weight),
                                        metadata: e.metadata.clone().into(),
                                    })
                                })
                                .collect::<napi::Result<_>>()?;
                            Ok(JsNodeView {
                                id: node_id_to_js(env, n.id)?,
                                vector: n.vector.iter().map(|&x| x as f64).collect(),
                                payload: n.payload.into(),
                                edges: edges_arr,
                                num_edges,
                            })
                        })
                        .transpose(),
                    DbBackend::F16(db) => db
                        .get(id)
                        .map_err(|error| to_napi_error(env, error))?
                        .map(|n| {
                            let num_edges = n.edges.len() as u32;
                            let edges_arr = n
                                .edges
                                .into_iter()
                                .map(|e| {
                                    Ok(JsEdge {
                                        target_id: node_id_to_js(env, e.target_id)?,
                                        label: e.label.clone(),
                                        weight: round_api_f32(e.weight),
                                        metadata: e.metadata.clone().into(),
                                    })
                                })
                                .collect::<napi::Result<_>>()?;
                            Ok(JsNodeView {
                                id: node_id_to_js(env, n.id)?,
                                vector: n.vector.iter().map(|x| x.to_f64()).collect(),
                                payload: n.payload.into(),
                                edges: edges_arr,
                                num_edges,
                            })
                        })
                        .transpose(),
                    DbBackend::U64(db) => db
                        .get(id)
                        .map_err(|error| to_napi_error(env, error))?
                        .map(|n| {
                            let num_edges = n.edges.len() as u32;
                            let edges_arr = n
                                .edges
                                .into_iter()
                                .map(|e| {
                                    Ok(JsEdge {
                                        target_id: node_id_to_js(env, e.target_id)?,
                                        label: e.label.clone(),
                                        weight: round_api_f32(e.weight),
                                        metadata: e.metadata.clone().into(),
                                    })
                                })
                                .collect::<napi::Result<_>>()?;
                            Ok(JsNodeView {
                                id: node_id_to_js(env, n.id)?,
                                vector: n.vector.iter().map(|&x| x as f64).collect(),
                                payload: n.payload.into(),
                                edges: edges_arr,
                                num_edges,
                            })
                        })
                        .transpose(),
                }
            }
        }

        /// 更新节点元数据
        #[napi]
        pub fn update_payload(&mut self, env: Env, id: f64, payload: NodeJson) -> napi::Result<()> {
            let payload = payload.0;
            dispatch!(self, mut db => db.update_payload(parse_node_id(env, id, "id")?, payload))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 部分更新节点 Payload（$set / $inc / $unset）
        ///
        /// 只修改指定字段，其他字段保持不变。
        ///
        /// ```js
        /// db.patchPayload(id, { $set: { name: "Alice" } })
        /// db.patchPayload(id, { $inc: { visits: 1 } })
        /// db.patchPayload(id, { $unset: { oldField: true } })
        /// db.patchPayload(id, { name: "Bob" })  // 简写，等价于 $set
        /// ```
        #[napi]
        pub fn patch_payload(&mut self, env: Env, id: f64, patch: NodeJson) -> napi::Result<()> {
            let patch = patch.0;
            dispatch!(self, mut db => db.patch_payload(parse_node_id(env, id, "id")?, patch))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 更新节点向量
        #[napi]
        pub fn update_vector(&mut self, env: Env, id: f64, vector: Vec<f64>) -> napi::Result<()> {
            let id = parse_node_id(env, id, "id")?;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    db.update_vector(id, &v)
                        .map_err(|error| to_napi_error(env, error))
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> =
                        vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.update_vector(id, &v)
                        .map_err(|error| to_napi_error(env, error))
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = parse_u64_vector(env, &vector)?;
                    db.update_vector(id, &v)
                        .map_err(|error| to_napi_error(env, error))
                }
            }
        }

        #[napi]
        pub fn delete_many_atomic(&mut self, env: Env, ids: Vec<f64>) -> napi::Result<u32> {
            let ids = ids
                .into_iter()
                .map(|id| parse_node_id(env, id, "id"))
                .collect::<napi::Result<Vec<_>>>()?;
            let deleted = dispatch!(self, mut db => db.delete_many_atomic(&ids))
                .map_err(|error| to_napi_error(env, error))?;
            u32::try_from(deleted).map_err(|_| napi::Error::from_reason("删除数量超出 u32"))
        }

        #[napi]
        pub fn compare_and_set_payload_field(
            &mut self,
            env: Env,
            id: f64,
            field: String,
            expected: NodeJson,
            replacement: NodeJson,
        ) -> napi::Result<()> {
            let expected = expected.0;
            let replacement = replacement.0;
            dispatch!(self, mut db => db.compare_and_set_payload_field(parse_node_id(env, id, "id")?, &field, &expected, replacement))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 删除节点（三层原子联删：向量 + Payload + 所有关联边）
        #[napi]
        pub fn delete(&mut self, env: Env, id: f64) -> napi::Result<()> {
            dispatch!(self, mut db => db.delete(parse_node_id(env, id, "id")?))
                .map_err(|error| to_napi_error(env, error))
        }

        // ── 图谱操作 ──

        /// 建立有向带权边
        #[napi]
        pub fn link(
            &mut self,
            env: Env,
            src: f64,
            dst: f64,
            label: Option<String>,
            weight: Option<f64>,
        ) -> napi::Result<()> {
            let label = label.as_deref().unwrap_or("related");
            let weight = weight.unwrap_or(1.0) as f32;
            dispatch!(self, mut db => db.link(parse_node_id(env, src, "src")?, parse_node_id(env, dst, "dst")?, label, weight))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn get_edge(
            &self,
            env: Env,
            src: f64,
            dst: f64,
            label: String,
        ) -> napi::Result<Option<JsEdge>> {
            Ok({
                dispatch!(self, db => db.get_edge(parse_node_id(env, src, "src")?, parse_node_id(env, dst, "dst")?, &label).map_err(|error| to_napi_error(env, error))?).map(|edge| Ok::<_,napi::Error>(JsEdge {
                target_id: node_id_to_js(env, edge.target_id)?,
                label: edge.label,
                weight: round_api_f32(edge.weight),
                metadata: edge.metadata.into(),
            })).transpose()?
            })
        }

        #[napi]
        pub fn upsert_edge(
            &mut self,
            env: Env,
            src: f64,
            dst: f64,
            label: String,
            weight: f64,
            metadata: Option<NodeJson>,
        ) -> napi::Result<()> {
            let metadata = metadata.map(|value| value.0);
            dispatch!(self, mut db => db.upsert_edge(
                parse_node_id(env, src, "src")?,
                parse_node_id(env, dst, "dst")?,
                &label,
                weight as f32,
                metadata.unwrap_or(serde_json::Value::Null),
            ))
            .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn update_edge(
            &mut self,
            env: Env,
            src: f64,
            dst: f64,
            label: String,
            weight: Option<f64>,
            metadata: Option<NodeJson>,
        ) -> napi::Result<()> {
            let metadata = metadata.map(|value| value.0);
            dispatch!(self, mut db => db.update_edge(
                parse_node_id(env, src, "src")?,
                parse_node_id(env, dst, "dst")?,
                &label,
                weight.map(|value| value as f32),
                metadata,
            ))
            .map_err(|error| to_napi_error(env, error))
        }

        /// 断开两节点间的边；提供 label 时仅删除该标签。
        #[napi]
        pub fn unlink(
            &mut self,
            env: Env,
            src: f64,
            dst: f64,
            label: Option<String>,
        ) -> napi::Result<()> {
            match label {
                Some(label) => {
                    dispatch!(self, mut db => db.unlink_label(parse_node_id(env, src, "src")?, parse_node_id(env, dst, "dst")?, &label))
                }
                None => dispatch!(self, mut db => db.unlink(parse_node_id(env, src, "src")?, parse_node_id(env, dst, "dst")?)),
            }
            .map_err(|error| to_napi_error(env, error))
        }

        /// 获取 N 跳邻居节点 ID 列表，可按标签白名单遍历。
        #[napi]
        pub fn neighbors(
            &self,
            env: Env,
            id: f64,
            depth: Option<f64>,
            labels: Option<Vec<String>>,
        ) -> napi::Result<Vec<f64>> {
            Ok({
                let depth = parse_safe_usize(env, depth.unwrap_or(1.0), "depth")?;
                dispatch!(self, db => db.neighbors_with_labels(parse_node_id(env, id, "id")?, depth, labels.as_deref()).map_err(|error| to_napi_error(env, error))?)
                .into_iter()
                .map(|value| node_id_to_js(env, value)).collect::<napi::Result<_>>()?
            })
        }

        #[napi]
        pub fn reachable(
            &self,
            env: Env,
            id: f64,
            options: Option<JsReachabilityOptions>,
        ) -> napi::Result<Vec<JsReachabilityResult>> {
            let config = parse_reachability_options(env, options)?;
            dispatch!(self, db => db.reachable(parse_node_id(env, id, "id")?, &config))
                .map_err(|error| to_napi_error(env, error))?
                .into_iter()
                .map(|value| to_js_reachability(env, value))
                .collect()
        }

        #[napi]
        pub fn reachable_detailed(
            &self,
            env: Env,
            id: f64,
            options: Option<JsReachabilityOptions>,
        ) -> napi::Result<JsReachabilityOutput> {
            let config = parse_reachability_options(env, options)?;
            let output =
                dispatch!(self, db => db.reachable_detailed(parse_node_id(env, id, "id")?, &config))
                    .map_err(|error| to_napi_error(env, error))?;
            Ok(JsReachabilityOutput {
                results: output
                    .results
                    .into_iter()
                    .map(|value| to_js_reachability(env, value))
                    .collect::<napi::Result<_>>()?,
                visited_nodes: output.visited_nodes as f64,
                traversed_edges: output.traversed_edges as f64,
                truncated: output.truncated,
            })
        }

        #[napi]
        pub fn query_subgraph(
            &self,
            env: Env,
            id: f64,
            options: Option<JsReachabilityOptions>,
        ) -> napi::Result<JsSubgraphResult> {
            let config = parse_reachability_options(env, options)?;
            let output =
                dispatch!(self, db => db.query_subgraph(parse_node_id(env, id, "id")?, &config))
                    .map_err(|error| to_napi_error(env, error))?;
            Ok(JsSubgraphResult {
                nodes: output
                    .nodes
                    .into_iter()
                    .map(|node| {
                        Ok(JsSubgraphNode {
                            id: node_id_to_js(env, node.id)?,
                            payload: node.payload.into(),
                        })
                    })
                    .collect::<napi::Result<_>>()?,
                edges: output
                    .edges
                    .into_iter()
                    .map(|edge| {
                        Ok(JsSubgraphEdge {
                            source_id: node_id_to_js(env, edge.source_id)?,
                            target_id: node_id_to_js(env, edge.target_id)?,
                            label: edge.label,
                            weight: round_api_f32(edge.weight),
                            metadata: edge.metadata.into(),
                        })
                    })
                    .collect::<napi::Result<_>>()?,
                visited_nodes: output.visited_nodes as f64,
                traversed_edges: output.traversed_edges as f64,
                truncated: output.truncated,
            })
        }

        #[napi]
        pub fn search_graph_first(
            &self,
            env: Env,
            query_vector: Vec<f64>,
            anchor_ids: Vec<f64>,
            top_k: f64,
            max_anchor_nodes: Option<f64>,
        ) -> napi::Result<Vec<JsSearchHit>> {
            let top_k = parse_positive_usize(env, top_k, "topK")?;
            let anchors: Vec<u64> = anchor_ids
                .into_iter()
                .map(|id| parse_node_id(env, id, "anchorId"))
                .collect::<napi::Result<_>>()?;
            let max_anchor_nodes = max_anchor_nodes
                .map(|value| parse_safe_usize(env, value, "maxAnchorNodes"))
                .transpose()?
                .unwrap_or(100_000);
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
            query_vector: Vec<f64>,
            top_k: f64,
        ) -> napi::Result<Vec<JsSearchHit>> {
            let top_k = parse_positive_usize(env, top_k, "topK")?;
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

        // ── 社区聚类 ──

        /// 基于物理记忆图谱进行 Leiden 社区发现
        ///
        /// **无锁设计**: 短暂持锁快照邻接表后立即释放，聚类在锁外计算。
        /// 调用期间数据库仍可正常读写。
        #[napi]
        pub fn leiden_cluster(
            &self,
            env: Env,
            config: Option<JsLeidenConfig>,
        ) -> napi::Result<JsClusterResult> {
            let cfg = config.unwrap_or(JsLeidenConfig {
                min_community_size: None,
                max_iterations: None,
                with_centroids: None,
            });
            let min_c = parse_safe_usize(
                env,
                cfg.min_community_size.unwrap_or(3.0),
                "minCommunitySize",
            )?;
            let max_iter = cfg
                .max_iterations
                .map(|v| parse_safe_usize(env, v, "maxIterations"))
                .transpose()?;
            let with_cent = cfg.with_centroids;

            let result = dispatch!(self, db => db.leiden_cluster(min_c, max_iter, with_cent))
                .map_err(|error| to_napi_error(env, error))?;

            // 排序确保确定性输出
            let mut sorted_nodes: Vec<_> = result.node_to_cluster.into_iter().collect();
            sorted_nodes.sort_by_key(|&(id, _)| id);

            let mut node_to_cluster = Vec::with_capacity(sorted_nodes.len() * 2);
            for (n, c) in sorted_nodes {
                node_to_cluster.push(node_id_to_js(env, n)?);
                node_to_cluster.push(f64::from(c));
            }

            // 簇标签: 排序后输出
            let mut sorted_sizes: Vec<_> = result.cluster_sizes.iter().collect();
            sorted_sizes.sort_by_key(|(c, _)| *c);

            let mut cluster_labels = Vec::with_capacity(sorted_sizes.len() * 2);
            for (c, size) in &sorted_sizes {
                cluster_labels.push(c.to_string());
                cluster_labels.push(format!("Cluster {} ({})", c, size));
            }

            // 质心: 排序后平铺
            let mut sorted_centroids: Vec<_> = result.centroids.into_iter().collect();
            sorted_centroids.sort_by_key(|(c, _)| *c);

            let mut centroids = Vec::new();
            for (c, v) in sorted_centroids {
                centroids.push(f64::from(c));
                for val in v {
                    centroids.push(val as f64);
                }
            }

            Ok(JsClusterResult {
                node_to_cluster,
                cluster_labels,
                centroids,
            })
        }

        // ── 向量检索 ──

        /// 混合检索：向量锚定 + 图谱扩散
        #[napi]
        pub fn search(
            &self,
            env: Env,
            query_vector: Vec<f64>,
            top_k: Option<f64>,
            expand_depth: Option<f64>,
            min_score: Option<f64>,
            payload_filter: Option<NodeJson>,
        ) -> napi::Result<Vec<JsSearchHit>> {
            let payload_filter = payload_filter.map(|value| value.0);
            let top_k = parse_positive_usize(env, top_k.unwrap_or(5.0), "topK")?;

            let expand_depth = parse_safe_usize(env, expand_depth.unwrap_or(0.0), "expandDepth")?;
            let min_score = min_score.unwrap_or(0.5) as f32;
            let payload_filter = parse_payload_filter(payload_filter)?;

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
            query_vector: Vec<f64>,
            top_k: Option<f64>,
            expand_depth: Option<f64>,
            min_score: Option<f64>,
            payload_filter: Option<NodeJson>,
        ) -> napi::Result<JsGroupedSearchResult> {
            let payload_filter = payload_filter.map(|value| value.0);
            let top_k = parse_positive_usize(env, top_k.unwrap_or(5.0), "topK")?;
            let payload_filter = parse_payload_filter(payload_filter)?;
            let config = crate::database::SearchConfig {
                top_k,
                expand_depth: parse_safe_usize(env, expand_depth.unwrap_or(2.0), "expandDepth")?,
                min_score: min_score.unwrap_or(0.1) as f32,
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

        #[napi]
        pub fn search_batch(
            &self,
            env: Env,
            query_vectors: Vec<Vec<f64>>,
            top_k: Option<f64>,
            parallelism: Option<f64>,
            min_score: Option<f64>,
        ) -> napi::Result<napi::bindgen_prelude::AsyncTask<BatchSearchTask>> {
            let top_k = parse_positive_usize(env, top_k.unwrap_or(5.0), "topK")?;
            let backend = match &self.inner {
                DbBackend::F32(db) => SearchBackendHandle::F32(db.search_handle()),
                DbBackend::F16(db) => SearchBackendHandle::F16(db.search_handle()),
                DbBackend::U64(db) => SearchBackendHandle::U64(db.search_handle()),
            };
            Ok(napi::bindgen_prelude::AsyncTask::new(BatchSearchTask {
                backend,
                queries: query_vectors,
                search_config: crate::database::SearchConfig {
                    top_k,
                    expand_depth: 0,
                    min_score: min_score.unwrap_or(0.5) as f32,
                    enable_advanced_pipeline: false,
                    ..Default::default()
                },
                batch_config: crate::database::BatchSearchConfig {
                    parallelism: parse_safe_usize(env, parallelism.unwrap_or(0.0), "parallelism")?,
                },
            }))
        }

        /// 认知检索引擎：完全参数化暴露的高级功能 (FISTA, DPP, SA-PPR)
        #[napi]
        pub fn search_advanced(
            &self,
            env: Env,
            query_vector: Vec<f64>,
            config: Option<JsSearchConfig>,
        ) -> napi::Result<Vec<JsSearchHit>> {
            let mut cfg = config.unwrap_or(JsSearchConfig {
                top_k: None,
                recall_k: None,
                rerank_k: None,
                expand_depth: None,
                min_score: None,
                teleport_alpha: None,
                enable_advanced_pipeline: None,
                enable_sparse_residual: None,
                fista_lambda: None,
                fista_threshold: None,
                enable_dpp: None,
                dpp_quality_weight: None,
                enable_refractory_fatigue: None,
                custom_query_text: None,
                payload_filter: None,
                enable_text_hybrid_search: None,
                text_boost: None,
                force_brute_force: None,
                diffusion_bias: None,
                expand_labels: None,
                max_edges_per_node: None,
                min_edge_weight: None,
                edge_direction: None,
            });

            let top_k = parse_positive_usize(env, cfg.top_k.unwrap_or(5.0), "topK")?;
            let payload_filter =
                parse_payload_filter(cfg.payload_filter.take().map(|value| value.0))?;
            let core_config = crate::database::SearchConfig {
                top_k,
                recall_k: parse_safe_usize(env, cfg.recall_k.unwrap_or(0.0), "recallK")?,
                rerank_k: parse_safe_usize(env, cfg.rerank_k.unwrap_or(0.0), "rerankK")?,
                expand_depth: parse_safe_usize(
                    env,
                    cfg.expand_depth.unwrap_or(2.0),
                    "expandDepth",
                )?,
                min_score: cfg.min_score.unwrap_or(0.1) as f32,
                teleport_alpha: cfg.teleport_alpha.unwrap_or(0.0) as f32,
                enable_advanced_pipeline: cfg.enable_advanced_pipeline.unwrap_or(true),
                enable_sparse_residual: cfg.enable_sparse_residual.unwrap_or(false),
                fista_lambda: cfg.fista_lambda.unwrap_or(0.1) as f32,
                fista_threshold: cfg.fista_threshold.unwrap_or(0.3) as f32,
                enable_dpp: cfg.enable_dpp.unwrap_or(false),
                dpp_quality_weight: cfg.dpp_quality_weight.unwrap_or(1.0) as f32,
                enable_refractory_fatigue: cfg.enable_refractory_fatigue.unwrap_or(false),
                enable_text_hybrid_search: cfg.enable_text_hybrid_search.unwrap_or(false),
                text_boost: cfg.text_boost.unwrap_or(1.5) as f32,
                force_brute_force: cfg.force_brute_force.unwrap_or(false),
                diffusion_bias: cfg
                    .diffusion_bias
                    .map(|v| v.iter().map(|&x| x as f32).collect()),
                expand_labels: cfg.expand_labels,
                max_edges_per_node: cfg
                    .max_edges_per_node
                    .map(|value| parse_safe_usize(env, value, "maxEdgesPerNode"))
                    .transpose()?
                    .unwrap_or(0),
                min_edge_weight: cfg.min_edge_weight.unwrap_or(0.0) as f32,
                edge_direction: parse_edge_direction(cfg.edge_direction.as_deref())?,
                payload_filter,
                ..Default::default()
            };

            let q_text = cfg.custom_query_text.as_deref();

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
            query_vector: Vec<f64>,
            query_text: String,
            top_k: Option<f64>,
            expand_depth: Option<f64>,
            min_score: Option<f64>,
            hybrid_alpha: Option<f64>,
            payload_filter: Option<NodeJson>,
        ) -> napi::Result<Vec<JsSearchHit>> {
            let payload_filter = payload_filter.map(|value| value.0);
            let top_k = parse_positive_usize(env, top_k.unwrap_or(5.0), "topK")?;

            let expand_depth = parse_safe_usize(env, expand_depth.unwrap_or(2.0), "expandDepth")?;
            let min_score = min_score.unwrap_or(0.1) as f32;
            let alpha = hybrid_alpha.unwrap_or(0.7) as f32;
            let payload_filter = parse_payload_filter(payload_filter)?;
            // 简单的启发式权重换算
            let boost = (1.0 - alpha).max(0.1) * 3.0;

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

        // ── 文本索引 ──

        /// 对节点建立用于双路召回的长文本 BM25 索引
        #[napi]
        pub fn index_text(&mut self, env: Env, id: f64, text: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.index_text(parse_node_id(env, id, "id")?, &text))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 对节点建立用于精确命中的 AC自动机 高级关键词索引
        #[napi]
        pub fn index_keyword(&mut self, env: Env, id: f64, keyword: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.index_keyword(parse_node_id(env, id, "id")?, &keyword))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 预热关键词自动机；损坏或缺失的延期索引会返回错误。
        #[napi]
        pub fn build_text_index(&mut self, env: Env) -> napi::Result<()> {
            dispatch!(self, mut db => db.build_text_index())
                .map_err(|error| to_napi_error(env, error))
        }

        /// 从完整正文与关键词集合重建文本索引，通过 flush 或 close 持久化。
        #[napi]
        pub fn rebuild_text_index(
            &mut self,
            env: Env,
            documents: Vec<JsTextIndexDocument>,
        ) -> napi::Result<()> {
            let documents = documents
                .into_iter()
                .map(|document| {
                    Ok(crate::index::text::TextIndexDocument {
                        id: parse_safe_usize(env, document.id, "文本重建节点 ID")? as u64,
                        text: document.text,
                        keywords: document.keywords.unwrap_or_default(),
                    })
                })
                .collect::<napi::Result<Vec<_>>>()?;
            dispatch!(self, mut db => db.rebuild_text_index(&documents))
                .map_err(|error| to_napi_error(env, error))
        }

        // ── 属性二级索引 ──

        /// 创建属性索引：对指定 payload 字段建立倒排索引
        ///
        /// ```js
        /// db.createIndex('name')   // 之后 tql('FIND {name: "Alice"} RETURN *') 使用 O(1) 索引
        /// ```
        #[napi]
        pub fn indexed_lookup(
            &self,
            env: Env,
            equalities: NodeJson,
            max_results: Option<f64>,
        ) -> napi::Result<Vec<f64>> {
            let equalities = equalities.0;
            let object = equalities.as_object().ok_or_else(|| {
                napi::Error::from_reason("equalities 必须是对象 (equalities must be an object)")
            })?;
            let equalities = object
                .iter()
                .map(|(field, value)| (field.clone(), value.clone()))
                .collect::<Vec<_>>();
            dispatch!(self, db => db.indexed_lookup(&equalities, parse_safe_usize(env, max_results.unwrap_or(10_000.0), "maxResults")?))
                .map_err(|error| to_napi_error(env, error)).and_then(|value| node_ids_to_js(env, value))
        }

        #[napi]
        pub fn create_ngram_index(&mut self, env: Env, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.create_ngram_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn drop_ngram_index(&mut self, env: Env, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.drop_ngram_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn substring_lookup(
            &self,
            env: Env,
            field: String,
            needle: String,
            max_results: Option<f64>,
        ) -> napi::Result<Vec<f64>> {
            dispatch!(self, db => db.substring_lookup(&field, &needle, parse_safe_usize(env, max_results.unwrap_or(10_000.0), "maxResults")?))
                .map_err(|error| to_napi_error(env, error)).and_then(|value| node_ids_to_js(env, value))
        }

        #[napi]
        pub fn create_unique_index(&mut self, env: Env, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.create_unique_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn create_unique_composite_index(
            &mut self,
            env: Env,
            fields: Vec<String>,
        ) -> napi::Result<()> {
            dispatch!(self, mut db => db.create_unique_composite_index(&fields))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn list_unique_indexes(&self, env: Env) -> napi::Result<Vec<Vec<String>>> {
            Ok(
                dispatch!(self, db => db.list_unique_indexes().map_err(|error| to_napi_error(env, error))?),
            )
        }

        #[napi]
        pub fn create_index(&mut self, env: Env, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.create_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn create_ordered_index(&mut self, env: Env, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.create_ordered_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn create_composite_index(
            &mut self,
            env: Env,
            fields: Vec<String>,
        ) -> napi::Result<()> {
            dispatch!(self, mut db => db.create_composite_index(&fields))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn create_bitmap_index(&mut self, env: Env, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.create_bitmap_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 删除属性索引（查询仍可用，退化为全扫描）
        #[napi]
        pub fn drop_index(&mut self, env: Env, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.drop_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn drop_ordered_index(&mut self, env: Env, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.drop_ordered_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn drop_composite_index(&mut self, env: Env, fields: Vec<String>) -> napi::Result<()> {
            dispatch!(self, mut db => db.drop_composite_index(&fields))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn drop_bitmap_index(&mut self, env: Env, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.drop_bitmap_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn index_info(&self, env: Env) -> napi::Result<NodeJson> {
            dispatch!(self, db => node_json_value(db.index_info().map_err(|error| to_napi_error(env, error))?))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        #[napi]
        pub fn storage_info(&self) -> napi::Result<NodeJson> {
            dispatch!(self, db => node_json_value(db.storage_info()))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        // ── 轻量级单字段查询 ──

        /// 获取节点的 payload（不含向量，比 get() 更轻量）
        #[napi]
        pub fn get_payload(&self, env: Env, id: f64) -> napi::Result<Option<NodeJson>> {
            Ok(
                dispatch!(self, db => db.get_payload(parse_node_id(env, id, "id")?).map_err(|error| to_napi_error(env, error))?).map(NodeJson),
            )
        }

        /// 获取节点的出边列表
        #[napi]
        pub fn get_edges(&self, env: Env, id: f64) -> napi::Result<Vec<JsEdge>> {
            Ok({
                dispatch!(self, db => db.get_edges(parse_node_id(env, id, "id")?).map_err(|error| to_napi_error(env, error))?)
                    .into_iter()
                    .map(|e| Ok(JsEdge {
                        target_id: node_id_to_js(env, e.target_id)?,
                        label: e.label,
                        weight: round_api_f32(e.weight),
                        metadata: e.metadata.into(),
                    })).collect::<napi::Result<_>>()?
            })
        }

        /// 获取节点完整入边，可按标签过滤。
        #[napi]
        pub fn get_incoming_edges(
            &self,
            env: Env,
            id: f64,
            label: Option<String>,
        ) -> napi::Result<Vec<JsIncomingEdge>> {
            Ok({
                dispatch!(self, db => db.get_incoming_edges(parse_node_id(env, id, "id")?, label.as_deref()).map_err(|error| to_napi_error(env, error))?)
                .into_iter()
                .map(|edge| Ok(JsIncomingEdge {
                    source_id: node_id_to_js(env, edge.source_id)?,
                    target_id: node_id_to_js(env, edge.target_id)?,
                    label: edge.label,
                    weight: round_api_f32(edge.weight),
                    metadata: edge.metadata.into(),
                })).collect::<napi::Result<_>>()?
            })
        }

        // ── TQL 统一查询 ──

        /// 执行 TQL (Trivium Query Language) 统一查询
        ///
        /// 支持三种入口：MATCH (图遍历) / FIND (文档过滤) / SEARCH (向量检索)
        ///
        /// ```js
        /// // 图遍历
        /// const rows = db.tql('MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b')
        /// // 文档过滤
        /// const rows = db.tql('FIND {type: "event", heat: {$gte: 0.7}} RETURN *')
        /// ```
        #[napi]
        pub fn tql(&self, env: Env, query: String) -> napi::Result<Vec<NodeJson>> {
            match &self.inner {
                DbBackend::F32(db) => db
                    .tql_values(&query)
                    .map_err(|error| to_napi_error(env, error))
                    .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
                DbBackend::F16(db) => db
                    .tql_values(&query)
                    .map_err(|error| to_napi_error(env, error))
                    .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
                DbBackend::U64(db) => db
                    .tql_values(&query)
                    .map_err(|error| to_napi_error(env, error))
                    .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
            }
        }

        #[napi]
        pub fn prepare_tql(&self, env: Env, query: String) -> napi::Result<JsPreparedTql> {
            dispatch!(self, db => db.prepare_tql(&query))
                .map(|inner| JsPreparedTql { inner })
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn execute_prepared_tql(
            &self,
            env: Env,
            prepared: &JsPreparedTql,
            parameters: NodeJson,
        ) -> napi::Result<Vec<NodeJson>> {
            let parameters = parameters.0;
            let object = parameters.as_object().ok_or_else(|| {
                napi::Error::from_reason(
                    "Prepared TQL 参数必须是对象 (Prepared TQL parameters must be an object)",
                )
            })?;
            let mut values = std::collections::HashMap::new();
            for (name, value) in object {
                values.insert(
                    name.clone(),
                    crate::query::tql_prepared::TqlParamValue::from_json(value)
                        .map_err(|error| to_napi_error(env, error))?,
                );
            }
            match &self.inner {
                DbBackend::F32(db) => db
                    .execute_prepared_tql(&prepared.inner, &values)
                    .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
                DbBackend::F16(db) => db
                    .execute_prepared_tql(&prepared.inner, &values)
                    .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
                DbBackend::U64(db) => db
                    .execute_prepared_tql(&prepared.inner, &values)
                    .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
            }
            .map_err(|error| to_napi_error(env, error))
        }

        /// 执行 TQL 写操作（CREATE / SET / DELETE / DETACH DELETE）
        ///
        /// 返回 { affected: number, createdIds: number[] }
        ///
        /// ```js
        /// const result = db.tqlMut('CREATE (a {name: "Alice", age: 30})')
        /// console.log(result.affected)     // 1
        /// console.log(result.createdIds)   // [1]
        ///
        /// db.tqlMut('MATCH (a {name: "Alice"}) SET a.age == 31')
        /// db.tqlMut('MATCH (a {name: "Alice"}) DELETE a')
        /// ```
        #[napi]
        pub fn tql_mut(&mut self, env: Env, query: String) -> napi::Result<NodeJson> {
            let result =
                dispatch!(self, mut db => db.tql_mut_with_id_limit(&query,9_007_199_254_740_991))
                    .map_err(|error| to_napi_error(env, error))?;
            Ok(serde_json::json!({
                "affected": result.affected,
                "createdIds": result.created_ids,
            })
            .into())
        }

        // ── 持久化与管理 ──

        /// 手动落盘
        #[napi]
        pub fn flush(&mut self, env: Env) -> napi::Result<()> {
            dispatch!(self, mut db => db.flush()).map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn publish_generation_manifest(
            &mut self,
            env: Env,
            generation_id: String,
        ) -> napi::Result<NodeJson> {
            let manifest =
                dispatch!(self, mut db => db.publish_generation_manifest(&generation_id))
                    .map_err(|error| to_napi_error(env, error))?;
            node_json_value(manifest).map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        /// 运行时切换 WAL 同步模式
        #[napi]
        pub fn set_sync_mode(&mut self, env: Env, mode: String) -> napi::Result<()> {
            let sm = parse_sync_mode(&mode)?;
            dispatch!(self, mut db => db.set_sync_mode(sm))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 启动后台自动压缩（每 interval_secs 秒落盘一次，默认 2 小时=7200秒）
        #[napi]
        pub fn enable_auto_compaction(
            &mut self,
            env: Env,
            interval_secs: Option<f64>,
        ) -> napi::Result<()> {
            let secs =
                parse_safe_usize(env, interval_secs.unwrap_or(7200.0), "intervalSecs")? as u64;
            dispatch!(self, mut db => db.enable_auto_compaction(std::time::Duration::from_secs(secs)))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn set_auto_build_quiver(&mut self, env: Env, enabled: bool) -> napi::Result<()> {
            dispatch!(self, mut db => db.set_auto_build_quiver(enabled))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 停止后台自动压缩
        #[napi]
        pub fn clear_search_state(&self, env: Env) -> napi::Result<()> {
            dispatch!(self, db => db.clear_search_state())
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn disable_auto_compaction(&mut self) {
            dispatch!(self, mut db => db.disable_auto_compaction());
        }

        /// 手动触发全量压实（阻塞当前线程）
        #[napi]
        pub fn compact(&mut self, env: Env) -> napi::Result<()> {
            dispatch!(self, mut db => db.compact()).map_err(|error| to_napi_error(env, error))
        }

        /// 为后续插入主动预留额外节点容量。
        #[napi]
        pub fn reserve_nodes(&self, env: Env, additional: f64) -> napi::Result<()> {
            let additional = parse_safe_usize(env, additional, "additional")?;
            dispatch!(self, db => db.reserve_nodes(additional))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 设置内存上限（MiB），0 = 无限制。
        #[napi]
        pub fn set_memory_limit(&mut self, env: Env, mb: f64) -> napi::Result<()> {
            let mb = parse_safe_usize(env, mb, "memoryLimitMb")?;
            let bytes = mb
                .checked_mul(1024 * 1024)
                .ok_or_else(|| napi::Error::from_reason("memoryLimitMb 换算字节时溢出"))?;
            dispatch!(self, mut db => db.set_memory_limit(bytes))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 读取多个数据库共享的配额、工作线程和任务统计。
        #[napi]
        pub fn resource_stats(&self) -> napi::Result<NodeJson> {
            node_json_value(dispatch!(self, db => db.resource_stats()))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        #[napi]
        pub fn instance_stats(&self) -> napi::Result<NodeJson> {
            node_json_value(dispatch!(self, db => db.instance_stats()))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        /// 估算当前内存占用（字节）
        #[napi]
        pub fn estimated_memory(&self) -> f64 {
            dispatch!(self, db => db.estimated_memory()) as f64
        }

        /// 获取向量维度
        #[napi]
        pub fn dim(&self) -> u32 {
            dispatch!(self, db => db.dim()) as u32
        }

        /// 获取节点总数
        #[napi]
        pub fn node_count(&self, env: Env) -> napi::Result<u32> {
            Ok({
                dispatch!(self, db => db.node_count().map_err(|error| to_napi_error(env, error))?)
                    as u32
            })
        }

        /// 获取所有活跃节点 ID
        #[napi]
        pub fn all_node_ids(&self, env: Env) -> napi::Result<Vec<f64>> {
            Ok({
                dispatch!(self, db => db.all_node_ids().map_err(|error| to_napi_error(env, error))?)
                    .into_iter()
                    .map(|value| node_id_to_js(env, value))
                    .collect::<napi::Result<_>>()?
            })
        }

        /// 维度迁移：结构复制到新维度数据库，返回需要更新向量的节点 ID 列表
        #[napi]
        pub fn migrate(&self, env: Env, new_path: String, new_dim: f64) -> napi::Result<Vec<f64>> {
            let new_dim = parse_positive_usize(env, new_dim, "newDim")?;
            self.all_node_ids(env)?;
            match &self.inner {
                DbBackend::F32(db) => {
                    let (_, ids) = db
                        .migrate_to(&new_path, new_dim)
                        .map_err(|error| to_napi_error(env, error))?;
                    ids.into_iter()
                        .map(|value| node_id_to_js(env, value))
                        .collect::<napi::Result<_>>()
                }
                DbBackend::F16(db) => {
                    let (_, ids) = db
                        .migrate_to(&new_path, new_dim)
                        .map_err(|error| to_napi_error(env, error))?;
                    ids.into_iter()
                        .map(|value| node_id_to_js(env, value))
                        .collect::<napi::Result<_>>()
                }
                DbBackend::U64(db) => {
                    let (_, ids) = db
                        .migrate_to(&new_path, new_dim)
                        .map_err(|error| to_napi_error(env, error))?;
                    ids.into_iter()
                        .map(|value| node_id_to_js(env, value))
                        .collect::<napi::Result<_>>()
                }
            }
        }

        /// 获取 dtype 字符串（"f32" / "f16" / "u64"）
        #[napi(getter)]
        pub fn dtype(&self) -> String {
            self.dtype.clone()
        }

        /// 检查节点是否存在
        #[napi]
        pub fn contains(&self, env: Env, id: f64) -> napi::Result<bool> {
            Ok(
                dispatch!(self, db => db.contains(parse_node_id(env, id, "id")?).map_err(|error| to_napi_error(env, error))?),
            )
        }

        #[napi]
        pub fn request_shutdown(
            &mut self,
            env: Env,
            this: This<JsObject>,
        ) -> napi::Result<NodeJson> {
            let status = dispatch!(self, mut db => db.request_shutdown())
                .map_err(|e| to_napi_error(env, e))?;
            super::super::node_hook_refs::flush(env, &this)?;
            node_json_value(status).map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        #[napi]
        pub fn shutdown_status(&self, env: Env, this: This<JsObject>) -> napi::Result<NodeJson> {
            super::super::node_hook_refs::flush(env, &this)?;
            node_json_value(dispatch!(self, db => db.shutdown_status()))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        #[napi]
        pub fn close_timeout(
            &mut self,
            env: Env,
            this: This<JsObject>,
            timeout_ms: f64,
        ) -> napi::Result<bool> {
            let milliseconds = parse_safe_usize(env, timeout_ms, "timeoutMs")?;
            let closed = dispatch!(self, mut db => db.close_timeout(std::time::Duration::from_millis(milliseconds as u64)))
                .map_err(|e| to_napi_error(env, e))?;
            super::super::node_hook_refs::flush(env, &this)?;
            Ok(closed)
        }

        #[napi]
        pub fn create_snapshot(
            &mut self,
            env: Env,
            destination: String,
            generation_id: String,
        ) -> napi::Result<NodeJson> {
            let manifest =
                dispatch!(self, mut db => db.create_snapshot(&destination, &generation_id))
                    .map_err(|e| to_napi_error(env, e))?;
            node_json_value(manifest).map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 显式关闭数据库（落盘后释放资源）
        #[napi]
        pub fn close(&mut self, env: Env, this: This<JsObject>) -> napi::Result<()> {
            let result = match dispatch!(self, mut db => db.close()) {
                Ok(()) | Err(crate::error::TriviumError::DatabaseClosed) => Ok(()),
                Err(error) => Err(to_napi_error(env, error)),
            };
            super::super::node_hook_refs::flush(env, &this)?;
            result
        }
    } // impl TriviumDB
} // mod nodejs
