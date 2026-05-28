//! `exec` 子命令：非交互执行单条 TQL。
//!
//! Phase 0 占位实现，完整逻辑在 Phase 1 填充。

use crate::CliResult;
use crate::db_handle::DbHandle;
use crate::formatter::OutputFormat;

pub fn run(
    _handle: &mut DbHandle,
    query: &str,
    _mutate: bool,
    _format: OutputFormat,
) -> CliResult {
    println!("exec '{query}': 尚未实现 (Phase 1)");
    Ok(())
}
