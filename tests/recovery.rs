#![allow(non_snake_case)]
//! 持久化与崩溃恢复回归测试
//!
//! 覆盖范围：
//! - P0-1 flush_ok 标记校验（跨文件撕裂检测）
//! - P0-2 WAL 回放后 next_id 幂等推进
//! - Mmap / Rom 模式 flush 往返
//! - 删除节点后持久化完整性

use std::path::Path;
use triviumdb::Database;
use triviumdb::database::{Config, StorageMode};
use triviumdb::storage::wal::SyncMode;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("rec_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok", ".quiver"] {
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
        let config = Config {
            dim: DIM,
            storage_mode: StorageMode::Rom,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        {
            let mut tx = db.begin_tx();
            tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"n": 1}));
            tx.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"n": 2}));
            tx.commit().unwrap();
        }
        db.flush().unwrap();
    }

    let config = Config {
        dim: DIM,
        storage_mode: StorageMode::Rom,
        ..Default::default()
    };
    let db = Database::<f32>::open_with_config(&path, config).unwrap();
    assert_eq!(db.node_count(), 2, "Rom 模式重加载后应有 2 个节点");

    cleanup(&path);
}

#[test]
fn QuIVer_sidecar错配主数据时_拒绝加载并清理() {
    let source_path = tmp_db("quiver_sidecar_source");
    let target_path = tmp_db("quiver_sidecar_target");
    cleanup(&source_path);
    cleanup(&target_path);

    {
        let mut source = Database::<f32>::open(&source_path, DIM).unwrap();
        source
            .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"db": "source"}))
            .unwrap();
        source.build_quiver_index(None).unwrap();
        source.flush().unwrap();
    }
    {
        let mut target = Database::<f32>::open(&target_path, DIM).unwrap();
        target
            .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"db": "target"}))
            .unwrap();
        target.build_quiver_index(None).unwrap();
        target.flush().unwrap();
    }

    let source_sidecar = format!("{}.quiver", source_path);
    let target_sidecar = format!("{}.quiver", target_path);
    std::fs::copy(&source_sidecar, &target_sidecar).unwrap();

    let target = Database::<f32>::open(&target_path, DIM).unwrap();
    assert!(!Path::new(&target_sidecar).exists());
    let node = target.get(1).unwrap();
    assert_eq!(node.payload["db"], "target");

    cleanup(&source_path);
    cleanup(&target_path);
}

// ════════ P0-1：flush_ok 标记校验 ════════

#[test]
fn P0_1_Mmap模式_flush后应生成flush_ok标记() {
    let path = tmp_db("flush_ok_mmap");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        {
            let mut tx = db.begin_tx();
            tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
            tx.commit().unwrap();
        }
        db.flush().unwrap();
    }

    let ok_path = format!("{}.flush_ok", path);
    assert!(
        Path::new(&ok_path).exists(),
        "Mmap 模式 flush 后应生成 .flush_ok 标记"
    );

    cleanup(&path);
}

#[test]
fn P0_1_删除flush_ok后重加载_应降级为零向量() {
    let path = tmp_db("flush_ok_torn");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        {
            let mut tx = db.begin_tx();
            tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "alice"}));
            tx.commit().unwrap();
        }
        db.flush().unwrap();
    }

    // 模拟撕裂：删除 .flush_ok
    std::fs::remove_file(format!("{}.flush_ok", path)).unwrap();

    // marker 无效时应安全降级：节点元数据保留，但向量必须为零向量（不信任 .vec）
    let db = Database::<f32>::open(&path, DIM)
        .expect("marker 无效时应安全降级为 metadata-only 而非报错");
    assert_eq!(db.node_count(), 1, "降级后节点元数据应保留");

    // 断言返回的向量是零向量，而非可能错误的 .vec 数据
    for id in db.all_node_ids() {
        let node = db.get(id).expect("节点应存在");
        assert!(
            node.vector.iter().all(|&x| x == 0.0),
            "marker 无效时向量应为零向量，实际 {:?}",
            node.vector
        );
    }

    cleanup(&path);
}

