#![allow(non_snake_case)]
//! Database 读并发核心不变量门禁。
//!
//! 覆盖共享读确定性、真实读写隔离、writer 最终进展、generation 推进、slot ABA、
//! Payload 惰性解析、QuIVer 候选一致性以及持久化恢复。

use std::sync::{Arc, Barrier, Mutex};

use serde_json::json;
use triviumdb::database::{Config, SearchConfig};
use triviumdb::storage::memtable::MemTable;
use triviumdb::{Database, TriviumError};

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("concurrency_invariant_{name}"))
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

fn exact_config(top_k: usize) -> SearchConfig {
    SearchConfig {
        top_k,
        recall_k: top_k,
        rerank_k: top_k,
        expand_depth: 0,
        min_score: -1.0,
        force_brute_force: true,
        ..Default::default()
    }
}

#[test]
fn Generation规则_向量与非向量修改推进正确代际() {
    let mut mt = MemTable::<f32>::new(DIM);
    assert_eq!((mt.generation(), mt.vector_generation()), (0, 0));

    mt.insert_with_id(1, &[1.0, 0.0, 0.0, 0.0], json!({"v": 1}))
        .unwrap();
    assert_eq!((mt.generation(), mt.vector_generation()), (1, 1));
    mt.insert_with_id(2, &[0.0, 1.0, 0.0, 0.0], json!({"v": 2}))
        .unwrap();
    assert_eq!((mt.generation(), mt.vector_generation()), (2, 2));

    mt.update_payload(1, json!({"v": 3})).unwrap();
    assert_eq!((mt.generation(), mt.vector_generation()), (3, 2));
    mt.link(1, 2, "rel".to_string(), 1.0).unwrap();
    assert_eq!((mt.generation(), mt.vector_generation()), (4, 2));
    mt.unlink(1, 2).unwrap();
    assert_eq!((mt.generation(), mt.vector_generation()), (5, 2));

    mt.update_vector(1, &[0.0, 0.0, 1.0, 0.0]).unwrap();
    assert_eq!((mt.generation(), mt.vector_generation()), (6, 3));
    mt.delete(2).unwrap();
    assert_eq!((mt.generation(), mt.vector_generation()), (7, 4));
}

#[test]
fn Writer进展_持续并发读结束前写者最终取得写锁() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    let path = tmp_db("writer_progress");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id = db.insert(&[1.0, 0.0, 0.0, 0.0], json!({"id": 1})).unwrap();
    let db = Arc::new(db);
    let stop = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(Barrier::new(9));
    let config = exact_config(1);
    let mut readers = Vec::new();
    for _ in 0..8 {
        let db = Arc::clone(&db);
        let stop = Arc::clone(&stop);
        let barrier = Arc::clone(&barrier);
        let config = config.clone();
        readers.push(std::thread::spawn(move || {
            barrier.wait();
            while !stop.load(Ordering::Acquire) {
                let hits = db.search_advanced(&[1.0, 0.0, 0.0, 0.0], &config).unwrap();
                assert_eq!(hits.len(), 1);
            }
        }));
    }

    let writer_db = Arc::clone(&db);
    let writer_barrier = Arc::clone(&barrier);
    let (done_tx, done_rx) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        writer_barrier.wait();
        let result = writer_db.update_vector(id, &[0.0, 1.0, 0.0, 0.0]);
        done_tx.send(result).unwrap();
    });

    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("持续读负载下写者必须最终取得写锁")
        .unwrap();
    stop.store(true, Ordering::Release);
    writer.join().unwrap();
    for reader in readers {
        reader.join().unwrap();
    }
    assert_eq!(db.get(id).unwrap().vector, [0.0, 1.0, 0.0, 0.0]);
    drop(db);
    cleanup(&path);
}

