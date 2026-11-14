//! Python 模块、类型、异常与函数的统一注册。

use super::database::PyTriviumDB;
use super::errors::{
    CapacityError, GenerationBusyError, ImmutableArtifactError, InvalidInputError, ReadOnlyError,
    RecoveryRequiredError, ResourceBudgetError, SnapshotDestinationError, SnapshotPublicationError,
    TriviumDBError, UnsupportedDatabaseVersionError,
};
use super::protocols::PyConsoleSession;
use super::queries::PyPreparedTql;
use super::requests::{
    _call_argument_error, _invoke_request, _request_operation, PyRequestContext,
};
use super::runtime::PyResourceRuntime;
use super::snapshots::{restore_snapshot, verify_snapshot};
use super::transaction::PyTransaction;
use super::types::{
    PyEdge, PyGroupedSearchResult, PyHookContext, PyIncomingEdge, PyNodeView, PyQueryRow,
    PyReachabilityResult, PyReachabilityStep, PySearchHit,
};
use pyo3::prelude::*;

#[pyfunction]
pub fn init_logger() {
    use tracing_subscriber::{EnvFilter, fmt};
    let _ = fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .try_init();
}

#[pymodule]
pub fn triviumdb(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("TriviumDBError", m.py().get_type::<TriviumDBError>())?;
    m.add("InvalidInputError", m.py().get_type::<InvalidInputError>())?;
    m.add("CapacityError", m.py().get_type::<CapacityError>())?;
    m.add(
        "SnapshotDestinationError",
        m.py().get_type::<SnapshotDestinationError>(),
    )?;
    m.add("ReadOnlyError", m.py().get_type::<ReadOnlyError>())?;
    m.add(
        "RecoveryRequiredError",
        m.py().get_type::<RecoveryRequiredError>(),
    )?;
    m.add(
        "ImmutableArtifactError",
        m.py().get_type::<ImmutableArtifactError>(),
    )?;
    m.add(
        "GenerationBusyError",
        m.py().get_type::<GenerationBusyError>(),
    )?;
    m.add(
        "UnsupportedDatabaseVersionError",
        m.py().get_type::<UnsupportedDatabaseVersionError>(),
    )?;
    m.add(
        "ResourceBudgetError",
        m.py().get_type::<ResourceBudgetError>(),
    )?;
    m.add(
        "SnapshotPublicationError",
        m.py().get_type::<SnapshotPublicationError>(),
    )?;
    m.add_class::<PyResourceRuntime>()?;
    m.add_class::<PyRequestContext>()?;
    m.add_class::<PyConsoleSession>()?;
    m.add_class::<PyPreparedTql>()?;
    m.add_class::<PyTriviumDB>()?;
    m.add_class::<PySearchHit>()?;
    m.add_class::<PyGroupedSearchResult>()?;
    m.add_class::<PyReachabilityStep>()?;
    m.add_class::<PyReachabilityResult>()?;
    m.add_class::<PyEdge>()?;
    m.add_class::<PyIncomingEdge>()?;
    m.add_class::<PyNodeView>()?;
    m.add_class::<PyQueryRow>()?;
    m.add_class::<PyHookContext>()?;
    m.add_class::<PyTransaction>()?;
    m.add_function(wrap_pyfunction!(init_logger, m)?)?;
    m.add_function(wrap_pyfunction!(verify_snapshot, m)?)?;
    m.add_function(wrap_pyfunction!(restore_snapshot, m)?)?;
    // 私有诊断桥接不加入公开导出目录。
    m.setattr(
        "_call_argument_error",
        wrap_pyfunction!(_call_argument_error, m)?,
    )?;
    m.setattr("_invoke_request", wrap_pyfunction!(_invoke_request, m)?)?;
    m.setattr(
        "_request_operation",
        wrap_pyfunction!(_request_operation, m)?,
    )?;
    Ok(())
}
