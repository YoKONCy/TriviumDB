//! 关闭、排空、取消及上下文管理器入口。

use super::arguments::integer_value;
use super::database::{PyTriviumDB, database_read, database_write};
use super::errors::to_py_error;
use super::json::json_to_pyobject;
use pyo3::prelude::*;

#[pymethods]
impl PyTriviumDB {
    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    #[pyo3(signature = (_exc_type=None, _exc_val=None, _exc_tb=None))]
    fn __exit__(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_val: Option<&Bound<'_, PyAny>>,
        _exc_tb: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        Self::close(slf, py)?;
        Ok(false)
    }

    fn request_shutdown(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let (_request, mut borrowed) = database_write(slf)?;
        let this = &mut *borrowed;
        let status = dispatch!(this, mut db => db.request_shutdown()).map_err(to_py_error)?;
        let value = serde_json::to_value(status)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        json_to_pyobject(py, &value)
    }

    fn shutdown_status(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<PyObject> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let value = serde_json::to_value(dispatch!(this, db => db.shutdown_status()))
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        json_to_pyobject(py, &value)
    }

    fn close_timeout(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        timeout_ms: Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        let (_request, mut borrowed) = database_write(slf)?;
        let this = &mut *borrowed;
        let timeout_ms: u64 = integer_value(&timeout_ms, "timeout_ms", None)?;
        py.allow_threads(|| dispatch!(this, mut db => db.close_timeout(std::time::Duration::from_millis(timeout_ms))))
                .map_err(to_py_error)
    }

    /// 显式关闭数据库（落盘后释放资源）
    fn close(slf: &Bound<'_, Self>, py: Python<'_>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf)?;
        let this = &mut *borrowed;
        match py.allow_threads(|| dispatch!(this, mut db => db.close())) {
            Ok(()) | Err(crate::error::TriviumError::DatabaseClosed) => Ok(()),
            Err(error) => Err(to_py_error(error)),
        }
    }
}
