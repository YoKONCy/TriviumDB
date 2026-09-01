#![cfg(feature = "test-hooks")]
#![allow(non_snake_case)]
//! 通过 test-hooks 精确控制执行交错，不使用 sleep 猜测时序。

use std::sync::mpsc;
use std::sync::{MutexGuard, OnceLock};
use std::time::Duration;

use serde_json::json;
use triviumdb::Database;
use triviumdb::database::SearchConfig;
use triviumdb::storage::wal::{Wal, WalEntry};
use triviumdb::test_hooks::{ConcurrencyPoint, hit, pause_at};

const DIM: usize = 4;

fn test_serial_guard() -> MutexGuard<'static, ()> {
    static SERIAL: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    SERIAL
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("deterministic_interleaving_{name}"))
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
        ".quiver",
        ".quiver.meta",
        ".text",
        ".text.meta",
    ] {
        std::fs::remove_file(format!("{path}{ext}")).ok();
    }
}

#[test]
fn Failpoint_能够确定等待到达并释放目标线程() {
    let _serial = test_serial_guard();
    let waiter = pause_at(ConcurrencyPoint::BeforeMemtableApply);
    let (done_tx, done_rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        hit(ConcurrencyPoint::BeforeMemtableApply);
        done_tx.send(()).unwrap();
    });

    waiter.wait_until_arrived();
    assert!(
        done_rx.try_recv().is_err(),
        "目标线程到达 failpoint 后必须保持阻塞"
    );
    waiter.release();
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    thread.join().unwrap();
}

#[test]
fn 事务交错_WAL完整提交后才允许应用MemTable() {
    let _serial = test_serial_guard();
    let path = tmp_db("wal_before_apply");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let waiter = pause_at(ConcurrencyPoint::AfterWalAppend);

    let thread = std::thread::spawn(move || {
        let ids = {
            let mut tx = db.begin_tx();
            tx.insert(&[1.0, 0.0, 0.0, 0.0], json!({"phase": "committed"}));
            tx.commit().unwrap()
        };
        (db, ids)
    });

    waiter.wait_until_arrived();
    let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            WalEntry::Insert { payload, .. } if payload.contains("committed")
        )),
        "到达 AfterWalAppend 时，事务封条和完整条目必须已经可恢复"
    );

    waiter.release();
    let (db, ids) = thread.join().unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(db.get_payload(ids[0]).unwrap()["phase"], "committed");

    drop(db);
    cleanup(&path);
}

#[test]
fn 压实交错_主数据已保存但WAL未清理时释放后可完整重载() {
    let _serial = test_serial_guard();
    let path = tmp_db("compact_before_wal_clear");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id = db
        .insert(&[1.0, 2.0, 3.0, 4.0], json!({"state": "must-survive"}))
        .unwrap();
    let waiter = pause_at(ConcurrencyPoint::BeforeWalClear);

    let thread = std::thread::spawn(move || {
        db.compact().unwrap();
        db
    });
    waiter.wait_until_arrived();

    // 此时主文件保存已成功，但 WAL 仍保留。无论在该边界崩溃还是正常继续，
    // 重开都必须得到同一个提交，不能丢失或产生重复节点。
    assert!(std::path::Path::new(&path).exists());
    let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
    assert!(!entries.is_empty(), "BeforeWalClear 边界 WAL 必须尚未清理");

    waiter.release();
    let db = thread.join().unwrap();
    drop(db);

    let reopened = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(reopened.node_count(), 1);
    assert_eq!(reopened.get_payload(id).unwrap()["state"], "must-survive");
    drop(reopened);
    cleanup(&path);
}

