#![allow(non_snake_case)]
//! 图谱算法集成测试
//!
//! 覆盖范围：
//! - 路径算法 (pathfinding): 最短路径、可变长路径、全路径枚举、K-hop 邻域
//! - 索引 (label_index): 边标签倒排索引的增删维护正确性
//! - 图分析 (wcc, centrality, pagerank): 连通分量、度/Betweenness 中心性、PageRank
//! - 字段查找 (find_nodes_by_field): Payload JSON 字段匹配

use std::collections::HashMap;
use triviumdb::graph::centrality::*;
use triviumdb::graph::pagerank::*;
use triviumdb::graph::pathfinding::*;
use triviumdb::graph::wcc::*;
use triviumdb::storage::memtable::MemTable;

const DIM: usize = 2;

// ═══════════════════════════════════════════════════════════════════════
//  通用测试图谱构建器
// ═══════════════════════════════════════════════════════════════════════

/// 链路测试图谱：
///
/// ```text
///     1 --knows--> 2 --knows--> 3
///     |                         ^
///     +------works_at----> 4 ---knows---+
///     1 --knows--> 5
/// ```
fn build_path_graph() -> MemTable<f32> {
    let mut mt = MemTable::new(DIM);
    for i in 1..=5 {
        mt.insert_with_id(
            i,
            &[i as f32, 0.0],
            serde_json::json!({"name": format!("N{}", i)}),
        )
        .unwrap();
    }
    mt.link(1, 2, "knows".into(), 1.0).unwrap();
    mt.link(2, 3, "knows".into(), 1.0).unwrap();
    mt.link(1, 4, "works_at".into(), 1.0).unwrap();
    mt.link(4, 3, "knows".into(), 1.0).unwrap();
    mt.link(1, 5, "knows".into(), 1.0).unwrap();
    mt
}

/// 断开测试图谱：
///
/// ```text
/// 组件 A: 1 → 2 → 3    (链式)
/// 组件 B: 4 ↔ 5         (双向)
/// 组件 C: 6             (孤立)
/// ```
fn build_disconnected_graph() -> MemTable<f32> {
    let mut mt = MemTable::new(DIM);
    for i in 1..=6 {
        mt.insert_with_id(
            i,
            &[i as f32, 0.0],
            serde_json::json!({"name": format!("N{}", i)}),
        )
        .unwrap();
    }
    mt.link(1, 2, "knows".into(), 1.0).unwrap();
    mt.link(2, 3, "knows".into(), 1.0).unwrap();
    mt.link(4, 5, "works_at".into(), 1.0).unwrap();
    mt.link(5, 4, "works_at".into(), 1.0).unwrap();
    mt
}

/// 星形测试图谱：
///
/// ```text
///     2
///     ↑
/// 3 ← 1 → 4
///     ↓
///     5
/// ```
fn build_star_graph() -> MemTable<f32> {
    let mut mt = MemTable::new(DIM);
    for i in 1..=5 {
        mt.insert_with_id(
            i,
            &[i as f32, 0.0],
            serde_json::json!({"name": format!("N{}", i)}),
        )
        .unwrap();
    }
    mt.link(1, 2, "connects".into(), 1.0).unwrap();
    mt.link(1, 3, "connects".into(), 1.0).unwrap();
    mt.link(1, 4, "connects".into(), 1.0).unwrap();
    mt.link(1, 5, "connects".into(), 1.0).unwrap();
    mt
}

/// 链式测试图谱：1 → 2 → 3 → 4 → 5
fn build_chain_graph() -> MemTable<f32> {
    let mut mt = MemTable::new(DIM);
    for i in 1..=5 {
        mt.insert_with_id(i, &[i as f32, 0.0], serde_json::json!({}))
            .unwrap();
    }
    mt.link(1, 2, "e".into(), 1.0).unwrap();
    mt.link(2, 3, "e".into(), 1.0).unwrap();
    mt.link(3, 4, "e".into(), 1.0).unwrap();
    mt.link(4, 5, "e".into(), 1.0).unwrap();
    mt
}

