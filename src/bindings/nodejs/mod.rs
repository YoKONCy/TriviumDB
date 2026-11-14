//! Node.js napi-rs 绑定的模块装配与公共导出。
//!
//! 值转换、请求借用及运行时对象按职责组织，数据库语义由 Core 与 SDK 维护。

/// 统一分发宏：对三种后端执行相同的表达式
macro_rules! dispatch {
    ($self:expr, $db:ident => $expr:expr) => {
        match &$self.inner {
            $crate::bindings::nodejs::database::DbBackend::F32($db) => $expr,
            $crate::bindings::nodejs::database::DbBackend::F16($db) => $expr,
            $crate::bindings::nodejs::database::DbBackend::U64($db) => $expr,
        }
    };
    ($self:expr, mut $db:ident => $expr:expr) => {
        match &mut $self.inner {
            $crate::bindings::nodejs::database::DbBackend::F32($db) => $expr,
            $crate::bindings::nodejs::database::DbBackend::F16($db) => $expr,
            $crate::bindings::nodejs::database::DbBackend::U64($db) => $expr,
        }
    };
}

mod arguments;
mod batch_search;
mod database;
mod errors;
mod graph;
mod hook_refs;
mod hooks;
mod indexes;
mod json;
mod lifecycle;
mod maintenance;
mod options;
mod protocols;
mod queries;
mod records;
#[cfg(all(not(test), not(target_family = "wasm")))]
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

pub use batch_search::BatchSearchTask;
pub use database::TriviumDB;
pub use protocols::JsConsoleSession;
pub use queries::JsPreparedTql;
pub use requests::{JsRequestContext, call_argument_info, invoke_request, request_operation};
pub use runtime::{JsResourceLimits, JsResourceRuntime};
pub use snapshots::{restore_snapshot, verify_snapshot};
pub use types::{
    JsClusterResult, JsDatabaseOptions, JsEdge, JsGroupedSearchResult, JsHookContext,
    JsIncomingEdge, JsLeidenConfig, JsNodeView, JsReachabilityOutput, JsReachabilityResult,
    JsReachabilityStep, JsSearchHit, JsSearchWithContextResult, JsSubgraphEdge, JsSubgraphNode,
    JsSubgraphResult,
};

/// 原 Rust 绑定模块路径的兼容导出。
#[allow(clippy::module_inception)]
pub mod nodejs {
    pub use super::{
        BatchSearchTask, JsClusterResult, JsConsoleSession, JsDatabaseOptions, JsEdge,
        JsGroupedSearchResult, JsHookContext, JsIncomingEdge, JsLeidenConfig, JsNodeView,
        JsPreparedTql, JsReachabilityOutput, JsReachabilityResult, JsReachabilityStep,
        JsRequestContext, JsResourceLimits, JsResourceRuntime, JsSearchHit,
        JsSearchWithContextResult, JsSubgraphEdge, JsSubgraphNode, JsSubgraphResult, TriviumDB,
        call_argument_info, invoke_request, request_operation, restore_snapshot, verify_snapshot,
    };
}
