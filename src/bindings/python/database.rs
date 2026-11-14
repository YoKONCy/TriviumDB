//! 数据库句柄、dtype 分发、构造及基础信息。

use super::arguments::{
    constructor_argument, constructor_arguments, constructor_mib, extract_argument,
};
use super::errors::{constructor_input_error, to_py_error};
use super::options::{
    parse_access_mode, parse_missing_index_policy, parse_row_overflow, parse_sync_mode,
};
use super::runtime::parse_python_runtime;
use crate::database::Database as GenericDatabase;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

pub(super) enum DbBackend {
    F32(GenericDatabase<f32>),
    F16(GenericDatabase<half::f16>),
    U64(GenericDatabase<u64>),
}

/// Python 侧的 TriviumDB 包装器
#[pyclass(name = "TriviumDB")]
pub struct PyTriviumDB {
    pub(super) inner: DbBackend,
    pub(super) dtype: String,
}

// 在 PyO3 借用前登记请求：同线程重入与其他线程的借用争用分别返回稳定类别。
// 请求作用域只保留对象身份；Bound 和 PyRef 保证对象在作用域内存活。
pub(super) fn database_read<'py>(
    database: &Bound<'py, PyTriviumDB>,
) -> PyResult<(crate::sdk::RequestScope, PyRef<'py, PyTriviumDB>)> {
    let request = crate::sdk::RequestScope::enter(database.as_ptr() as usize, "python_callback")
        .map_err(to_py_error)?;
    let borrowed = database.try_borrow().map_err(|_| {
        to_py_error(crate::TriviumError::DatabaseBusy {
            resource: "python_database",
        })
    })?;
    Ok((request, borrowed))
}

pub(super) fn database_write<'py>(
    database: &Bound<'py, PyTriviumDB>,
) -> PyResult<(crate::sdk::RequestScope, PyRefMut<'py, PyTriviumDB>)> {
    let request = crate::sdk::RequestScope::enter(database.as_ptr() as usize, "python_callback")
        .map_err(to_py_error)?;
    let borrowed = database.try_borrow_mut().map_err(|_| {
        to_py_error(crate::TriviumError::DatabaseBusy {
            resource: "python_database",
        })
    })?;
    Ok((request, borrowed))
}

#[pymethods]
impl PyTriviumDB {
    #[getter]
    fn dtype(slf: &Bound<'_, Self>) -> PyResult<String> {
        let (_request, borrowed) = database_read(slf)?;
        Ok(borrowed.dtype.clone())
    }