// ═══════════════════════════════════════════════════════════════════════
//  shortest_path 测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn 最短路径_直达() {
    let mt = build_path_graph();
    let path = shortest_path(&mt, 1, 2, 5, None).unwrap();
    assert_eq!(path, vec![1, 2]);
}

#[test]
fn 最短路径_两跳() {
    let mt = build_path_graph();
    let path = shortest_path(&mt, 1, 3, 5, None).unwrap();
    assert_eq!(path.len(), 3);
    assert_eq!(*path.first().unwrap(), 1);
    assert_eq!(*path.last().unwrap(), 3);
}

#[test]
fn 最短路径_不可达() {
    let mt = build_path_graph();
    let result = shortest_path(&mt, 5, 1, 10, None);
    assert!(result.is_none());
}

#[test]
fn 最短路径_同节点() {
    let mt = build_path_graph();
    let path = shortest_path(&mt, 3, 3, 5, None).unwrap();
    assert_eq!(path, vec![3]);
}

#[test]
fn 最短路径_标签过滤() {
    let mt = build_path_graph();
    let path = shortest_path(&mt, 1, 3, 5, Some("knows")).unwrap();
    assert_eq!(path, vec![1, 2, 3]);

    let result = shortest_path(&mt, 1, 3, 5, Some("works_at"));
    assert!(result.is_none());
}

#[test]
fn 最短路径_深度限制() {
    let mt = build_path_graph();
    let result = shortest_path(&mt, 1, 3, 1, None);
    assert!(result.is_none());
}

// ═══════════════════════════════════════════════════════════════════════
//  variable_length_paths 测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn 可变长路径_基础() {
    let mt = build_path_graph();
    let results = variable_length_paths(&mt, 1, 1, 2, Some("knows"), 100);
    assert!(results.len() >= 3);
    for (_, path) in &results {
        assert_eq!(path[0], 1);
    }
}

#[test]
fn 可变长路径_最小深度() {
    let mt = build_path_graph();
    let results = variable_length_paths(&mt, 1, 2, 3, Some("knows"), 100);
    for (_, path) in &results {
        assert!(path.len() >= 3);
    }
}

#[test]
fn 可变长路径_结果数熔断() {
    let mt = build_path_graph();
    let results = variable_length_paths(&mt, 1, 1, 10, None, 2);
    assert!(results.len() <= 2);
}

// ═══════════════════════════════════════════════════════════════════════
//  all_paths 测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn 全路径_双路由() {
    let mt = build_path_graph();
    let paths = all_paths(&mt, 1, 3, 5, None, 100);
    assert_eq!(paths.len(), 2);
    for path in &paths {
        assert_eq!(*path.first().unwrap(), 1);
        assert_eq!(*path.last().unwrap(), 3);
    }
}

#[test]
fn 全路径_标签过滤() {
    let mt = build_path_graph();
    let paths = all_paths(&mt, 1, 3, 5, Some("knows"), 100);
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], vec![1, 2, 3]);
}

#[test]
fn 全路径_无路由() {
    let mt = build_path_graph();
    let paths = all_paths(&mt, 5, 1, 10, None, 100);
    assert!(paths.is_empty());
}

#[test]
fn 全路径_熔断() {
    let mt = build_path_graph();
    let paths = all_paths(&mt, 1, 3, 5, None, 1);
    assert_eq!(paths.len(), 1);
}

// ═══════════════════════════════════════════════════════════════════════
//  k_hop_neighbors 测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn K跳邻域_基础() {
    let mt = build_path_graph();
    let neighbors = k_hop_neighbors(&mt, 1, 1, None);
    assert_eq!(neighbors.len(), 4);
    assert_eq!(neighbors[&1], 0);
    assert_eq!(neighbors[&2], 1);
    assert_eq!(neighbors[&4], 1);
    assert_eq!(neighbors[&5], 1);
}

#[test]
fn K跳邻域_两跳() {
    let mt = build_path_graph();
    let neighbors = k_hop_neighbors(&mt, 1, 2, None);
    assert!(neighbors.contains_key(&3));
    assert_eq!(neighbors[&3], 2);
}

