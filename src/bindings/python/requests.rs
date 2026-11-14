//! 请求上下文、调用形状与宿主执行桥接。

use super::arguments::{constructor_argument, constructor_arguments, extract_argument};
use super::errors::{attach_error_descriptor, attach_error_info, to_py_error};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyTuple};

#[pyclass(name = "RequestContext", frozen)]
pub struct PyRequestContext {
    inner: crate::sdk::RequestContext,
}

#[pymethods]
impl PyRequestContext {
    #[new]
    #[pyo3(signature = (*args, **kwargs), text_signature="(request_id, timeout_ms=0)")]
    fn new(
        py: Python<'_>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let values = constructor_arguments(py, "RequestContext", args, kwargs)?;
        let request_id: String = extract_argument(
            py,
            &constructor_argument(&values, "request_id")?,
            "request_id",
            None,
        )?;
        let timeout_ms: u64 = extract_argument(
            py,
            &constructor_argument(&values, "timeout_ms")?,
            "timeout_ms",
            None,
        )?;
        crate::sdk::RequestContext::with_timeout(
            &request_id,
            std::time::Duration::from_millis(timeout_ms),
            false,
        )
        .map(|inner| Self { inner })
        .map_err(to_py_error)
    }

    #[getter]
    fn request_id(&self) -> &str {
        self.inner.request_id()
    }
    fn cancel(&self) {
        self.inner.cancel();
    }
    fn check(&self) -> PyResult<()> {
        self.inner.control().check().map_err(to_py_error)
    }

    fn __enter__(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<Py<Self>> {
        py.import("triviumdb._requests")?
            .getattr("enter")?
            .call1((slf,))
            .map_err(|error| request_scope_error(py, error))?;
        Ok(slf.clone().unbind())
    }

    fn __exit__(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        _exc_type: &Bound<'_, PyAny>,
        _exc_val: &Bound<'_, PyAny>,
        _exc_tb: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        py.import("triviumdb._requests")?
            .getattr("exit")?
            .call1((slf,))
            .map_err(|error| request_scope_error(py, error))?;
        Ok(false)
    }
}

fn request_scope_error(py: Python<'_>, error: PyErr) -> PyErr {
    if error.is_instance_of::<PyValueError>(py) {
        attach_error_info(
            error,
            crate::TriviumError::InvalidInput(
                "请求上下文进入或退出无效 (Invalid request context entry or exit)".into(),
            )
            .at_field("request_context"),
        )
    } else {
        error
    }
}

#[pyfunction]
pub(super) fn _invoke_request(
    context: &PyRequestContext,
    owner: &str,
    operation: &str,
    callback: &Bound<'_, PyAny>,
    args: &Bound<'_, PyTuple>,
    kwargs: &Bound<'_, PyDict>,
) -> PyResult<PyObject> {
    let admission = |error| to_py_error(crate::sdk::request_admission(error, owner, operation));
    let _scope = context.inner.enter().map_err(admission)?;
    if operation != "console_request" {
        crate::request::check_current().map_err(admission)?;
    }
    Ok(callback.call(args, Some(kwargs))?.into_any().unbind())
}

#[pyfunction]
pub(super) fn _request_operation(owner: &str, operation: &str) -> bool {
    crate::sdk::request_operation(owner, operation)
}

#[pyfunction]
pub(super) fn _call_argument_error(
    py: Python<'_>,
    exception: Bound<'_, PyAny>,
    owner: &str,
    operation: &str,
    field: &Bound<'_, PyString>,
    kind: &str,
) -> PyResult<Py<pyo3::exceptions::PyBaseException>> {
    let info =
        crate::sdk::call_argument_info(owner, operation, "arguments", kind).map_err(to_py_error)?;
    let exception = attach_error_descriptor(PyErr::from_value(exception), info);
    // Python 关键字允许孤立 surrogate，目标字段直接保留原始 PyString 内容。
    let target = PyDict::new(py);
    target.set_item("field", field)?;
    exception.value(py).setattr("target", target)?;
    Ok(exception.into_value(py))
}
