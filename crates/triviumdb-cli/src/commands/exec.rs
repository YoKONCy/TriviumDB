//! `exec` 子命令：非交互执行单条 TQL（读 / 写）。

use std::time::Instant;

use colored::Colorize;

use crate::CliResult;
use crate::db_handle::DbHandle;
use crate::formatter::{OutputFormat, format_rows};

pub fn run(handle: &mut DbHandle, query: &str, mutate: bool, format: OutputFormat) -> CliResult {
    let start = Instant::now();

    // 文本模式提供位置诊断；JSON 模式交给统一错误出口序列化。
    let parsed = if mutate {
        triviumdb::query::tql_parser::parse_tql_statement_diagnostic(query).map(|_| ())
    } else {
        triviumdb::query::tql_parser::parse_tql_diagnostic(query).map(|_| ())
    };
    if let Err(error) = parsed {
        if format != OutputFormat::Json
            && let triviumdb::TriviumError::QuerySyntax {
                reason,
                span: Some(span),
                ..
            } = &error
        {
            let err = triviumdb::query::tql_lexer::ParseErrorAt::new(reason, span.start_byte);
            eprint!(
                "{}",
                crate::diagnostics::Diagnostic::from_parse_error(query, &err).render_ansi()
            );
        }
        let error = if mutate {
            triviumdb::TriviumError::WriteFailure {
                source: Box::new(error),
                write: triviumdb::error::WriteErrorInfo {
                    phase: triviumdb::error::WritePhase::Validation,
                    outcome: triviumdb::error::WriteOutcome::NotCommitted,
                    transaction_consumed: None,
                },
            }
        } else {
            error
        };
        return Err(error.into());
    }

    if mutate {
        let summary = handle.tql_mut(query)?;
        handle.flush()?;
        let elapsed = start.elapsed();
        println!(
            "{} 影响行数 / affected={}, 新建 ID / created_ids={:?} ({:.2?})",
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
        eprintln!(
            "{}",
            format!("{n} 行 / row(s)，耗时 / elapsed {elapsed:.2?}").dimmed()
        );
    }

    Ok(())
}