#[test]
fn QuIVer交错_发布前保持阻塞释放后索引可查询() {
    let _serial = test_serial_guard();
    let path = tmp_db("quiver_publish");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for id in 1..=64u64 {
        db.insert_with_id(
            id,
            &[id as f32, (id as f32).sin(), 1.0, 0.0],
            json!({"id": id}),
        )
        .unwrap();
    }
    let waiter = pause_at(ConcurrencyPoint::BeforeQuiverPublish);
    let (done_tx, done_rx) = mpsc::channel();

    let thread = std::thread::spawn(move || {
        db.build_quiver_index(None).unwrap();
        done_tx.send(()).unwrap();
        db
    });
    waiter.wait_until_arrived();
    assert!(
        done_rx.try_recv().is_err(),
        "QuIVer 发布前构建调用不得提前返回"
    );

    waiter.release();
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let db = thread.join().unwrap();
    let hits = db
        .search_advanced(
            &[1.0, 1.0f32.sin(), 1.0, 0.0],
            &SearchConfig {
                top_k: 5,
                recall_k: 16,
                rerank_k: 8,
                expand_depth: 0,
                min_score: -1.0,
                force_brute_force: false,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(hits.iter().any(|hit| hit.id == 1));

    drop(db);
    cleanup(&path);
}

#[test]
fn QuIVer交错_构建期间向量变化会拒绝过期索引() {
    let _serial = test_serial_guard();
    let path = tmp_db("quiver_stale_generation");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for id in 1..=64u64 {
        db.insert_with_id(
            id,
            &[id as f32, (id as f32).sin(), 1.0, 0.0],
            json!({"id": id}),
        )
        .unwrap();
    }
    let db = std::sync::Arc::new(db);
    let waiter = pause_at(ConcurrencyPoint::BeforeQuiverPublish);
    let build_db = std::sync::Arc::clone(&db);
    let thread = std::thread::spawn(move || build_db.build_quiver_index(None));

    waiter.wait_until_arrived();
    db.update_vector(1, &[0.0, 1.0, 0.0, 0.0]).unwrap();
    waiter.release();

    assert!(thread.join().unwrap().is_err(), "过期 QuIVer 不得发布");
    db.build_quiver_index(None).unwrap();
    let hits = db
        .search_advanced(
            &[0.0, 1.0, 0.0, 0.0],
            &SearchConfig {
                top_k: 5,
                recall_k: 16,
                rerank_k: 8,
                expand_depth: 0,
                min_score: -1.0,
                force_brute_force: false,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(hits.iter().any(|hit| hit.id == 1));

    drop(db);
    cleanup(&path);
}

#[test]
fn MergedCache交错_发布前搜索阻塞释放后结果完整() {
    let _serial = test_serial_guard();
    let path = tmp_db("merged_cache_publish");
    cleanup(&path);
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for id in 1..=32u64 {
            db.insert_with_id(id, &[id as f32, 1.0, 0.0, 0.0], json!({"id": id}))
                .unwrap();
        }
        db.flush().unwrap();
    }

    // 重开后向量位于 mmap 基础层，强制暴力搜索会首次物化 merged cache。
    let db = Database::<f32>::open(&path, DIM).unwrap();
    let waiter = pause_at(ConcurrencyPoint::BeforeMergedCachePublish);
    let (done_tx, done_rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let hits = db
            .search_advanced(
                &[32.0, 1.0, 0.0, 0.0],
                &SearchConfig {
                    top_k: 4,
                    recall_k: 4,
                    rerank_k: 4,
                    expand_depth: 0,
                    min_score: -1.0,
                    force_brute_force: true,
                    ..Default::default()
                },
            )
            .unwrap();
        done_tx.send(hits.len()).unwrap();
        db
    });
    waiter.wait_until_arrived();
    assert!(
        done_rx.try_recv().is_err(),
        "merged cache 标记有效前搜索不得继续"
    );

    waiter.release();
    assert_eq!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 4);
    let db = thread.join().unwrap();
    drop(db);
    cleanup(&path);
}

#[test]
fn QuIVer候选交错_候选生成与精排边界均可精确控制() {
    let _serial = test_serial_guard();
    let path = tmp_db("candidate_rerank");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for id in 1..=64u64 {
        db.insert_with_id(
            id,
            &[id as f32, (id as f32 * 0.25).sin(), 1.0, 0.0],
            json!({"id": id}),
        )
        .unwrap();
    }
    db.build_quiver_index(None).unwrap();
    let db = std::sync::Arc::new(db);
    let candidate_waiter = pause_at(ConcurrencyPoint::QuiverCandidateProduced);
    let rerank_waiter = pause_at(ConcurrencyPoint::BeforeVectorRerank);
    let (done_tx, done_rx) = mpsc::channel();
    let search_db = std::sync::Arc::clone(&db);

    let thread = std::thread::spawn(move || {
        let hits = search_db
            .search_advanced(
                &[64.0, (64.0f32 * 0.25).sin(), 1.0, 0.0],
                &SearchConfig {
                    top_k: 8,
                    recall_k: 16,
                    rerank_k: 8,
                    expand_depth: 0,
                    min_score: -1.0,
                    force_brute_force: false,
                    ..Default::default()
                },
            )
            .unwrap();
        done_tx.send(hits).unwrap();
    });

    candidate_waiter.wait_until_arrived();
    assert!(done_rx.try_recv().is_err());
    candidate_waiter.release();

    rerank_waiter.wait_until_arrived();
    assert!(done_rx.try_recv().is_err());
    rerank_waiter.release();

    let hits = done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(hits.len(), 8);
    for hit in &hits {
        assert_eq!(hit.payload["id"], hit.id);
    }
    thread.join().unwrap();
    drop(db);
    cleanup(&path);
}

#[test]
fn Fatigue交错_有状态查询保持串行而无状态查询不受影响() {
    let _serial = test_serial_guard();
    let path = tmp_db("stateful_serialization");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.insert(&[1.0, 0.0, 0.0, 0.0], json!({"id": 1})).unwrap();
    let db = std::sync::Arc::new(db);
    let before_lock = pause_at(ConcurrencyPoint::BeforeStatefulSearchLock);
    let entered = pause_at(ConcurrencyPoint::StatefulSearchEntered);
    let config = SearchConfig {
        top_k: 1,
        recall_k: 1,
        rerank_k: 1,
        expand_depth: 0,
        min_score: -1.0,
        force_brute_force: true,
        enable_refractory_fatigue: true,
        ..Default::default()
    };

    let first_db = std::sync::Arc::clone(&db);
    let first_cfg = config.clone();
    let first = std::thread::spawn(move || {
        first_db
            .search_advanced(&[1.0, 0.0, 0.0, 0.0], &first_cfg)
            .unwrap()
    });
    before_lock.wait_until_arrived();
    before_lock.release();
    entered.wait_until_arrived();

    let second_before_lock = pause_at(ConcurrencyPoint::BeforeStatefulSearchLock);
    let second_db = std::sync::Arc::clone(&db);
    let second = std::thread::spawn(move || {
        second_db
            .search_advanced(&[1.0, 0.0, 0.0, 0.0], &config)
            .unwrap()
    });
    second_before_lock.wait_until_arrived();
    second_before_lock.release();
    assert_eq!(entered.arrivals(), 1, "第二个有状态查询不得越过串行锁");

    entered.release();
    assert_eq!(first.join().unwrap().len(), 1);
    assert_eq!(second.join().unwrap().len(), 1);
    drop(db);
    cleanup(&path);
}

#[test]
fn Fatigue清理交错_只能在线性顺序上发生于完整查询之间() {
    let _serial = test_serial_guard();
    let path = tmp_db("fatigue_clear_serialization");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.insert(&[1.0, 0.0, 0.0, 0.0], json!({"id": 1})).unwrap();
    let db = std::sync::Arc::new(db);
    let query_entered = pause_at(ConcurrencyPoint::StatefulSearchEntered);
    let config = SearchConfig {
        top_k: 1,
        recall_k: 1,
        rerank_k: 1,
        expand_depth: 0,
        min_score: -1.0,
        force_brute_force: true,
        enable_refractory_fatigue: true,
        ..Default::default()
    };

    let query_db = std::sync::Arc::clone(&db);
    let query = std::thread::spawn(move || {
        query_db
            .search_advanced(&[1.0, 0.0, 0.0, 0.0], &config)
            .unwrap()
    });
    query_entered.wait_until_arrived();

    let clear_before_lock = pause_at(ConcurrencyPoint::BeforeClearSearchStateLock);
    let clear_entered = pause_at(ConcurrencyPoint::ClearSearchStateEntered);
    let clear_db = std::sync::Arc::clone(&db);
    let clear = std::thread::spawn(move || clear_db.clear_search_state());
    clear_before_lock.wait_until_arrived();
    clear_before_lock.release();
    assert_eq!(
        clear_entered.arrivals(),
        0,
        "活动 fatigue 查询期间清理不得进入"
    );

    query_entered.release();
    assert_eq!(query.join().unwrap().len(), 1);
    clear_entered.wait_until_arrived();
    clear_entered.release();
    clear.join().unwrap();
    drop(db);
    cleanup(&path);
}

#[test]
fn 查询写交错_更新向量必须等待完整查询释放读锁() {
    let _serial = test_serial_guard();
    let path = tmp_db("query_update_isolation");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id = db.insert(&[1.0, 0.0, 0.0, 0.0], json!({"id": 1})).unwrap();
    let db = std::sync::Arc::new(db);
    let query_waiter = pause_at(ConcurrencyPoint::SearchLockAcquired);
    let config = SearchConfig {
        top_k: 1,
        recall_k: 1,
        rerank_k: 1,
        expand_depth: 0,
        min_score: -1.0,
        force_brute_force: true,
        ..Default::default()
    };

    let query_db = std::sync::Arc::clone(&db);
    let query = std::thread::spawn(move || {
        query_db
            .search_advanced(&[1.0, 0.0, 0.0, 0.0], &config)
            .unwrap()
    });
    query_waiter.wait_until_arrived();

    let writer_before_lock = pause_at(ConcurrencyPoint::BeforeUpdateVectorWriteLock);
    let writer_acquired = pause_at(ConcurrencyPoint::UpdateVectorWriteLockAcquired);
    let writer_db = std::sync::Arc::clone(&db);
    let writer = std::thread::spawn(move || writer_db.update_vector(id, &[0.0, 1.0, 0.0, 0.0]));
    writer_before_lock.wait_until_arrived();
    writer_before_lock.release();
    assert_eq!(
        writer_acquired.arrivals(),
        0,
        "查询持有 read guard 时写者不得进入"
    );

    query_waiter.release();
    assert_eq!(query.join().unwrap().len(), 1);
    writer_acquired.wait_until_arrived();
    writer_acquired.release();
    writer.join().unwrap().unwrap();
    assert_eq!(db.get(id).unwrap().vector, [0.0, 1.0, 0.0, 0.0]);
    drop(db);
    cleanup(&path);
}

#[test]
fn QuIVer单飞_同代际只有一个构建者() {
    let _serial = test_serial_guard();
    let path = tmp_db("quiver_singleflight");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for id in 1..=64u64 {
        db.insert_with_id(id, &[id as f32, 1.0, 0.0, 0.0], json!({"id": id}))
            .unwrap();
    }
    let db = std::sync::Arc::new(db);
    let before_claim = pause_at(ConcurrencyPoint::BeforeQuiverBuildClaim);
    let build_started = pause_at(ConcurrencyPoint::QuiverBuildStarted);

    let first_db = std::sync::Arc::clone(&db);
    let first = std::thread::spawn(move || first_db.build_quiver_index(None));
    before_claim.wait_until_arrived();
    before_claim.release();
    build_started.wait_until_arrived();

    let follower_waiting = pause_at(ConcurrencyPoint::QuiverBuildFollowerWaiting);
    let second_db = std::sync::Arc::clone(&db);
    let second = std::thread::spawn(move || second_db.build_quiver_index(None));
    follower_waiting.wait_until_arrived();
    assert_eq!(build_started.arrivals(), 1, "同代际不得启动第二次构建");
    follower_waiting.release();

    build_started.release();
    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();
    drop(db);
    cleanup(&path);
}

#[test]
fn 查询交错_RwLock允许第二查询进入共享读区() {
    let _serial = test_serial_guard();
    let path = tmp_db("search_concurrent_baseline");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.insert(&[1.0, 0.0, 0.0, 0.0], json!({"id": 1})).unwrap();
    let db = std::sync::Arc::new(db);
    let first_waiter = pause_at(ConcurrencyPoint::SearchLockAcquired);
    let config = SearchConfig {
        top_k: 1,
        recall_k: 1,
        rerank_k: 1,
        expand_depth: 0,
        min_score: -1.0,
        force_brute_force: true,
        ..Default::default()
    };

    let first_db = std::sync::Arc::clone(&db);
    let first_config = config.clone();
    let first = std::thread::spawn(move || {
        first_db
            .search_advanced(&[1.0, 0.0, 0.0, 0.0], &first_config)
            .unwrap()
    });
    first_waiter.wait_until_arrived();

    // 第一个查询仍在 failpoint 中持有 read guard，第二个查询必须也能到达同一位置。
    let second_db = std::sync::Arc::clone(&db);
    let second = std::thread::spawn(move || {
        second_db
            .search_advanced(&[1.0, 0.0, 0.0, 0.0], &config)
            .unwrap()
    });
    first_waiter.wait_until_arrivals(2);
    first_waiter.release();

    assert_eq!(first.join().unwrap().len(), 1);
    assert_eq!(second.join().unwrap().len(), 1);
    drop(db);
    cleanup(&path);
}
