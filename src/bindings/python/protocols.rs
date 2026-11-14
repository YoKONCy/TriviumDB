//! 文档、图、QuIVer 和 Console 的共享 JSON 协议入口。

use super::arguments::{
    constructor_argument, constructor_arguments, extract_argument, read_string,
};
use super::database::{PyTriviumDB, database_read, database_write};
use super::errors::{argument_input_error, to_py_error};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

/// 控制台策略与取消状态不持有数据库或文件锁。
#[pyclass(name = "ConsoleSession", frozen)]
pub struct PyConsoleSession {
    inner: crate::ConsoleSession,
}

#[pymethods]
impl PyConsoleSession {
    #[new]
    #[pyo3(signature = (*args, **kwargs), text_signature="(options_json='{}')")]
    fn new(
        py: Python<'_>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let values = constructor_arguments(py, "ConsoleSession", args, kwargs)?;
        let options_json: String = extract_argument(
            py,
            &constructor_argument(&values, "options_json")?,
            "options_json",
            None,
        )?;
        crate::ConsoleSession::from_json(&options_json)
            .map(|inner| Self { inner })
            .map_err(|error| to_py_error(error.at_field("options_json")))
    }

    fn detach(&self) {
        self.inner.detach();
    }
    fn is_detached(&self) -> bool {
        self.inner.is_detached()
    }
    fn is_drained(&self) -> bool {
        self.inner.is_drained()
    }
    fn cancel(&self, request_id: Bound<'_, PyAny>) -> PyResult<bool> {
        let request_id = read_string(&request_id, "request_id")?;
        Ok(self.inner.cancel(&request_id))
    }
}

#[pymethods]
impl PyTriviumDB {
    /// 文档JSON适配复用Core的解析、来源映射、分批发布、修订、检索与证据上下文。
    fn document_request(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        request_json: Bound<'_, PyAny>,
    ) -> PyResult<String> {
        let (_request, mut borrowed) = database_write(slf)?;
        let this = &mut *borrowed;
        let request_json = read_string(&request_json, "request_json")?;
        py.allow_threads(
            || dispatch!(this, mut db => crate::sdk::documents::request(db, &request_json)),
        )
        .map_err(to_py_error)
    }

    /// SSD运行时JSON适配复用Core的事实绑定、驻留、缓存与I/O生命周期。
    fn quiver_request(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        request_json: Bound<'_, PyAny>,
    ) -> PyResult<String> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let request_json = read_string(&request_json, "request_json")?;
        py.allow_threads(|| dispatch!(this, db => crate::sdk::quiver::request(db, &request_json)))
            .map_err(to_py_error)
    }

    /// 命名图JSON适配复用Core的成员、独立边、修订及作用域契约。
    fn graph_request(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        request_json: Bound<'_, PyAny>,
    ) -> PyResult<String> {
        let (_request, mut borrowed) = database_write(slf)?;
        let this = &mut *borrowed;
        let request_json = read_string(&request_json, "request_json")?;
        py.allow_threads(
            || dispatch!(this, mut db => crate::sdk::named_graphs::request(db, &request_json)),
        )
        .map_err(to_py_error)
    }

    /// 原样传输 JSON 字符串，避免宿主 JavaScript 对 u64 的中间降精度。
    fn console_request(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        session: Bound<'_, PyAny>,
        request_json: Bound<'_, PyAny>,
    ) -> PyResult<String> {
        let (_request, mut borrowed) = database_write(slf)?;
        let this = &mut *borrowed;
        let session = session
            .extract::<PyRef<'_, PyConsoleSession>>()
            .map_err(|error| argument_input_error(error, "session", None))?;
        let session = session.inner.clone();
        let request_json = read_string(&request_json, "request_json")?;
        py.allow_threads(|| dispatch!(this, mut db => db.console_request(&session, &request_json)))
            .map_err(to_py_error)
    }
}
