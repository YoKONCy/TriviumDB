#![allow(non_snake_case)]
//! GJB-5000B 快速断电循环测试
//!
//! 模拟军舰电源不稳定环境下的频繁断电重启，验证 TriviumDB 的
//! WAL + flush_ok 原子提交机制在反复中断下的数据完整性。

use std::process::Command;
use triviumdb::database::Database;

#[cfg(feature = "test-hooks")]
use triviumdb::test_hooks::{ConcurrencyPoint, pause_at};

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("pwr_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok", ".tmp", ".vec.tmp"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

#[cfg(feature = "test-hooks")]
fn publication_point(name: &str) -> ConcurrencyPoint {
    match name {
        "after_vec" => ConcurrencyPoint::AfterVecPersisted,
        "after_tdb" => ConcurrencyPoint::AfterTdbPersisted,
        "before_marker" => ConcurrencyPoint::BeforeFlushMarkerRename,
        "after_marker" => ConcurrencyPoint::AfterFlushMarkerRename,
        other => panic!("未知发布阶段: {other}"),
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn __publication_power_loss_child_entry() {
    let Ok(path) = std::env::var("TRIVIUM_PUBLICATION_CHILD_PATH") else {
        return;
    };
    let phase = std::env::var("TRIVIUM_PUBLICATION_CHILD_PHASE").unwrap();
    let ready = std::env::var("TRIVIUM_PUBLICATION_CHILD_READY").unwrap();
    let waiter = pause_at(publication_point(&phase));
    let mut database = Database::<f32>::open(&path, DIM).unwrap();
    for id in 5..=7u64 {
        database
            .insert_with_id(
                id,
                &[id as f32, 1.0, 2.0, 3.0],
                serde_json::json!({"generation": "new", "id": id}),
            )
            .unwrap();
    }
    let worker = std::thread::spawn(move || database.flush().unwrap());
    waiter.wait_until_arrived();
    std::fs::write(&ready, phase.as_bytes()).unwrap();
    std::mem::forget(waiter);
    std::mem::forget(worker);
    loop {
        std::thread::park();
    }
}

#[cfg(feature = "test-hooks")]
fn assert_exact_generation_or_fail_closed(path: &str) {
    let Ok(database) = Database::<f32>::open(path, DIM) else {
        return;
    };
    let count = database.node_count();
    assert!(
        count == 4 || count == 7,
        "只能恢复完整旧代或完整新代，实际节点数={count}"
    );
    for id in 1..=count as u64 {
        let node = database
            .get(id)
            .unwrap_or_else(|| panic!("恢复状态缺少节点 {id}"));
        assert_eq!(
            node.vector,
            vec![id as f32, 1.0, 2.0, 3.0],
            "节点 {id} 的向量来自混合代际"
        );
        let payload = database.get_payload(id).unwrap();
        assert_eq!(payload["id"], id);
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn PWR_00A_发布阶段真实强杀只允许完整旧代_完整新代或fail_closed() {
    for phase in ["after_vec", "after_tdb", "before_marker", "after_marker"] {
        let path = tmp_db(&format!("publication_{phase}"));
        cleanup(&path);
        {
            let mut database = Database::<f32>::open(&path, DIM).unwrap();
            for id in 1..=4u64 {
                database
                    .insert_with_id(
                        id,
                        &[id as f32, 1.0, 2.0, 3.0],
                        serde_json::json!({"generation": "old", "id": id}),
                    )
                    .unwrap();
            }
            database.flush().unwrap();
        }

        let ready = format!("{path}.{phase}.ready");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .env("TRIVIUM_PUBLICATION_CHILD_PATH", &path)
            .env("TRIVIUM_PUBLICATION_CHILD_PHASE", phase)
            .env("TRIVIUM_PUBLICATION_CHILD_READY", &ready)
            .arg("__publication_power_loss_child_entry")
            .arg("--exact")
            .arg("--nocapture")
            .spawn()
            .unwrap();
        for _ in 0..400 {
            if std::path::Path::new(&ready).exists() {
                break;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "发布阶段子进程提前退出: {phase}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            std::path::Path::new(&ready).exists(),
            "子进程未到达发布阶段: {phase}"
        );
        child.kill().unwrap();
        child.wait().unwrap();
        assert_exact_generation_or_fail_closed(&path);
        std::fs::remove_file(&ready).ok();
        cleanup(&path);
    }
}

#[test]
fn __power_loss_child_entry() {
    let Ok(path) = std::env::var("TRIVIUM_POWER_LOSS_CHILD") else {
        return;
    };
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for index in 0..8u32 {
        db.insert(
            &[index as f32, 1.0, 0.0, 0.0],
            serde_json::json!({"phase": "wal", "index": index}),
        )
        .unwrap();
    }
    // 阻止析构；父进程会立即强制终止这个小型专用子进程。
    std::mem::forget(db);
    std::fs::write(format!("{path}.ready"), b"ready").unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn PWR_00_真实强杀子进程后WAL保持可恢复() {
    let path = tmp_db("real_process_kill");
    cleanup(&path);
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .env("TRIVIUM_POWER_LOSS_CHILD", &path)
        .arg("__power_loss_child_entry")
        .arg("--exact")
        .arg("--nocapture")
        .spawn()
        .unwrap();
    let ready = format!("{path}.ready");
    for _ in 0..200 {
        if std::path::Path::new(&ready).exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        std::path::Path::new(&ready).exists(),
        "子进程未到达故障注入点"
    );
    child.kill().unwrap();
    child.wait().unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.node_count(), 8, "强制终止后必须回放完整 WAL 帧");
    std::fs::remove_file(ready).ok();
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  1. 快速断电循环 — open/write/drop 循环
// ════════════════════════════════════════════════════════════════

/// 100 轮快速循环：open → 随机操作 → 强制 drop（模拟断电） → reopen
/// 验证每轮 reopen 后引擎状态一致，不 panic
#[test]
fn PWR_01_快速断电循环_100轮() {
    let path = tmp_db("rapid_cycle");
    cleanup(&path);

    let mut max_seen = 0usize;

    for round in 0..100u32 {
        // 打开
        let mut db = match Database::<f32>::open(&path, DIM) {
            Ok(db) => db,
            Err(e) => {
                panic!(
                    "第 {} 轮: 引擎无法打开: {} (max_seen={})",
                    round, e, max_seen
                );
            }
        };

        let count = db.node_count();
        assert!(
            count >= max_seen.saturating_sub(1), // 允许丢失 WAL 中未 flush 的数据
            "第 {} 轮: 节点数 {} 低于历史最高 {} 超过 1 个（数据意外丢失）",
            round,
            count,
            max_seen
        );

        // 每 10 轮 flush 一次
        if round % 10 == 0 {
            // 先 insert 再 flush
            for j in 0..5u32 {
                db.insert(
                    &[round as f32, j as f32, 0.0, 0.0],
                    serde_json::json!({"r": round, "j": j}),
                )
                .unwrap();
            }
            db.flush().unwrap();
        } else {
            // 不 flush，只写 WAL
            for j in 0..3u32 {
                db.insert(
                    &[round as f32, j as f32, 0.0, 0.0],
                    serde_json::json!({"r": round, "j": j}),
                )
                .unwrap();
            }
        }

        let new_count = db.node_count();
        if new_count > max_seen {
            max_seen = new_count;
        }

        // 强制 drop（模拟断电，不 flush）
        drop(db);
    }

    // 最终打开验证
    let db = Database::<f32>::open(&path, DIM).unwrap();
    eprintln!(
        "  ✅ 100 轮断电循环: 最终 {} 个节点 (历史最高 {})",
        db.node_count(),
        max_seen
    );
    assert!(db.node_count() > 0, "100 轮后应至少有一些数据存活");

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  2. flush 中途断电模拟 — .tmp 文件残留
// ════════════════════════════════════════════════════════════════

/// 模拟 flush 中途断电的场景：
/// 手工创建 .tmp 文件（模拟原子 rename 前被杀），
/// 验证引擎下次启动时忽略 .tmp 并从旧 .tdb 正确加载
#[test]
fn PWR_02_flush中途断电_tmp残留_原子性验证() {
    let path = tmp_db("flush_interrupt");
    cleanup(&path);

    // 正常创建和 flush
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..50u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"idx": i}))
                .unwrap();
        }
        db.flush().unwrap();
    }

    // 模拟 flush 中途断电：创建 .tmp 文件（一个不完整的新版本）
    let tmp_path = format!("{}.tmp", path);
    let corrupt_content = b"TVDB\x05\x00INCOMPLETE_FLUSH_DATA";
    std::fs::write(&tmp_path, corrupt_content).unwrap();

    // 同时模拟 .vec.tmp
    let vec_tmp_path = format!("{}.vec.tmp", path);
    std::fs::write(&vec_tmp_path, b"CORRUPT_VEC").unwrap();

    // 重新打开 — 应忽略 .tmp，从旧 .tdb 加载
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        db.node_count(),
        50,
        "flush 中途断电后应从旧 .tdb 恢复完整 50 个节点"
    );

    eprintln!(
        "  ✅ flush 中途断电: 从旧 .tdb 恢复 {} 个节点",
        db.node_count()
    );

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  3. 删除+断电循环 — 验证 tombstone 持久性
// ════════════════════════════════════════════════════════════════

/// 插入 → 删除 → flush → 断电 → reopen 循环
/// 验证 tombstone 在断电循环中正确持久化
#[test]
fn PWR_03_删除后断电_tombstone持久化() {
    let path = tmp_db("delete_cycle");
    cleanup(&path);

    // 插入 100 个节点
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..100u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
                .unwrap();
        }
        db.flush().unwrap();
    }

    // 删除前 50 个 + flush + 断电
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        let ids: Vec<u64> = db.all_node_ids().iter().take(50).copied().collect();
        for id in &ids {
            db.delete(*id).unwrap();
        }
        db.flush().unwrap();
        // 断电 (drop)
    }

    // 重新打开 — 验证删除持久化
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        db.node_count(),
        50,
        "删除 50 个节点后 flush + 断电重启，应剩余 50 个"
    );

    eprintln!(
        "  ✅ 删除 + 断电: 剩余 {} 个节点，tombstone 正确持久化",
        db.node_count()
    );

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  4. 事务提交 + 断电 — WAL 原子性
// ════════════════════════════════════════════════════════════════

/// 事务 commit 后立即断电（不 flush），验证 WAL 回放的完整性
#[test]
fn PWR_04_事务commit后断电_WAL原子回放() {
    let path = tmp_db("tx_crash");
    cleanup(&path);

    // 事务提交但不 flush
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        let mut tx = db.begin_tx();
        for i in 0..20u32 {
            tx.insert(
                &[i as f32, 0.0, 0.0, 0.0],
                serde_json::json!({"tx_item": i}),
            );
        }
        tx.commit().unwrap();
        // 不 flush，直接断电
    }

    // 重新打开 — WAL 应回放事务中的 20 条记录
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        db.node_count(),
        20,
        "事务 commit 后断电，WAL 回放应恢复 20 个节点"
    );

    // 验证 payload 完整性
    for &id in &db.all_node_ids() {
        let payload = db.get_payload(id).unwrap();
        assert!(
            payload.get("tx_item").is_some(),
            "节点 {} 的 payload 应包含 tx_item 字段",
            id
        );
    }

    eprintln!(
        "  ✅ 事务 + 断电: WAL 回放恢复 {} 个节点，payload 完整",
        db.node_count()
    );

    cleanup(&path);
}
