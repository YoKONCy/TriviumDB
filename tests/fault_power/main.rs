#![allow(non_snake_case)]
//! GJB-5000B 快速断电循环测试
//!
//! 模拟军舰电源不稳定环境下的频繁断电重启，验证 TriviumDB 的
//! WAL + flush_ok 原子提交机制在反复中断下的数据完整性。

use std::process::Command;
use triviumdb::database::Database;

#[cfg(feature = "test-hooks")]
mod text_publication;

#[cfg(feature = "test-hooks")]
mod migration_publication;
#[cfg(feature = "test-hooks")]
#[path = "../support/mod.rs"]
mod support;

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

fn child_command() -> Command {
    let executable = std::env::current_exe().unwrap();
    if std::env::var_os("TRIVIUM_TEST_QEMU_AARCH64").is_some() {
        let mut command = Command::new("qemu-aarch64");
        command
            .arg("-L")
            .arg("/usr/aarch64-linux-gnu")
            .arg(executable);
        command
    } else {
        Command::new(executable)
    }
}

#[cfg(feature = "test-hooks")]
fn publication_point(name: &str) -> ConcurrencyPoint {
    match name {
        "after_backup" => ConcurrencyPoint::AfterPublicationPrepared,
        "after_vec" => ConcurrencyPoint::AfterVecPersisted,
        "after_tdb" => ConcurrencyPoint::AfterTdbPersisted,
        "before_marker" => ConcurrencyPoint::BeforeFlushMarkerRename,
        "after_marker" => ConcurrencyPoint::AfterFlushMarkerRename,
        "during_restore" => ConcurrencyPoint::AfterPublicationRestore,
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
    let case = std::env::var("TRIVIUM_PUBLICATION_CHILD_CASE").unwrap();
    let (dim, base) = publication_shape(&case);
    let waiter = pause_at(publication_point(&phase));
    let worker = std::thread::spawn(move || {
        let mut database = Database::<f32>::open_with_config(
            &path,
            triviumdb::database::Config {
                dim,
                sync_mode: triviumdb::storage::wal::SyncMode::Full,
                ..Default::default()
            },
        )
        .unwrap();
        if phase == "during_restore" {
            panic!("恢复断点未阻塞可写打开");
        }
        for id in base + 1..=base + 3 {
            database
                .insert_with_id(
                    id,
                    &publication_vector(id, dim),
                    serde_json::json!({"generation": "new", "id": id}),
                )
                .unwrap();
        }
        if case == "rewrite" {
            database.update_vector(1, &vec![-1.0; dim]).unwrap();
            database
                .update_payload(1, serde_json::json!({"generation": "new", "id": 1}))
                .unwrap();
            database.delete(2).unwrap();
            database.link(1, 3, "更新后的边", 2.0).unwrap();
            database.compact().unwrap();
        } else {
            database.flush().unwrap();
        }
    });
    waiter.wait_until_arrived();
    std::fs::write(&ready, "已到达断点".as_bytes()).unwrap();
    std::mem::forget(waiter);
    std::mem::forget(worker);
    loop {
        std::thread::park();
    }
}

#[cfg(feature = "test-hooks")]
fn publication_shape(case: &str) -> (usize, u64) {
    match case {
        "first" => (DIM, 0),
        "append" => (DIM, 4),
        "rewrite" => (65, 2048),
        _ => panic!("未知发布场景: {case}"),
    }
}

#[cfg(feature = "test-hooks")]
fn publication_vector(id: u64, dim: usize) -> Vec<f32> {
    (0..dim)
        .map(|axis| id as f32 + axis as f32 * 0.125)
        .collect()
}

#[cfg(feature = "test-hooks")]
fn assert_exact_recovered_data(database: &Database<f32>, case: &str) {
    let (dim, base) = publication_shape(case);
    let expected: Vec<u64> = (1..=base + 3)
        .filter(|id| case != "rewrite" || *id != 2)
        .collect();
    let mut actual = database.all_node_ids().unwrap();
    actual.sort_unstable();
    assert_eq!(
        actual, expected,
        "必须恢复所有已同步事务，不能用空库或旧代代替"
    );
    for id in expected {
        let node = database
            .get(id)
            .unwrap()
            .unwrap_or_else(|| panic!("恢复状态缺少节点 {id}"));
        assert_eq!(
            node.vector,
            if case == "rewrite" && id == 1 {
                vec![-1.0; dim]
            } else {
                publication_vector(id, dim)
            },
            "节点 {id} 的向量来自混合代际"
        );
        let payload = database.get_payload(id).unwrap().unwrap();
        assert_eq!(payload["id"], id);
        assert_eq!(
            payload["generation"],
            if id > base || (case == "rewrite" && id == 1) {
                "new"
            } else {
                "old"
            }
        );
    }
    if case == "rewrite" {
        let edges = database.get_edges(1).unwrap();
        assert!(
            edges.iter().any(|edge| edge.target_id == 3
                && edge.label == "更新后的边"
                && edge.weight == 2.0)
        );
    }
}

#[cfg(feature = "test-hooks")]
#[test]
fn PWR_00A_发布阶段强杀必须恢复全部已同步事务() {
    for case in ["first", "append", "rewrite"] {
        for phase in [
            "after_backup",
            "after_vec",
            "after_tdb",
            "before_marker",
            "after_marker",
            "during_restore",
        ] {
            let path = tmp_db(&format!(
                "publication_{case}_{phase}_{}",
                std::process::id()
            ));
            let (dim, base) = publication_shape(case);
            cleanup(&path);
            if base > 0 {
                let mut database = Database::<f32>::open(&path, dim).unwrap();
                for id in 1..=base {
                    database
                        .insert_with_id(
                            id,
                            &publication_vector(id, dim),
                            serde_json::json!({"generation": "old", "id": id}),
                        )
                        .unwrap();
                }
                database.flush().unwrap();
            }

            kill_publication_child(
                &path,
                if phase == "during_restore" {
                    "after_tdb"
                } else {
                    phase
                },
                case,
            );
            if phase == "during_restore" {
                kill_publication_child(&path, phase, case);
            }
            let before = publication_files(&path);
            assert!(
                Database::<f32>::open_read_only(&path, dim).is_err(),
                "待恢复的 WAL/发布日志必须阻止只读打开"
            );
            assert!(Database::<f32>::open_immutable(&path, dim).is_err());
            assert_eq!(
                publication_files(&path),
                before,
                "只读与不可变打开失败不得改变任何文件"
            );

            let mut database = Database::<f32>::open(&path, dim).unwrap();
            assert_exact_recovered_data(&database, case);
            database.flush().unwrap();
            drop(database);
            let reader = Database::<f32>::open_read_only(&path, dim).unwrap();
            assert_exact_recovered_data(&reader, case);
            drop(reader);
            cleanup(&path);
        }
    }
}

#[cfg(feature = "test-hooks")]
fn kill_publication_child(path: &str, phase: &str, case: &str) {
    let ready = format!("{path}.{phase}.ready");
    std::fs::remove_file(&ready).ok();
    let mut child = child_command()
        .env("TRIVIUM_PUBLICATION_CHILD_PATH", path)
        .env("TRIVIUM_PUBLICATION_CHILD_PHASE", phase)
        .env("TRIVIUM_PUBLICATION_CHILD_READY", &ready)
        .env("TRIVIUM_PUBLICATION_CHILD_CASE", case)
        .arg("__publication_power_loss_child_entry")
        .arg("--exact")
        .arg("--nocapture")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !std::path::Path::new(&ready).exists() && std::time::Instant::now() < deadline {
        assert!(
            child.try_wait().unwrap().is_none(),
            "发布阶段子进程提前退出: {phase}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // 无论是否到达断点都回收子进程，避免测试失败后残留锁或后台线程。
    let arrived = std::path::Path::new(&ready).exists();
    child.kill().ok();
    child.wait().unwrap();
    assert!(arrived, "子进程未到达发布阶段: {phase}");
    std::fs::remove_file(ready).ok();
}

#[cfg(feature = "test-hooks")]
fn publication_files(path: &str) -> std::collections::BTreeMap<String, Vec<u8>> {
    let path = std::path::Path::new(path);
    let prefix = path.file_name().unwrap().to_str().unwrap();
    std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            (name == prefix || name.starts_with(&format!("{prefix}.")))
                .then(|| (name, std::fs::read(entry.path()).unwrap()))
        })
        .collect()
}

#[test]
fn __group_commit_power_loss_child_entry() {
    let Ok(path) = std::env::var("TRIVIUM_GROUP_COMMIT_CHILD") else {
        return;
    };
    let ready = format!("{path}.ready");
    let mut database = Database::<f32>::open_with_config(
        &path,
        triviumdb::database::Config {
            dim: DIM,
            storage_mode: triviumdb::database::StorageMode::Mmap,
            sync_mode: triviumdb::storage::wal::SyncMode::Full,
            ..Default::default()
        },
    )
    .unwrap();
    let (results, synced) = database
        .group_commit(|database| {
            (1..=12u64)
                .map(|id| {
                    database.insert_with_id(
                        id,
                        &[id as f32, 1.0, 0.0, 0.0],
                        serde_json::json!({"group": true, "id": id}),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert!(synced && results.iter().all(Result::is_ok));
    std::fs::write(&ready, b"ready").unwrap();
    std::mem::forget(database);
    loop {
        std::thread::park();
    }
}

#[test]
fn PWR_00B_group_commit确认后强杀可恢复全部独立事务() {
    let path = tmp_db("group_commit_kill");
    cleanup(&path);
    let ready = format!("{path}.ready");
    let mut child = child_command()
        .env("TRIVIUM_GROUP_COMMIT_CHILD", &path)
        .arg("__group_commit_power_loss_child_entry")
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
            "Group Commit 子进程提前退出"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        std::path::Path::new(&ready).exists(),
        "子进程未完成 Group Commit"
    );
    child.kill().unwrap();
    child.wait().unwrap();

    let database = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(database.node_count().unwrap(), 12);
    for id in 1..=12u64 {
        assert_eq!(database.get_payload(id).unwrap().unwrap()["id"], id);
    }
    std::fs::remove_file(ready).ok();
    cleanup(&path);
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
    let mut child = child_command()
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
    assert_eq!(db.node_count().unwrap(), 8, "强制终止后必须回放完整 WAL 帧");
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

        let count = db.node_count().unwrap();
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

        let new_count = db.node_count().unwrap();
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
        db.node_count().unwrap(),
        max_seen
    );
    assert!(db.node_count().unwrap() > 0, "100 轮后应至少有一些数据存活");

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
        db.node_count().unwrap(),
        50,
        "flush 中途断电后应从旧 .tdb 恢复完整 50 个节点"
    );

    eprintln!(
        "  ✅ flush 中途断电: 从旧 .tdb 恢复 {} 个节点",
        db.node_count().unwrap()
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
        let ids: Vec<u64> = db
            .all_node_ids()
            .unwrap()
            .iter()
            .take(50)
            .copied()
            .collect();
        for id in &ids {
            db.delete(*id).unwrap();
        }
        db.flush().unwrap();
        // 断电 (drop)
    }

    // 重新打开 — 验证删除持久化
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        db.node_count().unwrap(),
        50,
        "删除 50 个节点后 flush + 断电重启，应剩余 50 个"
    );

    eprintln!(
        "  ✅ 删除 + 断电: 剩余 {} 个节点，tombstone 正确持久化",
        db.node_count().unwrap()
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
        db.node_count().unwrap(),
        20,
        "事务 commit 后断电，WAL 回放应恢复 20 个节点"
    );

    // 验证 payload 完整性
    for &id in &db.all_node_ids().unwrap() {
        let payload = db.get_payload(id).unwrap().unwrap();
        assert!(
            payload.get("tx_item").is_some(),
            "节点 {} 的 payload 应包含 tx_item 字段",
            id
        );
    }

    eprintln!(
        "  ✅ 事务 + 断电: WAL 回放恢复 {} 个节点，payload 完整",
        db.node_count().unwrap()
    );

    cleanup(&path);
}

#[cfg(feature = "test-hooks")]
mod snapshot;
