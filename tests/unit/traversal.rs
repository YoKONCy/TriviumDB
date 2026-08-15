//! graph/traversal.rs expand_graph 的单元测试
//!
//! 覆盖: expand_graph 的 SA-PPR/反向抑制/侧向截断/不应期疲劳 等全部参数路径

use serde_json::json;
use triviumdb::graph::traversal::expand_graph;
use triviumdb::node::SearchHit;
use triviumdb::storage::memtable::MemTable;

const DIM: usize = 2;

fn build_graph() -> MemTable<f32> {
    let mut mt = MemTable::new(DIM);
    for i in 1..=5 {
        mt.insert_with_id(i, &[i as f32, 0.0], json!({"id": i}))
            .unwrap();
    }
    // 1->2->3, 1->4->5
    mt.link(1, 2, "knows".into(), 0.8).unwrap();
    mt.link(2, 3, "knows".into(), 0.6).unwrap();
    mt.link(1, 4, "works".into(), 0.5).unwrap();
    mt.link(4, 5, "works".into(), 0.7).unwrap();
    mt
}

fn seed(id: u64, score: f32) -> SearchHit {
    SearchHit {
        id,
        score,
        payload: serde_json::Value::Null,
    }
}

#[test]
fn expand_depth0_返回原始seeds() {
    let mt = build_graph();
    let seeds = vec![seed(1, 1.0)];
    let result = expand_graph(&mt, seeds.clone(), 0, 0.0, false, 0, false, None);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, 1);
}

#[test]
fn expand_depth1_扩展邻居() {
    let mt = build_graph();
    let seeds = vec![seed(1, 1.0)];
    let result = expand_graph(&mt, seeds, 1, 0.0, false, 0, false, None);
    assert!(result.len() >= 3, "应扩展到 1,2,4 至少 3 个节点");
}

#[test]
fn expand_depth2_扩展二跳() {
    let mt = build_graph();
    let seeds = vec![seed(1, 1.0)];
    let result = expand_graph(&mt, seeds, 2, 0.0, false, 0, false, None);
    assert!(result.len() >= 4, "两跳应到达 3 和 5");
}

#[test]
fn expand_SA_PPR重启因子() {
    let mt = build_graph();
    let seeds = vec![seed(1, 1.0)];

    let r_no_restart = expand_graph(&mt, seeds.clone(), 1, 0.0, false, 0, false, None);
    let r_restart = expand_graph(&mt, seeds, 1, 0.5, false, 0, false, None);

    let score_no = r_no_restart.iter().find(|h| h.id == 2).unwrap().score;
    let score_restart = r_restart.iter().find(|h| h.id == 2).unwrap().score;
    let seed_restart = r_restart.iter().find(|h| h.id == 1).unwrap().score;
    assert!(score_restart < score_no, "重启应减少传播到邻居的能量");
    assert!(seed_restart > 1.0, "重启能量必须重新注入个性化种子");
}

#[test]
fn expand_SA_PPR出度不放大单层能量() {
    let mut mt = MemTable::new(DIM);
    for id in 1..=11 {
        mt.insert_with_id(id, &[id as f32, 1.0], json!({"id": id}))
            .unwrap();
    }
    for id in 2..=11 {
        mt.link(1, id, "related".into(), 1.0).unwrap();
    }
    let result = expand_graph(&mt, vec![seed(1, 1.0)], 1, 0.0, false, 0, false, None);
    let neighbor_energy: f32 = result
        .iter()
        .filter(|hit| hit.id != 1)
        .map(|hit| hit.score)
        .sum();
    assert!((neighbor_energy - 1.0).abs() < 1e-5);
}

