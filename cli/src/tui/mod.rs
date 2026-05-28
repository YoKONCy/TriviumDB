//! TUI 模式：全屏终端可视化面板。
//!
//! Phase 0 占位实现，完整逻辑在 Phase 2b 填充。

use crate::CliResult;
use crate::db_handle::DbHandle;

pub fn run(handle: DbHandle, path: &str) -> CliResult {
    println!(
        "TUI '{path}' (dtype={}, nodes={}): 尚未实现 (Phase 2b)",
        handle.dtype(),
        handle.node_count()
    );
    Ok(())
}
