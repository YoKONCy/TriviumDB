//! Python PyO3 公共绑定。
//!
//! 负责 Python 值转换、GIL 释放、上下文管理器和异常映射，不复制核心查询逻辑。批量
//! 计算在进入 Rust 后释放 GIL；`.pyi` 由已编译模块生成并通过 stubtest 防止签名漂移。

#[cfg(feature = "python")]
#[allow(clippy::module_inception)]
pub mod python {
    use crate::database::Database as GenericDatabase;
    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;
    use pyo3::types::{PyDict, PyList};

    pyo3::create_exception!(triviumdb, TriviumDBError, pyo3::exceptions::PyRuntimeError);
    pyo3::create_exception!(triviumdb, InvalidInputError, pyo3::exceptions::PyValueError);
    pyo3::create_exception!(triviumdb, CapacityError, pyo3::exceptions::PyMemoryError);
    pyo3::create_exception!(
        triviumdb,
        SnapshotDestinationError,
        pyo3::exceptions::PyFileExistsError
    );
    pyo3::create_exception!(triviumdb, ReadOnlyError, TriviumDBError);
    pyo3::create_exception!(triviumdb, RecoveryRequiredError, TriviumDBError);
    pyo3::create_exception!(triviumdb, ImmutableArtifactError, TriviumDBError);
    pyo3::create_exception!(triviumdb, GenerationBusyError, TriviumDBError);
    pyo3::create_exception!(triviumdb, UnsupportedDatabaseVersionError, TriviumDBError);
    pyo3::create_exception!(triviumdb, ResourceBudgetError, TriviumDBError);
    pyo3::create_exception!(triviumdb, SnapshotPublicationError, TriviumDBError);

    #[pyclass(name = "ResourceRuntime", frozen)]
    pub struct PyResourceRuntime {
        inner: crate::resources::ResourceRuntime,
    }

    /// 控制台策略与取消状态不持有数据库或文件锁。
    #[pyclass(name = "ConsoleSession", frozen)]
    pub struct PyConsoleSession {
        inner: crate::ConsoleSession,
    }

    #[pymethods]
    impl PyConsoleSession {
        #[new]
        #[pyo3(signature = (options_json="{}"))]
        fn new(options_json: &str) -> PyResult<Self> {
            crate::ConsoleSession::from_json(options_json)
                .map(|inner| Self { inner })
                .map_err(to_py_error)
        }

        fn detach(&self) {
            self.inner.detach();
        }
        fn is_detached(&self) -> bool {
            self.inner.is_detached()
        }
        fn cancel(&self, request_id: &str) -> bool {
            self.inner.cancel(request_id)
        }
    }

    #[pymethods]
    impl PyResourceRuntime {
        #[new]
        #[pyo3(signature = (memory_limit_mb=0, max_threads=0, max_active_tasks=0, maintenance_memory_limit_mb=0))]
        fn new(
            memory_limit_mb: usize,
            max_threads: usize,
            max_active_tasks: usize,
            maintenance_memory_limit_mb: usize,
        ) -> PyResult<Self> {
            let memory_limit = memory_limit_mb.checked_mul(1024 * 1024).ok_or_else(|| {
                pyo3::exceptions::PyOverflowError::new_err("memory_limit_mb 换算溢出")
            })?;
            crate::resources::ResourceRuntime::new(crate::resources::ResourceLimits {
                maintenance_memory_limit: maintenance_memory_limit_mb
                    .checked_mul(1024 * 1024)
                    .ok_or_else(|| {
                        pyo3::exceptions::PyOverflowError::new_err("维护额度换算溢出")
                    })?,
                memory_limit,
                max_threads,
                max_active_tasks,
            })
            .map(|inner| Self { inner })
            .map_err(to_py_error)
        }

        fn stats(&self, py: Python<'_>) -> PyResult<PyObject> {
            let value = serde_json::to_value(self.inner.stats())
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }
    }

    fn to_py_error(error: crate::error::TriviumError) -> PyErr {
        use crate::error::TriviumError;
        let exception = match &error {
            TriviumError::SnapshotPublicationUncertain { .. } => {
                SnapshotPublicationError::new_err(error.to_string())
            }
            TriviumError::SnapshotDestinationExists { .. } => {
                SnapshotDestinationError::new_err(error.to_string())
            }
            TriviumError::ResourceBudgetExceeded { .. } => {
                ResourceBudgetError::new_err(error.to_string())
            }
            TriviumError::ReadOnlyViolation { .. } => ReadOnlyError::new_err(error.to_string()),
            TriviumError::RecoveryRequired { .. } => {
                RecoveryRequiredError::new_err(error.to_string())
            }
            TriviumError::ImmutableArtifactInvalid { .. } => {
                ImmutableArtifactError::new_err(error.to_string())
            }
            TriviumError::GenerationBusy { .. } => GenerationBusyError::new_err(error.to_string()),
            TriviumError::UnsupportedDatabaseVersion { .. } => {
                UnsupportedDatabaseVersionError::new_err(error.to_string())
            }
            TriviumError::CapacityReservationRejected { .. }
            | TriviumError::CapacityAllocationFailed { .. } => {
                CapacityError::new_err(error.to_string())
            }
            TriviumError::InvalidInput(_) => InvalidInputError::new_err(error.to_string()),
            _ => TriviumDBError::new_err(error.to_string()),
        };
        Python::with_gil(|py| -> PyResult<()> {
            let value = exception.value(py);
            value.setattr("code", error.code())?;
            let details = error.details();
            value.setattr("details", json_to_pyobject(py, &details)?)?;
            // 保留资源预算等已有的直接属性，字段的权威值来自内核。
            if let Some(fields) = details.as_object() {
                for (name, field) in fields {
                    value.setattr(name.as_str(), json_to_pyobject(py, field)?)?;
                }
            }
            Ok(())
        })
        .map_or_else(|conversion| conversion, |()| exception)
    }

    enum DbBackend {
        F32(GenericDatabase<f32>),
        F16(GenericDatabase<half::f16>),
        U64(GenericDatabase<u64>),
    }

    fn tql_row_to_json<T: crate::VectorType>(
        row: std::collections::HashMap<String, crate::query::tql_executor::TqlValue<T>>,
    ) -> serde_json::Value {
        use crate::query::tql_executor::TqlValue;
        let mut output = serde_json::Map::new();
        for (name, value) in row {
            let value = match value {
                TqlValue::Node(node) => serde_json::json!({
                    "id": node.id,
                    "payload": node.payload,
                    "num_edges": node.edges.len(),
                }),
                TqlValue::Edge(edge) => serde_json::json!({
                    "type": "edge",
                    "source": edge.source_id,
                    "target": edge.target_id,
                    "label": edge.label,
                    "weight": edge.weight,
                    "metadata": edge.metadata,
                }),
                TqlValue::Int(value) => serde_json::json!(value),
                TqlValue::UInt(value) => serde_json::json!(value),
                TqlValue::Float(value) => serde_json::json!(value),
                TqlValue::String(value) => serde_json::json!(value),
                TqlValue::Bool(value) => serde_json::json!(value),
                TqlValue::Path(value) => serde_json::json!(value),
                TqlValue::List(value) => serde_json::Value::Array(value),
                TqlValue::Object(value) => serde_json::Value::Object(value),
                TqlValue::Null => serde_json::Value::Null,
            };
            output.insert(name, value);
        }
        serde_json::Value::Object(output)
    }

    #[pyclass(name = "PreparedTql")]
    pub struct PyPreparedTql {
        inner: crate::query::tql_prepared::PreparedTql,
    }

    fn parse_row_overflow(value: &str) -> PyResult<crate::database::RowOverflowPolicy> {
        match value {
            "throw" => Ok(crate::database::RowOverflowPolicy::Throw),
            "break" => Ok(crate::database::RowOverflowPolicy::Break),
            _ => Err(pyo3::exceptions::PyValueError::new_err(
                "row_overflow 必须是 throw / break",
            )),
        }
    }

