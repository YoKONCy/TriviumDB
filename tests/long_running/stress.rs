#![allow(non_snake_case)]
//! 极端压力测试与边缘条件 (Fuzzing / Stress / Edge Cases)
//! 向 SQLite 的 TH3 严苛测试看齐，模拟现实中的变态业务场景：
//! - 多线程读写并发（并发压测）
//! - 极端病态向量（全零向量导致的除零 NaN 问题）
//! - 图谱极度自环与重边
//! - 狂暴随机写入后的图谱与数据一致性验证

use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use triviumdb::database::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("stress_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

// ════════ 1. 物理并发安全性测试 ════════

#[test]
fn 测试_并发读写安全_单写多读互不恐慌() {
    let path = tmp_db("concurrent_rw");
    cleanup(&path);

    let db = Arc::new(Mutex::new(Database::<f32>::open(&path, DIM).unwrap()));

    // 初始化基座
    let id_base = db
        .lock()
        .unwrap()
        .insert(&[1.0, 1.0, 1.0, 1.0], serde_json::json!({"init": true}))
        .unwrap();

    let mut handles = vec![];
    let barrier = Arc::new(Barrier::new(6)); // 1 写 + 5 读

    // Spawn 5 Reader threads
    for _ in 0..5 {
        let db_clone = db.clone();
        let b_clone = barrier.clone();
        handles.push(thread::spawn(move || {
            b_clone.wait(); // 对齐起跑线
            let mut reads = 0;
            // 疯狂读取
            for _ in 0..1_000 {
                let lock = db_clone.lock().unwrap();
                let _ = lock.get(id_base);
                let _ = lock.search(&[1.0, 1.0, 1.0, 1.0], 5, 0, 0.0);
                drop(lock);
                reads += 1;
            }
            reads
        }));
    }

    // Spawn 1 Writer thread
    let db_write = db.clone();
    let b_write = barrier.clone();
    let writer_handle = thread::spawn(move || {
        b_write.wait(); // 对齐起跑线
        let mut writes = 0;
        for j in 0..500 {
            let mut lock = db_write.lock().unwrap();
            let _ = lock.insert(&[j as f32, 0.0, 0.0, 0.0], serde_json::json!({"seq": j}));
            if j % 50 == 0 {
                // 定期触发自动压缩重组（最危险的系统底层锁竞争区域）
                lock.compact().unwrap();
            }
            drop(lock);
            writes += 1;
        }
        writes
    });

    // 等待所有线程完成
    for h in handles {
        h.join().unwrap();
    }
    writer_handle.join().unwrap();

    assert_eq!(
        db.lock().unwrap().node_count(),
        501,
        "高速并发读写混合后，节点数量应完全正确且无丢失"
    );

    cleanup(&path);
}

// ════════ 2. 病态数学边界测试 ════════

#[test]
fn 测试_极端病态向量_零向量不得引发NaN() {
    let path = tmp_db("zero_vector");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 尝试插入完全是 0 的向量。全 0 向量在计算 Cosine Similarity 时分母会变成 0，导致 NaN。
    // 我们必须确保计算层进行了安全截断或者直接返回 0.0 得分，而不是让整个查询挂掉甚至泄漏 NaN。
    let zero_id = db
        .insert(&[0.0, 0.0, 0.0, 0.0], serde_json::json!({"type": "zero"}))
        .unwrap();
    let normal_id = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"type": "normal"}))
        .unwrap();

    // 使用正常查询向量去找，看零向量是否引发搜索崩溃
    let results = db.search(&[1.0, 0.0, 0.0, 0.0], 10, 0, -1.0).unwrap();

    // 我们期望返回：第一名肯定是 normal_id。
    let normal_hit = results.iter().find(|h| h.id == normal_id).unwrap();
    assert!(normal_hit.score > 0.99, "正常节点必须精确匹配");

    // 对于零向量，必须不能 panic！并且如果搜出来了，不能带有 NaN 的分数影响排序
    let zero_hit = results.iter().find(|h| h.id == zero_id);
    if let Some(h) = zero_hit {
        assert!(!h.score.is_nan(), "系统得分绝对不允许泄漏 NaN 破坏引擎");
        assert_eq!(h.score, 0.0, "除零安全短路应返回绝对 0.0");
    }

    cleanup(&path);
}

// ════════ 3. 病态图结构测试 ════════

