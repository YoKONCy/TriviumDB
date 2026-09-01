#![allow(non_snake_case)]
//! 核心数据结构属性测试 (Property-Based Testing)
//!
//! 使用 proptest 随机生成输入，验证以下不变量：
//!
//! 1. MemTable CRUD 不变量：任意操作序列后 node_count 一致
//! 2. Filter::from_json 往返一致性：合法 JSON → Filter → matches 不 panic
//! 3. VectorType::similarity 数学契约：余弦相似度 ∈ [-1, 1]
//! 4. WAL 序列化/反序列化幂等性
//! 5. Transaction 原子性：commit 全成功或全失败

use proptest::prelude::*;
use serde_json::json;

// ═══════════════════════════════════════════════════════════════
//  1. MemTable CRUD 操作序列不变量
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
enum MemOp {
    Insert(Vec<f32>),
    Delete(u64),
}

fn arb_vector(dim: usize) -> BoxedStrategy<Vec<f32>> {
    prop::collection::vec(-10.0f32..10.0f32, dim..=dim).boxed()
}

fn arb_memop(dim: usize) -> BoxedStrategy<MemOp> {
    prop_oneof![
        arb_vector(dim).prop_map(MemOp::Insert),
        (1u64..50).prop_map(MemOp::Delete),
    ]
    .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// 不变量：执行任意 insert/delete 序列后，node_count 应等于实际存活的节点数
    #[test]
    fn memtable_crud_node_count_不变量(ops in prop::collection::vec(arb_memop(4), 1..30)) {
        use triviumdb::storage::memtable::MemTable;
        let mut mt = MemTable::<f32>::new(4);
        let mut alive = std::collections::HashSet::new();

        for op in ops {
            match op {
                MemOp::Insert(vec) => {
                    if let Ok(id) = mt.insert(&vec, json!({})) {
                        alive.insert(id);
                    }
                }
                MemOp::Delete(id) => {
                    if mt.contains(id) {
                        let _ = mt.delete(id);
                        alive.remove(&id);
                    }
                }
            }
        }
        prop_assert_eq!(mt.node_count(), alive.len(),
            "node_count ({}) != alive set size ({})", mt.node_count(), alive.len());
    }

    /// 不变量：插入后 get_payload 一定返回 Some，delete 后一定返回 None
    #[test]
    fn memtable_insert_get_delete_一致性(
        vectors in prop::collection::vec(arb_vector(3), 1..20)
    ) {
        use triviumdb::storage::memtable::MemTable;
        let mut mt = MemTable::<f32>::new(3);
        let mut ids = Vec::new();

        for vec in &vectors {
            let id = mt.insert(vec, json!({"t": true})).unwrap();
            ids.push(id);
        }

        // 所有插入的节点都应存在
        for &id in &ids {
            prop_assert!(mt.contains(id), "插入后节点 {} 应存在", id);
            prop_assert!(mt.get_payload(id).is_some(), "插入后 payload {} 应存在", id);
        }

        // 删除一半
        for &id in ids.iter().step_by(2) {
            mt.delete(id).unwrap();
            prop_assert!(!mt.contains(id), "删除后节点 {} 不应存在", id);
            prop_assert!(mt.get_payload(id).is_none(), "删除后 payload {} 不应存在", id);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  2. Filter::from_json + matches 不 panic
// ═══════════════════════════════════════════════════════════════

fn arb_filter_json() -> BoxedStrategy<serde_json::Value> {
    let leaf = prop_oneof![
        // 隐式 eq
        Just(json!({"name": "Alice"})),
        Just(json!({"age": {"$gt": 18}})),
        Just(json!({"age": {"$gte": 0}})),
        Just(json!({"age": {"$lt": 100}})),
        Just(json!({"age": {"$lte": 50}})),
        Just(json!({"x": {"$eq": "hello"}})),
        Just(json!({"x": {"$ne": 42}})),
        Just(json!({"x": {"$in": [1, 2, 3]}})),
        Just(json!({"x": {"$nin": ["a", "b"]}})),
        Just(json!({"x": {"$exists": true}})),
        Just(json!({"x": {"$exists": false}})),
        Just(json!({"arr": {"$size": 3}})),
        Just(json!({"arr": {"$all": [1, 2]}})),
        Just(json!({"val": {"$type": "string"}})),
    ];
    leaf.boxed()
}

fn arb_payload() -> BoxedStrategy<serde_json::Value> {
    prop_oneof![
        Just(json!({})),
        Just(json!({"name": "Alice", "age": 25})),
        Just(json!({"x": "hello", "arr": [1, 2, 3]})),
        Just(json!({"val": 42, "x": true})),
        Just(json!({"age": 0, "x": null})),
    ]
    .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// 不变量：from_json 成功解析的 Filter 对任意 payload 调用 matches 绝不 panic
    #[test]
    fn filter_matches_绝不panic(
        filter_json in arb_filter_json(),
        payload in arb_payload()
    ) {
        use triviumdb::Filter;
        if let Ok(f) = Filter::from_json(&filter_json) {
            // 只要不 panic 就算通过
            let _ = f.matches(&payload);
            // extract_must_have_mask 也不应 panic
            let _ = f.extract_must_have_mask();
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  3. VectorType::similarity 数学契约
// ═══════════════════════════════════════════════════════════════

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// 不变量：余弦相似度 ∈ [-1, 1]（允许浮点误差 ε=0.01）
    #[test]
    fn cosine_similarity_范围不变量(
        a in prop::collection::vec(-100.0f32..100.0f32, 1..64),
    ) {
        use triviumdb::VectorType;
        let b = a.clone(); // 自身 similarity 应为 1.0（或 0.0 如果零向量）
        let sim = f32::similarity(&a, &b);

        if a.iter().all(|x| *x == 0.0) {
            prop_assert_eq!(sim, 0.0, "零向量 self-similarity 应为 0.0");
        } else {
            prop_assert!((sim - 1.0).abs() < 0.01,
                "非零向量 self-similarity 应接近 1.0, got {}", sim);
        }
    }

    /// 不变量：similarity(a, b) == similarity(b, a)（对称性）
    #[test]
    fn cosine_similarity_对称性(
        dim in 1usize..32,
        seed_a in 0u64..10000,
        seed_b in 0u64..10000,
    ) {
        use triviumdb::VectorType;
        // 用种子确定性生成向量
        let a: Vec<f32> = (0..dim).map(|i| ((seed_a as f32 + i as f32) * 0.37).sin()).collect();
        let b: Vec<f32> = (0..dim).map(|i| ((seed_b as f32 + i as f32) * 0.73).cos()).collect();

        let sim_ab = f32::similarity(&a, &b);
        let sim_ba = f32::similarity(&b, &a);

        prop_assert!((sim_ab - sim_ba).abs() < 1e-5,
            "similarity 应对称: sim(a,b)={} vs sim(b,a)={}", sim_ab, sim_ba);
    }

    /// 不变量：相似度值始终在 [-1.0 - ε, 1.0 + ε] 范围内
    #[test]
    fn cosine_similarity_绝对范围(
        a in prop::collection::vec(-50.0f32..50.0f32, 2..32),
        b in prop::collection::vec(-50.0f32..50.0f32, 2..32),
    ) {
        use triviumdb::VectorType;
        let dim = a.len().min(b.len());
        let sim = f32::similarity(&a[..dim], &b[..dim]);
        prop_assert!((-1.01..=1.01).contains(&sim),
            "similarity 应在 [-1,1]，got {}", sim);
    }
}

// ═══════════════════════════════════════════════════════════════
//  4. Transaction 原子性不变量
// ═══════════════════════════════════════════════════════════════

fn tmp_path(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("tdb_prop_{}", name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("test.tdb").to_string_lossy().to_string()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// 不变量：事务 commit 全成功 → 所有节点可见；失败 → 数据库状态完全不变
    #[test]
    fn transaction_原子性(
        n_good in 1usize..5,
        inject_bad in proptest::bool::ANY,
    ) {
        use triviumdb::Database;
        let path = tmp_path(&format!("tx_atom_{}", n_good));
        let mut db = Database::<f32>::open(&path, 3).unwrap();

        let initial_count = db.node_count();

        let mut tx = db.begin_tx();
        for _ in 0..n_good {
            tx.insert(&[1.0, 0.0, 0.0], json!({"ok": true}));
        }
        if inject_bad {
            // 注入一个非法操作：NaN 向量
            tx.insert(&[f32::NAN, 0.0, 0.0], json!({}));
        }

        let result = tx.commit();
        if inject_bad {
            // 有坏数据：事务应失败
            prop_assert!(result.is_err(), "含 NaN 的事务应失败");
            prop_assert_eq!(db.node_count(), initial_count,
                "失败的事务不应改变 node_count");
        } else {
            // 无坏数据：事务应成功
            prop_assert!(result.is_ok(), "合法事务应成功");
            let ids = result.unwrap();
            prop_assert_eq!(ids.len(), n_good);
            prop_assert_eq!(db.node_count(), initial_count + n_good);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  5. Edge (link/unlink) 不变量
// ═══════════════════════════════════════════════════════════════

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// 不变量：link(a,b) 后 in_degree(b) 增 1；unlink(a,b) 后 in_degree(b) 减 1
    #[test]
    fn link_unlink_in_degree_不变量(
        n_edges in 1usize..8,
    ) {
        use triviumdb::storage::memtable::MemTable;
        let mut mt = MemTable::<f32>::new(2);

        // 创建 n_edges+1 个节点
        let mut ids = Vec::new();
        for i in 0..=n_edges {
            ids.push(mt.insert(&[i as f32, 0.0], json!({})).unwrap());
        }

        let target = ids[0];
        let mut expected_in_degree = 0;

        // link 所有其他节点到 target
        for &src in &ids[1..] {
            mt.link(src, target, "test".into(), 1.0).unwrap();
            expected_in_degree += 1;
            prop_assert_eq!(mt.get_in_degree(target), expected_in_degree,
                "link 后 in_degree 应为 {}", expected_in_degree);
        }

        // unlink 所有
        for &src in &ids[1..] {
            mt.unlink(src, target).unwrap();
            expected_in_degree -= 1;
            prop_assert_eq!(mt.get_in_degree(target), expected_in_degree,
                "unlink 后 in_degree 应为 {}", expected_in_degree);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  6. WAL 序列化往返不变量
// ═══════════════════════════════════════════════════════════════

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// 不变量：WAL append → read_entries 往返，数据完全一致
    #[test]
    fn wal_往返一致性(
        id in 1u64..1000,
        vec in prop::collection::vec(-10.0f32..10.0f32, 4..=4),
        payload_str in "[a-zA-Z0-9 ]{0,50}",
    ) {
        use triviumdb::storage::wal::{Wal, WalEntry, SyncMode};

        let dir = std::env::temp_dir().join(format!("tdb_wal_prop_{}", id));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.tdb").to_string_lossy().to_string();

        {
            let mut wal = Wal::open_with_sync(&path, SyncMode::Normal).unwrap();
            let entry = WalEntry::Insert::<f32> {
                id,
                vector: vec.clone(),
                payload: json!({"text": payload_str}).to_string(),
            };
            wal.append(&entry).unwrap();
            wal.flush_writer();
        }

        let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
        prop_assert!(!entries.is_empty(), "WAL 应至少有一条记录");

        if let WalEntry::Insert { id: read_id, vector: read_vec, payload: read_payload } = &entries[0] {
            prop_assert_eq!(*read_id, id);
            prop_assert_eq!(read_vec, &vec);
            let parsed: serde_json::Value = serde_json::from_str(read_payload).unwrap();
            prop_assert_eq!(parsed["text"].as_str().unwrap(), payload_str.as_str());
        } else {
            prop_assert!(false, "第一条 WAL 记录应为 Insert");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
