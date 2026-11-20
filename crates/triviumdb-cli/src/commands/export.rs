//! `export` 子命令按原始 dtype 导出节点、Payload 和完整边。

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use colored::Colorize;

use crate::CliResult;
use crate::db_handle::DbHandle;

pub fn run(handle: &DbHandle, db_path: &str, output: &str) -> CliResult {
    let output_path = validate_output_path(db_path, output)?;
    let parent = output_path
        .parent()
        .ok_or("导出路径缺少目录 (Export path has no parent)")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    let mut ids = handle.get_all_ids()?;
    ids.sort_unstable();
    let pb = crate::util::progress_bar(ids.len() as u64, "export");
    let mut count = 0usize;
    {
        let mut writer = BufWriter::new(staged.as_file_mut());
        for id in ids {
            if let Some(record) = handle.export_node(id)? {
                writeln!(writer, "{}", serde_json::to_string(&record)?)?;
                count += 1;
            }
            pb.inc(1);
        }
        writer.flush()?;
    }
    pb.finish_and_clear();
    staged.as_file().sync_all()?;
    validate_output_path(db_path, output)?;
    staged.persist(&output_path).map_err(|error| error.error)?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    println!(
        "{} 已导出 {count} 个节点 / Exported {count} nodes -> {output}",
        "✓".green().bold()
    );
    Ok(())
}

pub(crate) fn validate_output_path(db_path: &str, output: &str) -> triviumdb::Result<PathBuf> {
    triviumdb::storage::artifacts::validate_destination(Path::new(db_path), Path::new(output))
        .map_err(|error| {
            if matches!(error.underlying(), triviumdb::TriviumError::InvalidInput(_)) {
                triviumdb::TriviumError::InvalidInput(format!(
                    "拒绝导出到数据库相关文件 (Refusing to export to a database-related file): {error}"
                )).at_field("destination")
            } else {
                error
            }
        })
}
