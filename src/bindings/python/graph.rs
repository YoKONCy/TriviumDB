//! 边、遍历、子图、图统计与聚类入口。

use super::arguments::{
    boolean_argument, extract_argument, float_argument, integer_argument, integer_value,
    read_sequence, read_string, string_argument, write_float, write_id, write_string,
};
use super::database::{PyTriviumDB, database_read, database_write};
use super::errors::{attach_error_info, to_py_error, write_admission_error};
use super::json::{json_to_pyobject, write_json};
use super::types::{
    PyEdge, PyIncomingEdge, PyReachabilityResult, round_api_f32, to_py_reachability,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

#[pymethods]
impl PyTriviumDB {
    #[pyo3(signature = (src, dst, label=string_argument("related"), weight=float_argument(1.0)), text_signature="($self, src, dst, label='related', weight=1.0)")]
    fn link(
        slf: &Bound<'_, Self>,
        src: Bound<'_, PyAny>,
        dst: Bound<'_, PyAny>,
        label: Py<PyAny>,
        weight: Py<PyAny>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let src = write_id(&src, "src")?;
        let dst = write_id(&dst, "dst")?;
        let label: String = write_string(label.bind(slf.py()), "label")?;
        let weight = write_float(weight.bind(slf.py()), "weight")?;
        dispatch!(this, mut db => db.link(src, dst, &label, weight)).map_err(to_py_error)
    }

    fn get_edge(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        src: Bound<'_, PyAny>,
        dst: Bound<'_, PyAny>,
        label: Bound<'_, PyAny>,
    ) -> PyResult<Option<PyEdge>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let src = extract_argument(py, &src, "src", None)?;
        let dst = extract_argument(py, &dst, "dst", None)?;
        let label: String = extract_argument(py, &label, "label", None)?;
        Ok({
            dispatch!(this, db => db.get_edge(src, dst, &label).map_err(to_py_error)?)
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

    #[pyo3(signature = (src, dst, label, weight=float_argument(1.0), metadata=None), text_signature="($self, src, dst, label, weight=1.0, metadata=None)")]
    fn upsert_edge(
        slf: &Bound<'_, Self>,
        src: Bound<'_, PyAny>,
        dst: Bound<'_, PyAny>,
        label: Bound<'_, PyAny>,
        weight: Py<PyAny>,
        metadata: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let src = write_id(&src, "src")?;
        let dst = write_id(&dst, "dst")?;
        let label: String = write_string(&label, "label")?;
        let weight = write_float(weight.bind(slf.py()), "weight")?;
        let metadata = metadata
            .map(|value| write_json(value, "metadata"))
            .transpose()?
            .unwrap_or(serde_json::Value::Null);
        dispatch!(this, mut db => db.upsert_edge(src, dst, &label, weight, metadata))
            .map_err(to_py_error)
    }

    #[pyo3(signature = (src, dst, label, weight=None, metadata=None))]
    fn update_edge(
        slf: &Bound<'_, Self>,
        src: Bound<'_, PyAny>,
        dst: Bound<'_, PyAny>,
        label: Bound<'_, PyAny>,
        weight: Option<Bound<'_, PyAny>>,
        metadata: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let src = write_id(&src, "src")?;
        let dst = write_id(&dst, "dst")?;
        let label: String = write_string(&label, "label")?;
        let weight = weight
            .map(|value| write_float(&value, "weight"))
            .transpose()?;
        dispatch!(this, mut db => db.update_edge(
            src,
            dst,
            &label,
            weight,
            metadata.map(|value| write_json(value, "metadata")).transpose()?,
        ))
        .map_err(to_py_error)
    }

    #[pyo3(signature = (src, dst, label=None))]
    fn unlink(
        slf: &Bound<'_, Self>,
        src: Bound<'_, PyAny>,
        dst: Bound<'_, PyAny>,
        label: Option<Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let src = write_id(&src, "src")?;
        let dst = write_id(&dst, "dst")?;
        let label: Option<String> = label
            .map(|value| write_string(&value, "label"))
            .transpose()?;
        match label.as_deref() {
            Some(label) => dispatch!(this, mut db => db.unlink_label(src, dst, label)),
            None => dispatch!(this, mut db => db.unlink(src, dst)),
        }
        .map_err(to_py_error)
    }

    /// 获取节点的出边列表
    fn get_edges(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        id: Bound<'_, PyAny>,
    ) -> PyResult<Vec<PyEdge>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let id = extract_argument(py, &id, "id", None)?;
        Ok({
            dispatch!(this, db => db.get_edges(id).map_err(to_py_error)?)
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
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        id: Bound<'_, PyAny>,
        label: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Vec<PyIncomingEdge>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let id = extract_argument(py, &id, "id", None)?;
        let label: Option<String> = label
            .map(|value| extract_argument(py, value, "label", None))
            .transpose()?;
        Ok({
            dispatch!(this, db => db.get_incoming_edges(id, label.as_deref()).map_err(to_py_error)?)
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

    #[pyo3(signature = (id, depth=integer_argument(1), labels=None), text_signature="($self, id, depth=1, labels=None)")]
    fn neighbors(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        id: Bound<'_, PyAny>,
        depth: Py<PyAny>,
        labels: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Vec<u64>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let id = extract_argument(py, &id, "id", None)?;
        let depth = extract_argument(py, depth.bind(py), "depth", None)?;
        let labels: Option<Vec<String>> = labels
            .map(|value| read_sequence(value, "labels", read_string))
            .transpose()?;
        Ok({
            dispatch!(this, db => db.neighbors_with_labels(id, depth, labels.as_deref()).map_err(to_py_error)?)
        })
    }

    #[pyo3(signature = (id, min_depth=integer_argument(1), max_depth=integer_argument(1), labels=None, direction=string_argument("outgoing"), max_visited_nodes=integer_argument(10_000), max_results=integer_argument(10_000), max_edges=integer_argument(50_000)), text_signature="($self, id, min_depth=1, max_depth=1, labels=None, direction='outgoing', max_visited_nodes=10000, max_results=10000, max_edges=50000)")]
    fn reachable(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        id: Bound<'_, PyAny>,
        min_depth: Py<PyAny>,
        max_depth: Py<PyAny>,
        labels: Option<&Bound<'_, PyAny>>,
        direction: Py<PyAny>,
        max_visited_nodes: Py<PyAny>,
        max_results: Py<PyAny>,
        max_edges: Py<PyAny>,
    ) -> PyResult<Vec<PyReachabilityResult>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let id = extract_argument(py, &id, "id", None)?;
        let min_depth = extract_argument(py, min_depth.bind(py), "min_depth", None)?;
        let max_depth = extract_argument(py, max_depth.bind(py), "max_depth", None)?;
        let labels = labels
            .map(|value| read_sequence(value, "labels", read_string))
            .transpose()?;
        let direction: String = extract_argument(py, direction.bind(py), "direction", None)?;
        let max_visited_nodes =
            extract_argument(py, max_visited_nodes.bind(py), "max_visited_nodes", None)?;
        let max_results = extract_argument(py, max_results.bind(py), "max_results", None)?;
        let max_edges = extract_argument(py, max_edges.bind(py), "max_edges", None)?;
        let direction = match direction.as_str() {
            "outgoing" => crate::graph::reachability::ReachabilityDirection::Outgoing,
            "incoming" => crate::graph::reachability::ReachabilityDirection::Incoming,
            "both" => crate::graph::reachability::ReachabilityDirection::Both,
            _ => {
                let reason = "direction 必须是 outgoing / incoming / both (direction must be outgoing / incoming / both)";
                return Err(attach_error_info(
                    pyo3::exceptions::PyValueError::new_err(reason),
                    crate::TriviumError::InvalidInput(reason.into()).at_field("direction"),
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
        dispatch!(this, db => db.reachable(id, &config))
            .map(|results| {
                results
                    .into_iter()
                    .map(|result| to_py_reachability(py, result))
                    .collect::<PyResult<Vec<_>>>()
            })
            .map_err(to_py_error)?
    }

    #[pyo3(signature = (id, min_depth=integer_argument(1), max_depth=integer_argument(1), labels=None, direction=string_argument("outgoing"), max_visited_nodes=integer_argument(10_000), max_results=integer_argument(10_000), max_edges=integer_argument(50_000)), text_signature="($self, id, min_depth=1, max_depth=1, labels=None, direction='outgoing', max_visited_nodes=10000, max_results=10000, max_edges=50000)")]
    fn reachable_detailed(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        id: Bound<'_, PyAny>,
        min_depth: Py<PyAny>,
        max_depth: Py<PyAny>,
        labels: Option<&Bound<'_, PyAny>>,
        direction: Py<PyAny>,
        max_visited_nodes: Py<PyAny>,
        max_results: Py<PyAny>,
        max_edges: Py<PyAny>,
    ) -> PyResult<PyObject> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let id = extract_argument(py, &id, "id", None)?;
        let min_depth = extract_argument(py, min_depth.bind(py), "min_depth", None)?;
        let max_depth = extract_argument(py, max_depth.bind(py), "max_depth", None)?;
        let labels = labels
            .map(|value| read_sequence(value, "labels", read_string))
            .transpose()?;
        let direction: String = extract_argument(py, direction.bind(py), "direction", None)?;
        let max_visited_nodes =
            extract_argument(py, max_visited_nodes.bind(py), "max_visited_nodes", None)?;
        let max_results = extract_argument(py, max_results.bind(py), "max_results", None)?;
        let max_edges = extract_argument(py, max_edges.bind(py), "max_edges", None)?;
        let direction = match direction.as_str() {
            "outgoing" => crate::graph::reachability::ReachabilityDirection::Outgoing,
            "incoming" => crate::graph::reachability::ReachabilityDirection::Incoming,
            "both" => crate::graph::reachability::ReachabilityDirection::Both,
            _ => {
                let reason = "direction 必须是 outgoing / incoming / both (direction must be outgoing / incoming / both)";
                return Err(attach_error_info(
                    pyo3::exceptions::PyValueError::new_err(reason),
                    crate::TriviumError::InvalidInput(reason.into()).at_field("direction"),
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
            dispatch!(this, db => db.reachable_detailed(id, &config)).map_err(to_py_error)?;
        let value = serde_json::to_value(output)
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        json_to_pyobject(py, &value)
    }

    #[pyo3(signature = (id, min_depth=integer_argument(1), max_depth=integer_argument(1), labels=None, direction=string_argument("outgoing"), max_visited_nodes=integer_argument(10_000), max_results=integer_argument(10_000), max_edges=integer_argument(50_000)), text_signature="($self, id, min_depth=1, max_depth=1, labels=None, direction='outgoing', max_visited_nodes=10000, max_results=10000, max_edges=50000)")]
    fn query_subgraph(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        id: Bound<'_, PyAny>,
        min_depth: Py<PyAny>,
        max_depth: Py<PyAny>,
        labels: Option<&Bound<'_, PyAny>>,
        direction: Py<PyAny>,
        max_visited_nodes: Py<PyAny>,
        max_results: Py<PyAny>,
        max_edges: Py<PyAny>,
    ) -> PyResult<PyObject> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let id = extract_argument(py, &id, "id", None)?;
        let min_depth = extract_argument(py, min_depth.bind(py), "min_depth", None)?;
        let max_depth = extract_argument(py, max_depth.bind(py), "max_depth", None)?;
        let labels = labels
            .map(|value| read_sequence(value, "labels", read_string))
            .transpose()?;
        let direction: String = extract_argument(py, direction.bind(py), "direction", None)?;
        let max_visited_nodes =
            extract_argument(py, max_visited_nodes.bind(py), "max_visited_nodes", None)?;
        let max_results = extract_argument(py, max_results.bind(py), "max_results", None)?;
        let max_edges = extract_argument(py, max_edges.bind(py), "max_edges", None)?;
        let direction = match direction.as_str() {
            "outgoing" => crate::graph::reachability::ReachabilityDirection::Outgoing,
            "incoming" => crate::graph::reachability::ReachabilityDirection::Incoming,
            "both" => crate::graph::reachability::ReachabilityDirection::Both,
            _ => {
                let reason = "direction 必须是 outgoing / incoming / both (direction must be outgoing / incoming / both)";
                return Err(attach_error_info(
                    pyo3::exceptions::PyValueError::new_err(reason),
                    crate::TriviumError::InvalidInput(reason.into()).at_field("direction"),
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
        let output = dispatch!(this, db => db.query_subgraph(id, &config)).map_err(to_py_error)?;
        let value = serde_json::to_value(output)
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        json_to_pyobject(py, &value)
    }

    fn graph_stats(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let value =
            serde_json::to_value(dispatch!(this, db => db.graph_stats().map_err(to_py_error)?))
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        json_to_pyobject(py, &value)
    }

    fn validate_graph(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let value =
            serde_json::to_value(dispatch!(this, db => db.validate_graph().map_err(to_py_error)?))
                .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        json_to_pyobject(py, &value)
    }

    fn repair_graph_indexes(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let report = dispatch!(this, mut db => db.repair_graph_indexes()).map_err(to_py_error)?;
        let value = serde_json::to_value(report)
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        json_to_pyobject(py, &value)
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
    #[pyo3(signature = (min_community_size=integer_argument(3), max_iterations=integer_argument(15), compute_centroids=boolean_argument(true)), text_signature="($self, min_community_size=3, max_iterations=15, compute_centroids=True)")]
    fn leiden_cluster(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        min_community_size: Py<PyAny>,
        max_iterations: Py<PyAny>,
        compute_centroids: Py<PyAny>,
    ) -> PyResult<PyObject> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let min_community_size =
            integer_value(min_community_size.bind(py), "min_community_size", None)?;
        let max_iterations = integer_value(max_iterations.bind(py), "max_iterations", None)?;
        let compute_centroids: bool =
            extract_argument(py, compute_centroids.bind(py), "compute_centroids", None)?;
        let result = dispatch!(this, db => db.leiden_cluster(
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
}
