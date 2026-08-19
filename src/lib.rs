#![allow(clippy::too_many_arguments)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::suspicious_open_options)]
#![allow(clippy::unnecessary_sort_by)]

pub mod database;
pub mod error;
pub mod filter;
pub mod graph;
pub mod hook;
pub mod index;
pub mod node;
pub mod query;
pub mod storage;

pub mod cognitive;

/// FFI 绑定层（Python / Node.js）
pub mod bindings;

pub use database::Database;
pub use error::{Result, TriviumError};
pub use filter::Filter;
pub use hook::{CompositeHook, FfiHook, HookContext, NoopHook, SearchHook};
pub use node::{Edge, IncomingEdge, NodeId, NodeView, SearchHit};
pub mod vector;
pub use vector::VectorType;

// PyO3 模块入口：当 maturin 构建 cdylib 时，Python import 会调用此处
#[cfg(feature = "python")]
pub use bindings::python::python::triviumdb;
