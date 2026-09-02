//! REPL 模式：交互式 TQL + 点号元命令。

mod completer;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

use colored::Colorize;
use rustyline::Editor;
use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use serde_json::Value;

use crate::CliResult;
use crate::commands;
use crate::db_handle::DbHandle;
use crate::formatter::{OutputFormat, format_rows};
use crate::util::human_bytes;
use completer::ReplHelper;

pub fn run(mut handle: DbHandle, path: &str, format: OutputFormat) -> CliResult {
    let mut current_format = format;
    banner(&handle, path);

    let mut rl: Editor<ReplHelper, FileHistory> = Editor::new()?;
    rl.set_helper(Some(ReplHelper));

    let hist = history_file();
    if let Some(h) = &hist {
        let _ = rl.load_history(h);
    }

    loop {
        match rl.readline("tql> ") {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(input);
                if input.starts_with('.') {
                    if handle_meta(input, &mut handle, path, &mut current_format) {
                        break;
                    }
                } else {
                    run_tql(input, &mut handle, current_format);
                }
            }
            Err(ReadlineError::Interrupted) => continue, // Ctrl-C：取消当前行
            Err(ReadlineError::Eof) => break,            // Ctrl-D：退出
            Err(e) => {
                eprintln!("{} {e}", "错误 / Error:".red());
                break;
            }
        }
    }

    if let Some(h) = &hist {
        let _ = rl.save_history(h);
    }
    println!("再见 / Bye");
    Ok(())
}

fn banner(handle: &DbHandle, path: &str) {
    let line = format!(
        "TriviumDB REPL | {path} | dim={} | nodes={} | {}",
        handle.dim(),
        handle.node_count(),
        handle.dtype()
    );
    println!("{}", line.cyan().bold());
    println!(
        "{}",
        "直接输入 TQL 执行，或用 .help 查看元命令，.quit 退出。/ Enter TQL directly, use .help for meta commands, or .quit to exit.".dimmed()
    );
}

/// 执行一条 TQL（自动判定读 / 写）。
fn run_tql(input: &str, handle: &mut DbHandle, format: OutputFormat) {
    let query = input.trim_end_matches(';').trim();
    let start = Instant::now();

    // 先做带位置的预校验，错误时渲染 caret 诊断
    if is_mutation(query) {
        if let Err(err) = triviumdb::query::tql_parser::parse_tql_statement_with_pos(query) {
            eprint!(
                "{}",
                crate::diagnostics::Diagnostic::from_parse_error(query, &err).render_ansi()
            );
            return;
        }
        match handle.tql_mut(query) {
            Ok(s) => {
                let flush = handle.flush();
                println!(
                    "{} 影响行数 / affected={}, 新建 ID / created_ids={:?} ({:.2?})",
                    "OK".green().bold(),
                    s.affected,
                    s.created_ids,
                    start.elapsed()
                );
                if let Err(e) = flush {
                    eprintln!(
                        "{} 写入成功但落盘失败 / Write succeeded but flush failed: {e}",
                        "警告 / Warning:".yellow()
                    );
                }
            }
            Err(e) => eprintln!("{} {e}", "错误 / Error:".red()),
        }
    } else {
        if let Err(err) = triviumdb::query::tql_parser::parse_tql_with_pos(query) {
            eprint!(
                "{}",
                crate::diagnostics::Diagnostic::from_parse_error(query, &err).render_ansi()
            );
            return;
        }
        match handle.tql(query) {
            Ok(rows) => {
                let n = rows.len();
                println!("{}", format_rows(&rows, format));
                println!(
                    "{}",
                    format!("{n} 行 / row(s)，耗时 / elapsed {:.2?}", start.elapsed()).dimmed()
                );
            }
            Err(e) => eprintln!("{} {e}", "错误 / Error:".red()),
        }
    }
}

/// 通过首关键词粗判是否为写操作。
fn is_mutation(query: &str) -> bool {
    let up = query.trim_start().to_ascii_uppercase();
    ["CREATE", "SET", "DELETE", "DETACH", "MERGE", "REMOVE"]
        .iter()
        .any(|kw| up.starts_with(kw))
}

