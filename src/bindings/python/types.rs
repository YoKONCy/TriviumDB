//! 查询结果与 Hook 上下文的语言数据类型。

use super::json::json_to_pyobject;
use pyo3::prelude::*;

pub(super) fn to_py_reachability(
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

pub(super) fn round_api_f32(value: f32) -> f64 {
    ((value as f64) * 1_000_000.0).round() / 1_000_000.0
}

pub(super) fn search_hit_to_python(
    py: Python<'_>,
    hit: crate::node::SearchHit,
) -> PyResult<PySearchHit> {
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