#[test]
fn 测试_图谱自环与重边_极度震荡防雪崩() {
    let path = tmp_db("graph_pathological");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let id1 = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "A"}))
        .unwrap();
    let id2 = db
        .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "B"}))
        .unwrap();

    // 疯狂的自我循环 (Self Loop)
    db.link(id1, id1, "self_love", 10.0).unwrap();
    db.link(id1, id1, "self_love", 20.0).unwrap();

    // 疯狂的重复连接 (Duplicate Edges)
    for _ in 0..100 {
        db.link(id1, id2, "spam", 1.0).unwrap();
        db.link(id2, id1, "spam_back", 1.0).unwrap();
    }

    // 执行带衰减的图发散（扩散深度 = 6，模拟极度震荡循环回声）
    let search_config = triviumdb::database::SearchConfig {
        expand_depth: 6,
        top_k: 10,
        ..Default::default()
    };
    let hits = db
        .search_advanced(&[1.0, 0.0, 0.0, 0.0], &search_config)
        .unwrap();

    assert!(
        !hits.is_empty(),
        "必须能返回结果，绝对不能被重边死循环或者耗尽调用栈"
    );

    // 取出来的值应该是正常数字，不能因为能量过度震荡出现 Infinity
    for h in hits {
        assert!(
            !h.score.is_infinite() && !h.score.is_nan(),
            "在狂暴循环的图中能量不能溢出到 Infinity: Node {}, Score {}",
            h.id,
            h.score
        );
    }

    cleanup(&path);
}

// ════════ 4. WAL 疯狂崩溃切片回归 ════════

#[test]
fn 测试_暴力中断随机重放_多轮假死不丢数据() {
    let path = tmp_db("fuzz_recovery");
    cleanup(&path);

    let mut total_inserted = 0;

    // 模拟应用反复崩溃重启 4 次，每次在 WAL 里疯狂写很多没来得及 flush 的东西就“停电”
    for loop_i in 0..4 {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        assert_eq!(
            db.node_count(),
            total_inserted,
            "第 {} 次重载，节点数量必须与坠毁前强一致",
            loop_i
        );

        let mut tx = db.begin_tx();
        for j in 0..200 {
            tx.insert(
                &[j as f32, loop_i as f32, 0.0, 0.0],
                serde_json::json!({"c": j}),
            );
        }
        tx.commit().unwrap();

        total_inserted += 200;

        // 故意不 db.flush()，直接通过作用域 Drop 掉 db。模拟断电杀进程，全靠 WAL 保命。
    }

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        db.node_count(),
        800,
        "多轮暴力中断后，所有通过 WAL 预写追加的数据必须一粒不落地找回"
    );

    cleanup(&path);
}

// ════════ 5. ID 单调递增绝对保证 ════════

#[test]
fn 测试_ID单调性_跨越删除和WAL回放后绝不复用() {
    let path = tmp_db("id_monotonic");
    cleanup(&path);

    let mut prev_max_id = 0u64;

    // 模拟 3 轮"插入→删除→断电→重启"，每轮都检查新 ID 是否严格大于历史最大值
    for round in 0..3 {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();

        let id = db
            .insert(
                &[round as f32, 0.0, 0.0, 0.0],
                serde_json::json!({"round": round}),
            )
            .unwrap();
        assert!(
            id > prev_max_id,
            "第 {} 轮: 新 ID ({}) 必须严格大于历史最大 ID ({})",
            round,
            id,
            prev_max_id
        );
        prev_max_id = id;

        // 删除刚插入的节点
        db.delete(id).unwrap();
        // 不 flush，直接 Drop（模拟断电）
    }

    // 最终再开一次，确认 ID 分配器已经向前推进
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let final_id = db
        .insert(&[9.9, 9.9, 9.9, 9.9], serde_json::json!({"final": true}))
        .unwrap();
    assert!(
        final_id > prev_max_id,
        "WAL 多轮回放后 ID 分配器必须持续走高，不能回退复用"
    );

    cleanup(&path);
}

// ════════ 6. Mmap / Rom 双模式热切换持久化完整性 ════════

