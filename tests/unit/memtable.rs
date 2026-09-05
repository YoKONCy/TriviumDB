//! MemTable 核心存储引擎的单元测试
//!
//! 覆盖: new, insert, insert_with_id, raw_insert, delete, link, unlink,
//!       update_payload, update_vector, get_vector, get_payload, get_edges,
//!       contains, node_count, all_node_ids, find_nodes_by_field,
//!       property_index, fatigue, text_index, estimated_memory_bytes,
//!       register_node, register_tombstone, advance_next_id, 等全部公开方法

use serde_json::json;
use triviumdb::storage::memtable::{MemTable, auto_quiver_node_threshold};

const DIM: usize = 3;

fn make_mt() -> MemTable<f32> {
    MemTable::new(DIM)
}

// ═══════════════════════════════════════════════════════════════
//  构造与基本属性
// ═══════════════════════════════════════════════════════════════

#[test]
fn quiver自动阈值随维度下降并保持上下界() {
    assert_eq!(auto_quiver_node_threshold(0), 10_000);
    assert_eq!(auto_quiver_node_threshold(128), 10_000);
    assert_eq!(auto_quiver_node_threshold(768), 10_000);
    assert_eq!(auto_quiver_node_threshold(1024), 7_813);
    assert_eq!(auto_quiver_node_threshold(1536), 5_209);
    assert_eq!(auto_quiver_node_threshold(2048), 3_907);
    assert_eq!(auto_quiver_node_threshold(3072), 2_605);
    assert_eq!(auto_quiver_node_threshold(usize::MAX), 2_500);
}

#[test]
fn quiver自动构建严格遵守动态阈值与禁用配置() {
    let threshold = auto_quiver_node_threshold(3072);
    let mut below = MemTable::<f32>::new(3072);
    let vector = vec![0.0; 3072];
    for id in 1..threshold as u64 {
        below
            .insert_with_id(id, &vector, serde_json::Value::Null)
            .unwrap();
    }
    assert!(!below.auto_quiver_build_needed());
    below
        .insert_with_id(threshold as u64, &vector, serde_json::Value::Null)
        .unwrap();
    assert!(below.auto_quiver_build_needed());
    below.set_auto_build_quiver(false);
    assert!(!below.auto_quiver_build_needed());
}

#[test]
fn new_空表() {
    let mt = make_mt();
    assert_eq!(mt.dim(), DIM);
    assert_eq!(mt.node_count(), 0);
    assert!(mt.all_node_ids().is_empty());
    assert_eq!(mt.next_id_value(), 1);
}

#[test]
fn new_with_next_id() {
    let mt = MemTable::<f32>::new_with_next_id(4, 100);
    assert_eq!(mt.dim(), 4);
    assert_eq!(mt.next_id_value(), 100);
}

// ═══════════════════════════════════════════════════════════════
//  insert / insert_with_id / raw_insert
// ═══════════════════════════════════════════════════════════════

#[test]
fn insert_自增ID() {
    let mut mt = make_mt();
    let id1 = mt.insert(&[1.0, 2.0, 3.0], json!({"a": 1})).unwrap();
    let id2 = mt.insert(&[4.0, 5.0, 6.0], json!({"a": 2})).unwrap();
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    assert_eq!(mt.node_count(), 2);
}

#[test]
fn insert_维度不匹配() {
    let mut mt = make_mt();
    let before_next = mt.next_id_value();
    let err = mt.insert(&[1.0, 2.0], json!({}));
    assert!(err.is_err(), "维度不匹配插入应被拒绝");
    assert_eq!(mt.node_count(), 0, "失败插入不能产生节点");
    assert_eq!(mt.next_id_value(), before_next, "失败插入不能消耗自增 ID");
}

