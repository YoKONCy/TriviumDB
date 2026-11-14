//! 共享资源运行时的构造、句柄校验与统计。

use super::arguments::{
    constructor_argument, constructor_arguments, constructor_mib, extract_argument,
};
use super::errors::{attach_error_info, to_py_error};
use super::json::json_to_pyobject;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

#[pyclass(name = "ResourceRuntime", frozen)]
pub struct PyResourceRuntime {
    inner: crate::resources::ResourceRuntime,
}

#[pymethods]
impl PyResourceRuntime {
    #[new]
    #[pyo3(signature = (*args, **kwargs), text_signature="(memory_limit_mb=0, max_threads=0, max_active_tasks=0, maintenance_memory_limit_mb=0)")]
    fn new(
        py: Python<'_>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let values = constructor_arguments(py, "ResourceRuntime", args, kwargs)?;
        let memory_limit_mb = constructor_argument(&values, "memory_limit_mb")?;
        let max_threads = constructor_argument(&values, "max_threads")?;
        let max_active_tasks = constructor_argument(&values, "max_active_tasks")?;
        let maintenance_memory_limit_mb =
            constructor_argument(&values, "maintenance_memory_limit_mb")?;
        let memory_limit_mb = extract_argument(py, &memory_limit_mb, "memory_limit_mb", None)?;
        let max_threads = extract_argument(py, &max_threads, "max_threads", None)?;
        let max_active_tasks = extract_argument(py, &max_active_tasks, "max_active_tasks", None)?;
        let maintenance_memory_limit_mb = extract_argument(
            py,
            &maintenance_memory_limit_mb,
            "maintenance_memory_limit_mb",
            None,
        )?;
        let memory_limit = constructor_mib(memory_limit_mb, "memory_limit_mb")?;
        crate::resources::ResourceRuntime::new(crate::resources::ResourceLimits {
            maintenance_memory_limit: constructor_mib(
                maintenance_memory_limit_mb,
                "maintenance_memory_limit_mb",
            )?,
            memory_limit,
            max_threads,
            max_active_tasks,
        })
        .map(|inner| Self { inner })
        .map_err(|error| {
            let error = match error.info().target {
                Some(crate::error::ErrorTarget::Field { field })
                    if field == "maintenance_memory_limit" =>
                {
                    error.at_field("maintenance_memory_limit_mb")
                }
                _ => error,
            };
            to_py_error(error)
        })
    }

    fn stats(&self, py: Python<'_>) -> PyResult<PyObject> {
        let value = serde_json::to_value(self.inner.stats())
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        json_to_pyobject(py, &value)
    }
}

pub(super) fn parse_python_runtime(
    runtime: Option<Bound<'_, PyAny>>,
    write: Option<crate::error::WriteErrorInfo>,
) -> PyResult<crate::resources::ResourceRuntime> {
    let Some(value) = runtime else {
        return Ok(crate::resources::ResourceRuntime::shared_default());
    };
    // PyRef 按原生类型提取，错误对象的宿主属性和转换协议均不参与句柄校验。
    let runtime = value
        .extract::<PyRef<'_, PyResourceRuntime>>()
        .map_err(|error| {
            let input = crate::TriviumError::InvalidInput(
                "runtime 必须是 ResourceRuntime (runtime must be a ResourceRuntime)".into(),
            )
            .at_field("runtime");
            let input = match write {
                Some(write) => input.with_write_info(write),
                None => input,
            };
            attach_error_info(error, input)
        })?;
    Ok(runtime.inner.clone())
}