#[test]
fn 并发读_16线程同版本精确搜索结果完全一致() {
    let path = tmp_db("deterministic_search");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..128u64 {
        let angle = i as f32 * 0.03125;
        db.insert_with_id(
            i + 1,
            &[angle.cos(), angle.sin(), (angle * 0.5).cos(), 1.0],
            json!({"id": i + 1}),
        )
        .unwrap();
    }

    let query = [1.0, 0.2, 0.4, 1.0];
    let config = exact_config(16);
    let expected = db.search_advanced(&query, &config).unwrap();
    let expected_ids: Vec<_> = expected.iter().map(|hit| hit.id).collect();
    let expected_scores: Vec<_> = expected.iter().map(|hit| hit.score).collect();

    let db = Arc::new(db);
    let barrier = Arc::new(Barrier::new(16));
    let mut threads = Vec::new();
    for _ in 0..16 {
        let db = Arc::clone(&db);
        let barrier = Arc::clone(&barrier);
        let expected_ids = expected_ids.clone();
        let expected_scores = expected_scores.clone();
        let config = config.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..100 {
                let hits = db.search_advanced(&query, &config).unwrap();
                assert_eq!(
                    hits.iter().map(|hit| hit.id).collect::<Vec<_>>(),
                    expected_ids,
                    "同一版本的并发无状态查询不得改变排名"
                );
                for (hit, expected_score) in hits.iter().zip(&expected_scores) {
                    assert_eq!(
                        hit.score.to_bits(),
                        expected_score.to_bits(),
                        "同一版本的并发无状态查询分数必须逐位一致"
                    );
                    assert_eq!(hit.payload["id"], hit.id);
                }
            }
        }));
    }

    for thread in threads {
        thread.join().unwrap();
    }
    drop(db);
    cleanup(&path);
}

#[test]
fn 并发读_冷Payload首次惰性解析只产生一致值() {
    let path = tmp_db("lazy_payload");
    cleanup(&path);
    let id;
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        id = db
            .insert(
                &[1.0, 0.0, 0.0, 0.0],
                json!({"版本": 7, "nested": {"name": "并发惰性解析"}}),
            )
            .unwrap();
        db.flush().unwrap();
    }

    // 重开后 PayloadEntry 的 parsed OnceLock 尚未初始化，多个线程竞争首次解析。
    let db = Arc::new(Database::<f32>::open(&path, DIM).unwrap());
    let barrier = Arc::new(Barrier::new(16));
    let mut threads = Vec::new();
    for _ in 0..16 {
        let db = Arc::clone(&db);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..200 {
                let payload = db.get_payload(id).expect("节点必须始终存在");
                assert_eq!(payload["版本"], 7);
                assert_eq!(payload["nested"]["name"], "并发惰性解析");
            }
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }

    drop(db);
    cleanup(&path);
}

#[test]
fn Slot复用_旧ID绝不读取新节点的向量或Payload() {
    let mut mt = MemTable::<f32>::new(DIM);
    let old_id = mt
        .insert(&[1.0, 0.0, 0.0, 0.0], json!({"owner": "old"}))
        .unwrap();
    let old_slot_count = mt.internal_slot_count();

    mt.delete(old_id).unwrap();
    let new_id = mt
        .insert(&[0.0, 1.0, 0.0, 0.0], json!({"owner": "new"}))
        .unwrap();

    assert_eq!(
        mt.internal_slot_count(),
        old_slot_count,
        "删除后的槽位应被复用"
    );
    assert_ne!(old_id, new_id, "复用物理槽位不能复用逻辑 NodeId");
    assert!(!mt.contains(old_id));
    assert!(
        mt.get_vector(old_id).is_none(),
        "旧 ID 不得解析到新槽位向量"
    );
    assert!(
        mt.get_payload(old_id).is_none(),
        "旧 ID 不得解析到新槽位 Payload"
    );
    assert_eq!(mt.get_vector(new_id).unwrap(), &[0.0, 1.0, 0.0, 0.0]);
    assert_eq!(mt.get_payload(new_id).unwrap()["owner"], "new");
}

