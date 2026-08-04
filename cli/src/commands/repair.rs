//! `repair` 子命令：数据库诊断与修复（迁移自旧 `trivium_repair` 工具）。

use std::collections::HashMap;
use std::path::Path;

use colored::Colorize;

use crate::CliResult;
use crate::db_handle::{CliRows, DType, DbHandle, sniff_header};
use crate::formatter::{OutputFormat, format_rows};

/// 快速检查文件头与 WAL/主库存在性（不挂载数据库）。
pub fn check(path: &str) -> CliResult {
    let wal_path = format!("{path}.wal");
    let tdb_exists = Path::new(path).exists();
    let wal_exists = Path::new(&wal_path).exists();

    println!("{} {}", "扫描数据库环境:".bold(), path);
    println!("   ├─ WAL 日志文件存在: {wal_exists}");
    println!("   ├─ TDB 主库文件存在: {tdb_exists}");

    if tdb_exists {
        match sniff_header(path) {
            Ok(h) => {
                println!(
                    "   └─ 架构参数 => Version: v{}, Dimension: {}",
                    h.version, h.dim
                );
                println!("{}", "数据库头部完好，可安全挂载。".green());
            }
            Err(e) => {
                println!("{} {e}", "头部探查失败 (主库可能已物理损坏):".red());
            }
        }
    } else {
        println!(
            "{}",
            "警告: 主库文件不存在！若仅有 WAL，下次启动会尝试纯靠 WAL 追平数据。".yellow()
        );
    }

    Ok(())
}

/// 强制挂载并导出全部节点（维度从文件头嗅探，缺省回退为 4 以防 panic）。
pub fn dump(path: &str, dtype: DType, format: OutputFormat) -> CliResult {
    let dim = sniff_header(path).map(|h| h.dim).unwrap_or(4);
    println!("尝试以维度 {dim} 强制挂载数据库...");

    let handle = DbHandle::open(path, dim, dtype)?;
    println!(
        "{} 存活节点总数: {}",
        "挂载成功！".green(),
        handle.node_count()
    );
    println!("{}", "─".repeat(50));

    let mut rows: CliRows = Vec::new();
    for id in handle.get_all_ids() {
        if let Some(node) = handle.get(id) {
            let mut row = HashMap::new();
            row.insert("node".to_string(), node);
            rows.push(row);
        }
    }

    println!("{}", format_rows(&rows, format));
    Ok(())
}
