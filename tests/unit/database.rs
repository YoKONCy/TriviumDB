//! Database 公开 API 的单元测试
//!
//! 覆盖: open, insert, insert_with_id, link, unlink, delete,
//!       update_payload, update_vector, flush, compact,
//!       set_hook, clear_hook, set_sync_mode, set_memory_limit,
//!       estimated_memory, get_payload, get_vector, get_edges,
//!       node_count, contains, search, query (TQL),
//!       find_nodes_by_field, register_property_index

use serde_json::json;
use triviumdb::{Database, TriviumError};

fn temp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("tdb_unit_{}", name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("test.tdb").to_string_lossy().to_string()
}

fn open_db(name: &str) -> Database<f32> {
    Database::open(&temp_db(name), 3).unwrap()
}

// ═══════════════════════════════════════════════════════════════
//  Open / Config
// ═══════════════════════════════════════════════════════════════

#[test]
fn open_新建数据库() {
    let db = open_db("open_new");
    assert_eq!(db.node_count(), 0);
}

#[test]
fn open_维度0报错() {
    let result = Database::<f32>::open(&temp_db("dim0"), 0);
    assert!(result.is_err());
}

#[test]
fn open_with_sync() {
    let path = temp_db("sync");
    let db = Database::<f32>::open_with_sync(&path, 3, triviumdb::storage::wal::SyncMode::Full);
    assert!(db.is_ok());
}

// ═══════════════════════════════════════════════════════════════
//  CRUD 操作
// ═══════════════════════════════════════════════════════════════

#[test]
fn insert_和_get() {
    let mut db = open_db("insert_get");
    let id = db
        .insert(&[1.0, 2.0, 3.0], json!({"name": "test"}))
        .unwrap();
    assert!(db.contains(id));
    assert_eq!(db.node_count(), 1);

    let payload = db.get_payload(id).unwrap();
    assert_eq!(payload["name"], "test");
}

#[test]
fn insert_with_id_和_get() {
    let mut db = open_db("insert_id");
    db.insert_with_id(42, &[1.0, 0.0, 0.0], json!({})).unwrap();
    assert!(db.contains(42));

    let node = db.get(42).unwrap();
    assert_eq!(node.vector, vec![1.0, 0.0, 0.0]);
    assert_eq!(node.id, 42);
}

#[test]
fn insert_with_id_拒绝墓碑保留ID零() {
    let mut db = open_db("insert_zero_id");
    let err = db.insert_with_id(0, &[1.0, 0.0, 0.0], json!({}));

    assert!(matches!(err, Err(TriviumError::InvalidInput(_))));
    assert_eq!(db.node_count(), 0);
}

#[test]
fn insert_with_id_拒绝无后继的最大ID() {
    let mut db = open_db("insert_max_id");
    let err = db.insert_with_id(u64::MAX, &[1.0, 0.0, 0.0], json!({}));

    assert!(matches!(err, Err(TriviumError::InvalidInput(_))));
    assert_eq!(db.node_count(), 0);
    assert_eq!(db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap(), 1);
}

#[test]
fn insert_在最大可用ID后返回空间耗尽且不写入() {
    let mut db = open_db("insert_exhausted_id_space");
    db.insert_with_id(u64::MAX - 1, &[1.0, 0.0, 0.0], json!({}))
        .unwrap();

    let err = db.insert(&[0.0, 1.0, 0.0], json!({}));
    assert!(matches!(err, Err(TriviumError::InvalidInput(_))));
    assert_eq!(db.node_count(), 1);
    assert!(db.contains(u64::MAX - 1));
    assert!(!db.contains(0));
}

#[test]
fn delete_操作() {
    let mut db = open_db("delete");
    let id = db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap();
    db.delete(id).unwrap();
    assert!(!db.contains(id));
    assert_eq!(db.node_count(), 0);
}

#[test]
fn link_和_unlink() {
    let mut db = open_db("link_unlink");
    let id1 = db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap();
    let id2 = db.insert(&[0.0, 1.0, 0.0], json!({})).unwrap();

    db.link(id1, id2, "knows", 1.0).unwrap();
    let edges = db.get_edges(id1);
    assert_eq!(edges.len(), 1);

    db.unlink(id1, id2).unwrap();
    let edges = db.get_edges(id1);
    assert!(edges.is_empty());
}

#[test]
fn update_payload_操作() {
    let mut db = open_db("update_payload");
    let id = db.insert(&[1.0, 0.0, 0.0], json!({"v": 1})).unwrap();
    db.update_payload(id, json!({"v": 2})).unwrap();
    assert_eq!(db.get_payload(id).unwrap()["v"], 2);
}

