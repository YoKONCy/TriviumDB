//! Node.js napi-rs 公共绑定。
//!
//! 该层只负责 JS 值校验、camelCase API、异步任务和稳定错误码映射，核心语义必须委托
//! Rust Database/TQL 实现。u64 NodeId 在一等查询值中使用字符串，避免超过 JS 安全整数。

#[cfg(feature = "nodejs")]
#[allow(clippy::module_inception)]
pub mod nodejs {
    use crate::database::Database as GenericDatabase;
    use napi_derive::napi;

    fn to_napi_error(error: crate::error::TriviumError) -> napi::Error {
        let code = match &error {
            crate::error::TriviumError::ReadOnlyViolation { .. } => "TDB_READ_ONLY",
            crate::error::TriviumError::RecoveryRequired { .. } => "TDB_RECOVERY_REQUIRED",
            crate::error::TriviumError::ImmutableArtifactInvalid { .. } => "TDB_IMMUTABLE_ARTIFACT",
            crate::error::TriviumError::GenerationBusy { .. } => "TDB_GENERATION_BUSY",
            crate::error::TriviumError::UnsupportedDatabaseVersion { .. } => {
                "TDB_UNSUPPORTED_DATABASE_VERSION"
            }
            crate::error::TriviumError::UnsupportedWalVersion { .. } => {
                "TDB_UNSUPPORTED_WAL_VERSION"
            }
            crate::error::TriviumError::UnsupportedSidecarVersion { .. } => {
                "TDB_UNSUPPORTED_SIDECAR_VERSION"
            }
            crate::error::TriviumError::ApiMigrationRequired { .. } => "TDB_API_MIGRATION_REQUIRED",
            crate::error::TriviumError::QueryParse(_) => "TDB_QUERY_PARSE",
            crate::error::TriviumError::QueryExecution(_) => "TDB_QUERY_EXECUTION",
            crate::error::TriviumError::DimensionMismatch { .. } => "TDB_DIMENSION_MISMATCH",
            crate::error::TriviumError::NodeNotFound(_) => "TDB_NODE_NOT_FOUND",
            crate::error::TriviumError::InvalidInput(_) => "TDB_INVALID_INPUT",
            _ => "TDB_ERROR",
        };
        napi::Error::new(napi::Status::GenericFailure, format!("{code}: {error}"))
    }

    fn node_tql_row_to_json<T: crate::VectorType>(
        row: std::collections::HashMap<String, crate::query::tql_executor::TqlValue<T>>,
    ) -> serde_json::Value {
        use crate::query::tql_executor::TqlValue;
        let mut obj = serde_json::Map::new();
        for (name, value) in row {
            let value = match value {
                TqlValue::Node(node) => serde_json::json!({
                    "id": node.id.to_string(),
                    "payload": node.payload,
                    "numEdges": node.edges.len(),
                }),
                TqlValue::Int(value) => serde_json::json!(value),
                TqlValue::Float(value) => serde_json::json!(value),
                TqlValue::String(value) => serde_json::json!(value),
                TqlValue::Bool(value) => serde_json::json!(value),
                TqlValue::Path(value) => serde_json::json!(
                    value
                        .into_iter()
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>()
                ),
                TqlValue::List(value) => serde_json::Value::Array(value),
                TqlValue::Null => serde_json::Value::Null,
            };
            obj.insert(name, value);
        }
        serde_json::Value::Object(obj)
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
        type Output = Vec<Vec<crate::node::SearchHit>>;
        type JsValue = Vec<Vec<JsSearchHit>>;

        fn compute(&mut self) -> napi::Result<Self::Output> {
            match &self.backend {
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
                        .map(|query| query.iter().map(|value| *value as u64).collect())
                        .collect::<Vec<Vec<u64>>>();
                    db.search_batch(&queries, &self.search_config, &self.batch_config)
                }
            }
            .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        fn resolve(
            &mut self,
            _env: napi::Env,
            output: Self::Output,
        ) -> napi::Result<Self::JsValue> {
            Ok(output
                .into_iter()
                .map(|hits| {
                    hits.into_iter()
                        .map(|hit| JsSearchHit {
                            id: hit.id as f64,
                            score: hit.score as f64,
                            payload: hit.payload,
                        })
                        .collect()
                })
                .collect())
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
        pub payload: serde_json::Value,
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
        fn 边权重_api舍入到六位小数() {
            assert_eq!(round_api_f32(0.9f32), 0.9);
            assert_eq!(round_api_f32(0.12345678f32), 0.123457);
        }
    }

    fn search_hit_to_js(hit: crate::node::SearchHit) -> JsSearchHit {
        JsSearchHit {
            id: hit.id as f64,
            score: hit.score as f64,
            payload: hit.payload,
        }
    }

    #[napi(object)]
    pub struct JsReachabilityStep {
        pub from: f64,
        pub to: f64,
        pub label: String,
        pub weight: f64,
        pub metadata: serde_json::Value,
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
        pub payload: serde_json::Value,
    }

    #[napi(object)]
    pub struct JsSubgraphEdge {
        pub source_id: f64,
        pub target_id: f64,
        pub label: String,
        pub weight: f64,
        pub metadata: serde_json::Value,
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
        pub min_depth: Option<u32>,
        pub max_depth: Option<u32>,
        pub labels: Option<Vec<String>>,
        pub direction: Option<String>,
        pub max_visited_nodes: Option<f64>,
        pub max_results: Option<f64>,
        pub max_edges: Option<f64>,
    }

    /// 数据库打开与容量规划配置。
    #[napi(object)]
    pub struct JsDatabaseOptions {
        pub dim: Option<u32>,
        pub dtype: Option<String>,
        pub sync_mode: Option<String>,
        pub storage_mode: Option<String>,
        pub auto_build_quiver: Option<bool>,
        pub load_text_index: Option<bool>,
        pub expected_nodes: Option<f64>,
        pub memory_limit_mb: Option<f64>,
        pub access_mode: Option<String>,
        pub missing_index_policy: Option<String>,
    }

    /// 高级管线专用配置结构
    #[napi(object)]
    pub struct JsSearchConfig {
        /// top_k 用 i64 承载：u32 会把 JS 负数无符号化成超大值（静默返回全库）
        pub top_k: Option<i64>,
        pub recall_k: Option<i64>,
        pub rerank_k: Option<i64>,
        pub expand_depth: Option<u32>,
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
        pub payload_filter: Option<serde_json::Value>,
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
        pub metadata: serde_json::Value,
    }

    /// 完整入边视图
    #[napi(object)]
    pub struct JsIncomingEdge {
        pub source_id: f64,
        pub target_id: f64,
        pub label: String,
        pub weight: f64,
        pub metadata: serde_json::Value,
    }

    /// 节点完整视图
    #[napi(object)]
    pub struct JsNodeView {
        pub id: f64,
        pub vector: Vec<f64>,
        pub payload: serde_json::Value,
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
        pub min_community_size: Option<u32>,
        /// 最大迭代轮次 (默认 15)
        pub max_iterations: Option<u32>,
        /// 是否计算质心 (默认 true)
        pub with_centroids: Option<bool>,
    }

    /// Hook 管线执行上下文（包含各阶段计时统计和自定义数据）
    #[napi(object)]
    pub struct JsHookContext {
        /// 各管线阶段的耗时统计（JSON 对象, 单位: 毫秒）
        pub timings: serde_json::Value,
        /// 每阶段候选数量
        pub counts: serde_json::Value,
        /// Hook 注入的自定义数据
        pub custom_data: serde_json::Value,
        pub observations: serde_json::Value,
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

    fn parse_safe_usize(value: f64, name: &str) -> napi::Result<usize> {
        if !value.is_finite()
            || value < 0.0
            || value.fract() != 0.0
            || value > 9_007_199_254_740_991.0
            || value > usize::MAX as f64
        {
            return Err(napi::Error::from_reason(format!(
                "{name} 必须是 JavaScript 安全范围内的非负整数"
            )));
        }
        Ok(value as usize)
    }

    fn parse_reachability_options(
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
            min_depth: options.min_depth.unwrap_or(1) as usize,
            max_depth: options.max_depth.unwrap_or(1) as usize,
            labels: options.labels,
            direction,
            max_visited_nodes: options
                .max_visited_nodes
                .map(|value| parse_safe_usize(value, "maxVisitedNodes"))
                .transpose()?
                .unwrap_or(10_000),
            max_results: options
                .max_results
                .map(|value| parse_safe_usize(value, "maxResults"))
                .transpose()?
                .unwrap_or(10_000),
            max_edges: options
                .max_edges
                .map(|value| parse_safe_usize(value, "maxEdges"))
                .transpose()?
                .unwrap_or(50_000),
            max_frontier_size: 10_000,
            exhaustion_policy: crate::graph::budget::BudgetExhaustionPolicy::Partial,
        })
    }

