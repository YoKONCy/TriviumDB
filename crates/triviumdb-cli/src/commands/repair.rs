//! `repair` 子命令：数据库诊断与修复（迁移自旧 `trivium_repair` 工具）。

use std::collections::HashMap;
use std::path::Path;

use colored::Colorize;

use crate::CliResult;
use crate::db_handle::{CliRows, CliValue, DType, DbHandle, sniff_header};
use crate::formatter::{OutputFormat, format_rows};

fn inspect_inputs(path: &str) -> Result<(bool, bool), triviumdb::TriviumError> {
    let main_exists = Path::new(path).try_exists()?;
    let wal_exists = Path::new(&format!("{path}.wal")).try_exists()?;
    if !main_exists && !wal_exists {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("主库和 WAL 均不存在 (Database and WAL are missing): {path}"),
        )
        .into());
    }
    Ok((main_exists, wal_exists))
}

/// 快速检查文件头与 WAL/主库存在性（不挂载数据库）。
pub fn check(path: &str) -> CliResult {
    let (tdb_exists, wal_exists) = inspect_inputs(path)?;

    println!(
        "{} {}",
        "扫描数据库环境 / Scanning database environment:".bold(),
        path
    );
    println!("   ├─ WAL 日志文件存在 / WAL file exists: {wal_exists}");
    println!("   ├─ TDB 主库文件存在 / TDB database file exists: {tdb_exists}");

    if tdb_exists {
        match sniff_header(path) {
            Ok(h) => {
                println!(
                    "   └─ 架构参数 / Schema => 版本 / Version: v{}, 维度 / Dimension: {}",
                    h.version, h.dim
                );
                println!(
                    "{}",
                    "数据库头部完好，可安全挂载。/ Database header is intact and safe to open."
                        .green()
                );
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    } else {
        return Err(triviumdb::TriviumError::CorruptedFile(
            "缺少带类型的基础检查点，请保留 WAL 并使用原库的基础检查点恢复 (Typed base checkpoint missing; preserve the WAL and recover with the original database's base checkpoint)".into(),
        ).into());
    }

    Ok(())
}

/// 强制挂载并导出全部节点（维度从文件头嗅探，缺省回退为 4 以防 panic）。
pub fn dump(path: &str, dtype: DType, format: OutputFormat) -> CliResult {
    inspect_inputs(path)?;
    let dim = sniff_header(path).map(|h| h.dim).unwrap_or(4);
    eprintln!(
        "尝试以维度 {dim} 强制挂载数据库 / Attempting forced database open with dimension {dim}..."
    );

    let handle = DbHandle::open(path, dim, dtype)?;
    eprintln!(
        "{} 存活节点总数 / Live nodes: {}",
        "挂载成功 / Open succeeded!".green(),
        handle.node_count()?
    );

    let mut ids = handle.get_all_ids()?;
    ids.sort_unstable();
    let mut rows = CliRows::with_capacity(ids.len());
    for id in ids {
        let node = handle
            .get(id)?
            .ok_or(triviumdb::TriviumError::NodeNotFound(id))?;
        rows.push(HashMap::from([("node".to_string(), CliValue::Node(node))]));
    }

    println!("{}", format_rows(&rows, format).trim_end_matches('\n'));
    Ok(())
}