#[test]
fn QuIVer与Slot复用_旧候选不得读取新节点向量() {
    let path = tmp_db("quiver_slot_aba");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let old_id = db
        .insert(&[1.0, 0.0, 0.0, 0.0], json!({"owner": "old"}))
        .unwrap();
    let stable_id = db
        .insert(&[0.0, 0.0, 1.0, 0.0], json!({"owner": "stable"}))
        .unwrap();
    db.build_quiver_index(None).unwrap();

    db.delete(old_id).unwrap();
    let new_id = db
        .insert(&[0.0, 1.0, 0.0, 0.0], json!({"owner": "new"}))
        .unwrap();

    let config = SearchConfig {
        top_k: 2,
        recall_k: 8,
        rerank_k: 4,
        expand_depth: 0,
        min_score: -1.0,
        force_brute_force: false,
        ..Default::default()
    };
    let hits = db.search_advanced(&[0.0, 1.0, 0.0, 0.0], &config).unwrap();

    assert!(hits.iter().all(|hit| hit.id != old_id));
    let new_hit = hits
        .iter()
        .find(|hit| hit.id == new_id)
        .expect("复用槽位的新节点应可由 QuIVer 正确召回");
    assert_eq!(new_hit.payload["owner"], "new");
    assert!(hits.iter().any(|hit| hit.id == stable_id));

    drop(db);
    cleanup(&path);
}

#[test]
fn 向量更新_查询只能看到完整旧排名或完整新排名() {
    let path = tmp_db("vector_update");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let first = db
        .insert(&[1.0, 0.0, 0.0, 0.0], json!({"name": "first"}))
        .unwrap();
    let second = db
        .insert(&[0.0, 1.0, 0.0, 0.0], json!({"name": "second"}))
        .unwrap();
    let config = exact_config(2);

    let before = db.search_advanced(&[1.0, 0.0, 0.0, 0.0], &config).unwrap();
    assert_eq!(before[0].id, first);
    assert_eq!(before[0].payload["name"], "first");

    db.update_vector(first, &[-1.0, 0.0, 0.0, 0.0]).unwrap();
    db.update_vector(second, &[1.0, 0.0, 0.0, 0.0]).unwrap();

    let after = db.search_advanced(&[1.0, 0.0, 0.0, 0.0], &config).unwrap();
    assert_eq!(after[0].id, second);
    assert_eq!(after[0].payload["name"], "second");
    assert_eq!(after[1].id, first);
    assert_eq!(after[1].payload["name"], "first");

    drop(db);
    cleanup(&path);
}

#[test]
fn 图更新_正向反向入度与标签索引始终联动() {
    let mut mt = MemTable::<f32>::new(DIM);
    mt.insert_with_id(1, &[1.0, 0.0, 0.0, 0.0], json!({}))
        .unwrap();
    mt.insert_with_id(2, &[0.0, 1.0, 0.0, 0.0], json!({}))
        .unwrap();
    mt.insert_with_id(3, &[0.0, 0.0, 1.0, 0.0], json!({}))
        .unwrap();

    mt.link(1, 2, "related".into(), 1.0).unwrap();
    mt.link(3, 2, "related".into(), 0.5).unwrap();
    assert_eq!(mt.get_edges(1).unwrap().len(), 1);
    assert_eq!(mt.get_edges(3).unwrap().len(), 1);
    assert_eq!(mt.get_in_degree(2), 2);
    assert_eq!(mt.get_incoming_sources(2).len(), 2);
    assert_eq!(mt.get_edges_by_label("related").len(), 2);

    mt.unlink(1, 2).unwrap();
    assert!(mt.get_edges(1).is_none_or(|edges| edges.is_empty()));
    assert_eq!(mt.get_in_degree(2), 1);
    assert_eq!(mt.get_incoming_sources(2), &[3]);
    assert_eq!(mt.get_edges_by_label("related"), &[(3, 2)]);

    mt.delete(2).unwrap();
    assert_eq!(mt.get_in_degree(2), 0);
    assert!(mt.get_incoming_sources(2).is_empty());
    assert!(mt.get_edges(3).is_none_or(|edges| edges.is_empty()));
    assert!(mt.get_edges_by_label("related").is_empty());
}

#[test]
fn 容量规划_ExpectedNodes受内存预算约束且打开失败不污染数据() {
    let path = tmp_db("expected_nodes_budget");
    cleanup(&path);
    let error = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: 1024,
            expected_nodes: Some(1_000_000),
            memory_limit: 1024,
            ..Default::default()
        },
    )
    .err()
    .expect("超预算 expected_nodes 必须拒绝");
    assert!(matches!(
        error,
        TriviumError::CapacityReservationRejected { .. }
    ));
    assert!(!std::path::Path::new(&path).exists());
    cleanup(&path);
}

