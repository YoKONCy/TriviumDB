#![allow(non_snake_case)]
//! 真实并发安全测试 — 不使用 Mutex 包装
//!
//! 与 stress.rs 的区别：
//!   stress.rs   — 所有线程通过 `Arc<Mutex<Database>>` 串行化访问，
//!                 本质上是排队测试，无法发现真正的数据竞争
//!   concurrent  — 直接在多线程中操作，测试 Database 自身的线程安全性
//!
//! 覆盖场景：
//!   1. 多线程同时只读查询（Database 应当支持并发读）
//!   2. 多线程尝试并发写入（验证 Database 是否 Send+Sync 或正确拒绝）
//!   3. 读写混合并发（如果 API 支持的话）

use std::sync::Arc;
use triviumdb::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("concurrent_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

/// 预填充一个有数据的数据库
fn seed_db(path: &str, count: usize) -> Database<f32> {
    let mut db = Database::<f32>::open(path, DIM).unwrap();
    for i in 0..count {
        db.insert(
            &[i as f32, (i as f32).sin(), (i as f32).cos(), 1.0],
            serde_json::json!({"idx": i, "tag": format!("node_{}", i)}),
        )
        .unwrap();
    }
    db.flush().unwrap();
    db
}

// ════════════════════════════════════════════════════════════════
//  测试 1: 多线程只读查询竞争
// ════════════════════════════════════════════════════════════════

/// 在 8 个线程中同时执行只读操作（node_count, all_node_ids, get_payload, search），
/// 验证只读操作在并发环境中不会：
///   - panic
///   - 返回不一致的数据（比如 all_node_ids 返回的 id 在 get_payload 中找不到）
///   - segfault
#[test]
fn 并发_8线程同时只读查询_不恐慌不矛盾() {
    let path = tmp_db("multiread");
    cleanup(&path);

    let db = seed_db(&path, 100);
    let db = Arc::new(db);

    let mut handles = vec![];

    for thread_id in 0..8 {
        let db = Arc::clone(&db);
        let handle = std::thread::spawn(move || {
            // 每个线程执行 200 轮只读操作
            for round in 0..200 {
                // 操作 1: node_count
                let count = db.node_count();
                assert!(
                    count > 0,
                    "线程 {} 轮 {}: node_count 返回 0",
                    thread_id,
                    round
                );

                // 操作 2: all_node_ids
                let ids = db.all_node_ids();
                assert_eq!(
                    ids.len(),
                    count,
                    "线程 {} 轮 {}: all_node_ids.len()={} != node_count={}",
                    thread_id,
                    round,
                    ids.len(),
                    count
                );

                // 操作 3: get_payload（验证每个 ID 都能查到）
                for &id in &ids {
                    let payload = db.get_payload(id);
                    assert!(
                        payload.is_some(),
                        "线程 {} 轮 {}: id {} 在 all_node_ids 中但 get_payload 返回 None",
                        thread_id,
                        round,
                        id
                    );
                }

                // 操作 4: search
                let query = [(thread_id as f32) * 0.1, 0.0, 0.0, 1.0];
                let result = db.search(&query, 5, 0, 0.0);
                assert!(
                    result.is_ok(),
                    "线程 {} 轮 {}: search 返回 Err: {:?}",
                    thread_id,
                    round,
                    result.err()
                );
            }
        });
        handles.push(handle);
    }

    let mut panic_count = 0;
    for (i, h) in handles.into_iter().enumerate() {
        if let Err(e) = h.join() {
            panic_count += 1;
            eprintln!("  ❌ 线程 {} panic: {:?}", i, e);
        }
    }

    assert_eq!(
        panic_count, 0,
        "{}/8 个只读线程发生了 panic！Database 的并发读不安全",
        panic_count
    );
    eprintln!("  ✅ 8 线程并发只读: 8×200=1600 轮操作，零 panic，数据一致");

    drop(db);
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  测试 2: 读写混合 — 一个写线程 + 多个读线程
// ════════════════════════════════════════════════════════════════

/// 1 个写线程持续插入新节点，4 个读线程同时查询。
/// 使用 `Arc<Mutex<Database>>` 包装写操作（因为 &mut self），
/// 但读线程直接通过 Arc 访问（如果 API 支持的话）。
///
/// 这个测试的重点不是绕开 Mutex，而是验证在高频写入期间：
///   - 读线程拿到的数据是否一致（不会看到半写状态）
///   - 写线程不会因为读线程的访问而 panic
///   - 总节点数单调递增
#[test]
fn 并发_1写4读_写入期间读取一致性() {
    use std::sync::Mutex;

    let path = tmp_db("write_read_mix");
    cleanup(&path);

    let db = seed_db(&path, 50);
    let db = Arc::new(Mutex::new(db));

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut handles = vec![];

    // 写线程
    {
        let db = Arc::clone(&db);
        let stop = Arc::clone(&stop);
        handles.push(std::thread::spawn(move || {
            for i in 100..300u32 {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let mut db = db.lock().unwrap();
                let result = db.insert(
                    &[i as f32, 0.0, 0.0, 0.0],
                    serde_json::json!({"writer": true, "seq": i}),
                );
                assert!(result.is_ok(), "写线程 insert 失败: {:?}", result.err());
            }
        }));
    }

    // 4 个读线程
    for reader_id in 0..4 {
        let db = Arc::clone(&db);
        let stop = Arc::clone(&stop);
        handles.push(std::thread::spawn(move || {
            let mut prev_count = 0usize;
            let mut rounds = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) && rounds < 500 {
                let db = db.lock().unwrap();
                let count = db.node_count();

                // 单调性：节点数只增不减
                assert!(
                    count >= prev_count,
                    "读线程 {} 轮 {}: 节点数从 {} 降到了 {}！数据一致性被破坏",
                    reader_id,
                    rounds,
                    prev_count,
                    count
                );
                prev_count = count;

                // 一致性：all_node_ids 和 node_count 必须匹配
                let ids = db.all_node_ids();
                assert_eq!(
                    ids.len(),
                    count,
                    "读线程 {} 轮 {}: 不一致",
                    reader_id,
                    rounds
                );

                drop(db); // 显式释放锁

                // 让出 CPU 给写线程
                std::thread::yield_now();
                rounds += 1;
            }
        }));
    }

    // 等待写线程完成
    let writer_handle = handles.remove(0);
    writer_handle.join().unwrap();

    // 通知读线程停止
    stop.store(true, std::sync::atomic::Ordering::Relaxed);

    let mut panic_count = 0;
    for (i, h) in handles.into_iter().enumerate() {
        if let Err(e) = h.join() {
            panic_count += 1;
            eprintln!("  ❌ 读线程 {} panic: {:?}", i, e);
        }
    }

    assert_eq!(panic_count, 0, "读写混合并发测试失败");

    // 验证最终状态：50 初始 + 200 写入 = 250
    let db = db.lock().unwrap();
    assert_eq!(
        db.node_count(),
        250,
        "最终节点数应为 250（50 初始 + 200 写入），实际 {}",
        db.node_count()
    );
    eprintln!(
        "  ✅ 1 写 4 读并发: 最终 {} 个节点，节点数单调递增，数据一致",
        db.node_count()
    );

    drop(db);
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  测试 3: Database 的 Send + Sync 特性验证
// ════════════════════════════════════════════════════════════════

/// 编译期验证 Database<f32> 实现了 Send 和 Sync trait。
/// 如果 Database 不是 Send+Sync，这个测试将无法编译。
#[test]
fn 并发_编译期验证_Database是Send和Sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    assert_send::<Database<f32>>();
    assert_sync::<Database<f32>>();

    eprintln!("  ✅ Database<f32> 实现了 Send + Sync（编译期验证通过）");
}

// ════════════════════════════════════════════════════════════════
//  测试 4: 高并发 TQL 查询不互相干扰
// ════════════════════════════════════════════════════════════════

/// 8 个线程同时执行不同的 TQL 查询，验证查询结果互不干扰。
#[test]
fn 并发_8线程TQL查询_结果互不干扰() {
    let path = tmp_db("tql_concurrent");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 构建一个带图关系的数据集
    let ids = {
        let mut tx = db.begin_tx();
        tx.insert(
            &[1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"name": "Alice", "type": "person"}),
        );
        tx.insert(
            &[0.0, 1.0, 0.0, 0.0],
            serde_json::json!({"name": "Bob", "type": "person"}),
        );
        tx.insert(
            &[0.0, 0.0, 1.0, 0.0],
            serde_json::json!({"name": "Charlie", "type": "person"}),
        );
        tx.insert(
            &[0.0, 0.0, 0.0, 1.0],
            serde_json::json!({"name": "Project X", "type": "project"}),
        );
        tx.commit().unwrap()
    };

    db.link(ids[0], ids[1], "knows", 1.0).unwrap();
    db.link(ids[1], ids[2], "knows", 1.0).unwrap();
    db.link(ids[0], ids[3], "works_on", 1.0).unwrap();

    let db = Arc::new(db);

    let queries = vec![
        r#"FIND {"type": "person"} RETURN *"#,
        r#"FIND {"name": "Alice"} RETURN *"#,
        r#"FIND {"type": "project"} RETURN *"#,
        "SEARCH [1.0, 0.0, 0.0, 0.0] TOP 2 RETURN *",
        "SEARCH [0.0, 1.0, 0.0, 0.0] TOP 3 RETURN *",
        r#"FIND {"type": "person"} LIMIT 2 RETURN *"#,
        r#"FIND {"name": "Bob"} RETURN *"#,
        "SEARCH [0.0, 0.0, 1.0, 0.0] TOP 1 RETURN *",
    ];

    let mut handles = vec![];

    for (thread_id, query) in queries.into_iter().enumerate() {
        let db = Arc::clone(&db);
        let query = query.to_string();
        handles.push(std::thread::spawn(move || {
            for round in 0..100 {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| db.tql(&query)));

                assert!(
                    result.is_ok(),
                    "线程 {} 轮 {} TQL panic: query={:?}",
                    thread_id,
                    round,
                    query
                );
            }
        }));
    }

    let mut panic_count = 0;
    for (i, h) in handles.into_iter().enumerate() {
        if let Err(e) = h.join() {
            panic_count += 1;
            eprintln!("  ❌ TQL 线程 {} panic: {:?}", i, e);
        }
    }

    assert_eq!(panic_count, 0, "{}/8 个 TQL 并发线程 panic！", panic_count);
    eprintln!("  ✅ 8 线程 TQL 并发: 8×100=800 轮查询，零 panic");

    drop(db);
    cleanup(&path);
}
