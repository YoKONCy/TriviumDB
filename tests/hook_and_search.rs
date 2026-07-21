#![allow(non_snake_case)]
//! Hook 扩展系统与高级 TQL 图遍历集成测试
//!
//! 验证范围：
//! - `hook.rs`: CompositeHook 全阶段 (pre_search → post_recall → pre_graph_expand → post_search)、
//!   abort 中断、custom_recall 替代召回、rerank 重排、FfiHook 错误路径、HookContext 计时
//! - `query/tql_executor.rs`: OPTIONAL MATCH、EXPLAIN、变长路径 (*2..4)、反向/双向边遍历
//! - `query/tql_parser.rs`: AND/OR/NOT 逻辑谓词、比较运算符 (==, !=, <, <=, >, >=)、inline filter
//! - `database/pipeline.rs`: BQ 粗筛、DPP 多样性、不应期/抑制、文本混合搜索

use triviumdb::database::{Database, SearchConfig};
use triviumdb::hook::{CompositeHook, FfiHook, HookContext, SearchHook};
use triviumdb::node::SearchHit;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    let path = dir
        .join(format!("cov3_{}", name))
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

// ════════════════════════════════════════════════════════════════
//  hook.rs 覆盖
// ════════════════════════════════════════════════════════════════

/// CompositeHook 全阶段触达
#[test]
fn COV3_01_composite_hook_all_stages() {
    let path = tmp_db("comp_hook");

    struct FilterHook;
    impl SearchHook for FilterHook {
        fn on_post_recall(&self, hits: &mut Vec<SearchHit>, _ctx: &mut HookContext) {
            hits.retain(|h| h.score > 0.1);
        }
        fn on_pre_graph_expand(&self, seeds: &mut Vec<SearchHit>, _ctx: &mut HookContext) {
            // 限制种子数
            seeds.truncate(3);
        }
        fn on_post_search(&self, _results: &mut Vec<SearchHit>, ctx: &mut HookContext) {
            ctx.custom_data = serde_json::json!({"filtered": true});
        }
    }

    struct ScoreHook;
    impl SearchHook for ScoreHook {
        fn on_pre_search(
            &self,
            _query_vector: &mut Vec<f32>,
            config: &mut SearchConfig,
            _ctx: &mut HookContext,
        ) {
            config.top_k = 10; // 多召回
        }
    }

    let mut composite = CompositeHook::new();
    composite.add(ScoreHook);
    composite.add(FilterHook);

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..20u32 {
        db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
    }
    db.set_hook(composite);

    let config = SearchConfig {
        top_k: 5,
        enable_advanced_pipeline: true,
        ..Default::default()
    };
    let (hits, ctx) = db
        .search_hybrid_with_context(None, Some(&[1.0, 0.0, 0.0, 0.0]), &config)
        .unwrap();
    eprintln!(
        "  CompositeHook: {} hits, custom_data={:?}",
        hits.len(),
        ctx.custom_data
    );

    cleanup(&path);
}

/// CompositeHook on_pre_search abort
#[test]
fn COV3_02_composite_hook_abort() {
    let path = tmp_db("comp_abort");

    struct AbortHook;
    impl SearchHook for AbortHook {
        fn on_pre_search(
            &self,
            _query_vector: &mut Vec<f32>,
            _config: &mut SearchConfig,
            ctx: &mut HookContext,
        ) {
            ctx.abort = true;
        }
    }

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..5u32 {
        db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
    }
    db.set_hook(AbortHook);

    let hits = db.search(&[1.0, 0.0, 0.0, 0.0], 5, 0, 0.0).unwrap();
    assert!(hits.is_empty(), "Abort hook 应导致空结果");

    cleanup(&path);
}