    #[pymethods]
    impl PyPreparedTql {
        fn parameter_names(&self) -> Vec<String> {
            self.inner
                .parameter_names()
                .into_iter()
                .map(str::to_owned)
                .collect()
        }
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
        py: Python<'_>,
        result: crate::graph::reachability::ReachabilityResult,
    ) -> PyResult<PyReachabilityResult> {
        Ok(PyReachabilityResult {
            source_id: result.source_id,
            target_id: result.target_id,
            depth: result.depth,
            path: result.path,
            steps: result
                .steps
                .into_iter()
                .map(|step| {
                    Ok(PyReachabilityStep {
                        from_id: step.from,
                        to_id: step.to,
                        label: step.label,
                        weight: round_api_f32(step.weight),
                        metadata: json_to_pyobject(py, &step.metadata)?,
                    })
                })
                .collect::<PyResult<Vec<_>>>()?,
        })
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
    pub struct PyReachabilityStep {
        #[pyo3(get)]
        pub from_id: u64,
        #[pyo3(get)]
        pub to_id: u64,
        #[pyo3(get)]
        pub label: String,
        #[pyo3(get)]
        pub weight: f64,
        #[pyo3(get)]
        pub metadata: PyObject,
    }

    impl Clone for PyReachabilityStep {
        fn clone(&self) -> Self {
            Python::with_gil(|py| Self {
                from_id: self.from_id,
                to_id: self.to_id,
                label: self.label.clone(),
                weight: self.weight,
                metadata: self.metadata.clone_ref(py),
            })
        }
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
    pub struct PyEdge {
        #[pyo3(get)]
        pub target_id: u64,
        #[pyo3(get)]
        pub label: String,
        #[pyo3(get)]
        pub weight: f64,
        #[pyo3(get)]
        pub metadata: PyObject,
    }

    impl Clone for PyEdge {
        fn clone(&self) -> Self {
            Python::with_gil(|py| Self {
                target_id: self.target_id,
                label: self.label.clone(),
                weight: self.weight,
                metadata: self.metadata.clone_ref(py),
            })
        }
    }

    #[pyclass(name = "IncomingEdge")]
    pub struct PyIncomingEdge {
        #[pyo3(get)]
        pub source_id: u64,
        #[pyo3(get)]
        pub target_id: u64,
        #[pyo3(get)]
        pub label: String,
        #[pyo3(get)]
        pub weight: f64,
        #[pyo3(get)]
        pub metadata: PyObject,
    }

    fn round_api_f32(value: f32) -> f64 {
        ((value as f64) * 1_000_000.0).round() / 1_000_000.0
    }

    fn search_hit_to_python(py: Python<'_>, hit: crate::node::SearchHit) -> PyResult<PySearchHit> {
        Ok(PySearchHit {
            id: hit.id,
            score: hit.score,
            payload: json_to_pyobject(py, &hit.payload)?,
        })
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

    fn json_to_pyobject(py: Python<'_>, val: &serde_json::Value) -> PyResult<PyObject> {
        use pyo3::IntoPyObjectExt;
        match val {
            serde_json::Value::Null => Ok(py.None()),
            serde_json::Value::Bool(b) => b.into_py_any(py),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    i.into_py_any(py)
                } else if let Some(u) = n.as_u64() {
                    u.into_py_any(py)
                } else {
                    n.as_f64()
                        .ok_or_else(|| {
                            PyValueError::new_err("JSON 数值无效 (Invalid JSON number)")
                        })?
                        .into_py_any(py)
                }
            }
            serde_json::Value::String(s) => s.into_py_any(py),
            serde_json::Value::Array(arr) => {
                let values = arr
                    .iter()
                    .map(|v| json_to_pyobject(py, v))
                    .collect::<PyResult<Vec<_>>>()?;
                Ok(PyList::new(py, values)?.into_any().unbind())
            }
            serde_json::Value::Object(map) => {
                let dict = PyDict::new(py);
                for (k, v) in map {
                    dict.set_item(k, json_to_pyobject(py, v)?)?;
                }
                Ok(dict.into_any().unbind())
            }
        }
    }

    fn pyobject_to_json(obj: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
        fn convert(obj: &Bound<'_, PyAny>, depth: usize) -> PyResult<serde_json::Value> {
            use pyo3::exceptions::{PyOverflowError, PyTypeError};
            use pyo3::types::{PyBool, PyFloat, PyInt, PyString, PyTuple};
            if obj.is_none() {
                return Ok(serde_json::Value::Null);
            }
            if obj.is_instance_of::<PyBool>() {
                return Ok(obj.extract::<bool>()?.into());
            }
            if obj.is_instance_of::<PyInt>() {
                if let Ok(value) = obj.extract::<i64>() {
                    return Ok(value.into());
                }
                return obj
                    .extract::<u64>()
                    .map(serde_json::Value::from)
                    .map_err(|_| {
                        PyOverflowError::new_err(
                            "JSON 整数超出 i64/u64 范围 (JSON integer exceeds i64/u64 range)",
                        )
                    });
            }
            if obj.is_instance_of::<PyFloat>() {
                return serde_json::Number::from_f64(obj.extract()?)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| {
                        PyValueError::new_err("JSON 浮点数必须有限 (JSON floats must be finite)")
                    });
            }
            if obj.is_instance_of::<PyString>() {
                return Ok(obj.extract::<String>()?.into());
            }
            if depth >= 127 {
                return Err(PyValueError::new_err(
                    "JSON 容器嵌套超过 127 层 (JSON container nesting exceeds 127 levels)",
                ));
            }
            if let Ok(dict) = obj.downcast::<PyDict>() {
                let mut map = serde_json::Map::new();
                for (k, v) in dict.iter() {
                    if !k.is_instance_of::<PyString>() {
                        return Err(PyTypeError::new_err(
                            "JSON 对象键必须是字符串 (JSON object keys must be strings)",
                        ));
                    }
                    map.insert(k.extract::<String>()?, convert(&v, depth + 1)?);
                }
                return Ok(serde_json::Value::Object(map));
            }
            let values = if let Ok(list) = obj.downcast::<PyList>() {
                list.iter()
                    .map(|v| convert(&v, depth + 1))
                    .collect::<PyResult<Vec<_>>>()?
            } else if let Ok(tuple) = obj.downcast::<PyTuple>() {
                tuple
                    .iter()
                    .map(|v| convert(&v, depth + 1))
                    .collect::<PyResult<Vec<_>>>()?
            } else {
                return Err(PyTypeError::new_err(
                    "Payload 包含不支持的类型 (Payload contains an unsupported type)",
                ));
            };
            Ok(serde_json::Value::Array(values))
        }
        convert(obj, 0)
    }

    use crate::filter::Filter;

