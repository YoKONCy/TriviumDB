use triviumdb::Database;

use crate::support;

#[test]
fn test_reverse_edge_avalanche_deletion() {
    let root = support::TestDir::new("reverse_edge");
    let path = root.file("reverse-edge.tdb");
    let db_path = path.as_str();

    let mut db = Database::<f32>::open(db_path, 8).unwrap();
    db.disable_auto_compaction();

    let center_id = db
        .insert(&[0.0; 8], serde_json::json!({"role": "center"}))
        .unwrap();

    let num_fans = 10_000;

    // 创建入边来源，验证删除高入度节点的完整级联。
    let mut fan_ids = Vec::new();
    for i in 0..num_fans {
        let fan_id = db
            .insert(&[1.0; 8], serde_json::json!({"role": "fan", "id": i}))
            .unwrap();
        db.link(fan_id, center_id, "follows", 1.0).unwrap();
        fan_ids.push(fan_id);
    }

    assert_eq!(db.node_count().unwrap(), num_fans as usize + 1);

    // 每条入边必须实际存在。
    for fan_id in &fan_ids {
        let fan_node = db.get(*fan_id).unwrap().unwrap();
        assert_eq!(fan_node.edges.len(), 1);
        assert_eq!(fan_node.edges[0].target_id, center_id);
    }

    // 删除中心节点后核对全部关联边。
    db.delete(center_id).unwrap();
    assert!(db.get(center_id).unwrap().is_none());
    assert_eq!(db.node_count().unwrap(), num_fans as usize);

    // 所有来源节点的出边均已清理。
    for fan_id in &fan_ids {
        let fan_node = db.get(*fan_id).unwrap().unwrap();
        assert!(
            fan_node.edges.is_empty(),
            "来源节点仍保留指向已删除中心的边"
        );
    }

    // 独立断边同步移除反向目录。
    let node_a = db.insert(&[0.0; 8], serde_json::json!("A")).unwrap();
    let node_b = db.insert(&[0.0; 8], serde_json::json!("B")).unwrap();
    db.link(node_a, node_b, "test", 1.0).unwrap();
    db.unlink(node_a, node_b).unwrap();
    db.delete(node_b).unwrap();
    let a_node = db.get(node_a).unwrap().unwrap();
    assert!(a_node.edges.is_empty());

    drop(db);
}
