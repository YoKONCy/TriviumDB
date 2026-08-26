#[cfg(feature = "python")]
#[allow(clippy::module_inception)]
pub mod python {
    use crate::database::Database as GenericDatabase;
    use pyo3::prelude::*;
    use pyo3::types::{PyDict, PyList};

    pyo3::create_exception!(triviumdb, ReadOnlyError, pyo3::exceptions::PyRuntimeError);
    pyo3::create_exception!(
        triviumdb,
        RecoveryRequiredError,
        pyo3::exceptions::PyRuntimeError
    );
    pyo3::create_exception!(
        triviumdb,
        ImmutableArtifactError,
        pyo3::exceptions::PyRuntimeError
    );
    pyo3::create_exception!(
        triviumdb,
        GenerationBusyError,
        pyo3::exceptions::PyRuntimeError
    );

    fn to_py_error(error: crate::error::TriviumError) -> PyErr {
        match error {
            crate::error::TriviumError::ReadOnlyViolation { .. } => {
                ReadOnlyError::new_err(error.to_string())
            }
            crate::error::TriviumError::RecoveryRequired { .. } => {
                RecoveryRequiredError::new_err(error.to_string())
            }
            crate::error::TriviumError::ImmutableArtifactInvalid { .. } => {
                ImmutableArtifactError::new_err(error.to_string())
            }
            crate::error::TriviumError::GenerationBusy { .. } => {
                GenerationBusyError::new_err(error.to_string())
            }
            crate::error::TriviumError::InvalidInput(message) => {
                pyo3::exceptions::PyValueError::new_err(message)
            }
            _ => pyo3::exceptions::PyRuntimeError::new_err(error.to_string()),
        }
    }

    enum DbBackend {
        F32(GenericDatabase<f32>),
        F16(GenericDatabase<half::f16>),
        U64(GenericDatabase<u64>),
    }

    /// Python 侧的 TriviumDB 包装器
    #[pyclass(name = "TriviumDB")]
    pub struct PyTriviumDB {
        inner: DbBackend,
        #[pyo3(get)]
        dtype: String,
    }

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

    fn to_py_reachability(
        result: crate::graph::reachability::ReachabilityResult,
    ) -> PyReachabilityResult {
        PyReachabilityResult {
            source_id: result.source_id,
            target_id: result.target_id,
            depth: result.depth,
            path: result.path,
            steps: result
                .steps
                .into_iter()
                .map(|step| PyReachabilityStep {
                    from_id: step.from,
                    to_id: step.to,
                    label: step.label,
                })
                .collect(),
        }
    }

    /// Python 侧的查询命中结果
    #[pyclass(name = "SearchHit")]
    pub struct PySearchHit {
        #[pyo3(get)]
        pub id: u64,
        #[pyo3(get)]
        pub score: f32,
        #[pyo3(get)]
        pub payload: PyObject,
    }

    #[pyclass(name = "GroupedSearchResult")]
    pub struct PyGroupedSearchResult {
        #[pyo3(get)]
        pub semantic_hits: PyObject,
        #[pyo3(get)]
        pub graph_hits: PyObject,
    }

    #[pyclass(name = "ReachabilityStep")]
    #[derive(Clone)]
    pub struct PyReachabilityStep {
        #[pyo3(get)]
        pub from_id: u64,
        #[pyo3(get)]
        pub to_id: u64,
        #[pyo3(get)]
        pub label: String,
    }

    #[pyclass(name = "ReachabilityResult")]
    pub struct PyReachabilityResult {
        #[pyo3(get)]
        pub source_id: u64,
        #[pyo3(get)]
        pub target_id: u64,
        #[pyo3(get)]
        pub depth: usize,
        #[pyo3(get)]
        pub path: Vec<u64>,
        #[pyo3(get)]
        pub steps: Vec<PyReachabilityStep>,
    }

    #[pyclass(name = "Edge")]
    #[derive(Clone)]
    pub struct PyEdge {
        #[pyo3(get)]
        pub target_id: u64,
        #[pyo3(get)]
        pub label: String,
        #[pyo3(get)]
        pub weight: f64,
    }

    #[pyclass(name = "IncomingEdge")]
    #[derive(Clone)]
    pub struct PyIncomingEdge {
        #[pyo3(get)]
        pub source_id: u64,
        #[pyo3(get)]
        pub target_id: u64,
        #[pyo3(get)]
        pub label: String,
        #[pyo3(get)]
        pub weight: f64,
    }

    fn round_api_f32(value: f32) -> f64 {
        ((value as f64) * 1_000_000.0).round() / 1_000_000.0
    }

    fn search_hit_to_python(py: Python<'_>, hit: crate::node::SearchHit) -> PySearchHit {
        PySearchHit {
            id: hit.id,
            score: hit.score,
            payload: json_to_pyobject(py, &hit.payload),
        }
    }

    /// Python 侧的节点完整视图
    #[pyclass(name = "NodeView")]
    pub struct PyNodeView {
        #[pyo3(get)]
        pub id: u64,
        #[pyo3(get)]
        pub vector: PyObject, // 可能是 f32/f16(透传给py仍是float)/u64
        #[pyo3(get)]
        pub payload: PyObject,
        #[pyo3(get)]
        pub edges: Vec<PyEdge>,
        #[pyo3(get)]
        pub num_edges: usize,
    }

    /// Python 侧的 Cypher 查询单行结果
    /// 每一行是一个变量名 -> 节点视图的映射
    /// 例如: MATCH (a)-[:knows]->(b) RETURN a, b
    /// 则 row.get("a") 和 row.get("b") 各返回对应的节点
    #[pyclass(name = "QueryRow")]
    pub struct PyQueryRow {
        /// 变量名 -> (id, payload_dict)
        #[pyo3(get)]
        pub row: PyObject,
    }

    /// Hook 管线执行上下文（包含各阶段计时统计和自定义数据）
    #[pyclass(name = "HookContext")]
    pub struct PyHookContext {
        /// 各管线阶段的耗时统计（阶段名 → 耗时微秒数）
        #[pyo3(get)]
        pub timings: PyObject,
        /// 每阶段候选数量
        #[pyo3(get)]
        pub counts: PyObject,
        /// Hook 注入的自定义数据
        #[pyo3(get)]
        pub custom_data: PyObject,
        #[pyo3(get)]
        pub observations: PyObject,
        /// 管线是否被 Hook 提前终止
        #[pyo3(get)]
        pub aborted: bool,
    }

    #[pymethods]
    impl PyHookContext {
        fn __repr__(&self, py: Python<'_>) -> String {
            format!(
                "HookContext(aborted={}, timings={:?})",
                self.aborted,
                self.timings
                    .bind(py)
                    .repr()
                    .map(|r| r.to_string())
                    .unwrap_or_default()
            )
        }
    }

    #[pymethods]
    impl PyQueryRow {
        fn __repr__(&self, py: Python<'_>) -> String {
            format!(
                "QueryRow({:?})",
                self.row
                    .bind(py)
                    .repr()
                    .map(|r| r.to_string())
                    .unwrap_or_default()
            )
        }
    }

    // ════════ 辅助转换 ════════