#[test]
fn update_vector_操作() {
    let mut db = open_db("update_vector");
    let id = db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap();
    db.update_vector(id, &[0.0, 1.0, 0.0]).unwrap();
    let vec = db.get(id).unwrap().vector;
    assert_eq!(vec, vec![0.0, 1.0, 0.0]);
}

// ═══════════════════════════════════════════════════════════════
//  flush / compact
// ═══════════════════════════════════════════════════════════════

#[test]
fn flush_空数据库() {
    let mut db = open_db("flush_empty");
    db.flush().unwrap();
}

#[test]
fn flush_和_compact() {
    let mut db = open_db("flush_compact");
    db.insert(&[1.0, 0.0, 0.0], json!({"a": 1})).unwrap();
    db.flush().unwrap();
    db.compact().unwrap();
    assert_eq!(db.node_count(), 1);
}

// ═══════════════════════════════════════════════════════════════
//  Hook / Config API
// ═══════════════════════════════════════════════════════════════

#[test]
fn set_hook_和_clear_hook() {
    let mut db = open_db("hook");
    db.set_hook(triviumdb::NoopHook);
    db.clear_hook();
}

#[test]
fn set_sync_mode() {
    let mut db = open_db("sync_mode");
    db.set_sync_mode(triviumdb::storage::wal::SyncMode::Full);
    db.set_sync_mode(triviumdb::storage::wal::SyncMode::Normal);
}

#[test]
fn set_memory_limit_和_estimated_memory() {
    let mut db = open_db("mem_limit");
    db.set_memory_limit(1024 * 1024 * 100);
    db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap();
    assert!(db.estimated_memory() > 0);
}

// ═══════════════════════════════════════════════════════════════
//  find_nodes_by_field / property_index
// ═══════════════════════════════════════════════════════════════

// find_nodes_by_field is a MemTable method tested in memtable.rs
// Database-level field filtering is done via TQL

#[test]
fn create_index_和_drop_index() {
    let mut db = open_db("prop_idx");
    db.create_index("role");
    db.insert(&[1.0, 0.0, 0.0], json!({"role": "admin"}))
        .unwrap();
    db.insert(&[0.0, 1.0, 0.0], json!({"role": "user"}))
        .unwrap();

    // 通过 TQL 验证索引加速查询可用
    let result = db.tql("FIND {role: \"admin\"} RETURN *");
    assert!(result.is_ok());

    db.drop_index("role");
}

// ═══════════════════════════════════════════════════════════════
//  Search / TQL
// ═══════════════════════════════════════════════════════════════

#[test]
fn search_基础() {
    let mut db = open_db("search_basic");
    db.insert(&[1.0, 0.0, 0.0], json!({"name": "a"})).unwrap();
    db.insert(&[0.0, 1.0, 0.0], json!({"name": "b"})).unwrap();

    let results = db.search(&[1.0, 0.0, 0.0], 5, 0, 0.0).unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].payload["name"], "a");
}

#[test]
fn search_advanced() {
    let mut db = open_db("search_adv");
    db.insert(&[1.0, 0.0, 0.0], json!({"name": "a"})).unwrap();
    db.insert(&[0.0, 1.0, 0.0], json!({"name": "b"})).unwrap();

    let config = triviumdb::database::SearchConfig::default();
    let results = db.search_advanced(&[1.0, 0.0, 0.0], &config).unwrap();
    assert!(!results.is_empty());
}

// ═══════════════════════════════════════════════════════════════
//  错误路径
// ═══════════════════════════════════════════════════════════════

#[test]
fn delete_不存在节点() {
    let mut db = open_db("del_404");
    assert!(db.delete(999).is_err());
}

#[test]
fn update_payload_不存在() {
    let mut db = open_db("upd_404");
    assert!(db.update_payload(999, json!({})).is_err());
}

// ═══════════════════════════════════════════════════════════════
//  patch_payload (部分更新 API)
// ═══════════════════════════════════════════════════════════════

#[test]
fn patch_payload_set_操作() {
    let mut db = open_db("patch_set");
    let id = db
        .insert(&[1.0, 0.0, 0.0], json!({"name": "Alice", "age": 25}))
        .unwrap();
    db.patch_payload(id, json!({"$set": {"age": 26, "city": "Tokyo"}}))
        .unwrap();
    let p = db.get_payload(id).unwrap();
    assert_eq!(p["name"], "Alice", "$set 不应影响未修改字段");
    assert_eq!(p["age"], 26);
    assert_eq!(p["city"], "Tokyo");
}

