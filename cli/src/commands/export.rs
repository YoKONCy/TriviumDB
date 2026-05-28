//! `export` 子命令：导出全部节点为 JSONL（每行一个 JSON 对象）。
//!
//! 行格式：`{"id":1,"vector":[...],"payload":{...},"edges":[{"target","label","weight"}]}`

use std::fs::File;
use std::io::{BufWriter, Write};

use colored::Colorize;
use serde_json::json;

use crate::CliResult;
use crate::db_handle::DbHandle;

pub fn run(handle: &DbHandle, output: &str) -> CliResult {
    let file = File::create(output)?;
    let mut w = BufWriter::new(file);

    let mut count = 0usize;
    for id in handle.get_all_ids() {
        if let Some(node) = handle.get(id) {
            let line = json!({
                "id": node.id,
                "vector": node.vector,
                "payload": node.payload,
                "edges": node.edges.iter().map(|e| json!({
                    "target": e.target_id,
                    "label": e.label,
                    "weight": e.weight,
                })).collect::<Vec<_>>(),
            });
            writeln!(w, "{}", serde_json::to_string(&line)?)?;
            count += 1;
        }
    }
    w.flush()?;

    println!("{} 导出 {count} 个节点 -> {output}", "✓".green().bold());
    Ok(())
}