#[test]
fn insert_NaN拒绝() {
    let mut mt = make_mt();
    let before_next = mt.next_id_value();
    let err = mt.insert(&[f32::NAN, 0.0, 0.0], json!({}));
    assert!(err.is_err(), "NaN 向量插入应被拒绝");
    assert_eq!(mt.node_count(), 0, "NaN 插入不能产生节点");
    assert_eq!(mt.next_id_value(), before_next, "NaN 插入不能消耗自增 ID");
}

#[test]
fn insert_Infinity拒绝() {
    let mut mt = make_mt();
    let before_next = mt.next_id_value();
    let err = mt.insert(&[f32::INFINITY, 0.0, 0.0], json!({}));
    assert!(err.is_err(), "Infinity 向量插入应被拒绝");
    assert_eq!(mt.node_count(), 0, "Infinity 插入不能产生节点");
    assert_eq!(
        mt.next_id_value(),
        before_next,
        "Infinity 插入不能消耗自增 ID"
    );
}

#[test]
fn insert_with_id_基础() {
    let mut mt = make_mt();
    mt.insert_with_id(42, &[1.0, 2.0, 3.0], json!({"x": 1}))
        .unwrap();
    assert!(mt.contains(42));
    assert_eq!(mt.node_count(), 1);
    assert!(mt.next_id_value() > 42);
}

#[test]
fn insert_with_id_ID0拒绝() {
    let mut mt = make_mt();
    assert!(mt.insert_with_id(0, &[1.0, 2.0, 3.0], json!({})).is_err());
    assert!(!mt.contains(0));
    assert_eq!(mt.next_id_value(), 1);
}

#[test]
fn insert_with_id_重复ID报错() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 2.0, 3.0], json!({})).unwrap();
    let before_count = mt.node_count();
    let before_next = mt.next_id_value();
    let err = mt.insert_with_id(1, &[4.0, 5.0, 6.0], json!({}));
    assert!(err.is_err(), "重复 ID 应被拒绝");
    assert_eq!(mt.node_count(), before_count, "重复 ID 失败不能改变节点数");
    assert_eq!(
        mt.next_id_value(),
        before_next,
        "重复 ID 失败不能推进 next_id"
    );
    assert_eq!(
        mt.get_vector(1).unwrap(),
        &[1.0, 2.0, 3.0],
        "重复 ID 不能覆盖原向量"
    );
}

#[test]
fn raw_insert_跳过NaN检查() {
    let mut mt = make_mt();
    // raw_insert 用于 WAL 恢复，不检查 NaN
    let result = mt.raw_insert(1, &[f32::NAN, 0.0, 0.0], json!({"raw": true}));
    assert!(result.is_ok(), "raw_insert 应允许 WAL 恢复路径写入 NaN");
    assert_eq!(mt.node_count(), 1, "raw_insert 成功后必须产生一个节点");
    assert_eq!(mt.get_payload(1).unwrap().get("raw"), Some(&json!(true)));
}

// ═══════════════════════════════════════════════════════════════
//  get_vector / get_payload / get_edges / contains
// ═══════════════════════════════════════════════════════════════

#[test]
fn get_vector_往返() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 2.0, 3.0], json!({})).unwrap();
    mt.ensure_vectors_cache(true);
    let v = mt.get_vector(1).unwrap();
    assert_eq!(v, &[1.0, 2.0, 3.0]);
}

#[test]
fn get_vector_不存在() {
    let mt = make_mt();
    assert!(mt.get_vector(999).is_none());
}

#[test]
fn get_payload_往返() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 2.0, 3.0], json!({"name": "test"}))
        .unwrap();
    let p = mt.get_payload(1).unwrap();
    assert_eq!(p["name"], "test");
}

#[test]
fn contains_存在与不存在() {
    let mut mt = make_mt();
    mt.insert_with_id(5, &[1.0, 2.0, 3.0], json!({})).unwrap();
    assert!(mt.contains(5));
    assert!(!mt.contains(6));
}

// ═══════════════════════════════════════════════════════════════
//  delete
// ═══════════════════════════════════════════════════════════════