#[test]
fn patch_payload_inc_操作() {
    let mut db = open_db("patch_inc");
    let id = db
        .insert(&[1.0, 0.0, 0.0], json!({"visits": 10}))
        .unwrap();
    db.patch_payload(id, json!({"$inc": {"visits": 3}}))
        .unwrap();
    assert_eq!(db.get_payload(id).unwrap()["visits"], 13);

    // 连续递增
    db.patch_payload(id, json!({"$inc": {"visits": -5}}))
        .unwrap();
    assert_eq!(db.get_payload(id).unwrap()["visits"], 8);
}

#[test]
fn patch_payload_unset_操作() {
    let mut db = open_db("patch_unset");
    let id = db
        .insert(&[1.0, 0.0, 0.0], json!({"a": 1, "b": 2}))
        .unwrap();
    db.patch_payload(id, json!({"$unset": {"b": true}}))
        .unwrap();
    let p = db.get_payload(id).unwrap();
    assert_eq!(p["a"], 1);
    assert!(p.get("b").is_none());
}

#[test]
fn patch_payload_简写模式() {
    let mut db = open_db("patch_short");
    let id = db
        .insert(&[1.0, 0.0, 0.0], json!({"x": 1}))
        .unwrap();
    db.patch_payload(id, json!({"y": 2})).unwrap();
    let p = db.get_payload(id).unwrap();
    assert_eq!(p["x"], 1, "简写不应覆盖已有字段");
    assert_eq!(p["y"], 2);
}

#[test]
fn patch_payload_组合操作() {
    let mut db = open_db("patch_combo");
    let id = db
        .insert(
            &[1.0, 0.0, 0.0],
            json!({"name": "Alice", "score": 100, "deprecated": true}),
        )
        .unwrap();
    db.patch_payload(
        id,
        json!({
            "$set": {"name": "Bob"},
            "$inc": {"score": -10},
            "$unset": {"deprecated": true}
        }),
    )
    .unwrap();
    let p = db.get_payload(id).unwrap();
    assert_eq!(p["name"], "Bob");
    assert_eq!(p["score"], 90);
    assert!(p.get("deprecated").is_none());
}

#[test]
fn patch_payload_不存在节点() {
    let mut db = open_db("patch_404");
    assert!(
        db.patch_payload(999, json!({"$set": {"x": 1}})).is_err(),
        "对不存在节点 patch_payload 应返回 Err"
    );
}

#[test]
fn patch_payload_WAL持久化() {
    // 验证 patch 后关闭再打开，数据仍在
    let path = temp_db("patch_wal");
    let id;
    {
        let mut db = Database::<f32>::open(&path, 3).unwrap();
        id = db
            .insert(&[1.0, 0.0, 0.0], json!({"counter": 0}))
            .unwrap();
        db.patch_payload(id, json!({"$inc": {"counter": 42}}))
            .unwrap();
        db.close().unwrap();
    }
    {
        let db = Database::<f32>::open(&path, 3).unwrap();
        let p = db.get_payload(id).unwrap();
        assert_eq!(p["counter"], 42, "WAL 恢复后 patch 结果应持久化");
    }
}

#[test]
fn update_vector_不存在() {
    let mut db = open_db("vec_404");
    assert!(db.update_vector(999, &[1.0, 0.0, 0.0]).is_err());
}

#[test]
fn link_不存在节点() {
    let mut db = open_db("link_404");
    db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap();
    assert!(db.link(1, 999, "x", 1.0).is_err());
}

#[test]
fn insert_维度不匹配() {
    let mut db = open_db("dim_mismatch");
    assert!(db.insert(&[1.0, 0.0], json!({})).is_err());
}

#[test]
fn neighbors_基础() {
    let mut db = open_db("neighbors");
    let id1 = db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap();
    let id2 = db.insert(&[0.0, 1.0, 0.0], json!({})).unwrap();
    let id3 = db.insert(&[0.0, 0.0, 1.0], json!({})).unwrap();
    db.link(id1, id2, "a", 1.0).unwrap();
    db.link(id2, id3, "b", 1.0).unwrap();

    let n1 = db.neighbors(id1, 1);
    assert_eq!(n1.len(), 1);
    let n2 = db.neighbors(id1, 2);
    assert_eq!(n2.len(), 2);
}

