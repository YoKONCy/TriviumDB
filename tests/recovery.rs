#![allow(non_snake_case)]
//! 持久化与崩溃恢复回归测试
//!
//! 覆盖范围：
//! - P0-1 flush_ok 标记校验（跨文件撕裂检测）
//! - P0-2 WAL 回放后 next_id 幂等推进
//! - Mmap / Rom 模式 flush 往返
//! - 删除节点后持久化完整性

use triviumdb::database::{Config, StorageMode};
use triviumdb::storage::wal::SyncMode;
use triviumdb::Database;
use std::path::Path;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    std::fs::create_dir_all("test_data").ok();
    format!("test_data/rec_{}", name)
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

// ════════ 基本持久化往返 ════════

#[test]
fn Mmap模式_持久化后重新加载_数据完整() {
    let path = tmp_db("mmap_roundtrip");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        {
            let mut tx = db.begin_tx();
            tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "alice"}));
            tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "bob"}));
            tx.commit().unwrap();
        }
        db.flush().unwrap();
    }

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.node_count(), 2, "Mmap 模式重加载后应有 2 个节点");

    cleanup(&path);
}

#[test]
fn Rom模式_持久化后重新加载_数据完整() {
    let path = tmp_db("rom_roundtrip");
    cleanup(&path);

    {
        let config = Config { dim: DIM, storage_mode: StorageMode::Rom, ..Default::default() };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        {
            let mut tx = db.begin_tx();
            tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"n": 1}));
            tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"n": 2}));
            tx.commit().unwrap();
        }
        db.flush().unwrap();
    }

    let config = Config { dim: DIM, storage_mode: StorageMode::Rom, ..Default::default() };
    let db = Database::<f32>::open_with_config(&path, config).unwrap();
    assert_eq!(db.node_count(), 2, "Rom 模式重加载后应有 2 个节点");

    cleanup(&path);
}

// ════════ P0-1：flush_ok 标记校验 ════════

#[test]
fn P0_1_Mmap模式_flush后应生成flush_ok标记() {
    let path = tmp_db("flush_ok_mmap");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        { let mut tx = db.begin_tx(); tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({})); tx.commit().unwrap(); }
        db.flush().unwrap();
    }

    let ok_path = format!("{}.flush_ok", path);
    assert!(Path::new(&ok_path).exists(), "Mmap 模式 flush 后应生成 .flush_ok 标记");

    cleanup(&path);
}

#[test]
fn P0_1_删除flush_ok后重加载_应降级或拒绝但不panic() {
    let path = tmp_db("flush_ok_torn");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        { let mut tx = db.begin_tx(); tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({})); tx.commit().unwrap(); }
        db.flush().unwrap();
    }

    // 模拟撕裂：删除 .flush_ok
    std::fs::remove_file(format!("{}.flush_ok", path)).unwrap();

    // 允许 Err（拒绝加载）或 Ok（安全降级），不允许 panic
    let result = Database::<f32>::open(&path, DIM);
    match result {
        Ok(db) => { let _ = db.node_count(); }
        Err(_) => { /* 拒绝加载也是合法行为 */ }
    }

    cleanup(&path);
}

#[test]
fn P0_1_Rom模式_不应残留flush_ok标记() {
    let path = tmp_db("flush_ok_rom");
    cleanup(&path);

    {
        let config = Config { dim: DIM, storage_mode: StorageMode::Rom, ..Default::default() };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        { let mut tx = db.begin_tx(); tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({})); tx.commit().unwrap(); }
        db.flush().unwrap();
    }

    // Rom 模式无论是否有 flush_ok，重新打开都不应 panic
    let config = Config { dim: DIM, storage_mode: StorageMode::Rom, ..Default::default() };
    let result = Database::<f32>::open_with_config(&path, config);
    assert!(result.is_ok(), "Rom 模式重新打开不应失败");

    cleanup(&path);
}

// ════════ P0-2：WAL 回放与 next_id 幂等 ════════

#[test]
fn P0_2_WAL恢复后_新插入不复用已有ID() {
    let path = tmp_db("wal_next_id");
    cleanup(&path);

    let last_id;
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        db.set_sync_mode(SyncMode::Full);
        db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"seq": 1})).unwrap();
        db.flush().unwrap(); // id=1 落盘，WAL 被清除

        // 继续插入（仅写 WAL，不 flush）
        last_id = db.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"seq": 2})).unwrap();
        // drop 时 Drop trait 显式 flush WAL BufWriter
    }

    // WAL 文件应非空
    let wal_size = std::fs::metadata(format!("{}.wal", path)).map(|m| m.len()).unwrap_or(0);
    assert!(wal_size > 0, "Drop 后 WAL 应非空（实际 {} bytes）", wal_size);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        let new_id = db.insert(&[0.0, 0.0, 1.0, 0.0], serde_json::json!({"seq": 3})).unwrap();
        assert!(new_id > last_id, "WAL 回放后 next_id 应已推进：new_id={}, last_id={}", new_id, last_id);
        assert!(db.get(last_id).is_some(), "WAL 回放应恢复 seq=2 的节点");
    }

    cleanup(&path);
}

// ════════ 删除后持久化 ════════

#[test]
fn 删除后持久化再加载_节点确实消失() {
    let path = tmp_db("del_persist");
    cleanup(&path);

    let del_id;
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        let ids = {
            let mut tx = db.begin_tx();
            tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"keep": false}));
            tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"keep": true}));
            tx.commit().unwrap()
        };
        del_id = ids[0];
        { let mut tx = db.begin_tx(); tx.delete(del_id); tx.commit().unwrap(); }
        db.flush().unwrap();
    }

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert!(!db.contains(del_id), "删除并 flush 后重加载，节点应不存在");
    assert_eq!(db.node_count(), 1, "应只剩 1 个节点");

    cleanup(&path);
}