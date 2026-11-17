//! `export` 子命令按原始 dtype 导出节点、Payload 和完整边。

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use colored::Colorize;

use crate::CliResult;
use crate::db_handle::DbHandle;

pub fn run(handle: &DbHandle, db_path: &str, output: &str) -> CliResult {
    validate_output_path(db_path, output)?;
    let output_path = normalized_path(Path::new(output))?;
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

// 固定制品及其临时、代际文件共享受保护的名称族。
use triviumdb::storage::artifacts::{
    RESERVED_SUFFIXES as PROTECTED_SUFFIXES, is_reserved_name as protected_name,
};

pub(crate) fn validate_output_path(db_path: &str, output: &str) -> CliResult {
    let database = normalized_path(Path::new(db_path))?;
    let output_path = normalized_path(Path::new(output))?;
    let parent = database
        .parent()
        .ok_or("数据库路径缺少目录 (Database path has no parent)")?;
    let name = database
        .file_name()
        .ok_or("数据库路径缺少文件名 (Database path has no file name)")?
        .to_string_lossy();
    let mut protected = vec![database.clone()];
    protected.extend(
        PROTECTED_SUFFIXES
            .iter()
            .map(|suffix| parent.join(format!("{name}{suffix}"))),
    );
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        if protected_name(&entry.file_name().to_string_lossy(), &name) {
            protected.push(entry.path());
        }
    }
    let reserved_name = output_path
        .parent()
        .is_some_and(|directory| paths_equal(directory, parent))
        && output_path
            .file_name()
            .is_some_and(|file| protected_name(&file.to_string_lossy(), &name));
    let mut conflicts = reserved_name;
    for path in protected {
        let resolved = normalized_path(&path)?;
        conflicts |= paths_equal(&output_path, &resolved)
            || output_path.starts_with(&resolved) && path.is_dir();
        if output_path.try_exists()? && path.try_exists()? {
            conflicts |= same_file::is_same_file(&output_path, &path)?;
        }
    }
    if conflicts {
        return Err(format!(
            "拒绝导出到数据库相关文件 (Refusing to export to a database-related file): {output}"
        )
        .into());
    }
    Ok(())
}

fn normalized_path(path: &Path) -> std::io::Result<PathBuf> {
    if path.try_exists()? {
        return path.canonicalize();
    }
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "路径缺少文件名 (Path has no file name)",
        )
    })?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent.canonicalize()?.join(name))
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
