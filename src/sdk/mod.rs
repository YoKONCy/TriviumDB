//! 内核 SDK：共享实例访问、请求准入与宿主适配。
//!
//! SDK 将请求交给 `database::Database`；事务、WAL、文件租约、恢复与关闭排空
//! 由存储内核执行。克隆句柄共享同一实例，事务准备可在准入之前完成。

mod console;
mod errors;
pub mod facade;
mod shared;

#[cfg(feature = "python")]
pub(crate) use shared::RequestScope;

pub use crate::database::console::{ConsoleFeature, ConsoleOptions, ConsoleSession};
pub use console::ConsoleHost;
pub use errors::json_error_details;
pub use facade::{DatabaseReader, DatabaseWriter};
pub use shared::{SharedDatabase, WeakDatabase};
