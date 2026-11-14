//! Python PyO3 绑定的模块装配与公共导出。
//!
//! 值转换、请求借用及运行时对象按职责组织，数据库语义由 Core 与 SDK 维护。

macro_rules! dispatch {
    ($self:expr, $db:ident => $expr:expr) => {
        match &$self.inner {
            $crate::bindings::python::database::DbBackend::F32($db) => $expr,
            $crate::bindings::python::database::DbBackend::F16($db) => $expr,
            $crate::bindings::python::database::DbBackend::U64($db) => $expr,
        }
    };
    ($self:expr, mut $db:ident => $expr:expr) => {
        match &mut $self.inner {
            $crate::bindings::python::database::DbBackend::F32($db) => $expr,
            $crate::bindings::python::database::DbBackend::F16($db) => $expr,
            $crate::bindings::python::database::DbBackend::U64($db) => $expr,
        }
    };
}

mod arguments;
mod database;
mod errors;
mod graph;
mod hooks;
mod indexes;
mod json;
mod lifecycle;
mod maintenance;
mod options;
mod protocols;
mod queries;
mod records;
mod registration;
mod requests;
mod runtime;
mod search;
mod snapshots;
#[cfg(test)]
mod tests;
mod text;
mod transaction;
mod types;

pub use database::PyTriviumDB;
pub use errors::{
    CapacityError, GenerationBusyError, ImmutableArtifactError, InvalidInputError, ReadOnlyError,
    RecoveryRequiredError, ResourceBudgetError, SnapshotDestinationError, SnapshotPublicationError,
    TriviumDBError, UnsupportedDatabaseVersionError,
};
pub use protocols::PyConsoleSession;
pub use queries::PyPreparedTql;
pub use registration::{init_logger, triviumdb};
pub use requests::PyRequestContext;
pub use runtime::PyResourceRuntime;
pub use types::{
    PyEdge, PyGroupedSearchResult, PyHookContext, PyIncomingEdge, PyNodeView, PyQueryRow,
    PyReachabilityResult, PyReachabilityStep, PySearchHit,
};

/// 原 Rust 绑定模块路径的兼容导出。
#[allow(clippy::module_inception)]
pub mod python {
    pub use super::{
        CapacityError, GenerationBusyError, ImmutableArtifactError, InvalidInputError,
        PyConsoleSession, PyEdge, PyGroupedSearchResult, PyHookContext, PyIncomingEdge, PyNodeView,
        PyPreparedTql, PyQueryRow, PyReachabilityResult, PyReachabilityStep, PyRequestContext,
        PyResourceRuntime, PySearchHit, PyTriviumDB, ReadOnlyError, RecoveryRequiredError,
        ResourceBudgetError, SnapshotDestinationError, SnapshotPublicationError, TriviumDBError,
        UnsupportedDatabaseVersionError, init_logger, triviumdb,
    };
}