    fn dict_to_filter(_py: Python<'_>, dict: &Bound<'_, PyDict>) -> PyResult<Filter> {
        // 将 PyDict 转为 serde_json::Value，再统一调用 Filter::from_json
        let json_val = pyobject_to_json(&dict.clone().into_any())?;
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

    fn parse_edge_direction(value: &str) -> PyResult<crate::database::EdgeDirection> {
        match value {
            "out" | "outgoing" => Ok(crate::database::EdgeDirection::Outgoing),
            "in" | "incoming" => Ok(crate::database::EdgeDirection::Incoming),
            "both" => Ok(crate::database::EdgeDirection::Both),
            _ => Err(pyo3::exceptions::PyValueError::new_err(
                "edge_direction 必须是 out / in / both",
            )),
        }
    }

    #[pymethods]
    impl PyTriviumDB {
        #[new]
        #[pyo3(signature = (path, dim=1536, dtype="f32", sync_mode="normal", load_text_index=false, auto_build_quiver=true, expected_nodes=None, memory_limit_mb=0, payload_cache_mb=64, payload_cache_entry_mb=8, access_mode="read_write", missing_index_policy="fallback", max_query_rows=None, row_overflow="throw", runtime=None))]
        fn new(
            path: &str,
            dim: usize,
            dtype: &str,
            sync_mode: &str,
            load_text_index: bool,
            auto_build_quiver: bool,
            expected_nodes: Option<usize>,
            memory_limit_mb: usize,
            payload_cache_mb: usize,
            payload_cache_entry_mb: usize,
            access_mode: &str,
            missing_index_policy: &str,
            max_query_rows: Option<usize>,
            row_overflow: &str,
            runtime: Option<PyRef<'_, PyResourceRuntime>>,
        ) -> PyResult<Self> {
            let sm = parse_sync_mode(sync_mode)?;
            let memory_limit = memory_limit_mb.checked_mul(1024 * 1024).ok_or_else(|| {
                pyo3::exceptions::PyOverflowError::new_err("memory_limit_mb 换算字节时溢出")
            })?;
            let payload_cache_bytes =
                payload_cache_mb.checked_mul(1024 * 1024).ok_or_else(|| {
                    pyo3::exceptions::PyOverflowError::new_err("payload_cache_mb 换算字节时溢出")
                })?;
            let payload_cache_entry_bytes = payload_cache_entry_mb
                .checked_mul(1024 * 1024)
                .ok_or_else(|| {
                    pyo3::exceptions::PyOverflowError::new_err(
                        "payload_cache_entry_mb 换算字节时溢出",
                    )
                })?;
            let config = crate::database::Config {
                dim,
                sync_mode: sm,
                load_text_index,
                auto_build_quiver,
                expected_nodes,
                memory_limit,
                payload_cache_bytes,
                payload_cache_entry_bytes,
                access_mode: parse_access_mode(access_mode)?,
                missing_index_policy: parse_missing_index_policy(missing_index_policy)?,
                max_query_rows,
                row_overflow: parse_row_overflow(row_overflow)?,
                ..Default::default()
            };
            let runtime = runtime
                .map(|runtime| runtime.inner.clone())
                .unwrap_or_else(crate::resources::ResourceRuntime::shared_default);
            let inner = match dtype {
                "f32" => DbBackend::F32(
                    GenericDatabase::<f32>::open_with_runtime(path, config, runtime.clone())
                        .map_err(to_py_error)?,
                ),
                "f16" => DbBackend::F16(
                    GenericDatabase::<half::f16>::open_with_runtime(path, config, runtime.clone())
                        .map_err(to_py_error)?,
                ),
                "u64" => DbBackend::U64(
                    GenericDatabase::<u64>::open_with_runtime(path, config, runtime.clone())
                        .map_err(to_py_error)?,
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
            dispatch!(self, mut db => db.set_sync_mode(sm)).map_err(to_py_error)
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
            dispatch!(self, mut db => db.set_hook(ffi_hook)).map_err(to_py_error)
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
        fn set_hook(&mut self, hook: PyObject) -> PyResult<()> {
            let wrapper = PySearchHookWrapper { py_hook: hook };
            dispatch!(self, mut db => db.set_hook(wrapper)).map_err(to_py_error)
        }

        fn __traverse__(
            &self,
            visit: pyo3::PyVisit<'_>,
        ) -> std::result::Result<(), pyo3::PyTraverseError> {
            // GC 遍历不获取 GIL，也不等待正在关闭的 Hook 槽。
            dispatch!(self, db => db.try_with_hook(|hook| {
                // 原生查询仍持有 Hook 时，该引用属于 GC 图之外的活动根。
                if std::sync::Arc::strong_count(hook) == 1
                    && let Some(wrapper) = (hook.as_ref() as &dyn std::any::Any).downcast_ref::<PySearchHookWrapper>()
                {
                    visit.call(&wrapper.py_hook)?;
                }
                Ok(())
            })).unwrap_or(Ok(()))
        }

        fn __clear__(&mut self) {
            dispatch!(self, mut db => db.clear_hook());
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

        fn insert(
            &mut self,
            _py: Python<'_>,
            vector: Bound<'_, PyAny>,
            payload: &Bound<'_, PyAny>,
        ) -> PyResult<u64> {
            let json = pyobject_to_json(payload)?;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    db.insert(&vec, json).map_err(to_py_error)
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    db.insert(&vec16, json).map_err(to_py_error)
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = vector.extract()?;
                    db.insert(&vec, json).map_err(to_py_error)
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
            let json = pyobject_to_json(payload)?;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    db.insert_with_id(id, &vec, json)
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    db.insert_with_id(id, &vec16, json)
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = vector.extract()?;
                    db.insert_with_id(id, &vec, json)
                }
            }
            .map_err(to_py_error)
        }

        fn upsert_with_id(
            &mut self,
            id: u64,
            vector: Bound<'_, PyAny>,
            payload: &Bound<'_, PyAny>,
        ) -> PyResult<()> {
            let json = pyobject_to_json(payload)?;
            match &mut self.inner {
                DbBackend::F32(db) => db.upsert_with_id(id, &vector.extract::<Vec<f32>>()?, json),
                DbBackend::F16(db) => {
                    let vector = vector
                        .extract::<Vec<f32>>()?
                        .into_iter()
                        .map(half::f16::from_f32)
                        .collect::<Vec<_>>();
                    db.upsert_with_id(id, &vector, json)
                }
                DbBackend::U64(db) => db.upsert_with_id(id, &vector.extract::<Vec<u64>>()?, json),
            }
            .map_err(to_py_error)
        }

        #[pyo3(signature = (src, dst, label="related", weight=1.0))]
        fn link(&mut self, src: u64, dst: u64, label: &str, weight: f32) -> PyResult<()> {
            dispatch!(self, mut db => db.link(src, dst, label, weight)).map_err(to_py_error)
        }

        fn get_edge(
            &self,
            py: Python<'_>,
            src: u64,
            dst: u64,
            label: &str,
        ) -> PyResult<Option<PyEdge>> {
            Ok({
                dispatch!(self, db => db.get_edge(src, dst, label).map_err(to_py_error)?)
                    .map(|edge| -> PyResult<PyEdge> {
                        Ok(PyEdge {
                            target_id: edge.target_id,
                            label: edge.label,
                            weight: round_api_f32(edge.weight),
                            metadata: json_to_pyobject(py, &edge.metadata)?,
                        })
                    })
                    .transpose()?
            })
        }

        #[pyo3(signature = (src, dst, label, weight=1.0, metadata=None))]
        fn upsert_edge(
            &mut self,
            src: u64,
            dst: u64,
            label: &str,
            weight: f32,
            metadata: Option<&Bound<'_, PyAny>>,
        ) -> PyResult<()> {
            let metadata = metadata
                .map(pyobject_to_json)
                .transpose()?
                .unwrap_or(serde_json::Value::Null);
            dispatch!(self, mut db => db.upsert_edge(src, dst, label, weight, metadata))
                .map_err(to_py_error)
        }

        #[pyo3(signature = (src, dst, label, weight=None, metadata=None))]
        fn update_edge(
            &mut self,
            src: u64,
            dst: u64,
            label: &str,
            weight: Option<f32>,
            metadata: Option<&Bound<'_, PyAny>>,
        ) -> PyResult<()> {
            dispatch!(self, mut db => db.update_edge(
                src,
                dst,
                label,
                weight,
                metadata.map(pyobject_to_json).transpose()?,
            ))
            .map_err(to_py_error)
        }

        #[pyo3(signature = (query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=0, min_score=0.5, payload_filter=None, max_edges_per_node=0, min_edge_weight=0.0, edge_direction="out"))]
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
            max_edges_per_node: usize,
            min_edge_weight: f32,
            edge_direction: &str,
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
                max_edges_per_node,
                min_edge_weight,
                edge_direction: parse_edge_direction(edge_direction)?,
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

        #[pyo3(signature = (query_vector, top_k=5, recall_k=0, rerank_k=0, expand_depth=2, min_score=0.1, payload_filter=None, max_edges_per_node=0, min_edge_weight=0.0, edge_direction="out"))]
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
            max_edges_per_node: usize,
            min_edge_weight: f32,
            edge_direction: &str,
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
                max_edges_per_node,
                min_edge_weight,
                edge_direction: parse_edge_direction(edge_direction)?,
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
            expand_labels=None,
            max_edges_per_node=0,
            min_edge_weight=0.0,
            edge_direction="out"
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
            max_edges_per_node: usize,
            min_edge_weight: f32,
            edge_direction: &str,
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
                max_edges_per_node,
                min_edge_weight,
                edge_direction: parse_edge_direction(edge_direction)?,
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

        fn delete_many_atomic(&mut self, ids: Vec<u64>) -> PyResult<usize> {
            dispatch!(self, mut db => db.delete_many_atomic(&ids)).map_err(to_py_error)
        }

        fn compare_and_set_payload_field(
            &mut self,
            id: u64,
            field: &str,
            expected: &Bound<'_, PyAny>,
            replacement: &Bound<'_, PyAny>,
        ) -> PyResult<()> {
            let expected = pyobject_to_json(expected)?;
            let replacement = pyobject_to_json(replacement)?;
            dispatch!(self, mut db => db.compare_and_set_payload_field(id, field, &expected, replacement))
                .map_err(to_py_error)
        }

        fn delete(&mut self, id: u64) -> PyResult<()> {
            dispatch!(self, mut db => db.delete(id)).map_err(to_py_error)
        }

        #[pyo3(signature = (src, dst, label=None))]
        fn unlink(&mut self, src: u64, dst: u64, label: Option<&str>) -> PyResult<()> {
            match label {
                Some(label) => dispatch!(self, mut db => db.unlink_label(src, dst, label)),
                None => dispatch!(self, mut db => db.unlink(src, dst)),
            }
            .map_err(to_py_error)
        }

        fn update_payload(
            &mut self,
            _py: Python<'_>,
            id: u64,
            payload: &Bound<'_, PyAny>,
        ) -> PyResult<()> {
            let json = pyobject_to_json(payload)?;
            dispatch!(self, mut db => db.update_payload(id, json)).map_err(to_py_error)
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
            let json = pyobject_to_json(patch)?;
            dispatch!(self, mut db => db.patch_payload(id, json)).map_err(to_py_error)
        }
        fn update_vector(&mut self, vector: Bound<'_, PyAny>, id: u64) -> PyResult<()> {
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    db.update_vector(id, &vec).map_err(to_py_error)
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    db.update_vector(id, &vec16).map_err(to_py_error)
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = vector.extract()?;
                    db.update_vector(id, &vec).map_err(to_py_error)
                }
            }
        }

        fn index_text(&mut self, id: u64, text: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.index_text(id, text)).map_err(to_py_error)
        }

        fn index_keyword(&mut self, id: u64, keyword: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.index_keyword(id, keyword)).map_err(to_py_error)
        }