#[test]
fn delete_基础() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 2.0, 3.0], json!({})).unwrap();
    mt.delete(1).unwrap();
    assert!(!mt.contains(1));
    assert_eq!(mt.node_count(), 0);
}

#[test]
fn delete_不存在报错() {
    let mut mt = make_mt();
    assert!(mt.delete(999).is_err());
}

#[test]
fn delete_后槽位回收() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.delete(1).unwrap();
    // 再插入应复用空闲槽
    let id2 = mt.insert(&[2.0, 0.0, 0.0], json!({})).unwrap();
    assert!(mt.contains(id2));
    assert_eq!(mt.node_count(), 1);
}

#[test]
fn delete_清理出边和入边() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0], json!({})).unwrap();
    mt.link(1, 2, "knows".into(), 1.0).unwrap();
    assert_eq!(mt.get_in_degree(2), 1);

    mt.delete(1).unwrap();
    // 入边应被清理
    assert_eq!(mt.get_in_degree(2), 0);
    assert!(mt.get_edges(1).is_none());
}

// ═══════════════════════════════════════════════════════════════
//  link / unlink / 图谱操作
// ═══════════════════════════════════════════════════════════════

#[test]
fn link_基础() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0], json!({})).unwrap();
    mt.link(1, 2, "knows".into(), 0.8).unwrap();

    let edges = mt.get_edges(1).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target_id, 2);
    assert_eq!(edges[0].label, "knows");
    assert_eq!(edges[0].weight, 0.8);
}

#[test]
fn link_同三元组更新权重且索引不重复() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0], json!({})).unwrap();
    mt.link(1, 2, "knows".into(), 0.5).unwrap();
    mt.link(1, 2, "knows".into(), 0.9).unwrap();

    let edges = mt.get_edges(1).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].weight, 0.9);
    assert_eq!(mt.get_in_degree(2), 1);
    assert_eq!(mt.get_incoming_sources(2), &[1]);
    assert_eq!(mt.get_edges_by_label("knows"), vec![(1, 2)]);
}

#[test]
fn 节点ID耗尽时拒绝且不修改状态() {
    let mut mt = MemTable::<f32>::new_with_next_id(1, u64::MAX);
    let result = mt.insert(&[1.0], json!({"value": 1}));
    assert!(result.is_err());
    assert_eq!(mt.node_count(), 0);
    assert_eq!(mt.next_id_value(), u64::MAX);

    let mut mt = MemTable::<f32>::new(1);
    let result = mt.insert_with_id(u64::MAX, &[1.0], json!({"value": 1}));
    assert!(result.is_err());
    assert_eq!(mt.node_count(), 0);
    assert_eq!(mt.next_id_value(), 1);
}

#[test]
fn unlink_label_保留其他标签及反向索引() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0], json!({})).unwrap();
    mt.link(1, 2, "knows".into(), 1.0).unwrap();
    mt.link(1, 2, "works".into(), 2.0).unwrap();

    mt.unlink_label(1, 2, "knows").unwrap();
    assert_eq!(mt.get_edges(1).unwrap().len(), 1);
    assert_eq!(mt.get_in_degree(2), 1);
    assert_eq!(mt.get_incoming_sources(2), &[1]);
    assert!(mt.get_edges_by_label("knows").is_empty());
    assert_eq!(mt.get_incoming_edges(2, Some("works")).len(), 1);

    mt.unlink_label(1, 2, "works").unwrap();
    assert_eq!(mt.get_in_degree(2), 0);
    assert!(mt.get_incoming_sources(2).is_empty());
}

#[test]
fn link_拒绝非有限权重() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0], json!({})).unwrap();
    assert!(mt.link(1, 2, "bad".into(), f32::NAN).is_err());
    assert!(mt.link(1, 2, "bad".into(), f32::INFINITY).is_err());
    assert!(mt.get_edges(1).is_none_or(|edges| edges.is_empty()));
}

#[test]
fn link_不存在的节点报错() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    assert!(mt.link(1, 999, "x".into(), 1.0).is_err());
    assert!(mt.link(999, 1, "x".into(), 1.0).is_err());
}