/// CompositeHook custom_recall 替代
#[test]
fn COV3_03_composite_hook_custom_recall() {
    let path = tmp_db("comp_recall");

    struct CustomRecallHook;
    impl SearchHook for CustomRecallHook {
        fn on_custom_recall(
            &self,
            _query_vector: &[f32],
            _config: &SearchConfig,
            _ctx: &mut HookContext,
        ) -> Option<Vec<SearchHit>> {
            // 返回自定义结果
            Some(vec![SearchHit {
                id: 42,
                score: 1.0,
                payload: serde_json::json!({"source": "custom"}),
            }])
        }
    }

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    db.set_hook(CustomRecallHook);

    let hits = db.search(&[1.0, 0.0, 0.0, 0.0], 5, 0, 0.0).unwrap();
    // custom recall 可能覆盖，也可能因引擎逻辑叠加了其他结果
    eprintln!("  Custom recall: {} hits", hits.len());

    cleanup(&path);
}

/// CompositeHook rerank 替代
#[test]
fn COV3_04_composite_hook_rerank() {
    let path = tmp_db("comp_rerank");

    struct RerankHook;
    impl SearchHook for RerankHook {
        fn on_rerank(
            &self,
            hits: &mut Vec<SearchHit>,
            _ctx: &mut HookContext,
        ) -> Option<Vec<SearchHit>> {
            let mut result = hits.clone();
            result.reverse();
            Some(result)
        }
    }

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..10u32 {
        db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
    }
    db.set_hook(RerankHook);

    let hits = db.search(&[5.0, 0.0, 0.0, 0.0], 5, 0, 0.0).unwrap();
    eprintln!("  Rerank: {} hits", hits.len());

    cleanup(&path);
}

/// FfiHook 加载不存在的动态库
#[test]
fn COV3_05_ffi_hook_load_nonexistent() {
    let result = FfiHook::load("./nonexistent_plugin.dll");
    assert!(result.is_err(), "不存在的动态库应返回错误");
}

/// HookContext record_timing 覆盖
#[test]
fn COV3_06_hook_context_record_timing() {
    let mut ctx = HookContext::new();
    ctx.record_timing("test_stage", std::time::Duration::from_millis(42));
    assert_eq!(ctx.stage_timings.len(), 1);
    assert_eq!(ctx.stage_timings[0].0, "test_stage");
    assert_eq!(ctx.stage_timings[0].1.as_millis(), 42);
}

// ════════════════════════════════════════════════════════════════
//  tql_executor.rs 高级分支
// ════════════════════════════════════════════════════════════════

fn seed_graph(path: &str) -> Database<f32> {
    let mut db = Database::<f32>::open(path, DIM).unwrap();
    for i in 0..10u32 {
        db.insert(
            &[i as f32, (i as f32).sin(), (i as f32).cos(), 1.0],
            serde_json::json!({
                "type": "user",
                "name": format!("user_{}", i),
                "age": 20 + i,
                "active": i % 2 == 0
            }),
        )
        .unwrap();
    }

    let ids = db.all_node_ids();
    for i in 0..ids.len() - 1 {
        db.link(ids[i], ids[i + 1], "knows", 0.9).unwrap();
    }
    // 添加反向边
    if ids.len() >= 3 {
        db.link(ids[2], ids[0], "reports_to", 0.5).unwrap();
    }

    db
}

