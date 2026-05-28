//! `info` 子命令：显示数据库元信息（文件头 + 实时统计）。

use std::path::Path;

use serde_json::json;
use tabled::builder::Builder;
use tabled::settings::Style;

use crate::CliResult;
use crate::db_handle::{DType, DbHandle, sniff_header};
use crate::formatter::OutputFormat;
use crate::util::{file_size, human_bytes_opt};

/// 非交互入口：打开数据库后展示元信息。
pub fn run(path: &str, dim: Option<usize>, dtype: DType, format: OutputFormat) -> CliResult {
    let handle = DbHandle::open_auto(path, dim, dtype)?;
    print_info(&handle, path, format)
}

/// 基于已打开的句柄展示元信息（供 REPL `.info` 复用，避免重复加锁打开）。
pub fn print_info(handle: &DbHandle, path: &str, format: OutputFormat) -> CliResult {
    // 不挂载也能读取的文件头信息
    let header = sniff_header(path).ok();
    let dtype = handle.dtype();

    let tdb_size = file_size(path);
    let vec_path = format!("{path}.vec");
    let wal_path = format!("{path}.wal");
    let quiver_path = format!("{path}.quiver");
    let vec_size = file_size(&vec_path);
    let wal_size = file_size(&wal_path);
    let quiver_exists = Path::new(&quiver_path).exists();

    let node_count = handle.node_count();
    let actual_dim = handle.dim();
    let mem = handle.estimated_memory();
    let file_version = header.map(|h| h.version);

    match format {
        OutputFormat::Json => {
            let v = json!({
                "path": path,
                "file_version": file_version,
                "dimension": actual_dim,
                "dtype": dtype.as_str(),
                "node_count": node_count,
                "estimated_memory_bytes": mem,
                "files": {
                    "tdb_bytes": tdb_size,
                    "vec_bytes": vec_size,
                    "wal_bytes": wal_size,
                    "quiver_present": quiver_exists,
                },
            });
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        _ => {
            // 预先计算所有 owned 字符串，保证 push_record 数组元素类型一致 (&str)
            let version_str = file_version
                .map(|v| format!("v{v}"))
                .unwrap_or_else(|| "—".into());
            let dim_str = actual_dim.to_string();
            let nodes_str = fmt_int(node_count as u64);
            let mem_str = human_bytes_opt(Some(mem as u64));
            let tdb_str = human_bytes_opt(tdb_size);
            let vec_str = human_bytes_opt(vec_size);
            let wal_str = format!(
                "{}{}",
                human_bytes_opt(wal_size),
                if wal_size.unwrap_or(0) > 0 { " (pending)" } else { "" }
            );
            let quiver_str = if quiver_exists { "present" } else { "—" };

            let mut b = Builder::default();
            b.push_record(["字段 (Field)", "值 (Value)"]);
            b.push_record(["Path", path]);
            b.push_record(["File Version", version_str.as_str()]);
            b.push_record(["Dimension", dim_str.as_str()]);
            b.push_record(["Data Type", dtype.as_str()]);
            b.push_record(["Node Count", nodes_str.as_str()]);
            b.push_record(["Estimated Memory", mem_str.as_str()]);
            b.push_record(["File .tdb", tdb_str.as_str()]);
            b.push_record(["File .vec", vec_str.as_str()]);
            b.push_record(["WAL", wal_str.as_str()]);
            b.push_record(["QuIVer Index", quiver_str]);

            let mut table = b.build();
            table.with(Style::rounded());
            println!("{table}");
        }
    }

    Ok(())
}

/// 千位分隔的整数格式化。
fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let len = bytes.len();
    for (i, c) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}