#[test]
fn unlink_基础() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0], json!({})).unwrap();
    mt.link(1, 2, "knows".into(), 1.0).unwrap();
    mt.unlink(1, 2).unwrap();

    let edges = mt.get_edges(1).unwrap();
    assert!(edges.is_empty());
    assert_eq!(mt.get_in_degree(2), 0);
}

#[test]
fn get_in_degree_和_incoming_sources() {
    let mut mt = make_mt();
    for i in 1..=3 {
        mt.insert_with_id(i, &[i as f32, 0.0, 0.0], json!({}))
            .unwrap();
    }
    mt.link(1, 3, "a".into(), 1.0).unwrap();
    mt.link(2, 3, "b".into(), 1.0).unwrap();

    assert_eq!(mt.get_in_degree(3), 2);
    let sources = mt.get_incoming_sources(3);
    assert_eq!(sources.len(), 2);
}

#[test]
fn get_edges_by_label() {
    let mut mt = make_mt();
    for i in 1..=3 {
        mt.insert_with_id(i, &[i as f32, 0.0, 0.0], json!({}))
            .unwrap();
    }
    mt.link(1, 2, "knows".into(), 1.0).unwrap();
    mt.link(1, 3, "works_at".into(), 1.0).unwrap();

    let knows = mt.get_edges_by_label("knows");
    assert_eq!(knows.len(), 1);
    assert_eq!(knows[0], (1, 2));

    let works = mt.get_edges_by_label("works_at");
    assert_eq!(works.len(), 1);
}

// ═══════════════════════════════════════════════════════════════
//  update_payload / update_vector
// ═══════════════════════════════════════════════════════════════

#[test]
fn update_payload_基础() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"v": 1}))
        .unwrap();
    mt.update_payload(1, json!({"v": 2})).unwrap();
    assert_eq!(mt.get_payload(1).unwrap()["v"], 2);
}

#[test]
fn update_payload_不存在报错() {
    let mut mt = make_mt();
    assert!(mt.update_payload(999, json!({})).is_err());
}

// ═══════════════════════════════════════════════════════════════
//  patch_payload (部分更新)
// ═══════════════════════════════════════════════════════════════

#[test]
fn patch_payload_set_基础() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"name": "Alice", "age": 25}))
        .unwrap();
    mt.patch_payload(1, &json!({"$set": {"age": 26, "city": "Tokyo"}}))
        .unwrap();
    let p = mt.get_payload(1).unwrap();
    assert_eq!(p["name"], "Alice", "$set 不应影响未修改的字段");
    assert_eq!(p["age"], 26, "$set 应更新已有字段");
    assert_eq!(p["city"], "Tokyo", "$set 应创建新字段");
}

#[test]
fn patch_payload_inc_整数递增() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"visits": 10, "score": 3.5}))
        .unwrap();
    mt.patch_payload(1, &json!({"$inc": {"visits": 1, "score": -0.5}}))
        .unwrap();
    let p = mt.get_payload(1).unwrap();
    assert_eq!(p["visits"], 11);
    assert_eq!(p["score"], 3.0);
}

#[test]
fn patch_payload_inc_字段不存在视为零() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"name": "Alice"}))
        .unwrap();
    mt.patch_payload(1, &json!({"$inc": {"counter": 5}}))
        .unwrap();
    let p = mt.get_payload(1).unwrap();
    assert_eq!(p["counter"], 5, "不存在的字段 $inc 应视为从 0 开始");
}

#[test]
fn patch_payload_unset_删除字段() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"a": 1, "b": 2, "c": 3}))
        .unwrap();
    mt.patch_payload(1, &json!({"$unset": {"b": true}}))
        .unwrap();
    let p = mt.get_payload(1).unwrap();
    assert_eq!(p["a"], 1, "$unset 不应影响其他字段");
    assert!(p.get("b").is_none(), "$unset 应删除目标字段");
    assert_eq!(p["c"], 3);
}

