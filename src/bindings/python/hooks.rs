//! 宿主 Hook 回调、引用所有权与回调重入边界。

use super::arguments::read_string;
use super::database::{PyTriviumDB, database_write};
use super::errors::to_py_error;
use super::json::{json_to_pyobject, pyobject_to_json};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

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
    // 身份只用于准入匹配；正在调用的方法持有 PyRef，保证原实例存活。
    database_identity: usize,
}

// SAFETY: PyObject 本身是 Send+Sync（它只是一个引用计数指针），
// 实际的 Python 调用在 with_gil 中执行，由 GIL 保证线程安全。
unsafe impl Send for PySearchHookWrapper {}

// SAFETY: Python 回调共享访问均在 GIL 下执行，请求借用约束重入及数据库对象生命周期。
unsafe impl Sync for PySearchHookWrapper {}

impl PySearchHookWrapper {
    fn with_callback<R: Default>(
        &self,
        ctx: &mut crate::hook::HookContext,
        call: impl FnOnce(Python<'_>, &mut crate::hook::HookContext) -> R,
    ) -> R {
        let _request =
            match crate::sdk::RequestScope::callback(self.database_identity, "python_callback") {
                Ok(request) => request,
                Err(error) => {
                    ctx.error = Some(error.to_string());
                    return R::default();
                }
            };
        Python::with_gil(|py| call(py, ctx))
    }

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
        match json_to_pyobject(py, &ctx.custom_data).and_then(|v| dict.set_item("custom_data", v)) {
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
        self.with_callback(ctx, |py, ctx| {
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
        self.with_callback(ctx, |py, ctx| {
            let hook = self.py_hook.bind(py);
            let Ok(method) = hook.getattr("on_custom_recall") else {
                return None;
            };
            let py_ctx = PyDict::new(py);
            if !Self::set_context_data(py, &py_ctx, ctx) {
                return None;
            }
            match method.call1((query_vector.to_vec(), py_ctx)) {
                Ok(result) if !result.is_none() => match Self::py_to_hits(py, &result.unbind()) {
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
        self.with_callback(ctx, |py, ctx| {
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
                Ok(result) if !result.is_none() => match Self::py_to_hits(py, &result.unbind()) {
                    Ok(value) => *seeds = value,
                    Err(error) => Self::record_python_error(ctx, "on_pre_graph_expand", error),
                },
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
        self.with_callback(ctx, |py, ctx| {
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
                            Err(error) => Self::record_python_error(ctx, "on_post_recall", error),
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
        self.with_callback(ctx, |py, ctx| {
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
        self.with_callback(ctx, |py, ctx| {
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
                            Err(error) => Self::record_python_error(ctx, "on_post_search", error),
                        }
                    }
                    Ok(_) => {}
                    Err(error) => Self::record_python_error(ctx, "on_post_search", error),
                }
            }
        });
    }
}

#[pymethods]
impl PyTriviumDB {
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
    fn load_ffi_hook(slf: &Bound<'_, Self>, lib_path: Bound<'_, PyAny>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf)?;
        let this = &mut *borrowed;
        let lib_path = read_string(&lib_path, "lib_path")?;
        let ffi_hook = crate::hook::FfiHook::load(&lib_path).map_err(to_py_error)?;
        dispatch!(this, mut db => db.set_hook(ffi_hook)).map_err(to_py_error)
    }

    /// 清除当前已注册的 Hook，恢复为默认的零开销 NoopHook
    fn clear_hook(slf: &Bound<'_, Self>) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf)?;
        let this = &mut *borrowed;
        dispatch!(this, mut db => db.clear_hook());
        Ok(())
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
    fn set_hook(slf: &Bound<'_, Self>, hook: PyObject) -> PyResult<()> {
        let (_request, mut borrowed) = database_write(slf)?;
        let this = &mut *borrowed;
        let wrapper = PySearchHookWrapper {
            py_hook: hook,
            database_identity: slf.as_ptr() as usize,
        };
        dispatch!(this, mut db => db.set_hook(wrapper)).map_err(to_py_error)
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
}
