//! 语言实参与数值、字符串、序列的边界转换。

use super::errors::{
    argument_input_error, attach_error_info, constructor_input_error, to_py_error,
    write_input_error,
};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString, PyTuple};

/// 默认值构造为真实 Python 整数，使显式提取保留原有数值默认值。
pub(super) fn integer_argument(value: u64) -> Py<PyAny> {
    Python::with_gil(|py| match value.into_pyobject(py) {
        Ok(value) => value.into_any().unbind(),
        Err(never) => match never {},
    })
}

pub(super) fn string_argument(value: &str) -> Py<PyAny> {
    Python::with_gil(|py| pyo3::types::PyString::new(py, value).into_any().unbind())
}

pub(super) fn float_argument(value: f64) -> Py<PyAny> {
    Python::with_gil(|py| pyo3::types::PyFloat::new(py, value).into_any().unbind())
}

pub(super) fn boolean_argument(value: bool) -> Py<PyAny> {
    Python::with_gil(|py| {
        value
            .into_pyobject(py)
            .unwrap()
            .to_owned()
            .into_any()
            .unbind()
    })
}

pub(super) fn constructor_arguments<'py>(
    py: Python<'py>,
    owner: &str,
    args: &Bound<'py, PyTuple>,
    kwargs: Option<&Bound<'py, PyDict>>,
) -> PyResult<Bound<'py, PyDict>> {
    py.import("triviumdb.triviumdb")?
        .getattr("_bind_constructor")?
        .call1((owner, args, kwargs))?
        .downcast_into::<PyDict>()
        .map_err(Into::into)
}

pub(super) fn constructor_argument<'py>(
    values: &Bound<'py, PyDict>,
    field: &str,
) -> PyResult<Bound<'py, PyAny>> {
    values.get_item(field)?.ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err(
            "构造签名绑定缺少字段 (Constructor signature binding omitted a field)",
        )
    })
}

pub(super) fn constructor_mib(value: usize, field: &str) -> PyResult<usize> {
    value.checked_mul(1024 * 1024).ok_or_else(|| {
        constructor_input_error(
            pyo3::exceptions::PyOverflowError::new_err(
                "MiB 换算字节时溢出 (MiB conversion to bytes overflowed)",
            ),
            field,
        )
    })
}

pub(super) fn extract_management_argument<'py, T: FromPyObject<'py>>(
    py: Python<'py>,
    value: &Bound<'py, PyAny>,
    field: &str,
) -> PyResult<T> {
    extract_argument(
        py,
        value,
        field,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )
}

pub(super) fn extract_argument<'py, T: FromPyObject<'py>>(
    py: Python<'py>,
    value: &Bound<'py, PyAny>,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
) -> PyResult<T> {
    value.extract().map_err(|error| {
        use pyo3::types::{PyBool, PyBytes, PyComplex, PyFloat, PyInt, PyString, PyTuple};
        let value_type = value.get_type();
        let builtin = value.is_none()
            || [
                py.get_type::<PyBool>(),
                py.get_type::<PyInt>(),
                py.get_type::<PyFloat>(),
                py.get_type::<PyComplex>(),
                py.get_type::<PyString>(),
                py.get_type::<PyBytes>(),
                py.get_type::<PyList>(),
                py.get_type::<PyTuple>(),
                py.get_type::<PyDict>(),
            ]
            .iter()
            .any(|expected| value_type.is(expected));
        if builtin {
            let input = crate::TriviumError::InvalidInput(format!(
                "参数类型或范围无效 (Invalid argument type or range): {field}"
            ))
            .at_field(field);
            let input = match write {
                Some(write) => input.with_write_info(write),
                None => input,
            };
            attach_error_info(error, input)
        } else {
            // 自定义转换协议中的宿主异常保留原对象、cause 与已有诊断。
            error
        }
    })
}

fn numeric_protocol(value: &Bound<'_, PyAny>, slot: i32) -> bool {
    // SAFETY: Bound 与 GIL 保证对象和类型有效；只检测协议，不调用槽位指针。
    // Python 3.9 的 PyType_GetSlot 只支持堆类型，静态类型交给既有数值协议提取。
    unsafe {
        let value_type = value.get_type();
        if pyo3::ffi::PyType_GetFlags(value_type.as_type_ptr()) & pyo3::ffi::Py_TPFLAGS_HEAPTYPE
            == 0
        {
            pyo3::ffi::PyNumber_Check(value.as_ptr()) != 0
        } else {
            !pyo3::ffi::PyType_GetSlot(value_type.as_type_ptr(), slot).is_null()
        }
    }
}