#[test]
fn patch_payload_简写模式明确拒绝() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"x": 1}))
        .unwrap();
    let error = mt.patch_payload(1, &json!({"y": 2})).unwrap_err();
    assert!(matches!(
        error,
        triviumdb::TriviumError::ApiMigrationRequired { .. }
    ));
}

#[test]
fn patch_payload_组合操作() {
    let mut mt = make_mt();
    mt.insert_with_id(
        1,
        &[1.0, 0.0, 0.0],
        json!({"name": "Alice", "age": 25, "old": true}),
    )
    .unwrap();
    mt.patch_payload(
        1,
        &json!({
            "$set": {"name": "Bob"},
            "$inc": {"age": 1},
            "$unset": {"old": true}
        }),
    )
    .unwrap();
    let p = mt.get_payload(1).unwrap();
    assert_eq!(p["name"], "Bob");
    assert_eq!(p["age"], 26);
    assert!(p.get("old").is_none());
}

#[test]
fn patch_payload_不存在报错() {
    let mut mt = make_mt();
    assert!(mt.patch_payload(999, &json!({"$set": {"x": 1}})).is_err());
}

#[test]
fn patch_payload_属性索引自动维护() {
    let mut mt = make_mt();
    mt.register_property_index("status");
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"status": "active"}))
        .unwrap();

    // patch 修改 status
    mt.patch_payload(1, &json!({"$set": {"status": "inactive"}}))
        .unwrap();

    let active = mt
        .find_by_property_index("status", &json!("active"))
        .unwrap();
    assert!(active.is_empty(), "旧值索引应被清理");
    let inactive = mt
        .find_by_property_index("status", &json!("inactive"))
        .unwrap();
    assert_eq!(inactive.len(), 1, "新值应出现在索引中");
}

#[test]
fn update_vector_基础() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.update_vector(1, &[0.0, 1.0, 0.0]).unwrap();
    mt.ensure_vectors_cache(true);
    assert_eq!(mt.get_vector(1).unwrap(), &[0.0, 1.0, 0.0]);
}

#[test]
fn update_vector_维度不匹配() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    assert!(mt.update_vector(1, &[1.0, 2.0]).is_err());
}

#[test]
fn update_vector_NaN拒绝() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    assert!(mt.update_vector(1, &[f32::NAN, 0.0, 0.0]).is_err());
}

// ═══════════════════════════════════════════════════════════════
//  find_nodes_by_field
// ═══════════════════════════════════════════════════════════════

#[test]
fn find_nodes_by_field_基础() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"type": "person"}))
        .unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0], json!({"type": "event"}))
        .unwrap();
    mt.insert_with_id(3, &[0.0, 0.0, 1.0], json!({"type": "person"}))
        .unwrap();

    let persons = mt.find_nodes_by_field("type", &json!("person"));
    assert_eq!(persons.len(), 2);
}

// ═══════════════════════════════════════════════════════════════
//  属性二级索引
// ═══════════════════════════════════════════════════════════════

#[test]
fn property_index_注册和查询() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"role": "admin"}))
        .unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0], json!({"role": "user"}))
        .unwrap();

    assert!(!mt.has_property_index("role"));
    mt.register_property_index("role");
    assert!(mt.has_property_index("role"));

    let admins = mt.find_by_property_index("role", &json!("admin")).unwrap();
    assert_eq!(admins.len(), 1);
    assert_eq!(admins[0], 1);
}

#[test]
fn property_index_插入后自动维护() {
    let mut mt = make_mt();
    mt.register_property_index("color");
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"color": "red"}))
        .unwrap();

    let reds = mt.find_by_property_index("color", &json!("red")).unwrap();
    assert_eq!(reds.len(), 1);
}

#[test]
fn property_index_删除后清理() {
    let mut mt = make_mt();
    mt.register_property_index("color");
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"color": "red"}))
        .unwrap();
    mt.delete(1).unwrap();

    let reds = mt.find_by_property_index("color", &json!("red")).unwrap();
    assert!(reds.is_empty());
}

