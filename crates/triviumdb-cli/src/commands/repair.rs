//! `repair` 子命令：数据库诊断与修复（迁移自旧 `trivium_repair` 工具）。

use std::collections::HashMap;
use std::path::Path;

use colored::Colorize;

use crate::CliResult;
use crate::db_handle::{CliRows, CliValue, DType, DbHandle, sniff_header};
use crate::formatter::{OutputFormat, format_rows};

/// 两个诊断命令共用基础检查点准入，保持原始 I/O 与格式错误类别。
fn inspect_inputs(path: &str) -> Result<bool, triviumdb::TriviumError> {
    let wal_exists = Path::new(&format!("{path}.wal")).try_exists()?;
    if !Path::new(path).try_exists()? {
        if wal_exists {
            return Err(triviumdb::TriviumError::CorruptedFile(
                "缺少带类型的基础检查点，请保留 WAL 并使用原库的基础检查点恢复 (Typed base checkpoint missing; preserve the WAL and recover with the original database's base checkpoint)".into(),
            ));
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("主库和 WAL 均不存在 (Database and WAL are missing): {path}"),
        )
        .into());
    }
    Ok(wal_exists)
}

/// 快速检查文件头与 WAL 是否存在。
pub fn check(path: &str) -> CliResult {
    let wal_exists = inspect_inputs(path)?;
    let header = sniff_header(path)?;
    println!(
        "{} {}",
        "扫描数据库环境 / Scanning database environment:".bold(),
        path
    );
    println!("   ├─ WAL 日志文件存在 / WAL file exists: {wal_exists}");
    println!(
        "   └─ 架构参数 / Schema => 版本 / Version: v{}, 维度 / Dimension: {}",
        header.version, header.dim
    );
    println!(
        "{}",
        "当前格式文件头校验通过。/ Current-format database header validated.".green()
    );
    Ok(())
}

/// 从已发布的检查点只读导出节点。
pub fn dump(path: &str, dtype: DType, format: OutputFormat) -> CliResult {
    inspect_inputs(path)?;
    let handle = DbHandle::open_read_only(path, None, dtype)?;
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