/// 处理点号元命令，返回 `true` 表示请求退出。
fn handle_meta(input: &str, handle: &mut DbHandle, path: &str, format: &mut OutputFormat) -> bool {
    let mut parts = input.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();

    match cmd {
        ".quit" | ".exit" | ".q" => return true,
        ".help" | ".h" | ".?" => print_help(),
        ".info" => {
            let _ = commands::info::print_info(handle, path, *format);
        }
        ".stats" => print_stats(handle),
        ".flush" => match handle.flush() {
            Ok(_) => println!("{} 已落盘 / Flushed", "✓".green()),
            Err(e) => eprintln!("{} {e}", "错误 / Error:".red()),
        },
        ".compact" => {
            let _ = commands::compact::run(handle);
        }
        ".export" => {
            if arg.is_empty() {
                eprintln!("用法 / Usage: .export <file.jsonl>");
            } else {
                let _ = commands::export::run(handle, path, arg);
            }
        }
        ".format" => match OutputFormat::parse(arg) {
            Ok(f) => {
                *format = f;
                println!("输出格式已切换为 / Output format changed to: {arg}");
            }
            Err(e) => eprintln!("{} {e}", "错误 / Error:".red()),
        },
        ".schema" => print_schema(handle),
        other => eprintln!(
            "未知命令 / Unknown command: {other}（输入 .help 查看帮助 / Enter .help for help）"
        ),
    }
    false
}

fn print_help() {
    let lines = [
        (
            ".info",
            "数据库元信息（维度、节点数、文件大小、WAL、QuIVer）/ Database metadata (dimension, nodes, file size, WAL, QuIVer)",
        ),
        (
            ".stats",
            "实时统计（节点数、内存占用）/ Live statistics (nodes, memory)",
        ),
        (
            ".schema",
            "采样 payload 字段分布 / Sample payload field distribution",
        ),
        (".flush", "手动落盘 / Flush manually"),
        (".compact", "触发压实 / Trigger compaction"),
        (
            ".export <file>",
            "导出全部节点为 JSONL / Export all nodes as JSONL",
        ),
        (
            ".format <table|json|csv>",
            "切换输出格式 / Change output format",
        ),
        (".help", "显示本帮助 / Show this help"),
        (".quit / .exit", "退出 REPL / Exit REPL"),
    ];
    println!("{}", "元命令 / Meta commands:".bold());
    for (cmd, desc) in lines {
        println!("  {:<28} {}", cmd.cyan(), desc);
    }
    println!(
        "{}",
        "其余输入按 TQL 解析执行。/ Other input is parsed and executed as TQL.".dimmed()
    );
}

fn print_stats(handle: &DbHandle) {
    println!(
        "nodes={} | dim={} | dtype={} | est_memory={}",
        handle.node_count(),
        handle.dim(),
        handle.dtype(),
        human_bytes(handle.estimated_memory() as u64),
    );
}

/// 采样最多 1000 个节点，统计 payload 顶层字段出现次数与类型集合。
fn print_schema(handle: &DbHandle) {
    let ids = handle.get_all_ids();
    let mut fields: BTreeMap<String, (usize, BTreeSet<&'static str>)> = BTreeMap::new();
    let mut sampled = 0usize;

    for id in ids.iter().take(1000) {
        if let Some(Value::Object(obj)) = handle.get_payload(*id) {
            for (k, v) in &obj {
                let entry = fields.entry(k.clone()).or_default();
                entry.0 += 1;
                entry.1.insert(json_type(v));
            }
            sampled += 1;
        }
    }

    if fields.is_empty() {
        println!(
            "（无 payload 字段，采样 {sampled} 个节点 / No payload fields; sampled {sampled} nodes）"
        );
        return;
    }

    println!(
        "{}（采样 {sampled} 个节点 / sampled {sampled} nodes）",
        "Payload 结构 / Payload Schema:".bold()
    );
    for (field, (count, types)) in &fields {
        let types: Vec<&str> = types.iter().copied().collect();
        println!(
            "  {:<24} {:>5}  [{}]",
            field.cyan(),
            count,
            types.join(", ")
        );
    }
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn history_file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".triviumdb_history"))
}
