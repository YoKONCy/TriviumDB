//! `info` 子命令：显示数据库元信息。
//!
//! Phase 0 占位实现，完整逻辑在 Phase 1 填充。

use crate::CliResult;
use crate::db_handle::DType;
use crate::formatter::OutputFormat;

pub fn run(path: &str, _dim: Option<usize>, _dtype: DType, _format: OutputFormat) -> CliResult {
    println!("info '{path}': 尚未实现 (Phase 1)");
    Ok(())
}
