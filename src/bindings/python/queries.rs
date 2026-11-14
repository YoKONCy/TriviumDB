//! TQL、Prepared 参数与查询结果的语言适配。

use super::arguments::{extract_argument, extract_management_argument};
use super::database::{DbBackend, PyTriviumDB, database_read, database_write};
use super::errors::{argument_input_error, attach_error_info, to_py_error, write_admission_error};
use super::json::{json_to_pyobject, pyobject_to_json};
use super::types::PyQueryRow;
use pyo3::prelude::*;
use pyo3::types::PyDict;

fn collect_prepared_parameters(
    parameters: &Bound<'_, PyAny>,
    prepared: &crate::query::tql_prepared::PreparedTql,
) -> PyResult<std::collections::HashMap<String, crate::query::tql_prepared::TqlParamValue>> {
    let py = parameters.py();
    let mut values = std::collections::HashMap::new();
    let mut insert = |key: Bound<'_, PyAny>, value: Bound<'_, PyAny>| -> PyResult<()> {
        let name = key.extract::<String>().map_err(|error| {
            attach_error_info(
                error,
                crate::TriviumError::InvalidInput(
                    "参数映射键必须是字符串 (Parameter mapping keys must be strings)".into(),
                )
                .at_field("parameters"),
            )
        })?;
        let json = pyobject_to_json(&value).map_err(|error| {
            attach_error_info(
                error,
                prepared.locate_parameter_error(
                    crate::TriviumError::InvalidInput(
                        "参数值无法转换为 JSON (Parameter value cannot be converted to JSON)"
                            .into(),
                    )
                    .at_query_parameter(&name),
                ),
            )
        })?;
        let value = prepared
            .parameter_from_json(&name, &json)
            .map_err(to_py_error)?;
        values.insert(name, value);
        Ok(())
    };
    if let Ok(dictionary) = parameters.downcast::<PyDict>() {
        for (key, value) in dictionary.iter() {
            insert(key, value)?;
        }
    } else {
        let mapping = py.import("collections.abc")?.getattr("Mapping")?;
        if !parameters.is_instance(&mapping)? {
            let reason = "Prepared TQL 参数必须是映射 (Prepared TQL parameters must be a mapping)";
            return Err(attach_error_info(
                pyo3::exceptions::PyTypeError::new_err(reason),
                crate::TriviumError::InvalidInput(reason.into()).at_field("parameters"),
            ));
        }
        // Mapping 的 items、迭代与取值可执行宿主代码，异常沿原路径传播。
        for pair in parameters.call_method0("items")?.try_iter()? {
            let (key, value) = pair?.extract::<(Bound<'_, PyAny>, Bound<'_, PyAny>)>()?;
            insert(key, value)?;
        }
    }
    Ok(values)
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
            TqlValue::Edge(edge) => edge.query_json(edge.source_id.into(), edge.target_id.into()),
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

#[pymethods]
impl PyTriviumDB {
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
    fn tql(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query: Bound<'_, PyAny>,
    ) -> PyResult<Vec<PyQueryRow>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let query: String = extract_argument(py, &query, "query", None)?;
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
                            let _ =
                                node_dict.set_item("payload", json_to_pyobject(py, &node.payload)?);
                            let _ = node_dict.set_item("num_edges", node.edges.len());
                            let _ = py_row.set_item(name, node_dict);
                        }
                        TqlValue::Edge(edge) => {
                            let edge_dict = PyDict::new(py);
                            let _ = edge_dict.set_item("type", "edge");
                            let _ = edge_dict.set_item("source", edge.source_id);
                            if let Some(graph_id) = &edge.graph_id {
                                edge_dict.set_item("graph_id", graph_id)?;
                            }
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

        match &this.inner {
            DbBackend::F32(db) => {
                let rows = db.tql_values(&query).map_err(to_py_error)?;
                convert_rows(py, rows)
            }
            DbBackend::F16(db) => {
                let rows = db.tql_values(&query).map_err(to_py_error)?;
                convert_rows(py, rows)
            }
            DbBackend::U64(db) => {
                let rows = db.tql_values(&query).map_err(to_py_error)?;
                convert_rows(py, rows)
            }
        }
    }

    fn prepare_tql(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query: Bound<'_, PyAny>,
    ) -> PyResult<PyPreparedTql> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let query: String = extract_argument(py, &query, "query", None)?;
        dispatch!(this, db => db.prepare_tql(&query))
            .map(|inner| PyPreparedTql { inner })
            .map_err(to_py_error)
    }

    fn execute_prepared_tql(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        prepared: Bound<'_, PyAny>,
        parameters: &Bound<'_, PyAny>,
    ) -> PyResult<Vec<PyQueryRow>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let prepared = prepared
            .extract::<PyRef<'_, PyPreparedTql>>()
            .map_err(|error| argument_input_error(error, "prepared", None))?;
        let values = collect_prepared_parameters(parameters, &prepared.inner)?;
        let rows = match &this.inner {
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
    fn tql_mut(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        query: Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let query: String = extract_management_argument(py, &query, "query")?;
        let result = dispatch!(this, mut db => db.tql_mut(&query)).map_err(to_py_error)?;
        let dict = PyDict::new(py);
        let _ = dict.set_item("affected", result.affected);
        let created: Vec<u64> = result.created_ids;
        let _ = dict.set_item("created_ids", created);
        Ok(dict.into_any().unbind())
    }
}
