#![allow(non_snake_case)]
//! GJB-5000B IO 故障容错测试
//!
//! 验证 TriviumDB 在 IO 异常（文件损坏、磁盘满、权限不足）下的容错能力。

use triviumdb::database::{Config, Database};
use triviumdb::storage::wal::SyncMode;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("io_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &[
        "",
        ".wal",
        ".vec",
        ".lock",
        ".flush_ok",
        ".tmp",
        ".vec.tmp",
        ".flush_ok.tmp",
    ] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

fn seed_flushed_db(path: &str, count: u32) {
    let mut db = Database::<f32>::open(path, DIM).unwrap();
    for i in 0..count {
        db.insert(
            &[i as f32 + 1.0, i as f32 + 2.0, i as f32 + 3.0, 1.0],
            serde_json::json!({"idx": i, "phase": "flushed"}),
        )
        .unwrap();
    }
    db.flush().unwrap();
}

fn assert_flushed_payloads(db: &Database<f32>, count: u32) {
    assert_eq!(db.node_count(), count as usize, "已 flush 节点数量必须完整");
    for id in db.all_node_ids() {
        let node = db.get(id).expect("恢复节点必须能完整读取");
        let idx = node
            .payload
            .get("idx")
            .and_then(|value| value.as_u64())
            .expect("恢复节点必须携带 idx");
        assert!(idx < count as u64, "恢复节点 idx 不能越界: {idx}");
        assert_eq!(
            node.payload.get("phase").and_then(|value| value.as_str()),
            Some("flushed"),
            "恢复节点 payload.phase 必须保持原值"
        );
    }
}

fn open_with_sync(path: &str, mode: SyncMode) -> Database<f32> {
    Database::<f32>::open_with_config(
        path,
        Config {
            dim: DIM,
            sync_mode: mode,
            ..Default::default()
        },
    )
    .unwrap()
}

// ════════════════════════════════════════════════════════════════
//  1. WAL 文件被外部清空
// ════════════════════════════════════════════════════════════════

/// WAL 文件被外部清空为 0 字节后，引擎应能安全启动
#[test]
fn IO_01_WAL文件被清空_安全启动() {
    let path = tmp_db("wal_zeroed");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..50u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
                .unwrap();
        }
        db.flush().unwrap();
    }

    // 清空 WAL 文件
    let wal_path = format!("{}.wal", path);
    std::fs::write(&wal_path, b"").unwrap();

    // 重新打开
    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "WAL 被清空后不应 panic");
    let db = result.unwrap().unwrap();

    // flush 过的数据应该还在
    assert_eq!(db.node_count(), 50, "flush 过的数据不受 WAL 清空影响");

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  2. WAL 文件包含随机垃圾数据
// ════════════════════════════════════════════════════════════════

#[test]
fn IO_02_WAL文件注入随机垃圾_安全恢复() {
    let path = tmp_db("wal_garbage");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..30u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
                .unwrap();
        }
        db.flush().unwrap();
    }

    // 往 WAL 里写入随机垃圾
    let wal_path = format!("{}.wal", path);
    let garbage: Vec<u8> = (0..1024).map(|i| (i * 137 + 42) as u8).collect();
    std::fs::write(&wal_path, &garbage).unwrap();

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "WAL 垃圾数据不应导致 panic");
    let db = result.unwrap().unwrap();
    assert_eq!(db.node_count(), 30, "flush 过的数据应完整");

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  3. .tdb 文件全部用随机数据覆写
// ════════════════════════════════════════════════════════════════

#[test]
fn IO_03_TDB文件全覆写随机数据_安全拒绝() {
    let path = tmp_db("tdb_random");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
        db.flush().unwrap();
    }

    // 用随机数据完全覆写 .tdb
    let random_data: Vec<u8> = (0..4096).map(|i| (i * 31 + 7) as u8).collect();
    std::fs::write(&path, &random_data).unwrap();
    std::fs::remove_file(format!("{}.flush_ok", path)).ok();

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "随机覆写的 .tdb 不应导致 panic");
    let opened = result.unwrap();
    assert!(opened.is_err(), "随机覆写的 .tdb 应被拒绝加载");
    let err = opened.err().unwrap();
    assert!(
        !err.to_string().is_empty(),
        "拒绝随机 .tdb 必须返回可诊断错误"
    );

    cleanup(&path);
    let clean = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        clean.node_count(),
        0,
        "拒绝损坏库后同路径重建不能携带脏状态"
    );

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  4. .vec 和 .tdb 大小不匹配（跨文件不一致）
// ════════════════════════════════════════════════════════════════

