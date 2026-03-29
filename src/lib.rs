pub mod database;
pub mod error;
pub mod filter;
pub mod graph;
pub mod index;
pub mod node;
#[cfg(feature = "nodejs")]
pub mod nodejs;
#[cfg(feature = "python")]
pub mod python;
pub mod query;
pub mod storage;

pub mod cognitive;

pub use database::Database;
pub use error::{Result, TriviumError};
pub use filter::Filter;
pub use node::{Edge, NodeId, NodeView, SearchHit};
pub mod vector;
pub use vector::VectorType;

// PyO3 模块入口：当 maturin 构建 cdylib 时，Python import 会调用此处
#[cfg(feature = "python")]
pub use python::python::triviumdb;