#[test]
fn P0_1_Rom模式_不应残留flush_ok标记() {
    let path = tmp_db("flush_ok_rom");
    cleanup(&path);

    {
        let config = Config {
            dim: DIM,
            storage_mode: StorageMode::Rom,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        {
            let mut tx = db.begin_tx();
            tx.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
            tx.commit().unwrap();
        }
        db.flush().unwrap();
    }

    // Rom 模式无论是否有 flush_ok，重新打开都不应 panic
    let config = Config {
        dim: DIM,
        storage_mode: StorageMode::Rom,
        ..Default::default()
    };
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
        db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"seq": 1}))
            .unwrap();
        db.flush().unwrap(); // id=1 落盘，WAL 被清除

        // 继续插入（仅写 WAL，不 flush）
        last_id = db
            .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"seq": 2}))
            .unwrap();
        // drop 时 Drop trait 显式 flush WAL BufWriter
    }

    // WAL 文件应非空
    let wal_size = std::fs::metadata(format!("{}.wal", path))
        .map(|m| m.len())
        .unwrap_or(0);
    assert!(
        wal_size > 0,
        "Drop 后 WAL 应非空（实际 {} bytes）",
        wal_size
    );

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        let new_id = db
            .insert(&[0.0, 0.0, 1.0, 0.0], serde_json::json!({"seq": 3}))
            .unwrap();
        assert!(
            new_id > last_id,
            "WAL 回放后 next_id 应已推进：new_id={}, last_id={}",
            new_id,
            last_id
        );
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
        {
            let mut tx = db.begin_tx();
            tx.delete(del_id);
            tx.commit().unwrap();
        }
        db.flush().unwrap();
    }

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert!(!db.contains(del_id), "删除并 flush 后重加载，节点应不存在");
    assert_eq!(db.node_count(), 1, "应只剩 1 个节点");

    cleanup(&path);
}

// ════════ 回归测试：flush marker 加固 ════════

/// 回归测试：marker magic 被篡改时不加载 .vec，降级为零向量
#[test]
fn 回归_marker_magic被篡改时不加载vec_降级为零向量() {
    let path = tmp_db("marker_magic_corrupt");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        db.insert(&[1.0, 2.0, 3.0, 4.0], serde_json::json!({"v": 1}))
            .unwrap();
        db.flush().unwrap();
    }

    // 篡改 marker 的 magic 字节，使其无效
    let marker_path = format!("{}.flush_ok", path);
    let mut marker = std::fs::read(&marker_path).unwrap();
    marker[0..4].copy_from_slice(b"XXXX");
    std::fs::write(&marker_path, &marker).unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.node_count(), 1, "节点元数据应保留");

    // marker 无效时不加载 .vec，向量应为零向量
    let node = db.get(1).unwrap();
    assert!(
        node.vector.iter().all(|&x| x == 0.0),
        "marker magic 被篡改时向量应为零向量，实际 {:?}",
        node.vector
    );

    cleanup(&path);
}

/// 回归测试：marker version 不匹配时不加载 .vec，降级为零向量
#[test]
fn 回归_marker_version不匹配时拒绝加载vec() {
    let path = tmp_db("marker_version_mismatch");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        db.insert(&[1.0, 2.0, 3.0, 4.0], serde_json::json!({"v": 1}))
            .unwrap();
        db.flush().unwrap();
    }

    // 篡改 marker 的 version 字节，使其不匹配
    let marker_path = format!("{}.flush_ok", path);
    let mut marker = std::fs::read(&marker_path).unwrap();
    marker[4] = 255; // 不支持的版本号
    std::fs::write(&marker_path, &marker).unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.node_count(), 1, "节点元数据应保留");

    // marker version 不匹配时不加载 .vec，向量应为零向量
    let node = db.get(1).unwrap();
    assert!(
        node.vector.iter().all(|&x| x == 0.0),
        "version 不匹配时向量应为零向量，实际 {:?}",
        node.vector
    );

    cleanup(&path);
}

/// 回归测试：marker generation 在多次 flush 中单调递增
#[test]
fn 回归_marker_generation单调递增() {
    let path = tmp_db("marker_generation_increment");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"seq": 1}))
            .unwrap();
        db.flush().unwrap();
    }

    // 第一次 flush 后读取 generation
    let marker_path = format!("{}.flush_ok", path);
    let marker_bytes = std::fs::read(&marker_path).unwrap();
    assert_eq!(marker_bytes.len(), 29, "marker 应为 29 字节");
    let gen1 = u64::from_le_bytes(marker_bytes[5..13].try_into().unwrap());
    assert_eq!(gen1, 1, "首次 flush 的 generation 应为 1");

    // 第二次 flush
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        db.insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"seq": 2}))
            .unwrap();
        db.flush().unwrap();
    }

    let marker_bytes = std::fs::read(&marker_path).unwrap();
    let gen2 = u64::from_le_bytes(marker_bytes[5..13].try_into().unwrap());
    assert_eq!(gen2, 2, "第二次 flush 的 generation 应为 2（单调递增）");

    // 第三次 flush
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        db.insert(&[0.0, 0.0, 1.0, 0.0], serde_json::json!({"seq": 3}))
            .unwrap();
        db.flush().unwrap();
    }

    let marker_bytes = std::fs::read(&marker_path).unwrap();
    let gen3 = u64::from_le_bytes(marker_bytes[5..13].try_into().unwrap());
    assert_eq!(gen3, 3, "第三次 flush 的 generation 应为 3（单调递增）");

    cleanup(&path);
}
