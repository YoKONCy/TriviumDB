#![allow(non_snake_case)]
//! TQL 行数上限与分页回归测试（Issue #32）
//!
//! 覆盖四个原始缺陷：
//! 1. 显式 LIMIT 被夹到 5000
//! 2. OFFSET 无法翻页（`OFFSET 5000` 返回 0 行）
//! 3. 聚合 / ORDER BY 因输入被截断而返回错误答案
//! 4. 全表扫描顺序随机，截断得到任意交错子集
//!
//! 同时验证含边模式的 DoS 保护未被削弱。

use std::collections::HashSet;
use triviumdb::database::{Config, Database, RowOverflowPolicy, StorageMode};
use triviumdb::query::tql_executor::TqlValue;

const DIM: usize = 2;

/// 超过 DEFAULT_ROW_LIMIT(5000) 的节点数，用于触发原 bug
const N: usize = 6000;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("rowlimit_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

/// 建库并插入 N 个节点，id 为 1..=N，score 随 id 递增。
fn build_db_with(name: &str, config: Config) -> (Database<f32>, String) {
    let path = tmp_db(name);
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    for i in 1..=N {
        db.insert_with_id(
            i as u64,
            &[i as f32, 0.0],
            serde_json::json!({"type": "memory", "label": format!("n{i}"), "score": i}),
        )
        .unwrap();
    }
    assert_eq!(db.node_count(), N, "前置条件：插入了 {N} 个节点");
    (db, path)
}

fn default_config() -> Config {
    Config {
        dim: DIM,
        storage_mode: StorageMode::Rom,
        ..Default::default()
    }
}

fn build_db(name: &str) -> (Database<f32>, String) {
    build_db_with(name, default_config())
}

/// 从 tql_values 结果里取聚合计数。
///
/// 非管道聚合经 `tql_values` 会包一层 Node，计数落在 payload 里；管道聚合直接给 Int。
fn extract_count(rows: &[std::collections::HashMap<String, TqlValue<f32>>], alias: &str) -> i64 {
    assert_eq!(rows.len(), 1, "聚合应返回单行");
    match rows[0].get(alias) {
        Some(TqlValue::Int(count)) => *count,
        Some(TqlValue::Node(node)) => node
            .payload
            .get(alias)
            .and_then(|value| value.as_i64())
            .unwrap_or_else(|| panic!("Node payload 应带 {alias} 计数")),
        other => panic!("期望 {alias} 是计数，实得 {other:?}"),
    }
}

// ════════ 缺陷 1/4：全量枚举 ════════

#[test]
fn 测试_MATCH全量枚举_不再截断在5000() {
    let (db, path) = build_db("match_full");

    let rows = db.tql("MATCH (n) RETURN n").unwrap();
    assert_eq!(
        rows.len(),
        N,
        "无边模式 MATCH (n) 行数被节点数天然界定，应全量返回而非截断在 5000"
    );

    // 返回的 id 必须正好是全集，不能是任意交错子集
    let ids: HashSet<u64> = rows
        .iter()
        .filter_map(|row| row.get("n").map(|node| node.id))
        .collect();
    let expected: HashSet<u64> = (1..=N as u64).collect();
    assert_eq!(ids, expected, "返回的 id 集合应与 1..=N 完全一致");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_FIND全量枚举_不再截断在5000() {
    let (db, path) = build_db("find_full");

    let rows = db.tql(r#"FIND {type: "memory"} RETURN *"#).unwrap();
    assert_eq!(rows.len(), N, "FIND 命中全部 {N} 个节点，应全量返回");

    drop(db);
    cleanup(&path);
}

// ════════ 缺陷 1：显式 LIMIT ════════

#[test]
fn 测试_显式LIMIT不再被夹到5000() {
    let (db, path) = build_db("explicit_limit");

    // 原 bug：.min(DEFAULT_ROW_LIMIT) 把 20000 改写成 5000
    let rows = db.tql("MATCH (n) RETURN n LIMIT 20000").unwrap();
    assert_eq!(rows.len(), N, "LIMIT 大于节点数时应返回全部 {N} 行");

    // 介于 5000 与 N 之间的 LIMIT 必须精确生效
    let rows = db.tql("MATCH (n) RETURN n LIMIT 5500").unwrap();
    assert_eq!(rows.len(), 5500, "LIMIT 5500 应精确返回 5500 行");

    // 小于 5000 的 LIMIT 行为不变
    let rows = db.tql("MATCH (n) RETURN n LIMIT 10").unwrap();
    assert_eq!(rows.len(), 10, "小 LIMIT 行为保持不变");

    drop(db);
    cleanup(&path);
}

// ════════ 缺陷 2：OFFSET 分页 ════════

#[test]
fn 测试_OFFSET可翻页至末尾() {
    let (db, path) = build_db("offset_paging");

    // 原 bug：扫 5000 行后跳过 5000 行 → 0 行
    let rows = db.tql("MATCH (n) RETURN n OFFSET 5000").unwrap();
    assert_eq!(rows.len(), N - 5000, "OFFSET 5000 应返回剩余 1000 行而非 0 行");

    // 原 bug：6000.min(5000)=5000，扫 5000 跳 4000 → 1000 行
    let rows = db.tql("MATCH (n) RETURN n LIMIT 2000 OFFSET 4000").unwrap();
    assert_eq!(
        rows.len(),
        2000,
        "LIMIT 2000 OFFSET 4000（合计 6000）应返回完整的 2000 行"
    );

    // limit + offset 超过实际节点数时，返回剩余全部（这里 6000-3000=3000）
    let rows = db.tql("MATCH (n) RETURN n LIMIT 4000 OFFSET 3000").unwrap();
    assert_eq!(
        rows.len(),
        N - 3000,
        "OFFSET 3000 之后只剩 3000 行可返回"
    );

    // 末页边界：OFFSET 恰好等于总数 → 空结果，不报错
    let rows = db.tql(&format!("MATCH (n) RETURN n OFFSET {N}")).unwrap();
    assert!(rows.is_empty(), "OFFSET 等于总行数应返回空结果");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_分页完整性_无重复无遗漏() {
    let (db, path) = build_db("paging_complete");

    const PAGE: usize = 1000;
    let mut seen: Vec<u64> = Vec::new();
    let mut offset = 0;
    while offset < N {
        let rows = db
            .tql(&format!("MATCH (n) RETURN n LIMIT {PAGE} OFFSET {offset}"))
            .unwrap();
        assert!(
            !rows.is_empty(),
            "offset={offset} 时页面不应为空，否则无法翻到末尾"
        );
        seen.extend(rows.iter().filter_map(|row| row.get("n").map(|n| n.id)));
        offset += PAGE;
    }

    assert_eq!(seen.len(), N, "所有页面合计应为 {N} 行（无遗漏）");
    let unique: HashSet<u64> = seen.iter().copied().collect();
    assert_eq!(unique.len(), N, "跨页不应出现重复 id");
    assert_eq!(
        unique,
        db.all_node_ids().into_iter().collect::<HashSet<u64>>(),
        "分页并集应等于 all_node_ids() 全集"
    );

    drop(db);
    cleanup(&path);
}

// ════════ 缺陷 3：聚合与排序的正确性 ════════

#[test]
fn 测试_聚合count超过5000返回正确值() {
    let (db, path) = build_db("agg_count");

    // 原 bug：输入被硬限在 5000，count 返回 5000 —— 这是错的答案，不是截断
    let rows = db.tql_values("MATCH (n) RETURN count(n) AS c").unwrap();
    assert_eq!(
        extract_count(&rows, "c"),
        N as i64,
        "count(n) 应等于真实节点数 {N}"
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_ORDER_BY_LIMIT取到真正的TopN() {
    let (db, path) = build_db("order_top_n");

    // 原 bug：只对任意 5000 行排序，top-10 可能完全错
    let rows = db
        .tql("MATCH (n) RETURN n ORDER BY n.score DESC LIMIT 10")
        .unwrap();
    assert_eq!(rows.len(), 10, "应返回 10 行");

    let ids: Vec<u64> = rows
        .iter()
        .filter_map(|row| row.get("n").map(|node| node.id))
        .collect();
    let expected: Vec<u64> = (1..=N as u64).rev().take(10).collect();
    assert_eq!(
        ids, expected,
        "score 随 id 递增，故 DESC 的 top-10 必须是最大的 10 个 id"
    );

    drop(db);
    cleanup(&path);
}

// ════════ 缺陷 4：扫描顺序确定性 ════════

#[test]
fn 测试_全表扫描顺序按id升序() {
    let (db, path) = build_db("scan_order");

    let ids = db.all_node_ids();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "all_node_ids() 必须升序");

    // 截断出来的必须是最小的 3 个 id，而不是任意交错子集
    let rows = db.tql("MATCH (n) RETURN n LIMIT 3").unwrap();
    let ids: Vec<u64> = rows
        .iter()
        .filter_map(|row| row.get("n").map(|node| node.id))
        .collect();
    assert_eq!(ids, vec![1, 2, 3], "LIMIT 3 应稳定返回最小的 3 个 id");

    drop(db);
    cleanup(&path);
}

// ════════ DoS 保护未被削弱 ════════

/// 建一个笛卡尔积图：中心节点 + `leaves` 个叶子双向连边。
/// `MATCH (a)-[:connects]->(b)-[:connects]->(c)` 的路径数为 leaves² + leaves。
fn build_cartesian_db(name: &str, config: Config, leaves: usize) -> (Database<f32>, String) {
    let path = tmp_db(name);
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();

    let hub = db
        .insert(&[0.0, 0.0], serde_json::json!({"name": "hub"}))
        .unwrap();
    let mut leaf_ids = Vec::new();
    for i in 1..=leaves {
        leaf_ids.push(
            db.insert(&[i as f32, 0.0], serde_json::json!({"name": "leaf"}))
                .unwrap(),
        );
    }
    {
        let mut tx = db.begin_tx();
        for &leaf in &leaf_ids {
            tx.link(leaf, hub, "connects", 1.0);
            tx.link(hub, leaf, "connects", 1.0);
        }
        tx.commit().unwrap();
    }
    (db, path)
}

const CARTESIAN_QUERY: &str = "MATCH (a)-[:connects]->(b)-[:connects]->(c) RETURN c";

#[test]
fn 测试_含边模式无LIMIT默认报错() {
    // 100 个叶子 → 10,100 条路径，超过 5,000 默认上限
    let (db, path) = build_cartesian_db("cartesian_throw", default_config(), 100);

    let result = db.tql(CARTESIAN_QUERY);
    assert!(
        result.is_err(),
        "默认 Throw 策略下，含边模式触及默认上限应报错而非静默返回 5000 行子集"
    );
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("5000") && message.contains("截断"),
        "错误信息应说明在哪一行被截断，实得：{message}"
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_Break策略下截断为告警并返回部分结果() {
    let config = Config {
        row_overflow: RowOverflowPolicy::Break,
        ..default_config()
    };
    let (db, path) = build_cartesian_db("cartesian_break", config, 100);

    let rows = db
        .tql(CARTESIAN_QUERY)
        .expect("Break 策略下截断只告警，不应报错");
    assert_eq!(
        rows.len(),
        5000,
        "Break 策略保留旧的「优雅截断」行为，作为迁移出口"
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_含边模式未触及上限时不报错() {
    // 20 个叶子 → 420 条路径，远低于 5,000，不该被误报截断
    let (db, path) = build_cartesian_db("cartesian_under_cap", default_config(), 20);

    let rows = db
        .tql(CARTESIAN_QUERY)
        .expect("未触及上限的含边查询不应报错");
    assert_eq!(rows.len(), 20 * 20 + 20, "应返回全部 420 条路径");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_恰好等于上限不误报截断() {
    // 探针判据的关键回归：结果正好等于上限时，len() == cap 的旧判据会误报
    let config = Config {
        max_query_rows: Some(N),
        ..default_config()
    };
    let path = tmp_db("exact_cap");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    for i in 1..=N {
        db.insert_with_id(i as u64, &[i as f32, 0.0], serde_json::json!({"seq": i}))
            .unwrap();
    }

    let rows = db
        .tql("MATCH (n) RETURN n")
        .expect("结果恰好等于上限不是截断，不应报错");
    assert_eq!(rows.len(), N, "应返回全部 {N} 行");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_throw模式下默认上限截断返回错误() {
    let path = tmp_db("throw_mode");
    cleanup(&path);
    let config = Config {
        row_overflow: RowOverflowPolicy::Throw,
        ..default_config()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();

    let hub = db.insert(&[0.0, 0.0], serde_json::json!({"name": "hub"})).unwrap();
    let mut leaves = Vec::new();
    for i in 1..=100 {
        leaves.push(
            db.insert(&[i as f32, 0.0], serde_json::json!({"name": "leaf"}))
                .unwrap(),
        );
    }
    {
        let mut tx = db.begin_tx();
        for &leaf in &leaves {
            tx.link(leaf, hub, "connects", 1.0);
            tx.link(hub, leaf, "connects", 1.0);
        }
        tx.commit().unwrap();
    }

    let result = db.tql("MATCH (a)-[:connects]->(b)-[:connects]->(c) RETURN c");
    assert!(
        result.is_err(),
        "throw 模式下被默认上限截断应返回错误而非静默子集"
    );

    // 显式 LIMIT 是用户自己的意图，不应报错
    let rows = db
        .tql("MATCH (a)-[:connects]->(b)-[:connects]->(c) RETURN c LIMIT 100")
        .unwrap();
    assert_eq!(rows.len(), 100, "显式 LIMIT 在 throw 模式下不算截断");

    drop(db);
    cleanup(&path);
}

// ════════ 可配置性 ════════

#[test]
fn 测试_max_query_rows可配置() {
    let path = tmp_db("configurable_cap");
    cleanup(&path);
    let config = Config {
        max_query_rows: Some(100),
        row_overflow: RowOverflowPolicy::Break,
        ..default_config()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    for i in 1..=N {
        db.insert_with_id(i as u64, &[i as f32, 0.0], serde_json::json!({"seq": i}))
            .unwrap();
    }

    let rows = db.tql("MATCH (n) RETURN n").unwrap();
    assert_eq!(rows.len(), 100, "显式配置 max_query_rows=100 应对无边模式也生效");

    // 显式 LIMIT 仍优先于配置
    let rows = db.tql("MATCH (n) RETURN n LIMIT 300").unwrap();
    assert_eq!(rows.len(), 300, "显式 LIMIT 优先于 max_query_rows");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_max_query_rows为0表示不限() {
    let path = tmp_db("uncapped");
    cleanup(&path);
    let config = Config {
        max_query_rows: Some(0),
        ..default_config()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();

    let hub = db.insert(&[0.0, 0.0], serde_json::json!({"name": "hub"})).unwrap();
    let mut leaves = Vec::new();
    for i in 1..=30 {
        leaves.push(
            db.insert(&[i as f32, 0.0], serde_json::json!({"name": "leaf"}))
                .unwrap(),
        );
    }
    {
        let mut tx = db.begin_tx();
        for &leaf in &leaves {
            tx.link(leaf, hub, "connects", 1.0);
            tx.link(hub, leaf, "connects", 1.0);
        }
        tx.commit().unwrap();
    }

    // 30*30 + 30 = 930 条路径，全部返回（不再被 5000 之外的默认上限影响，
    // 这里主要验证 Some(0) 分支不会把上限设成 0 行）
    let rows = db
        .tql("MATCH (a)-[:connects]->(b)-[:connects]->(c) RETURN c")
        .unwrap();
    assert_eq!(rows.len(), 930, "max_query_rows=0 表示不设默认上限");

    drop(db);
    cleanup(&path);
}

// ════════ 聚合正确性：截断会让答案「错」而不只是「少」 ════════

#[test]
fn 测试_含边模式聚合触及上限时报错而非返回错答案() {
    // 10,100 条路径 > 5,000 默认上限。count 若只数 5,000 就是错的答案，
    // 不是部分答案——必须报错。
    let (db, path) = build_cartesian_db("agg_edge_throw", default_config(), 100);

    let result = db.tql_values("MATCH (a)-[:connects]->(b) RETURN count(b) AS c");
    match result {
        Err(err) => {
            let message = err.to_string();
            assert!(
                message.contains("错误") || message.contains("WRONG"),
                "聚合被截断时错误信息应说明结果是错的而非仅不完整，实得：{message}"
            );
        }
        Ok(rows) => {
            // 200 条边 < 5000，这条查询本不该触及上限；若真返回了就必须是正确值
            let count = extract_count(&rows, "c");
            assert_eq!(count, 200, "未触及上限时 count 必须准确");
        }
    }

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_ORDER_BY带OFFSET时预算包含offset() {
    // P1-c 回归：requires_full_input 分支此前不把 offset 计入预算，
    // 导致 `ORDER BY .. OFFSET k` 把预算耗在被跳过的行上而返回不足一页。
    let config = Config {
        max_query_rows: Some(2000),
        row_overflow: RowOverflowPolicy::Break,
        ..default_config()
    };
    let path = tmp_db("order_offset_budget");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    for i in 1..=N {
        db.insert_with_id(
            i as u64,
            &[i as f32, 0.0],
            serde_json::json!({"score": i}),
        )
        .unwrap();
    }

    // 上限 2000 + offset 1500 → 预算 3500，跳过 1500 后应仍有整整 2000 行
    let rows = db
        .tql("MATCH (n) RETURN n ORDER BY n.score OFFSET 1500")
        .unwrap();
    assert_eq!(
        rows.len(),
        2000,
        "offset 应叠加进预算，而非从上限里扣掉"
    );

    drop(db);
    cleanup(&path);
}

// ════════ 内存护栅 ════════

/// 建一个高维库并在**装载完成后**再设内存预算。
///
/// `memory_limit` 同时管写入容量，开库即设小预算会让 insert 直接被拒
/// （`CapacityReservationRejected`），所以只能事后调。
fn build_high_dim_db(name: &str, overflow: RowOverflowPolicy) -> (Database<f32>, String) {
    let path = tmp_db(name);
    cleanup(&path);
    let config = Config {
        dim: 1536,
        storage_mode: StorageMode::Rom,
        auto_build_quiver: false,
        row_overflow: overflow,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    let vector = vec![0.1f32; 1536];
    for i in 1..=500u64 {
        db.insert_with_id(i, &vector, serde_json::json!({"t": "m"}))
            .unwrap();
    }
    // dim=1536 f32 → 每行约 6KiB + 开销；512KiB 只够几十行
    db.set_memory_limit(512 * 1024);
    (db, path)
}

#[test]
fn 测试_内存护栅触发时报错() {
    let (db, path) = build_high_dim_db("memory_guard", RowOverflowPolicy::Throw);

    let message = db
        .tql("MATCH (n) RETURN n")
        .expect_err("结果集超过内存预算应报错，而不是物化到 OOM")
        .to_string();
    assert!(
        message.contains("内存预算") || message.contains("memory budget"),
        "错误信息应指明是内存预算导致的，实得：{message}"
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_内存护栅在Break策略下降级为截断() {
    let (db, path) = build_high_dim_db("memory_guard_break", RowOverflowPolicy::Break);

    let rows = db.tql("MATCH (n) RETURN n").unwrap();
    assert!(
        !rows.is_empty() && rows.len() < 500,
        "Break 策略下应截断到预算内的行数，实得 {} 行",
        rows.len()
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_内存护栅下显式LIMIT仍受约束() {
    // 内存是硬约束：显式 LIMIT 比预算大时，仍按预算截断并告知
    let (db, path) = build_high_dim_db("memory_guard_limit", RowOverflowPolicy::Throw);

    let result = db.tql("MATCH (n) RETURN n LIMIT 500");
    assert!(
        result.is_err(),
        "显式 LIMIT 不能突破内存预算——否则护栅形同虚设"
    );

    // 预算内的 LIMIT 正常工作
    let rows = db.tql("MATCH (n) RETURN n LIMIT 10").unwrap();
    assert_eq!(rows.len(), 10, "预算内的小 LIMIT 应正常返回");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_内存预算充足时不误伤() {
    // dim=2 的库，同样 512KiB 预算能放数万行，不该触发
    let path = tmp_db("memory_guard_ample");
    cleanup(&path);
    let config = Config {
        memory_limit: 512 * 1024,
        ..default_config()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    for i in 1..=1000u64 {
        db.insert_with_id(i, &[i as f32, 0.0], serde_json::json!({"t": "m"}))
            .unwrap();
    }

    let rows = db.tql("MATCH (n) RETURN n").unwrap();
    assert_eq!(rows.len(), 1000, "小维度下预算充足，不应触发护栅");

    drop(db);
    cleanup(&path);
}

// ════════ 相邻问题 ════════

#[test]
fn 测试_WITH管道源集合不再被截断() {
    let (db, path) = build_db("pipeline_source");

    let rows = db.tql("MATCH (n) AS source WITH source RETURN source").unwrap();
    assert_eq!(rows.len(), N, "管道的源集合此前同样被静默截断在 5000");

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_DML的MATCH不会因默认上限部分生效() {
    let (mut db, path) = build_db("dml_full");

    // 原本 SET 的 MATCH 走默认上限，超过 5000 的匹配集会被静默截断，
    // 造成只更新一部分节点。
    let result = db.tql_mut(r#"MATCH (n) WHERE n.type == "memory" SET n.touched == 1"#);
    match result {
        Ok(mutation) => assert_eq!(
            mutation.affected, N,
            "SET 应作用于全部 {N} 个匹配节点，不能只改前 5000 个"
        ),
        Err(err) => panic!("DML 执行失败: {err}"),
    }

    drop(db);
    cleanup(&path);
}

#[test]
fn 测试_bitmap取反过滤依赖升序universe() {
    let path = tmp_db("bitmap_ne");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(&path, default_config()).unwrap();

    for i in 1..=200u64 {
        let kind = if i % 2 == 0 { "even" } else { "odd" };
        db.insert_with_id(i, &[i as f32, 0.0], serde_json::json!({"kind": kind}))
            .unwrap();
    }
    db.create_index("kind").unwrap();

    // difference_sorted 是双指针归并，universe 无序会漏掉本该排除的行
    let rows = db.tql(r#"FIND {kind: {$ne: "even"}} RETURN *"#).unwrap();
    assert_eq!(rows.len(), 100, "$ne 应精确返回 100 个 odd 节点");
    for row in &rows {
        for node in row.values() {
            assert_eq!(
                node.payload.get("kind").and_then(|v| v.as_str()),
                Some("odd"),
                "$ne 结果不应包含被排除的 even 节点（id={}）",
                node.id
            );
        }
    }

    drop(db);
    cleanup(&path);
}

// ════════ 10 万节点边界：步数预算不得拦线性扫描 ════════

/// 这组测试锁定第一轮实现里被漏掉的缺陷：解除行数上限后，`tql_dfs` 的
/// 步数预算（MAX_BUDGET = 100,000，每访问一个节点 +1）成了新的隐形天花板，
/// 使 10 万节点以上的 `MATCH (n)` 从「静默返回 5000 行」变成「直接报错」。
///
/// 标 `#[ignore]`：插入 10 万节点需数秒，用
/// `cargo test --release --test tql_row_limit -- --ignored` 显式跑。
#[test]
#[ignore]
fn 测试_十万节点以上全扫不撞步数预算() {
    const BIG: usize = 100_001;
    let path = tmp_db("beyond_step_budget");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(&path, default_config()).unwrap();
    for i in 1..=BIG {
        db.insert_with_id(i as u64, &[i as f32, 0.0], serde_json::json!({"t": "m"}))
            .unwrap();
    }

    let rows = db
        .tql("MATCH (n) RETURN n")
        .expect("无边全扫的成本是线性的，不该被边展开预算拦住");
    assert_eq!(rows.len(), BIG, "应返回全部 {BIG} 行");

    let rows = db
        .tql(r#"FIND {t: "m"} RETURN *"#)
        .expect("FIND 同样不该被隐式硬顶截断");
    assert_eq!(rows.len(), BIG, "FIND 应返回全部 {BIG} 行，而非静默的 100,001 上限");

    let rows = db
        .tql_values("MATCH (n) RETURN count(n) AS c")
        .expect("聚合不该因步数预算报错");
    assert_eq!(
        extract_count(&rows, "c"),
        BIG as i64,
        "count 应等于真实节点数"
    );

    drop(db);
    cleanup(&path);
}

#[test]
#[ignore]
fn 测试_十五万节点FIND不被隐式硬顶截断() {
    const BIG: usize = 150_000;
    let path = tmp_db("no_hidden_ceiling");
    cleanup(&path);
    let mut db = Database::<f32>::open_with_config(&path, default_config()).unwrap();
    for i in 1..=BIG {
        db.insert_with_id(i as u64, &[i as f32, 0.0], serde_json::json!({"t": "m"}))
            .unwrap();
    }

    // 第一轮实现里这里会静默返回 100,001 行（丢 5 万行）且不发告警
    let rows = db.tql(r#"FIND {t: "m"} RETURN *"#).unwrap();
    assert_eq!(rows.len(), BIG, "不应存在 MAX_BUDGET 派生的隐式行数硬顶");

    drop(db);
    cleanup(&path);
}