    fn build_transaction<T: crate::VectorType>(
        operations: &[serde_json::Value],
        convert: fn(f64) -> T,
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
                Ok(parse_safe_usize(value, name)? as u64)
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
                            .map(convert)
                            .ok_or_else(|| napi::Error::from_reason("vector 必须是数字数组"))
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
        result: crate::graph::reachability::ReachabilityResult,
    ) -> JsReachabilityResult {
        JsReachabilityResult {
            source_id: result.source_id as f64,
            target_id: result.target_id as f64,
            depth: result.depth as u32,
            path: result.path.into_iter().map(|id| id as f64).collect(),
            steps: result
                .steps
                .into_iter()
                .map(|step| JsReachabilityStep {
                    from: step.from as f64,
                    to: step.to as f64,
                    label: step.label,
                    weight: round_api_f32(step.weight),
                    metadata: step.metadata,
                })
                .collect(),
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

    #[napi]
    impl TriviumDB {
        /// 打开或创建数据库
        ///
        /// ```js
        /// const db = new TriviumDB("data.tdb", { dim: 1536, dtype: "f32", syncMode: "normal" })
        /// ```
        #[napi(constructor)]
        pub fn new(
            path: String,
            options: Option<napi::bindgen_prelude::Either<u32, JsDatabaseOptions>>,
        ) -> napi::Result<Self> {
            let options = match options {
                Some(napi::bindgen_prelude::Either::A(_)) => {
                    return Err(napi::Error::from_reason(
                        "TDB_API_MIGRATION_REQUIRED: 数字位置参数已移除，请使用 options 对象 (numeric positional arguments were removed; use an options object)",
                    ));
                }
                Some(napi::bindgen_prelude::Either::B(options)) => options,
                None => JsDatabaseOptions {
                    dim: None,
                    dtype: None,
                    sync_mode: None,
                    storage_mode: None,
                    auto_build_quiver: None,
                    load_text_index: None,
                    expected_nodes: None,
                    memory_limit_mb: None,
                    access_mode: None,
                    missing_index_policy: None,
                },
            };
            let dim = options.dim.unwrap_or(1536) as usize;
            let dtype = options.dtype.unwrap_or_else(|| "f32".into());
            let expected_nodes = options
                .expected_nodes
                .map(|value| parse_safe_usize(value, "expectedNodes"))
                .transpose()?;
            let memory_limit_mb = options
                .memory_limit_mb
                .map(|value| parse_safe_usize(value, "memoryLimitMb"))
                .transpose()?
                .unwrap_or(0);
            let memory_limit = memory_limit_mb
                .checked_mul(1024 * 1024)
                .ok_or_else(|| napi::Error::from_reason("memoryLimitMb 换算字节时溢出"))?;
            let config = crate::database::Config {
                dim,
                sync_mode: parse_sync_mode(options.sync_mode.as_deref().unwrap_or("normal"))?,
                storage_mode: parse_storage_mode(options.storage_mode.as_deref())?,
                auto_build_quiver: options.auto_build_quiver.unwrap_or(true),
                load_text_index: options.load_text_index.unwrap_or(false),
                expected_nodes,
                memory_limit,
                access_mode: parse_access_mode(options.access_mode.as_deref())?,
                missing_index_policy: parse_missing_index_policy(
                    options.missing_index_policy.as_deref(),
                )?,
            };
            let dtype_str = dtype.as_str();

            let inner = match dtype_str {
                "f32" => DbBackend::F32(
                    GenericDatabase::<f32>::open_with_config(&path, config)
                        .map_err(to_napi_error)?,
                ),
                "f16" => DbBackend::F16(
                    GenericDatabase::<half::f16>::open_with_config(&path, config)
                        .map_err(to_napi_error)?,
                ),
                "u64" => DbBackend::U64(
                    GenericDatabase::<u64>::open_with_config(&path, config)
                        .map_err(to_napi_error)?,
                ),
                _ => return Err(napi::Error::from_reason("dtype 必须是 f32 / f16 / u64")),
            };
            Ok(Self {
                inner,
                dtype: dtype_str.to_string(),
            })
        }

        // ── Hook 管理 ──

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
        pub fn load_ffi_hook(&mut self, lib_path: String) -> napi::Result<()> {
            let ffi_hook = crate::hook::FfiHook::load(&lib_path)
                .map_err(|e| napi::Error::from_reason(format!("加载 FFI Hook 失败: {}", e)))?;
            dispatch!(self, mut db => db.set_hook(ffi_hook));
            Ok(())
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

            let top_k = cfg.top_k.unwrap_or(5);
            if top_k <= 0 {
                return Err(napi::Error::from_reason(format!(
                    "top_k 必须为正整数，收到 {top_k}"
                )));
            }
            let payload_filter = parse_payload_filter(cfg.payload_filter.take())?;
            let core_config = crate::database::SearchConfig {
                top_k: top_k as usize,
                recall_k: cfg.recall_k.unwrap_or(0).max(0) as usize,
                rerank_k: cfg.rerank_k.unwrap_or(0).max(0) as usize,
                expand_depth: cfg.expand_depth.unwrap_or(2) as usize,
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
                    .map(|value| parse_safe_usize(value, "maxEdgesPerNode"))
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
                    let v: Vec<u64> = query_vector.iter().map(|&x| x as u64).collect();
                    db.search_hybrid_with_context(q_text, Some(&v), &core_config)
                }
            }
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

            // 转换 hits
            let hits: Vec<JsSearchHit> = results
                .into_iter()
                .map(|h| JsSearchHit {
                    id: h.id as f64,
                    score: h.score as f64,
                    payload: h.payload,
                })
                .collect();

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
                timings: serde_json::Value::Object(timings_map),
                counts: serde_json::Value::Object(counts),
                custom_data: hook_ctx.custom_data,
                observations: serde_json::Value::Object(
                    hook_ctx
                        .observations
                        .iter()
                        .map(|(name, value)| (name.clone(), serde_json::json!(value)))
                        .collect(),
                ),
                aborted: hook_ctx.abort,
            };

            Ok(JsSearchWithContextResult { hits, context })
        }

        // ── CRUD ──

        /// 插入节点，返回新节点 ID
        #[napi]
        pub fn insert(
            &mut self,
            vector: Vec<f64>,
            payload: serde_json::Value,
        ) -> napi::Result<f64> {
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    db.insert(&v, payload)
                        .map(|id| id as f64)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> =
                        vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.insert(&v, payload)
                        .map(|id| id as f64)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = vector.iter().map(|&x| x as u64).collect();
                    db.insert(&v, payload)
                        .map(|id| id as f64)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
            }
        }