#[test]
fn K跳邻域_标签过滤() {
    let mt = build_path_graph();
    let neighbors = k_hop_neighbors(&mt, 1, 2, Some("knows"));
    assert!(neighbors.contains_key(&2));
    assert!(neighbors.contains_key(&5));
    assert!(neighbors.contains_key(&3));
    assert!(!neighbors.contains_key(&4));
}

// ═══════════════════════════════════════════════════════════════════════
//  label_index 测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn 边标签索引_正确维护() {
    let mt = build_path_graph();
    let knows = mt.get_edges_by_label("knows");
    assert_eq!(knows.len(), 4);
    let works_at = mt.get_edges_by_label("works_at");
    assert_eq!(works_at.len(), 1);
}

#[test]
fn 边标签索引_unlink后更新() {
    let mut mt = build_path_graph();
    mt.unlink(1, 2).unwrap();
    let knows = mt.get_edges_by_label("knows");
    assert_eq!(knows.len(), 3);
    assert!(!knows.contains(&(1, 2)));
}

#[test]
fn 边标签索引_delete后清理() {
    let mut mt = build_path_graph();
    mt.delete(2).unwrap();
    let knows = mt.get_edges_by_label("knows");
    assert_eq!(knows.len(), 2);
    assert!(!knows.contains(&(1, 2)));
    assert!(!knows.contains(&(2, 3)));
}

// ═══════════════════════════════════════════════════════════════════════
//  find_nodes_by_field 测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn 字段查找_多匹配() {
    let mut mt: MemTable<f32> = MemTable::new(DIM);
    mt.insert_with_id(
        1,
        &[1.0, 0.0],
        serde_json::json!({"type": "person", "name": "Alice"}),
    )
    .unwrap();
    mt.insert_with_id(
        2,
        &[2.0, 0.0],
        serde_json::json!({"type": "person", "name": "Bob"}),
    )
    .unwrap();
    mt.insert_with_id(
        3,
        &[3.0, 0.0],
        serde_json::json!({"type": "event", "name": "Meeting"}),
    )
    .unwrap();

    let persons = mt.find_nodes_by_field("type", &serde_json::json!("person"));
    assert_eq!(persons.len(), 2);
    assert!(persons.contains(&1));
    assert!(persons.contains(&2));

    let events = mt.find_nodes_by_field("type", &serde_json::json!("event"));
    assert_eq!(events.len(), 1);

    let empty = mt.find_nodes_by_field("type", &serde_json::json!("nonexistent"));
    assert!(empty.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════
//  WCC 测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn WCC_断开图谱() {
    let mt = build_disconnected_graph();
    let components = weakly_connected_components(&mt);
    assert_eq!(components.len(), 3, "应有 3 个连通分量");
    assert_eq!(components[0].len(), 3);
    assert_eq!(components[1].len(), 2);
    assert_eq!(components[2].len(), 1);
}

#[test]
fn WCC_全连通() {
    let mt = build_star_graph();
    let components = weakly_connected_components(&mt);
    assert_eq!(components.len(), 1);
    assert_eq!(components[0].len(), 5);
}

#[test]
fn WCC_空图() {
    let mt: MemTable<f32> = MemTable::new(DIM);
    let components = weakly_connected_components(&mt);
    assert!(components.is_empty());
}

#[test]
fn WCC_单节点() {
    let mut mt: MemTable<f32> = MemTable::new(DIM);
    mt.insert_with_id(1, &[1.0, 0.0], serde_json::json!({}))
        .unwrap();
    let components = weakly_connected_components(&mt);
    assert_eq!(components.len(), 1);
    assert_eq!(components[0], vec![1]);
}

// ═══════════════════════════════════════════════════════════════════════
//  度中心性测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn 度中心性_星形图_中心最高() {
    let mt = build_star_graph();
    let result = degree_centrality(&mt);
    assert_eq!(result[0].id, 1);
    assert_eq!(result[0].out_degree, 4);
    assert_eq!(result[0].in_degree, 0);
    assert_eq!(result[0].total_degree, 4);
    for dc in &result[1..] {
        assert_eq!(dc.total_degree, 1);
    }
}

