//! 属性索引与标准 Leiden 的公开能力基准。
//!
//! ## 被测能力
//! - 复合 ART：两个等值条件命中同一复合索引。
//! - Roaring Bitmap：两个低基数值执行 OR 并集。
//! - 标准 Leiden：加权弱桥社区上的 local moving、refinement 与 aggregation。
//!
//! ## 计时边界
//! 数据构建和索引创建全部位于 Criterion 计时区间外；索引项只测 Planner 生成
//! AccessPath 和候选集的延迟，Leiden 项测完整社区发现，不包含图构造。
//!
//! ## 数据分布
//! 索引数据使用 64 个 tenant、8 个 kind、4 个 state 的确定性笛卡尔分布。
//! Leiden 数据由 100 节点社区组成，社区内每个节点连接后续 4 个节点，相邻社区
//! 仅以权重 0.01 的弱桥连接。整个套件不使用随机数，跨运行可复现。
//!
//! 运行入口：`cargo bench --bench bench_indexes_and_leiden`。

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use serde_json::json;
use triviumdb::graph::leiden::{AdjacencySnapshot, LeidenConfig, run_leiden};
use triviumdb::query::planner::plan_filter;
use triviumdb::storage::memtable::MemTable;

fn indexed_table(nodes: u64) -> MemTable<f32> {
    // 二维向量只是满足 MemTable 契约；本组不读取向量，避免把向量评分混入索引延迟。
    let mut table = MemTable::new(2);
    for id in 1..=nodes {
        table
            .insert_with_id(
                id,
                &[id as f32, 1.0],
                json!({
                    "tenant": format!("tenant_{}", id % 64),
                    "kind": format!("kind_{}", id % 8),
                    "state": format!("state_{}", id % 4)
                }),
            )
            .unwrap();
    }
    table.register_composite_property_index(&["tenant".into(), "kind".into()]);
    table.register_bitmap_property_index("state");
    table
}

fn index_gate(criterion: &mut Criterion) {
    // 100K 足以形成稳定 posting，同时保持该能力套件适合开发机日常复现。
    let nodes = 100_000u64;
    let table = indexed_table(nodes);
    let mut group = criterion.benchmark_group("property_indexes");
    group.throughput(Throughput::Elements(nodes));
    let composite = triviumdb::Filter::And(vec![
        triviumdb::Filter::Eq("tenant".into(), json!("tenant_7")),
        triviumdb::Filter::Eq("kind".into(), json!("kind_7")),
    ]);
    let bitmap = triviumdb::Filter::Or(vec![
        triviumdb::Filter::Eq("state".into(), json!("state_1")),
        triviumdb::Filter::Eq("state".into(), json!("state_3")),
    ]);
    group.bench_function("composite_exact", |bench| {
        bench.iter(|| criterion::black_box(plan_filter(&composite, &table)))
    });
    group.bench_function("bitmap_or", |bench| {
        bench.iter(|| criterion::black_box(plan_filter(&bitmap, &table)))
    });
    group.finish();
}

fn leiden_graph(communities: usize, size: usize) -> AdjacencySnapshot {
    let mut edges = std::collections::HashMap::new();
    let mut node_ids = Vec::new();
    for community in 0..communities {
        let start = community * size;
        for offset in 0..size {
            let node = (start + offset + 1) as u64;
            node_ids.push(node);
            for step in 1..=4 {
                let target = (start + (offset + step) % size + 1) as u64;
                edges
                    .entry(node)
                    .or_insert_with(Vec::new)
                    .push((target, 1.0));
            }
        }
        if community + 1 < communities {
            edges
                .entry((start + size) as u64)
                .or_insert_with(Vec::new)
                .push(((start + size + 1) as u64, 0.01));
        }
    }
    AdjacencySnapshot { edges, node_ids }
}

fn leiden_gate(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("standard_leiden_gate");
    for nodes in [10_000usize, 50_000] {
        let graph = leiden_graph(nodes / 100, 100);
        group.throughput(Throughput::Elements(nodes as u64));
        group.bench_with_input(
            BenchmarkId::new("weighted_multilevel", nodes),
            &graph,
            |bench, graph| {
                bench.iter(|| {
                    criterion::black_box(run_leiden(
                        graph,
                        &LeidenConfig {
                            min_community_size: 1,
                            max_iterations: 32,
                            compute_centroids: false,
                        },
                    ))
                })
            },
        );
    }
    group.finish();
}

criterion_group!(benches, index_gate, leiden_gate);
criterion_main!(benches);