#[test]
fn IO_04_vec与tdb大小不匹配_降级加载() {
    let path = tmp_db("size_mismatch");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..100u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
                .unwrap();
        }
        db.flush().unwrap();
    }

    // 篡改 .flush_ok 中的大小校验值
    let marker_path = format!("{}.flush_ok", path);
    let bad_marker = [0u8; 16]; // 全零 = 大小不匹配
    std::fs::write(&marker_path, bad_marker).unwrap();

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "大小不匹配不应 panic");
    match result.unwrap() {
        Ok(mut db) => {
            assert_eq!(db.node_count(), 100, "降级加载必须保留 flush 完整节点");
            for id in db.all_node_ids() {
                assert!(
                    db.get_payload(id).is_some(),
                    "降级恢复出的节点必须可读取 payload"
                );
            }
            let id = db
                .insert(&[200.0, 0.0, 0.0, 0.0], serde_json::json!({"idx": 200}))
                .unwrap();
            assert!(id > 0, "降级恢复后应仍可安全写入");
            assert_eq!(db.node_count(), 101, "新写入不能覆盖已恢复节点");
        }
        Err(e) => {
            assert!(!e.to_string().is_empty(), "安全拒绝必须返回可诊断错误");
            cleanup(&path);
            let clean = Database::<f32>::open(&path, DIM).unwrap();
            assert_eq!(
                clean.node_count(),
                0,
                "拒绝不一致库后同路径重建不能携带脏状态"
            );
        }
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  5. 残留的 .tmp 文件不影响启动
// ════════════════════════════════════════════════════════════════

#[test]
fn IO_05_残留tmp文件_不影响正常启动() {
    let path = tmp_db("leftover_tmp");
    cleanup(&path);

    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..20u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
                .unwrap();
        }
        db.flush().unwrap();
    }

    // 制造残留 .tmp 文件（模拟 flush 中途崩溃）
    let tmp_path = format!("{}.tmp", path);
    std::fs::write(&tmp_path, b"corrupted partial write").unwrap();
    let vec_tmp_path = format!("{}.vec.tmp", path);
    std::fs::write(&vec_tmp_path, b"corrupted vec partial write").unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.node_count(), 20, "残留 .tmp 文件不应影响正常加载");

    cleanup(&path);
}

#[test]
fn IO_05B_tdb_tmp半截矩阵_必须忽略并保留旧快照() {
    let path = tmp_db("tdb_tmp_partial_matrix");
    cleanup(&path);
    seed_flushed_db(&path, 32);

    let mut replacement = std::fs::read(&path).unwrap();
    replacement.extend_from_slice(b"new generation bytes that must never be committed");
    let cut_points = [
        0usize,
        1,
        2,
        3,
        4,
        16,
        replacement.len() / 2,
        replacement.len() - 1,
    ];

    for cut in cut_points {
        std::fs::write(format!("{}.tmp", path), &replacement[..cut]).unwrap();
        let db = Database::<f32>::open(&path, DIM).unwrap();
        assert_flushed_payloads(&db, 32);
        drop(db);
        std::fs::remove_file(format!("{}.lock", path)).ok();
    }

    cleanup(&path);
}

#[test]
fn IO_05C_vec_tmp半截矩阵_必须忽略并保留旧向量文件() {
    let path = tmp_db("vec_tmp_partial_matrix");
    cleanup(&path);
    seed_flushed_db(&path, 24);

    let vec_path = format!("{}.vec", path);
    let mut replacement = std::fs::read(&vec_path).unwrap();
    replacement.extend_from_slice(&[0xA5; 128]);
    let cut_points = [
        0usize,
        1,
        3,
        7,
        16,
        replacement.len() / 2,
        replacement.len() - 1,
    ];

    for cut in cut_points {
        std::fs::write(format!("{}.vec.tmp", path), &replacement[..cut]).unwrap();
        let db = Database::<f32>::open(&path, DIM).unwrap();
        assert_flushed_payloads(&db, 24);
        let hits = db.search(&[1.0, 2.0, 3.0, 1.0], 1, 0, 0.0).unwrap();
        assert_eq!(hits.len(), 1, "旧 .vec 必须仍可参与搜索");
        drop(db);
        std::fs::remove_file(format!("{}.lock", path)).ok();
    }

    cleanup(&path);
}