#[test]
fn 度中心性_归一化() {
    let mt = build_star_graph();
    let result = degree_centrality(&mt);
    assert!((result[0].normalized - 1.0).abs() < 1e-10);
}

#[test]
fn 度中心性_空图() {
    let mt: MemTable<f32> = MemTable::new(DIM);
    let result = degree_centrality(&mt);
    assert!(result.is_empty());
}

#[test]
fn 度中心性_双向边() {
    let mt = build_disconnected_graph();
    let result = degree_centrality(&mt);
    let node4 = result.iter().find(|dc| dc.id == 4).unwrap();
    assert_eq!(node4.out_degree, 1);
    assert_eq!(node4.in_degree, 1);
    assert_eq!(node4.total_degree, 2);
}

// ═══════════════════════════════════════════════════════════════════════
//  Betweenness 中心性测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn Betweenness_链式图_中间节点最高() {
    let mt = build_chain_graph();
    let bc = betweenness_centrality(&mt, None, None);
    assert!(bc[&3] > bc[&1], "中间节点 3 的 betweenness 应高于端点 1");
    assert!(bc[&3] > bc[&5], "中间节点 3 的 betweenness 应高于端点 5");
    assert!(bc[&3] >= bc[&2]);
}

#[test]
fn Betweenness_星形图_叶节点无中介() {
    let mt = build_star_graph();
    let bc = betweenness_centrality(&mt, None, None);
    for &id in &[2u64, 3, 4, 5] {
        assert!((bc[&id] - 0.0).abs() < 1e-10, "叶节点 {} 不应有中介性", id);
    }
}

#[test]
fn Betweenness_标签过滤() {
    let mt = build_disconnected_graph();
    let bc = betweenness_centrality(&mt, Some("knows"), None);
    assert!(bc[&2] > 0.0, "节点 2 在 knows 链中是中介");
    assert!((bc[&4] - 0.0).abs() < 1e-10);
    assert!((bc[&5] - 0.0).abs() < 1e-10);
}

#[test]
fn Betweenness_采样模式() {
    let mt = build_chain_graph();
    let bc = betweenness_centrality(&mt, None, Some(2));
    assert_eq!(bc.len(), 5);
}

// ═══════════════════════════════════════════════════════════════════════
//  PageRank 测试
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn PageRank_星形图_叶节点高于中心() {
    let mt = build_star_graph();
    let config = PageRankConfig::default();
    let result = pagerank(&mt, &config, None);
    let pr_map: HashMap<u64, f64> = result.into_iter().collect();
    assert!(
        pr_map[&1] < pr_map[&2],
        "中心节点无入边，PageRank 应低于被指向的叶节点"
    );
}

#[test]
fn PageRank_总和收敛到1() {
    let mt = build_disconnected_graph();
    let config = PageRankConfig::default();
    let result = pagerank(&mt, &config, None);
    let total: f64 = result.iter().map(|(_, pr)| pr).sum();
    assert!(
        (total - 1.0).abs() < 0.01,
        "PageRank 总和应接近 1.0，实际为 {}",
        total
    );
}

#[test]
fn PageRank_空图() {
    let mt: MemTable<f32> = MemTable::new(DIM);
    let config = PageRankConfig::default();
    let result = pagerank(&mt, &config, None);
    assert!(result.is_empty());
}

#[test]
fn PageRank_标签过滤() {
    let mt = build_disconnected_graph();
    let config = PageRankConfig::default();
    let result = pagerank(&mt, &config, Some("knows"));
    let pr_map: HashMap<u64, f64> = result.into_iter().collect();
    assert!(
        pr_map[&3] > pr_map[&1],
        "节点 3 接收了 knows 链的全部权重传播"
    );
}

#[test]
fn PageRank_降序排列() {
    let mt = build_star_graph();
    let config = PageRankConfig::default();
    let result = pagerank(&mt, &config, None);
    for window in result.windows(2) {
        assert!(window[0].1 >= window[1].1, "PageRank 结果应按分数降序排列");
    }
}
