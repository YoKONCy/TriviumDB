//! `exec` 子命令：非交互执行单条 TQL（读 / 写）。

use std::time::Instant;

use colored::Colorize;

use crate::CliResult;
use crate::db_handle::DbHandle;
use crate::formatter::{OutputFormat, format_rows};

pub fn run(handle: &mut DbHandle, query: &str, mutate: bool, format: OutputFormat) -> CliResult {
    let start = Instant::now();

    // 解析阶段错误带位置高亮（caret），优于裸字符串
    if mutate {
        if let Err(err) = triviumdb::query::tql_parser::parse_tql_statement_with_pos(query) {
            eprint!(
                "{}",
                crate::diagnostics::Diagnostic::from_parse_error(query, &err).render_ansi()
            );
            return Err(err.msg.into());
        }
    } else if let Err(err) = triviumdb::query::tql_parser::parse_tql_with_pos(query) {
        eprint!(
            "{}",
            crate::diagnostics::Diagnostic::from_parse_error(query, &err).render_ansi()
        );
        return Err(err.msg.into());
    }

    if mutate {
        let summary = handle.tql_mut(query)?;
        handle.flush()?;
        let elapsed = start.elapsed();
        println!(
            "{} affected={}, created_ids={:?} ({:.2?})",
            "OK".green().bold(),
            summary.affected,
            summary.created_ids,
            elapsed
        );
    } else {
        let rows = handle.tql(query)?;
        let n = rows.len();
        println!("{}", format_rows(&rows, format));
        let elapsed = start.elapsed();
        // 计时信息走 stderr，避免污染管道 / JSON 输出
        eprintln!("{}", format!("{n} row(s) in {elapsed:.2?}").dimmed());
    }

    Ok(())
}
