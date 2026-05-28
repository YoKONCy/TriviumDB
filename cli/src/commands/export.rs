//! `export` 子命令：导出全部节点为 JSONL。
//!
//! Phase 0 占位实现，完整逻辑在 Phase 1 填充。

use crate::CliResult;
use crate::db_handle::DbHandle;

pub fn run(_handle: &DbHandle, output: &str) -> CliResult {
    println!("export -> '{output}': 尚未实现 (Phase 1)");
    Ok(())
}