#[test]
fn dim_和_all_node_ids() {
    let mut db = open_db("dim_ids");
    assert_eq!(db.dim(), 3);
    db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap();
    db.insert(&[0.0, 1.0, 0.0], json!({})).unwrap();
    assert_eq!(db.all_node_ids().len(), 2);
}

#[test]
fn tql_find_读查询() {
    let mut db = open_db("tql_read");
    db.insert(&[1.0, 0.0, 0.0], json!({"type": "person"}))
        .unwrap();
    let result = db.tql("FIND {type: \"person\"} RETURN *");
    assert!(result.is_ok());
}

#[test]
fn close_操作() {
    let mut db = open_db("close_op");
    db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap();
    db.close().unwrap();
}

// ═══════════════════════════════════════════════════════════════
//  Transaction 事务测试
// ═══════════════════════════════════════════════════════════════

#[test]
fn tx_空事务提交() {
    let mut db = open_db("tx_empty");
    let tx = db.begin_tx();
    let ids = tx.commit().unwrap();
    assert!(ids.is_empty());
}

#[test]
fn tx_insert_和_commit() {
    let mut db = open_db("tx_insert");
    {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0], json!({"name": "Alice"}));
        tx.insert(&[0.0, 1.0, 0.0], json!({"name": "Bob"}));
        assert_eq!(tx.pending_count(), 2);
        let ids = tx.commit().unwrap();
        assert_eq!(ids.len(), 2);
    }
    assert_eq!(db.node_count(), 2);
}

#[test]
fn tx_insert_with_id() {
    let mut db = open_db("tx_insert_id");
    {
        let mut tx = db.begin_tx();
        tx.insert_with_id(100, &[1.0, 0.0, 0.0], json!({}));
        tx.commit().unwrap();
    }
    assert!(db.contains(100));
}

#[test]
fn tx_insert_with_id_拒绝墓碑保留ID零() {
    let mut db = open_db("tx_insert_zero_id");
    let mut tx = db.begin_tx();
    tx.insert_with_id(0, &[1.0, 0.0, 0.0], json!({}));

    assert!(matches!(tx.commit(), Err(TriviumError::InvalidInput(_))));
    assert_eq!(db.node_count(), 0);
}

#[test]
fn tx_insert_with_id_拒绝无后继的最大ID且保持原子性() {
    let mut db = open_db("tx_insert_max_id");
    let mut tx = db.begin_tx();
    tx.insert_with_id(u64::MAX, &[1.0, 0.0, 0.0], json!({}));
    tx.insert(&[0.0, 1.0, 0.0], json!({}));

    assert!(matches!(tx.commit(), Err(TriviumError::InvalidInput(_))));
    assert_eq!(db.node_count(), 0);
    assert_eq!(db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap(), 1);
}

#[test]
fn tx_最大可用ID后自动分配失败且保持原子性() {
    let mut db = open_db("tx_exhausted_id_space");
    let mut tx = db.begin_tx();
    tx.insert_with_id(u64::MAX - 1, &[1.0, 0.0, 0.0], json!({}));
    tx.insert(&[0.0, 1.0, 0.0], json!({}));

    assert!(matches!(tx.commit(), Err(TriviumError::InvalidInput(_))));
    assert_eq!(db.node_count(), 0);
    assert!(!db.contains(u64::MAX - 1));
    assert!(!db.contains(0));
}

#[test]
fn tx_link_和_unlink() {
    let mut db = open_db("tx_link");
    db.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    db.insert_with_id(2, &[0.0, 1.0, 0.0], json!({})).unwrap();
    {
        let mut tx = db.begin_tx();
        tx.link(1, 2, "knows", 1.0);
        tx.commit().unwrap();
    }
    assert_eq!(db.get_edges(1).len(), 1);
    {
        let mut tx = db.begin_tx();
        tx.unlink(1, 2);
        tx.commit().unwrap();
    }
    assert!(db.get_edges(1).is_empty());
}

#[test]
fn tx_delete() {
    let mut db = open_db("tx_delete");
    db.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    {
        let mut tx = db.begin_tx();
        tx.delete(1);
        tx.commit().unwrap();
    }
    assert!(!db.contains(1));
}

#[test]
fn tx_update_payload_和_vector() {
    let mut db = open_db("tx_update");
    db.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"v": 1}))
        .unwrap();
    {
        let mut tx = db.begin_tx();
        tx.update_payload(1, json!({"v": 2}));
        tx.update_vector(1, &[0.0, 1.0, 0.0]);
        tx.commit().unwrap();
    }
    assert_eq!(db.get_payload(1).unwrap()["v"], 2);
    assert_eq!(db.get(1).unwrap().vector, vec![0.0, 1.0, 0.0]);
}