#[test]
fn property_index_update后更新() {
    let mut mt = make_mt();
    mt.register_property_index("status");
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"status": "active"}))
        .unwrap();
    mt.update_payload(1, json!({"status": "inactive"})).unwrap();

    let active = mt
        .find_by_property_index("status", &json!("active"))
        .unwrap();
    assert!(active.is_empty());
    let inactive = mt
        .find_by_property_index("status", &json!("inactive"))
        .unwrap();
    assert_eq!(inactive.len(), 1);
}

#[test]
fn property_index_drop() {
    let mut mt = make_mt();
    mt.register_property_index("x");
    assert!(mt.has_property_index("x"));
    mt.drop_property_index("x");
    assert!(!mt.has_property_index("x"));
}

#[test]
fn property_index_无索引返回None() {
    let mt = make_mt();
    assert!(mt.find_by_property_index("no_index", &json!("v")).is_none());
}

// ═══════════════════════════════════════════════════════════════
//  疲劳系统
// ═══════════════════════════════════════════════════════════════

#[test]
fn fatigue_标记和查询() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();

    assert_eq!(mt.get_fatigue(1), 0);
    mt.mark_fatigued(&[1]);
    assert_eq!(mt.get_fatigue(1), 1);
}

#[test]
fn fatigue_消耗() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.mark_fatigued(&[1]);
    mt.consume_fatigue(1);
    assert_eq!(mt.get_fatigue(1), 0);
}

#[test]
fn fatigue_批量消耗() {
    let mut mt = make_mt();
    for i in 1..=3 {
        mt.insert_with_id(i, &[i as f32, 0.0, 0.0], json!({}))
            .unwrap();
    }
    mt.mark_fatigued(&[1, 2, 3]);
    mt.consume_fatigue_batch(&[1, 3]);
    assert_eq!(mt.get_fatigue(1), 0);
    assert_eq!(mt.get_fatigue(2), 1);
    assert_eq!(mt.get_fatigue(3), 0);
}

// ═══════════════════════════════════════════════════════════════
//  文本索引接口
// ═══════════════════════════════════════════════════════════════

#[test]
fn text_index_keyword_和_bm25() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0], json!({})).unwrap();

    mt.index_keyword(1, "rust");
    mt.index_text(2, "hello world rust programming");
    mt.build_text_index();

    let ac = mt.text_engine().search_ac("rust");
    assert!(ac.contains_key(&1));

    let bm25 = mt.text_engine().search_bm25("rust", 1.5, 0.75);
    assert!(bm25.contains_key(&2));
}

#[test]
fn rebuild_text_index_from_payloads() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"desc": "hello world"}))
        .unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0], json!({"desc": "rust database"}))
        .unwrap();
    mt.rebuild_text_index_from_payloads();

    let results = mt.text_engine().search_bm25("rust", 1.5, 0.75);
    assert!(results.contains_key(&2));
}

// ═══════════════════════════════════════════════════════════════
//  其他辅助方法
// ═══════════════════════════════════════════════════════════════

#[test]
fn advance_next_id() {
    let mut mt = make_mt();
    assert_eq!(mt.next_id_value(), 1);
    mt.advance_next_id(100);
    assert_eq!(mt.next_id_value(), 100);
    mt.advance_next_id(50); // 不应倒退
    assert_eq!(mt.next_id_value(), 100);
}

#[test]
fn register_node_和_register_tombstone() {
    let mut mt = make_mt();
    mt.register_node(1, json!({"a": 1})).unwrap();
    assert!(mt.contains(1));
    assert_eq!(mt.node_count(), 1);

    mt.register_tombstone().unwrap();
    assert_eq!(mt.node_count(), 1); // tombstone 不算活跃节点
    assert_eq!(mt.internal_slot_count(), 2);
}

