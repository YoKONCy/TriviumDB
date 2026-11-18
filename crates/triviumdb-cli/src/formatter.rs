//! 输出格式化：把 [`CliRows`] / [`CliNode`] 渲染成 table / json / csv 文本。
//!
//! 被非交互命令、REPL 共享使用。TUI 有自己的渲染逻辑，不走这里。

use serde_json::{Value, json};
use tabled::builder::Builder;
use tabled::settings::{Style, Width};

use crate::db_handle::{CliNode, CliRows, CliValue, MutSummary};

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
            other => Err(format!(
                "未知输出格式: '{other}' (支持: table / json / csv)"
            )),
        }
    }
}

impl std::str::FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        OutputFormat::parse(s)
    }
}

/// 用紧凑文本显示投影值，节点显示为 `#id {payload}`。
fn table_cell(value: &CliValue) -> String {
    match value {
        CliValue::Node(node) => format!("#{} {}", node.id, compact_text(node.payload.to_string())),
        CliValue::Json(value) => compact_text(json_cell(value)),
    }
}

/// 表格显示对长文本做截断。
fn compact_text(s: String) -> String {
    const MAX: usize = 120;
    if s.chars().count() > MAX {
        let truncated: String = s.chars().take(MAX).collect();
        format!("{truncated}…")
    } else {
        s
    }
}

fn json_cell(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        value => value.to_string(),
    }
}

pub fn format_mutation(summary: &MutSummary, format: OutputFormat) -> Option<String> {
    match format {
        OutputFormat::Json => Some(
            json!({
                "affected": summary.affected,
                "created_ids": summary.created_ids,
            })
            .to_string(),
        ),
        OutputFormat::Csv => Some(format!(
            "affected,created_ids\n{},{}",
            summary.affected,
            csv_escape(&json!(&summary.created_ids).to_string()),
        )),
        OutputFormat::Table => None,
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
    match format {
        OutputFormat::Json => rows_to_json(rows),
        OutputFormat::Table if rows.is_empty() => "(0 rows)".to_string(),
        OutputFormat::Table => rows_to_table(rows),
        OutputFormat::Csv => format_csv(rows, collect_columns(rows)),
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
                .map(|(name, value)| {
                    let json = match value {
                        CliValue::Node(node) => node_to_json(node),
                        CliValue::Json(value) => value.clone(),
                    };
                    (name.clone(), json)
                })
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
                Some(value) => record.push(table_cell(value)),
                None => record.push(String::new()),
            }
        }
        builder.push_record(record);
    }

    let mut table = builder.build();
    table
        .with(Style::rounded())
        .with(Width::wrap(140).keep_words(true));
    format!("{table}\n{} row(s)", rows.len())
}

pub fn format_csv(rows: &CliRows, mut cols: Vec<String>) -> String {
    cols.sort();
    let mut out = String::new();

    // header
    out.push('#');
    for column in &cols {
        out.push(',');
        out.push_str(&csv_escape(column));
    }
    out.push('\n');

    for (i, row) in rows.iter().enumerate() {
        out.push_str(&(i + 1).to_string());
        for col in &cols {
            out.push(',');
            if let Some(value) = row.get(col) {
                let cell = match value {
                    CliValue::Node(node) => format!("#{} {}", node.id, node.payload),
                    CliValue::Json(value) => json_cell(value),
                };
                out.push_str(&csv_escape(&cell));
            }
        }
        out.push('\n');
    }
    out
}

/// CSV 字段转义：含逗号、引号或换行字符时用双引号包裹并转义内部引号。
fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_handle::CliNode;
    use std::collections::HashMap;

    fn make_node(id: u64, name: &str) -> CliValue {
        CliValue::Node(CliNode {
            id,
            vector: vec![1.0, 0.0, 0.0, 0.0],
            payload: serde_json::json!({"name": name}),
            edges: vec![],
        })
    }

    fn two_rows() -> CliRows {
        let mut r1 = HashMap::new();
        r1.insert("n".into(), make_node(1, "Alice"));
        let mut r2 = HashMap::new();
        r2.insert("n".into(), make_node(2, "Bob"));
        vec![r1, r2]
    }

    // ── format_rows empty ──────────────────────────────────────

    #[test]
    fn empty_rows_json() {
        assert_eq!(format_rows(&vec![], OutputFormat::Json), "[]");
    }

    #[test]
    fn empty_rows_csv() {
        assert_eq!(format_csv(&vec![], vec!["id".into()]), "#,id\n");
    }

    // ── format_rows non-empty ──────────────────────────────────

    #[test]
    fn json_format_valid_structure() {
        let rows = two_rows();
        let output = format_rows(&rows, OutputFormat::Json);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0]["n"]["id"].is_u64());
        assert_eq!(parsed[0]["n"]["payload"]["name"].as_str().unwrap(), "Alice");
        assert!(parsed[0]["n"]["edges"].is_array());
        assert!(parsed[0]["n"]["vector_dim"].is_u64());
    }

    #[test]
    fn csv_format_has_header_and_data() {
        let rows = two_rows();
        let output = format_rows(&rows, OutputFormat::Csv);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("#,"));
        assert!(lines[1].starts_with("1,"));
        assert!(lines[2].starts_with("2,"));
    }

    // ── csv_escape ─────────────────────────────────────────────

    #[test]
    fn csv_escape_plain_text() {
        assert_eq!(csv_escape("hello"), "hello");
    }

    #[test]
    fn csv_escape_with_comma() {
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
    }

    #[test]
    fn csv_escape_with_quotes() {
        assert_eq!(csv_escape(r#"say "hi""#), r#""say ""hi""""#);
    }

    #[test]
    fn csv_escape_with_newline() {
        assert_eq!(csv_escape("line1\nline2"), "\"line1\nline2\"");
    }

    // ── collect_columns ────────────────────────────────────────

    #[test]
    fn collect_columns_sorted_and_deduped() {
        let mut r1 = HashMap::new();
        r1.insert("b".into(), make_node(1, "X"));
        r1.insert("a".into(), make_node(2, "Y"));
        let mut r2 = HashMap::new();
        r2.insert("a".into(), make_node(3, "Z"));
        r2.insert("c".into(), make_node(4, "W"));
        let cols = collect_columns(&vec![r1, r2]);
        assert_eq!(cols, vec!["a", "b", "c"]);
    }

    // ── OutputFormat::parse ────────────────────────────────────

    #[test]
    fn output_format_parse_valid() {
        assert_eq!(OutputFormat::parse("table").unwrap(), OutputFormat::Table);
        assert_eq!(OutputFormat::parse("JSON").unwrap(), OutputFormat::Json);
        assert_eq!(OutputFormat::parse(" Csv ").unwrap(), OutputFormat::Csv);
    }

    #[test]
    fn output_format_parse_invalid() {
        assert!(OutputFormat::parse("xml").is_err());
    }
}