#[test]
fn tx_rollback_不影响数据库() {
    let mut db = open_db("tx_rollback");
    db.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    {
        let mut tx = db.begin_tx();
        tx.delete(1);
        tx.rollback();
    }
    assert!(db.contains(1), "rollback 后节点应仍存在");
}

#[test]
fn tx_insert_NaN向量报错() {
    let mut db = open_db("tx_nan");
    let mut tx = db.begin_tx();
    tx.insert(&[f32::NAN, 0.0, 0.0], json!({}));
    assert!(tx.commit().is_err());
    assert_eq!(db.node_count(), 0, "失败的事务不应改变状态");
}

#[test]
fn tx_insert_维度不匹配() {
    let mut db = open_db("tx_dim");
    let mut tx = db.begin_tx();
    tx.insert(&[1.0, 0.0], json!({})); // dim=2, expected=3
    assert!(tx.commit().is_err());
}

#[test]
fn tx_insert_with_id_重复ID报错() {
    let mut db = open_db("tx_dup_id");
    db.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    let mut tx = db.begin_tx();
    tx.insert_with_id(1, &[0.0, 1.0, 0.0], json!({}));
    assert!(tx.commit().is_err());
}

#[test]
fn tx_link_不存在节点报错() {
    let mut db = open_db("tx_link_404");
    let mut tx = db.begin_tx();
    tx.link(999, 888, "x", 1.0);
    assert!(tx.commit().is_err());
}

#[test]
fn tx_delete_不存在节点报错() {
    let mut db = open_db("tx_del_404");
    let mut tx = db.begin_tx();
    tx.delete(999);
    assert!(tx.commit().is_err());
}

#[test]
fn tx_update_payload_不存在报错() {
    let mut db = open_db("tx_upd_404");
    let mut tx = db.begin_tx();
    tx.update_payload(999, json!({}));
    assert!(tx.commit().is_err());
}

#[test]
fn tx_update_vector_不存在报错() {
    let mut db = open_db("tx_vec_404");
    let mut tx = db.begin_tx();
    tx.update_vector(999, &[1.0, 0.0, 0.0]);
    assert!(tx.commit().is_err());
}

#[test]
fn tx_update_vector_维度不匹配() {
    let mut db = open_db("tx_vec_dim");
    db.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    let mut tx = db.begin_tx();
    tx.update_vector(1, &[1.0, 0.0]); // dim=2, expected=3
    assert!(tx.commit().is_err());
}

#[test]
fn tx_update_vector_NaN报错() {
    let mut db = open_db("tx_vec_nan");
    db.insert_with_id(1, &[1.0, 0.0, 0.0], json!({})).unwrap();
    let mut tx = db.begin_tx();
    tx.update_vector(1, &[f32::INFINITY, 0.0, 0.0]);
    assert!(tx.commit().is_err());
}

#[test]
fn tx_insert_with_id_NaN报错() {
    let mut db = open_db("tx_id_nan");
    let mut tx = db.begin_tx();
    tx.insert_with_id(42, &[f32::NAN, 0.0, 0.0], json!({}));
    assert!(tx.commit().is_err());
}

#[test]
fn tx_insert_with_id_维度不匹配() {
    let mut db = open_db("tx_id_dim");
    let mut tx = db.begin_tx();
    tx.insert_with_id(42, &[1.0], json!({})); // dim=1, expected=3
    assert!(tx.commit().is_err());
}

#[test]
fn tx_unlink_不存在src报错() {
    let mut db = open_db("tx_unlink_404");
    let mut tx = db.begin_tx();
    tx.unlink(999, 1);
    assert!(tx.commit().is_err());
}

#[test]
fn tx_insert_后_在同一事务link() {
    let mut db = open_db("tx_insert_link");
    {
        let mut tx = db.begin_tx();
        tx.insert_with_id(10, &[1.0, 0.0, 0.0], json!({}));
        tx.insert_with_id(20, &[0.0, 1.0, 0.0], json!({}));
        tx.link(10, 20, "related", 0.5);
        let ids = tx.commit().unwrap();
        assert_eq!(ids.len(), 2);
    }
    assert_eq!(db.get_edges(10).len(), 1);
}