#[test]
fn estimated_memory_bytes_非零() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 2.0, 3.0], json!({"big": "payload"}))
        .unwrap();
    assert!(mt.estimated_memory_bytes() > 0);
}

#[test]
fn active_entries_跳过tombstone() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0], json!({})).unwrap();
    mt.delete(1).unwrap();

    let active: Vec<_> = mt.active_entries().collect();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].1, 2);
}

#[test]
fn flat_vectors_和_ensure_cache() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 2.0, 3.0], json!({})).unwrap();
    mt.insert_with_id(2, &[4.0, 5.0, 6.0], json!({})).unwrap();
    mt.ensure_vectors_cache(true);

    let flat = mt.flat_vectors();
    assert_eq!(flat.len(), 6);
}

#[test]
fn bq_signatures_重建() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.ensure_vectors_cache(true);
    assert!(!mt.bq_signatures_slice().is_empty());
    assert!(mt.get_bq_signature(0).is_some());
}

#[test]
fn quiver_手动构建和失效() {
    use triviumdb::index::quiver::QuIVerConfig;
    let mut mt = make_mt();
    for i in 1..=5u64 {
        mt.insert_with_id(i, &[i as f32, 0.0, 0.0], json!({}))
            .unwrap();
    }
    assert!(mt.quiver().is_none(), "数据量不足时 QuIVer 不应自动构建");
    mt.build_quiver(&QuIVerConfig::default());
    assert!(mt.quiver().is_some(), "手动构建后 QuIVer 应存在");

    // delete 使用 soft_delete（tombstone 标记），索引仍存在
    mt.delete(1).unwrap();
    assert!(
        mt.quiver().is_some(),
        "单次 delete 后 QuIVer 应仍存在（soft_delete）"
    );

    // 删除超过 25% 后索引失效（5 个节点中删除 2 个 = 40%）
    mt.delete(2).unwrap();
    assert!(mt.quiver().is_none(), "退化超过 25% 后 QuIVer 应自动失效");
}

#[test]
fn quiver_事务同步超过退化阈值后立即重建() {
    use triviumdb::index::quiver::QuIVerConfig;
    use triviumdb::storage::wal::WalEntry;

    let mut mt = make_mt();
    for i in 1..=5u64 {
        mt.insert_with_id(i, &[i as f32, 0.0, 0.0], json!({}))
            .unwrap();
    }
    mt.build_quiver(&QuIVerConfig::default());
    mt.set_quiver_sync_paused(true);
    mt.delete(1).unwrap();
    mt.delete(2).unwrap();
    mt.set_quiver_sync_paused(false);

    mt.quiver_sync_tx_entries(&[WalEntry::Delete { id: 1 }, WalEntry::Delete { id: 2 }]);

    let quiver = mt.quiver().expect("事务提交后必须恢复可用的 QuIVer 索引");
    assert_eq!(quiver.total_count(), 3);
    assert_eq!(quiver.active_count(), 3);
}

#[test]
fn fast_tags_非空() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"role": "admin"}))
        .unwrap();
    let tags = mt.fast_tags_slice();
    assert_eq!(tags.len(), 1);
    assert_ne!(tags[0], 0, "有 payload 的节点应有非零 bloom 签名");
}

#[test]
fn internal_indices() {
    let mut mt = make_mt();
    mt.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0], json!({})).unwrap();
    let ids = mt.internal_indices();
    assert_eq!(ids.len(), 2);
}

#[test]
fn get_id_by_index() {
    let mut mt = make_mt();
    mt.insert_with_id(42, &[1.0, 0.0, 0.0], json!({})).unwrap();
    assert_eq!(mt.get_id_by_index(0), 42);
}

#[test]
fn indexed_field_names() {
    let mut mt = make_mt();
    mt.register_property_index("a");
    mt.register_property_index("b");
    let names = mt.indexed_field_names();
    assert!(names.contains("a"));
    assert!(names.contains("b"));
    assert_eq!(names.len(), 2);
}