        /// 批量插入节点，返回新分配的 ID 列表
        #[napi]
        pub fn batch_insert(
            &mut self,
            vectors: Vec<Vec<f64>>,
            payloads: Vec<serde_json::Value>,
        ) -> napi::Result<Vec<f64>> {
            if vectors.len() != payloads.len() {
                return Err(napi::Error::from_reason("向量列表与负载列表长度不一致"));
            }
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
                        .map(|ids| ids.into_iter().map(|id| id as f64).collect())
                        .map_err(|error| napi::Error::from_reason(error.to_string()))
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
                        .map(|ids| ids.into_iter().map(|id| id as f64).collect())
                        .map_err(|error| napi::Error::from_reason(error.to_string()))
                }
                DbBackend::U64(db) => {
                    let converted: Vec<Vec<u64>> = vectors
                        .into_iter()
                        .map(|vector| vector.into_iter().map(|value| value as u64).collect())
                        .collect();
                    let mut tx = crate::database::TxBuilder::new();
                    for (vector, payload) in converted.iter().zip(payloads) {
                        tx.insert(vector, payload);
                    }
                    db.commit_tx(tx)
                        .map(|ids| ids.into_iter().map(|id| id as f64).collect())
                        .map_err(|error| napi::Error::from_reason(error.to_string()))
                }
            }
        }

        /// 批量插入指定 ID 的节点
        #[napi]
        pub fn batch_insert_with_ids(
            &mut self,
            ids: Vec<f64>,
            vectors: Vec<Vec<f64>>,
            payloads: Vec<serde_json::Value>,
        ) -> napi::Result<()> {
            if ids.len() != vectors.len() || vectors.len() != payloads.len() {
                return Err(napi::Error::from_reason("ID、向量与负载列表长度不一致"));
            }
            let parsed_ids: Vec<u64> = ids
                .into_iter()
                .map(|id| parse_safe_usize(id, "id").map(|value| value as u64))
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
                        .map_err(|error| napi::Error::from_reason(error.to_string()))
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
                        .map_err(|error| napi::Error::from_reason(error.to_string()))
                }
                DbBackend::U64(db) => {
                    let converted: Vec<Vec<u64>> = vectors
                        .into_iter()
                        .map(|vector| vector.into_iter().map(|value| value as u64).collect())
                        .collect();
                    let mut tx = crate::database::TxBuilder::new();
                    for ((id, vector), payload) in
                        parsed_ids.iter().zip(converted.iter()).zip(payloads)
                    {
                        tx.insert_with_id(*id, vector, payload);
                    }
                    db.commit_tx(tx)
                        .map(|_| ())
                        .map_err(|error| napi::Error::from_reason(error.to_string()))
                }
            }
        }

        /// 带指定 ID 插入节点
        #[napi]
        pub fn insert_with_id(
            &mut self,
            id: f64,
            vector: Vec<f64>,
            payload: serde_json::Value,
        ) -> napi::Result<()> {
            let id = parse_safe_usize(id, "id")? as u64;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    db.insert_with_id(id, &v, payload)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> =
                        vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.insert_with_id(id, &v, payload)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = vector.iter().map(|&x| x as u64).collect();
                    db.insert_with_id(id, &v, payload)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
            }
        }

        /// 使用自定义 ID 插入或覆盖节点。
        #[napi]
        pub fn upsert_with_id(
            &mut self,
            id: f64,
            vector: Vec<f64>,
            payload: serde_json::Value,
        ) -> napi::Result<()> {
            let id = parse_safe_usize(id, "id")? as u64;
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
                    let vector = vector
                        .into_iter()
                        .map(|value| value as u64)
                        .collect::<Vec<_>>();
                    db.upsert_with_id(id, &vector, payload)
                }
            }
            .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        /// 按 ID 获取节点，不存在时返回 null
        #[napi]
        pub fn commit_transaction(
            &mut self,
            operations: Vec<serde_json::Value>,
        ) -> napi::Result<Vec<f64>> {
            let ids = match &mut self.inner {
                DbBackend::F32(db) => {
                    db.commit_tx(build_transaction(&operations, |value| value as f32)?)
                }
                DbBackend::F16(db) => {
                    db.commit_tx(build_transaction(&operations, half::f16::from_f64)?)
                }
                DbBackend::U64(db) => {
                    db.commit_tx(build_transaction(&operations, |value| value as u64)?)
                }
            }
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            Ok(ids.into_iter().map(|id| id as f64).collect())
        }

        #[napi]
        pub fn graph_stats(&self) -> napi::Result<serde_json::Value> {
            serde_json::to_value(dispatch!(self, db => db.graph_stats()))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        #[napi]
        pub fn validate_graph(&self) -> napi::Result<serde_json::Value> {
            serde_json::to_value(dispatch!(self, db => db.validate_graph()))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        #[napi]
        pub fn repair_graph_indexes(&mut self) -> napi::Result<serde_json::Value> {
            let report = dispatch!(self, mut db => db.repair_graph_indexes())
                .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            serde_json::to_value(report)
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        #[napi]
        pub fn get(&self, id: f64) -> Option<JsNodeView> {
            let id = id as u64;
            match &self.inner {
                DbBackend::F32(db) => db.get(id).map(|n| {
                    let num_edges = n.edges.len() as u32;
                    let edges_arr = n
                        .edges
                        .into_iter()
                        .map(|e| JsEdge {
                            target_id: e.target_id as f64,
                            label: e.label.clone(),
                            weight: round_api_f32(e.weight),
                            metadata: e.metadata.clone(),
                        })
                        .collect();
                    JsNodeView {
                        id: n.id as f64,
                        vector: n.vector.iter().map(|&x| x as f64).collect(),
                        payload: n.payload,
                        edges: edges_arr,
                        num_edges,
                    }
                }),
                DbBackend::F16(db) => db.get(id).map(|n| {
                    let num_edges = n.edges.len() as u32;
                    let edges_arr = n
                        .edges
                        .into_iter()
                        .map(|e| JsEdge {
                            target_id: e.target_id as f64,
                            label: e.label.clone(),
                            weight: round_api_f32(e.weight),
                            metadata: e.metadata.clone(),
                        })
                        .collect();
                    JsNodeView {
                        id: n.id as f64,
                        vector: n.vector.iter().map(|x| x.to_f64()).collect(),
                        payload: n.payload,
                        edges: edges_arr,
                        num_edges,
                    }
                }),
                DbBackend::U64(db) => db.get(id).map(|n| {
                    let num_edges = n.edges.len() as u32;
                    let edges_arr = n
                        .edges
                        .into_iter()
                        .map(|e| JsEdge {
                            target_id: e.target_id as f64,
                            label: e.label.clone(),
                            weight: round_api_f32(e.weight),
                            metadata: e.metadata.clone(),
                        })
                        .collect();
                    JsNodeView {
                        id: n.id as f64,
                        vector: n.vector.iter().map(|&x| x as f64).collect(),
                        payload: n.payload,
                        edges: edges_arr,
                        num_edges,
                    }
                }),
            }
        }

        /// 更新节点元数据
        #[napi]
        pub fn update_payload(&mut self, id: f64, payload: serde_json::Value) -> napi::Result<()> {
            dispatch!(self, mut db => db.update_payload(id as u64, payload))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
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
        pub fn patch_payload(&mut self, id: f64, patch: serde_json::Value) -> napi::Result<()> {
            dispatch!(self, mut db => db.patch_payload(id as u64, patch))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 更新节点向量
        #[napi]
        pub fn update_vector(&mut self, id: f64, vector: Vec<f64>) -> napi::Result<()> {
            let id = id as u64;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    db.update_vector(id, &v)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> =
                        vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.update_vector(id, &v)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = vector.iter().map(|&x| x as u64).collect();
                    db.update_vector(id, &v)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
            }
        }

        /// 删除节点（三层原子联删：向量 + Payload + 所有关联边）
        #[napi]
        pub fn delete(&mut self, id: f64) -> napi::Result<()> {
            dispatch!(self, mut db => db.delete(id as u64))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        // ── 图谱操作 ──

        /// 建立有向带权边
        #[napi]
        pub fn link(
            &mut self,
            src: f64,
            dst: f64,
            label: Option<String>,
            weight: Option<f64>,
        ) -> napi::Result<()> {
            let label = label.as_deref().unwrap_or("related");
            let weight = weight.unwrap_or(1.0) as f32;
            dispatch!(self, mut db => db.link(src as u64, dst as u64, label, weight))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        #[napi]
        pub fn get_edge(&self, src: f64, dst: f64, label: String) -> Option<JsEdge> {
            dispatch!(self, db => db.get_edge(src as u64, dst as u64, &label)).map(|edge| JsEdge {
                target_id: edge.target_id as f64,
                label: edge.label,
                weight: round_api_f32(edge.weight),
                metadata: edge.metadata,
            })
        }

        #[napi]
        pub fn upsert_edge(
            &mut self,
            src: f64,
            dst: f64,
            label: String,
            weight: f64,
            metadata: Option<serde_json::Value>,
        ) -> napi::Result<()> {
            dispatch!(self, mut db => db.upsert_edge(
                src as u64,
                dst as u64,
                &label,
                weight as f32,
                metadata.unwrap_or(serde_json::Value::Null),
            ))
            .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        #[napi]
        pub fn update_edge(
            &mut self,
            src: f64,
            dst: f64,
            label: String,
            weight: Option<f64>,
            metadata: Option<serde_json::Value>,
        ) -> napi::Result<()> {
            dispatch!(self, mut db => db.update_edge(
                src as u64,
                dst as u64,
                &label,
                weight.map(|value| value as f32),
                metadata,
            ))
            .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        /// 断开两节点间的边；提供 label 时仅删除该标签。
        #[napi]
        pub fn unlink(&mut self, src: f64, dst: f64, label: Option<String>) -> napi::Result<()> {
            match label {
                Some(label) => {
                    dispatch!(self, mut db => db.unlink_label(src as u64, dst as u64, &label))
                }
                None => dispatch!(self, mut db => db.unlink(src as u64, dst as u64)),
            }
            .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 获取 N 跳邻居节点 ID 列表，可按标签白名单遍历。
        #[napi]
        pub fn neighbors(
            &self,
            id: f64,
            depth: Option<u32>,
            labels: Option<Vec<String>>,
        ) -> Vec<f64> {
            let depth = depth.unwrap_or(1) as usize;
            dispatch!(self, db => db.neighbors_with_labels(id as u64, depth, labels.as_deref()))
                .into_iter()
                .map(|id| id as f64)
                .collect()
        }

        #[napi]
        pub fn reachable(
            &self,
            id: f64,
            options: Option<JsReachabilityOptions>,
        ) -> napi::Result<Vec<JsReachabilityResult>> {
            let config = parse_reachability_options(options)?;
            dispatch!(self, db => db.reachable(id as u64, &config))
                .map(|results| results.into_iter().map(to_js_reachability).collect())
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        #[napi]
        pub fn reachable_detailed(
            &self,
            id: f64,
            options: Option<JsReachabilityOptions>,
        ) -> napi::Result<JsReachabilityOutput> {
            let config = parse_reachability_options(options)?;
            let output = dispatch!(self, db => db.reachable_detailed(id as u64, &config))
                .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            Ok(JsReachabilityOutput {
                results: output.results.into_iter().map(to_js_reachability).collect(),
                visited_nodes: output.visited_nodes as f64,
                traversed_edges: output.traversed_edges as f64,
                truncated: output.truncated,
            })
        }

        #[napi]
        pub fn query_subgraph(
            &self,
            id: f64,
            options: Option<JsReachabilityOptions>,
        ) -> napi::Result<JsSubgraphResult> {
            let config = parse_reachability_options(options)?;
            let output = dispatch!(self, db => db.query_subgraph(id as u64, &config))
                .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            Ok(JsSubgraphResult {
                nodes: output
                    .nodes
                    .into_iter()
                    .map(|node| JsSubgraphNode {
                        id: node.id as f64,
                        payload: node.payload,
                    })
                    .collect(),
                edges: output
                    .edges
                    .into_iter()
                    .map(|edge| JsSubgraphEdge {
                        source_id: edge.source_id as f64,
                        target_id: edge.target_id as f64,
                        label: edge.label,
                        weight: round_api_f32(edge.weight),
                        metadata: edge.metadata,
                    })
                    .collect(),
                visited_nodes: output.visited_nodes as f64,
                traversed_edges: output.traversed_edges as f64,
                truncated: output.truncated,
            })
        }

        #[napi]
        pub fn search_graph_first(
            &self,
            query_vector: Vec<f64>,
            anchor_ids: Vec<f64>,
            top_k: u32,
            max_anchor_nodes: Option<f64>,
        ) -> napi::Result<Vec<JsSearchHit>> {
            let anchors: Vec<u64> = anchor_ids
                .into_iter()
                .map(|id| parse_safe_usize(id, "anchorId").map(|id| id as u64))
                .collect::<napi::Result<_>>()?;
            let max_anchor_nodes = max_anchor_nodes
                .map(|value| parse_safe_usize(value, "maxAnchorNodes"))
                .transpose()?
                .unwrap_or(100_000);
            let hits = match &self.inner {
                DbBackend::F32(db) => {
                    let query: Vec<f32> = query_vector.iter().map(|value| *value as f32).collect();
                    db.search_graph_first(&query, &anchors, top_k as usize, max_anchor_nodes)
                }
                DbBackend::F16(db) => {
                    let query: Vec<half::f16> = query_vector
                        .iter()
                        .map(|value| half::f16::from_f64(*value))
                        .collect();
                    db.search_graph_first(&query, &anchors, top_k as usize, max_anchor_nodes)
                }
                DbBackend::U64(db) => {
                    let query: Vec<u64> = query_vector.iter().map(|value| *value as u64).collect();
                    db.search_graph_first(&query, &anchors, top_k as usize, max_anchor_nodes)
                }
            }
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            Ok(hits
                .into_iter()
                .map(|hit| JsSearchHit {
                    id: hit.id as f64,
                    score: hit.score as f64,
                    payload: hit.payload,
                })
                .collect())
        }

        #[napi]
        pub fn search_exact(
            &self,
            query_vector: Vec<f64>,
            top_k: u32,
        ) -> napi::Result<Vec<JsSearchHit>> {
            let hits = match &self.inner {
                DbBackend::F32(db) => {
                    let query: Vec<f32> = query_vector.iter().map(|value| *value as f32).collect();
                    db.search_exact(&query, top_k as usize)
                }
                DbBackend::F16(db) => {
                    let query: Vec<half::f16> = query_vector
                        .iter()
                        .map(|value| half::f16::from_f64(*value))
                        .collect();
                    db.search_exact(&query, top_k as usize)
                }
                DbBackend::U64(db) => {
                    let query: Vec<u64> = query_vector.iter().map(|value| *value as u64).collect();
                    db.search_exact(&query, top_k as usize)
                }
            }
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            Ok(hits
                .into_iter()
                .map(|hit| JsSearchHit {
                    id: hit.id as f64,
                    score: hit.score as f64,
                    payload: hit.payload,
                })
                .collect())
        }

        // ── 社区聚类 ──

        /// 基于物理记忆图谱进行 Leiden 社区发现
        ///
        /// **无锁设计**: 短暂持锁快照邻接表后立即释放，聚类在锁外计算。
        /// 调用期间数据库仍可正常读写。
        #[napi]
        pub fn leiden_cluster(
            &self,
            config: Option<JsLeidenConfig>,
        ) -> napi::Result<JsClusterResult> {
            let cfg = config.unwrap_or(JsLeidenConfig {
                min_community_size: None,
                max_iterations: None,
                with_centroids: None,
            });
            let min_c = cfg.min_community_size.unwrap_or(3) as usize;
            let max_iter = cfg.max_iterations.map(|v| v as usize);
            let with_cent = cfg.with_centroids;

            let result = dispatch!(self, db => db.leiden_cluster(min_c, max_iter, with_cent))
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;

            // 排序确保确定性输出
            let mut sorted_nodes: Vec<_> = result.node_to_cluster.into_iter().collect();
            sorted_nodes.sort_by_key(|&(id, _)| id);

            let mut node_to_cluster = Vec::with_capacity(sorted_nodes.len() * 2);
            for (n, c) in sorted_nodes {
                node_to_cluster.push(n as f64);
                node_to_cluster.push(c as f64);
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
                centroids.push(c as f64);
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
            query_vector: Vec<f64>,
            top_k: Option<i64>,
            expand_depth: Option<u32>,
            min_score: Option<f64>,
            payload_filter: Option<serde_json::Value>,
        ) -> napi::Result<Vec<JsSearchHit>> {
            let top_k = top_k.unwrap_or(5);
            if top_k <= 0 {
                return Err(napi::Error::from_reason(format!(
                    "top_k 必须为正整数，收到 {top_k}"
                )));
            }
            let top_k = top_k as usize;
            let expand_depth = expand_depth.unwrap_or(0) as usize;
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
                    let v: Vec<u64> = query_vector.iter().map(|&x| x as u64).collect();
                    db.search_hybrid(None, Some(&v), &search_config)
                }
            }
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

            Ok(hits
                .into_iter()
                .map(|h| JsSearchHit {
                    id: h.id as f64,
                    score: h.score as f64,
                    payload: h.payload,
                })
                .collect())
        }

        #[napi]
        pub fn search_grouped(
            &self,
            query_vector: Vec<f64>,
            top_k: Option<i64>,
            expand_depth: Option<u32>,
            min_score: Option<f64>,
            payload_filter: Option<serde_json::Value>,
        ) -> napi::Result<JsGroupedSearchResult> {
            let top_k = top_k.unwrap_or(5);
            if top_k <= 0 {
                return Err(napi::Error::from_reason(format!(
                    "top_k 必须为正整数，收到 {top_k}"
                )));
            }
            let payload_filter = parse_payload_filter(payload_filter)?;
            let config = crate::database::SearchConfig {
                top_k: top_k as usize,
                expand_depth: expand_depth.unwrap_or(2) as usize,
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
                    let vector: Vec<u64> = query_vector.iter().map(|&value| value as u64).collect();
                    db.search_hybrid_grouped(None, Some(&vector), &config)
                }
            }
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            Ok(JsGroupedSearchResult {
                semantic_hits: result
                    .semantic_hits
                    .into_iter()
                    .map(search_hit_to_js)
                    .collect(),
                graph_hits: result
                    .graph_hits
                    .into_iter()
                    .map(search_hit_to_js)
                    .collect(),
            })
        }

        #[napi]
        pub fn search_batch(
            &self,
            query_vectors: Vec<Vec<f64>>,
            top_k: Option<i64>,
            parallelism: Option<u32>,
            min_score: Option<f64>,
        ) -> napi::Result<napi::bindgen_prelude::AsyncTask<BatchSearchTask>> {
            let top_k = top_k.unwrap_or(5);
            if top_k <= 0 {
                return Err(napi::Error::from_reason(format!(
                    "top_k 必须为正整数，收到 {top_k}"
                )));
            }
            let backend = match &self.inner {
                DbBackend::F32(db) => SearchBackendHandle::F32(db.search_handle()),
                DbBackend::F16(db) => SearchBackendHandle::F16(db.search_handle()),
                DbBackend::U64(db) => SearchBackendHandle::U64(db.search_handle()),
            };
            Ok(napi::bindgen_prelude::AsyncTask::new(BatchSearchTask {
                backend,
                queries: query_vectors,
                search_config: crate::database::SearchConfig {
                    top_k: top_k as usize,
                    expand_depth: 0,
                    min_score: min_score.unwrap_or(0.5) as f32,
                    enable_advanced_pipeline: false,
                    ..Default::default()
                },
                batch_config: crate::database::BatchSearchConfig {
                    parallelism: parallelism.unwrap_or(0) as usize,
                },
            }))
        }

        /// 认知检索引擎：完全参数化暴露的高级功能 (FISTA, DPP, SA-PPR)
        #[napi]
        pub fn search_advanced(
            &self,
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

            let top_k = cfg.top_k.unwrap_or(5);
            if top_k <= 0 {
                return Err(napi::Error::from_reason(format!(
                    "top_k 必须为正整数，收到 {top_k}"
                )));
            }
            let payload_filter = parse_payload_filter(cfg.payload_filter.take())?;
            let core_config = crate::database::SearchConfig {
                top_k: top_k as usize,
                recall_k: cfg.recall_k.unwrap_or(0).max(0) as usize,
                rerank_k: cfg.rerank_k.unwrap_or(0).max(0) as usize,
                expand_depth: cfg.expand_depth.unwrap_or(2) as usize,
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
                    .map(|value| parse_safe_usize(value, "maxEdgesPerNode"))
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
                    let v: Vec<u64> = query_vector.iter().map(|&x| x as u64).collect();
                    db.search_hybrid(q_text, Some(&v), &core_config)
                }
            }
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

            Ok(hits
                .into_iter()
                .map(|h| JsSearchHit {
                    id: h.id as f64,
                    score: h.score as f64,
                    payload: h.payload,
                })
                .collect())
        }

        /// 混合检索增强入口：带图扩散的双路检索
        #[napi]
        pub fn search_hybrid(
            &self,
            query_vector: Vec<f64>,
            query_text: String,
            top_k: Option<i64>,
            expand_depth: Option<u32>,
            min_score: Option<f64>,
            hybrid_alpha: Option<f64>,
            payload_filter: Option<serde_json::Value>,
        ) -> napi::Result<Vec<JsSearchHit>> {
            let top_k = top_k.unwrap_or(5);
            if top_k <= 0 {
                return Err(napi::Error::from_reason(format!(
                    "top_k 必须为正整数，收到 {top_k}"
                )));
            }
            let top_k = top_k as usize;
            let expand_depth = expand_depth.unwrap_or(2) as usize;
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
                    let v: Vec<u64> = query_vector.iter().map(|&x| x as u64).collect();
                    db.search_hybrid(Some(&query_text), Some(&v), &core_config)
                }
            }
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

            Ok(hits
                .into_iter()
                .map(|h| JsSearchHit {
                    id: h.id as f64,
                    score: h.score as f64,
                    payload: h.payload,
                })
                .collect())
        }

        // ── 文本索引 ──

        /// 对节点建立用于双路召回的长文本 BM25 索引
        #[napi]
        pub fn index_text(&mut self, id: f64, text: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.index_text(id as u64, &text))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 对节点建立用于精确命中的 AC自动机 高级关键词索引
        #[napi]
        pub fn index_keyword(&mut self, id: f64, keyword: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.index_keyword(id as u64, &keyword))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 在批量插入或重启后必须调用，用于重编译自动机与词频
        #[napi]
        pub fn build_text_index(&mut self) {
            let _ = dispatch!(self, mut db => db.build_text_index());
        }

        // ── 属性二级索引 ──

        /// 创建属性索引：对指定 payload 字段建立倒排索引
        ///
        /// ```js
        /// db.createIndex('name')   // 之后 tql('FIND {name: "Alice"} RETURN *') 使用 O(1) 索引
        /// ```
        #[napi]
        pub fn create_index(&mut self, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.create_index(&field)).map_err(to_napi_error)
        }

        #[napi]
        pub fn create_ordered_index(&mut self, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.create_ordered_index(&field)).map_err(to_napi_error)
        }

        #[napi]
        pub fn create_composite_index(&mut self, fields: Vec<String>) -> napi::Result<()> {
            dispatch!(self, mut db => db.create_composite_index(&fields)).map_err(to_napi_error)
        }

        #[napi]
        pub fn create_bitmap_index(&mut self, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.create_bitmap_index(&field)).map_err(to_napi_error)
        }

        /// 删除属性索引（查询仍可用，退化为全扫描）
        #[napi]
        pub fn drop_index(&mut self, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.drop_index(&field)).map_err(to_napi_error)
        }

        #[napi]
        pub fn drop_ordered_index(&mut self, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.drop_ordered_index(&field)).map_err(to_napi_error)
        }

        #[napi]
        pub fn drop_composite_index(&mut self, fields: Vec<String>) -> napi::Result<()> {
            dispatch!(self, mut db => db.drop_composite_index(&fields)).map_err(to_napi_error)
        }

        #[napi]
        pub fn drop_bitmap_index(&mut self, field: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.drop_bitmap_index(&field)).map_err(to_napi_error)
        }

        #[napi]
        pub fn index_info(&self) -> napi::Result<serde_json::Value> {
            dispatch!(self, db => serde_json::to_value(db.index_info()))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        #[napi]
        pub fn storage_info(&self) -> napi::Result<serde_json::Value> {
            dispatch!(self, db => serde_json::to_value(db.storage_info()))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        // ── 轻量级单字段查询 ──

        /// 获取节点的 payload（不含向量，比 get() 更轻量）
        #[napi]
        pub fn get_payload(&self, id: f64) -> Option<serde_json::Value> {
            dispatch!(self, db => db.get_payload(id as u64))
        }

        /// 获取节点的出边列表
        #[napi]
        pub fn get_edges(&self, id: f64) -> Vec<JsEdge> {
            dispatch!(self, db => db.get_edges(id as u64))
                .into_iter()
                .map(|e| JsEdge {
                    target_id: e.target_id as f64,
                    label: e.label,
                    weight: round_api_f32(e.weight),
                    metadata: e.metadata,
                })
                .collect()
        }

        /// 获取节点完整入边，可按标签过滤。
        #[napi]
        pub fn get_incoming_edges(&self, id: f64, label: Option<String>) -> Vec<JsIncomingEdge> {
            dispatch!(self, db => db.get_incoming_edges(id as u64, label.as_deref()))
                .into_iter()
                .map(|edge| JsIncomingEdge {
                    source_id: edge.source_id as f64,
                    target_id: edge.target_id as f64,
                    label: edge.label,
                    weight: round_api_f32(edge.weight),
                    metadata: edge.metadata,
                })
                .collect()
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
        pub fn tql(&self, query: String) -> napi::Result<Vec<serde_json::Value>> {
            match &self.inner {
                DbBackend::F32(db) => db
                    .tql_values(&query)
                    .map_err(|e| napi::Error::from_reason(e.to_string()))
                    .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
                DbBackend::F16(db) => db
                    .tql_values(&query)
                    .map_err(|e| napi::Error::from_reason(e.to_string()))
                    .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
                DbBackend::U64(db) => db
                    .tql_values(&query)
                    .map_err(|e| napi::Error::from_reason(e.to_string()))
                    .map(|rows| rows.into_iter().map(node_tql_row_to_json).collect()),
            }
        }

        #[napi]
        pub fn prepare_tql(&self, query: String) -> napi::Result<JsPreparedTql> {
            dispatch!(self, db => db.prepare_tql(&query))
                .map(|inner| JsPreparedTql { inner })
                .map_err(to_napi_error)
        }

        #[napi]
        pub fn execute_prepared_tql(
            &self,
            prepared: &JsPreparedTql,
            parameters: serde_json::Value,
        ) -> napi::Result<Vec<serde_json::Value>> {
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
                        .map_err(to_napi_error)?,
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
            .map_err(to_napi_error)
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
        pub fn tql_mut(&mut self, query: String) -> napi::Result<serde_json::Value> {
            let result = dispatch!(self, mut db => db.tql_mut(&query)).map_err(to_napi_error)?;
            Ok(serde_json::json!({
                "affected": result.affected,
                "createdIds": result.created_ids,
            }))
        }

        // ── 持久化与管理 ──

        /// 手动落盘
        #[napi]
        pub fn flush(&mut self) -> napi::Result<()> {
            dispatch!(self, mut db => db.flush())
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        #[napi]
        pub fn publish_generation_manifest(
            &mut self,
            generation_id: String,
        ) -> napi::Result<serde_json::Value> {
            let manifest =
                dispatch!(self, mut db => db.publish_generation_manifest(&generation_id))
                    .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            serde_json::to_value(manifest)
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        /// 运行时切换 WAL 同步模式
        #[napi]
        pub fn set_sync_mode(&mut self, mode: String) -> napi::Result<()> {
            let sm = parse_sync_mode(&mode)?;
            dispatch!(self, mut db => db.set_sync_mode(sm))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        /// 启动后台自动压缩（每 interval_secs 秒落盘一次，默认 2 小时=7200秒）
        #[napi]
        pub fn enable_auto_compaction(&mut self, interval_secs: Option<u32>) -> napi::Result<()> {
            let secs = interval_secs.unwrap_or(7200);
            dispatch!(self, mut db => db.enable_auto_compaction(std::time::Duration::from_secs(secs as u64)))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        #[napi]
        pub fn set_auto_build_quiver(&mut self, enabled: bool) -> napi::Result<()> {
            dispatch!(self, mut db => db.set_auto_build_quiver(enabled))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        /// 停止后台自动压缩
        #[napi]
        pub fn clear_search_state(&self) {
            dispatch!(self, db => db.clear_search_state());
        }

        #[napi]
        pub fn disable_auto_compaction(&mut self) {
            dispatch!(self, mut db => db.disable_auto_compaction());
        }

        /// 手动触发全量压实（阻塞当前线程）
        #[napi]
        pub fn compact(&mut self) -> napi::Result<()> {
            dispatch!(self, mut db => db.compact())
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 为后续插入主动预留额外节点容量。
        #[napi]
        pub fn reserve_nodes(&self, additional: f64) -> napi::Result<()> {
            let additional = parse_safe_usize(additional, "additional")?;
            dispatch!(self, db => db.reserve_nodes(additional))
                .map_err(|error| napi::Error::from_reason(error.to_string()))
        }

        /// 设置内存上限（MiB），0 = 无限制。
        #[napi]
        pub fn set_memory_limit(&mut self, mb: f64) -> napi::Result<()> {
            let mb = parse_safe_usize(mb, "memoryLimitMb")?;
            let bytes = mb
                .checked_mul(1024 * 1024)
                .ok_or_else(|| napi::Error::from_reason("memoryLimitMb 换算字节时溢出"))?;
            dispatch!(self, mut db => db.set_memory_limit(bytes));
            Ok(())
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
        pub fn node_count(&self) -> u32 {
            dispatch!(self, db => db.node_count()) as u32
        }

        /// 获取所有活跃节点 ID
        #[napi]
        pub fn all_node_ids(&self) -> Vec<f64> {
            dispatch!(self, db => db.all_node_ids())
                .into_iter()
                .map(|id| id as f64)
                .collect()
        }

        /// 维度迁移：结构复制到新维度数据库，返回需要更新向量的节点 ID 列表
        #[napi]
        pub fn migrate(&self, new_path: String, new_dim: u32) -> napi::Result<Vec<f64>> {
            match &self.inner {
                DbBackend::F32(db) => {
                    let (_, ids) = db
                        .migrate_to(&new_path, new_dim as usize)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                    Ok(ids.into_iter().map(|id| id as f64).collect())
                }
                DbBackend::F16(db) => {
                    let (_, ids) = db
                        .migrate_to(&new_path, new_dim as usize)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                    Ok(ids.into_iter().map(|id| id as f64).collect())
                }
                DbBackend::U64(db) => {
                    let (_, ids) = db
                        .migrate_to(&new_path, new_dim as usize)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                    Ok(ids.into_iter().map(|id| id as f64).collect())
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
        pub fn contains(&self, id: f64) -> bool {
            dispatch!(self, db => db.contains(id as u64))
        }

        /// 显式关闭数据库（落盘后释放资源）
        #[napi]
        pub fn close(&mut self) -> napi::Result<()> {
            match dispatch!(self, mut db => db.close()) {
                Ok(()) | Err(crate::error::TriviumError::DatabaseClosed) => Ok(()),
                Err(error) => Err(to_napi_error(error)),
            }
        }
    } // impl TriviumDB
} // mod nodejs
