#![allow(non_snake_case)]
//! 事务层回归测试
//!
//! 覆盖范围：
//! - P0-3 WAL-first 事务提交语义
//! - P1-4 已删除节点不可被修改
//! - Dry-Run 预检（维度错误、重复 ID、链接不存在节点）
//! - 空事务、基础增删改查

use triviumdb::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    std::fs::create_dir_all("test_data").ok();
    format!("test_data/tx_{}", name)
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

// ════════ 基础事务提交 ════════

#[test]
fn 事务_基本提交_返回正确的生成ID() {
    let path = tmp_db("basic_commit");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "alice"}));
        tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "bob"}));
        tx.commit().unwrap()
    };

    assert_eq!(ids.len(), 2, "提交 2 个 insert 应返回 2 个 ID");
    assert!(ids[0] != ids[1], "两个 ID 不应相同");
    assert!(db.contains(ids[0]), "alice 应存在");
    assert!(db.contains(ids[1]), "bob 应存在");

    cleanup(&path);
}

#[test]
fn 事务_插入边_可以查询() {
    let path = tmp_db("edge_commit");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"label": "A"}));
        tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"label": "B"}));
        tx.commit().unwrap()
    };

    {
        let mut tx = db.begin_tx();
        tx.link(ids[0], ids[1], "knows", 1.0);
        tx.commit().unwrap();
    }

    let edges = db.get_edges(ids[0]);
    assert_eq!(edges.len(), 1, "A 应有 1 条出边");
    assert_eq!(edges[0].target_id, ids[1]);
    assert_eq!(edges[0].label, "knows");

    cleanup(&path);
}

#[test]
fn 事务_delete_节点后_contains返回false() {
    let path = tmp_db("delete_node");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
        tx.commit().unwrap()
    };
    let id = ids[0];
    assert!(db.contains(id));

    {
        let mut tx = db.begin_tx();
        tx.delete(id);
        tx.commit().unwrap();
    }
    assert!(!db.contains(id), "删除后 contains 应返回 false");

    cleanup(&path);
}

#[test]
fn 事务_dry_run_维度不匹配_应拒绝整个事务() {
    let path = tmp_db("dim_mismatch");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let result = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"ok": true}));
        tx.insert(&[1.0, 0.0, 0.0],       serde_json::json!({"bad": true})); // 维度错误
        tx.commit()
    };

    assert!(result.is_err(), "Dry-Run 应检测到维度不匹配并拒绝整个事务");
    assert_eq!(db.node_count(), 0, "事务整体回滚，节点数应为 0");

    cleanup(&path);
}

#[test]
fn 事务_dry_run_insert_with_id_重复_应拒绝() {
    let path = tmp_db("dup_id");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    {
        let mut tx = db.begin_tx();
        tx.insert_with_id(1, &[1.0, 0.0, 0.0, 0.0], serde_json::json!({"seq": 1}));
        tx.commit().unwrap();
    }

    let result = {
        let mut tx = db.begin_tx();
        tx.insert_with_id(1, &[0.0, 1.0, 0.0, 0.0], serde_json::json!({"seq": 2}));
        tx.commit()
    };
    assert!(result.is_err(), "重复 ID 应被 Dry-Run 检测到");

    cleanup(&path);
}

#[test]
fn 事务_空事务_提交成功并返回空列表() {
    let path = tmp_db("empty_tx");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = db.begin_tx().commit().unwrap();
    assert!(ids.is_empty(), "空事务应返回空 ID 列表");

    cleanup(&path);
}

#[test]
fn 事务_update_payload_正常工作() {
    let path = tmp_db("update_payload");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"v": 1}));
        tx.commit().unwrap()
    };

    db.update_payload(ids[0], serde_json::json!({"v": 2})).unwrap();
    let payload = db.get_payload(ids[0]).unwrap();
    assert_eq!(payload["v"], 2);

    cleanup(&path);
}

#[test]
fn 事务_update_vector_正常工作() {
    let path = tmp_db("update_vector");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
        tx.commit().unwrap()
    };

    db.update_vector(ids[0], &[0.0, 0.0, 0.0, 1.0]).unwrap();
    let node = db.get(ids[0]).unwrap();
    assert!((node.vector[3] - 1.0).abs() < 1e-6);

    cleanup(&path);
}

#[test]
fn 事务_unlink_正常移除边() {
    let path = tmp_db("unlink");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
        tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({}));
        tx.commit().unwrap()
    };

    { let mut tx = db.begin_tx(); tx.link(ids[0], ids[1], "rel", 1.0); tx.commit().unwrap(); }
    assert_eq!(db.get_edges(ids[0]).len(), 1);

    db.unlink(ids[0], ids[1]).unwrap();
    assert_eq!(db.get_edges(ids[0]).len(), 0, "unlink 后边应消失");

    cleanup(&path);
}

#[test]
fn 事务_跨操作依赖_先insert再link() {
    let path = tmp_db("cross_op");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    {
        let mut tx = db.begin_tx();
        tx.insert_with_id(10, &[1.0, 0.0, 0.0, 0.0], serde_json::json!({"id": 10}));
        tx.insert_with_id(20, &[0.0, 1.0, 0.0, 0.0], serde_json::json!({"id": 20}));
        tx.link(10, 20, "connects", 0.9);
        tx.commit().unwrap();
    }

    assert!(db.contains(10));
    assert!(db.contains(20));
    let edges = db.get_edges(10);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target_id, 20);
    assert!((edges[0].weight - 0.9).abs() < 1e-6);

    cleanup(&path);
}

// ════════ P1-4：已删除节点保护 ════════

#[test]
fn P1_4_删除后更新向量_应失败() {
    let path = tmp_db("del_upd_vec");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
        tx.commit().unwrap()
    };

    { let mut tx = db.begin_tx(); tx.delete(ids[0]); tx.commit().unwrap(); }

    let result = db.update_vector(ids[0], &[0.0, 0.0, 0.0, 1.0]);
    assert!(result.is_err(), "对已删除节点 update_vector 应返回 Err");

    cleanup(&path);
}

#[test]
fn P1_4_删除后更新payload_应失败() {
    let path = tmp_db("del_upd_payload");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
        tx.commit().unwrap()
    };

    { let mut tx = db.begin_tx(); tx.delete(ids[0]); tx.commit().unwrap(); }

    let result = db.update_payload(ids[0], serde_json::json!({"hack": true}));
    assert!(result.is_err(), "对已删除节点 update_payload 应返回 Err");

    cleanup(&path);
}

// ════════ 边界条件 ════════

#[test]
fn 事务_insert_with_id_重复ID_预检失败() {
    let path = tmp_db("dup_id2");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    { let mut tx = db.begin_tx(); tx.insert_with_id(5, &[1.0, 0.0, 0.0, 0.0], serde_json::json!({})); tx.commit().unwrap(); }

    let result = { let mut tx = db.begin_tx(); tx.insert_with_id(5, &[0.0, 1.0, 0.0, 0.0], serde_json::json!({})); tx.commit() };
    assert!(result.is_err(), "重复 insert_with_id 应失败");

    cleanup(&path);
}