        fn build_text_index(&mut self) -> PyResult<()> {
            dispatch!(self, mut db => db.build_text_index()).map_err(to_py_error)
        }

        /// 从完整正文与关键词集合重建文本索引，通过 flush 或 close 持久化。
        fn rebuild_text_index(&mut self, documents: Vec<Bound<'_, PyDict>>) -> PyResult<()> {
            let documents = documents
                .iter()
                .map(|document| {
                    for key in document.keys() {
                        let key = key.extract::<String>()?;
                        if !matches!(key.as_str(), "id" | "text" | "keywords") {
                            return Err(PyValueError::new_err(format!("文本重建字段无效：{key}")));
                        }
                    }
                    Ok(crate::index::text::TextIndexDocument {
                        id: document
                            .get_item("id")?
                            .ok_or_else(|| PyValueError::new_err("文本重建记录缺少 id"))?
                            .extract()?,
                        text: document
                            .get_item("text")?
                            .ok_or_else(|| PyValueError::new_err("文本重建记录缺少 text"))?
                            .extract()?,
                        keywords: document
                            .get_item("keywords")?
                            .map(|value| value.extract())
                            .transpose()?
                            .unwrap_or_default(),
                    })
                })
                .collect::<PyResult<Vec<_>>>()?;
            dispatch!(self, mut db => db.rebuild_text_index(&documents)).map_err(to_py_error)
        }

        // ════════ 属性二级索引 ════════