#[test]
fn expand_SA_PPR按出边权重归一化() {
    let mut mt = MemTable::new(DIM);
    for id in 1..=3 {
        mt.insert_with_id(id, &[id as f32, 1.0], json!({"id": id}))
            .unwrap();
    }
    mt.link(1, 2, "related".into(), 3.0).unwrap();
    mt.link(1, 3, "related".into(), 1.0).unwrap();
    let result = expand_graph(&mt, vec![seed(1, 1.0)], 1, 0.0, false, 0, false, None);
    let score2 = result.iter().find(|hit| hit.id == 2).unwrap().score;
    let score3 = result.iter().find(|hit| hit.id == 3).unwrap().score;
    assert!((score2 - 0.75).abs() < 1e-5);
    assert!((score3 - 0.25).abs() < 1e-5);
}

#[test]
fn expand_反向抑制() {
    let mut mt = MemTable::new(DIM);
    for i in 1..=4 {
        mt.insert_with_id(i, &[i as f32, 0.0], json!({"id": i}))
            .unwrap();
    }
    // 1->3, 2->3（3 有高入度），1->4（4 低入度）
    mt.link(1, 3, "a".into(), 1.0).unwrap();
    mt.link(2, 3, "a".into(), 1.0).unwrap();
    mt.link(1, 4, "a".into(), 1.0).unwrap();

    let seeds = vec![seed(1, 1.0)];
    let result = expand_graph(&mt, seeds, 1, 0.0, true, 0, false, None);

    let score3 = result
        .iter()
        .find(|h| h.id == 3)
        .map(|h| h.score)
        .unwrap_or(0.0);
    let score4 = result
        .iter()
        .find(|h| h.id == 4)
        .map(|h| h.score)
        .unwrap_or(0.0);
    assert!(
        score4 > score3,
        "低入度节点(4)应得分高于高入度节点(3): {} vs {}",
        score4,
        score3
    );
}

#[test]
fn expand_侧向截断() {
    let mt = build_graph();
    let seeds = vec![seed(1, 1.0)];
    // lateral_inhibition_threshold=1: 每轮只保留最强的 1 个节点
    let result = expand_graph(&mt, seeds, 2, 0.0, false, 1, false, None);
    // 由于截断，结果数应少于不截断时
    assert!(result.len() <= 4);
}

#[test]
fn expand_不应期疲劳() {
    let mt = build_graph();
    mt.mark_fatigued(&[2]); // 节点 2 处于疲劳状态

    let seeds = vec![seed(1, 1.0)];
    let r_fatigue = expand_graph(&mt, seeds.clone(), 1, 0.0, false, 0, true, None);
    let r_normal = expand_graph(&mt, seeds, 1, 0.0, false, 0, false, None);

    let score_f = r_fatigue
        .iter()
        .find(|h| h.id == 2)
        .map(|h| h.score)
        .unwrap_or(0.0);
    let score_n = r_normal
        .iter()
        .find(|h| h.id == 2)
        .map(|h| h.score)
        .unwrap_or(0.0);
    assert!(
        score_f < score_n,
        "疲劳节点应受到 85% 能量衰减: {} vs {}",
        score_f,
        score_n
    );
}

#[test]
fn expand_清空疲劳恢复无状态评测() {
    let mt = build_graph();
    mt.mark_fatigued(&[2]);
    assert!(mt.get_fatigue(2) > 0);
    mt.clear_fatigue();
    assert_eq!(mt.get_fatigue(2), 0);
}

#[test]
fn expand_inhibition边_负能量() {
    let mut mt = MemTable::new(DIM);
    for i in 1..=2 {
        mt.insert_with_id(i, &[i as f32, 0.0], json!({"id": i}))
            .unwrap();
    }
    mt.link(1, 2, "inhibition".into(), 1.0).unwrap();

    let seeds = vec![seed(1, 1.0)];
    let result = expand_graph(&mt, seeds, 1, 0.0, false, 0, false, None);
    let score2 = result
        .iter()
        .find(|h| h.id == 2)
        .map(|h| h.score)
        .unwrap_or(0.0);
    assert!(score2 < 0.0, "inhibition 边应产生负能量: {}", score2);
}

#[test]
fn expand_空seeds() {
    let mt = build_graph();
    let result = expand_graph(&mt, vec![], 2, 0.0, false, 0, false, None);
    assert!(result.is_empty());
}