#[test]
fn 容量规划_ReserveNodes零值与溢出均防呆拒绝() {
    let path = tmp_db("reserve_invalid");
    cleanup(&path);
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert!(matches!(
        db.reserve_nodes(0),
        Err(TriviumError::InvalidInput(_))
    ));
    assert!(matches!(
        db.reserve_nodes(usize::MAX),
        Err(TriviumError::CapacityAllocationFailed { .. })
            | Err(TriviumError::CapacityReservationRejected { .. })
    ));
    assert_eq!(db.node_count(), 0);
    drop(db);
    cleanup(&path);
}

#[test]
fn 容量规划_事务预留拒绝前后WAL与代际完全不变() {
    let path = tmp_db("reserve_wal_atomicity");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.set_memory_limit(1);
    let wal_before = std::fs::metadata(format!("{path}.wal"))
        .map(|meta| meta.len())
        .unwrap_or(0);
    let mut tx = triviumdb::database::TxBuilder::new();
    tx.insert(&[1.0, 0.0, 0.0, 0.0], json!({"id": 1}));
    let error = db.commit_tx(tx).unwrap_err();
    assert!(matches!(
        error,
        TriviumError::CapacityReservationRejected { .. }
    ));
    let wal_after = std::fs::metadata(format!("{path}.wal"))
        .map(|meta| meta.len())
        .unwrap_or(0);
    assert_eq!(wal_after, wal_before);
    assert_eq!(db.node_count(), 0);
    db.set_memory_limit(0);
    assert_eq!(db.insert(&[1.0, 0.0, 0.0, 0.0], json!({})).unwrap(), 1);
    drop(db);
    cleanup(&path);
}

#[test]
fn 容量规划_批次中非法向量不得产生半批数据() {
    let path = tmp_db("batch_validation_atomicity");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let mut tx = triviumdb::database::TxBuilder::new();
    tx.insert(&[1.0, 0.0, 0.0, 0.0], json!({"id": 1}));
    tx.insert(&[f32::NAN, 0.0, 0.0, 0.0], json!({"id": 2}));
    assert!(db.commit_tx(tx).is_err());
    assert_eq!(db.node_count(), 0);
    assert_eq!(db.insert(&[1.0, 0.0, 0.0, 0.0], json!({})).unwrap(), 1);
    drop(db);
    cleanup(&path);
}