        /// 创建属性索引：对指定 payload 字段建立倒排索引，加速 MATCH/FIND 查询
        ///
        /// 示例：
        /// ```python
        /// db.create_index("name")    # 之后 tql('FIND {name: "Alice"} RETURN *') 使用 O(1) 索引
        /// db.create_index("type")
        /// ```
        #[pyo3(signature = (equalities, max_results=10000))]
        fn indexed_lookup(
            &self,
            equalities: &Bound<'_, PyAny>,
            max_results: usize,
        ) -> PyResult<Vec<u64>> {
            let value = pyobject_to_json(equalities)?;
            let object = value.as_object().ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(
                    "equalities 必须是对象 (equalities must be an object)",
                )
            })?;
            let equalities = object
                .iter()
                .map(|(field, value)| (field.clone(), value.clone()))
                .collect::<Vec<_>>();
            dispatch!(self, db => db.indexed_lookup(&equalities, max_results)).map_err(to_py_error)
        }

        fn create_ngram_index(&mut self, field: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.create_ngram_index(field)).map_err(to_py_error)
        }

        fn drop_ngram_index(&mut self, field: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.drop_ngram_index(field)).map_err(to_py_error)
        }

        #[pyo3(signature = (field, needle, max_results=10000))]
        fn substring_lookup(
            &self,
            field: &str,
            needle: &str,
            max_results: usize,
        ) -> PyResult<Vec<u64>> {
            dispatch!(self, db => db.substring_lookup(field, needle, max_results))
                .map_err(to_py_error)
        }

        fn create_unique_index(&mut self, field: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.create_unique_index(field)).map_err(to_py_error)
        }

        fn create_unique_composite_index(&mut self, fields: Vec<String>) -> PyResult<()> {
            dispatch!(self, mut db => db.create_unique_composite_index(&fields))
                .map_err(to_py_error)
        }

        fn list_unique_indexes(&self) -> PyResult<Vec<Vec<String>>> {
            Ok(dispatch!(self, db => db.list_unique_indexes().map_err(to_py_error)?))
        }

        fn create_index(&mut self, field: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.create_index(field)).map_err(to_py_error)
        }

        fn create_ordered_index(&mut self, field: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.create_ordered_index(field)).map_err(to_py_error)
        }

        fn create_composite_index(&mut self, fields: Vec<String>) -> PyResult<()> {
            dispatch!(self, mut db => db.create_composite_index(&fields)).map_err(to_py_error)
        }

        fn create_bitmap_index(&mut self, field: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.create_bitmap_index(field)).map_err(to_py_error)
        }

        /// 删除属性索引（查询仍可用，退化为全扫描）
        fn drop_index(&mut self, field: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.drop_index(field)).map_err(to_py_error)
        }

        fn drop_ordered_index(&mut self, field: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.drop_ordered_index(field)).map_err(to_py_error)
        }

        fn drop_composite_index(&mut self, fields: Vec<String>) -> PyResult<()> {
            dispatch!(self, mut db => db.drop_composite_index(&fields)).map_err(to_py_error)
        }

        fn drop_bitmap_index(&mut self, field: &str) -> PyResult<()> {
            dispatch!(self, mut db => db.drop_bitmap_index(field)).map_err(to_py_error)
        }

        fn index_info(&self, py: Python<'_>) -> PyResult<PyObject> {
            let value =
                dispatch!(self, db => serde_json::to_value(db.index_info().map_err(to_py_error)?))
                    .map_err(|error| {
                        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
                    })?;
            json_to_pyobject(py, &value)
        }

        fn storage_info(&self, py: Python<'_>) -> PyResult<PyObject> {
            let value = dispatch!(self, db => serde_json::to_value(db.storage_info()))
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }

        // ════════ 轻量级单字段查询 ════════

        /// 获取节点的 payload（不含向量，比 get() 更轻量）
        fn get_payload(&self, py: Python<'_>, id: u64) -> PyResult<Option<PyObject>> {
            Ok({
                dispatch!(self, db => db.get_payload(id).map_err(to_py_error)?)
                    .map(|p| json_to_pyobject(py, &p))
                    .transpose()?
            })
        }

        /// 获取节点的出边列表
        fn get_edges(&self, py: Python<'_>, id: u64) -> PyResult<Vec<PyEdge>> {
            Ok({
                dispatch!(self, db => db.get_edges(id).map_err(to_py_error)?)
                    .into_iter()
                    .map(|e| {
                        Ok(PyEdge {
                            target_id: e.target_id,
                            label: e.label,
                            weight: round_api_f32(e.weight),
                            metadata: json_to_pyobject(py, &e.metadata)?,
                        })
                    })
                    .collect::<PyResult<Vec<_>>>()?
            })
        }

        #[pyo3(signature = (id, label=None))]
        fn get_incoming_edges(
            &self,
            py: Python<'_>,
            id: u64,
            label: Option<&str>,
        ) -> PyResult<Vec<PyIncomingEdge>> {
            Ok({
                dispatch!(self, db => db.get_incoming_edges(id, label).map_err(to_py_error)?)
                    .into_iter()
                    .map(|edge| {
                        Ok(PyIncomingEdge {
                            source_id: edge.source_id,
                            target_id: edge.target_id,
                            label: edge.label,
                            weight: round_api_f32(edge.weight),
                            metadata: json_to_pyobject(py, &edge.metadata)?,
                        })
                    })
                    .collect::<PyResult<Vec<_>>>()?
            })
        }

        fn get(&self, py: Python<'_>, id: u64) -> PyResult<Option<PyNodeView>> {
            match &self.inner {
                DbBackend::F32(db) => {
                    if let Some(n) = db.get(id).map_err(to_py_error)? {
                        return Ok(Some(PyNodeView {
                            id: n.id,
                            vector: n.vector.into_pyobject(py).unwrap().into_any().unbind(),
                            payload: json_to_pyobject(py, &n.payload)?,
                            edges: n
                                .edges
                                .iter()
                                .map(|e| {
                                    Ok(PyEdge {
                                        target_id: e.target_id,
                                        label: e.label.clone(),
                                        weight: round_api_f32(e.weight),
                                        metadata: json_to_pyobject(py, &e.metadata)?,
                                    })
                                })
                                .collect::<PyResult<Vec<_>>>()?,
                            num_edges: n.edges.len(),
                        }));
                    }
                }
                DbBackend::F16(db) => {
                    if let Some(n) = db.get(id).map_err(to_py_error)? {
                        let f32_vec: Vec<f32> = n.vector.into_iter().map(|f| f.to_f32()).collect();
                        return Ok(Some(PyNodeView {
                            id: n.id,
                            vector: f32_vec.into_pyobject(py).unwrap().into_any().unbind(),
                            payload: json_to_pyobject(py, &n.payload)?,
                            edges: n
                                .edges
                                .iter()
                                .map(|e| {
                                    Ok(PyEdge {
                                        target_id: e.target_id,
                                        label: e.label.clone(),
                                        weight: round_api_f32(e.weight),
                                        metadata: json_to_pyobject(py, &e.metadata)?,
                                    })
                                })
                                .collect::<PyResult<Vec<_>>>()?,
                            num_edges: n.edges.len(),
                        }));
                    }
                }
                DbBackend::U64(db) => {
                    if let Some(n) = db.get(id).map_err(to_py_error)? {
                        return Ok(Some(PyNodeView {
                            id: n.id,
                            vector: n.vector.into_pyobject(py).unwrap().into_any().unbind(),
                            payload: json_to_pyobject(py, &n.payload)?,
                            edges: n
                                .edges
                                .iter()
                                .map(|e| {
                                    Ok(PyEdge {
                                        target_id: e.target_id,
                                        label: e.label.clone(),
                                        weight: round_api_f32(e.weight),
                                        metadata: json_to_pyobject(py, &e.metadata)?,
                                    })
                                })
                                .collect::<PyResult<Vec<_>>>()?,
                            num_edges: n.edges.len(),
                        }));
                    }
                }
            }
            Ok(None)
        }

        #[pyo3(signature = (id, depth=1, labels=None))]
        fn neighbors(
            &self,
            id: u64,
            depth: usize,
            labels: Option<Vec<String>>,
        ) -> PyResult<Vec<u64>> {
            Ok({
                dispatch!(self, db => db.neighbors_with_labels(id, depth, labels.as_deref()).map_err(to_py_error)?)
            })
        }

        #[pyo3(signature = (id, min_depth=1, max_depth=1, labels=None, direction="outgoing", max_visited_nodes=10_000, max_results=10_000, max_edges=50_000))]
        fn reachable(
            &self,
            py: Python<'_>,
            id: u64,
            min_depth: usize,
            max_depth: usize,
            labels: Option<Vec<String>>,
            direction: &str,
            max_visited_nodes: usize,
            max_results: usize,
            max_edges: usize,
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
                max_results,
                max_edges,
                max_frontier_size: 10_000,
                exhaustion_policy: crate::graph::budget::BudgetExhaustionPolicy::Partial,
            };
            dispatch!(self, db => db.reachable(id, &config))
                .map(|results| {
                    results
                        .into_iter()
                        .map(|result| to_py_reachability(py, result))
                        .collect::<PyResult<Vec<_>>>()
                })
                .map_err(to_py_error)?
        }

        #[pyo3(signature = (id, min_depth=1, max_depth=1, labels=None, direction="outgoing", max_visited_nodes=10_000, max_results=10_000, max_edges=50_000))]
        fn reachable_detailed(
            &self,
            py: Python<'_>,
            id: u64,
            min_depth: usize,
            max_depth: usize,
            labels: Option<Vec<String>>,
            direction: &str,
            max_visited_nodes: usize,
            max_results: usize,
            max_edges: usize,
        ) -> PyResult<PyObject> {
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
                max_results,
                max_edges,
                max_frontier_size: 10_000,
                exhaustion_policy: crate::graph::budget::BudgetExhaustionPolicy::Partial,
            };
            let output =
                dispatch!(self, db => db.reachable_detailed(id, &config)).map_err(to_py_error)?;
            let value = serde_json::to_value(output)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }

        #[pyo3(signature = (id, min_depth=1, max_depth=1, labels=None, direction="outgoing", max_visited_nodes=10_000, max_results=10_000, max_edges=50_000))]
        fn query_subgraph(
            &self,
            py: Python<'_>,
            id: u64,
            min_depth: usize,
            max_depth: usize,
            labels: Option<Vec<String>>,
            direction: &str,
            max_visited_nodes: usize,
            max_results: usize,
            max_edges: usize,
        ) -> PyResult<PyObject> {
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
                max_results,
                max_edges,
                max_frontier_size: 10_000,
                exhaustion_policy: crate::graph::budget::BudgetExhaustionPolicy::Partial,
            };
            let output =
                dispatch!(self, db => db.query_subgraph(id, &config)).map_err(to_py_error)?;
            let value = serde_json::to_value(output)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
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

        fn graph_stats(&self, py: Python<'_>) -> PyResult<PyObject> {
            let value =
                serde_json::to_value(dispatch!(self, db => db.graph_stats().map_err(to_py_error)?))
                    .map_err(|error| {
                        pyo3::exceptions::PyRuntimeError::new_err(error.to_string())
                    })?;
            json_to_pyobject(py, &value)
        }

        fn validate_graph(&self, py: Python<'_>) -> PyResult<PyObject> {
            let value = serde_json::to_value(
                dispatch!(self, db => db.validate_graph().map_err(to_py_error)?),
            )
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }

        fn repair_graph_indexes(&mut self, py: Python<'_>) -> PyResult<PyObject> {
            let report =
                dispatch!(self, mut db => db.repair_graph_indexes()).map_err(to_py_error)?;
            let value = serde_json::to_value(report)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }

        fn node_count(&self) -> PyResult<usize> {
            Ok(dispatch!(self, db => db.node_count().map_err(to_py_error)?))
        }

        fn flush(&mut self) -> PyResult<()> {
            dispatch!(self, mut db => db.flush()).map_err(to_py_error)
        }

        fn publish_generation_manifest(&mut self, generation_id: &str) -> PyResult<PyObject> {
            let manifest = dispatch!(self, mut db => db.publish_generation_manifest(generation_id))
                .map_err(to_py_error)?;
            let value = serde_json::to_value(manifest)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            Python::with_gil(|py| json_to_pyobject(py, &value))
        }

        fn dim(&self) -> usize {
            dispatch!(self, db => db.dim())
        }

        /// 获取所有活跃节点的 ID 列表
        fn all_node_ids(&self) -> PyResult<Vec<u64>> {
            Ok(dispatch!(self, db => db.all_node_ids().map_err(to_py_error)?))
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
                    let (_new_db, ids) = db.migrate_to(new_path, new_dim).map_err(to_py_error)?;
                    Ok(ids)
                }
                DbBackend::F16(db) => {
                    let (_new_db, ids) = db.migrate_to(new_path, new_dim).map_err(to_py_error)?;
                    Ok(ids)
                }
                DbBackend::U64(db) => {
                    let (_new_db, ids) = db.migrate_to(new_path, new_dim).map_err(to_py_error)?;
                    Ok(ids)
                }
            }
        }

        #[pyo3(signature = (interval_secs=7200))]
        fn enable_auto_compaction(&mut self, interval_secs: u64) -> PyResult<()> {
            dispatch!(self, mut db => db.enable_auto_compaction(std::time::Duration::from_secs(interval_secs)))
                .map_err(to_py_error)
        }

        fn set_auto_build_quiver(&mut self, enabled: bool) -> PyResult<()> {
            dispatch!(self, mut db => db.set_auto_build_quiver(enabled)).map_err(to_py_error)
        }

        fn clear_search_state(&self) -> PyResult<()> {
            dispatch!(self, db => db.clear_search_state()).map_err(to_py_error)
        }

        fn disable_auto_compaction(&mut self) {
            dispatch!(self, mut db => db.disable_auto_compaction());
        }

        fn compact(&mut self) -> PyResult<()> {
            dispatch!(self, mut db => db.compact()).map_err(to_py_error)
        }

        /// 为后续插入主动预留额外节点容量。
        fn reserve_nodes(&mut self, additional: usize) -> PyResult<()> {
            dispatch!(self, db => db.reserve_nodes(additional)).map_err(to_py_error)
        }

        /// 设置内核内存预算（MiB），0 表示不限制。
        #[pyo3(signature = (mb=0))]
        fn set_memory_limit(&mut self, mb: usize) -> PyResult<()> {
            let bytes = mb.checked_mul(1024 * 1024).ok_or_else(|| {
                pyo3::exceptions::PyOverflowError::new_err("内存上限换算字节时溢出")
            })?;
            dispatch!(self, mut db => db.set_memory_limit(bytes)).map_err(to_py_error)
        }

        /// 读取多个数据库共享的配额、工作线程和任务统计。
        fn resource_stats(&self, py: Python<'_>) -> PyResult<PyObject> {
            let stats = dispatch!(self, db => db.resource_stats());
            let value = serde_json::to_value(stats)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }

        fn instance_stats(&self, py: Python<'_>) -> PyResult<PyObject> {
            let stats = dispatch!(self, db => db.instance_stats());
            let value = serde_json::to_value(stats)
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
            json_to_pyobject(py, &value)
        }

        fn estimated_memory(&self) -> usize {
            dispatch!(self, db => db.estimated_memory())
        }

        fn __len__(&self) -> PyResult<usize> {
            self.node_count()
        }

        fn __contains__(&self, id: u64) -> PyResult<bool> {
            Ok(dispatch!(self, db => db.contains(id).map_err(to_py_error)?))
        }

        fn __repr__(&self) -> String {
            format!(
                "TriviumDB(dtype={}, nodes={}, dim={})",
                self.dtype,
                self.node_count()
                    .map(|count| count.to_string())
                    .unwrap_or_else(|_| "已关闭".into()),
                self.dim()
            )
        }

        fn __enter__(slf: Py<Self>) -> Py<Self> {
            slf
        }

        #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
        fn __exit__(
            &mut self,
            py: Python<'_>,
            _exc_type: Option<&Bound<'_, PyAny>>,
            _exc_val: Option<&Bound<'_, PyAny>>,
            _exc_tb: Option<&Bound<'_, PyAny>>,
        ) -> PyResult<bool> {
            self.close(py)?;
            Ok(false)
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
                        tx.insert(&vec, pyobject_to_json(&payload_obj)?);
                    }
                    db.commit_tx(tx).map_err(to_py_error)
                }
                DbBackend::F16(db) => {
                    let mut tx = crate::database::TxBuilder::new();
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec_obj = vectors.get_item(i)?;
                        let vec: Vec<f32> = vec_obj.extract()?;
                        let vec16: Vec<half::f16> =
                            vec.into_iter().map(half::f16::from_f32).collect();
                        tx.insert(&vec16, pyobject_to_json(&payload_obj)?);
                    }
                    db.commit_tx(tx).map_err(to_py_error)
                }
                DbBackend::U64(db) => {
                    let mut tx = crate::database::TxBuilder::new();
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec_obj = vectors.get_item(i)?;
                        let vec: Vec<u64> = vec_obj.extract()?;
                        tx.insert(&vec, pyobject_to_json(&payload_obj)?);
                    }
                    db.commit_tx(tx).map_err(to_py_error)
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
                        tx.insert_with_id(ids[i], &vec, pyobject_to_json(&payload_obj)?);
                    }
                    db.commit_tx(tx).map(|_| ()).map_err(to_py_error)
                }
                DbBackend::F16(db) => {
                    let mut tx = crate::database::TxBuilder::new();
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec: Vec<f32> = vectors.get_item(i)?.extract()?;
                        let vec16: Vec<half::f16> =
                            vec.into_iter().map(half::f16::from_f32).collect();
                        tx.insert_with_id(ids[i], &vec16, pyobject_to_json(&payload_obj)?);
                    }
                    db.commit_tx(tx).map(|_| ()).map_err(to_py_error)
                }
                DbBackend::U64(db) => {
                    let mut tx = crate::database::TxBuilder::new();
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec: Vec<u64> = vectors.get_item(i)?.extract()?;
                        tx.insert_with_id(ids[i], &vec, pyobject_to_json(&payload_obj)?);
                    }
                    db.commit_tx(tx).map(|_| ()).map_err(to_py_error)
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
                rows: crate::query::tql_executor::TqlValueResult<T>,
            ) -> PyResult<Vec<PyQueryRow>> {
                use crate::query::tql_executor::TqlValue;
                let mut out = Vec::with_capacity(rows.len());
                for row in rows {
                    let py_row = PyDict::new(py);
                    for (name, value) in row {
                        match value {
                            TqlValue::Node(node) => {
                                let node_dict = PyDict::new(py);
                                let _ = node_dict.set_item("id", node.id);
                                let _ = node_dict
                                    .set_item("payload", json_to_pyobject(py, &node.payload)?);
                                let _ = node_dict.set_item("num_edges", node.edges.len());
                                let _ = py_row.set_item(name, node_dict);
                            }
                            TqlValue::Edge(edge) => {
                                let edge_dict = PyDict::new(py);
                                let _ = edge_dict.set_item("type", "edge");
                                let _ = edge_dict.set_item("source", edge.source_id);
                                let _ = edge_dict.set_item("target", edge.target_id);
                                let _ = edge_dict.set_item("label", edge.label);
                                let _ = edge_dict.set_item("weight", edge.weight);
                                let _ = edge_dict
                                    .set_item("metadata", json_to_pyobject(py, &edge.metadata)?);
                                let _ = py_row.set_item(name, edge_dict);
                            }
                            TqlValue::Int(value) => {
                                let _ = py_row.set_item(name, value);
                            }
                            TqlValue::UInt(value) => {
                                py_row.set_item(name, value)?;
                            }
                            TqlValue::Float(value) => {
                                let _ = py_row.set_item(name, value);
                            }
                            TqlValue::String(value) => {
                                let _ = py_row.set_item(name, value);
                            }
                            TqlValue::Bool(value) => {
                                let _ = py_row.set_item(name, value);
                            }
                            TqlValue::Path(value) => {
                                let _ = py_row.set_item(name, value);
                            }
                            TqlValue::List(value) => {
                                let _ = py_row.set_item(
                                    name,
                                    json_to_pyobject(py, &serde_json::Value::Array(value))?,
                                );
                            }
                            TqlValue::Object(value) => {
                                py_row.set_item(
                                    name,
                                    json_to_pyobject(py, &serde_json::Value::Object(value))?,
                                )?;
                            }
                            TqlValue::Null => {
                                let _ = py_row.set_item(name, py.None());
                            }
                        }
                    }
                    out.push(PyQueryRow {
                        row: py_row.into_any().unbind(),
                    });
                }
                Ok(out)
            }

            match &self.inner {
                DbBackend::F32(db) => {
                    let rows = db.tql_values(query).map_err(to_py_error)?;
                    convert_rows(py, rows)
                }
                DbBackend::F16(db) => {
                    let rows = db.tql_values(query).map_err(to_py_error)?;
                    convert_rows(py, rows)
                }
                DbBackend::U64(db) => {
                    let rows = db.tql_values(query).map_err(to_py_error)?;
                    convert_rows(py, rows)
                }
            }
        }

        fn prepare_tql(&self, query: &str) -> PyResult<PyPreparedTql> {
            dispatch!(self, db => db.prepare_tql(query))
                .map(|inner| PyPreparedTql { inner })
                .map_err(to_py_error)
        }

        fn execute_prepared_tql(
            &self,
            py: Python<'_>,
            prepared: &PyPreparedTql,
            parameters: &Bound<'_, PyDict>,
        ) -> PyResult<Vec<PyQueryRow>> {
            let mut values = std::collections::HashMap::new();
            for (key, value) in parameters.iter() {
                let name: String = key.extract()?;
                let json = pyobject_to_json(&value)?;
                values.insert(
                    name,
                    crate::query::tql_prepared::TqlParamValue::from_json(&json)
                        .map_err(to_py_error)?,
                );
            }
            let rows = match &self.inner {
                DbBackend::F32(db) => db
                    .execute_prepared_tql(&prepared.inner, &values)
                    .map(|rows| rows.into_iter().map(tql_row_to_json).collect::<Vec<_>>()),
                DbBackend::F16(db) => db
                    .execute_prepared_tql(&prepared.inner, &values)
                    .map(|rows| rows.into_iter().map(tql_row_to_json).collect::<Vec<_>>()),
                DbBackend::U64(db) => db
                    .execute_prepared_tql(&prepared.inner, &values)
                    .map(|rows| rows.into_iter().map(tql_row_to_json).collect::<Vec<_>>()),
            }
            .map_err(to_py_error)?;
            rows.into_iter()
                .map(|row| {
                    let value = json_to_pyobject(py, &row)?;
                    Ok(PyQueryRow { row: value })
                })
                .collect()
        }

        /// 原样传输 JSON 字符串，避免宿主 JavaScript 对 u64 的中间降精度。
        fn console_request(
            &mut self,
            py: Python<'_>,
            session: &PyConsoleSession,
            request_json: &str,
        ) -> PyResult<String> {
            let session = session.inner.clone();
            py.allow_threads(
                || dispatch!(self, mut db => db.console_request(&session, request_json)),
            )
            .map_err(to_py_error)
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
            let result = dispatch!(self, mut db => db.tql_mut(query)).map_err(to_py_error)?;
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
            .map_err(to_py_error)?;

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

        fn request_shutdown(&mut self, py: Python<'_>) -> PyResult<PyObject> {
            let status = dispatch!(self, mut db => db.request_shutdown()).map_err(to_py_error)?;
            let value = serde_json::to_value(status)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
            json_to_pyobject(py, &value)
        }

        fn shutdown_status(&self, py: Python<'_>) -> PyResult<PyObject> {
            let value = serde_json::to_value(dispatch!(self, db => db.shutdown_status()))
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
            json_to_pyobject(py, &value)
        }

        fn close_timeout(&mut self, py: Python<'_>, timeout_ms: u64) -> PyResult<bool> {
            py.allow_threads(|| dispatch!(self, mut db => db.close_timeout(std::time::Duration::from_millis(timeout_ms))))
                .map_err(to_py_error)
        }

        fn create_snapshot(
            &mut self,
            py: Python<'_>,
            destination: &str,
            generation_id: &str,
        ) -> PyResult<PyObject> {
            let manifest = py
                .allow_threads(
                    || dispatch!(self, mut db => db.create_snapshot(destination, generation_id)),
                )
                .map_err(to_py_error)?;
            let value = serde_json::to_value(manifest)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
            json_to_pyobject(py, &value)
        }

        /// 显式关闭数据库（落盘后释放资源）
        fn close(&mut self, py: Python<'_>) -> PyResult<()> {
            match py.allow_threads(|| dispatch!(self, mut db => db.close())) {
                Ok(()) | Err(crate::error::TriviumError::DatabaseClosed) => Ok(()),
                Err(error) => Err(to_py_error(error)),
            }
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
            vector: Bound<'_, PyAny>,
            payload: &Bound<'_, PyAny>,
        ) -> PyResult<()> {
            check_finished!(self);
            let json = pyobject_to_json(payload)?;
            match self.builder.as_mut().expect("TxBuilder missing") {
                TxBuilderBackend::F32(b) => {
                    let v: Vec<f32> = vector.extract()?;
                    b.insert(&v, json);
                }
                TxBuilderBackend::F16(b) => {
                    let values: Vec<f32> = vector.extract()?;
                    let v: Vec<half::f16> = values.into_iter().map(half::f16::from_f32).collect();
                    b.insert(&v, json);
                }
                TxBuilderBackend::U64(b) => {
                    let v: Vec<u64> = vector.extract()?;
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
            vector: Bound<'_, PyAny>,
            payload: &Bound<'_, PyAny>,
        ) -> PyResult<()> {
            check_finished!(self);
            let json = pyobject_to_json(payload)?;
            match self.builder.as_mut().expect("TxBuilder missing") {
                TxBuilderBackend::F32(b) => {
                    let v: Vec<f32> = vector.extract()?;
                    b.insert_with_id(id, &v, json);
                }
                TxBuilderBackend::F16(b) => {
                    let values: Vec<f32> = vector.extract()?;
                    let v: Vec<half::f16> = values.into_iter().map(half::f16::from_f32).collect();
                    b.insert_with_id(id, &v, json);
                }
                TxBuilderBackend::U64(b) => {
                    let v: Vec<u64> = vector.extract()?;
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
            let json = pyobject_to_json(payload)?;
            match self.builder.as_mut().expect("TxBuilder missing") {
                TxBuilderBackend::F32(b) => b.update_payload(id, json),
                TxBuilderBackend::F16(b) => b.update_payload(id, json),
                TxBuilderBackend::U64(b) => b.update_payload(id, json),
            }
            Ok(())
        }

        /// 缓冲一个更新向量操作
        fn update_vector(&mut self, id: u64, vector: Bound<'_, PyAny>) -> PyResult<()> {
            check_finished!(self);
            match self.builder.as_mut().expect("TxBuilder missing") {
                TxBuilderBackend::F32(b) => {
                    let v: Vec<f32> = vector.extract()?;
                    b.update_vector(id, &v);
                }
                TxBuilderBackend::F16(b) => {
                    let values: Vec<f32> = vector.extract()?;
                    let v: Vec<half::f16> = values.into_iter().map(half::f16::from_f32).collect();
                    b.update_vector(id, &v);
                }
                TxBuilderBackend::U64(b) => {
                    let v: Vec<u64> = vector.extract()?;
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
                (DbBackend::F32(db), TxBuilderBackend::F32(b)) => {
                    db.commit_tx(b).map_err(to_py_error)
                }
                (DbBackend::F16(db), TxBuilderBackend::F16(b)) => {
                    db.commit_tx(b).map_err(to_py_error)
                }
                (DbBackend::U64(db), TxBuilderBackend::U64(b)) => {
                    db.commit_tx(b).map_err(to_py_error)
                }
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
        fn record_python_error(ctx: &mut crate::hook::HookContext, stage: &str, error: PyErr) {
            ctx.error = Some(format!("Python Hook {stage} 执行失败: {error}"));
        }

        /// 将 Rust Vec<SearchHit> 转换为 Python list[dict]
        fn hits_to_py(py: Python<'_>, hits: &[crate::node::SearchHit]) -> PyResult<PyObject> {
            let values = hits
                .iter()
                .map(|h| -> PyResult<_> {
                    let d = PyDict::new(py);
                    d.set_item("id", h.id)?;
                    d.set_item("score", h.score)?;
                    d.set_item("payload", json_to_pyobject(py, &h.payload)?)?;
                    Ok(d)
                })
                .collect::<PyResult<Vec<_>>>()?;
            Ok(PyList::new(py, values)?.into_any().unbind())
        }

        fn py_to_hits(py: Python<'_>, obj: &PyObject) -> PyResult<Vec<crate::node::SearchHit>> {
            let list = obj.bind(py).downcast::<PyList>()?;
            list.iter()
                .map(|item| {
                    let dict = item.downcast::<PyDict>()?;
                    let required = |name: &str| {
                        dict.get_item(name)?.ok_or_else(|| {
                            PyValueError::new_err(format!(
                                "Hook 命中缺少字段 (Hook hit field missing): {name}"
                            ))
                        })
                    };
                    let id = required("id")?.extract::<u64>()?;
                    let score = required("score")?.extract::<f32>()?;
                    if id == 0 || !score.is_finite() {
                        return Err(PyValueError::new_err(
                            "Hook 命中 ID 或分数无效 (Invalid hook hit ID or score)",
                        ));
                    }
                    let payload = dict
                        .get_item("payload")?
                        .map(|v| pyobject_to_json(&v))
                        .transpose()?
                        .unwrap_or_default();
                    Ok(crate::node::SearchHit { id, score, payload })
                })
                .collect()
        }

        fn set_context_data(
            py: Python<'_>,
            dict: &Bound<'_, PyDict>,
            ctx: &mut crate::hook::HookContext,
        ) -> bool {
            match json_to_pyobject(py, &ctx.custom_data)
                .and_then(|v| dict.set_item("custom_data", v))
            {
                Ok(()) => true,
                Err(error) => {
                    Self::record_python_error(ctx, "context", error);
                    false
                }
            }
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
                    if !Self::set_context_data(py, &py_ctx, ctx) {
                        return;
                    }
                    let _ = py_ctx.set_item("abort", ctx.abort);

                    match method.call1((&py_vec, &py_ctx)) {
                        Ok(result) => {
                            if let Ok(new_vec) = result.extract::<Vec<f32>>() {
                                *query_vector = new_vec;
                            }
                            if let Ok(Some(abort_val)) = py_ctx.get_item("abort")
                                && let Ok(ab) = abort_val.extract::<bool>()
                            {
                                ctx.abort = ab;
                            }
                        }
                        Err(error) => Self::record_python_error(ctx, "on_pre_search", error),
                    }
                }
            });
        }

        fn on_custom_recall(
            &self,
            query_vector: &[f32],
            _config: &crate::database::SearchConfig,
            ctx: &mut crate::hook::HookContext,
        ) -> Option<Vec<crate::node::SearchHit>> {
            pyo3::Python::with_gil(|py| {
                let hook = self.py_hook.bind(py);
                let Ok(method) = hook.getattr("on_custom_recall") else {
                    return None;
                };
                let py_ctx = PyDict::new(py);
                if !Self::set_context_data(py, &py_ctx, ctx) {
                    return None;
                }
                match method.call1((query_vector.to_vec(), py_ctx)) {
                    Ok(result) if !result.is_none() => match Self::py_to_hits(py, &result.unbind())
                    {
                        Ok(hits) => Some(hits),
                        Err(error) => {
                            Self::record_python_error(ctx, "on_custom_recall", error);
                            None
                        }
                    },
                    Ok(_) => None,
                    Err(error) => {
                        Self::record_python_error(ctx, "on_custom_recall", error);
                        None
                    }
                }
            })
        }

        fn on_pre_graph_expand(
            &self,
            seeds: &mut Vec<crate::node::SearchHit>,
            ctx: &mut crate::hook::HookContext,
        ) {
            pyo3::Python::with_gil(|py| {
                let hook = self.py_hook.bind(py);
                let Ok(method) = hook.getattr("on_pre_graph_expand") else {
                    return;
                };
                let py_hits = match Self::hits_to_py(py, seeds) {
                    Ok(value) => value,
                    Err(error) => {
                        Self::record_python_error(ctx, "on_pre_graph_expand", error);
                        return;
                    }
                };
                let py_ctx = PyDict::new(py);
                if !Self::set_context_data(py, &py_ctx, ctx) {
                    return;
                }
                match method.call1((py_hits, py_ctx)) {
                    Ok(result) if !result.is_none() => {
                        match Self::py_to_hits(py, &result.unbind()) {
                            Ok(value) => *seeds = value,
                            Err(error) => {
                                Self::record_python_error(ctx, "on_pre_graph_expand", error)
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(error) => Self::record_python_error(ctx, "on_pre_graph_expand", error),
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
                    let py_hits = match Self::hits_to_py(py, hits) {
                        Ok(value) => value,
                        Err(error) => {
                            Self::record_python_error(ctx, "on_post_recall", error);
                            return;
                        }
                    };
                    let py_ctx = PyDict::new(py);
                    if !Self::set_context_data(py, &py_ctx, ctx) {
                        return;
                    }

                    match method.call1((&py_hits, &py_ctx)) {
                        Ok(result) if !result.is_none() => {
                            let obj = result.unbind();
                            match Self::py_to_hits(py, &obj) {
                                Ok(value) => *hits = value,
                                Err(error) => {
                                    Self::record_python_error(ctx, "on_post_recall", error)
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(error) => Self::record_python_error(ctx, "on_post_recall", error),
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
                    let py_hits = match Self::hits_to_py(py, hits) {
                        Ok(value) => value,
                        Err(error) => {
                            Self::record_python_error(ctx, "on_rerank", error);
                            return None;
                        }
                    };
                    let py_ctx = PyDict::new(py);
                    if !Self::set_context_data(py, &py_ctx, ctx) {
                        return None;
                    }
                    match method.call1((&py_hits, &py_ctx)) {
                        Ok(result) if !result.is_none() => {
                            return match Self::py_to_hits(py, &result.unbind()) {
                                Ok(hits) => Some(hits),
                                Err(error) => {
                                    Self::record_python_error(ctx, "on_rerank", error);
                                    None
                                }
                            };
                        }
                        Ok(_) => {}
                        Err(error) => Self::record_python_error(ctx, "on_rerank", error),
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
                    let py_hits = match Self::hits_to_py(py, hits) {
                        Ok(value) => value,
                        Err(error) => {
                            Self::record_python_error(ctx, "on_post_search", error);
                            return;
                        }
                    };
                    let py_ctx = PyDict::new(py);
                    if !Self::set_context_data(py, &py_ctx, ctx) {
                        return;
                    }
                    match method.call1((&py_hits, &py_ctx)) {
                        Ok(result) if !result.is_none() => {
                            match Self::py_to_hits(py, &result.unbind()) {
                                Ok(value) => *hits = value,
                                Err(error) => {
                                    Self::record_python_error(ctx, "on_post_search", error)
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(error) => Self::record_python_error(ctx, "on_post_search", error),
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

    #[pyfunction]
    #[pyo3(signature = (directory, runtime=None))]
    fn verify_snapshot(
        py: Python<'_>,
        directory: &str,
        runtime: Option<PyRef<'_, PyResourceRuntime>>,
    ) -> PyResult<PyObject> {
        let runtime = runtime
            .map(|r| r.inner.clone())
            .unwrap_or_else(crate::resources::ResourceRuntime::shared_default);
        let manifest = py
            .allow_threads(|| {
                crate::storage::snapshot::verify_snapshot_with_runtime(directory, &runtime)
            })
            .map_err(to_py_error)?;
        let value = serde_json::to_value(manifest)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        json_to_pyobject(py, &value)
    }

    #[pyfunction]
    #[pyo3(signature = (source, destination, runtime=None))]
    fn restore_snapshot(
        py: Python<'_>,
        source: &str,
        destination: &str,
        runtime: Option<PyRef<'_, PyResourceRuntime>>,
    ) -> PyResult<PyObject> {
        let runtime = runtime
            .map(|r| r.inner.clone())
            .unwrap_or_else(crate::resources::ResourceRuntime::shared_default);
        let manifest = py
            .allow_threads(|| {
                crate::storage::snapshot::restore_snapshot_with_runtime(
                    source,
                    destination,
                    &runtime,
                )
            })
            .map_err(to_py_error)?;
        let value = serde_json::to_value(manifest)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        json_to_pyobject(py, &value)
    }

    #[pymodule]
    pub fn triviumdb(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add("TriviumDBError", m.py().get_type::<TriviumDBError>())?;
        m.add("InvalidInputError", m.py().get_type::<InvalidInputError>())?;
        m.add("CapacityError", m.py().get_type::<CapacityError>())?;
        m.add(
            "SnapshotDestinationError",
            m.py().get_type::<SnapshotDestinationError>(),
        )?;
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
        m.add(
            "UnsupportedDatabaseVersionError",
            m.py().get_type::<UnsupportedDatabaseVersionError>(),
        )?;
        m.add(
            "ResourceBudgetError",
            m.py().get_type::<ResourceBudgetError>(),
        )?;
        m.add(
            "SnapshotPublicationError",
            m.py().get_type::<SnapshotPublicationError>(),
        )?;
        m.add_class::<PyResourceRuntime>()?;
        m.add_class::<PyConsoleSession>()?;
        m.add_class::<PyPreparedTql>()?;
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
        m.add_function(wrap_pyfunction!(verify_snapshot, m)?)?;
        m.add_function(wrap_pyfunction!(restore_snapshot, m)?)?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::round_api_f32;

        #[test]
        fn 核心错误完整映射到_python异常() {
            pyo3::prepare_freethreaded_python();
            pyo3::Python::with_gil(|py| {
                use pyo3::prelude::*;
                let json = py.import("json").unwrap();
                for (error, code, expected) in crate::error::tests::cases() {
                    let exception = super::to_py_error(error);
                    let value = exception.value(py);
                    assert_eq!(
                        value.getattr("code").unwrap().extract::<String>().unwrap(),
                        code
                    );
                    let expected = json.call_method1("loads", (expected.to_string(),)).unwrap();
                    assert!(
                        value.getattr("details").unwrap().eq(expected).unwrap(),
                        "{code}"
                    );
                }
            });
        }

        #[test]
        fn 边权重_api舍入到六位小数() {
            assert_eq!(round_api_f32(0.9f32), 0.9);
            assert_eq!(round_api_f32(0.12345678f32), 0.123457);
        }
    }
}
