//! Node.js napi-rs 公共绑定。
//!
//! 该层只负责 JS 值校验、camelCase API、异步任务和稳定错误码映射，核心语义必须委托
//! Rust Database/TQL 实现。u64 NodeId 在一等查询值中使用字符串，避免超过 JS 安全整数。

#[cfg(feature = "nodejs")]
#[allow(clippy::module_inception)]
pub mod nodejs {
    use crate::bindings::node_json::NodeJson;
    use crate::database::Database as GenericDatabase;
    use napi::bindgen_prelude::{Either, Function, This};
    use napi::{Env, JsObject, JsUnknown, ValueType};
    use napi_derive::napi;

    #[napi(js_name = "__callArgumentInfo")]
    pub fn call_argument_info(
        env: Env,
        owner: String,
        operation: String,
        field: String,
        kind: String,
    ) -> napi::Result<NodeJson> {
        let info = crate::sdk::call_argument_info(&owner, &operation, &field, &kind)
            .map_err(|error| to_napi_error(env, error))?;
        node_json_value(info).map_err(|error| napi::Error::from_reason(error.to_string()))
    }

    fn node_json_value(value: impl serde::Serialize) -> serde_json::Result<NodeJson> {
        serde_json::to_value(value).map(NodeJson)
    }

    #[napi(js_name = "RequestContext")]
    pub struct JsRequestContext {
        inner: crate::sdk::RequestContext,
    }

    #[napi]
    impl JsRequestContext {
        #[napi(constructor)]
        pub fn new(
            env: Env,
            request_id: JsUnknown,
            timeout_ms: Option<Either<f64, JsUnknown>>,
        ) -> napi::Result<Self> {
            let request_id = read_string(env, request_id, "requestId")?;
            let timeout_ms = parse_search_usize(env, timeout_ms, 0.0, "timeoutMs", false)?;
            crate::sdk::RequestContext::with_timeout(
                &request_id,
                std::time::Duration::from_millis(timeout_ms as u64),
                false,
            )
            .map(|inner| Self { inner })
            .map_err(|error| to_napi_error(env, error))
        }

        #[napi(getter)]
        pub fn request_id(&self) -> String {
            self.inner.request_id().to_owned()
        }
        #[napi]
        pub fn cancel(&self) {
            self.inner.cancel();
        }
        #[napi]
        pub fn check(&self, env: Env) -> napi::Result<()> {
            self.inner
                .control()
                .check()
                .map_err(|error| to_napi_error(env, error))
        }
    }

    #[napi(js_name = "__invokeRequest")]
    pub fn invoke_request(
        env: Env,
        context: &JsRequestContext,
        owner: String,
        operation: String,
        callback: Function<(), JsUnknown>,
    ) -> napi::Result<JsUnknown> {
        let admission = |error| {
            to_napi_error(
                env,
                crate::sdk::request_admission(error, &owner, &operation),
            )
        };
        let _scope = context.inner.enter().map_err(admission)?;
        if operation != "console_request" {
            crate::request::check_current().map_err(admission)?;
        }
        callback.call(())
    }

    #[napi(js_name = "__requestOperation")]
    pub fn request_operation(owner: String, operation: String) -> bool {
        crate::sdk::request_operation(&owner, &operation)
    }

    /// 会话只保存策略与取消状态，数据库仍由宿主持有。
    #[napi(js_name = "ConsoleSession")]
    pub struct JsConsoleSession {
        inner: crate::ConsoleSession,
    }

    #[napi]
    impl JsConsoleSession {
        #[napi(constructor)]
        pub fn new(env: Env, options_json: Option<JsUnknown>) -> napi::Result<Self> {
            let options_json = constructor_string(env, options_json, "{}", "optionsJson")?;
            crate::ConsoleSession::from_json(&options_json)
                .map(|inner| Self { inner })
                .map_err(|error| to_napi_error(env, error.at_field("optionsJson")))
        }

        #[napi]
        pub fn detach(&self) {
            self.inner.detach();
        }
        #[napi]
        pub fn is_detached(&self) -> bool {
            self.inner.is_detached()
        }
        #[napi]
        pub fn is_drained(&self) -> bool {
            self.inner.is_drained()
        }
        #[napi]
        pub fn cancel(&self, env: Env, request_id: JsUnknown) -> napi::Result<bool> {
            let request_id = read_string(env, request_id, "requestId")?;
            Ok(self.inner.cancel(&request_id))
        }
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