    fn json_to_pyobject(py: Python<'_>, val: &serde_json::Value) -> PyObject {
        match val {
            serde_json::Value::Null => py.None(),
            serde_json::Value::Bool(b) => (*b)
                .into_pyobject(py)
                .unwrap()
                .to_owned()
                .into_any()
                .unbind(),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    i.into_pyobject(py).unwrap().into_any().unbind()
                } else {
                    n.as_f64()
                        .unwrap_or(0.0)
                        .into_pyobject(py)
                        .unwrap()
                        .into_any()
                        .unbind()
                }
            }
            serde_json::Value::String(s) => s.into_pyobject(py).unwrap().into_any().unbind(),
            serde_json::Value::Array(arr) => {
                let list = PyList::new(py, arr.iter().map(|v| json_to_pyobject(py, v))).unwrap();
                list.into_any().unbind()
            }
            serde_json::Value::Object(map) => {
                let dict = PyDict::new(py);
                for (k, v) in map {
                    let _ = dict.set_item(k, json_to_pyobject(py, v));
                }
                dict.into_any().unbind()
            }
        }
    }

    fn pyobject_to_json(obj: &Bound<'_, PyAny>) -> serde_json::Value {
        if obj.is_none() {
            serde_json::Value::Null
        } else if let Ok(b) = obj.extract::<bool>() {
            serde_json::Value::Bool(b)
        } else if let Ok(i) = obj.extract::<i64>() {
            serde_json::json!(i)
        } else if let Ok(f) = obj.extract::<f64>() {
            serde_json::json!(f)
        } else if let Ok(s) = obj.extract::<String>() {
            serde_json::Value::String(s)
        } else if let Ok(dict) = obj.downcast::<PyDict>() {
            let mut map = serde_json::Map::new();
            for (k, v) in dict.iter() {
                if let Ok(key) = k.extract::<String>() {
                    map.insert(key, pyobject_to_json(&v));
                }
            }
            serde_json::Value::Object(map)
        } else if let Ok(list) = obj.downcast::<PyList>() {
            let arr: Vec<serde_json::Value> =
                list.iter().map(|item| pyobject_to_json(&item)).collect();
            serde_json::Value::Array(arr)
        } else {
            serde_json::Value::Null
        }
    }

    use crate::filter::Filter;

    fn dict_to_filter(_py: Python<'_>, dict: &Bound<'_, PyDict>) -> PyResult<Filter> {
        // 将 PyDict 转为 serde_json::Value，再统一调用 Filter::from_json
        let json_val = pyobject_to_json(&dict.clone().into_any());
        Filter::from_json(&json_val).map_err(pyo3::exceptions::PyValueError::new_err)
    }

    fn parse_sync_mode(s: &str) -> PyResult<crate::storage::wal::SyncMode> {
        crate::storage::wal::SyncMode::parse(s).map_err(pyo3::exceptions::PyValueError::new_err)
    }

    fn parse_access_mode(s: &str) -> PyResult<crate::database::AccessMode> {
        match s {
            "read_write" => Ok(crate::database::AccessMode::ReadWrite),
            "read_only" => Ok(crate::database::AccessMode::ReadOnly),
            "immutable" => Ok(crate::database::AccessMode::Immutable),
            _ => Err(pyo3::exceptions::PyValueError::new_err(
                "access_mode 必须是 read_write / read_only / immutable",
            )),
        }
    }

    fn parse_missing_index_policy(value: &str) -> PyResult<crate::database::MissingIndexPolicy> {
        match value {
            "fallback" => Ok(crate::database::MissingIndexPolicy::Fallback),
            "build_in_memory" => Ok(crate::database::MissingIndexPolicy::BuildInMemory),
            "error" => Ok(crate::database::MissingIndexPolicy::Error),
            _ => Err(pyo3::exceptions::PyValueError::new_err(
                "missing_index_policy 必须是 fallback / build_in_memory / error",
            )),
        }
    }

    #[pymethods]
    impl PyTriviumDB {
        #[new]
        #[pyo3(signature = (path, dim=1536, dtype="f32", sync_mode="normal", load_text_index=false, auto_build_quiver=true, expected_nodes=None, memory_limit_mb=0, access_mode="read_write", missing_index_policy="fallback"))]
        fn new(
            path: &str,
            dim: usize,
            dtype: &str,
            sync_mode: &str,
            load_text_index: bool,
            auto_build_quiver: bool,
            expected_nodes: Option<usize>,
            memory_limit_mb: usize,
            access_mode: &str,
            missing_index_policy: &str,
        ) -> PyResult<Self> {
            let sm = parse_sync_mode(sync_mode)?;
            let memory_limit = memory_limit_mb.checked_mul(1024 * 1024).ok_or_else(|| {
                pyo3::exceptions::PyOverflowError::new_err("memory_limit_mb 换算字节时溢出")
            })?;
            let config = crate::database::Config {
                dim,
                sync_mode: sm,
                load_text_index,
                auto_build_quiver,
                expected_nodes,
                memory_limit,
                access_mode: parse_access_mode(access_mode)?,
                missing_index_policy: parse_missing_index_policy(missing_index_policy)?,
                ..Default::default()
            };
            let inner = match dtype {
                "f32" => DbBackend::F32(
                    GenericDatabase::<f32>::open_with_config(path, config).map_err(to_py_error)?,
                ),
                "f16" => DbBackend::F16(
                    GenericDatabase::<half::f16>::open_with_config(path, config)
                        .map_err(to_py_error)?,
                ),
                "u64" => DbBackend::U64(
                    GenericDatabase::<u64>::open_with_config(path, config).map_err(to_py_error)?,
                ),
                _ => {
                    return Err(pyo3::exceptions::PyValueError::new_err(
                        "Unsupported dtype. Use 'f32', 'f16', or 'u64'",
                    ));
                }
            };
            Ok(Self {
                inner,
                dtype: dtype.to_string(),
            })
        }

        /// 运行时切换 WAL 同步模式: "full" / "normal" / "off"
        fn set_sync_mode(&mut self, mode: &str) -> PyResult<()> {
            let sm = parse_sync_mode(mode)?;
            dispatch!(self, mut db => db.set_sync_mode(sm))
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))
        }

        // ════════ Hook 管理 ════════

        /// 加载 C/C++ 动态库作为检索管线 Hook
        ///
        /// 动态库需要导出以下 C ABI 符号（均为可选）：
        /// - `trivium_recall`: 自定义召回
        /// - `trivium_rerank`: 自定义重排序
        ///
        /// 示例：
        /// ```python
        /// db.load_ffi_hook("./libmy_plugin.so")
        /// results = db.search(query_vec)  # 自动经过 C++ Hook
        /// ```
        fn load_ffi_hook(&mut self, lib_path: &str) -> PyResult<()> {
            let ffi_hook = crate::hook::FfiHook::load(lib_path).map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!("加载 FFI Hook 失败: {}", e))
            })?;
            dispatch!(self, mut db => db.set_hook(ffi_hook));
            Ok(())
        }

        /// 清除当前已注册的 Hook，恢复为默认的零开销 NoopHook
        fn clear_hook(&mut self) {
            dispatch!(self, mut db => db.clear_hook());
        }

        /// 注册一个 Python 原生 Hook 对象
        ///
        /// Python 类只需实现感兴趣的方法（鸭子类型，全部可选）：
        /// - on_pre_search(self, query_vector, ctx) -> Optional[list[float]]
        /// - on_post_recall(self, hits, ctx) -> Optional[list[dict]]
        /// - on_rerank(self, hits, ctx) -> Optional[list[dict]]
        /// - on_post_search(self, hits, ctx) -> Optional[list[dict]]
        ///
        /// 示例：
        /// ```python
        /// class MyHook:
        ///     def on_post_recall(self, hits, ctx):
        ///         return [h for h in hits if h["score"] > 0.5]
        ///
        /// db.set_hook(MyHook())
        /// ```
        fn set_hook(&mut self, hook: PyObject) {
            let wrapper = PySearchHookWrapper { py_hook: hook };
            dispatch!(self, mut db => db.set_hook(wrapper));
        }

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
        #[pyo3(signature = (query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=2, min_score=0.1, payload_filter=None))]
        fn search_with_context(
            &self,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            top_k: usize,
            recall_k: usize,
            rerank_k: usize,
            expand_depth: usize,
            min_score: f32,
            payload_filter: Option<&Bound<'_, PyDict>>,
        ) -> PyResult<(Vec<PySearchHit>, PyHookContext)> {
            let rust_filter = match payload_filter {
                Some(dict) => Some(dict_to_filter(py, dict)?),
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

            let (results, hook_ctx) = match &self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = query_vector.extract()?;
                    py.allow_threads(|| db.search_hybrid_with_context(None, Some(&vec), &config))
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = query_vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    py.allow_threads(|| db.search_hybrid_with_context(None, Some(&vec16), &config))
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = query_vector.extract()?;
                    py.allow_threads(|| db.search_hybrid_with_context(None, Some(&vec), &config))
                }
            }
            .map_err(|e: crate::error::TriviumError| {
                pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
            })?;

            // 转换搜索结果
            let hits: Vec<PySearchHit> = results
                .into_iter()
                .map(|h| PySearchHit {
                    id: h.id,
                    score: h.score,
                    payload: json_to_pyobject(py, &h.payload),
                })
                .collect();

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
                custom_data: json_to_pyobject(py, &hook_ctx.custom_data),
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

        fn insert(
            &mut self,
            _py: Python<'_>,
            vector: Bound<'_, PyAny>,
            payload: &Bound<'_, PyAny>,
        ) -> PyResult<u64> {
            let json = pyobject_to_json(payload);
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    db.insert(&vec, json)
                        .map_err(|e: crate::error::TriviumError| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        })
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    db.insert(&vec16, json)
                        .map_err(|e: crate::error::TriviumError| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        })
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = vector.extract()?;
                    db.insert(&vec, json)
                        .map_err(|e: crate::error::TriviumError| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        })
                }
            }
        }

        fn insert_with_id(
            &mut self,
            _py: Python<'_>,
            id: u64,
            vector: Bound<'_, PyAny>,
            payload: &Bound<'_, PyAny>,
        ) -> PyResult<()> {
            let json = pyobject_to_json(payload);
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    db.insert_with_id(id, &vec, json)
                        .map_err(|e: crate::error::TriviumError| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        })
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    db.insert_with_id(id, &vec16, json)
                        .map_err(|e: crate::error::TriviumError| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        })
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = vector.extract()?;
                    db.insert_with_id(id, &vec, json)
                        .map_err(|e: crate::error::TriviumError| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        })
                }
            }
        }

        #[pyo3(signature = (src, dst, label="related", weight=1.0))]
        fn link(&mut self, src: u64, dst: u64, label: &str, weight: f32) -> PyResult<()> {
            dispatch!(self, mut db => db.link(src, dst, label, weight)).map_err(
                |e: crate::error::TriviumError| {
                    pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                },
            )
        }

        #[pyo3(signature = (query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=0, min_score=0.5, payload_filter=None))]
        fn search(
            &self,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            top_k: usize,
            recall_k: usize,
            rerank_k: usize,
            expand_depth: usize,
            min_score: f32,
            payload_filter: Option<&Bound<'_, PyDict>>,
        ) -> PyResult<Vec<PySearchHit>> {
            let rust_filter = match payload_filter {
                Some(dict) => Some(dict_to_filter(py, dict)?),
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
                ..Default::default()
            };

            let results = match &self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = query_vector.extract()?;
                    py.allow_threads(|| db.search_hybrid(None, Some(&vec), &config))
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = query_vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    py.allow_threads(|| db.search_hybrid(None, Some(&vec16), &config))
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = query_vector.extract()?;
                    py.allow_threads(|| db.search_hybrid(None, Some(&vec), &config))
                }
            }
            .map_err(|e: crate::error::TriviumError| {
                pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
            })?;

            Ok(results
                .into_iter()
                .map(|h| PySearchHit {
                    id: h.id,
                    score: h.score,
                    payload: json_to_pyobject(py, &h.payload),
                })
                .collect())
        }

        #[pyo3(signature = (query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=2, min_score=0.1, payload_filter=None))]
        fn search_grouped(
            &self,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            top_k: usize,
            recall_k: usize,
            rerank_k: usize,
            expand_depth: usize,
            min_score: f32,
            payload_filter: Option<&Bound<'_, PyDict>>,
        ) -> PyResult<PyGroupedSearchResult> {
            let rust_filter = payload_filter
                .map(|dict| dict_to_filter(py, dict))
                .transpose()?;
            let config = crate::database::SearchConfig {
                top_k,
                recall_k,
                rerank_k,
                expand_depth,
                min_score,
                payload_filter: rust_filter,
                ..Default::default()
            };
            let result = match &self.inner {
                DbBackend::F32(db) => {
                    let vector: Vec<f32> = query_vector.extract()?;
                    py.allow_threads(|| db.search_hybrid_grouped(None, Some(&vector), &config))
                }
                DbBackend::F16(db) => {
                    let vector: Vec<f32> = query_vector.extract()?;
                    let vector: Vec<half::f16> =
                        vector.into_iter().map(half::f16::from_f32).collect();
                    py.allow_threads(|| db.search_hybrid_grouped(None, Some(&vector), &config))
                }
                DbBackend::U64(db) => {
                    let vector: Vec<u64> = query_vector.extract()?;
                    py.allow_threads(|| db.search_hybrid_grouped(None, Some(&vector), &config))
                }
            }
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            Ok(PyGroupedSearchResult {
                semantic_hits: result
                    .semantic_hits
                    .into_iter()
                    .map(|hit| search_hit_to_python(py, hit))
                    .collect::<Vec<_>>()
                    .into_pyobject(py)
                    .unwrap()
                    .into_any()
                    .unbind(),
                graph_hits: result
                    .graph_hits
                    .into_iter()
                    .map(|hit| search_hit_to_python(py, hit))
                    .collect::<Vec<_>>()
                    .into_pyobject(py)
                    .unwrap()
                    .into_any()
                    .unbind(),
            })
        }

        #[pyo3(signature = (query_vectors, top_k=5, recall_k=0, rerank_k=0, expand_depth=0, min_score=0.5, parallelism=0))]
        fn search_batch(
            &self,
            py: Python<'_>,
            query_vectors: Bound<'_, PyAny>,
            top_k: usize,
            recall_k: usize,
            rerank_k: usize,
            expand_depth: usize,
            min_score: f32,
            parallelism: usize,
        ) -> PyResult<Vec<Vec<PySearchHit>>> {
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
            let results = match &self.inner {
                DbBackend::F32(db) => {
                    let queries: Vec<Vec<f32>> = query_vectors.extract()?;
                    py.allow_threads(|| db.search_batch(&queries, &config, &batch_config))
                }
                DbBackend::F16(db) => {
                    let queries: Vec<Vec<f32>> = query_vectors.extract()?;
                    let queries: Vec<Vec<half::f16>> = queries
                        .into_iter()
                        .map(|query| query.into_iter().map(half::f16::from_f32).collect())
                        .collect();
                    py.allow_threads(|| db.search_batch(&queries, &config, &batch_config))
                }
                DbBackend::U64(db) => {
                    let queries: Vec<Vec<u64>> = query_vectors.extract()?;
                    py.allow_threads(|| db.search_batch(&queries, &config, &batch_config))
                }
            }
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;

            Ok(results
                .into_iter()
                .map(|hits| {
                    hits.into_iter()
                        .map(|hit| PySearchHit {
                            id: hit.id,
                            score: hit.score,
                            payload: json_to_pyobject(py, &hit.payload),
                        })
                        .collect()
                })
                .collect())
        }

        #[pyo3(signature = (
            query_vector,
            top_k=5,
            recall_k=0,
            rerank_k=0,
            expand_depth=2,
            min_score=0.1,
            teleport_alpha=0.0,
            enable_advanced_pipeline=true,
            enable_sparse_residual=false,
            fista_lambda=0.1,
            fista_threshold=0.3,
            enable_dpp=false,
            dpp_quality_weight=1.0,
            enable_refractory_fatigue=false,
            enable_text_hybrid_search=false,
            text_boost=1.5,
            custom_query_text=None,
            payload_filter=None,
            force_brute_force=false,
            expand_labels=None
        ))]
        fn search_advanced(
            &self,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            top_k: usize,
            recall_k: usize,
            rerank_k: usize,
            expand_depth: usize,
            min_score: f32,
            teleport_alpha: f32,
            enable_advanced_pipeline: bool,
            enable_sparse_residual: bool,
            fista_lambda: f32,
            fista_threshold: f32,
            enable_dpp: bool,
            dpp_quality_weight: f32,
            enable_refractory_fatigue: bool,
            enable_text_hybrid_search: bool,
            text_boost: f32,
            custom_query_text: Option<String>,
            payload_filter: Option<&Bound<'_, PyDict>>,
            force_brute_force: bool,
            expand_labels: Option<Vec<String>>,
        ) -> PyResult<Vec<PySearchHit>> {
            // 解析 payload_filter（类 MongoDB 语法的 dict -> Rust Filter）
            let rust_filter = match payload_filter {
                Some(dict) => Some(dict_to_filter(py, dict)?),
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
                payload_filter: rust_filter,
                ..Default::default()
            };

            let q_text = custom_query_text.as_deref();

            let results = match &self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = query_vector.extract()?;
                    py.allow_threads(|| db.search_hybrid(q_text, Some(&vec), &config))
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = query_vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    py.allow_threads(|| db.search_hybrid(q_text, Some(&vec16), &config))
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = query_vector.extract()?;
                    py.allow_threads(|| db.search_hybrid(q_text, Some(&vec), &config))
                }
            }
            .map_err(|e: crate::error::TriviumError| {
                pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
            })?;

            Ok(results
                .into_iter()
                .map(|h| PySearchHit {
                    id: h.id,
                    score: h.score,
                    payload: json_to_pyobject(py, &h.payload),
                })
                .collect())
        }

        #[pyo3(signature = (query_vector, query_text, top_k=5, expand_depth=2, min_score=0.1, hybrid_alpha=0.7, payload_filter=None))]
        fn search_hybrid(
            &self,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            query_text: &str,
            top_k: usize,
            expand_depth: usize,
            min_score: f32,
            hybrid_alpha: f32,
            payload_filter: Option<&Bound<'_, PyDict>>,
        ) -> PyResult<Vec<PySearchHit>> {
            let rust_filter = match payload_filter {
                Some(dict) => Some(dict_to_filter(py, dict)?),
                None => None,
            };

            // hybrid_alpha 越大，向量分数占比越高。
            // TriviumDB 底层使用 text_boost = (1.0 - alpha) * 2.5 作为启发式倍率
            let boost = (1.0 - hybrid_alpha).max(0.1) * 3.0;
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
            let results = match &self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = query_vector.extract()?;
                    py.allow_threads(|| db.search_hybrid(Some(&query_text), Some(&vec), &config))
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = query_vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    py.allow_threads(|| db.search_hybrid(Some(&query_text), Some(&vec16), &config))
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = query_vector.extract()?;
                    py.allow_threads(|| db.search_hybrid(Some(&query_text), Some(&vec), &config))
                }
            }
            .map_err(|e: crate::error::TriviumError| {
                pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
            })?;
            Ok(results
                .into_iter()
                .map(|h| PySearchHit {
                    id: h.id,
                    score: h.score,
                    payload: json_to_pyobject(py, &h.payload),
                })
                .collect())
        }

        fn delete(&mut self, id: u64) -> PyResult<()> {
            dispatch!(self, mut db => db.delete(id)).map_err(|e: crate::error::TriviumError| {
                pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
            })
        }

        #[pyo3(signature = (src, dst, label=None))]
        fn unlink(&mut self, src: u64, dst: u64, label: Option<&str>) -> PyResult<()> {
            match label {
                Some(label) => dispatch!(self, mut db => db.unlink_label(src, dst, label)),
                None => dispatch!(self, mut db => db.unlink(src, dst)),
            }
            .map_err(|e: crate::error::TriviumError| {
                pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
            })
        }

        fn update_payload(
            &mut self,
            _py: Python<'_>,
            id: u64,
            payload: &Bound<'_, PyAny>,
        ) -> PyResult<()> {
            let json = pyobject_to_json(payload);
            dispatch!(self, mut db => db.update_payload(id, json)).map_err(
                |e: crate::error::TriviumError| {
                    pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                },
            )
        }

        /// 部分更新节点 Payload（$set / $inc / $unset）
        ///
        /// 只修改指定字段，其他字段保持不变。
        ///
        /// 示例：
        /// ```python
        /// db.patch_payload(id, {"$set": {"name": "Alice"}})
        /// db.patch_payload(id, {"$inc": {"visits": 1}})
        /// db.patch_payload(id, {"$unset": {"old_field": True}})
        /// db.patch_payload(id, {"name": "Bob"})  # 简写，等价于 $set
        /// ```
        fn patch_payload(
            &mut self,
            _py: Python<'_>,
            id: u64,
            patch: &Bound<'_, PyAny>,
        ) -> PyResult<()> {
            let json = pyobject_to_json(patch);
            dispatch!(self, mut db => db.patch_payload(id, json)).map_err(
                |e: crate::error::TriviumError| {
                    pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                },
            )
        }
        fn update_vector(&mut self, vector: Bound<'_, PyAny>, id: u64) -> PyResult<()> {
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    db.update_vector(id, &vec)
                        .map_err(|e: crate::error::TriviumError| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        })
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    db.update_vector(id, &vec16)
                        .map_err(|e: crate::error::TriviumError| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        })
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = vector.extract()?;
                    db.update_vector(id, &vec)
                        .map_err(|e: crate::error::TriviumError| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        })
                }
            }
        }

        fn index_text(&mut self, id: u64, text: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.index_text(id, text)).map_err(
                |e: crate::error::TriviumError| {
                    pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                },
            )
        }

        fn index_keyword(&mut self, id: u64, keyword: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.index_keyword(id, keyword)).map_err(
                |e: crate::error::TriviumError| {
                    pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                },
            )
        }

        fn build_text_index(&mut self) {
            let _ = dispatch!(self, mut db => db.build_text_index());
        }

        // ════════ 属性二级索引 ════════

        /// 创建属性索引：对指定 payload 字段建立倒排索引，加速 MATCH/FIND 查询
        ///
        /// 示例：
        /// ```python
        /// db.create_index("name")    # 之后 tql('FIND {name: "Alice"} RETURN *') 使用 O(1) 索引
        /// db.create_index("type")
        /// ```
        fn create_index(&mut self, field: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.create_index(field))
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))
        }

        /// 删除属性索引（查询仍可用，退化为全扫描）
        fn drop_index(&mut self, field: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.drop_index(field))
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))
        }

        // ════════ 轻量级单字段查询 ════════

        /// 获取节点的 payload（不含向量，比 get() 更轻量）
        fn get_payload(&self, py: Python<'_>, id: u64) -> Option<PyObject> {
            dispatch!(self, db => db.get_payload(id)).map(|p| json_to_pyobject(py, &p))
        }

        /// 获取节点的出边列表
        fn get_edges(&self, id: u64) -> Vec<PyEdge> {
            dispatch!(self, db => db.get_edges(id))
                .into_iter()
                .map(|e| PyEdge {
                    target_id: e.target_id,
                    label: e.label,
                    weight: round_api_f32(e.weight),
                })
                .collect()
        }

        #[pyo3(signature = (id, label=None))]
        fn get_incoming_edges(&self, id: u64, label: Option<&str>) -> Vec<PyIncomingEdge> {
            dispatch!(self, db => db.get_incoming_edges(id, label))
                .into_iter()
                .map(|edge| PyIncomingEdge {
                    source_id: edge.source_id,
                    target_id: edge.target_id,
                    label: edge.label,
                    weight: round_api_f32(edge.weight),
                })
                .collect()
        }

        fn get(&self, py: Python<'_>, id: u64) -> PyResult<Option<PyNodeView>> {
            match &self.inner {
                DbBackend::F32(db) => {
                    if let Some(n) = db.get(id) {
                        return Ok(Some(PyNodeView {
                            id: n.id,
                            vector: n.vector.into_pyobject(py).unwrap().into_any().unbind(),
                            payload: json_to_pyobject(py, &n.payload),
                            edges: n
                                .edges
                                .iter()
                                .map(|e| PyEdge {
                                    target_id: e.target_id,
                                    label: e.label.clone(),
                                    weight: round_api_f32(e.weight),
                                })
                                .collect(),
                            num_edges: n.edges.len(),
                        }));
                    }
                }
                DbBackend::F16(db) => {
                    if let Some(n) = db.get(id) {
                        let f32_vec: Vec<f32> = n.vector.into_iter().map(|f| f.to_f32()).collect();
                        return Ok(Some(PyNodeView {
                            id: n.id,
                            vector: f32_vec.into_pyobject(py).unwrap().into_any().unbind(),
                            payload: json_to_pyobject(py, &n.payload),
                            edges: n
                                .edges
                                .iter()
                                .map(|e| PyEdge {
                                    target_id: e.target_id,
                                    label: e.label.clone(),
                                    weight: round_api_f32(e.weight),
                                })
                                .collect(),
                            num_edges: n.edges.len(),
                        }));
                    }
                }
                DbBackend::U64(db) => {
                    if let Some(n) = db.get(id) {
                        return Ok(Some(PyNodeView {
                            id: n.id,
                            vector: n.vector.into_pyobject(py).unwrap().into_any().unbind(),
                            payload: json_to_pyobject(py, &n.payload),
                            edges: n
                                .edges
                                .iter()
                                .map(|e| PyEdge {
                                    target_id: e.target_id,
                                    label: e.label.clone(),
                                    weight: round_api_f32(e.weight),
                                })
                                .collect(),
                            num_edges: n.edges.len(),
                        }));
                    }
                }
            }
            Ok(None)
        }

        #[pyo3(signature = (id, depth=1, labels=None))]
        fn neighbors(&self, id: u64, depth: usize, labels: Option<Vec<String>>) -> Vec<u64> {
            dispatch!(self, db => db.neighbors_with_labels(id, depth, labels.as_deref()))
        }

        #[pyo3(signature = (id, min_depth=1, max_depth=1, labels=None, direction="outgoing", max_visited_nodes=10_000))]
        fn reachable(
            &self,
            id: u64,
            min_depth: usize,
            max_depth: usize,
            labels: Option<Vec<String>>,
            direction: &str,
            max_visited_nodes: usize,
        ) -> PyResult<Vec<PyReachabilityResult>> {
            let direction = match direction {
                "outgoing" => crate::graph::reachability::ReachabilityDirection::Outgoing,
                "incoming" => crate::graph::reachability::ReachabilityDirection::Incoming,
                "both" => crate::graph::reachability::ReachabilityDirection::Both,
                _ => {
                    return Err(pyo3::exceptions::PyValueError::new_err(
                        "direction 必须是 outgoing / incoming / both",
                    ));
                }
            };
            let config = crate::graph::reachability::ReachabilityConfig {
                min_depth,
                max_depth,
                labels,
                direction,
                max_visited_nodes,
            };
            dispatch!(self, db => db.reachable(id, &config))
                .map(|results| results.into_iter().map(to_py_reachability).collect())
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))
        }

        #[pyo3(signature = (query_vector, anchor_ids, top_k, max_anchor_nodes=100_000))]
        fn search_graph_first(
            &self,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            anchor_ids: Vec<u64>,
            top_k: usize,
            max_anchor_nodes: usize,
        ) -> PyResult<Vec<PySearchHit>> {
            let hits = match &self.inner {
                DbBackend::F32(db) => {
                    let query: Vec<f32> = query_vector.extract()?;
                    db.search_graph_first(&query, &anchor_ids, top_k, max_anchor_nodes)
                }
                DbBackend::F16(db) => {
                    let query: Vec<f32> = query_vector.extract()?;
                    let query: Vec<half::f16> =
                        query.into_iter().map(half::f16::from_f32).collect();
                    db.search_graph_first(&query, &anchor_ids, top_k, max_anchor_nodes)
                }
                DbBackend::U64(db) => {
                    let query: Vec<u64> = query_vector.extract()?;
                    db.search_graph_first(&query, &anchor_ids, top_k, max_anchor_nodes)
                }
            }
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            Ok(hits
                .into_iter()
                .map(|hit| PySearchHit {
                    id: hit.id,
                    score: hit.score,
                    payload: json_to_pyobject(py, &hit.payload),
                })
                .collect())
        }

        #[pyo3(signature = (query_vector, top_k))]
        fn search_exact(
            &self,
            py: Python<'_>,
            query_vector: Bound<'_, PyAny>,
            top_k: usize,
        ) -> PyResult<Vec<PySearchHit>> {
            let hits = match &self.inner {
                DbBackend::F32(db) => {
                    let query: Vec<f32> = query_vector.extract()?;
                    db.search_exact(&query, top_k)
                }
                DbBackend::F16(db) => {
                    let query: Vec<f32> = query_vector.extract()?;
                    let query: Vec<half::f16> =
                        query.into_iter().map(half::f16::from_f32).collect();
                    db.search_exact(&query, top_k)
                }
                DbBackend::U64(db) => {
                    let query: Vec<u64> = query_vector.extract()?;
                    db.search_exact(&query, top_k)
                }
            }
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            Ok(hits
                .into_iter()
                .map(|hit| PySearchHit {
                    id: hit.id,
                    score: hit.score,
                    payload: json_to_pyobject(py, &hit.payload),
                })
                .collect())
        }

        fn node_count(&self) -> usize {
            dispatch!(self, db => db.node_count())
        }

        fn flush(&mut self) -> PyResult<()> {
            dispatch!(self, mut db => db.flush()).map_err(|e: crate::error::TriviumError| {
                pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
            })
        }

        fn publish_generation_manifest(&mut self, generation_id: &str) -> PyResult<PyObject> {
            let manifest = dispatch!(self, mut db => db.publish_generation_manifest(generation_id))
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            let value = serde_json::to_value(manifest)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            Ok(Python::with_gil(|py| json_to_pyobject(py, &value)))
        }

        fn dim(&self) -> usize {
            dispatch!(self, db => db.dim())
        }

        /// 获取所有活跃节点的 ID 列表
        fn all_node_ids(&self) -> Vec<u64> {
            dispatch!(self, db => db.all_node_ids())
        }

        /// 维度迁移：将当前数据库的所有节点和边迁移到一个新维度的数据库。
        ///
        /// 向量会被置零（因为维度变了），需要后续调用 update_vector 按节点 ID 逐个更新。
        ///
        /// 返回需要更新向量的节点 ID 列表。
        ///
        /// 示例：
        /// ```python
        /// ids = old_db.migrate("new.tdb", new_dim=1536)
        /// new_db = triviumdb.TriviumDB("new.tdb", dim=1536)
        /// for nid in ids:
        ///     new_vec = new_model.encode(payloads[nid]["text"]).tolist()
        ///     new_db.update_vector(new_vec, nid)
        /// ```
        fn migrate(&self, new_path: &str, new_dim: usize) -> PyResult<Vec<u64>> {
            match &self.inner {
                DbBackend::F32(db) => {
                    let (_new_db, ids) = db.migrate_to(new_path, new_dim).map_err(
                        |e: crate::error::TriviumError| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        },
                    )?;
                    Ok(ids)
                }
                DbBackend::F16(db) => {
                    let (_new_db, ids) = db.migrate_to(new_path, new_dim).map_err(
                        |e: crate::error::TriviumError| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        },
                    )?;
                    Ok(ids)
                }
                DbBackend::U64(db) => {
                    let (_new_db, ids) = db.migrate_to(new_path, new_dim).map_err(
                        |e: crate::error::TriviumError| {
                            pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                        },
                    )?;
                    Ok(ids)
                }
            }
        }

        #[pyo3(signature = (interval_secs=7200))]
        fn enable_auto_compaction(&mut self, interval_secs: u64) -> PyResult<()> {
            dispatch!(self, mut db => db.enable_auto_compaction(std::time::Duration::from_secs(interval_secs)))
                .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))
        }

        fn set_auto_build_quiver(&mut self, enabled: bool) -> PyResult<()> {
            dispatch!(self, mut db => db.set_auto_build_quiver(enabled))
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))
        }

        fn clear_search_state(&self) {
            dispatch!(self, db => db.clear_search_state());
        }

        fn disable_auto_compaction(&mut self) {
            dispatch!(self, mut db => db.disable_auto_compaction());
        }

        fn compact(&mut self) -> PyResult<()> {
            dispatch!(self, mut db => db.compact()).map_err(|e: crate::error::TriviumError| {
                pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
            })
        }

        /// 为后续插入主动预留额外节点容量。
        fn reserve_nodes(&mut self, additional: usize) -> PyResult<()> {
            dispatch!(self, db => db.reserve_nodes(additional)).map_err(|error| match error {
                crate::error::TriviumError::CapacityReservationRejected { .. }
                | crate::error::TriviumError::CapacityAllocationFailed { .. } => {
                    pyo3::exceptions::PyMemoryError::new_err(error.to_string())
                }
                _ => pyo3::exceptions::PyValueError::new_err(error.to_string()),
            })
        }

        /// 设置内核内存预算（MiB），0 表示不限制。
        #[pyo3(signature = (mb=0))]
        fn set_memory_limit(&mut self, mb: usize) -> PyResult<()> {
            let bytes = mb.checked_mul(1024 * 1024).ok_or_else(|| {
                pyo3::exceptions::PyOverflowError::new_err("内存上限换算字节时溢出")
            })?;
            dispatch!(self, mut db => db.set_memory_limit(bytes));
            Ok(())
        }

        /// 查询当前估算内存占用（字节）
        fn estimated_memory(&self) -> usize {
            dispatch!(self, db => db.estimated_memory())
        }

        fn __len__(&self) -> usize {
            self.node_count()
        }

        fn __contains__(&self, id: u64) -> bool {
            dispatch!(self, db => db.contains(id))
        }

        fn __repr__(&self) -> String {
            format!(
                "TriviumDB(dtype={}, nodes={}, dim={})",
                self.dtype,
                self.node_count(),
                self.dim()
            )
        }

        fn __enter__(slf: Py<Self>) -> Py<Self> {
            slf
        }

        #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
        fn __exit__(
            &mut self,
            _exc_type: Option<&Bound<'_, PyAny>>,
            _exc_val: Option<&Bound<'_, PyAny>>,
            _exc_tb: Option<&Bound<'_, PyAny>>,
        ) -> PyResult<bool> {
            // 上下文管理器退出时关闭数据库并释放单写锁，
            // 若块内已显式 close()（数据库已关闭）则视为无操作。
            match dispatch!(self, mut db => db.close()) {
                Ok(()) | Err(crate::error::TriviumError::DatabaseClosed) => Ok(false),
                Err(e) => Err(pyo3::exceptions::PyRuntimeError::new_err(e.to_string())),
            }
        }

        fn batch_insert(
            &mut self,
            _py: Python<'_>,
            vectors: Bound<'_, PyList>,
            payloads: &Bound<'_, PyList>,
        ) -> PyResult<Vec<u64>> {
            if vectors.len() != payloads.len() {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "vectors and payloads must have the same length",
                ));
            }
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let mut tx = crate::database::TxBuilder::new();
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec_obj = vectors.get_item(i)?;
                        let vec: Vec<f32> = vec_obj.extract()?;
                        tx.insert(&vec, pyobject_to_json(&payload_obj));
                    }
                    db.commit_tx(tx).map_err(|error| {
                        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
                    })
                }
                DbBackend::F16(db) => {
                    let mut tx = crate::database::TxBuilder::new();
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec_obj = vectors.get_item(i)?;
                        let vec: Vec<f32> = vec_obj.extract()?;
                        let vec16: Vec<half::f16> =
                            vec.into_iter().map(half::f16::from_f32).collect();
                        tx.insert(&vec16, pyobject_to_json(&payload_obj));
                    }
                    db.commit_tx(tx).map_err(|error| {
                        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
                    })
                }
                DbBackend::U64(db) => {
                    let mut tx = crate::database::TxBuilder::new();
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec_obj = vectors.get_item(i)?;
                        let vec: Vec<u64> = vec_obj.extract()?;
                        tx.insert(&vec, pyobject_to_json(&payload_obj));
                    }
                    db.commit_tx(tx).map_err(|error| {
                        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
                    })
                }
            }
        }

        fn batch_insert_with_ids(
            &mut self,
            _py: Python<'_>,
            ids: Vec<u64>,
            vectors: Bound<'_, PyList>,
            payloads: &Bound<'_, PyList>,
        ) -> PyResult<()> {
            if vectors.len() != payloads.len() || ids.len() != vectors.len() {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "ids, vectors and payloads must have the same length",
                ));
            }

            match &mut self.inner {
                DbBackend::F32(db) => {
                    let mut tx = crate::database::TxBuilder::new();
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec: Vec<f32> = vectors.get_item(i)?.extract()?;
                        tx.insert_with_id(ids[i], &vec, pyobject_to_json(&payload_obj));
                    }
                    db.commit_tx(tx).map(|_| ()).map_err(|error| {
                        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
                    })
                }
                DbBackend::F16(db) => {
                    let mut tx = crate::database::TxBuilder::new();
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec: Vec<f32> = vectors.get_item(i)?.extract()?;
                        let vec16: Vec<half::f16> =
                            vec.into_iter().map(half::f16::from_f32).collect();
                        tx.insert_with_id(ids[i], &vec16, pyobject_to_json(&payload_obj));
                    }
                    db.commit_tx(tx).map(|_| ()).map_err(|error| {
                        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
                    })
                }
                DbBackend::U64(db) => {
                    let mut tx = crate::database::TxBuilder::new();
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec: Vec<u64> = vectors.get_item(i)?.extract()?;
                        tx.insert_with_id(ids[i], &vec, pyobject_to_json(&payload_obj));
                    }
                    db.commit_tx(tx).map(|_| ()).map_err(|error| {
                        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
                    })
                }
            }
        }

        /// 执行 TQL (Trivium Query Language) 统一查询
        ///
        /// 支持三种入口：MATCH (图遍历) / FIND (文档过滤) / SEARCH (向量检索)
        ///
        /// 示例：
        /// ```python
        /// # 图遍历
        /// rows = db.tql('MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b')
        /// for row in rows:
        ///     node = row.row["b"]   # {"id": ..., "payload": {...}}
        ///
        /// # 文档过滤
        /// rows = db.tql('FIND {type: "event", heat: {$gte: 0.7}} RETURN *')
        /// ```
        fn tql(&self, py: Python<'_>, query: &str) -> PyResult<Vec<PyQueryRow>> {
            fn convert_rows<T: crate::VectorType>(
                py: Python<'_>,
                rows: Vec<std::collections::HashMap<String, crate::node::Node<T>>>,
            ) -> PyResult<Vec<PyQueryRow>> {
                let mut out = Vec::with_capacity(rows.len());
                for row in rows {
                    let py_row = PyDict::new(py);
                    for (var_name, node) in &row {
                        let node_dict = PyDict::new(py);
                        let _ = node_dict.set_item("id", node.id);
                        let _ = node_dict.set_item("payload", json_to_pyobject(py, &node.payload));
                        let _ = node_dict.set_item("num_edges", node.edges.len());
                        let _ = py_row.set_item(var_name, node_dict);
                    }
                    out.push(PyQueryRow {
                        row: py_row.into_any().unbind(),
                    });
                }
                Ok(out)
            }

            match &self.inner {
                DbBackend::F32(db) => {
                    let rows = db.tql(query).map_err(|e: crate::error::TriviumError| {
                        pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                    })?;
                    convert_rows(py, rows)
                }
                DbBackend::F16(db) => {
                    let rows = db.tql(query).map_err(|e: crate::error::TriviumError| {
                        pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                    })?;
                    convert_rows(py, rows)
                }
                DbBackend::U64(db) => {
                    let rows = db.tql(query).map_err(|e: crate::error::TriviumError| {
                        pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                    })?;
                    convert_rows(py, rows)
                }
            }
        }

        /// 执行 TQL 写操作（CREATE / SET / DELETE / DETACH DELETE）
        ///
        /// 返回 dict: {"affected": int, "created_ids": list[int]}
        ///
        /// 示例：
        /// ```python
        /// result = db.tql_mut('CREATE (a {name: "Alice", age: 30})')
        /// print(result["affected"])      # 1
        /// print(result["created_ids"])   # [1]
        ///
        /// db.tql_mut('MATCH (a {name: "Alice"}) SET a.age == 31')
        /// db.tql_mut('MATCH (a {name: "Alice"}) DELETE a')
        /// ```
        fn tql_mut(&mut self, py: Python<'_>, query: &str) -> PyResult<PyObject> {
            let result = dispatch!(self, mut db => db.tql_mut(query)).map_err(
                |e: crate::error::TriviumError| {
                    pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
                },
            )?;
            let dict = PyDict::new(py);
            let _ = dict.set_item("affected", result.affected);
            let created: Vec<u64> = result.created_ids;
            let _ = dict.set_item("created_ids", created);
            Ok(dict.into_any().unbind())
        }

        // ════════ Leiden 社区检测 ════════

        /// Leiden 社区聚类
        ///
        /// 基于图谱边结构进行 Leiden/Louvain 近似社区发现。
        /// 返回一个字典，包含:
        /// - communities: list[list[int]] — 每个社区的节点 ID 列表
        /// - centroids: dict[int, list[float]] — 社区质心向量（可选）
        /// - num_clusters: int — 发现的社区总数
        ///
        /// 示例：
        /// ```python
        /// result = db.leiden_cluster(min_community_size=3, max_iterations=15)
        /// for community in result["communities"]:
        ///     print(f"社区: {community}")
        /// ```
        #[pyo3(signature = (min_community_size=3, max_iterations=15, compute_centroids=true))]
        fn leiden_cluster(
            &self,
            py: Python<'_>,
            min_community_size: usize,
            max_iterations: usize,
            compute_centroids: bool,
        ) -> PyResult<PyObject> {
            let result = dispatch!(self, db => db.leiden_cluster(
                min_community_size,
                Some(max_iterations),
                Some(compute_centroids),
            ))
            .map_err(|e: crate::error::TriviumError| {
                pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
            })?;

            // 按社区分组: cluster_id -> [node_ids]
            let mut clusters: std::collections::HashMap<u32, Vec<u64>> =
                std::collections::HashMap::new();
            for (&node_id, &cluster_id) in &result.node_to_cluster {
                clusters.entry(cluster_id).or_default().push(node_id);
            }

            // 排序确保确定性输出
            let mut sorted_keys: Vec<u32> = clusters.keys().copied().collect();
            sorted_keys.sort_unstable();

            let communities = PyList::new(
                py,
                sorted_keys.iter().map(|k| {
                    let mut ids = clusters.get(k).cloned().unwrap_or_default();
                    ids.sort_unstable();
                    ids
                }),
            )?;

            let centroids_dict = PyDict::new(py);
            if compute_centroids {
                for &k in &sorted_keys {
                    if let Some(centroid) = result.centroids.get(&k) {
                        let _ = centroids_dict.set_item(k, centroid.clone());
                    }
                }
            }

            let out = PyDict::new(py);
            let _ = out.set_item("communities", communities);
            let _ = out.set_item("centroids", centroids_dict);
            let _ = out.set_item("num_clusters", result.num_clusters);
            Ok(out.into_any().unbind())
        }

        // ════════ 事务 ════════

        /// 开启一个轻量级事务，返回 PyTransaction 对象
        ///
        /// 支持上下文管理器风格：
        /// ```python
        /// with db.transaction() as tx:
        ///     tx.insert([1.0, 0.0], {"name": "Alice"})
        ///     tx.link(1, 2, label="knows")
        ///     # 正常退出 → 自动 commit
        ///     # 异常 → 自动 rollback
        /// ```
        fn transaction(slf: Py<Self>, py: Python<'_>) -> PyResult<PyTransaction> {
            let dtype = slf.borrow(py).dtype.clone();
            let builder = match dtype.as_str() {
                "f32" => TxBuilderBackend::F32(crate::database::TxBuilder::new()),
                "f16" => TxBuilderBackend::F16(crate::database::TxBuilder::new()),
                "u64" => TxBuilderBackend::U64(crate::database::TxBuilder::new()),
                _ => {
                    return Err(pyo3::exceptions::PyValueError::new_err(format!(
                        "不支持的 dtype: {}",
                        dtype
                    )));
                }
            };
            Ok(PyTransaction {
                db: slf,
                builder: Some(builder),
                finished: false,
            })
        }

        /// 显式关闭数据库（落盘后释放资源）
        fn close(&mut self) -> PyResult<()> {
            dispatch!(self, mut db => db.close()).map_err(|e: crate::error::TriviumError| {
                pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
            })
        }
    }

    // ════════════════════════════════════════════════════════
    //  PyTransaction — 基于 Rust TxBuilder 的事务绑定
    // ════════════════════════════════════════════════════════

    /// 按 dtype 分发的 TxBuilder 后端
    enum TxBuilderBackend {
        F32(crate::database::TxBuilder<f32>),
        F16(crate::database::TxBuilder<half::f16>),
        U64(crate::database::TxBuilder<u64>),
    }

    /// Python 侧的轻量级事务对象
    ///
    /// 底层直接使用 Rust TxBuilder 收集操作，commit 时调用 Database::commit_tx。
    /// 支持上下文管理器 (with 语句)。
    #[pyclass(name = "Transaction")]
    struct PyTransaction {
        db: Py<PyTriviumDB>,
        builder: Option<TxBuilderBackend>,
        finished: bool,
    }

    /// 检查事务是否已结束的辅助宏
    macro_rules! check_finished {
        ($self:expr) => {
            if $self.finished {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "事务已结束（已提交或已回滚），不能继续添加操作",
                ));
            }
        };
    }

    #[pymethods]
    impl PyTransaction {
        /// 缓冲一个插入操作
        fn insert(
            &mut self,
            _py: Python<'_>,
            vector: Vec<f64>,
            payload: &Bound<'_, PyAny>,
        ) -> PyResult<()> {
            check_finished!(self);
            let json = pyobject_to_json(payload);
            match self.builder.as_mut().expect("TxBuilder missing") {
                TxBuilderBackend::F32(b) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    b.insert(&v, json);
                }
                TxBuilderBackend::F16(b) => {
                    let v: Vec<half::f16> = vector
                        .iter()
                        .map(|&x| half::f16::from_f32(x as f32))
                        .collect();
                    b.insert(&v, json);
                }
                TxBuilderBackend::U64(b) => {
                    let v: Vec<u64> = vector.iter().map(|&x| x as u64).collect();
                    b.insert(&v, json);
                }
            }
            Ok(())
        }

        /// 缓冲一个带自定义 ID 的插入操作
        fn insert_with_id(
            &mut self,
            _py: Python<'_>,
            id: u64,
            vector: Vec<f64>,
            payload: &Bound<'_, PyAny>,
        ) -> PyResult<()> {
            check_finished!(self);
            let json = pyobject_to_json(payload);
            match self.builder.as_mut().expect("TxBuilder missing") {
                TxBuilderBackend::F32(b) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    b.insert_with_id(id, &v, json);
                }
                TxBuilderBackend::F16(b) => {
                    let v: Vec<half::f16> = vector
                        .iter()
                        .map(|&x| half::f16::from_f32(x as f32))
                        .collect();
                    b.insert_with_id(id, &v, json);
                }
                TxBuilderBackend::U64(b) => {
                    let v: Vec<u64> = vector.iter().map(|&x| x as u64).collect();
                    b.insert_with_id(id, &v, json);
                }
            }
            Ok(())
        }

        /// 缓冲一个连边操作
        #[pyo3(signature = (src, dst, label="related", weight=1.0))]
        fn link(&mut self, src: u64, dst: u64, label: &str, weight: f32) -> PyResult<()> {
            check_finished!(self);
            match self.builder.as_mut().expect("TxBuilder missing") {
                TxBuilderBackend::F32(b) => b.link(src, dst, label, weight),
                TxBuilderBackend::F16(b) => b.link(src, dst, label, weight),
                TxBuilderBackend::U64(b) => b.link(src, dst, label, weight),
            }
            Ok(())
        }

        /// 缓冲一个删除操作
        fn delete(&mut self, id: u64) -> PyResult<()> {
            check_finished!(self);
            match self.builder.as_mut().expect("TxBuilder missing") {
                TxBuilderBackend::F32(b) => b.delete(id),
                TxBuilderBackend::F16(b) => b.delete(id),
                TxBuilderBackend::U64(b) => b.delete(id),
            }
            Ok(())
        }

        /// 缓冲一个断边操作
        #[pyo3(signature = (src, dst, label=None))]
        fn unlink(&mut self, src: u64, dst: u64, label: Option<&str>) -> PyResult<()> {
            check_finished!(self);
            match (self.builder.as_mut().expect("TxBuilder missing"), label) {
                (TxBuilderBackend::F32(b), Some(label)) => b.unlink_label(src, dst, label),
                (TxBuilderBackend::F16(b), Some(label)) => b.unlink_label(src, dst, label),
                (TxBuilderBackend::U64(b), Some(label)) => b.unlink_label(src, dst, label),
                (TxBuilderBackend::F32(b), None) => b.unlink(src, dst),
                (TxBuilderBackend::F16(b), None) => b.unlink(src, dst),
                (TxBuilderBackend::U64(b), None) => b.unlink(src, dst),
            }
            Ok(())
        }

        /// 缓冲一个更新 payload 操作
        fn update_payload(
            &mut self,
            _py: Python<'_>,
            id: u64,
            payload: &Bound<'_, PyAny>,
        ) -> PyResult<()> {
            check_finished!(self);
            let json = pyobject_to_json(payload);
            match self.builder.as_mut().expect("TxBuilder missing") {
                TxBuilderBackend::F32(b) => b.update_payload(id, json),
                TxBuilderBackend::F16(b) => b.update_payload(id, json),
                TxBuilderBackend::U64(b) => b.update_payload(id, json),
            }
            Ok(())
        }

        /// 缓冲一个更新向量操作
        fn update_vector(&mut self, id: u64, vector: Vec<f64>) -> PyResult<()> {
            check_finished!(self);
            match self.builder.as_mut().expect("TxBuilder missing") {
                TxBuilderBackend::F32(b) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    b.update_vector(id, &v);
                }
                TxBuilderBackend::F16(b) => {
                    let v: Vec<half::f16> = vector
                        .iter()
                        .map(|&x| half::f16::from_f32(x as f32))
                        .collect();
                    b.update_vector(id, &v);
                }
                TxBuilderBackend::U64(b) => {
                    let v: Vec<u64> = vector.iter().map(|&x| x as u64).collect();
                    b.update_vector(id, &v);
                }
            }
            Ok(())
        }

        /// 当前事务中缓冲的操作数
        fn pending_count(&self) -> usize {
            match self.builder.as_ref() {
                Some(TxBuilderBackend::F32(b)) => b.pending_count(),
                Some(TxBuilderBackend::F16(b)) => b.pending_count(),
                Some(TxBuilderBackend::U64(b)) => b.pending_count(),
                None => 0,
            }
        }

        /// 原子提交事务：Dry-Run 预检 + WAL-first 写入
        fn commit(&mut self, py: Python<'_>) -> PyResult<Vec<u64>> {
            if self.finished {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "事务已结束（已提交或已回滚），不能重复提交",
                ));
            }
            self.finished = true;
            let builder = self.builder.take().expect("TxBuilder missing");
            let mut db_ref = self.db.borrow_mut(py);

            match (&mut db_ref.inner, builder) {
                (DbBackend::F32(db), TxBuilderBackend::F32(b)) => db
                    .commit_tx(b)
                    .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string())),
                (DbBackend::F16(db), TxBuilderBackend::F16(b)) => db
                    .commit_tx(b)
                    .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string())),
                (DbBackend::U64(db), TxBuilderBackend::U64(b)) => db
                    .commit_tx(b)
                    .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string())),
                _ => Err(pyo3::exceptions::PyRuntimeError::new_err("dtype 不匹配")),
            }
        }

        /// 回滚事务（丢弃所有缓冲操作）
        fn rollback(&mut self) -> PyResult<()> {
            if self.finished {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "事务已结束（已提交或已回滚），不能重复回滚",
                ));
            }
            self.finished = true;
            self.builder.take();
            Ok(())
        }

        // 上下文管理器支持
        fn __enter__(slf: Py<Self>) -> Py<Self> {
            slf
        }

        #[pyo3(signature = (exc_type=None, _exc_val=None, _exc_tb=None))]
        fn __exit__(
            &mut self,
            py: Python<'_>,
            exc_type: Option<&Bound<'_, PyAny>>,
            _exc_val: Option<&Bound<'_, PyAny>>,
            _exc_tb: Option<&Bound<'_, PyAny>>,
        ) -> PyResult<bool> {
            if self.finished {
                return Ok(false);
            }
            if exc_type.is_some() {
                self.finished = true;
                self.builder.take();
            } else {
                self.commit(py)?;
            }
            Ok(false)
        }

        fn __repr__(&self) -> String {
            format!(
                "Transaction(pending={}, finished={})",
                self.pending_count(),
                self.finished
            )
        }
    }

    // ════════════════════════════════════════════════════════
    //  PySearchHookWrapper — Python 原生 Hook 支持
    // ════════════════════════════════════════════════════════

    /// 将 Python 对象包装为 Rust SearchHook trait 实现
    ///
    /// Python 类只需实现感兴趣的方法（鸭子类型）：
    /// - on_pre_search(self, query_vector, config, ctx) -> None
    /// - on_post_recall(self, hits, ctx) -> None
    /// - on_rerank(self, hits, ctx) -> Optional[list]
    /// - on_post_search(self, hits, ctx) -> None
    struct PySearchHookWrapper {
        py_hook: PyObject,
    }

    // SAFETY: PyObject 本身是 Send+Sync（它只是一个引用计数指针），
    // 实际的 Python 调用在 with_gil 中执行，由 GIL 保证线程安全。
    unsafe impl Send for PySearchHookWrapper {}
    unsafe impl Sync for PySearchHookWrapper {}

    impl PySearchHookWrapper {
        /// 将 Rust Vec<SearchHit> 转换为 Python list[dict]
        fn hits_to_py(py: Python<'_>, hits: &[crate::node::SearchHit]) -> PyObject {
            let list = pyo3::types::PyList::new(
                py,
                hits.iter().map(|h| {
                    let d = PyDict::new(py);
                    let _ = d.set_item("id", h.id);
                    let _ = d.set_item("score", h.score);
                    let _ = d.set_item("payload", json_to_pyobject(py, &h.payload));
                    d
                }),
            )
            .expect("创建 Python list 失败");
            list.into_any().unbind()
        }

        /// 将 Python list[dict] 转换回 Rust Vec<SearchHit>
        fn py_to_hits(py: Python<'_>, obj: &PyObject) -> Vec<crate::node::SearchHit> {
            let mut hits = Vec::new();
            if let Ok(list) = obj.bind(py).downcast::<pyo3::types::PyList>() {
                for item in list.iter() {
                    if let Ok(dict) = item.downcast::<PyDict>() {
                        let id = dict
                            .get_item("id")
                            .ok()
                            .flatten()
                            .and_then(|v| v.extract::<u64>().ok())
                            .unwrap_or(0);
                        let score = dict
                            .get_item("score")
                            .ok()
                            .flatten()
                            .and_then(|v| v.extract::<f32>().ok())
                            .unwrap_or(0.0);
                        let payload = dict
                            .get_item("payload")
                            .ok()
                            .flatten()
                            .map(|v| pyobject_to_json(&v))
                            .unwrap_or(serde_json::Value::Null);
                        hits.push(crate::node::SearchHit { id, score, payload });
                    }
                }
            }
            hits
        }
    }

    impl crate::hook::SearchHook for PySearchHookWrapper {
        fn on_pre_search(
            &self,
            query_vector: &mut Vec<f32>,
            _config: &mut crate::database::SearchConfig,
            ctx: &mut crate::hook::HookContext,
        ) {
            pyo3::Python::with_gil(|py| {
                let hook = self.py_hook.bind(py);
                if let Ok(method) = hook.getattr("on_pre_search")
                    && let Ok(py_vec) = pyo3::types::PyList::new(py, query_vector.iter())
                {
                    let py_ctx = PyDict::new(py);
                    let _ = py_ctx.set_item("custom_data", json_to_pyobject(py, &ctx.custom_data));
                    let _ = py_ctx.set_item("abort", ctx.abort);

                    if let Ok(result) = method.call1((&py_vec, &py_ctx)) {
                        // 如果返回了新向量，替换之
                        if let Ok(new_vec) = result.extract::<Vec<f32>>() {
                            *query_vector = new_vec;
                        }
                        // 检查 ctx.abort 是否被修改
                        if let Ok(Some(abort_val)) = py_ctx.get_item("abort")
                            && let Ok(ab) = abort_val.extract::<bool>()
                        {
                            ctx.abort = ab;
                        }
                    }
                }
            });
        }

        fn on_post_recall(
            &self,
            hits: &mut Vec<crate::node::SearchHit>,
            ctx: &mut crate::hook::HookContext,
        ) {
            pyo3::Python::with_gil(|py| {
                let hook = self.py_hook.bind(py);
                if let Ok(method) = hook.getattr("on_post_recall") {
                    let py_hits = Self::hits_to_py(py, hits);
                    let py_ctx = PyDict::new(py);
                    let _ = py_ctx.set_item("custom_data", json_to_pyobject(py, &ctx.custom_data));

                    if let Ok(result) = method.call1((&py_hits, &py_ctx)) {
                        // 如果返回了列表，替换 hits
                        if !result.is_none() {
                            let obj = result.unbind();
                            *hits = Self::py_to_hits(py, &obj);
                        }
                    }
                }
            });
        }

        fn on_rerank(
            &self,
            hits: &mut Vec<crate::node::SearchHit>,
            ctx: &mut crate::hook::HookContext,
        ) -> Option<Vec<crate::node::SearchHit>> {
            pyo3::Python::with_gil(|py| {
                let hook = self.py_hook.bind(py);
                if let Ok(method) = hook.getattr("on_rerank") {
                    let py_hits = Self::hits_to_py(py, hits);
                    let py_ctx = PyDict::new(py);
                    let _ = py_ctx.set_item("custom_data", json_to_pyobject(py, &ctx.custom_data));

                    if let Ok(result) = method.call1((&py_hits, &py_ctx))
                        && !result.is_none()
                    {
                        let obj = result.unbind();
                        return Some(Self::py_to_hits(py, &obj));
                    }
                }
                None
            })
        }

        fn on_post_search(
            &self,
            hits: &mut Vec<crate::node::SearchHit>,
            ctx: &mut crate::hook::HookContext,
        ) {
            pyo3::Python::with_gil(|py| {
                let hook = self.py_hook.bind(py);
                if let Ok(method) = hook.getattr("on_post_search") {
                    let py_hits = Self::hits_to_py(py, hits);
                    let py_ctx = PyDict::new(py);
                    let _ = py_ctx.set_item("custom_data", json_to_pyobject(py, &ctx.custom_data));

                    if let Ok(result) = method.call1((&py_hits, &py_ctx))
                        && !result.is_none()
                    {
                        let obj = result.unbind();
                        *hits = Self::py_to_hits(py, &obj);
                    }
                }
            });
        }
    }

    #[pyfunction]
    pub fn init_logger() {
        use tracing_subscriber::{EnvFilter, fmt};
        let _ = fmt()
            .with_env_filter(
                EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()),
            )
            .try_init();
    }

    #[pymodule]
    pub fn triviumdb(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add("ReadOnlyError", m.py().get_type::<ReadOnlyError>())?;
        m.add(
            "RecoveryRequiredError",
            m.py().get_type::<RecoveryRequiredError>(),
        )?;
        m.add(
            "ImmutableArtifactError",
            m.py().get_type::<ImmutableArtifactError>(),
        )?;
        m.add(
            "GenerationBusyError",
            m.py().get_type::<GenerationBusyError>(),
        )?;
        m.add_class::<PyTriviumDB>()?;
        m.add_class::<PySearchHit>()?;
        m.add_class::<PyGroupedSearchResult>()?;
        m.add_class::<PyReachabilityStep>()?;
        m.add_class::<PyReachabilityResult>()?;
        m.add_class::<PyEdge>()?;
        m.add_class::<PyIncomingEdge>()?;
        m.add_class::<PyNodeView>()?;
        m.add_class::<PyQueryRow>()?;
        m.add_class::<PyHookContext>()?;
        m.add_class::<PyTransaction>()?;
        m.add_function(wrap_pyfunction!(init_logger, m)?)?;
        Ok(())
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
}
