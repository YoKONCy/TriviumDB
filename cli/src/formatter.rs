//! 输出格式化：把 [`CliRows`] / [`CliNode`] 渲染成 table / json / csv 文本。
//!
//! 被非交互命令、REPL 共享使用。TUI 有自己的渲染逻辑，不走这里。

use serde_json::{json, Value};
use tabled::builder::Builder;
use tabled::settings::{Style, Width};

use crate::db_handle::{CliNode, CliRows};

/// 输出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Table,
    Json,
    Csv,
}

impl OutputFormat {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "table" => Ok(OutputFormat::Table),
            "json" => Ok(OutputFormat::Json),
            "csv" => Ok(OutputFormat::Csv),
            other => Err(format!("未知输出格式: '{other}' (支持: table / json / csv)")),
        }
    }
}

impl std::str::FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        OutputFormat::parse(s)
    }
}

/// 把一个节点压缩成 `#id {payload}` 形式的单元格文本。
fn node_cell(node: &CliNode) -> String {
    format!("#{} {}", node.id, compact_json(&node.payload))
}

/// 紧凑 JSON：对象去掉多余空格，过长时截断。
fn compact_json(v: &Value) -> String {
    let s = v.to_string();
    const MAX: usize = 120;
    if s.chars().count() > MAX {
        let truncated: String = s.chars().take(MAX).collect();
        format!("{truncated}…")
    } else {
        s
    }
}

/// 收集所有行里出现过的变量名（保持稳定排序）。
fn collect_columns(rows: &CliRows) -> Vec<String> {
    let mut cols: Vec<String> = Vec::new();
    for row in rows {
        for k in row.keys() {
            if !cols.contains(k) {
                cols.push(k.clone());
            }
        }
    }
    cols.sort();
    cols
}

/// 渲染 TQL 查询结果。
pub fn format_rows(rows: &CliRows, format: OutputFormat) -> String {
    if rows.is_empty() {
        return match format {
            OutputFormat::Json => "[]".to_string(),
            _ => "(0 rows)".to_string(),
        };
    }

    match format {
        OutputFormat::Json => rows_to_json(rows),
        OutputFormat::Table => rows_to_table(rows),
        OutputFormat::Csv => rows_to_csv(rows),
    }
}

fn node_to_json(node: &CliNode) -> Value {
    json!({
        "id": node.id,
        "payload": node.payload,
        "edges": node.edges.iter().map(|e| json!({
            "target": e.target_id,
            "label": e.label,
            "weight": e.weight,
        })).collect::<Vec<_>>(),
        "vector_dim": node.vector.len(),
    })
}

fn rows_to_json(rows: &CliRows) -> String {
    let arr: Vec<Value> = rows
        .iter()
        .map(|row| {
            let obj: serde_json::Map<String, Value> = row
                .iter()
                .map(|(k, v)| (k.clone(), node_to_json(v)))
                .collect();
            Value::Object(obj)
        })
        .collect();
    serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
}

fn rows_to_table(rows: &CliRows) -> String {
    let cols = collect_columns(rows);
    let mut builder = Builder::default();

    let mut header = Vec::with_capacity(cols.len() + 1);
    header.push("#".to_string());
    header.extend(cols.iter().cloned());
    builder.push_record(header);

    for (i, row) in rows.iter().enumerate() {
        let mut record = Vec::with_capacity(cols.len() + 1);
        record.push((i + 1).to_string());
        for col in &cols {
            match row.get(col) {
                Some(node) => record.push(node_cell(node)),
                None => record.push(String::new()),
            }
        }
        builder.push_record(record);
    }

    let mut table = builder.build();
    table.with(Style::rounded()).with(Width::wrap(140).keep_words(true));
    format!("{table}\n{} row(s)", rows.len())
}

fn rows_to_csv(rows: &CliRows) -> String {
    let cols = collect_columns(rows);
    let mut out = String::new();

    // header
    out.push_str("#,");
    out.push_str(
        &cols
            .iter()
            .map(|c| csv_escape(c))
            .collect::<Vec<_>>()
            .join(","),
    );
    out.push('\n');

    for (i, row) in rows.iter().enumerate() {
        out.push_str(&(i + 1).to_string());
        for col in &cols {
            out.push(',');
            if let Some(node) = row.get(col) {
                out.push_str(&csv_escape(&node_cell(node)));
            }
        }
        out.push('\n');
    }
    out
}

/// CSV 字段转义：含逗号/引号/换行时用双引号包裹并转义内部引号。
fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