    #[new]
    #[pyo3(signature = (*args, **kwargs), text_signature="(path, dim=1536, dtype='f32', sync_mode='normal', load_text_index=False, auto_build_quiver=True, expected_nodes=None, memory_limit_mb=0, payload_cache_mb=64, payload_cache_entry_mb=8, access_mode='read_write', missing_index_policy='fallback', max_query_rows=None, row_overflow='throw', runtime=None)")]
    fn new(
        py: Python<'_>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let values = constructor_arguments(py, "TriviumDB", args, kwargs)?;
        let path = constructor_argument(&values, "path")?;
        let dim = constructor_argument(&values, "dim")?.unbind();
        let dtype = constructor_argument(&values, "dtype")?.unbind();
        let sync_mode = constructor_argument(&values, "sync_mode")?.unbind();
        let load_text_index = constructor_argument(&values, "load_text_index")?.unbind();
        let auto_build_quiver = constructor_argument(&values, "auto_build_quiver")?.unbind();
        let expected_nodes: Option<Bound<'_, PyAny>> =
            constructor_argument(&values, "expected_nodes")?.extract()?;
        let memory_limit_mb = constructor_argument(&values, "memory_limit_mb")?.unbind();
        let payload_cache_mb = constructor_argument(&values, "payload_cache_mb")?.unbind();
        let payload_cache_entry_mb =
            constructor_argument(&values, "payload_cache_entry_mb")?.unbind();
        let access_mode = constructor_argument(&values, "access_mode")?.unbind();
        let missing_index_policy = constructor_argument(&values, "missing_index_policy")?.unbind();
        let max_query_rows: Option<Bound<'_, PyAny>> =
            constructor_argument(&values, "max_query_rows")?.extract()?;
        let row_overflow = constructor_argument(&values, "row_overflow")?.unbind();
        let runtime = constructor_argument(&values, "runtime")?.extract()?;
        let path: String = extract_argument(py, &path, "path", None)?;
        let dim = extract_argument(py, dim.bind(py), "dim", None)?;
        let dtype: String = extract_argument(py, dtype.bind(py), "dtype", None)?;
        let sync_mode: String = extract_argument(py, sync_mode.bind(py), "sync_mode", None)?;
        let load_text_index =
            extract_argument(py, load_text_index.bind(py), "load_text_index", None)?;
        let auto_build_quiver =
            extract_argument(py, auto_build_quiver.bind(py), "auto_build_quiver", None)?;
        let expected_nodes = expected_nodes
            .map(|value| extract_argument(py, &value, "expected_nodes", None))
            .transpose()?;
        let memory_limit_mb =
            extract_argument(py, memory_limit_mb.bind(py), "memory_limit_mb", None)?;
        let payload_cache_mb =
            extract_argument(py, payload_cache_mb.bind(py), "payload_cache_mb", None)?;
        let payload_cache_entry_mb = extract_argument(
            py,
            payload_cache_entry_mb.bind(py),
            "payload_cache_entry_mb",
            None,
        )?;
        let access_mode: String = extract_argument(py, access_mode.bind(py), "access_mode", None)?;
        let missing_index_policy: String = extract_argument(
            py,
            missing_index_policy.bind(py),
            "missing_index_policy",
            None,
        )?;
        let max_query_rows = max_query_rows
            .map(|value| extract_argument(py, &value, "max_query_rows", None))
            .transpose()?;
        let row_overflow: String =
            extract_argument(py, row_overflow.bind(py), "row_overflow", None)?;
        let sm = parse_sync_mode(&sync_mode)
            .map_err(|error| constructor_input_error(error, "sync_mode"))?;
        let memory_limit = constructor_mib(memory_limit_mb, "memory_limit_mb")?;
        let payload_cache_bytes = constructor_mib(payload_cache_mb, "payload_cache_mb")?;
        let payload_cache_entry_bytes =
            constructor_mib(payload_cache_entry_mb, "payload_cache_entry_mb")?;
        let config = crate::database::Config {
            dim,
            sync_mode: sm,
            load_text_index,
            auto_build_quiver,
            expected_nodes,
            memory_limit,
            payload_cache_bytes,
            payload_cache_entry_bytes,
            access_mode: parse_access_mode(&access_mode)
                .map_err(|error| constructor_input_error(error, "access_mode"))?,
            missing_index_policy: parse_missing_index_policy(&missing_index_policy)
                .map_err(|error| constructor_input_error(error, "missing_index_policy"))?,
            max_query_rows,
            row_overflow: parse_row_overflow(&row_overflow)
                .map_err(|error| constructor_input_error(error, "row_overflow"))?,
            ..Default::default()
        };
        let runtime = parse_python_runtime(runtime, None)?;
        let inner = match dtype.as_str() {
            "f32" => DbBackend::F32(
                GenericDatabase::<f32>::open_with_runtime(&path, config, runtime.clone())
                    .map_err(to_py_error)?,
            ),
            "f16" => DbBackend::F16(
                GenericDatabase::<half::f16>::open_with_runtime(&path, config, runtime.clone())
                    .map_err(to_py_error)?,
            ),
            "u64" => DbBackend::U64(
                GenericDatabase::<u64>::open_with_runtime(&path, config, runtime.clone())
                    .map_err(to_py_error)?,
            ),
            _ => {
                return Err(constructor_input_error(
                    pyo3::exceptions::PyValueError::new_err(
                        "dtype 必须是 f32 / f16 / u64 (dtype must be f32 / f16 / u64)",
                    ),
                    "dtype",
                ));
            }
        };
        Ok(Self {
            inner,
            dtype: dtype.to_string(),
        })
    }

    fn node_count(slf: &Bound<'_, Self>) -> PyResult<usize> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        Ok(dispatch!(this, db => db.node_count().map_err(to_py_error)?))
    }

    fn dim(slf: &Bound<'_, Self>) -> PyResult<usize> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        Ok(dispatch!(this, db => db.dim()))
    }

    /// 获取所有活跃节点的 ID 列表
    fn all_node_ids(slf: &Bound<'_, Self>) -> PyResult<Vec<u64>> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        Ok(dispatch!(this, db => db.all_node_ids().map_err(to_py_error)?))
    }

    fn __len__(slf: &Bound<'_, Self>) -> PyResult<usize> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        dispatch!(this, db => db.node_count()).map_err(to_py_error)
    }

    fn __contains__(slf: &Bound<'_, Self>, py: Python<'_>, id: Bound<'_, PyAny>) -> PyResult<bool> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        let id = extract_argument(py, &id, "id", None)?;
        Ok(dispatch!(this, db => db.contains(id).map_err(to_py_error)?))
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let (_request, borrowed) = database_read(slf)?;
        let this = &*borrowed;
        Ok(format!(
            "TriviumDB(dtype={}, nodes={}, dim={})",
            this.dtype,
            dispatch!(this, db => db.node_count())
                .map(|count| count.to_string())
                .unwrap_or_else(|_| "已关闭".into()),
            dispatch!(this, db => db.dim())
        ))
    }
}
