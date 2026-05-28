//! `compact` 子命令：手动触发压缩。
//!
//! Phase 0 占位实现，完整逻辑在 Phase 1 填充。

use crate::CliResult;
use crate::db_handle::DbHandle;

pub fn run(_handle: &mut DbHandle) -> CliResult {
    println!("compact: 尚未实现 (Phase 1)");
    Ok(())
}
