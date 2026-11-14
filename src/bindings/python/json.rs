//! 宿主对象与 JSON 内容的无损转换。

use super::errors::argument_input_error;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

// ════════ 辅助转换 ════════

pub(super) fn json_to_pyobject(py: Python<'_>, val: &serde_json::Value) -> PyResult<PyObject> {
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
                    .ok_or_else(|| PyValueError::new_err("JSON 数值无效 (Invalid JSON number)"))?
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

pub(super) fn pyobject_to_json(obj: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    pyobject_to_json_at(obj, None, None)
}

pub(super) fn write_json(obj: &Bound<'_, PyAny>, field: &str) -> PyResult<serde_json::Value> {
    pyobject_to_json_at(
        obj,
        Some(field),
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )
}

pub(super) fn read_json(obj: &Bound<'_, PyAny>, field: &str) -> PyResult<serde_json::Value> {
    pyobject_to_json_at(obj, Some(field), None)
}

pub(super) fn pyobject_to_json_at(
    obj: &Bound<'_, PyAny>,
    field: Option<&str>,
    write: Option<crate::error::WriteErrorInfo>,
) -> PyResult<serde_json::Value> {
    fn convert(
        obj: &Bound<'_, PyAny>,
        depth: usize,
        field: Option<&str>,
        write: Option<crate::error::WriteErrorInfo>,
    ) -> PyResult<serde_json::Value> {
        let result = (|| {
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
                    let name = k.extract::<String>()?;
                    let path = field.map(|field| crate::sdk::field_member(field, &name));
                    map.insert(name, convert(&v, depth + 1, path.as_deref(), write)?);
                }
                return Ok(serde_json::Value::Object(map));
            }
            let values = if let Ok(list) = obj.downcast::<PyList>() {
                list.iter()
                    .enumerate()
                    .map(|(index, v)| {
                        let path =
                            field.map(|field| crate::sdk::field_member(field, &index.to_string()));
                        convert(&v, depth + 1, path.as_deref(), write)
                    })
                    .collect::<PyResult<Vec<_>>>()?
            } else if let Ok(tuple) = obj.downcast::<PyTuple>() {
                tuple
                    .iter()
                    .enumerate()
                    .map(|(index, v)| {
                        let path =
                            field.map(|field| crate::sdk::field_member(field, &index.to_string()));
                        convert(&v, depth + 1, path.as_deref(), write)
                    })
                    .collect::<PyResult<Vec<_>>>()?
            } else {
                return Err(PyTypeError::new_err(
                    "Payload 包含不支持的类型 (Payload contains an unsupported type)",
                ));
            };
            Ok(serde_json::Value::Array(values))
        })();
        result.map_err(|error: PyErr| match field {
            Some(field) if !error.value(obj.py()).hasattr("target").unwrap_or(false) => {
                argument_input_error(error, field, write)
            }
            _ => error,
        })
    }
    convert(obj, 0, field, write)
}

pub(super) fn mapping_dictionary<'py>(
    value: &Bound<'py, PyAny>,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
) -> PyResult<Bound<'py, PyDict>> {
    let py = value.py();
    if let Ok(dictionary) = value.downcast::<PyDict>() {
        Ok(dictionary.clone())
    } else {
        let mapping = py.import("collections.abc")?.getattr("Mapping")?;
        if !value.is_instance(&mapping)? {
            let reason = "参数必须是映射 (Argument must be a mapping)";
            return Err(argument_input_error(
                pyo3::exceptions::PyTypeError::new_err(reason),
                field,
                write,
            ));
        }
        // 映射协议在已有数据库请求借用内执行，宿主异常保留原对象。
        Ok(py
            .get_type::<PyDict>()
            .call1((value,))?
            .downcast_into::<PyDict>()?)
    }
}
