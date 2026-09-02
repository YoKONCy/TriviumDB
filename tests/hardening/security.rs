#![allow(non_snake_case)]
//! TriviumDB 安全性与极端防御机制测试
//!
//! 覆盖范围：
//! - NaN / Inf 毒素向量静默注入拦截
//! - WAL 幽灵事务（未闭合 Tx）的截断与丢弃
//! - WAL 尾部物理损坏（CRC 不匹配/乱码）的截肢与继续写入能力

use std::fs::OpenOptions;
use std::io::Write;
use triviumdb::database::Database;
use triviumdb::storage::wal::WalEntry;

const DIM: usize = 2;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("sec_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

// ════════ 毒素向量拦截 ════════

#[test]
fn 测试_毒素向量拦截_拒绝NaN与Inf() {
    let path = tmp_db("nan_inf");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 正常插入
    assert!(db.insert(&[1.0, 2.0], serde_json::json!({})).is_ok());

    // 尝试插入 NaN
    let res1 = db.insert(&[f32::NAN, 1.0], serde_json::json!({}));
    assert!(res1.is_err(), "应拒绝包含 NaN 的向量");

    // 尝试插入 Infinity
    let res2 = db.insert(&[0.0, f32::INFINITY], serde_json::json!({}));
    assert!(res2.is_err(), "应拒绝包含 Infinity 的向量");

    let mut tx = db.begin_tx();
    tx.insert(&[f32::NEG_INFINITY, 0.0], serde_json::json!({}));
    let res3 = tx.commit();
    assert!(res3.is_err(), "事务模式中也应拒绝包含 -Infinity 的向量");

    cleanup(&path);
}

// ════════ WAL 物理截断与纠错 ════════

/// 写入人工构造的 WAL 记录
fn append_raw_wal_entry<T: serde::Serialize>(path: &str, entry: &WalEntry<T>, corrupt_crc: bool) {
    let wal_path = format!("{}.wal", path);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&wal_path)
        .unwrap();
    if file.metadata().unwrap().len() == 0 {
        file.write_all(b"TVWL").unwrap();
        file.write_all(&triviumdb::storage::wal::WAL_VERSION.to_le_bytes())
            .unwrap();
    }

    let data = bincode::serialize(entry).unwrap();
    let mut checksum = crc32fast::hash(&data);
    if corrupt_crc {
        checksum = checksum.wrapping_add(1); // 人为破坏 CRC
    }

    let len = data.len() as u32;
    file.write_all(&len.to_le_bytes()).unwrap();
    file.write_all(&data).unwrap();
    file.write_all(&checksum.to_le_bytes()).unwrap();
}

#[test]
fn 测试_WAL丢失TxCommit的幽灵事务_被正确截断并保护后续追加() {
    let path = tmp_db("wal_phantom_tx");
    cleanup(&path);

    // 1. 手动构造一个有头无尾的幽灵事务，以及一个后续的正常独立操作，模拟复杂的断电残留
    append_raw_wal_entry::<f32>(&path, &WalEntry::TxBegin { tx_id: 100 }, false);
    append_raw_wal_entry::<f32>(
        &path,
        &WalEntry::Insert {
            id: 1,
            vector: vec![1.0, 1.0],
            payload: "{}".to_string(),
        },
        false,
    );
    // 故意没有 TxCommit!!

    // 2. 第一次恢复：这个挂起的事务应该被彻底丢弃，并且文件被截掉
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        // 因为被截断了，所以 0 个节点
        assert_eq!(db.node_count(), 0, "幽灵事务中的数据应被丢弃");

        // 追加正常数据！如果截断没做好，这个追加会紧随在未闭合的事务后面
        db.insert(&[2.0, 2.0], serde_json::json!({"valid": true}))
            .unwrap();
    }

    // 3. 第二次恢复：验证后面追加的正常数据能够被安全读取，没有被幽灵事务“误吞”
    {
        let db = Database::<f32>::open(&path, DIM).unwrap();
        assert_eq!(db.node_count(), 1, "后续正常追加的数据应该存活");
        let payload = db.get_payload(1).unwrap();
        assert_eq!(payload["valid"], true);
    }

    cleanup(&path);
}

