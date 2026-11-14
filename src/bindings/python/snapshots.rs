//! 快照校验、恢复及数据库快照发布入口。

use super::arguments::{read_string, write_string};
use super::database::{PyTriviumDB, database_write};
use super::errors::{to_py_error, write_admission_error};
use super::json::json_to_pyobject;
use super::runtime::parse_python_runtime;
use pyo3::prelude::*;

#[pyfunction]
#[pyo3(signature = (directory, runtime=None))]
pub(super) fn verify_snapshot(
    py: Python<'_>,
    directory: Bound<'_, PyAny>,
    runtime: Option<Bound<'_, PyAny>>,
) -> PyResult<PyObject> {
    let directory = read_string(&directory, "directory")?;
    let runtime = parse_python_runtime(runtime, None)?;
    let manifest = py
        .allow_threads(|| {
            crate::storage::snapshot::verify_snapshot_with_runtime(&directory, &runtime)
        })
        .map_err(to_py_error)?;
    let value = serde_json::to_value(manifest)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
    json_to_pyobject(py, &value)
}

#[pyfunction]
#[pyo3(signature = (source, destination, runtime=None))]
pub(super) fn restore_snapshot(
    py: Python<'_>,
    source: Bound<'_, PyAny>,
    destination: Bound<'_, PyAny>,
    runtime: Option<Bound<'_, PyAny>>,
) -> PyResult<PyObject> {
    let source = write_string(&source, "source")?;
    let destination = write_string(&destination, "destination")?;
    let runtime = parse_python_runtime(
        runtime,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )?;
    let manifest = py
        .allow_threads(|| {
            crate::storage::snapshot::restore_snapshot_with_runtime(&source, &destination, &runtime)
        })
        .map_err(to_py_error)?;
    let value = serde_json::to_value(manifest)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
    json_to_pyobject(py, &value)
}

#[pymethods]
impl PyTriviumDB {
    fn create_snapshot(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        destination: Bound<'_, PyAny>,
        generation_id: Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let (_request, mut borrowed) = database_write(slf).map_err(write_admission_error)?;
        let this = &mut *borrowed;
        let destination = write_string(&destination, "destination")?;
        let generation_id = write_string(&generation_id, "generation_id")?;
        let manifest = py
            .allow_threads(
                || dispatch!(this, mut db => db.create_snapshot(&destination, &generation_id)),
            )
            .map_err(to_py_error)?;
        let value = serde_json::to_value(manifest)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        json_to_pyobject(py, &value)
    }
}
