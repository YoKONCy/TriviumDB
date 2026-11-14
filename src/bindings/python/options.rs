//! 查询、过滤与数据库配置的语言表示。

use super::errors::{argument_input_error, to_py_error};
use super::json::{mapping_dictionary, read_json};
use crate::filter::Filter;
use pyo3::prelude::*;

pub(super) fn parse_row_overflow(value: &str) -> PyResult<crate::database::RowOverflowPolicy> {
    match value {
        "throw" => Ok(crate::database::RowOverflowPolicy::Throw),
        "break" => Ok(crate::database::RowOverflowPolicy::Break),
        _ => Err(pyo3::exceptions::PyValueError::new_err(
            "row_overflow 必须是 throw / break",
        )),
    }
}

pub(super) fn mapping_to_filter(_py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Filter> {
    let dictionary = mapping_dictionary(value, "payload_filter", None)?;
    let json = read_json(dictionary.as_any(), "payload_filter")?;
    Filter::from_json(&json).map_err(|reason| {
        to_py_error(crate::TriviumError::InvalidInput(reason).at_field("payload_filter"))
    })
}

pub(super) fn parse_sync_mode(s: &str) -> PyResult<crate::storage::wal::SyncMode> {
    crate::storage::wal::SyncMode::parse(s).map_err(pyo3::exceptions::PyValueError::new_err)
}

pub(super) fn parse_access_mode(s: &str) -> PyResult<crate::database::AccessMode> {
    match s {
        "read_write" => Ok(crate::database::AccessMode::ReadWrite),
        "read_only" => Ok(crate::database::AccessMode::ReadOnly),
        "immutable" => Ok(crate::database::AccessMode::Immutable),
        _ => Err(pyo3::exceptions::PyValueError::new_err(
            "access_mode 必须是 read_write / read_only / immutable",
        )),
    }
}

pub(super) fn parse_missing_index_policy(
    value: &str,
) -> PyResult<crate::database::MissingIndexPolicy> {
    match value {
        "fallback" => Ok(crate::database::MissingIndexPolicy::Fallback),
        "build_in_memory" => Ok(crate::database::MissingIndexPolicy::BuildInMemory),
        "error" => Ok(crate::database::MissingIndexPolicy::Error),
        _ => Err(pyo3::exceptions::PyValueError::new_err(
            "missing_index_policy 必须是 fallback / build_in_memory / error",
        )),
    }
}

pub(super) fn parse_edge_direction(value: &str) -> PyResult<crate::database::EdgeDirection> {
    match value {
        "out" | "outgoing" => Ok(crate::database::EdgeDirection::Outgoing),
        "in" | "incoming" => Ok(crate::database::EdgeDirection::Incoming),
        "both" => Ok(crate::database::EdgeDirection::Both),
        _ => Err(argument_input_error(
            pyo3::exceptions::PyValueError::new_err(
                "edge_direction 必须是 out / in / both (edge_direction must be out / in / both)",
            ),
            "edge_direction",
            None,
        )),
    }
}