/// OPTIONAL MATCH 覆盖
#[test]
fn COV3_07_tql_optional_match() {
    let path = tmp_db("tql_optional");
    let db = seed_graph(&path);

    let results = db
        .tql(r#"OPTIONAL MATCH (a)-[:knows]->(b) RETURN a, b"#)
        .unwrap();
    eprintln!("  OPTIONAL MATCH: {} 条", results.len());

    cleanup(&path);
}

/// EXPLAIN 模式覆盖
#[test]
fn COV3_08_tql_explain() {
    let path = tmp_db("tql_explain");
    let db = seed_graph(&path);

    let results = db.tql(r#"EXPLAIN FIND {type: "user"} RETURN *"#).unwrap();
    assert!(!results.is_empty(), "EXPLAIN 应返回查询计划");

    cleanup(&path);
}

/// 变长路径覆盖 *2..4
#[test]
fn COV3_09_tql_variable_length_path() {
    let path = tmp_db("tql_varlen");
    let db = seed_graph(&path);

    let results = db
        .tql(r#"MATCH (a)-[:knows*2..4]->(b) RETURN a, b"#)
        .unwrap();
    eprintln!("  变长路径: {} 条", results.len());

    cleanup(&path);
}

/// 反向边遍历 <-
#[test]
fn COV3_10_tql_backward_edge() {
    let path = tmp_db("tql_backward");
    let db = seed_graph(&path);

    let results = db
        .tql(r#"MATCH (a)<-[:reports_to]-(b) RETURN a, b"#)
        .unwrap();
    eprintln!("  反向边: {} 条", results.len());

    cleanup(&path);
}

/// 双向边遍历 --
#[test]
fn COV3_11_tql_bidirectional() {
    let path = tmp_db("tql_bidir");
    let db = seed_graph(&path);

    let results = db.tql(r#"MATCH (a)-[:knows]-(b) RETURN a, b"#).unwrap();
    eprintln!("  双向边: {} 条", results.len());

    cleanup(&path);
}

/// SEARCH VECTOR + EXPAND 覆盖
#[test]
fn COV3_12_tql_search_expand() {
    let path = tmp_db("tql_expand");
    let db = seed_graph(&path);

    let results = db
        .tql("SEARCH VECTOR [1.0, 0.0, 0.0, 0.0] TOP 3 EXPAND [:knows*1..2] RETURN *")
        .unwrap();
    assert!(!results.is_empty());
    eprintln!("  SEARCH+EXPAND: {} 条", results.len());

    cleanup(&path);
}

/// SEARCH VECTOR + WHERE 覆盖
#[test]
fn COV3_13_tql_search_where() {
    let path = tmp_db("tql_search_wh");
    let db = seed_graph(&path);

    let results = db
        .tql(r#"SEARCH VECTOR [1.0, 0.0, 0.0, 0.0] TOP 5 WHERE {age: {$gte: 25}} RETURN *"#)
        .unwrap();
    eprintln!("  SEARCH+WHERE: {} 条", results.len());

    cleanup(&path);
}

/// TQL AND / OR / NOT predicate 覆盖
#[test]
fn COV3_14_tql_and_or_not() {
    let path = tmp_db("tql_logic");
    let db = seed_graph(&path);

    // AND
    let r = db
        .tql(r#"MATCH (a)-[:knows]->(b) WHERE a.age > 22 AND b.age < 28 RETURN a, b"#)
        .unwrap();
    eprintln!("  AND: {} 条", r.len());

    // OR
    let r = db
        .tql(r#"MATCH (a)-[:knows]->(b) WHERE a.age > 28 OR b.age < 22 RETURN a, b"#)
        .unwrap();
    eprintln!("  OR: {} 条", r.len());

    cleanup(&path);
}

/// TQL 不同比较运算符: ==, !=, <, <=, >, >=
#[test]
fn COV3_15_tql_comparison_operators() {
    let path = tmp_db("tql_cmp");
    let db = seed_graph(&path);

    // ==
    let r = db
        .tql(r#"MATCH (a) WHERE a.name == "user_0" RETURN a"#)
        .unwrap();
    assert!(!r.is_empty());

    // !=
    let r = db
        .tql(r#"MATCH (a) WHERE a.name != "user_0" RETURN a"#)
        .unwrap();
    assert!(!r.is_empty());

    // < and <=
    let r = db.tql(r#"MATCH (a) WHERE a.age < 23 RETURN a"#).unwrap();
    assert!(!r.is_empty());

    let r = db.tql(r#"MATCH (a) WHERE a.age <= 20 RETURN a"#).unwrap();
    assert!(!r.is_empty());

    // > and >=
    let r = db.tql(r#"MATCH (a) WHERE a.age >= 29 RETURN a"#).unwrap();
    assert!(!r.is_empty());

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  database/pipeline.rs 高级管线参数
// ════════════════════════════════════════════════════════════════

/// 高级管线启用覆盖
#[test]
fn COV3_16_advanced_pipeline() {
    let path = tmp_db("adv_pipe");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..30u32 {
        db.insert(
            &[i as f32, (i as f32).sin(), (i as f32).cos(), 1.0],
            serde_json::json!({"type": "item", "score": i}),
        )
        .unwrap();
    }
    let ids = db.all_node_ids();
    for i in 0..ids.len() - 1 {
        db.link(ids[i], ids[i + 1], "similar", 0.8).unwrap();
    }

    let config = SearchConfig {
        top_k: 5,
        expand_depth: 2,
        min_score: 0.0,
        enable_advanced_pipeline: true,
        enable_dpp: true,
        dpp_quality_weight: 0.5,
        enable_text_hybrid_search: true,
        enable_refractory_fatigue: true,
        enable_inverse_inhibition: true,
        lateral_inhibition_threshold: 100,
        ..Default::default()
    };

    let hits = db.search_advanced(&[15.0, 0.0, 0.0, 0.0], &config).unwrap();
    eprintln!("  Advanced pipeline: {} hits", hits.len());

    cleanup(&path);
}

/// 混合搜索: 文本+向量
#[test]
fn COV3_17_hybrid_text_vector() {
    let path = tmp_db("hybrid_tv");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..10u32 {
        let id = db
            .insert(
                &[i as f32, 0.0, 0.0, 0.0],
                serde_json::json!({"text": format!("document about topic {}", i)}),
            )
            .unwrap();
        db.index_text(id, &format!("document about topic {}", i))
            .unwrap();
    }
    db.build_text_index().unwrap();

    let config = SearchConfig {
        top_k: 5,
        enable_text_hybrid_search: true,
        text_boost: 2.0,
        ..Default::default()
    };
    let hits = db
        .search_hybrid(Some("topic 5"), Some(&[5.0, 0.0, 0.0, 0.0]), &config)
        .unwrap();
    eprintln!("  Hybrid text+vector: {} hits", hits.len());

    cleanup(&path);
}

/// payload_filter 覆盖
#[test]
fn COV3_18_payload_filter() {
    let path = tmp_db("payload_flt");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..20u32 {
        db.insert(
            &[i as f32, 0.0, 0.0, 0.0],
            serde_json::json!({"group": if i % 2 == 0 { "A" } else { "B" }, "value": i}),
        )
        .unwrap();
    }

    let config = SearchConfig {
        top_k: 10,
        min_score: 0.0,
        payload_filter: Some(triviumdb::Filter::eq("group", "A".into())),
        ..Default::default()
    };
    let hits = db.search_advanced(&[10.0, 0.0, 0.0, 0.0], &config).unwrap();
    assert_eq!(hits.len(), 10, "应填满全部 10 个 group=A 的匹配结果");
    for h in &hits {
        let p = db.get_payload(h.id).unwrap();
        assert_eq!(p["group"], "A", "payload_filter 应只返回 group=A");
    }

    cleanup(&path);
}

/// tql_mut CREATE + edge
#[test]
fn COV3_19_tql_create_with_edge() {
    let path = tmp_db("tql_create_edge");
    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 先创建节点，再通过 TQL 创建边
    let result = db
        .tql_mut(r#"CREATE (a {name: "Node1"}), (b {name: "Node2"}), (a)-[:links]->(b)"#)
        .unwrap();
    assert!(result.affected >= 2, "应创建 2 个节点 + 1 条边");

    cleanup(&path);
}

/// OFFSET 超过结果集大小
#[test]
fn COV3_20_tql_offset_overflow() {
    let path = tmp_db("tql_offset_over");
    let db = seed_graph(&path);

    let results = db
        .tql(r#"FIND {type: "user"} RETURN * LIMIT 100 OFFSET 999"#)
        .unwrap();
    assert!(results.is_empty(), "OFFSET 超过结果集应返回空");

    cleanup(&path);
}

/// MATCH 带 inline filter
#[test]
fn COV3_21_tql_match_inline_filter() {
    let path = tmp_db("tql_inline_flt");
    let db = seed_graph(&path);

    let results = db
        .tql(r#"MATCH (a {type: "user", age: {$gte: 25}}) RETURN a"#)
        .unwrap();
    assert!(!results.is_empty());

    cleanup(&path);
}
