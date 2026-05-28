//! `import` 子命令：从 JSONL 批量导入节点。
//!
//! Phase 0 占位实现，完整逻辑在 Phase 1 填充。

use crate::CliResult;
use crate::db_handle::DbHandle;

pub fn run(_handle: &mut DbHandle, input: &str) -> CliResult {
    println!("import <- '{input}': 尚未实现 (Phase 1)");
    Ok(())
}
