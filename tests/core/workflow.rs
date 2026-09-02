#![allow(non_snake_case)]
//! TriviumDB 业务逻辑链路全景测试
//!
//! 覆盖范围：
//! - 所有对外零散 API 的自由组合与嵌套调用
//! - 事务与原子操作（含 Rollback 链路）的完整生命周期
//! - 向量修改、图谱增删改查跨状态机的连贯性
//! - Vector Search 与 Cypher 筛选的协同混合调用

use triviumdb::database::{Config, Database, SearchConfig, StorageMode};
use triviumdb::filter::Filter;

const DIM: usize = 3;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("workflow_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

// ════════ 完整生命周期测试 ════════

#[test]
fn 测试_全业务链路_社交网络复杂流转() {
    let path = tmp_db("social_network");
    cleanup(&path);

    // 1. 定制化配置初始化
    let config = Config {
        dim: DIM,
        storage_mode: StorageMode::Mmap,
        ..Default::default()
    };
    // 使用默认配置
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();

    let alice_id = 1;
    let bob_id = 2;
    let charlie_id = 3;

    // 2. 第一阶段：多节点基础组装 (事务 Commit)
    {
        let mut tx = db.begin_tx();
        tx.insert_with_id(
            alice_id,
            &[1.0, 0.0, 0.0],
            serde_json::json!({"name": "Alice", "level": 10}),
        );
        tx.insert_with_id(
            bob_id,
            &[0.0, 1.0, 0.0],
            serde_json::json!({"name": "Bob", "level": 20}),
        );
        tx.insert_with_id(
            charlie_id,
            &[0.0, 0.0, 1.0],
            serde_json::json!({"name": "Charlie", "level": 30}),
        );

        // 构建初始关系
        tx.link(alice_id, bob_id, "follows", 1.0);
        tx.link(bob_id, charlie_id, "follows", 0.5);
        tx.commit().expect("第一阶段事务提交失败");
    }

    // 验证初始状态：节点数量应为 3
    assert_eq!(db.node_count(), 3);

    // 3. 第二阶段：变更图谱结构与属性 (事务 Commit)
    {
        let mut tx = db.begin_tx();
        // Alice 修改了她的自身隐空间偏好
        tx.update_vector(alice_id, &[0.8, 0.2, 0.0]);
        // Bob 升级了
        tx.update_payload(bob_id, serde_json::json!({"name": "Bob", "level": 25}));
        // Alice 取消了经过 Bob 甚至直接去 follow Charlie
        tx.link(alice_id, charlie_id, "follows", 0.9);
        // Bob 和 Charlie 断交
        tx.unlink(bob_id, charlie_id);

        tx.commit().expect("第二阶段更新事务失败");
    }

    // 4. 第三阶段：废弃操作测试 (事务 Rollback)
    {
        let mut tx = db.begin_tx();
        let dave_id = 4;
        tx.insert_with_id(
            dave_id,
            &[0.5, 0.5, 0.5],
            serde_json::json!({"name": "Dave"}),
        );
        tx.link(alice_id, dave_id, "follows", 1.0);

        // 突然反悔，没有调用 tx.commit()，直接丢弃（Rust 借用期结束会自动 Drop，但此处我们显式证明不提交便不可见）
        // 实际上什么都不用做，直接走出作用域等于阻断
    }

    // 验证 Rollback 有效：节点数仍然应该是 3
    assert_eq!(db.node_count(), 3);

    // 5. 第四阶段：复杂查询层结合 (Cypher + Vector)
    {
        // a. 纯图谱查询：测试修改后的结构：Alice follow 了谁？(应当是 Bob 和 Charlie)
        let results = db
            .tql_nodes(r#"MATCH (a {name: "Alice"})-[:follows]->(b) RETURN b"#)
            .unwrap();
        assert_eq!(results.len(), 2, "Alice 的两次关注结果应当独立可见");

        // b. 特征过滤查询：从查理的属性入手，且通过 KNN 混合搜索寻找与其最相近的人，通过 Cypher 进行结果倒排过滤
        // 我们搜索一个倾向于 Charlie 方向 [0.0, 0.0, 1.0] 的向量，过滤时要求 Level > 15 才能计入靶心
        let mix_results = db
            .search_hybrid(
                None,
                Some(&[0.0, 0.0, 0.9]),
                &SearchConfig {
                    top_k: 5,
                    min_score: -1.0, // 允许非正相关节点通过，重点测试 Payload 过滤
                    payload_filter: Some(Filter::Gt("level".into(), 15.0)),
                    ..Default::default()
                },
            )
            .unwrap();

        // 应该返回 Charlie 和 Bob，不包含 Alice (Alice 的 level = 10，被拦截；Bob 是 25)
        assert_eq!(mix_results.len(), 2, "混合查询应当过滤掉 level=10 的 Alice");

        // 排序检查，由于 [0.0,0.0,0.9] 和 Charlie [0.0,0.0,1.0] 的距离极近，Charlie 必定是 top1
        assert_eq!(
            mix_results[0].id, charlie_id,
            "距离最近的应当是 Charlie 自身"
        );
    }

    // 6. 第五阶段：销毁与全清 (直接操作层与底层 GC)
    {
        // 管理员封禁 Bob
        db.delete(bob_id).expect("独立接口删除失败");

        // 执行大规模底层空间压实，确保图谱和 QuIVer 索引同时裁剪墓碑完成对齐
        db.compact().expect("压实异常");

        // 验证 Bob 已被彻底清理
        assert!(!db.contains(bob_id), "节点记录应当已拔除");
        assert_eq!(db.node_count(), 2, "此时图谱网络内只剩 2 人");

        // Alice 是否还是 follow Bob?
        let rel_check = db
            .tql_nodes(r#"MATCH (a {name: "Alice"})-[:follows]->(b) RETURN b"#)
            .unwrap();
        // 虽然曾经 Alice -> Bob，并且 Alice -> Charlie。但是 Bob 被删了，只剩 Charlie
        assert_eq!(
            rel_check.len(),
            1,
            "悬空出边应当在 Delete -> Compact 后被彻底摧毁"
        );
        let survior_id = rel_check[0].get("b").unwrap().id;
        assert_eq!(survior_id, charlie_id, "幸存的只有 Charlie");
    }

    cleanup(&path);
}

#[test]
fn 测试_独立写操作API_与无事务修改的一致性() {
    let path = tmp_db("direct_api_mutation");
    cleanup(&path);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 直接操作，不用 BeginTx
    let id1 = db
        .insert(&[1.0, 1.0, 1.0], serde_json::json!({"status": "new"}))
        .unwrap();

    // 直接更新
    db.update_payload(id1, serde_json::json!({"status": "updated"}))
        .unwrap();
    db.update_vector(id1, &[2.0, 2.0, 2.0]).unwrap();

    // 强制落盘校验
    db.flush().unwrap();

    // 验证更新立即穿透生效
    let p = db.get_payload(id1).unwrap();
    assert_eq!(p["status"], "updated");

    let n = db.get(id1).unwrap();
    assert_eq!(n.vector, vec![2.0, 2.0, 2.0]);

    cleanup(&path);
}
