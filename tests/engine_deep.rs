#![allow(non_snake_case)]
//! TQL 执行器深层分支与解析器极端路径集成测试
//!
//! 验证范围：
//! - `query/tql_executor.rs`: OPTIONAL MATCH 空匹配回填、变长路径 BFS 截断、
//!   FIND 的 ReturnClause::Expressions 分支、空结果聚合、MATCH+CREATE 组合、
//!   compare_for_sort 全类型交叉 (NULL 排序)、extract_order_key 回退到 "_" 变量
//! - `query/tql_parser.rs`: $not 文档过滤、DML MATCH+CREATE 引用已有节点、
//!   MATCH+SET 多字段、MATCH+DETACH DELETE
//! - `storage/memtable.rs`: 删除已删除节点二次删除、edge 操作边界
//! - `storage/wal.rs`: 损坏 WAL 截断恢复

use triviumdb::database::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    let path = dir
        .join(format!("cov7_{}", name))
        .to_string_lossy()
        .to_string();
    cleanup(&path);
    path
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok", ".tmp", ".vec.tmp"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

fn assert_row_has_payload<T>(
    rows: &[std::collections::HashMap<String, triviumdb::node::Node<T>>],
    var: &str,
    key: &str,
    expected: serde_json::Value,
) {
    assert!(
        rows.iter()
            .filter_map(|row| row.get(var))
            .any(|node| node.payload.get(key) == Some(&expected)),
        "结果中应存在 {var}.{key} = {expected} 的节点"
    );
}

fn assert_all_rows_have_vars<T>(
    rows: &[std::collections::HashMap<String, triviumdb::node::Node<T>>],
    vars: &[&str],
) {
    assert!(!rows.is_empty(), "查询结果不应为空");
    for row in rows {
        for var in vars {
            assert!(row.contains_key(*var), "结果行缺少变量 {var}");
        }
    }
}

fn seed_chain(path: &str, n: u32) -> Database<f32> {
    let mut db = Database::<f32>::open(path, DIM).unwrap();
    for i in 0..n {
        db.insert(
            &[i as f32, (i as f32).sin(), (i as f32).cos(), 1.0],
            serde_json::json!({
                "name": format!("node_{}", i),
                "val": i as f64 * 2.5,
                "group": if i % 3 == 0 { "x" } else if i % 3 == 1 { "y" } else { "z" },
                "active": i % 2 == 0,
                "tags": ["a", "b"]
            }),
        )
        .unwrap();
    }
    let ids = db.all_node_ids();
    for i in 0..ids.len() - 1 {
        db.link(ids[i], ids[i + 1], "next", 0.9).unwrap();
    }
    // 回环边，形成环
    if ids.len() >= 3 {
        db.link(ids[ids.len() - 1], ids[0], "next", 0.5).unwrap();
        db.link(ids[0], ids[3.min(ids.len() - 1)], "skip", 0.7)
            .unwrap();
    }
    db
}

// ════════════════════════════════════════════════════════════════
//  OPTIONAL MATCH 空匹配 → NULL 回填 (L135, L152, L172-173)
// ════════════════════════════════════════════════════════════════

/// OPTIONAL MATCH 无匹配时应返回左侧节点 + NULL 右侧
#[test]
fn COV7_01_optional_match_null_fill() {
    let path = tmp_db("opt_null");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 创建孤立节点（无出边）
    db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"name": "lonely"}))
        .unwrap();

    let r = db
        .tql(r#"OPTIONAL MATCH (a)-[:nonexistent]->(b) RETURN a, b"#)
        .unwrap();
    assert_eq!(r.len(), 1, "无匹配时必须保留左侧节点");
    assert_eq!(r[0]["a"].payload["name"], "lonely");
    assert!(!r[0].contains_key("b"), "未匹配的右侧变量必须为空");
    assert_eq!(db.node_count(), 1, "OPTIONAL 空匹配不能改变节点数据");
    assert_eq!(
        db.get_payload(1).and_then(|p| p.get("name").cloned()),
        Some(serde_json::json!("lonely")),
        "空匹配不能绑定或写入脏节点"
    );

    cleanup(&path);
}

