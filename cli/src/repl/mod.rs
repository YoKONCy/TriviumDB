//! REPL 模式：交互式 TQL + 元命令。
//!
//! Phase 0 占位实现，完整逻辑在 Phase 2a 填充。

use crate::CliResult;
use crate::db_handle::DbHandle;
use crate::formatter::OutputFormat;

pub fn run(handle: DbHandle, path: &str, _format: OutputFormat) -> CliResult {
    println!(
        "REPL '{path}' (dtype={}, nodes={}): 尚未实现 (Phase 2a)",
        handle.dtype(),
        handle.node_count()
    );
    Ok(())
}