#[test]
fn IO_05D_vec追加半截_flush_ok不匹配_不得产生脏节点() {
    let path = tmp_db("vec_append_partial_matrix");
    cleanup(&path);
    seed_flushed_db(&path, 16);

    let vec_path = format!("{}.vec", path);
    let base_vec = std::fs::read(&vec_path).unwrap();
    let extra_vector_bytes: Vec<u8> = [17.0f32, 18.0, 19.0, 1.0]
        .into_iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();

    for cut in 1..extra_vector_bytes.len() {
        let mut torn_vec = base_vec.clone();
        torn_vec.extend_from_slice(&extra_vector_bytes[..cut]);
        std::fs::write(&vec_path, torn_vec).unwrap();

        let db = Database::<f32>::open(&path, DIM).unwrap();
        assert_flushed_payloads(&db, 16);
        let ids = db.all_node_ids();
        assert!(
            ids.iter().all(|id| *id <= 16),
            "半截追加的 .vec 不能制造新节点: {ids:?}"
        );
        drop(db);
        std::fs::remove_file(format!("{}.lock", path)).ok();
    }

    std::fs::write(&vec_path, base_vec).unwrap();
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  6. 并发多进程锁竞争
// ════════════════════════════════════════════════════════════════

#[test]
fn IO_06_文件锁_同一路径不能打开两次() {
    let path = tmp_db("double_open");
    cleanup(&path);

    let _db1 = Database::<f32>::open(&path, DIM).unwrap();

    // 尝试第二次打开
    let result = Database::<f32>::open(&path, DIM);
    match result {
        Ok(_) => panic!("同一路径不应允许两个实例同时打开"),
        Err(e) => {
            let err_msg = e.to_string();
            assert!(
                err_msg.contains("already opened") || err_msg.contains("locked"),
                "错误信息应提示文件已锁定: {}",
                err_msg
            );
        }
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  7. WAL 写入后不 flush 直接退出 → 重启恢复
// ════════════════════════════════════════════════════════════════

#[test]
fn IO_07_纯WAL恢复_不flush直接退出() {
    let path = tmp_db("wal_only");
    cleanup(&path);

    // 第一轮：只写 WAL，不 flush
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        let mut tx = db.begin_tx();
        for i in 0..100u32 {
            tx.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"idx": i}));
        }
        tx.commit().unwrap();
        // 故意不 flush，直接 drop
    }

    // 重新打开
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.node_count(), 100, "纯 WAL 恢复应找回所有 100 个节点");

    cleanup(&path);
}

#[test]
fn IO_08_SyncMode_Normal和Full_drop后必须恢复完整WAL() {
    for mode in [SyncMode::Normal, SyncMode::Full] {
        let path = tmp_db(&format!("sync_recover_{mode:?}"));
        cleanup(&path);

        {
            let mut db = open_with_sync(&path, mode);
            for i in 0..12u32 {
                db.insert(
                    &[i as f32, 0.0, 0.0, 0.0],
                    serde_json::json!({"idx": i, "mode": format!("{mode:?}")}),
                )
                .unwrap();
            }
        }

        let db = Database::<f32>::open(&path, DIM).unwrap();
        assert_eq!(
            db.node_count(),
            12,
            "{mode:?} 在正常 drop 后必须能完整恢复已写 WAL"
        );
        for id in db.all_node_ids() {
            let payload = db.get_payload(id).expect("恢复节点必须有 payload");
            assert_eq!(
                payload.get("mode").and_then(|value| value.as_str()),
                Some(format!("{mode:?}").as_str()),
                "恢复节点必须来自对应 SyncMode 写入"
            );
        }

        cleanup(&path);
    }
}

#[test]
fn IO_09_SyncMode_Off未flush_writer时允许WAL不可见() {
    let path = tmp_db("sync_off_unflushed");
    cleanup(&path);

    {
        let mut db = open_with_sync(&path, SyncMode::Off);
        for i in 0..8u32 {
            db.insert(
                &[i as f32, 0.0, 0.0, 0.0],
                serde_json::json!({"idx": i, "mode": "Off"}),
            )
            .unwrap();
        }
        let wal_size = std::fs::metadata(format!("{}.wal", path))
            .map(|meta| meta.len())
            .unwrap_or(0);
        assert_eq!(
            wal_size, 6,
            "SyncMode::Off 在 writer 未 drop/flush 前只应写入固定 WAL 版本头"
        );
    }

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        db.node_count(),
        8,
        "SyncMode::Off 在正常 drop 刷出 BufWriter 后仍应可恢复"
    );

    cleanup(&path);
}

#[test]
fn IO_10_SyncMode_Normal写入后WAL应立即可见_Full至少同等可见() {
    for mode in [SyncMode::Normal, SyncMode::Full] {
        let path = tmp_db(&format!("sync_visible_{mode:?}"));
        cleanup(&path);

        let mut db = open_with_sync(&path, mode);
        db.insert(
            &[1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"idx": 1, "mode": format!("{mode:?}")}),
        )
        .unwrap();
        let wal_size = std::fs::metadata(format!("{}.wal", path))
            .map(|meta| meta.len())
            .unwrap_or(0);
        assert!(
            wal_size > 0,
            "{mode:?} 每次 append 后必须至少 flush 到可见 WAL 文件"
        );
        drop(db);

        cleanup(&path);
    }
}