#[test]
fn 测试_WAL遇尾部乱码死字节_精确截除防止污染() {
    let path = tmp_db("wal_corrupted_tail");
    cleanup(&path);

    // 1. 正常的插入
    append_raw_wal_entry::<f32>(
        &path,
        &WalEntry::Insert {
            id: 1,
            vector: vec![1.0, 1.0],
            payload: "{}".to_string(),
        },
        false,
    );

    // 2. 尾部增加几个随机死字节乱码（模拟写了一半的崩溃）
    {
        let wal_path = format!("{}.wal", path);
        let mut file = OpenOptions::new().append(true).open(&wal_path).unwrap();
        file.write_all(&[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
    }

    // 3. 第一次恢复：正常数据保留，乱码被识别并截断
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        assert_eq!(db.node_count(), 1, "前半段正常数据应该被恢复");

        // 紧接着插入新数据
        db.insert(&[3.0, 3.0], serde_json::json!({"safe": "yes"}))
            .unwrap();
    }

    // 4. 第二次恢复：新数据没有被原先的乱码阻挡，完美读取
    {
        let db = Database::<f32>::open(&path, DIM).unwrap();
        assert_eq!(db.node_count(), 2, "乱码被截除，后续的新数据完美恢复");
    }

    cleanup(&path);
}

// ════════ 资源配额与恶意负载测试 ════════

#[test]
fn 测试_大规模写入_安全触发Mmap多次扩容() {
    let path = tmp_db("mmap_resize_pressure");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    // VEC_POOL_INITIAL_CAPACITY 一般很小（比如 1024 或者更少），此处压入上万数据强迫多次 mmap remap
    for i in 0..15000 {
        db.insert(&[i as f32, i as f32], serde_json::json!({"seq": i}))
            .unwrap();
    }
    assert_eq!(
        db.node_count(),
        15000,
        "Mmap 动态扩容（甚至多次翻倍）未导致数据截断"
    );

    // 随机抽查扩容后的数据完好度（ID=15000 的记录是循环中的第 14999 次迭代写入的）
    let p = db.get_payload(15000).unwrap();
    assert_eq!(p["seq"], 14999);

    cleanup(&path);
}

#[test]
fn 测试_密集图谱_万级笛卡尔积防OOM_LazyEvaluation有效性() {
    let path = tmp_db("dense_graph_oom");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 构造一个超级中心节点 (0)
    let hub_id = db
        .insert(&[0.0, 0.0], serde_json::json!({"name": "hub"}))
        .unwrap();

    // 构造 100 个叶子节点 (1..=100)
    let mut leaf_ids = vec![];
    for i in 1..=100 {
        let id = db
            .insert(
                &[i as f32, 0.0],
                serde_json::json!({"name": "leaf", "val": i}),
            )
            .unwrap();
        leaf_ids.push(id);
    }

    // 让所有的叶子都指向中心节点，中心节点也指向所有的叶子！
    {
        let mut tx = db.begin_tx();
        for &leaf in &leaf_ids {
            tx.link(leaf, hub_id, "connects", 1.0);
            tx.link(hub_id, leaf, "connects", 1.0);
        }
        tx.commit().unwrap();
    }

    // 执行一波超级笛卡尔积： MATCH (a)-[:connects]->(b)-[:connects]->(c)
    // 路径有两种：
    // 1. 叶子 (100) -> 中心 (1) -> 叶子 (100) = 10,000 条结果！
    // 2. 中心 (1) -> 叶子 (100) -> 中心 (1) = 100 条结果！
    // 预期总路径 10100 条。
    // 如果没有 LazyEvaluation，会在中间层产生 10100 份巨型 Node 深拷贝，造成严重阻塞内存飙升
    let results = db
        .tql_nodes("MATCH (a)-[:connects]->(b)-[:connects]->(c) RETURN c LIMIT 10100")
        .unwrap();

    assert_eq!(
        results.len(),
        10_100,
        "显式 LIMIT 覆盖默认风险上限后，必须返回预算内的完整笛卡尔积"
    );

    cleanup(&path);
}

// ════════ 文件系统与竞态测试 ════════

#[test]
fn 测试_跨进程文件锁_强硬阻断双重绑定() {
    let path = tmp_db("file_exclusive_lock");
    cleanup(&path);

    // 第一个实例（模拟进程 A）获得锁
    let _db_a = Database::<f32>::open(&path, DIM).unwrap();

    // 第二个实例（模拟偷偷启动的无关进程 B）尝试读写同一文件，必定被立刻拒绝，不会被阻塞住挂起
    let db_b_result = Database::<f32>::open(&path, DIM);

    assert!(
        db_b_result.is_err(),
        "必须阻断！文件已被当前进程内的另一个 Database 句柄（甚至跨进程）独占锁死"
    );

    let err_msg = db_b_result.err().unwrap().to_string();
    assert!(
        err_msg.contains("already opened by another") || err_msg.contains("Database locked"),
        "错误信息不匹配：{}",
        err_msg
    );

    cleanup(&path);
}

#[test]
fn 测试_巨型恶意载荷拦截_防OOM与日志撑爆() {
    let path = tmp_db("giant_payload_rejection");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 生成一个 10MB 的超大僵尸 JSON Payload
    let giant_string = "A".repeat(10 * 1024 * 1024);
    let giant_payload = serde_json::json!({ "story": giant_string });

    // 1. 直写 API 拦截
    let res = db.insert(&[0.1, 0.2], giant_payload.clone());
    assert!(res.is_err(), "10MB 超大直写必须被拦截");
    assert!(res.unwrap_err().to_string().contains("Payload too large"));

    // 2. 事务层面拦截
    let mut tx = db.begin_tx();
    tx.insert(&[0.9, 0.8], giant_payload.clone());
    let tx_res = tx.commit();
    assert!(tx_res.is_err(), "事务提交并未被拦截");
    let err_str = tx_res.unwrap_err().to_string();
    assert!(
        err_str.contains("Payload too large"),
        "不在预期的错误范围内: {}",
        err_str
    );

    // 3. 超大维度向量维度越界拦截 (单次恶意请求 10万 维度)
    let giant_vector = vec![0.5_f32; 100_000];
    let vec_res = db.insert(&giant_vector, serde_json::json!({"status": "evil"}));
    assert!(vec_res.is_err(), "超大维度向量必须被拦截");
    assert!(
        vec_res
            .unwrap_err()
            .to_string()
            .contains("dimension mismatch")
    );

    // 4. 确保引擎依然存活且状态未受污染
    db.insert(&[1.0, 1.0], serde_json::json!({"status": "ok"}))
        .unwrap();
    assert_eq!(
        db.node_count(),
        1,
        "恶意载荷彻底被拦截且未污染任何游标，正常写入不受影响"
    );

    cleanup(&path);
}
