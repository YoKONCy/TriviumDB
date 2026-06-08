//! `compact` 子命令：手动触发全量重写与压实。

use std::time::Instant;

use colored::Colorize;

use crate::CliResult;
use crate::db_handle::DbHandle;
use crate::util::human_bytes;

pub fn run(handle: &mut DbHandle) -> CliResult {
    let nodes = handle.node_count();
    let before = handle.estimated_memory();

    let start = Instant::now();
    handle.compact()?;
    let elapsed = start.elapsed();

    let after = handle.estimated_memory();
    println!(
        "{} 压实完成: {nodes} 节点 | 内存 {} -> {} | 耗时 {elapsed:.2?}",
        "✓".green().bold(),
        human_bytes(before as u64),
        human_bytes(after as u64),
    );
    Ok(())
}
