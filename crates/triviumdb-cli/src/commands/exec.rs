//! `exec` 子命令：非交互执行单条 TQL（读 / 写）。

use std::time::Instant;

use colored::Colorize;

use crate::CliResult;
use crate::db_handle::DbHandle;
use crate::formatter::{OutputFormat, format_csv, format_mutation, format_rows};

pub fn run(handle: &mut DbHandle, query: &str, mutate: bool, format: OutputFormat) -> CliResult {
    let start = Instant::now();

    // 文本模式提供位置诊断；JSON 模式交给统一错误出口序列化。
    let parsed = if mutate {
        triviumdb::query::tql_parser::parse_tql_statement_diagnostic(query).map(|_| None)
    } else {
        triviumdb::query::tql_parser::parse_tql_diagnostic(query).map(Some)
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
        if let Some(output) = format_mutation(&summary, format) {
            println!("{output}");
        } else {
            println!(
                "{} 影响行数 / affected={}, 新建 ID / created_ids={:?} ({:.2?})",
                "OK".green().bold(),
                summary.affected,
                summary.created_ids,
                start.elapsed()
            );
        }
    } else {
        let rows = handle.tql(query)?;
        let n = rows.len();
        let output = match (format, parsed?) {
            (OutputFormat::Csv, Some(query)) if rows.is_empty() => format_csv(
                &rows,
                triviumdb::query::tql_executor::result_columns(&query),
            ),
            _ => format_rows(&rows, format),
        };
        println!("{}", output.trim_end_matches('\n'));
        let elapsed = start.elapsed();
        // 计时信息走 stderr，避免污染管道 / JSON 输出
        eprintln!(
            "{}",
            format!("{n} 行 / row(s)，耗时 / elapsed {elapsed:.2?}").dimmed()
        );
    }

    Ok(())
}
