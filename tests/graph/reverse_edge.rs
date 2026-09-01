use std::time::Instant;
use triviumdb::Database;

#[test]
fn test_reverse_edge_avalanche_deletion() {
    let db_path = "test_reverse_edge_avalanche.tdb";
    let _ = std::fs::remove_file(db_path);
    let _ = std::fs::remove_file(format!("{}.vec", db_path));
    let _ = std::fs::remove_file(format!("{}.wal", db_path));
    let _ = std::fs::remove_file(format!("{}.lock", db_path));

    let mut db = Database::<f32>::open(db_path, 8).unwrap();
    db.disable_auto_compaction();

    let center_id = db
        .insert(&[0.0; 8], serde_json::json!({"role": "center"}))
        .unwrap();

    let num_fans = 10_000;

    // Create fan nodes and link to center
    let mut fan_ids = Vec::new();
    for i in 0..num_fans {
        let fan_id = db
            .insert(&[1.0; 8], serde_json::json!({"role": "fan", "id": i}))
            .unwrap();
        db.link(fan_id, center_id, "follows", 1.0).unwrap();
        fan_ids.push(fan_id);
    }

    assert_eq!(db.node_count(), num_fans as usize + 1);

    // Verify center is heavily linked
    for fan_id in &fan_ids {
        let fan_node = db.get(*fan_id).unwrap();
        assert_eq!(fan_node.edges.len(), 1);
        assert_eq!(fan_node.edges[0].target_id, center_id);
    }

    // Now delete the center node
    let start = Instant::now();
    db.delete(center_id).unwrap();
    let elapsed = start.elapsed();

    println!(
        "Deleted center node with {} incoming edges in {:?}",
        num_fans, elapsed
    );

    // Verify deletion speed (should be virtually instantaneous, < 500ms even in debug mode)
    assert!(
        elapsed.as_millis() < 500,
        "Deletion took too long, might be hitting O(E) avalanche!"
    );

    // Verify correctness: center is gone
    assert!(db.get(center_id).is_none());
    assert_eq!(db.node_count(), num_fans as usize);

    // Verify correctness: all fans have NO outgoing edges pointing to center!
    for fan_id in &fan_ids {
        let fan_node = db.get(*fan_id).unwrap();
        assert!(
            fan_node.edges.is_empty(),
            "Fan should not have any remaining edges to the deleted center"
        );
    }

    // Verify unlink cleanup logic correctly removes reverse edge
    let node_a = db.insert(&[0.0; 8], serde_json::json!("A")).unwrap();
    let node_b = db.insert(&[0.0; 8], serde_json::json!("B")).unwrap();
    db.link(node_a, node_b, "test", 1.0).unwrap();
    db.unlink(node_a, node_b).unwrap();
    // After unlink, deleting B should theoretically not need to touch A anymore.
    db.delete(node_b).unwrap();
    let a_node = db.get(node_a).unwrap();
    assert!(a_node.edges.is_empty());

    // Cleanup
    drop(db);
    let _ = std::fs::remove_file(db_path);
    let _ = std::fs::remove_file(format!("{}.vec", db_path));
    let _ = std::fs::remove_file(format!("{}.wal", db_path));
    let _ = std::fs::remove_file(format!("{}.lock", db_path));
}
