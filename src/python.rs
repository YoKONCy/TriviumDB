#[cfg(feature = "python")]
pub mod python {
    use pyo3::prelude::*;
    use pyo3::types::{PyDict, PyList};
    use crate::database::Database as GenericDatabase;

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
        pub num_edges: usize,
    }

    // ════════ 辅助转换 ════════

    fn json_to_pyobject(py: Python<'_>, val: &serde_json::Value) -> PyObject {
        match val {
            serde_json::Value::Null => py.None(),
            serde_json::Value::Bool(b) => (*b).into_pyobject(py).unwrap().to_owned().into_any().unbind(),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    i.into_pyobject(py).unwrap().into_any().unbind()
                } else {
                    n.as_f64().unwrap_or(0.0).into_pyobject(py).unwrap().into_any().unbind()
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

    fn pyobject_to_json(py: Python<'_>, obj: &Bound<'_, PyAny>) -> serde_json::Value {
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
                    map.insert(key, pyobject_to_json(py, &v));
                }
            }
            serde_json::Value::Object(map)
        } else if let Ok(list) = obj.downcast::<PyList>() {
            let arr: Vec<serde_json::Value> = list.iter()
                .map(|item| pyobject_to_json(py, &item))
                .collect();
            serde_json::Value::Array(arr)
        } else {
            serde_json::Value::Null
        }
    }

    use crate::filter::Filter;

    fn dict_to_filter(py: Python<'_>, dict: &Bound<'_, PyDict>) -> PyResult<Filter> {
        let mut filters = Vec::new();
        for (k, v) in dict.iter() {
            let key = k.extract::<String>()?;
            
            if key == "$and" {
                if let Ok(list) = v.downcast::<PyList>() {
                    let sub_filters = list.iter().map(|item| {
                        let sub_dict = item.downcast::<PyDict>()?;
                        dict_to_filter(py, sub_dict)
                    }).collect::<PyResult<Vec<_>>>()?;
                    filters.push(Filter::And(sub_filters));
                }
                continue;
            }
            if key == "$or" {
                if let Ok(list) = v.downcast::<PyList>() {
                    let sub_filters = list.iter().map(|item| {
                        let sub_dict = item.downcast::<PyDict>()?;
                        dict_to_filter(py, sub_dict)
                    }).collect::<PyResult<Vec<_>>>()?;
                    filters.push(Filter::Or(sub_filters));
                }
                continue;
            }

            if let Ok(op_dict) = v.downcast::<PyDict>() {
                for (op_k, op_v) in op_dict.iter() {
                    let op_str = op_k.extract::<String>()?;
                    let val = pyobject_to_json(py, &op_v);
                    
                    let filter_op = match op_str.as_str() {
                        "$eq" => Filter::Eq(key.clone(), val),
                        "$ne" => Filter::Ne(key.clone(), val),
                        "$gt" => {
                            let n = val.as_f64().ok_or_else(|| pyo3::exceptions::PyValueError::new_err("$gt requires a number"))?;
                            Filter::Gt(key.clone(), n)
                        }
                        "$gte" => {
                            let n = val.as_f64().ok_or_else(|| pyo3::exceptions::PyValueError::new_err("$gte requires a number"))?;
                            Filter::Gte(key.clone(), n)
                        }
                        "$lt" => {
                            let n = val.as_f64().ok_or_else(|| pyo3::exceptions::PyValueError::new_err("$lt requires a number"))?;
                            Filter::Lt(key.clone(), n)
                        }
                        "$lte" => {
                            let n = val.as_f64().ok_or_else(|| pyo3::exceptions::PyValueError::new_err("$lte requires a number"))?;
                            Filter::Lte(key.clone(), n)
                        }
                        "$in" => {
                            if let serde_json::Value::Array(arr) = val {
                                Filter::In(key.clone(), arr)
                            } else {
                                return Err(pyo3::exceptions::PyValueError::new_err("$in requires a list"));
                            }
                        }
                        _ => return Err(pyo3::exceptions::PyValueError::new_err(format!("Unsupported operator: {}", op_str))),
                    };
                    filters.push(filter_op);
                }
            } else {
                let val = pyobject_to_json(py, &v);
                filters.push(Filter::Eq(key, val));
            }
        }
        
        if filters.is_empty() {
            Ok(Filter::Eq("none".into(), serde_json::Value::Null))
        } else if filters.len() == 1 {
            Ok(filters.pop().unwrap())
        } else {
            Ok(Filter::And(filters))
        }
    }

    fn parse_sync_mode(s: &str) -> PyResult<crate::storage::wal::SyncMode> {
        match s {
            "full" => Ok(crate::storage::wal::SyncMode::Full),
            "normal" => Ok(crate::storage::wal::SyncMode::Normal),
            "off" => Ok(crate::storage::wal::SyncMode::Off),
            _ => Err(pyo3::exceptions::PyValueError::new_err(
                "Unsupported sync_mode. Use 'full', 'normal', or 'off'"
            )),
        }
    }

    #[pymethods]
    impl PyTriviumDB {
        #[new]
        #[pyo3(signature = (path, dim=1536, dtype="f32", sync_mode="normal"))]
        fn new(path: &str, dim: usize, dtype: &str, sync_mode: &str) -> PyResult<Self> {
            let sm = parse_sync_mode(sync_mode)?;
            let inner = match dtype {
                "f32" => DbBackend::F32(GenericDatabase::<f32>::open_with_sync(path, dim, sm).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?),
                "f16" => DbBackend::F16(GenericDatabase::<half::f16>::open_with_sync(path, dim, sm).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?),
                "u64" => DbBackend::U64(GenericDatabase::<u64>::open_with_sync(path, dim, sm).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?),
                _ => return Err(pyo3::exceptions::PyValueError::new_err("Unsupported dtype. Use 'f32', 'f16', or 'u64'")),
            };
            Ok(Self { inner, dtype: dtype.to_string() })
        }

        /// 运行时切换 WAL 同步模式: "full" / "normal" / "off"
        fn set_sync_mode(&mut self, mode: &str) -> PyResult<()> {
            let sm = parse_sync_mode(mode)?;
            dispatch!(self, mut db => db.set_sync_mode(sm));
            Ok(())
        }

        fn insert(&mut self, py: Python<'_>, vector: Bound<'_, PyAny>, payload: &Bound<'_, PyAny>) -> PyResult<u64> {
            let json = pyobject_to_json(py, payload);
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    db.insert(&vec, json).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    db.insert(&vec16, json).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = vector.extract()?;
                    db.insert(&vec, json).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
                }
            }
        }

        fn insert_with_id(&mut self, py: Python<'_>, id: u64, vector: Bound<'_, PyAny>, payload: &Bound<'_, PyAny>) -> PyResult<()> {
            let json = pyobject_to_json(py, payload);
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    db.insert_with_id(id, &vec, json).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    db.insert_with_id(id, &vec16, json).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = vector.extract()?;
                    db.insert_with_id(id, &vec, json).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
                }
            }
        }

        #[pyo3(signature = (src, dst, label="related", weight=1.0))]
        fn link(&mut self, src: u64, dst: u64, label: &str, weight: f32) -> PyResult<()> {
            dispatch!(self, mut db => db.link(src, dst, label, weight)).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        }

        #[pyo3(signature = (query_vector, top_k=5, expand_depth=0, min_score=0.5))]
        fn search(&self, py: Python<'_>, query_vector: Bound<'_, PyAny>, top_k: usize, expand_depth: usize, min_score: f32) -> PyResult<Vec<PySearchHit>> {
            let results = match &self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = query_vector.extract()?;
                    db.search(&vec, top_k, expand_depth, min_score)
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = query_vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    db.search(&vec16, top_k, expand_depth, min_score)
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = query_vector.extract()?;
                    db.search(&vec, top_k, expand_depth, min_score)
                }
            }.map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

            Ok(results.into_iter().map(|h| PySearchHit {
                id: h.id,
                score: h.score,
                payload: json_to_pyobject(py, &h.payload),
            }).collect())
        }

        fn delete(&mut self, id: u64) -> PyResult<()> {
            dispatch!(self, mut db => db.delete(id))
                .map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        }

        fn unlink(&mut self, src: u64, dst: u64) -> PyResult<()> {
            dispatch!(self, mut db => db.unlink(src, dst))
                .map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        }

        fn update_payload(&mut self, py: Python<'_>, id: u64, payload: &Bound<'_, PyAny>) -> PyResult<()> {
            let json = pyobject_to_json(py, payload);
            dispatch!(self, mut db => db.update_payload(id, json))
                .map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        }

        fn update_vector(&mut self, vector: Bound<'_, PyAny>, id: u64) -> PyResult<()> {
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    db.update_vector(id, &vec).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
                }
                DbBackend::F16(db) => {
                    let vec: Vec<f32> = vector.extract()?;
                    let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                    db.update_vector(id, &vec16).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
                }
                DbBackend::U64(db) => {
                    let vec: Vec<u64> = vector.extract()?;
                    db.update_vector(id, &vec).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
                }
            }
        }

        fn get(&self, py: Python<'_>, id: u64) -> PyResult<Option<PyNodeView>> {
            match &self.inner {
                DbBackend::F32(db) => {
                    if let Some(n) = db.get(id) {
                        return Ok(Some(PyNodeView {
                            id: n.id,
                            vector: n.vector.into_pyobject(py).unwrap().into_any().unbind(),
                            payload: json_to_pyobject(py, &n.payload),
                            num_edges: n.edges.len(),
                        }))
                    }
                }
                DbBackend::F16(db) => {
                    if let Some(n) = db.get(id) {
                        let f32_vec: Vec<f32> = n.vector.into_iter().map(|f| f.to_f32()).collect();
                        return Ok(Some(PyNodeView {
                            id: n.id,
                            vector: f32_vec.into_pyobject(py).unwrap().into_any().unbind(),
                            payload: json_to_pyobject(py, &n.payload),
                            num_edges: n.edges.len(),
                        }))
                    }
                }
                DbBackend::U64(db) => {
                    if let Some(n) = db.get(id) {
                        return Ok(Some(PyNodeView {
                            id: n.id,
                            vector: n.vector.into_pyobject(py).unwrap().into_any().unbind(),
                            payload: json_to_pyobject(py, &n.payload),
                            num_edges: n.edges.len(),
                        }))
                    }
                }
            }
            Ok(None)
        }

        #[pyo3(signature = (id, depth=1))]
        fn neighbors(&self, id: u64, depth: usize) -> Vec<u64> {
            dispatch!(self, db => db.neighbors(id, depth))
        }

        fn node_count(&self) -> usize {
            dispatch!(self, db => db.node_count())
        }

        fn flush(&mut self) -> PyResult<()> {
            dispatch!(self, mut db => db.flush()).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))
        }

        fn dim(&self) -> usize {
            dispatch!(self, db => db.dim())
        }

        #[pyo3(signature = (interval_secs=30))]
        fn enable_auto_compaction(&mut self, interval_secs: u64) {
            dispatch!(self, mut db => db.enable_auto_compaction(std::time::Duration::from_secs(interval_secs)));
        }

        fn disable_auto_compaction(&mut self) {
            dispatch!(self, mut db => db.disable_auto_compaction());
        }

        /// 设置内存上限（MB），超出时自动 flush
        /// 设为 0 表示无限制
        #[pyo3(signature = (mb=0))]
        fn set_memory_limit(&mut self, mb: usize) {
            let bytes = mb * 1024 * 1024;
            dispatch!(self, mut db => db.set_memory_limit(bytes));
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
            format!("TriviumDB(dtype={}, nodes={}, dim={})", self.dtype, self.node_count(), self.dim())
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
            self.flush()?;
            Ok(false)
        }

        fn batch_insert(
            &mut self,
            py: Python<'_>,
            vectors: Bound<'_, PyList>,
            payloads: &Bound<'_, PyList>,
        ) -> PyResult<Vec<u64>> {
            if vectors.len() != payloads.len() {
                return Err(pyo3::exceptions::PyValueError::new_err("vectors and payloads must have the same length"));
            }
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let mut ids = Vec::with_capacity(vectors.len());
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec_obj = vectors.get_item(i)?;
                        let vec: Vec<f32> = vec_obj.extract()?;
                        let json = pyobject_to_json(py, &payload_obj);
                        let id = db.insert(&vec, json).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
                        ids.push(id);
                    }
                    Ok(ids)
                }
                DbBackend::F16(db) => {
                    let mut ids = Vec::with_capacity(vectors.len());
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec_obj = vectors.get_item(i)?;
                        let vec: Vec<f32> = vec_obj.extract()?;
                        let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                        let json = pyobject_to_json(py, &payload_obj);
                        let id = db.insert(&vec16, json).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
                        ids.push(id);
                    }
                    Ok(ids)
                }
                DbBackend::U64(db) => {
                    let mut ids = Vec::with_capacity(vectors.len());
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec_obj = vectors.get_item(i)?;
                        let vec: Vec<u64> = vec_obj.extract()?;
                        let json = pyobject_to_json(py, &payload_obj);
                        let id = db.insert(&vec, json).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
                        ids.push(id);
                    }
                    Ok(ids)
                }
            }
        }

        fn batch_insert_with_ids(
            &mut self,
            py: Python<'_>,
            ids: Vec<u64>,
            vectors: Bound<'_, PyList>,
            payloads: &Bound<'_, PyList>,
        ) -> PyResult<()> {
            if vectors.len() != payloads.len() || ids.len() != vectors.len() {
                return Err(pyo3::exceptions::PyValueError::new_err("ids, vectors and payloads must have the same length"));
            }
            
            match &mut self.inner {
                DbBackend::F32(db) => {
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec_obj = vectors.get_item(i)?;
                        let vec: Vec<f32> = vec_obj.extract()?;
                        let json = pyobject_to_json(py, &payload_obj);
                        db.insert_with_id(ids[i], &vec, json).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
                    }
                    Ok(())
                }
                DbBackend::F16(db) => {
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec_obj = vectors.get_item(i)?;
                        let vec: Vec<f32> = vec_obj.extract()?;
                        let vec16: Vec<half::f16> = vec.into_iter().map(half::f16::from_f32).collect();
                        let json = pyobject_to_json(py, &payload_obj);
                        db.insert_with_id(ids[i], &vec16, json).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
                    }
                    Ok(())
                }
                DbBackend::U64(db) => {
                    for (i, payload_obj) in payloads.iter().enumerate() {
                        let vec_obj = vectors.get_item(i)?;
                        let vec: Vec<u64> = vec_obj.extract()?;
                        let json = pyobject_to_json(py, &payload_obj);
                        db.insert_with_id(ids[i], &vec, json).map_err(|e: crate::error::TriviumError| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
                    }
                    Ok(())
                }
            }
        }

        fn filter_where(&self, py: Python<'_>, condition: &Bound<'_, PyDict>) -> PyResult<Vec<PyNodeView>> {
            let filter = dict_to_filter(py, condition)?;
            let mut result_list = Vec::new();
            match &self.inner {
                DbBackend::F32(db) => {
                    for n in db.filter_where(&filter) {
                        result_list.push(PyNodeView {
                            id: n.id,
                            vector: n.vector.into_pyobject(py).unwrap().into_any().unbind(),
                            payload: json_to_pyobject(py, &n.payload),
                            num_edges: n.edges.len(),
                        });
                    }
                }
                DbBackend::F16(db) => {
                    for n in db.filter_where(&filter) {
                        let f32_vec: Vec<f32> = n.vector.into_iter().map(|f| f.to_f32()).collect();
                        result_list.push(PyNodeView {
                            id: n.id,
                            vector: f32_vec.into_pyobject(py).unwrap().into_any().unbind(),
                            payload: json_to_pyobject(py, &n.payload),
                            num_edges: n.edges.len(),
                        });
                    }
                }
                DbBackend::U64(db) => {
                    for n in db.filter_where(&filter) {
                        result_list.push(PyNodeView {
                            id: n.id,
                            vector: n.vector.into_pyobject(py).unwrap().into_any().unbind(),
                            payload: json_to_pyobject(py, &n.payload),
                            num_edges: n.edges.len(),
                        });
                    }
                }
            }
            Ok(result_list)
        }
    }

    #[pyfunction]
    pub fn init_logger() {
        use tracing_subscriber::{fmt, EnvFilter};
        let _ = fmt()
            .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
            .try_init();
    }

    #[pymodule]
    pub fn triviumdb(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add_class::<PyTriviumDB>()?;
        m.add_class::<PySearchHit>()?;
        m.add_class::<PyNodeView>()?;
        m.add_function(wrap_pyfunction!(init_logger, m)?)?;
        Ok(())
    }
}
