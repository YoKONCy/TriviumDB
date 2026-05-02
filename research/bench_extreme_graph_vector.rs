use serde_json::json;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::Instant;
use triviumdb::Database;
use triviumdb::database::SearchConfig;

const NODE_COUNT: usize = 500_000;
const EDGE_COUNT: usize = 10_000_000;
const DIM: usize = 1536;
const TOP_K: usize = 10;
const QUERY_COUNT: usize = 8;
const VECTOR_SEED: u64 = 0x7a1f_2026_042f_5000;

struct ExtremeReport {
    nodes: usize,
    edges: usize,
    dim: usize,
    top_k: usize,
    query_count: usize,
    raw_vector_bytes: u64,
    estimated_edge_bytes: u64,
    trivium_memory_bytes: u64,
    trivium_disk_bytes: u64,
    rss_after_open: u64,
    rss_after_nodes: u64,
    rss_after_edges: u64,
    rss_after_flush: u64,
    rss_after_reopen: u64,
    rss_after_bruteforce: u64,
    rss_after_bq: u64,
    rss_after_neighbors: u64,
    rss_after_tql: u64,
    rss_peak_bytes: u64,
    node_insert_secs: f64,
    edge_insert_secs: f64,
    flush_secs: f64,
    reopen_secs: f64,
    brute_qps: f64,
    bq_qps: f64,
    bq_recall: f64,
    neighbors_qps: f64,
    neighbors_avg_count: f64,
    tql_expand_qps: f64,
    tql_expand_avg_rows: f64,
}

fn cleanup_db(path: &str) {
    for ext in &[
        "",
        ".wal",
        ".vec",
        ".lock",
        ".flush_ok",
        ".tmp",
        ".vec.tmp",
        ".flush_ok.tmp",
    ] {
        fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

fn disk_bytes(path: &str) -> u64 {
    ["", ".wal", ".vec", ".flush_ok"]
        .iter()
        .map(|ext| fs::metadata(format!("{}{}", path, ext)).map_or(0, |meta| meta.len()))
        .sum()
}

fn rss_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let Ok(status) = fs::read_to_string("/proc/self/status") else {
            return 0;
        };
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                return rest
                    .split_whitespace()
                    .next()
                    .and_then(|kb| kb.parse::<u64>().ok())
                    .map(|kb| kb * 1024)
                    .unwrap_or(0);
            }
        }
        0
    }

    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

