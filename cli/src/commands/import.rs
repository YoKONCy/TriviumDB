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
        records.push(parse_record(&v, lineno + 1)?);
    }

    // Pass 1: 插入节点
    let pb = crate::util::progress_bar(records.len() as u64, "nodes");
    let mut inserted = 0usize;
    for rec in &records {
        match rec.id {
            Some(id) => handle.insert_with_id_f32(id, &rec.vector, rec.payload.clone())?,
            None => {
                handle.insert_f32(&rec.vector, rec.payload.clone())?;
            }
        }
        inserted += 1;
        pb.inc(1);
    }
    pb.finish_and_clear();

    // Pass 2: 建立边（仅对有显式 id 的记录）
    let total_edges: u64 = records
        .iter()
        .filter(|r| r.id.is_some())
        .map(|r| r.edges.len() as u64)
        .sum();
    let pb = crate::util::progress_bar(total_edges, "edges");
    let mut linked = 0usize;
    let mut failed_edges = 0usize;
    for rec in &records {
        if let Some(src) = rec.id {
            for (target, label, weight) in &rec.edges {
                match handle.link(src, *target, label, *weight) {
                    Ok(_) => linked += 1,
                    Err(e) => {
                        failed_edges += 1;
                        eprintln!(
                            "warning: 边导入失败 src={src} target={target} label={label}: {e}"
                        );
                    }
                }
                pb.inc(1);
            }
        }
    }
    pb.finish_and_clear();

    handle.flush()?;
    if failed_edges == 0 {
        println!(
            "{} 导入 {inserted} 个节点, {linked} 条边 <- {input}",
            "✓".green().bold()
        );
    } else {
        println!(
            "{} 导入 {inserted} 个节点, {linked} 条边, {failed_edges} 条边失败 <- {input}",
            "✓".green().bold()
        );
    }
    Ok(())
}

fn parse_record(v: &Value, lineno: usize) -> Result<Record, String> {
    let obj = v
        .as_object()
        .ok_or_else(|| format!("第 {lineno} 行必须是 JSON 对象"))?;
    let id = match obj.get("id") {
        Some(x) => Some(
            x.as_u64()
                .ok_or_else(|| format!("第 {lineno} 行 id 必须是非负整数"))?,
        ),
        None => None,
    };
    let vector_value = obj
        .get("vector")
        .ok_or_else(|| format!("第 {lineno} 行缺少 vector 字段"))?;
    let vector_array = vector_value
        .as_array()
        .ok_or_else(|| format!("第 {lineno} 行 vector 必须是数组"))?;
    if vector_array.is_empty() {
        return Err(format!("第 {lineno} 行 vector 不能为空"));
    }
    let mut vector = Vec::with_capacity(vector_array.len());
    for (idx, item) in vector_array.iter().enumerate() {
        let value = item
            .as_f64()
            .ok_or_else(|| format!("第 {lineno} 行 vector[{idx}] 必须是数字"))?
            as f32;
        if !value.is_finite() {
            return Err(format!("第 {lineno} 行 vector[{idx}] 不是有限 f32 数值"));
        }
        vector.push(value);
    }
    let payload = obj.get("payload").cloned().unwrap_or(Value::Null);
    let edges = match obj.get("edges") {
        Some(value) => {
            let arr = value
                .as_array()
                .ok_or_else(|| format!("第 {lineno} 行 edges 必须是数组"))?;
            let mut edges = Vec::with_capacity(arr.len());
            for (idx, edge) in arr.iter().enumerate() {
                let edge_obj = edge
                    .as_object()
                    .ok_or_else(|| format!("第 {lineno} 行 edges[{idx}] 必须是对象"))?;
                let target = edge_obj
                    .get("target")
                    .and_then(|t| t.as_u64())
                    .ok_or_else(|| format!("第 {lineno} 行 edges[{idx}].target 必须是非负整数"))?;
                let label = match edge_obj.get("label") {
                    Some(label) => label
                        .as_str()
                        .ok_or_else(|| format!("第 {lineno} 行 edges[{idx}].label 必须是字符串"))?
                        .to_string(),
                    None => String::new(),
                };
                let weight = match edge_obj.get("weight") {
                    Some(weight) => {
                        let value = weight.as_f64().ok_or_else(|| {
                            format!("第 {lineno} 行 edges[{idx}].weight 必须是数字")
                        })? as f32;
                        if !value.is_finite() {
                            return Err(format!(
                                "第 {lineno} 行 edges[{idx}].weight 不是有限 f32 数值"
                            ));
                        }
                        value
                    }
                    None => 1.0,
                };
                edges.push((target, label, weight));
            }
            edges
        }
        None => Vec::new(),
    };

    Ok(Record {
        id,
        vector,
        payload,
        edges,
    })
}