pub(super) fn integer_value<'py, T: FromPyObject<'py>>(
    value: &Bound<'py, PyAny>,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
) -> PyResult<T> {
    if !numeric_protocol(value, pyo3::ffi::Py_nb_index) {
        return Err(argument_input_error(
            pyo3::exceptions::PyTypeError::new_err(
                "参数必须支持整数协议 (Argument must support the integer protocol)",
            ),
            field,
            write,
        ));
    }
    extract_argument(value.py(), value, field, write)
}

pub(super) fn float_value(
    value: &Bound<'_, PyAny>,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
) -> PyResult<f32> {
    let numeric = numeric_protocol(value, pyo3::ffi::Py_nb_index)
        || numeric_protocol(value, pyo3::ffi::Py_nb_float);
    if !numeric {
        return Err(argument_input_error(
            pyo3::exceptions::PyTypeError::new_err(
                "参数必须支持数值协议 (Argument must support a numeric protocol)",
            ),
            field,
            write,
        ));
    }
    extract_argument(value.py(), value, field, write)
}

pub(super) fn string_value(
    value: &Bound<'_, PyAny>,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
) -> PyResult<String> {
    if !value.is_instance_of::<PyString>() {
        return Err(argument_input_error(
            pyo3::exceptions::PyTypeError::new_err("参数必须是字符串 (Argument must be a string)"),
            field,
            write,
        ));
    }
    value
        .extract()
        .map_err(|error| argument_input_error(error, field, write))
}

pub(super) fn read_id(value: &Bound<'_, PyAny>, field: &str) -> PyResult<u64> {
    integer_value(value, field, None)
}

pub(super) fn write_id(value: &Bound<'_, PyAny>, field: &str) -> PyResult<u64> {
    integer_value(
        value,
        field,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )
}

pub(super) fn read_float(value: &Bound<'_, PyAny>, field: &str) -> PyResult<f32> {
    float_value(value, field, None)
}

pub(super) fn write_float(value: &Bound<'_, PyAny>, field: &str) -> PyResult<f32> {
    float_value(
        value,
        field,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )
}

pub(super) fn read_string(value: &Bound<'_, PyAny>, field: &str) -> PyResult<String> {
    string_value(value, field, None)
}

pub(super) fn write_string(value: &Bound<'_, PyAny>, field: &str) -> PyResult<String> {
    string_value(
        value,
        field,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )
}

pub(super) fn write_list<'py>(
    value: &Bound<'py, PyAny>,
    field: &str,
) -> PyResult<Bound<'py, PyList>> {
    value
        .clone()
        .downcast_into::<PyList>()
        .map_err(|error| write_input_error(error.into(), field))
}

pub(super) fn write_sequence<T>(
    value: &Bound<'_, PyAny>,
    field: &str,
    parse: impl Fn(&Bound<'_, PyAny>, &str) -> PyResult<T>,
) -> PyResult<Vec<T>> {
    sequence_value(
        value,
        field,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        parse,
    )
}

pub(super) fn read_sequence<T>(
    value: &Bound<'_, PyAny>,
    field: &str,
    parse: impl Fn(&Bound<'_, PyAny>, &str) -> PyResult<T>,
) -> PyResult<Vec<T>> {
    sequence_value(value, field, None, parse)
}

pub(super) fn sequence_value<T>(
    value: &Bound<'_, PyAny>,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
    parse: impl Fn(&Bound<'_, PyAny>, &str) -> PyResult<T>,
) -> PyResult<Vec<T>> {
    // SAFETY: Bound 保持对象存活且 GIL 覆盖检测；只调用 CPython 协议检测，不解引用裸内存。
    let sequence = unsafe { pyo3::ffi::PySequence_Check(value.as_ptr()) != 0 };
    if !sequence || value.is_instance_of::<PyString>() {
        return Err(argument_input_error(
            pyo3::exceptions::PyTypeError::new_err(
                "参数必须是非字符串序列 (Argument must be a non-string sequence)",
            ),
            field,
            write,
        ));
    }
    // 复用 PyO3 的长度提示容错规则；按真实元素增长，避免相信宿主提供的巨大提示。
    let _ = value.len();
    let mut result = Vec::new();
    for (index, item) in value.try_iter()?.enumerate() {
        let item = item?;
        let path = crate::sdk::field_member(field, &index.to_string());
        let parsed = parse(&item, &path)?;
        result.try_reserve(1).map_err(|error| {
            let error = crate::TriviumError::CapacityAllocationFailed {
                reason: format!("参数缓冲分配失败 (Argument buffer allocation failed): {error}"),
            }
            .at_field(&path);
            to_py_error(match write {
                Some(write) => error.with_write_info(write),
                None => error,
            })
        })?;
        result.push(parsed);
    }
    Ok(result)
}