#[test]
fn 容量规划_已有Mmap库ExpectedNodes按目标总量补足() {
    let path = tmp_db("expected_nodes_existing_mmap");
    cleanup(&path);
    {
        let mut db = Database::<half::f16>::open(&path, 8).unwrap();
        let vector = vec![half::f16::from_f32(1.0); 8];
        for id in 1..=32u64 {
            db.insert_with_id(id, &vector, json!({"id": id})).unwrap();
        }
        db.flush().unwrap();
    }
    let mut db = Database::<half::f16>::open_with_config(
        &path,
        Config {
            dim: 8,
            expected_nodes: Some(64),
            memory_limit: 1024 * 1024,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(db.node_count(), 32);
    let vector = vec![half::f16::from_f32(0.5); 8];
    for id in 33..=64u64 {
        db.insert_with_id(id, &vector, json!({"id": id})).unwrap();
    }
    assert_eq!(db.node_count(), 64);
    drop(db);
    cleanup(&path);
}

#[test]
fn Flush不得隐式构建QuIVer索引() {
    let path = tmp_db("flush_without_quiver");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for id in 1..=10_001u64 {
        db.insert_with_id(id, &[id as f32, 1.0, 0.0, 0.0], json!({"id": id}))
            .unwrap();
    }

    db.flush().unwrap();
    assert!(
        !std::path::Path::new(&format!("{path}.quiver")).exists(),
        "纯持久化 flush 不得隐式构建或写出 QuIVer"
    );
    assert!(
        std::path::Path::new(&format!("{path}.vec")).exists(),
        "Mmap flush 仍须正确写出向量文件"
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn QuIVer构建峰值预算_显式构建应在分配前拒绝() {
    let path = tmp_db("quiver_memory_gate");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for id in 1..=128u64 {
        db.insert_with_id(id, &[id as f32, 1.0, -1.0, 0.5], json!({"id": id}))
            .unwrap();
    }
    db.set_memory_limit(1);

    let error = db.build_quiver_index(None).unwrap_err().to_string();
    assert!(error.contains("预计峰值"));
    assert!(error.contains("内存上限"));
    assert!(
        !std::path::Path::new(&format!("{path}.quiver")).exists(),
        "预算拒绝后不得发布半成品索引"
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn QuIVer构建峰值预算_自动构建应返回明确错误() {
    let path = tmp_db("quiver_auto_memory_gate");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for id in 1..=10_001u64 {
        db.insert_with_id(id, &[id as f32, 1.0, -1.0, 0.5], json!({"id": id}))
            .unwrap();
    }
    db.set_memory_limit(1);

    let error = db
        .search(&[1.0, 1.0, -1.0, 0.5], 10, 0, -1.0)
        .unwrap_err()
        .to_string();
    assert!(error.contains("自动构建预计峰值"));
    assert!(error.contains("内存上限"));

    drop(db);
    cleanup(&path);
}

#[test]
fn QuIVer流式签名构建_FP16无需全量FP32副本仍可检索() {
    let path = tmp_db("quiver_f16_streaming");
    cleanup(&path);
    let mut db = Database::<half::f16>::open(&path, 8).unwrap();
    for id in 1..=256u64 {
        let mut vector = vec![half::f16::from_f32(0.0); 8];
        vector[(id as usize) % 8] = half::f16::from_f32(1.0);
        db.insert_with_id(id, &vector, json!({"id": id})).unwrap();
    }

    db.build_quiver_index(None).unwrap();
    let query = vec![half::f16::from_f32(1.0); 8];
    let results = db.search(&query, 10, 0, -1.0).unwrap();
    assert_eq!(results.len(), 10);
    assert!(results.iter().all(|hit| (1..=256).contains(&hit.id)));

    drop(db);
    cleanup(&path);
}

#[test]
fn BQ重建_重复准备后容量与内容保持稳定() {
    let mut mt = MemTable::<half::f16>::new(130);
    let vector = vec![half::f16::from_f32(1.0); 130];
    for id in 1..=64u64 {
        mt.insert_with_id(id, &vector, json!({"id": id})).unwrap();
    }

    mt.prepare_persistence_cache(false);
    let first = mt.bq_signatures_slice().to_vec();
    mt.prepare_persistence_cache(false);
    assert_eq!(mt.bq_signatures_slice(), first.as_slice());
    assert_eq!(mt.bq_signatures_slice().len(), 64);
}

#[test]
fn 串行提交基线_多线程写入后Flush重载不丢提交() {
    let path = tmp_db("writers_reload");
    cleanup(&path);
    let db = Arc::new(Mutex::new(Database::<f32>::open(&path, DIM).unwrap()));
    let barrier = Arc::new(Barrier::new(8));
    let mut threads = Vec::new();

    for writer in 0..8u64 {
        let db = Arc::clone(&db);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            for seq in 0..50u64 {
                let id = writer * 1_000 + seq + 1;
                db.lock()
                    .unwrap()
                    .insert_with_id(
                        id,
                        &[writer as f32, seq as f32, 1.0, 0.0],
                        json!({"writer": writer, "seq": seq}),
                    )
                    .unwrap();
            }
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }

    let mutex = match Arc::try_unwrap(db) {
        Ok(mutex) => mutex,
        Err(_) => panic!("所有写线程必须已结束"),
    };
    let mut db = mutex.into_inner().unwrap();
    assert_eq!(db.node_count(), 400);
    db.flush().unwrap();
    drop(db);

    let reopened = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(reopened.node_count(), 400);
    for writer in 0..8u64 {
        for seq in 0..50u64 {
            let id = writer * 1_000 + seq + 1;
            let node = reopened.get(id).expect("所有成功提交的节点都必须可恢复");
            assert_eq!(node.payload["writer"], writer);
            assert_eq!(node.payload["seq"], seq);
            assert_eq!(node.vector, [writer as f32, seq as f32, 1.0, 0.0]);
        }
    }

    drop(reopened);
    cleanup(&path);
}