    fn parse_runtime(
        env: Env,
        runtime: Option<JsUnknown>,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> napi::Result<crate::resources::ResourceRuntime> {
        let invalid = || {
            let error = crate::TriviumError::InvalidInput(
                "runtime 必须是有效的 ResourceRuntime 句柄 (runtime must contain a valid ResourceRuntime handle)".into(),
            ).at_field("runtime");
            to_napi_error(
                env,
                match write {
                    Some(write) => error.with_write_info(write),
                    None => error,
                },
            )
        };
        Ok(if let Some(value) = runtime {
            if value.get_type()? != ValueType::Object {
                return Err(invalid());
            }
            let object = JsObject::try_from(value)?;
            if !object.has_own_property("__tdbResourceRuntime")? {
                return Err(invalid());
            }
            let handle: napi::bindgen_prelude::External<crate::resources::ResourceRuntime> = object
                .get_named_property("__tdbResourceRuntime")
                .map_err(|error| {
                    // 已知句柄类型拒绝附加字段，getter 或 Proxy 的宿主异常原样传播。
                    if error.status == napi::Status::InvalidArg {
                        invalid()
                    } else {
                        error
                    }
                })?;
            (*handle).clone()
        } else {
            crate::resources::ResourceRuntime::shared_default()
        })
    }

    pub(crate) fn to_napi_error(env: Env, error: crate::error::TriviumError) -> napi::Error {
        let mut info = error.info();
        if let Some(
            crate::error::ErrorTarget::Field { field }
            | crate::error::ErrorTarget::FieldReference { field, .. },
        ) = &mut info.target
        {
            *field = match field.as_str() {
                "min_score" => "minScore",
                "request_id" => "requestId",
                "timeout_ms" => "timeoutMs",
                "teleport_alpha" => "teleportAlpha",
                "fista_lambda" => "fistaLambda",
                "fista_threshold" => "fistaThreshold",
                "dpp_quality_weight" => "dppQualityWeight",
                "text_boost" => "textBoost",
                "min_edge_weight" => "minEdgeWeight",
                "diffusion_bias" => "diffusionBias",
                "hybrid_alpha" => "hybridAlpha",
                "max_results" => "maxResults",
                _ => field.as_str(),
            }
            .to_owned();
        }
        let details = crate::sdk::json_error_details(info.details);
        let result = env
            .create_error(napi::Error::from_reason(info.message))
            .and_then(|mut object| {
                object.set_named_property("schema_version", info.schema_version)?;
                object.set_named_property("code", info.code)?;
                object.set_named_property("details", details.clone())?;
                if let Some(stage) = info.stage {
                    object.set_named_property("stage", stage.as_str())?;
                }
                if let Some(target) = info.target {
                    let target = serde_json::to_value(target)
                        .map_err(|error| napi::Error::from_reason(error.to_string()))?;
                    object.set_named_property("target", target)?;
                }
                if let Some(write) = info.write {
                    let write = serde_json::to_value(write)
                        .map_err(|error| napi::Error::from_reason(error.to_string()))?;
                    object.set_named_property("write", write)?;
                }
                if let Some(fields) = details.as_object() {
                    for (name, value) in fields {
                        if matches!(
                            name.as_str(),
                            "schema_version"
                                | "code"
                                | "message"
                                | "details"
                                | "stage"
                                | "target"
                                | "write"
                        ) {
                            continue;
                        }
                        object.set_named_property(name, value.clone())?;
                    }
                }
                Ok(napi::Error::from(object.into_unknown()))
            });
        match result {
            Ok(error) | Err(error) => error,
        }
    }

    fn with_napi_write_info(
        env: Env,
        error: napi::Error,
        write: crate::error::WriteErrorInfo,
    ) -> napi::Error {
        let result = napi::JsError::from(error)
            .into_unknown(env)
            .coerce_to_object()
            .and_then(|mut object| {
                let write = serde_json::to_value(write)
                    .map_err(|error| napi::Error::from_reason(error.to_string()))?;
                object.set_named_property("write", write)?;
                Ok(napi::Error::from(object.into_unknown()))
            });
        match result {
            Ok(error) | Err(error) => error,
        }
    }

    #[napi]
    pub fn verify_snapshot(
        env: Env,
        directory: JsUnknown,
        runtime: Option<JsUnknown>,
    ) -> napi::Result<NodeJson> {
        let directory = read_string(env, directory, "directory")?;
        let runtime = parse_runtime(env, runtime, None)?;
        let manifest = crate::storage::snapshot::verify_snapshot_with_runtime(&directory, &runtime)
            .map_err(|e| to_napi_error(env, e))?;
        node_json_value(manifest).map_err(|e| napi::Error::from_reason(e.to_string()))
    }

    #[napi]
    pub fn restore_snapshot(
        env: Env,
        source: JsUnknown,
        destination: JsUnknown,
        runtime: Option<JsUnknown>,
    ) -> napi::Result<NodeJson> {
        let source = write_string(env, source, "source")?;
        let destination = write_string(env, destination, "destination")?;
        let runtime = parse_runtime(
            env,
            runtime,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )?;
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

    /// 多个数据库共享的运行时资源配置，字节统计不等于进程 RSS。
    #[napi(object)]
    pub struct JsResourceLimits {
        pub maintenance_memory_limit_mb: Option<Either<f64, JsUnknown>>,
        pub memory_limit_mb: Option<Either<f64, JsUnknown>>,
        pub max_threads: Option<Either<f64, JsUnknown>>,
        pub max_active_tasks: Option<Either<f64, JsUnknown>>,
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
            options: Option<JsUnknown>,
        ) -> napi::Result<Self> {
            let object = constructor_options(env, options)?;
            let options = JsResourceLimits {
                maintenance_memory_limit_mb: object.get("maintenanceMemoryLimitMb")?,
                memory_limit_mb: object.get("memoryLimitMb")?,
                max_threads: object.get("maxThreads")?,
                max_active_tasks: object.get("maxActiveTasks")?,
            };
            let memory_limit = constructor_mib(
                env,
                parse_search_usize(env, options.memory_limit_mb, 0.0, "memoryLimitMb", false)?,
                "memoryLimitMb",
                9_007_199_254_740_991,
            )?;
            let inner = crate::resources::ResourceRuntime::new(crate::resources::ResourceLimits {
                memory_limit,
                maintenance_memory_limit: constructor_mib(
                    env,
                    parse_search_usize(
                        env,
                        options.maintenance_memory_limit_mb,
                        0.0,
                        "maintenanceMemoryLimitMb",
                        false,
                    )?,
                    "maintenanceMemoryLimitMb",
                    9_007_199_254_740_991,
                )?,
                max_threads: parse_search_usize(
                    env,
                    options.max_threads,
                    0.0,
                    "maxThreads",
                    false,
                )?,
                max_active_tasks: parse_search_usize(
                    env,
                    options.max_active_tasks,
                    0.0,
                    "maxActiveTasks",
                    false,
                )?,
            })
            .map_err(|error| {
                let error = match error.info().target {
                    Some(crate::error::ErrorTarget::Field { field }) if field == "max_threads" => {
                        error.at_field("maxThreads")
                    }
                    Some(crate::error::ErrorTarget::Field { field })
                        if field == "maintenance_memory_limit" =>
                    {
                        error.at_field("maintenanceMemoryLimitMb")
                    }
                    _ => error,
                };
                to_napi_error(env, error)
            })?;
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

    /// 数据库打开与容量规划配置。
    #[napi(object)]
    pub struct JsDatabaseOptions {
        pub runtime: Option<JsUnknown>,
        pub dim: Option<Either<f64, JsUnknown>>,
        pub dtype: Option<JsUnknown>,
        pub sync_mode: Option<JsUnknown>,
        pub storage_mode: Option<JsUnknown>,
        pub auto_build_quiver: Option<Either<bool, JsUnknown>>,
        pub load_text_index: Option<Either<bool, JsUnknown>>,
        pub expected_nodes: Option<Either<f64, JsUnknown>>,
        pub memory_limit_mb: Option<Either<f64, JsUnknown>>,
        pub payload_cache_mb: Option<Either<f64, JsUnknown>>,
        pub payload_cache_entry_mb: Option<Either<f64, JsUnknown>>,
        pub access_mode: Option<JsUnknown>,
        pub missing_index_policy: Option<JsUnknown>,
        pub max_query_rows: Option<Either<f64, JsUnknown>>,
        pub row_overflow: Option<JsUnknown>,
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
        validate_safe_usize(value, name).map_err(|error| to_napi_error(env, error))
    }

    fn validate_safe_usize(value: f64, name: &str) -> crate::Result<usize> {
        if !value.is_finite()
            || value < 0.0
            || value.fract() != 0.0
            || value > 9_007_199_254_740_991.0
            || value > usize::MAX as f64
        {
            return Err(crate::TriviumError::InvalidInput(format!(
                    "{name} 必须是 JavaScript 安全范围内的非负整数 ({name} must be a non-negative JavaScript safe integer)"
                )).at_field(name));
        }
        Ok(value as usize)
    }

    fn parse_management_usize(
        env: Env,
        value: Either<f64, JsUnknown>,
        field: &str,
    ) -> napi::Result<usize> {
        parse_usize_argument(
            env,
            value,
            field,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )
    }

    fn parse_usize_argument(
        env: Env,
        value: Either<f64, JsUnknown>,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> napi::Result<usize> {
        let parsed = match value {
            Either::A(value) => validate_safe_usize(value, field),
            Either::B(_) => Err(crate::TriviumError::InvalidInput(format!(
                "参数必须是数值 (Argument must be a number): {field}"
            ))
            .at_field(field)),
        };
        parsed.map_err(|error| {
            to_napi_error(
                env,
                match write {
                    Some(write) => error.with_write_info(write),
                    None => error,
                },
            )
        })
    }

    fn parse_management_string(
        env: Env,
        value: Either<String, JsUnknown>,
        field: &str,
    ) -> napi::Result<String> {
        parse_string_argument(
            env,
            value,
            field,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )
    }

    fn parse_string_argument(
        env: Env,
        value: Either<String, JsUnknown>,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> napi::Result<String> {
        match value {
            Either::A(value) => Ok(value),
            Either::B(_) => {
                let error = crate::TriviumError::InvalidInput(format!(
                    "参数必须是字符串 (Argument must be a string): {field}"
                ))
                .at_field(field);
                Err(to_napi_error(
                    env,
                    match write {
                        Some(write) => error.with_write_info(write),
                        None => error,
                    },
                ))
            }
        }
    }

    fn invalid_constructor_field(env: Env, field: &str, reason: &str) -> napi::Error {
        to_napi_error(
            env,
            crate::TriviumError::InvalidInput(reason.into()).at_field(field),
        )
    }

    fn write_input_error(env: Env, field: &str, reason: &str) -> napi::Error {
        argument_input_error(
            env,
            field,
            reason,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )
    }

    fn argument_input_error(
        env: Env,
        field: &str,
        reason: &str,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> napi::Error {
        let input = crate::TriviumError::InvalidInput(reason.into()).at_field(field);
        to_napi_error(
            env,
            match write {
                Some(write) => input.with_write_info(write),
                None => input,
            },
        )
    }

    fn write_number(env: Env, value: JsUnknown, field: &str) -> napi::Result<f64> {
        number_value(
            env,
            value,
            field,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )
    }

    fn read_number(env: Env, value: JsUnknown, field: &str) -> napi::Result<f64> {
        number_value(env, value, field, None)
    }

    fn number_value(
        env: Env,
        value: JsUnknown,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> napi::Result<f64> {
        if value.get_type()? != ValueType::Number {
            return Err(argument_input_error(
                env,
                field,
                "参数必须是数值 (Argument must be a number)",
                write,
            ));
        }
        napi::JsNumber::try_from(value)?.get_double()
    }

    fn write_id(env: Env, value: JsUnknown, field: &str) -> napi::Result<u64> {
        let value = write_number(env, value, field)?;
        validate_safe_usize(value, field)
            .map(|value| value as u64)
            .map_err(|error| {
                to_napi_error(
                    env,
                    error.with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )
            })
    }

    fn raw_string(
        env: Env,
        value: JsUnknown,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> napi::Result<String> {
        let parsed = if value.get_type()? == ValueType::String {
            crate::bindings::node_json::string_from_js(value)
        } else {
            Err(napi::Error::new(
                napi::Status::InvalidArg,
                "参数必须是字符串 (Argument must be a string)",
            ))
        };
        parsed.map_err(|error| {
            if error.status != napi::Status::InvalidArg {
                return error;
            }
            let input = crate::TriviumError::InvalidInput(error.reason).at_field(field);
            to_napi_error(
                env,
                match write {
                    Some(write) => input.with_write_info(write),
                    None => input,
                },
            )
        })
    }

    fn write_string(env: Env, value: JsUnknown, field: &str) -> napi::Result<String> {
        raw_string(
            env,
            value,
            field,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        )
    }

    fn write_array<T>(
        env: Env,
        value: JsUnknown,
        field: &str,
        parse: impl FnMut(Env, JsUnknown, &str) -> napi::Result<T>,
    ) -> napi::Result<Vec<T>> {
        array_value(
            env,
            value,
            field,
            Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
            parse,
        )
    }

    fn read_array<T>(
        env: Env,
        value: JsUnknown,
        field: &str,
        parse: impl FnMut(Env, JsUnknown, &str) -> napi::Result<T>,
    ) -> napi::Result<Vec<T>> {
        array_value(env, value, field, None, parse)
    }

    fn array_value<T>(
        env: Env,
        value: JsUnknown,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
        mut parse: impl FnMut(Env, JsUnknown, &str) -> napi::Result<T>,
    ) -> napi::Result<Vec<T>> {
        let invalid = || {
            argument_input_error(
                env,
                field,
                "参数必须是数组 (Argument must be an array)",
                write,
            )
        };
        if value.get_type()? != ValueType::Object {
            return Err(invalid());
        }
        let array = JsObject::try_from(value)?;
        if !array.is_array()? {
            return Err(invalid());
        }
        let mut result = Vec::new();
        for index in 0..array.get_array_length()? {
            let path = crate::sdk::field_member(field, &index.to_string());
            let item = parse(env, array.get_element(index)?, &path)?;
            result.try_reserve(1).map_err(|error| {
                let error = crate::TriviumError::CapacityAllocationFailed {
                    reason: format!(
                        "参数缓冲分配失败 (Argument buffer allocation failed): {error}"
                    ),
                }
                .at_field(&path);
                to_napi_error(
                    env,
                    match write {
                        Some(write) => error.with_write_info(write),
                        None => error,
                    },
                )
            })?;
            result.push(item);
        }
        Ok(result)
    }

    fn write_json(env: Env, value: JsUnknown, field: &str) -> napi::Result<serde_json::Value> {
        NodeJson::from_js_at(env, value, field).map(|value| value.0)
    }

    fn read_json(env: Env, value: JsUnknown, field: &str) -> napi::Result<serde_json::Value> {
        NodeJson::from_js_field(env, value, field, None).map(|value| value.0)
    }

    fn read_id(env: Env, value: JsUnknown, field: &str) -> napi::Result<u64> {
        validate_safe_usize(read_number(env, value, field)?, field)
            .map(|value| value as u64)
            .map_err(|error| to_napi_error(env, error))
    }

    fn read_string(env: Env, value: JsUnknown, field: &str) -> napi::Result<String> {
        raw_string(env, value, field, None)
    }

    fn read_vector(env: Env, value: JsUnknown, field: &str, dtype: &str) -> napi::Result<Vec<f64>> {
        read_array(env, value, field, |env, value, path| {
            let number = read_number(env, value, path)?;
            if dtype == "u64" {
                validate_u64_component(number)
                    .map_err(|error| to_napi_error(env, error.at_field(path)))?;
            }
            Ok(number)
        })
    }

    fn argument_object(
        env: Env,
        value: JsUnknown,
        field: &str,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> napi::Result<JsObject> {
        let invalid = || {
            argument_input_error(
                env,
                field,
                "参数必须是非数组对象 (Argument must be a non-array object)",
                write,
            )
        };
        if value.get_type()? != ValueType::Object {
            return Err(invalid());
        }
        let object = JsObject::try_from(value)?;
        if object.is_array()? {
            return Err(invalid());
        }
        Ok(object)
    }

    fn optional_number(
        env: Env,
        value: Option<JsUnknown>,
        default: f64,
        field: &str,
    ) -> napi::Result<f64> {
        value
            .map(|value| read_number(env, value, field))
            .transpose()
            .map(|value| value.unwrap_or(default))
    }

    fn optional_bool(
        env: Env,
        value: Option<JsUnknown>,
        default: bool,
        field: &str,
    ) -> napi::Result<bool> {
        value
            .map(|value| {
                if value.get_type()? != ValueType::Boolean {
                    return Err(argument_input_error(
                        env,
                        field,
                        "参数必须是布尔值 (Argument must be a boolean)",
                        None,
                    ));
                }
                napi::JsBoolean::try_from(value)?.get_value()
            })
            .transpose()
            .map(|value| value.unwrap_or(default))
    }

    fn write_vector(
        env: Env,
        value: JsUnknown,
        field: &str,
        dtype: &str,
    ) -> napi::Result<Vec<f64>> {
        write_array(env, value, field, |env, value, path| {
            let number = write_number(env, value, path)?;
            if dtype == "u64" {
                validate_u64_component(number).map_err(|error| {
                    to_napi_error(
                        env,
                        error
                            .at_field(path)
                            .with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                    )
                })?;
            }
            Ok(number)
        })
    }

    fn constructor_options(env: Env, options: Option<JsUnknown>) -> napi::Result<JsObject> {
        let Some(value) = options else {
            return env.create_object();
        };
        let invalid = || {
            invalid_constructor_field(
                env,
                "options",
                "配置必须是非数组对象 (Options must be a non-array object)",
            )
        };
        if value.get_type()? != ValueType::Object {
            return Err(invalid());
        }
        let object = JsObject::try_from(value)?;
        if object.is_array()? {
            return Err(invalid());
        }
        Ok(object)
    }

    fn constructor_string(
        env: Env,
        value: Option<JsUnknown>,
        default: &str,
        field: &str,
    ) -> napi::Result<String> {
        match value {
            Some(value) => raw_string(env, value, field, None),
            None => Ok(default.into()),
        }
    }

    fn constructor_bool(
        env: Env,
        value: Option<Either<bool, JsUnknown>>,
        default: bool,
        field: &str,
    ) -> napi::Result<bool> {
        match value {
            None => Ok(default),
            Some(Either::A(value)) => Ok(value),
            Some(Either::B(_)) => Err(invalid_constructor_field(
                env,
                field,
                "参数必须是布尔值 (Argument must be a boolean)",
            )),
        }
    }

    fn constructor_mib(
        env: Env,
        value: usize,
        field: &str,
        max_bytes: u128,
    ) -> napi::Result<usize> {
        value.checked_mul(1024 * 1024).filter(|bytes| (*bytes as u128) <= max_bytes).ok_or_else(|| {
            invalid_constructor_field(env, field, "MiB 换算超出字节表示范围 (MiB conversion exceeds the byte representation range)")
        })
    }

    fn parse_search_usize(
        env: Env,
        value: Option<Either<f64, JsUnknown>>,
        default: f64,
        field: &str,
        positive: bool,
    ) -> napi::Result<usize> {
        let value = parse_usize_argument(env, value.unwrap_or(Either::A(default)), field, None)?;
        if positive && value == 0 {
            return Err(to_napi_error(
                env,
                crate::TriviumError::InvalidInput(format!(
                    "{field} 必须大于零 ({field} must be positive)"
                ))
                .at_field(field),
            ));
        }
        Ok(value)
    }

    fn collect_prepared_parameters(
        env: Env,
        parameters: JsUnknown,
        prepared: &crate::query::tql_prepared::PreparedTql,
    ) -> napi::Result<std::collections::HashMap<String, crate::query::tql_prepared::TqlParamValue>>
    {
        let invalid_container = || {
            to_napi_error(
                env,
                crate::TriviumError::InvalidInput(
                    "Prepared TQL 参数必须是对象 (Prepared TQL parameters must be an object)"
                        .into(),
                )
                .at_field("parameters"),
            )
        };
        if parameters.get_type()? != ValueType::Object {
            return Err(invalid_container());
        }
        let object = JsObject::try_from(parameters)?;
        if object.is_array()? {
            return Err(invalid_container());
        }
        let keys = object.get_property_names()?;
        let mut values = std::collections::HashMap::new();
        for index in 0..keys.get_array_length()? {
            let name = keys
                .get_element::<napi::JsString>(index)?
                .into_utf8()?
                .as_str()?
                .to_owned();
            // 枚举与 getter 可执行宿主代码，原异常直接传播。
            let value: JsUnknown = object.get_property(env.create_string(&name)?)?;
            let invalid_value = || {
                to_napi_error(
                    env,
                    prepared.locate_parameter_error(crate::TriviumError::InvalidInput(
                        "参数值无法表示为 Prepared 标量 (Value cannot be represented as a Prepared scalar)".into(),
                    )
                    .at_query_parameter(&name)),
                )
            };
            match value.get_type()? {
                ValueType::Null | ValueType::Boolean | ValueType::Number | ValueType::String => {}
                // 复合值在标量边界拒绝，避免遍历无效参数中的循环或宿主 getter。
                _ => return Err(invalid_value()),
            }
            let json = NodeJson::from_js(env, value).map_err(|error| {
                if error.status == napi::Status::InvalidArg {
                    invalid_value()
                } else {
                    error
                }
            })?;
            let value = prepared
                .parameter_from_json(&name, &json)
                .map_err(|error| to_napi_error(env, error))?;
            values.insert(name, value);
        }
        Ok(values)
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
        options: Option<JsUnknown>,
    ) -> napi::Result<crate::graph::reachability::ReachabilityConfig> {
        let object = match options {
            Some(value) => argument_object(env, value, "options", None)?,
            None => env.create_object()?,
        };
        let direction = object
            .get("direction")?
            .map(|value| read_string(env, value, "direction"))
            .transpose()?;
        let direction = match direction.as_deref().unwrap_or("outgoing") {
            "outgoing" => crate::graph::reachability::ReachabilityDirection::Outgoing,
            "incoming" => crate::graph::reachability::ReachabilityDirection::Incoming,
            "both" => crate::graph::reachability::ReachabilityDirection::Both,
            _ => {
                return Err(argument_input_error(
                    env,
                    "direction",
                    "direction 必须是 outgoing / incoming / both (direction must be outgoing / incoming / both)",
                    None,
                ));
            }
        };
        Ok(crate::graph::reachability::ReachabilityConfig {
            min_depth: parse_search_usize(env, object.get("minDepth")?, 1.0, "minDepth", false)?,
            max_depth: parse_search_usize(env, object.get("maxDepth")?, 1.0, "maxDepth", false)?,
            labels: object
                .get("labels")?
                .map(|value| read_array(env, value, "labels", read_string))
                .transpose()?,
            direction,
            max_visited_nodes: parse_search_usize(
                env,
                object.get("maxVisitedNodes")?,
                10_000.0,
                "maxVisitedNodes",
                false,
            )?,
            max_results: parse_search_usize(
                env,
                object.get("maxResults")?,
                10_000.0,
                "maxResults",
                false,
            )?,
            max_edges: parse_search_usize(
                env,
                object.get("maxEdges")?,
                50_000.0,
                "maxEdges",
                false,
            )?,
            max_frontier_size: 10_000,
            exhaustion_policy: crate::graph::budget::BudgetExhaustionPolicy::Partial,
        })
    }

    fn parse_search_config(
        env: Env,
        value: Option<JsUnknown>,
        advanced: bool,
    ) -> napi::Result<(crate::database::SearchConfig, Option<String>)> {
        let object = match value {
            Some(value) => argument_object(env, value, "config", None)?,
            None => env.create_object()?,
        };
        let config = crate::database::SearchConfig {
            top_k: parse_search_usize(env, object.get("topK")?, 5.0, "topK", true)?,
            recall_k: parse_search_usize(env, object.get("recallK")?, 0.0, "recallK", false)?,
            rerank_k: parse_search_usize(env, object.get("rerankK")?, 0.0, "rerankK", false)?,
            expand_depth: parse_search_usize(
                env,
                object.get("expandDepth")?,
                2.0,
                "expandDepth",
                false,
            )?,
            min_score: optional_number(env, object.get("minScore")?, 0.1, "minScore")? as f32,
            teleport_alpha: optional_number(
                env,
                object.get("teleportAlpha")?,
                0.0,
                "teleportAlpha",
            )? as f32,
            enable_advanced_pipeline: optional_bool(
                env,
                object.get("enableAdvancedPipeline")?,
                advanced,
                "enableAdvancedPipeline",
            )?,
            enable_sparse_residual: optional_bool(
                env,
                object.get("enableSparseResidual")?,
                false,
                "enableSparseResidual",
            )?,
            fista_lambda: optional_number(env, object.get("fistaLambda")?, 0.1, "fistaLambda")?
                as f32,
            fista_threshold: optional_number(
                env,
                object.get("fistaThreshold")?,
                0.3,
                "fistaThreshold",
            )? as f32,
            enable_dpp: optional_bool(env, object.get("enableDpp")?, false, "enableDpp")?,
            dpp_quality_weight: optional_number(
                env,
                object.get("dppQualityWeight")?,
                1.0,
                "dppQualityWeight",
            )? as f32,
            enable_refractory_fatigue: optional_bool(
                env,
                object.get("enableRefractoryFatigue")?,
                false,
                "enableRefractoryFatigue",
            )?,
            enable_text_hybrid_search: optional_bool(
                env,
                object.get("enableTextHybridSearch")?,
                false,
                "enableTextHybridSearch",
            )?,
            text_boost: optional_number(env, object.get("textBoost")?, 1.5, "textBoost")? as f32,
            force_brute_force: optional_bool(
                env,
                object.get("forceBruteForce")?,
                false,
                "forceBruteForce",
            )?,
            diffusion_bias: object
                .get("diffusionBias")?
                .map(|value| {
                    read_array(env, value, "diffusionBias", read_number)
                        .map(|values| values.into_iter().map(|value| value as f32).collect())
                })
                .transpose()?,
            expand_labels: object
                .get("expandLabels")?
                .map(|value| read_array(env, value, "expandLabels", read_string))
                .transpose()?,
            max_edges_per_node: parse_search_usize(
                env,
                object.get("maxEdgesPerNode")?,
                0.0,
                "maxEdgesPerNode",
                false,
            )?,
            min_edge_weight: optional_number(
                env,
                object.get("minEdgeWeight")?,
                0.0,
                "minEdgeWeight",
            )? as f32,
            edge_direction: parse_edge_direction(
                env,
                object
                    .get("edgeDirection")?
                    .map(|value| read_string(env, value, "edgeDirection"))
                    .transpose()?
                    .as_deref(),
            )?,
            payload_filter: parse_payload_filter(
                env,
                object
                    .get("payloadFilter")?
                    .map(|value| read_json(env, value, "payloadFilter"))
                    .transpose()?,
            )?,
            ..Default::default()
        };
        let query_text = object
            .get("customQueryText")?
            .map(|value| read_string(env, value, "customQueryText"))
            .transpose()?;
        Ok((config, query_text))
    }

    fn build_transaction<T: crate::VectorType>(
        env: Env,
        operations: &[serde_json::Value],
        convert: impl Fn(f64, &str) -> napi::Result<T>,
    ) -> napi::Result<crate::database::TxBuilder<T>> {
        let mut tx = crate::database::TxBuilder::new();
        for (index, operation) in operations.iter().enumerate() {
            let path = crate::sdk::field_member("operations", &index.to_string());
            let invalid = |field: &str| {
                argument_input_error(
                    env,
                    &crate::sdk::field_member(&path, field),
                    "事务操作字段无效或缺失 (Transaction operation field is invalid or missing)",
                    None,
                )
            };
            let kind = operation
                .get("type")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("type"))?;
            let id = |name: &str| -> napi::Result<u64> {
                let value = operation
                    .get(name)
                    .and_then(serde_json::Value::as_f64)
                    .ok_or_else(|| invalid(name))?;
                Ok(parse_safe_usize(env, value, &crate::sdk::field_member(&path, name))? as u64)
            };
            let vector = || -> napi::Result<Vec<T>> {
                operation
                    .get("vector")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| invalid("vector"))?
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        let field = crate::sdk::field_member(
                            &crate::sdk::field_member(&path, "vector"),
                            &index.to_string(),
                        );
                        value
                            .as_f64()
                            .ok_or_else(|| {
                                argument_input_error(
                                    env,
                                    &field,
                                    "向量成员必须是数字 (Vector components must be numbers)",
                                    None,
                                )
                            })
                            .and_then(|value| convert(value, &field))
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
                        .map(|value| value.as_str().ok_or_else(|| invalid("label")))
                        .transpose()?
                        .unwrap_or("related"),
                    operation
                        .get("weight")
                        .map(|value| value.as_f64().ok_or_else(|| invalid("weight")))
                        .transpose()?
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
                        .ok_or_else(|| invalid("label"))?,
                ),
                _ => {
                    return Err(invalid("type"));
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

    fn parse_row_overflow(
        env: Env,
        value: Option<&str>,
    ) -> napi::Result<crate::database::RowOverflowPolicy> {
        match value.unwrap_or("throw") {
            "throw" => Ok(crate::database::RowOverflowPolicy::Throw),
            "break" => Ok(crate::database::RowOverflowPolicy::Break),
            _ => Err(invalid_constructor_field(
                env,
                "rowOverflow",
                "rowOverflow 必须是 throw / break (rowOverflow must be throw / break)",
            )),
        }
    }

    fn parse_storage_mode(
        env: Env,
        value: Option<&str>,
    ) -> napi::Result<crate::database::StorageMode> {
        match value.unwrap_or("mmap") {
            "mmap" => Ok(crate::database::StorageMode::Mmap),
            "rom" => Ok(crate::database::StorageMode::Rom),
            _ => Err(invalid_constructor_field(
                env,
                "storageMode",
                "storageMode 必须是 mmap / rom (storageMode must be mmap / rom)",
            )),
        }
    }

    fn parse_access_mode(
        env: Env,
        value: Option<&str>,
    ) -> napi::Result<crate::database::AccessMode> {
        match value.unwrap_or("readWrite") {
            "readWrite" => Ok(crate::database::AccessMode::ReadWrite),
            "readOnly" => Ok(crate::database::AccessMode::ReadOnly),
            "immutable" => Ok(crate::database::AccessMode::Immutable),
            _ => Err(invalid_constructor_field(
                env,
                "accessMode",
                "accessMode 必须是 readWrite / readOnly / immutable (accessMode must be readWrite / readOnly / immutable)",
            )),
        }
    }

    fn parse_missing_index_policy(
        env: Env,
        value: Option<&str>,
    ) -> napi::Result<crate::database::MissingIndexPolicy> {
        match value.unwrap_or("fallback") {
            "fallback" => Ok(crate::database::MissingIndexPolicy::Fallback),
            "buildInMemory" => Ok(crate::database::MissingIndexPolicy::BuildInMemory),
            "error" => Ok(crate::database::MissingIndexPolicy::Error),
            _ => Err(invalid_constructor_field(
                env,
                "missingIndexPolicy",
                "missingIndexPolicy 必须是 fallback / buildInMemory / error (missingIndexPolicy must be fallback / buildInMemory / error)",
            )),
        }
    }

    fn parse_sync_mode(env: Env, s: &str) -> napi::Result<crate::storage::wal::SyncMode> {
        crate::storage::wal::SyncMode::parse(s)
            .map_err(|reason| invalid_constructor_field(env, "syncMode", &reason))
    }

    fn parse_payload_filter(
        env: Env,
        value: Option<serde_json::Value>,
    ) -> napi::Result<Option<crate::filter::Filter>> {
        value
            .map(|filter| {
                crate::filter::Filter::from_json(&filter).map_err(|error| {
                    to_napi_error(
                        env,
                        crate::TriviumError::InvalidInput(format!(
                            "payloadFilter 无效 (Invalid payloadFilter): {error}"
                        ))
                        .at_field("payloadFilter"),
                    )
                })
            })
            .transpose()
    }

    fn parse_edge_direction(
        env: Env,
        value: Option<&str>,
    ) -> napi::Result<crate::database::EdgeDirection> {
        match value.unwrap_or("out") {
            "out" | "outgoing" => Ok(crate::database::EdgeDirection::Outgoing),
            "in" | "incoming" => Ok(crate::database::EdgeDirection::Incoming),
            "both" => Ok(crate::database::EdgeDirection::Both),
            _ => Err(argument_input_error(
                env,
                "edgeDirection",
                "edgeDirection 必须是 out / in / both (edgeDirection must be out / in / both)",
                None,
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
            let next = dispatch!(self,db=>db.next_id_for_binding()).map_err(|error| {
                to_napi_error(
                    env,
                    error.with_write_info(crate::error::WriteErrorInfo {
                        transaction_consumed: None,
                        ..crate::error::WriteErrorInfo::TRANSACTION_ADMISSION
                    }),
                )
            })?;
            let last = next.checked_add(count as u64 - 1).ok_or_else(|| {
                to_napi_error(
                    env,
                    crate::TriviumError::InvalidInput(
                        "节点 ID 分配溢出 (Node ID allocation overflow)".into(),
                    )
                    .with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )
            })?;
            node_id_to_js(env, last).map_err(|error| {
                with_napi_write_info(
                    env,
                    error,
                    crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION,
                )
            })?;
            Ok(())
        }

        fn check_transaction_ids(
            &self,
            env: Env,
            operations: &[serde_json::Value],
        ) -> napi::Result<()> {
            let mut next = dispatch!(self,db=>db.next_id_for_binding())
                .map_err(|error| to_napi_error(env, error))?;
            for (index, operation) in operations.iter().enumerate() {
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
                                argument_input_error(
                                    env,
                                    &format!("/operations/{index}/id"),
                                    "缺少事务 ID (Missing transaction ID)",
                                    None,
                                )
                            })?;
                        next = next
                            .max(parse_node_id(env, id, &format!("/operations/{index}/id"))? + 1);
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
        pub fn new(env: Env, path: JsUnknown, options: Option<JsUnknown>) -> napi::Result<Self> {
            let path = raw_string(env, path, "path", None)?;
            if let Some(value) = options.as_ref()
                && value.get_type()? == ValueType::Number
            {
                return Err(to_napi_error(
                    env,
                    crate::TriviumError::ApiMigrationRequired {
                        removed_api: "new TriviumDB(path, dim)",
                        replacement: "new TriviumDB(path, { dim })",
                    }
                    .at_field("options"),
                ));
            }
            let object = constructor_options(env, options)?;
            let options = JsDatabaseOptions {
                runtime: object.get("runtime")?,
                dim: object.get("dim")?,
                dtype: object.get("dtype")?,
                sync_mode: object.get("syncMode")?,
                storage_mode: object.get("storageMode")?,
                auto_build_quiver: object.get("autoBuildQuiver")?,
                load_text_index: object.get("loadTextIndex")?,
                expected_nodes: object.get("expectedNodes")?,
                memory_limit_mb: object.get("memoryLimitMb")?,
                payload_cache_mb: object.get("payloadCacheMb")?,
                payload_cache_entry_mb: object.get("payloadCacheEntryMb")?,
                access_mode: object.get("accessMode")?,
                missing_index_policy: object.get("missingIndexPolicy")?,
                max_query_rows: object.get("maxQueryRows")?,
                row_overflow: object.get("rowOverflow")?,
            };
            let dim = parse_search_usize(env, options.dim, 1536.0, "dim", true)?;
            let dtype = constructor_string(env, options.dtype, "f32", "dtype")?;
            let expected_nodes = options
                .expected_nodes
                .map(|value| parse_usize_argument(env, value, "expectedNodes", None))
                .transpose()?;
            let memory_limit_mb = options
                .memory_limit_mb
                .map(|value| parse_usize_argument(env, value, "memoryLimitMb", None))
                .transpose()?
                .unwrap_or(0);
            let memory_limit =
                constructor_mib(env, memory_limit_mb, "memoryLimitMb", usize::MAX as u128)?;
            let mb_to_bytes =
                |value: Option<Either<f64, JsUnknown>>, name: &str, default_mb: usize| {
                    let mb = value
                        .map(|value| parse_usize_argument(env, value, name, None))
                        .transpose()?
                        .unwrap_or(default_mb);
                    constructor_mib(env, mb, name, usize::MAX as u128)
                };
            let payload_cache_bytes = mb_to_bytes(options.payload_cache_mb, "payloadCacheMb", 64)?;
            let payload_cache_entry_bytes =
                mb_to_bytes(options.payload_cache_entry_mb, "payloadCacheEntryMb", 8)?;
            let max_query_rows = options
                .max_query_rows
                .map(|value| parse_usize_argument(env, value, "maxQueryRows", None))
                .transpose()?;
            let config = crate::database::Config {
                dim,
                sync_mode: parse_sync_mode(
                    env,
                    &constructor_string(env, options.sync_mode, "normal", "syncMode")?,
                )?,
                storage_mode: parse_storage_mode(
                    env,
                    Some(&constructor_string(
                        env,
                        options.storage_mode,
                        "mmap",
                        "storageMode",
                    )?),
                )?,
                auto_build_quiver: constructor_bool(
                    env,
                    options.auto_build_quiver,
                    true,
                    "autoBuildQuiver",
                )?,
                load_text_index: constructor_bool(
                    env,
                    options.load_text_index,
                    false,
                    "loadTextIndex",
                )?,
                expected_nodes,
                memory_limit,
                payload_cache_bytes,
                payload_cache_entry_bytes,
                access_mode: parse_access_mode(
                    env,
                    Some(&constructor_string(
                        env,
                        options.access_mode,
                        "readWrite",
                        "accessMode",
                    )?),
                )?,
                missing_index_policy: parse_missing_index_policy(
                    env,
                    Some(&constructor_string(
                        env,
                        options.missing_index_policy,
                        "fallback",
                        "missingIndexPolicy",
                    )?),
                )?,
                max_query_rows,
                row_overflow: parse_row_overflow(
                    env,
                    Some(&constructor_string(
                        env,
                        options.row_overflow,
                        "throw",
                        "rowOverflow",
                    )?),
                )?,
            };
            let runtime = parse_runtime(env, options.runtime, None)?;
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
                _ => {
                    return Err(invalid_constructor_field(
                        env,
                        "dtype",
                        "dtype 必须是 f32 / f16 / u64 (dtype must be f32 / f16 / u64)",
                    ));
                }
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
        pub fn load_ffi_hook(&mut self, env: Env, lib_path: JsUnknown) -> napi::Result<()> {
            let lib_path = read_string(env, lib_path, "libPath")?;
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

        // ── CRUD ──

        /// 插入节点，返回新节点 ID
        #[napi]
        pub fn insert(
            &mut self,
            env: Env,
            vector: JsUnknown,
            payload: JsUnknown,
        ) -> napi::Result<f64> {
            let vector = write_vector(env, vector, "vector", &self.dtype)?;
            let payload = write_json(env, payload, "payload")?;
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
            vectors: JsUnknown,
            payloads: JsUnknown,
        ) -> napi::Result<Vec<f64>> {
            let vectors = write_array(env, vectors, "vectors", |env, value, path| {
                write_vector(env, value, path, &self.dtype)
            })?;
            let payloads = write_array(env, payloads, "payloads", write_json)?;
            if vectors.len() != payloads.len() {
                return Err(write_input_error(
                    env,
                    "payloads",
                    "向量列表与 Payload 列表长度不一致 (Vector and payload list lengths differ)",
                ));
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
                        .map_err(crate::sdk::direct_write_error)
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
                        .map_err(crate::sdk::direct_write_error)
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
                        .map_err(crate::sdk::direct_write_error)
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
            ids: JsUnknown,
            vectors: JsUnknown,
            payloads: JsUnknown,
        ) -> napi::Result<()> {
            let parsed_ids = write_array(env, ids, "ids", write_id)?;
            let vectors = write_array(env, vectors, "vectors", |env, value, path| {
                write_vector(env, value, path, &self.dtype)
            })?;
            let payloads = write_array(env, payloads, "payloads", write_json)?;
            if vectors.len() != payloads.len() {
                return Err(write_input_error(
                    env,
                    "payloads",
                    "向量列表与 Payload 列表长度不一致 (Vector and payload list lengths differ)",
                ));
            }
            if parsed_ids.len() != vectors.len() {
                return Err(write_input_error(
                    env,
                    "ids",
                    "ID 列表与向量列表长度不一致 (ID and vector list lengths differ)",
                ));
            }
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
                        .map_err(crate::sdk::direct_write_error)
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
                        .map_err(crate::sdk::direct_write_error)
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
                        .map_err(crate::sdk::direct_write_error)
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
            id: JsUnknown,
            vector: JsUnknown,
            payload: JsUnknown,
        ) -> napi::Result<()> {
            let id = write_id(env, id, "id")?;
            let vector = write_vector(env, vector, "vector", &self.dtype)?;
            let payload = write_json(env, payload, "payload")?;
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
            id: JsUnknown,
            vector: JsUnknown,
            payload: JsUnknown,
        ) -> napi::Result<()> {
            let id = write_id(env, id, "id")?;
            let vector = write_vector(env, vector, "vector", &self.dtype)?;
            let payload = write_json(env, payload, "payload")?;
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
            operations: JsUnknown,
        ) -> napi::Result<Vec<f64>> {
            dispatch!(self, db => db.check_transaction_admission()).map_err(|error| {
                to_napi_error(
                    env,
                    error.with_write_info(crate::error::WriteErrorInfo::TRANSACTION_ADMISSION),
                )
            })?;
            let write = crate::error::WriteErrorInfo {
                phase: crate::error::WritePhase::Validation,
                ..crate::error::WriteErrorInfo::TRANSACTION_ADMISSION
            };
            let operations = NodeJson::from_js_field(env, operations, "operations", Some(write))?.0;
            let operations = operations.as_array().ok_or_else(|| {
                argument_input_error(
                    env,
                    "operations",
                    "事务操作必须是数组 (Transaction operations must be an array)",
                    Some(write),
                )
            })?;
            let validation = |error| {
                with_napi_write_info(
                    env,
                    error,
                    crate::error::WriteErrorInfo {
                        phase: crate::error::WritePhase::Validation,
                        ..crate::error::WriteErrorInfo::TRANSACTION_ADMISSION
                    },
                )
            };
            self.check_transaction_ids(env, operations)
                .map_err(validation)?;
            let ids = match &mut self.inner {
                DbBackend::F32(db) => db.commit_tx(
                    build_transaction(env, operations, |value, _| Ok(value as f32))
                        .map_err(validation)?,
                ),
                DbBackend::F16(db) => db.commit_tx(
                    build_transaction(env, operations, |value, _| Ok(half::f16::from_f64(value)))
                        .map_err(validation)?,
                ),
                DbBackend::U64(db) => db.commit_tx(
                    build_transaction(env, operations, |value, field| {
                        validate_u64_component(value)
                            .map_err(|error| to_napi_error(env, error.at_field(field)))
                    })
                    .map_err(validation)?,
                ),
            }
            .map_err(|error| to_napi_error(env, error))?;
            ids.into_iter()
                .map(|value| node_id_to_js(env, value))
                .collect::<napi::Result<_>>()
                .map_err(|error| {
                    with_napi_write_info(
                        env,
                        error,
                        crate::error::WriteErrorInfo {
                            phase: crate::error::WritePhase::Response,
                            outcome: crate::error::WriteOutcome::Committed,
                            transaction_consumed: Some(true),
                        },
                    )
                })
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
        pub fn get(
            &self,
            env: Env,
            id: Either<f64, JsUnknown>,
        ) -> napi::Result<Option<JsNodeView>> {
            {
                let id = parse_usize_argument(env, id, "id", None)? as u64;
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
        pub fn update_payload(
            &mut self,
            env: Env,
            id: JsUnknown,
            payload: JsUnknown,
        ) -> napi::Result<()> {
            let id = write_id(env, id, "id")?;
            let payload = write_json(env, payload, "payload")?;
            dispatch!(self, mut db => db.update_payload(id, payload))
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
        /// ```
        #[napi]
        pub fn patch_payload(
            &mut self,
            env: Env,
            id: JsUnknown,
            patch: JsUnknown,
        ) -> napi::Result<()> {
            let id = write_id(env, id, "id")?;
            let patch = write_json(env, patch, "patch")?;
            dispatch!(self, mut db => db.patch_payload(id, patch))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 更新节点向量
        #[napi]
        pub fn update_vector(
            &mut self,
            env: Env,
            id: JsUnknown,
            vector: JsUnknown,
        ) -> napi::Result<()> {
            let id = write_id(env, id, "id")?;
            let vector = write_vector(env, vector, "vector", &self.dtype)?;
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
        pub fn delete_many_atomic(&mut self, env: Env, ids: JsUnknown) -> napi::Result<u32> {
            let ids = write_array(env, ids, "ids", write_id)?;
            let deleted = dispatch!(self, mut db => db.delete_many_atomic(&ids))
                .map_err(|error| to_napi_error(env, error))?;
            u32::try_from(deleted).map_err(|_| napi::Error::from_reason("删除数量超出 u32"))
        }

        #[napi]
        pub fn compare_and_set_payload_field(
            &mut self,
            env: Env,
            id: JsUnknown,
            field: JsUnknown,
            expected: JsUnknown,
            replacement: JsUnknown,
        ) -> napi::Result<()> {
            let id = write_id(env, id, "id")?;
            let field = write_string(env, field, "field")?;
            let expected = write_json(env, expected, "expected")?;
            let replacement = write_json(env, replacement, "replacement")?;
            dispatch!(self, mut db => db.compare_and_set_payload_field(id, &field, &expected, replacement))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 删除节点（三层原子联删：向量 + Payload + 所有关联边）
        #[napi]
        pub fn delete(&mut self, env: Env, id: JsUnknown) -> napi::Result<()> {
            let id = write_id(env, id, "id")?;
            dispatch!(self, mut db => db.delete(id)).map_err(|error| to_napi_error(env, error))
        }

        // ── 图谱操作 ──

        /// 建立有向带权边
        #[napi]
        pub fn link(
            &mut self,
            env: Env,
            src: JsUnknown,
            dst: JsUnknown,
            label: Option<JsUnknown>,
            weight: Option<JsUnknown>,
        ) -> napi::Result<()> {
            let src = write_id(env, src, "src")?;
            let dst = write_id(env, dst, "dst")?;
            let label = label
                .map(|value| write_string(env, value, "label"))
                .transpose()?
                .unwrap_or_else(|| "related".into());
            let weight = weight
                .map(|value| write_number(env, value, "weight"))
                .transpose()?
                .unwrap_or(1.0) as f32;
            dispatch!(self, mut db => db.link(src, dst, &label, weight))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn get_edge(
            &self,
            env: Env,
            src: Either<f64, JsUnknown>,
            dst: Either<f64, JsUnknown>,
            label: JsUnknown,
        ) -> napi::Result<Option<JsEdge>> {
            let src = parse_usize_argument(env, src, "src", None)? as u64;
            let dst = parse_usize_argument(env, dst, "dst", None)? as u64;
            let label = raw_string(env, label, "label", None)?;
            Ok({
                dispatch!(self, db => db.get_edge(src, dst, &label).map_err(|error| to_napi_error(env, error))?).map(|edge| Ok::<_,napi::Error>(JsEdge {
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
            src: JsUnknown,
            dst: JsUnknown,
            label: JsUnknown,
            weight: JsUnknown,
            metadata: Option<JsUnknown>,
        ) -> napi::Result<()> {
            let src = write_id(env, src, "src")?;
            let dst = write_id(env, dst, "dst")?;
            let label = write_string(env, label, "label")?;
            let weight = write_number(env, weight, "weight")?;
            let metadata = metadata
                .map(|value| write_json(env, value, "metadata"))
                .transpose()?;
            dispatch!(self, mut db => db.upsert_edge(
                src,
                dst,
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
            src: JsUnknown,
            dst: JsUnknown,
            label: JsUnknown,
            weight: Option<JsUnknown>,
            metadata: Option<JsUnknown>,
        ) -> napi::Result<()> {
            let src = write_id(env, src, "src")?;
            let dst = write_id(env, dst, "dst")?;
            let label = write_string(env, label, "label")?;
            let weight = weight
                .map(|value| write_number(env, value, "weight"))
                .transpose()?;
            let metadata = metadata
                .map(|value| write_json(env, value, "metadata"))
                .transpose()?;
            dispatch!(self, mut db => db.update_edge(
                src,
                dst,
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
            src: JsUnknown,
            dst: JsUnknown,
            label: Option<JsUnknown>,
        ) -> napi::Result<()> {
            let src = write_id(env, src, "src")?;
            let dst = write_id(env, dst, "dst")?;
            let label = label
                .map(|value| write_string(env, value, "label"))
                .transpose()?;
            match label {
                Some(label) => {
                    dispatch!(self, mut db => db.unlink_label(src, dst, &label))
                }
                None => dispatch!(self, mut db => db.unlink(src, dst)),
            }
            .map_err(|error| to_napi_error(env, error))
        }

        /// 获取 N 跳邻居节点 ID 列表，可按标签白名单遍历。
        #[napi]
        pub fn neighbors(
            &self,
            env: Env,
            id: Either<f64, JsUnknown>,
            depth: Option<Either<f64, JsUnknown>>,
            labels: Option<JsUnknown>,
        ) -> napi::Result<Vec<f64>> {
            let labels = labels
                .map(|value| read_array(env, value, "labels", read_string))
                .transpose()?;
            Ok({
                let depth = parse_search_usize(env, depth, 1.0, "depth", false)?;
                let id = parse_usize_argument(env, id, "id", None)? as u64;
                dispatch!(self, db => db.neighbors_with_labels(id, depth, labels.as_deref()).map_err(|error| to_napi_error(env, error))?)
                .into_iter()
                .map(|value| node_id_to_js(env, value)).collect::<napi::Result<_>>()?
            })
        }

        #[napi]
        pub fn reachable(
            &self,
            env: Env,
            id: Either<f64, JsUnknown>,
            options: Option<JsUnknown>,
        ) -> napi::Result<Vec<JsReachabilityResult>> {
            let config = parse_reachability_options(env, options)?;
            let id = parse_usize_argument(env, id, "id", None)? as u64;
            dispatch!(self, db => db.reachable(id, &config))
                .map_err(|error| to_napi_error(env, error))?
                .into_iter()
                .map(|value| to_js_reachability(env, value))
                .collect()
        }

        #[napi]
        pub fn reachable_detailed(
            &self,
            env: Env,
            id: Either<f64, JsUnknown>,
            options: Option<JsUnknown>,
        ) -> napi::Result<JsReachabilityOutput> {
            let config = parse_reachability_options(env, options)?;
            let id = parse_usize_argument(env, id, "id", None)? as u64;
            let output = dispatch!(self, db => db.reachable_detailed(id, &config))
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
            id: Either<f64, JsUnknown>,
            options: Option<JsUnknown>,
        ) -> napi::Result<JsSubgraphResult> {
            let config = parse_reachability_options(env, options)?;
            let id = parse_usize_argument(env, id, "id", None)? as u64;
            let output = dispatch!(self, db => db.query_subgraph(id, &config))
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

        // ── 社区聚类 ──

        /// 基于物理记忆图谱进行 Leiden 社区发现
        ///
        /// **无锁设计**: 短暂持锁快照邻接表后立即释放，聚类在锁外计算。
        /// 调用期间数据库仍可正常读写。
        #[napi]
        pub fn leiden_cluster(
            &self,
            env: Env,
            config: Option<JsUnknown>,
        ) -> napi::Result<JsClusterResult> {
            let object = match config {
                Some(value) => argument_object(env, value, "config", None)?,
                None => env.create_object()?,
            };
            let min_c = parse_search_usize(
                env,
                object.get("minCommunitySize")?,
                3.0,
                "minCommunitySize",
                false,
            )?;
            let max_iter = parse_search_usize(
                env,
                object.get("maxIterations")?,
                15.0,
                "maxIterations",
                false,
            )?;
            let with_cent =
                optional_bool(env, object.get("withCentroids")?, true, "withCentroids")?;

            let result =
                dispatch!(self, db => db.leiden_cluster(min_c, Some(max_iter), Some(with_cent)))
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

        #[napi]
        pub fn search_batch(
            &self,
            env: Env,
            query_vectors: JsUnknown,
            top_k: Option<Either<f64, JsUnknown>>,
            parallelism: Option<Either<f64, JsUnknown>>,
            min_score: Option<JsUnknown>,
        ) -> napi::Result<napi::bindgen_prelude::AsyncTask<BatchSearchTask>> {
            let query_vectors =
                read_array(env, query_vectors, "queryVectors", |env, value, path| {
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

        // ── 文本索引 ──

        /// 对节点建立用于双路召回的长文本 BM25 索引
        #[napi]
        pub fn index_text(&mut self, env: Env, id: JsUnknown, text: JsUnknown) -> napi::Result<()> {
            let id = write_id(env, id, "id")?;
            let text = write_string(env, text, "text")?;
            dispatch!(self, mut db => db.index_text(id, &text))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 对节点建立用于精确命中的 AC自动机 高级关键词索引
        #[napi]
        pub fn index_keyword(
            &mut self,
            env: Env,
            id: JsUnknown,
            keyword: JsUnknown,
        ) -> napi::Result<()> {
            let id = write_id(env, id, "id")?;
            let keyword = write_string(env, keyword, "keyword")?;
            dispatch!(self, mut db => db.index_keyword(id, &keyword))
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
        pub fn rebuild_text_index(&mut self, env: Env, documents: JsUnknown) -> napi::Result<()> {
            let documents = write_array(env, documents, "documents", |env, value, path| {
                let write = Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION);
                let document = argument_object(env, value, path, write)?;
                let keys = document.get_all_property_names(
                    napi::KeyCollectionMode::OwnOnly,
                    napi::KeyFilter::Enumerable,
                    napi::KeyConversion::NumbersToStrings,
                )?;
                for index in 0..keys.get_array_length()? {
                    let key = write_string(env, keys.get_element(index)?, path)?;
                    if !matches!(key.as_str(), "id" | "text" | "keywords") {
                        return Err(argument_input_error(
                            env,
                            &crate::sdk::field_member(path, &key),
                            "文本重建记录包含未知字段 (Text rebuild record contains an unknown field)",
                            write,
                        ));
                    }
                }
                let required = |name: &str| {
                    document.get(name)?.ok_or_else(|| {
                        argument_input_error(
                            env,
                            &crate::sdk::field_member(path, name),
                            "文本重建记录缺少字段 (Text rebuild record is missing a field)",
                            write,
                        )
                    })
                };
                Ok(crate::index::text::TextIndexDocument {
                    id: write_id(env, required("id")?, &crate::sdk::field_member(path, "id"))?,
                    text: write_string(
                        env,
                        required("text")?,
                        &crate::sdk::field_member(path, "text"),
                    )?,
                    keywords: document
                        .get("keywords")?
                        .map(|value| {
                            write_array(
                                env,
                                value,
                                &crate::sdk::field_member(path, "keywords"),
                                write_string,
                            )
                        })
                        .transpose()?
                        .unwrap_or_default(),
                })
            })?;
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
            equalities: JsUnknown,
            max_results: Option<Either<f64, JsUnknown>>,
        ) -> napi::Result<Vec<f64>> {
            let value = read_json(env, equalities, "equalities")?;
            let object = value.as_object().ok_or_else(|| {
                argument_input_error(
                    env,
                    "equalities",
                    "equalities 必须是对象 (equalities must be an object)",
                    None,
                )
            })?;
            let equalities = object
                .iter()
                .map(|(field, value)| (field.clone(), value.clone()))
                .collect::<Vec<_>>();
            let max_results = parse_search_usize(env, max_results, 10_000.0, "maxResults", false)?;
            dispatch!(self, db => db.indexed_lookup(&equalities, max_results))
                .map_err(|error| to_napi_error(env, error))
                .and_then(|value| node_ids_to_js(env, value))
        }

        #[napi]
        pub fn create_ngram_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
            let field = write_string(env, field, "field")?;
            dispatch!(self, mut db => db.create_ngram_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn drop_ngram_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
            let field = write_string(env, field, "field")?;
            dispatch!(self, mut db => db.drop_ngram_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn substring_lookup(
            &self,
            env: Env,
            field: JsUnknown,
            needle: JsUnknown,
            max_results: Option<Either<f64, JsUnknown>>,
        ) -> napi::Result<Vec<f64>> {
            let field = read_string(env, field, "field")?;
            let needle = read_string(env, needle, "needle")?;
            let max_results = parse_search_usize(env, max_results, 10_000.0, "maxResults", false)?;
            dispatch!(self, db => db.substring_lookup(&field, &needle, max_results))
                .map_err(|error| to_napi_error(env, error))
                .and_then(|value| node_ids_to_js(env, value))
        }

        #[napi]
        pub fn create_unique_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
            let field = write_string(env, field, "field")?;
            dispatch!(self, mut db => db.create_unique_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn create_unique_composite_index(
            &mut self,
            env: Env,
            fields: JsUnknown,
        ) -> napi::Result<()> {
            let fields = write_array(env, fields, "fields", write_string)?;
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
        pub fn create_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
            let field = write_string(env, field, "field")?;
            dispatch!(self, mut db => db.create_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn create_ordered_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
            let field = write_string(env, field, "field")?;
            dispatch!(self, mut db => db.create_ordered_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn create_composite_index(&mut self, env: Env, fields: JsUnknown) -> napi::Result<()> {
            let fields = write_array(env, fields, "fields", write_string)?;
            dispatch!(self, mut db => db.create_composite_index(&fields))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn create_bitmap_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
            let field = write_string(env, field, "field")?;
            dispatch!(self, mut db => db.create_bitmap_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 删除属性索引（查询仍可用，退化为全扫描）
        #[napi]
        pub fn drop_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
            let field = write_string(env, field, "field")?;
            dispatch!(self, mut db => db.drop_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn drop_ordered_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
            let field = write_string(env, field, "field")?;
            dispatch!(self, mut db => db.drop_ordered_index(&field))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn drop_composite_index(&mut self, env: Env, fields: JsUnknown) -> napi::Result<()> {
            let fields = write_array(env, fields, "fields", write_string)?;
            dispatch!(self, mut db => db.drop_composite_index(&fields))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn drop_bitmap_index(&mut self, env: Env, field: JsUnknown) -> napi::Result<()> {
            let field = write_string(env, field, "field")?;
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
        pub fn get_payload(
            &self,
            env: Env,
            id: Either<f64, JsUnknown>,
        ) -> napi::Result<Option<NodeJson>> {
            let id = parse_usize_argument(env, id, "id", None)? as u64;
            Ok(
                dispatch!(self, db => db.get_payload(id).map_err(|error| to_napi_error(env, error))?).map(NodeJson),
            )
        }

        /// 获取节点的出边列表
        #[napi]
        pub fn get_edges(&self, env: Env, id: Either<f64, JsUnknown>) -> napi::Result<Vec<JsEdge>> {
            let id = parse_usize_argument(env, id, "id", None)? as u64;
            Ok({
                dispatch!(self, db => db.get_edges(id).map_err(|error| to_napi_error(env, error))?)
                    .into_iter()
                    .map(|e| {
                        Ok(JsEdge {
                            target_id: node_id_to_js(env, e.target_id)?,
                            label: e.label,
                            weight: round_api_f32(e.weight),
                            metadata: e.metadata.into(),
                        })
                    })
                    .collect::<napi::Result<_>>()?
            })
        }

        /// 获取节点完整入边，可按标签过滤。
        #[napi]
        pub fn get_incoming_edges(
            &self,
            env: Env,
            id: Either<f64, JsUnknown>,
            label: Option<JsUnknown>,
        ) -> napi::Result<Vec<JsIncomingEdge>> {
            let id = parse_usize_argument(env, id, "id", None)? as u64;
            let label = label
                .map(|value| raw_string(env, value, "label", None))
                .transpose()?;
            Ok({
                dispatch!(self, db => db.get_incoming_edges(id, label.as_deref()).map_err(|error| to_napi_error(env, error))?)
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
        pub fn tql(
            &self,
            env: Env,
            query: Either<String, JsUnknown>,
        ) -> napi::Result<Vec<NodeJson>> {
            let query = parse_string_argument(env, query, "query", None)?;
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
        pub fn prepare_tql(
            &self,
            env: Env,
            query: Either<String, JsUnknown>,
        ) -> napi::Result<JsPreparedTql> {
            let query = parse_string_argument(env, query, "query", None)?;
            dispatch!(self, db => db.prepare_tql(&query))
                .map(|inner| JsPreparedTql { inner })
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn execute_prepared_tql(
            &self,
            env: Env,
            prepared: &JsPreparedTql,
            parameters: JsUnknown,
        ) -> napi::Result<Vec<NodeJson>> {
            let values = collect_prepared_parameters(env, parameters, &prepared.inner)?;
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

        /// 文档JSON适配复用Core的解析、来源映射、分批发布、修订、检索与证据上下文。
        #[napi]
        pub fn document_request(
            &mut self,
            env: Env,
            request_json: JsUnknown,
        ) -> napi::Result<String> {
            let request_json = read_string(env, request_json, "requestJson")?;
            dispatch!(self, mut db => crate::sdk::documents::request(db, &request_json))
                .map_err(|error| to_napi_error(env, error))
        }

        /// SSD运行时JSON适配复用Core的事实绑定、驻留、缓存与I/O生命周期。
        #[napi]
        pub fn quiver_request(&self, env: Env, request_json: JsUnknown) -> napi::Result<String> {
            let request_json = read_string(env, request_json, "requestJson")?;
            dispatch!(self, db => crate::sdk::quiver::request(db, &request_json))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 命名图JSON适配复用Core的成员、独立边、修订及作用域契约。
        #[napi]
        pub fn graph_request(&mut self, env: Env, request_json: JsUnknown) -> napi::Result<String> {
            let request_json = read_string(env, request_json, "requestJson")?;
            dispatch!(self, mut db => crate::sdk::named_graphs::request(db, &request_json))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 同步执行宿主控制台请求。
        #[napi]
        pub fn console_request(
            &mut self,
            env: Env,
            session: &JsConsoleSession,
            request_json: JsUnknown,
        ) -> napi::Result<String> {
            let request_json = read_string(env, request_json, "requestJson")?;
            dispatch!(self, mut db => db.console_request(&session.inner, &request_json))
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
        /// 执行内核 TQL 写操作。
        #[napi]
        pub fn tql_mut(
            &mut self,
            env: Env,
            query: Either<String, JsUnknown>,
        ) -> napi::Result<NodeJson> {
            let query = parse_management_string(env, query, "query")?;
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
            generation_id: Either<String, JsUnknown>,
        ) -> napi::Result<NodeJson> {
            let generation_id = parse_management_string(env, generation_id, "generationId")?;
            let manifest =
                dispatch!(self, mut db => db.publish_generation_manifest(&generation_id))
                    .map_err(|error| to_napi_error(env, error))?;
            node_json_value(manifest).map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        /// 运行时切换 WAL 同步模式
        #[napi]
        pub fn set_sync_mode(
            &mut self,
            env: Env,
            mode: Either<String, JsUnknown>,
        ) -> napi::Result<()> {
            let mode = parse_management_string(env, mode, "mode")?;
            let sm = crate::storage::wal::SyncMode::parse(&mode).map_err(|reason| {
                to_napi_error(
                    env,
                    crate::TriviumError::InvalidInput(reason)
                        .at_field("mode")
                        .with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )
            })?;
            dispatch!(self, mut db => db.set_sync_mode(sm))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 启动后台自动压缩（每 interval_secs 秒落盘一次，默认 2 小时=7200秒）
        #[napi]
        pub fn enable_auto_compaction(
            &mut self,
            env: Env,
            interval_secs: Option<Either<f64, JsUnknown>>,
        ) -> napi::Result<()> {
            let secs = parse_management_usize(
                env,
                interval_secs.unwrap_or(Either::A(7200.0)),
                "intervalSecs",
            )? as u64;
            dispatch!(self, mut db => db.enable_auto_compaction(std::time::Duration::from_secs(secs)))
                .map_err(|error| to_napi_error(env, error))
        }

        #[napi]
        pub fn set_auto_build_quiver(
            &mut self,
            env: Env,
            enabled: Either<bool, JsUnknown>,
        ) -> napi::Result<()> {
            let enabled = match enabled {
                Either::A(enabled) => enabled,
                Either::B(_) => {
                    return Err(to_napi_error(
                        env,
                        crate::TriviumError::InvalidInput(
                            "enabled 必须是布尔值 (enabled must be a boolean)".into(),
                        )
                        .at_field("enabled")
                        .with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                    ));
                }
            };
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
        pub fn reserve_nodes(
            &self,
            env: Env,
            additional: Either<f64, JsUnknown>,
        ) -> napi::Result<()> {
            let additional = parse_management_usize(env, additional, "additional")?;
            dispatch!(self, db => db.reserve_nodes(additional))
                .map_err(|error| to_napi_error(env, error))
        }

        /// 设置内存上限（MiB），0 = 无限制。
        #[napi]
        pub fn set_memory_limit(
            &mut self,
            env: Env,
            mb: Either<f64, JsUnknown>,
        ) -> napi::Result<()> {
            let mb = parse_management_usize(env, mb, "mb")?;
            let bytes = mb.checked_mul(1024 * 1024).ok_or_else(|| {
                to_napi_error(
                    env,
                    crate::TriviumError::InvalidInput(
                        "内存上限换算字节时溢出 (Memory limit overflows when converted to bytes)"
                            .into(),
                    )
                    .at_field("mb")
                    .with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )
            })?;
            dispatch!(self, mut db => db.set_memory_limit(bytes)).map_err(|error| {
                let error = if error.info().target.is_some() {
                    error.at_field("mb")
                } else {
                    error
                };
                to_napi_error(env, error)
            })
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
        pub fn migrate(
            &self,
            env: Env,
            new_path: JsUnknown,
            new_dim: JsUnknown,
        ) -> napi::Result<Vec<f64>> {
            let new_path = write_string(env, new_path, "newPath")?;
            let new_dim = write_number(env, new_dim, "newDim").and_then(|value| {
                parse_positive_usize(env, value, "newDim").map_err(|error| {
                    with_napi_write_info(
                        env,
                        error,
                        crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION,
                    )
                })
            })?;
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
        pub fn contains(&self, env: Env, id: Either<f64, JsUnknown>) -> napi::Result<bool> {
            let id = parse_usize_argument(env, id, "id", None)? as u64;
            Ok(dispatch!(self, db => db.contains(id).map_err(|error| to_napi_error(env, error))?))
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
            timeout_ms: JsUnknown,
        ) -> napi::Result<bool> {
            let milliseconds =
                parse_safe_usize(env, read_number(env, timeout_ms, "timeoutMs")?, "timeoutMs")?;
            let closed = dispatch!(self, mut db => db.close_timeout(std::time::Duration::from_millis(milliseconds as u64)))
                .map_err(|e| to_napi_error(env, e))?;
            super::super::node_hook_refs::flush(env, &this)?;
            Ok(closed)
        }

        #[napi]
        pub fn create_snapshot(
            &mut self,
            env: Env,
            destination: JsUnknown,
            generation_id: JsUnknown,
        ) -> napi::Result<NodeJson> {
            let destination = write_string(env, destination, "destination")?;
            let generation_id = write_string(env, generation_id, "generationId")?;
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
