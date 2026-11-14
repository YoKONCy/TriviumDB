//! 结构化错误、异常类别及写入结果的语言适配。

use super::json::json_to_pyobject;
use pyo3::prelude::*;

pyo3::create_exception!(triviumdb, TriviumDBError, pyo3::exceptions::PyRuntimeError);

pyo3::create_exception!(triviumdb, InvalidInputError, pyo3::exceptions::PyValueError);

pyo3::create_exception!(triviumdb, CapacityError, pyo3::exceptions::PyMemoryError);

pyo3::create_exception!(
    triviumdb,
    SnapshotDestinationError,
    pyo3::exceptions::PyFileExistsError
);

pyo3::create_exception!(triviumdb, ReadOnlyError, TriviumDBError);

pyo3::create_exception!(triviumdb, RecoveryRequiredError, TriviumDBError);

pyo3::create_exception!(triviumdb, ImmutableArtifactError, TriviumDBError);

pyo3::create_exception!(triviumdb, GenerationBusyError, TriviumDBError);

pyo3::create_exception!(triviumdb, UnsupportedDatabaseVersionError, TriviumDBError);

pyo3::create_exception!(triviumdb, ResourceBudgetError, TriviumDBError);

pyo3::create_exception!(triviumdb, SnapshotPublicationError, TriviumDBError);

pub(super) fn to_py_error(error: crate::error::TriviumError) -> PyErr {
    use crate::error::TriviumError;
    let exception = match error.underlying() {
        TriviumError::SnapshotPublicationUncertain { .. } => {
            SnapshotPublicationError::new_err(error.to_string())
        }
        TriviumError::SnapshotDestinationExists { .. } => {
            SnapshotDestinationError::new_err(error.to_string())
        }
        TriviumError::ResourceBudgetExceeded { .. } => {
            ResourceBudgetError::new_err(error.to_string())
        }
        TriviumError::ReadOnlyViolation { .. } => ReadOnlyError::new_err(error.to_string()),
        TriviumError::RecoveryRequired { .. } => RecoveryRequiredError::new_err(error.to_string()),
        TriviumError::ImmutableArtifactInvalid { .. } => {
            ImmutableArtifactError::new_err(error.to_string())
        }
        TriviumError::GenerationBusy { .. } => GenerationBusyError::new_err(error.to_string()),
        TriviumError::UnsupportedDatabaseVersion { .. } => {
            UnsupportedDatabaseVersionError::new_err(error.to_string())
        }
        TriviumError::CapacityReservationRejected { .. }
        | TriviumError::CapacityAllocationFailed { .. } => {
            CapacityError::new_err(error.to_string())
        }
        TriviumError::InvalidInput(_) => InvalidInputError::new_err(error.to_string()),
        _ => TriviumDBError::new_err(error.to_string()),
    };
    attach_error_info(exception, error)
}

pub(super) fn attach_error_info(exception: PyErr, error: crate::TriviumError) -> PyErr {
    attach_error_descriptor(exception, error.info())
}

pub(super) fn attach_error_descriptor(exception: PyErr, info: crate::error::ErrorInfo) -> PyErr {
    Python::with_gil(|py| -> PyResult<()> {
        let value = exception.value(py);
        value.setattr("schema_version", info.schema_version)?;
        value.setattr("code", info.code)?;
        value.setattr("message", info.message)?;
        value.setattr(
            "stage",
            info.stage.map(crate::error::QueryErrorStage::as_str),
        )?;
        let target = serde_json::to_value(info.target)
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        value.setattr("target", json_to_pyobject(py, &target)?)?;
        let write = serde_json::to_value(info.write)
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        value.setattr("write", json_to_pyobject(py, &write)?)?;
        let details = info.details;
        value.setattr("details", json_to_pyobject(py, &details)?)?;
        // 保留资源预算等已有的直接属性，字段的权威值来自内核。
        if let Some(fields) = details.as_object() {
            for (name, field) in fields {
                if matches!(
                    name.as_str(),
                    "schema_version"
                        | "code"
                        | "message"
                        | "details"
                        | "stage"
                        | "target"
                        | "write"
                ) {
                    continue;
                }
                value.setattr(name.as_str(), json_to_pyobject(py, field)?)?;
            }
        }
        Ok(())
    })
    .map_or_else(|conversion| conversion, |()| exception)
}

pub(super) const TRANSACTION_VALIDATION: crate::error::WriteErrorInfo =
    crate::error::WriteErrorInfo {
        transaction_consumed: Some(false),
        ..crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION
    };

pub(super) fn transaction_admission_error(error: PyErr) -> PyErr {
    python_write_error(error, crate::error::WriteErrorInfo::TRANSACTION_ADMISSION)
}

pub(super) fn write_admission_error(error: PyErr) -> PyErr {
    python_write_error(error, crate::error::WriteErrorInfo::MAINTENANCE_ADMISSION)
}

fn python_write_error(error: PyErr, write: crate::error::WriteErrorInfo) -> PyErr {
    Python::with_gil(|py| -> PyResult<()> {
        let write = serde_json::to_value(write)
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        error
            .value(py)
            .setattr("write", json_to_pyobject(py, &write)?)
    })
    .map_or_else(|conversion| conversion, |()| error)
}

pub(super) fn constructor_input_error(error: PyErr, field: &str) -> PyErr {
    attach_error_info(
        error,
        crate::TriviumError::InvalidInput(
            "构造参数值无效 (Invalid constructor argument value)".into(),
        )
        .at_field(field),
    )
}

pub(super) fn write_input_error(error: PyErr, field: &str) -> PyErr {
    argument_input_error(
        error,
        field,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )
}

pub(super) fn argument_input_error(
    error: PyErr,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
) -> PyErr {
    let input =
        crate::TriviumError::InvalidInput("参数无法转换 (Argument conversion failed)".into())
            .at_field(field);
    attach_error_info(
        error,
        match write {
            Some(write) => input.with_write_info(write),
            None => input,
        },
    )
}
