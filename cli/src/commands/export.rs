//! `export` 子命令：导出全部节点为 JSONL（每行一个 JSON 对象）。
//!
//! 行格式：`{"id":1,"vector":[...],"payload":{...},"edges":[{"target","label","weight"}]}`

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};

use colored::Colorize;
use serde_json::json;

use crate::CliResult;
use crate::db_handle::DbHandle;

pub fn run(handle: &DbHandle, db_path: &str, output: &str) -> CliResult {
    validate_output_path(db_path, output)?;

    let file = File::create(output)?;
    let mut w = BufWriter::new(file);

    let ids = handle.get_all_ids();
    let pb = crate::util::progress_bar(ids.len() as u64, "export");
    let mut count = 0usize;
    for id in ids {
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
        pb.inc(1);
    }
    pb.finish_and_clear();
    w.flush()?;

    println!("{} 导出 {count} 个节点 -> {output}", "✓".green().bold());
    Ok(())
}

fn validate_output_path(db_path: &str, output: &str) -> CliResult {
    let output_path = normalized_path(Path::new(output))?;
    for protected in protected_paths(db_path) {
        if paths_equal(&output_path, &normalized_path(&protected)?) {
            return Err(format!(
                "拒绝导出到数据库相关文件: {}（会覆盖或破坏数据库文件）",
                output
            )
            .into());
        }
    }
    Ok(())
}

fn protected_paths(db_path: &str) -> Vec<PathBuf> {
    [
        db_path.to_string(),
        format!("{db_path}.vec"),
        format!("{db_path}.wal"),
        format!("{db_path}.flush_ok"),
        format!("{db_path}.lock"),
        format!("{db_path}.quiver"),
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

fn normalized_path(path: &Path) -> std::io::Result<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in abs.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}
