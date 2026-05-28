//! `import` 子命令：从 JSONL 批量导入节点。
//!
//! 采用两遍策略：第一遍插入所有节点（保留显式 `id`），第二遍建立边，
//! 确保边引用的目标节点已存在。行格式与 `export` 一致。

use std::fs::File;
use std::io::{BufRead, BufReader};

use colored::Colorize;
use serde_json::Value;

use crate::CliResult;
use crate::db_handle::DbHandle;

struct Record {
    id: Option<u64>,
    vector: Vec<f32>,
    payload: Value,
    edges: Vec<(u64, String, f32)>,
}

pub fn run(handle: &mut DbHandle, input: &str) -> CliResult {
    let file = File::open(input)?;
    let reader = BufReader::new(file);

    let mut records: Vec<Record> = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(trimmed)
            .map_err(|e| format!("第 {} 行 JSON 解析失败: {e}", lineno + 1))?;
        records.push(parse_record(&v));
    }

    // Pass 1: 插入节点
    let mut inserted = 0usize;
    for rec in &records {
        match rec.id {
            Some(id) => handle.insert_with_id_f32(id, &rec.vector, rec.payload.clone())?,
            None => {
                handle.insert_f32(&rec.vector, rec.payload.clone())?;
            }
        }
        inserted += 1;
    }

    // Pass 2: 建立边（仅对有显式 id 的记录）
    let mut linked = 0usize;
    for rec in &records {
        if let Some(src) = rec.id {
            for (target, label, weight) in &rec.edges {
                if handle.link(src, *target, label, *weight).is_ok() {
                    linked += 1;
                }
            }
        }
    }

    handle.flush()?;
    println!(
        "{} 导入 {inserted} 个节点, {linked} 条边 <- {input}",
        "✓".green().bold()
    );
    Ok(())
}

fn parse_record(v: &Value) -> Record {
    let id = v.get("id").and_then(|x| x.as_u64());
    let vector = v
        .get("vector")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|n| n.as_f64().map(|f| f as f32))
                .collect::<Vec<f32>>()
        })
        .unwrap_or_default();
    let payload = v.get("payload").cloned().unwrap_or(Value::Null);
    let edges = v
        .get("edges")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    let target = e.get("target").and_then(|t| t.as_u64())?;
                    let label = e
                        .get("label")
                        .and_then(|l| l.as_str())
                        .unwrap_or("")
                        .to_string();
                    let weight = e.get("weight").and_then(|w| w.as_f64()).unwrap_or(1.0) as f32;
                    Some((target, label, weight))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Record {
        id,
        vector,
        payload,
        edges,
    }
}