#[test]
fn tx_insert_后_在同一事务delete() {
    let mut db = open_db("tx_insert_del");
    {
        let mut tx = db.begin_tx();
        tx.insert_with_id(10, &[1.0, 0.0, 0.0], json!({}));
        tx.delete(10);
        tx.commit().unwrap();
    }
    assert!(!db.contains(10));
}

// ═══════════════════════════════════════════════════════════════
//  TQL DML 写操作
// ═══════════════════════════════════════════════════════════════

#[test]
fn tql_mut_create_节点() {
    let mut db = open_db("tql_create");
    let result = db.tql_mut(r#"CREATE ({name: "Alice", age: 30})"#).unwrap();
    assert_eq!(result.affected, 1);
    assert_eq!(result.created_ids.len(), 1);
    assert!(db.contains(result.created_ids[0]));
}

#[test]
fn tql_mut_读查询降级() {
    let mut db = open_db("tql_read_mut");
    db.insert(&[1.0, 0.0, 0.0], json!({"type": "x"})).unwrap();
    let result = db.tql_mut("FIND {type: \"x\"} RETURN *").unwrap();
    assert_eq!(result.affected, 0);
}

// ═══════════════════════════════════════════════════════════════
//  search_hybrid / search_hybrid_with_context
// ═══════════════════════════════════════════════════════════════

#[test]
fn search_hybrid_text_only() {
    let mut db = open_db("hybrid_text");
    let rust_id = db
        .insert(&[1.0, 0.0, 0.0], json!({"name": "rust"}))
        .unwrap();
    let python_id = db
        .insert(&[0.0, 1.0, 0.0], json!({"name": "python"}))
        .unwrap();
    db.index_text(rust_id, "hello world rust").unwrap();
    db.index_text(python_id, "hello world python").unwrap();
    db.build_text_index().unwrap();

    let config = triviumdb::database::SearchConfig {
        top_k: 5,
        expand_depth: 0,
        min_score: -1.0,
        enable_advanced_pipeline: true,
        enable_text_hybrid_search: true,
        ..Default::default()
    };
    let results = db.search_hybrid(Some("rust"), None, &config).unwrap();
    assert_eq!(results.len(), 1, "文本召回应只命中 rust 节点");
    assert_eq!(results[0].id, rust_id, "文本召回应返回被索引的 rust 节点");
    assert_eq!(
        results[0]
            .payload
            .get("name")
            .and_then(|value| value.as_str()),
        Some("rust"),
        "文本召回结果必须携带原始 payload"
    );
}

#[test]
fn search_hybrid_with_context() {
    let mut db = open_db("hybrid_ctx");
    let id = db.insert(&[1.0, 0.0, 0.0], json!({"name": "a"})).unwrap();

    let config = triviumdb::database::SearchConfig {
        top_k: 1,
        expand_depth: 0,
        min_score: -1.0,
        ..Default::default()
    };
    let (results, ctx) = db
        .search_hybrid_with_context(None, Some(&[1.0, 0.0, 0.0]), &config)
        .unwrap();
    assert_eq!(results.len(), 1, "Context 检索应返回完整 Top1");
    assert_eq!(results[0].id, id, "Context 检索应命中查询向量对应节点");
    assert_eq!(
        results[0]
            .payload
            .get("name")
            .and_then(|value| value.as_str()),
        Some("a"),
        "Context 检索结果必须携带原始 payload"
    );
    assert!(!ctx.abort, "默认 Hook 不能提前终止检索");
    let stage_names: std::collections::HashSet<_> = ctx
        .stage_timings
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    for expected in [
        "hook_pre_search",
        "hook_custom_recall",
        "hook_post_recall",
        "hook_pre_graph_expand",
        "graph_expand",
        "hook_rerank",
        "hook_post_search",
    ] {
        assert!(
            stage_names.contains(expected),
            "Context 必须记录阶段耗时: {expected}"
        );
    }
}

#[test]
fn get_all_ids() {
    let mut db = open_db("all_ids");
    db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap();
    db.insert(&[0.0, 1.0, 0.0], json!({})).unwrap();
    let ids = db.get_all_ids();
    assert_eq!(ids.len(), 2);
}

#[test]
fn get_不存在返回None() {
    let db = open_db("get_none");
    assert!(db.get(999).is_none());
}

#[test]
fn index_keyword_和_text() {
    let mut db = open_db("idx_kw");
    let id = db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap();
    db.index_keyword(id, "rust_lang").unwrap();
    db.index_text(id, "triviumdb is a graph database").unwrap();
    db.build_text_index().unwrap();
}