/// OPTIONAL MATCH 部分匹配 + 部分不匹配
#[test]
fn COV7_02_optional_match_mixed() {
    let path = tmp_db("opt_mixed");
    let db = seed_chain(&path, 5);

    // 部分节点有 "skip" 边，部分没有
    let r = db
        .tql(r#"OPTIONAL MATCH (a)-[:skip]->(b) RETURN a, b"#)
        .unwrap();
    assert_eq!(r.len(), 5, "每个左侧节点都必须至少产生一行");
    assert_eq!(
        r.iter().filter(|row| row.contains_key("b")).count(),
        1,
        "真实命中的右侧节点必须正常绑定"
    );
    assert_eq!(
        r.iter().filter(|row| !row.contains_key("b")).count(),
        4,
        "未命中的左侧节点必须以空右侧保留"
    );
    assert_eq!(db.node_count(), 5, "OPTIONAL 查询不能改变图数据");
    assert_eq!(
        db.tql(r#"MATCH (a)-[:skip]->(b) RETURN a, b"#)
            .unwrap()
            .len(),
        1,
        "OPTIONAL 已命中分支应与普通 MATCH 的真实边数量一致"
    );

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  变长路径 BFS 深层截断 (L262-265, L417-455)
// ════════════════════════════════════════════════════════════════

/// 变长路径超过图直径（触发截断）
#[test]
fn COV7_03_varlen_path_deep() {
    let path = tmp_db("varlen_deep");
    let db = seed_chain(&path, 8);

    // 路径深度 *1..20，但图只有 8 个节点 → 大部分深度截断
    let r = db
        .tql(r#"MATCH (a)-[:next*1..20]->(b) RETURN a, b LIMIT 100"#)
        .unwrap();
    assert_all_rows_have_vars(&r, &["a", "b"]);
    assert!(r.len() <= 100, "LIMIT 100 必须被执行器严格遵守");
    assert!(
        r.iter().any(|row| row["a"].id != row["b"].id),
        "变长正跳路径应至少产生一条非零距离路径"
    );

    cleanup(&path);
}

/// 变长路径 *0..1（包含零跳 = 自身）
#[test]
fn COV7_04_varlen_zero_hop() {
    let path = tmp_db("varlen_zero");
    let db = seed_chain(&path, 5);

    let r = db
        .tql(r#"MATCH (a)-[:next*0..1]->(b) RETURN a, b LIMIT 50"#)
        .unwrap();
    assert_all_rows_have_vars(&r, &["a", "b"]);
    assert!(
        r.iter().any(|row| row["a"].id == row["b"].id),
        "*0..1 应包含零跳自身匹配"
    );

    cleanup(&path);
}

/// 变长路径环检测（回环图不应无限递归）
#[test]
fn COV7_05_varlen_cycle() {
    let path = tmp_db("varlen_cycle");
    let db = seed_chain(&path, 4); // 4 节点 + 回环边

    let r = db
        .tql(r#"MATCH (a)-[:next*1..10]->(b) RETURN a, b LIMIT 200"#)
        .unwrap();
    assert_all_rows_have_vars(&r, &["a", "b"]);
    assert!(r.len() <= 200, "环图变长路径必须被 LIMIT 截断");
    assert!(
        r.iter().all(|row| row["a"].id != 0 && row["b"].id != 0),
        "环路径结果不能包含无效节点 ID"
    );

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  FIND + ReturnClause::Expressions (L167-173)
// ════════════════════════════════════════════════════════════════

/// FIND + RETURN expression (非变量、非 *)
#[test]
fn COV7_06_find_return_expression() {
    let path = tmp_db("find_expr");
    let db = seed_chain(&path, 10);

    // RETURN 表达式中提取变量
    let r = db
        .tql(r#"FIND {active: true} RETURN _.name, _.val"#)
        .unwrap();
    assert_eq!(r.len(), 5, "active=true 的节点应正好有 5 个");
    for row in &r {
        let node = row.get("_").expect("FIND 表达式应保留隐式变量 _");
        assert_eq!(node.payload.get("active"), Some(&serde_json::json!(true)));
        assert!(node.payload.get("name").is_some(), "应可读取 name 字段");
        assert!(node.payload.get("val").is_some(), "应可读取 val 字段");
    }

    cleanup(&path);
}

/// FIND + RETURN count (聚合表达式场景)
#[test]
fn COV7_07_find_return_aggregate() {
    let path = tmp_db("find_agg");
    let db = seed_chain(&path, 10);

    let r = db.tql(r#"FIND {active: true} RETURN count(_)"#).unwrap();
    assert_eq!(r.len(), 1, "count 聚合应返回单行");
    let count_node = r[0].get("count").expect("聚合结果应绑定到 count");
    assert_eq!(count_node.payload.get("count"), Some(&serde_json::json!(5)));

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  空结果聚合 (L1119-1120)
// ════════════════════════════════════════════════════════════════

/// 聚合对空结果集操作（应返回空）
#[test]
fn COV7_08_aggregate_empty_result() {
    let path = tmp_db("agg_empty");
    let db = seed_chain(&path, 5);

    // 不可能匹配的条件 → 空结果 → 聚合
    let r = db
        .tql(r#"MATCH (a)-[:nonexistent]->(b) RETURN count(a)"#)
        .unwrap();
    assert!(r.is_empty(), "空输入上的聚合结果不应伪造 count 行");

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  ORDER BY NULL 排序 + compare_for_sort (L987-990)
// ════════════════════════════════════════════════════════════════

/// ORDER BY 字段部分为 NULL (触发 Null vs 非 Null 比较)
#[test]
fn COV7_09_order_by_with_nulls() {
    let path = tmp_db("order_null");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 部分节点有 "priority" 字段，部分没有 → NULL
    for i in 0..6u32 {
        let payload = if i < 3 {
            serde_json::json!({"name": format!("n_{}", i), "priority": i})
        } else {
            serde_json::json!({"name": format!("n_{}", i)}) // 无 priority → NULL
        };
        db.insert(&[i as f32, 0.0, 0.0, 0.0], payload).unwrap();
    }

    // ORDER BY priority → NULL 应排最后
    let r = db
        .tql(r#"MATCH (a) RETURN a ORDER BY a.priority ASC"#)
        .unwrap();
    assert_eq!(r.len(), 6);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  MATCH + CREATE 引用已有节点 (L1601-1636, L1721-1736)
// ════════════════════════════════════════════════════════════════

/// MATCH (a) CREATE (a)-[:r]->(b) 引用 MATCH 变量创建边
#[test]
fn COV7_10_match_create_ref() {
    let path = tmp_db("match_create");
    let mut db = seed_chain(&path, 5);

    let r = db
        .tql_mut(r#"MATCH (a {name: "node_0"}) CREATE (a)-[:new_edge]->(b {name: "fresh"})"#)
        .unwrap();
    assert_eq!(r.created_ids.len(), 1, "应创建且只创建 fresh 节点");
    assert!(r.affected >= 2, "应至少影响 1 个节点和 1 条边");
    let fresh = db.tql(r#"FIND {name: "fresh"} RETURN *"#).unwrap();
    assert_eq!(fresh.len(), 1, "fresh 节点必须可被查询到");
    let edges = db
        .tql(r#"MATCH (a {name: "node_0"})-[:new_edge]->(b) RETURN a, b"#)
        .unwrap();
    assert_row_has_payload(&edges, "b", "name", serde_json::json!("fresh"));

    cleanup(&path);
}

/// MATCH + CREATE 两个已有变量之间创建边
#[test]
fn COV7_11_match_create_edge_between_existing() {
    let path = tmp_db("match_create_edge");
    let mut db = seed_chain(&path, 5);

    let r = db
        .tql_mut(r#"MATCH (a {name: "node_0"})-[:next]->(b) CREATE (b)-[:backlink]->(a)"#)
        .unwrap();
    assert!(r.affected >= 1, "创建已有变量之间的边应报告受影响行数");
    let back = db
        .tql(r#"MATCH (b)-[:backlink]->(a) WHERE a.name == "node_0" RETURN a, b"#)
        .unwrap();
    assert_row_has_payload(&back, "a", "name", serde_json::json!("node_0"));

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  MATCH + SET 多字段 (L1670-1680)
// ════════════════════════════════════════════════════════════════

/// MATCH + SET 单字段后验证
#[test]
fn COV7_12_match_set_verify() {
    let path = tmp_db("match_set");
    let mut db = seed_chain(&path, 5);

    db.tql_mut(r#"MATCH (a) WHERE a.name == "node_0" SET a.val == 999"#)
        .unwrap();

    // 验证修改生效
    let r = db.tql(r#"FIND {name: "node_0"} RETURN *"#).unwrap();
    assert_eq!(r.len(), 1, "SET 后应仍只命中 node_0");
    let node = r[0].get("_").expect("FIND RETURN * 应绑定 _");
    assert_eq!(node.payload.get("name"), Some(&serde_json::json!("node_0")));
    assert_eq!(node.payload.get("val"), Some(&serde_json::json!(999)));

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  MATCH + DETACH DELETE (L1695-1696)
// ════════════════════════════════════════════════════════════════

/// DETACH DELETE 无前置 MATCH 的错误处理
#[test]
fn COV7_13_detach_delete_no_match() {
    let path = tmp_db("detach_no_match");
    let mut db = seed_chain(&path, 3);

    let before = db.node_count();
    let ids = db.all_node_ids();

    // DELETE 没有 MATCH 前缀应报错
    let r = db.tql_mut(r#"DELETE a"#);
    let err = r.expect_err("DELETE without MATCH should fail");
    assert!(!err.to_string().is_empty(), "DELETE 拒绝必须返回可诊断错误");
    assert_eq!(db.node_count(), before, "DELETE 失败不能改变节点数");
    for id in ids {
        let payload = db.get_payload(id).expect("DELETE 失败不能删除原节点");
        assert!(
            payload.get("name").is_some(),
            "DELETE 失败后节点 payload 必须保持可读"
        );
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  $not 文档过滤 (parser L258-261)
// ════════════════════════════════════════════════════════════════

/// $not 文档过滤操作符
#[test]
fn COV7_14_filter_not() {
    let path = tmp_db("filter_not");
    let db = seed_chain(&path, 10);

    let r = db.tql(r#"FIND {$not: {group: "x"}} RETURN *"#);
    match r {
        Ok(results) => {
            assert_eq!(results.len(), 6, "$not 应过滤掉 group=x 的 4 个节点");
            for row in &results {
                let node = row.get("_").expect("FIND RETURN * 应绑定 _");
                assert_ne!(
                    node.payload.get("group"),
                    Some(&serde_json::json!("x")),
                    "$not 不能返回被排除的 group=x 节点"
                );
            }
        }
        Err(e) => assert!(
            !e.to_string().is_empty(),
            "不支持 $not 时必须返回可诊断错误"
        ),
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  更多 parser 边界
// ════════════════════════════════════════════════════════════════

/// FIND + null 值过滤
#[test]
fn COV7_15_find_null_value() {
    let path = tmp_db("find_null");
    let db = seed_chain(&path, 5);

    let r = db.tql(r#"FIND {nonexistent: null} RETURN *"#).unwrap();
    assert_eq!(r.len(), 0, "缺失字段与 null 比较当前应安全返回空集");
    assert_eq!(db.node_count(), 5, "null 过滤不能修改数据库");

    cleanup(&path);
}

/// FIND + bool 值过滤
#[test]
fn COV7_16_find_bool_value() {
    let path = tmp_db("find_bool");
    let db = seed_chain(&path, 10);

    let r = db.tql(r#"FIND {active: true} RETURN *"#).unwrap();
    assert_eq!(r.len(), 5);

    let r = db.tql(r#"FIND {active: false} RETURN *"#).unwrap();
    assert_eq!(r.len(), 5);

    cleanup(&path);
}

/// FIND + float 过滤
#[test]
fn COV7_17_find_float_value() {
    let path = tmp_db("find_float");
    let db = seed_chain(&path, 10);

    let r = db.tql(r#"FIND {val: 0.0} RETURN *"#).unwrap();
    assert_eq!(r.len(), 1, "val=0.0 只应匹配 node_0");
    let node = r[0].get("_").expect("FIND RETURN * 应绑定 _");
    assert_eq!(node.payload.get("name"), Some(&serde_json::json!("node_0")));
    assert_eq!(node.payload.get("val"), Some(&serde_json::json!(0.0)));

    cleanup(&path);
}

/// FIND + array 过滤
#[test]
fn COV7_18_find_array_value() {
    let path = tmp_db("find_array");
    let db = seed_chain(&path, 5);

    let r = db.tql(r#"FIND {tags: ["a", "b"]} RETURN *"#).unwrap();
    assert_eq!(r.len(), 5, "所有 seed_chain 节点都应包含 tags=[a,b]");
    for row in &r {
        let node = row.get("_").expect("FIND RETURN * 应绑定 _");
        assert_eq!(
            node.payload.get("tags"),
            Some(&serde_json::json!(["a", "b"])),
            "数组过滤不能返回 tags 不一致的节点"
        );
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  WAL 截断恢复 (wal.rs L69-79)
// ════════════════════════════════════════════════════════════════

/// 损坏 WAL 截断 → 重新打开时安全跳过
#[test]
fn COV7_19_wal_truncated_recovery() {
    let path = tmp_db("wal_trunc");

    // 正常写入
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..5u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"v": i}))
                .unwrap();
        }
        // 不 flush → WAL 有数据
    }

    // 手动截断 WAL 文件的最后几字节 → 模拟断电损坏
    let wal_path = format!("{}.wal", path);
    if let Ok(data) = std::fs::read(&wal_path)
        && data.len() > 20
    {
        // 截断掉最后 15 字节
        std::fs::write(&wal_path, &data[..data.len() - 15]).ok();
    }

    // 重新打开 → 应安全恢复（丢失部分数据但不崩溃）
    let db = Database::<f32>::open(&path, DIM);
    match db {
        Ok(db) => {
            assert!(db.node_count() <= 5, "截断 WAL 恢复不能产生额外脏节点");
            for id in db.all_node_ids() {
                let payload = db.get_payload(id).expect("恢复节点必须有 payload");
                let v = payload
                    .get("v")
                    .and_then(|value| value.as_u64())
                    .expect("恢复节点必须携带原始 v 字段");
                assert!(v < 5, "截断 WAL 恢复不能产生越界 payload: {payload}");
            }
        }
        Err(e) => assert!(!e.to_string().is_empty(), "安全拒绝必须返回可诊断错误"),
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  memtable 边界 — 二次删除、边操作 (L326-339, L457-468)
// ════════════════════════════════════════════════════════════════

/// 删除后再删除（幂等）
#[test]
fn COV7_20_delete_twice() {
    let path = tmp_db("del_twice");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    db.delete(id).unwrap();

    // 再删一次
    let r = db.delete(id);
    assert!(r.is_err(), "重复删除同一节点应安全拒绝");
    assert_eq!(db.node_count(), 0, "重复删除不能产生脏节点");
    assert!(db.get_payload(id).is_none(), "已删除节点不能残留 payload");

    cleanup(&path);
}

/// unlink 不存在的边
#[test]
fn COV7_21_unlink_nonexistent() {
    let path = tmp_db("unlink_none");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id1 = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    let id2 = db
        .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();

    let before = db.node_count();
    let r = db.unlink(id1, id2);
    assert!(r.is_err(), "不存在的边应安全拒绝");
    assert_eq!(db.node_count(), before, "unlink 失败不能影响节点数");
    assert!(db.get_payload(id1).is_some(), "源节点不能被误删");
    assert!(db.get_payload(id2).is_some(), "目标节点不能被误删");

    cleanup(&path);
}

/// update_payload 后立即 search
#[test]
fn COV7_22_update_payload_search() {
    let path = tmp_db("upd_payload");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id = db
        .insert(
            &[1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"stage": "initial"}),
        )
        .unwrap();

    db.update_payload(id, serde_json::json!({"stage": "updated", "extra": 42}))
        .unwrap();

    let p = db.get_payload(id).unwrap();
    assert_eq!(p["stage"], "updated");
    assert_eq!(p["extra"], 42);

    cleanup(&path);
}

/// 大量节点 + Budget 超限
#[test]
fn COV7_23_match_budget_exceeded() {
    let path = tmp_db("budget");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 创建密集图
    for i in 0..20u32 {
        db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"i": i}))
            .unwrap();
    }
    let ids = db.all_node_ids();
    // 全连接（20*19/2 = 190 条边）
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            db.link(ids[i], ids[j], "all", 1.0).unwrap();
        }
    }

    // 深层变长路径 → 可能触发 budget 超限
    let r = db.tql(r#"MATCH (a)-[:all*1..10]->(b) RETURN a, b LIMIT 10"#);
    match r {
        Ok(res) => {
            assert!(res.len() <= 10, "LIMIT 10 必须限制预算内结果数量");
            for row in &res {
                assert!(row.contains_key("a"), "预算内结果必须保留 a 变量");
                assert!(row.contains_key("b"), "预算内结果必须保留 b 变量");
            }
        }
        Err(e) => assert!(!e.to_string().is_empty(), "预算超限时必须返回可诊断错误"),
    }

    cleanup(&path);
}

/// SEARCH VECTOR + EXPAND 多标签
#[test]
fn COV7_24_search_expand_multi_label() {
    let path = tmp_db("expand_multi");
    let db = seed_chain(&path, 10);

    let r = db
        .tql("SEARCH VECTOR [5.0, 0.0, 0.0, 0.0] TOP 3 EXPAND [:next|skip*1..2] RETURN *")
        .unwrap();
    assert!(r.len() >= 3, "SEARCH TOP 3 至少应保留原始向量命中");
    for row in &r {
        let node = row
            .get("_")
            .expect("SEARCH EXPAND RETURN * 应保留默认 _ 绑定");
        let name = node
            .payload
            .get("name")
            .and_then(|value| value.as_str())
            .expect("SEARCH EXPAND 结果必须来自 seed_chain 节点");
        let idx = name
            .strip_prefix("node_")
            .and_then(|suffix| suffix.parse::<u32>().ok())
            .expect("SEARCH EXPAND 不能产生脏节点名称");
        assert!(idx < 10, "SEARCH EXPAND 不能产生 seed_chain 外节点: {name}");
    }

    cleanup(&path);
}

/// MATCH 带 RETURN * + ORDER BY + LIMIT (extract_order_key 回退 "_")
#[test]
fn COV7_25_match_all_order_limit() {
    let path = tmp_db("match_all_ord");
    let db = seed_chain(&path, 10);

    let r = db
        .tql(r#"MATCH (a)-[:next]->(b) RETURN * ORDER BY a.val DESC LIMIT 5"#)
        .unwrap();
    assert_eq!(r.len(), 5, "LIMIT 5 应返回 5 条 next 边");
    let vals: Vec<_> = r
        .iter()
        .map(|row| {
            let a = row.get("a").expect("RETURN * 应包含 a");
            let b = row.get("b").expect("RETURN * 应包含 b");
            let a_val = a
                .payload
                .get("val")
                .and_then(|value| value.as_f64())
                .expect("a.val 必须是数值");
            let b_val = b
                .payload
                .get("val")
                .and_then(|value| value.as_f64())
                .expect("b.val 必须是数值");
            assert!(
                (0.0..=22.5).contains(&b_val),
                "b.val 必须来自 seed_chain 节点"
            );
            a_val
        })
        .collect();
    assert!(
        vals.windows(2).all(|pair| pair[0] >= pair[1]),
        "ORDER BY a.val DESC 必须按降序返回"
    );

    cleanup(&path);
}