#[test]
fn 测试_Mmap到Rom热切换_数据不丢不裂() {
    use triviumdb::database::{Config, StorageMode};

    let path = tmp_db("mode_switch_mmap_to_rom");
    cleanup(&path);

    // 第 1 阶段：Mmap 模式写入并 flush
    {
        let config = Config {
            dim: DIM,
            storage_mode: StorageMode::Mmap,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        let mut tx = db.begin_tx();
        for i in 0..50 {
            tx.insert(
                &[i as f32, 0.0, 0.0, 0.0],
                serde_json::json!({"mode": "mmap", "i": i}),
            );
        }
        tx.commit().unwrap();
        db.flush().unwrap();
    }

    // 第 2 阶段：用 Rom 模式重新打开同一库——引擎应自动读取 + 重组
    {
        let config = Config {
            dim: DIM,
            storage_mode: StorageMode::Rom,
            ..Default::default()
        };
        let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
        assert_eq!(
            db.node_count(),
            50,
            "从 Mmap 切换到 Rom 后，节点数量必须一致"
        );

        // 验证数据可读
        let p = db.get_payload(1).unwrap();
        assert_eq!(p["mode"], "mmap");

        // 在 Rom 模式下继续写入新数据
        db.insert(
            &[99.0, 99.0, 99.0, 99.0],
            serde_json::json!({"mode": "rom"}),
        )
        .unwrap();
        db.flush().unwrap();
    }

    // 第 3 阶段：再切回 Mmap 检查所有 51 条都在
    {
        let config = Config {
            dim: DIM,
            storage_mode: StorageMode::Mmap,
            ..Default::default()
        };
        let db = Database::<f32>::open_with_config(&path, config).unwrap();
        assert_eq!(db.node_count(), 51, "双向模式热切换后数据必须完好无损");
    }

    cleanup(&path);
}

// ════════ 7. 图谱边存活一致性（删除节点后悬空边不得返回幽灵数据）════════

#[test]
fn 测试_悬空边防御_删除节点后不得返回幽灵搜索结果() {
    let path = tmp_db("dangling_edge");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    let id_a = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "A"}))
        .unwrap();
    let id_b = db
        .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({"name": "B"}))
        .unwrap();
    let id_c = db
        .insert(&[0.0, 0.0, 1.0, 0.0], serde_json::json!({"name": "C"}))
        .unwrap();

    db.link(id_a, id_b, "follows", 1.0).unwrap();
    db.link(id_b, id_c, "follows", 1.0).unwrap();

    // 删掉 B（中间桥梁）
    db.delete(id_b).unwrap();

    // 向量搜索不应返回已删除的 B
    let results = db.search(&[0.0, 1.0, 0.0, 0.0], 10, 0, -1.0).unwrap();
    for hit in &results {
        assert_ne!(hit.id, id_b, "已删除节点 B 绝不允许出现在向量检索结果中");
    }

    // 图谱扩散搜索也不应返回 B
    let config = triviumdb::database::SearchConfig {
        expand_depth: 3,
        top_k: 10,
        ..Default::default()
    };
    let expanded = db.search_advanced(&[1.0, 0.0, 0.0, 0.0], &config).unwrap();
    for hit in &expanded {
        assert_ne!(hit.id, id_b, "已删除节点 B 在图谱扩散中也绝不允许出现");
    }

    cleanup(&path);
}

// ════════ 8. 空库极端操作不得恐慌 ════════

#[test]
fn 测试_空库极端操作_一切安全返回() {
    let path = tmp_db("empty_db_ops");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 对空库发起搜索
    let results = db.search(&[1.0, 0.0, 0.0, 0.0], 10, 0, 0.0).unwrap();
    assert_eq!(results.len(), 0, "空库搜索必须返回空列表，不能 panic");

    // 对空库删除不存在的 ID
    let del_res = db.delete(99999);
    assert!(del_res.is_err(), "删除不存在的 ID 必须返回错误");

    // 对空库获取不存在的 payload
    let payload = db.get_payload(99999);
    assert!(payload.is_none(), "空库 get_payload 应返回 None");

    // 对空库执行 compact
    db.compact().unwrap(); // 必须安全

    // 对空库 flush
    db.flush().unwrap(); // 必须安全

    // 空事务
    let ids = db.begin_tx().commit().unwrap();
    assert!(ids.is_empty());

    assert_eq!(db.node_count(), 0);
    cleanup(&path);
}

// ════════ 9. 反复删光再插入（ID 不重用 + 墓碑对齐连续完整性）════════

#[test]
fn 测试_反复全删全插_墓碑与ID分配器绝不错位() {
    let path = tmp_db("wipeout_cycle");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let mut highest_id = 0u64;

    // 重复 5 轮："插入 100 个 → 全部删掉 → 验证"
    for cycle in 0..5 {
        let mut ids = vec![];
        for j in 0..100 {
            let id = db
                .insert(
                    &[cycle as f32, j as f32, 0.0, 0.0],
                    serde_json::json!({"cycle": cycle, "j": j}),
                )
                .unwrap();
            assert!(
                id > highest_id,
                "第 {} 轮第 {} 条: ID={} 必须大于历史最大 {}",
                cycle,
                j,
                id,
                highest_id
            );
            highest_id = id;
            ids.push(id);
        }
        assert_eq!(db.node_count(), 100, "第 {} 轮插入后应有 100 节点", cycle);

        // 全部删除
        for &id in &ids {
            db.delete(id).unwrap();
        }
        assert_eq!(db.node_count(), 0, "第 {} 轮全删后应归零", cycle);

        // 压实，清理墓碑
        db.compact().unwrap();
    }

    // 最终再插入 1 条，确认整个系统依然完好如初
    let final_id = db
        .insert(
            &[42.0, 42.0, 42.0, 42.0],
            serde_json::json!({"survived": true}),
        )
        .unwrap();
    assert!(final_id > highest_id);
    assert_eq!(db.node_count(), 1);

    cleanup(&path);
}