fn update_peak_rss(peak: &mut u64) -> u64 {
    let current = rss_bytes();
    *peak = (*peak).max(current);
    current
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn unit_float(seed: u64) -> f32 {
    let bits = (splitmix64(seed) >> 40) as u32;
    bits as f32 / ((1u32 << 24) as f32)
}

fn vector_value(node_idx: usize, dim_idx: usize) -> f32 {
    let cluster = node_idx % 256;
    let base = if dim_idx % 256 == cluster { 1.0 } else { 0.0 };
    let noise = unit_float(
        VECTOR_SEED
            ^ ((node_idx as u64).wrapping_mul(0x9e37_79b1))
            ^ ((dim_idx as u64).wrapping_mul(0xbf58_476d)),
    ) - 0.5;
    base + noise * 0.08
}

fn gen_vector(node_idx: usize) -> Vec<f32> {
    let mut v: Vec<f32> = (0..DIM)
        .map(|dim_idx| vector_value(node_idx, dim_idx))
        .collect();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    for x in &mut v {
        *x /= norm;
    }
    v
}

fn dot_with_node(query: &[f32], node_idx: usize) -> f32 {
    let mut raw = [0.0f32; DIM];
    let mut norm = 0.0f32;
    for (dim_idx, slot) in raw.iter_mut().enumerate() {
        let value = vector_value(node_idx, dim_idx);
        *slot = value;
        norm += value * value;
    }
    let norm = norm.sqrt().max(1e-9);
    query
        .iter()
        .zip(raw.iter())
        .map(|(q, v)| q * (*v / norm))
        .sum()
}

fn brute_force_truth(query: &[f32], top_k: usize) -> Vec<u64> {
    let mut best: Vec<(u64, f32)> = Vec::with_capacity(top_k + 1);
    for node_idx in 0..NODE_COUNT {
        let score = dot_with_node(query, node_idx);
        if best.len() < top_k {
            best.push((node_idx as u64 + 1, score));
            best.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        } else if score > best.last().map(|(_, s)| *s).unwrap_or(f32::NEG_INFINITY) {
            best.pop();
            best.push((node_idx as u64 + 1, score));
            best.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        }
    }
    best.into_iter().map(|(id, _)| id).collect()
}

fn recall_at_k(ground_truth: &[u64], result: &[u64]) -> f64 {
    if ground_truth.is_empty() {
        return 1.0;
    }
    let gt_set: HashSet<u64> = ground_truth.iter().copied().collect();
    let hits = result.iter().filter(|id| gt_set.contains(id)).count();
    hits as f64 / ground_truth.len() as f64
}

fn write_extreme_report(report: &ExtremeReport) {
    fs::create_dir_all("target/bench-report").unwrap();
    let md = format!(
        "# 极限图向量规模资源占用与性能报告\n\n\
| 指标 | 数值 |\n\
|---|---:|\n\
| 节点数 | {} |\n\
| 边数 | {} |\n\
| 维度 | {} |\n\
| TopK | {} |\n\
| 查询数 | {} |\n\
| 原始向量数据量估算(bytes) | {} |\n\
| 边结构裸数据估算(bytes) | {} |\n\
| TriviumDB 内存估算(bytes) | {} |\n\
| TriviumDB 磁盘占用(bytes) | {} |\n\
| 打开数据库后 RSS(bytes) | {} |\n\
| 节点写入后 RSS(bytes) | {} |\n\
| 边写入后 RSS(bytes) | {} |\n\
| flush 后 RSS(bytes) | {} |\n\
| reopen 后 RSS(bytes) | {} |\n\
| BruteForce 查询后 RSS(bytes) | {} |\n\
| BQ 查询后 RSS(bytes) | {} |\n\
| neighbors 查询后 RSS(bytes) | {} |\n\
| TQL EXPAND 查询后 RSS(bytes) | {} |\n\
| RSS 峰值(bytes) | {} |\n\
| 节点写入耗时(s) | {:.3} |\n\
| 边写入耗时(s) | {:.3} |\n\
| flush 耗时(s) | {:.3} |\n\
| reopen 耗时(s) | {:.3} |\n\
| BruteForce QPS | {:.3} |\n\
| BQ 5% QPS | {:.3} |\n\
| BQ 5% Recall | {:.6} |\n\
| neighbors QPS | {:.3} |\n\
| neighbors 平均返回数 | {:.3} |\n\
| TQL EXPAND QPS | {:.3} |\n\
| TQL EXPAND 平均返回行数 | {:.3} |\n",
        report.nodes,
        report.edges,
        report.dim,
        report.top_k,
        report.query_count,
        report.raw_vector_bytes,
        report.estimated_edge_bytes,
        report.trivium_memory_bytes,
        report.trivium_disk_bytes,
        report.rss_after_open,
        report.rss_after_nodes,
        report.rss_after_edges,
        report.rss_after_flush,
        report.rss_after_reopen,
        report.rss_after_bruteforce,
        report.rss_after_bq,
        report.rss_after_neighbors,
        report.rss_after_tql,
        report.rss_peak_bytes,
        report.node_insert_secs,
        report.edge_insert_secs,
        report.flush_secs,
        report.reopen_secs,
        report.brute_qps,
        report.bq_qps,
        report.bq_recall,
        report.neighbors_qps,
        report.neighbors_avg_count,
        report.tql_expand_qps,
        report.tql_expand_avg_rows
    );
    fs::write(
        "target/bench-report/bench_extreme_graph_vector_report.md",
        md,
    )
    .unwrap();

    let mut jsonl = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("target/bench-report/bench_extreme_graph_vector_report.jsonl")
        .unwrap();
    writeln!(
        jsonl,
        "{{\"nodes\":{},\"edges\":{},\"dim\":{},\"top_k\":{},\"query_count\":{},\"raw_vector_bytes\":{},\"estimated_edge_bytes\":{},\"trivium_memory_bytes\":{},\"trivium_disk_bytes\":{},\"rss_after_open\":{},\"rss_after_nodes\":{},\"rss_after_edges\":{},\"rss_after_flush\":{},\"rss_after_reopen\":{},\"rss_after_bruteforce\":{},\"rss_after_bq\":{},\"rss_after_neighbors\":{},\"rss_after_tql\":{},\"rss_peak_bytes\":{},\"node_insert_secs\":{:.6},\"edge_insert_secs\":{:.6},\"flush_secs\":{:.6},\"reopen_secs\":{:.6},\"brute_qps\":{:.6},\"bq_qps\":{:.6},\"bq_recall\":{:.6},\"neighbors_qps\":{:.6},\"neighbors_avg_count\":{:.6},\"tql_expand_qps\":{:.6},\"tql_expand_avg_rows\":{:.6}}}",
        report.nodes,
        report.edges,
        report.dim,
        report.top_k,
        report.query_count,
        report.raw_vector_bytes,
        report.estimated_edge_bytes,
        report.trivium_memory_bytes,
        report.trivium_disk_bytes,
        report.rss_after_open,
        report.rss_after_nodes,
        report.rss_after_edges,
        report.rss_after_flush,
        report.rss_after_reopen,
        report.rss_after_bruteforce,
        report.rss_after_bq,
        report.rss_after_neighbors,
        report.rss_after_tql,
        report.rss_peak_bytes,
        report.node_insert_secs,
        report.edge_insert_secs,
        report.flush_secs,
        report.reopen_secs,
        report.brute_qps,
        report.bq_qps,
        report.bq_recall,
        report.neighbors_qps,
        report.neighbors_avg_count,
        report.tql_expand_qps,
        report.tql_expand_avg_rows
    )
    .unwrap();
}

fn main() {
    let db_path = "bench_extreme_graph_vector.tdb";
    cleanup_db(db_path);

    let mut rss_peak = rss_bytes();
    let mut db = Database::<f32>::open(db_path, DIM).expect("无法创建极限 benchmark 数据库");
    db.disable_auto_compaction();
    let rss_after_open = update_peak_rss(&mut rss_peak);

    eprintln!(
        "开始极限报告：{} 节点、{} 边、{} 维",
        NODE_COUNT, EDGE_COUNT, DIM
    );

    let t_nodes = Instant::now();
    for node_idx in 0..NODE_COUNT {
        let vector = gen_vector(node_idx);
        db.insert_with_id(
            node_idx as u64 + 1,
            &vector,
            json!({
                "idx": node_idx,
                "bucket": node_idx % 256,
                "kind": if node_idx % 2 == 0 { "doc" } else { "entity" }
            }),
        )
        .unwrap();
        if node_idx % 10_000 == 0 {
            update_peak_rss(&mut rss_peak);
        }
    }
    let node_insert_secs = t_nodes.elapsed().as_secs_f64();
    let rss_after_nodes = update_peak_rss(&mut rss_peak);

    let t_edges = Instant::now();
    for edge_idx in 0..EDGE_COUNT {
        let src = (edge_idx % NODE_COUNT) as u64 + 1;
        let step = 1 + ((edge_idx / NODE_COUNT) % 20) as u64;
        let dst = ((src - 1 + step) % NODE_COUNT as u64) + 1;
        let label = match edge_idx % 4 {
            0 => "rel",
            1 => "near",
            2 => "cite",
            _ => "topic",
        };
        db.link(src, dst, label, 1.0).unwrap();
        if edge_idx % 100_000 == 0 {
            update_peak_rss(&mut rss_peak);
        }
    }
    let edge_insert_secs = t_edges.elapsed().as_secs_f64();
    let rss_after_edges = update_peak_rss(&mut rss_peak);

    let t_flush = Instant::now();
    db.flush().unwrap();
    let flush_secs = t_flush.elapsed().as_secs_f64();
    let trivium_memory_bytes = db.estimated_memory() as u64;
    let trivium_disk_bytes = disk_bytes(db_path);
    let rss_after_flush = update_peak_rss(&mut rss_peak);

    drop(db);
    let t_reopen = Instant::now();
    let db = Database::<f32>::open(db_path, DIM).expect("无法重新打开极限 benchmark 数据库");
    let reopen_secs = t_reopen.elapsed().as_secs_f64();
    let rss_after_reopen = update_peak_rss(&mut rss_peak);

    let queries: Vec<Vec<f32>> = (0..QUERY_COUNT).map(|i| gen_vector(i * 4096 + 7)).collect();

    let t_brute = Instant::now();
    let mut ground_truths = Vec::with_capacity(QUERY_COUNT);
    let brute_cfg = SearchConfig {
        top_k: TOP_K,
        enable_bq_coarse_search: false,
        ..Default::default()
    };
    for query in &queries {
        let truth = brute_force_truth(query, TOP_K);
        let result = db
            .search_hybrid(None, Some(query.as_slice()), &brute_cfg)
            .unwrap();
        let result_ids: Vec<u64> = result.iter().map(|hit| hit.id).collect();
        let api_recall = recall_at_k(&truth, &result_ids);
        eprintln!(
            "BruteForce API 与 streaming 真值 Recall@{} = {:.4}",
            TOP_K, api_recall
        );
        ground_truths.push(truth);
        update_peak_rss(&mut rss_peak);
    }
    let brute_qps = QUERY_COUNT as f64 / t_brute.elapsed().as_secs_f64();
    let rss_after_bruteforce = update_peak_rss(&mut rss_peak);

    let t_bq = Instant::now();
    let bq_cfg = SearchConfig {
        top_k: TOP_K,
        enable_bq_coarse_search: true,
        bq_candidate_ratio: 0.05,
        ..Default::default()
    };
    let mut bq_recall_sum = 0.0;
    for (idx, query) in queries.iter().enumerate() {
        let result = db
            .search_hybrid(None, Some(query.as_slice()), &bq_cfg)
            .unwrap();
        let result_ids: Vec<u64> = result.iter().map(|hit| hit.id).collect();
        bq_recall_sum += recall_at_k(&ground_truths[idx], &result_ids);
        update_peak_rss(&mut rss_peak);
    }
    let bq_qps = QUERY_COUNT as f64 / t_bq.elapsed().as_secs_f64();
    let bq_recall = bq_recall_sum / QUERY_COUNT as f64;
    let rss_after_bq = update_peak_rss(&mut rss_peak);

    let neighbor_samples = [1_u64, 1024, 65_536, 262_144, 499_999];
    let t_neighbors = Instant::now();
    let mut neighbor_total = 0usize;
    for &id in &neighbor_samples {
        let ids = db.neighbors(id, 2);
        neighbor_total += ids.len();
        update_peak_rss(&mut rss_peak);
    }
    let neighbors_qps = neighbor_samples.len() as f64 / t_neighbors.elapsed().as_secs_f64();
    let neighbors_avg_count = neighbor_total as f64 / neighbor_samples.len() as f64;
    let rss_after_neighbors = update_peak_rss(&mut rss_peak);

    let t_tql = Instant::now();
    let mut tql_rows = 0usize;
    for query in &queries[..2] {
        let vector_literal = query
            .iter()
            .map(|v| format!("{:.8}", v))
            .collect::<Vec<_>>()
            .join(",");
        let tql = format!(
            "SEARCH VECTOR [{}] TOP {} EXPAND [:rel*1..2] RETURN * LIMIT 100",
            vector_literal, TOP_K
        );
        tql_rows += db.tql(&tql).unwrap().len();
        update_peak_rss(&mut rss_peak);
    }
    let tql_expand_qps = 2.0 / t_tql.elapsed().as_secs_f64();
    let tql_expand_avg_rows = tql_rows as f64 / 2.0;
    let rss_after_tql = update_peak_rss(&mut rss_peak);

    write_extreme_report(&ExtremeReport {
        nodes: NODE_COUNT,
        edges: EDGE_COUNT,
        dim: DIM,
        top_k: TOP_K,
        query_count: QUERY_COUNT,
        raw_vector_bytes: (NODE_COUNT * DIM * std::mem::size_of::<f32>()) as u64,
        estimated_edge_bytes: (EDGE_COUNT
            * (std::mem::size_of::<u64>() * 2 + std::mem::size_of::<f32>()))
            as u64,
        trivium_memory_bytes,
        trivium_disk_bytes,
        rss_after_open,
        rss_after_nodes,
        rss_after_edges,
        rss_after_flush,
        rss_after_reopen,
        rss_after_bruteforce,
        rss_after_bq,
        rss_after_neighbors,
        rss_after_tql,
        rss_peak_bytes: rss_peak,
        node_insert_secs,
        edge_insert_secs,
        flush_secs,
        reopen_secs,
        brute_qps,
        bq_qps,
        bq_recall,
        neighbors_qps,
        neighbors_avg_count,
        tql_expand_qps,
        tql_expand_avg_rows,
    });

    drop(db);
    cleanup_db(db_path);
}
