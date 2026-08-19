//! Agent 记忆图语义：业务边 vs in_workspace 粘合边
//!
//! 下游 dsh-trivium 把记忆记成「节点 + 有向带权边」。
//! 业务边是 about / decided / broke / fixed；每条记忆还连一条
//! in_workspace 到工作区根。若 expand / neighbors 沿全部边扩散，
//! 工作区根会把整库粘成一团。

use triviumdb::database::SearchConfig;
use triviumdb::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    let path = dir
        .join(format!("agent_mem_{}", name))
        .to_string_lossy()
        .to_string();
    cleanup(&path);
    path
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        let _ = std::fs::remove_file(format!("{}{}", path, ext));
    }
}

const BUSINESS: &[&str] = &["about", "decided", "broke", "fixed"];

fn business_labels() -> Vec<String> {
    BUSINESS.iter().map(|s| (*s).to_string()).collect()
}

/// preference -[:about]-> entity
/// preference -[:in_workspace]-> workspace
/// decision  -[:decided]-> entity
/// decision  -[:in_workspace]-> workspace
fn build_memory_graph(path: &str) -> (Database<f32>, u64, u64, u64, u64) {
    let mut db = Database::<f32>::open(path, DIM).unwrap();
    db.disable_auto_compaction();

    let workspace = db
        .insert(
            &[0.0, 0.0, 0.0, 1.0],
            serde_json::json!({"kind": "workspace"}),
        )
        .unwrap();
    let entity = db
        .insert(
            &[0.0, 1.0, 0.0, 0.0],
            serde_json::json!({"kind": "entity", "name": "auth"}),
        )
        .unwrap();
    let preference = db
        .insert(
            &[1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"kind": "preference", "text": "prefer jwt"}),
        )
        .unwrap();
    let decision = db
        .insert(
            &[0.9, 0.1, 0.0, 0.0],
            serde_json::json!({"kind": "decision", "text": "use jwt"}),
        )
        .unwrap();

    db.link(preference, entity, "about", 1.0).unwrap();
    db.link(preference, workspace, "in_workspace", 1.0).unwrap();
    db.link(decision, entity, "decided", 1.0).unwrap();
    db.link(decision, workspace, "in_workspace", 1.0).unwrap();

    (db, workspace, entity, preference, decision)
}

#[test]
fn 入边能从entity找到about过来的preference() {
    let path = tmp_db("incoming");
    let (db, workspace, entity, preference, decision) = build_memory_graph(&path);

    let about = db.get_incoming_edges(entity, Some("about"));
    assert_eq!(about.len(), 1);
    assert_eq!(about[0].source_id, preference);
    assert_eq!(about[0].label, "about");

    let decided = db.get_incoming_edges(entity, Some("decided"));
    assert_eq!(decided.len(), 1);
    assert_eq!(decided[0].source_id, decision);

    let all_in = db.get_incoming_edges(entity, None);
    assert_eq!(all_in.len(), 2);

    let ws_in = db.get_incoming_edges(workspace, None);
    assert_eq!(ws_in.len(), 2);
    assert!(ws_in.iter().all(|e| e.label == "in_workspace"));

    drop(db);
    cleanup(&path);
}

#[test]
fn 业务边扩1跳不含workspace根() {
    let path = tmp_db("expand_hop");
    let (db, workspace, entity, preference, decision) = build_memory_graph(&path);
    let labels = business_labels();

    let unfiltered = db.neighbors(preference, 1);
    assert!(unfiltered.contains(&entity));
    assert!(
        unfiltered.contains(&workspace),
        "不传 labels 必须保持历史行为：in_workspace 仍会扩到工作区根"
    );

    let filtered = db.neighbors_with_labels(preference, 1, Some(&labels));
    assert!(filtered.contains(&entity), "about 应扩到 entity");
    assert!(
        !filtered.contains(&workspace),
        "业务白名单不应顺着 in_workspace 扩到工作区根"
    );
    assert!(!filtered.contains(&decision));

    let from_decision = db.neighbors_with_labels(decision, 1, Some(&labels));
    assert!(from_decision.contains(&entity));
    assert!(!from_decision.contains(&workspace));

    drop(db);
    cleanup(&path);
}

#[test]
fn search_expand_labels_不把workspace粘进结果() {
    let path = tmp_db("search_expand");
    let (db, workspace, entity, preference, _decision) = build_memory_graph(&path);
    let labels = business_labels();
    let query = [1.0, 0.0, 0.0, 0.0];

    let glued = db
        .search_advanced(
            &query,
            &SearchConfig {
                top_k: 10,
                expand_depth: 1,
                min_score: 0.1,
                enable_advanced_pipeline: false,
                ..Default::default()
            },
        )
        .unwrap();
    let glued_ids: Vec<u64> = glued.iter().map(|h| h.id).collect();
    assert!(glued_ids.contains(&preference));
    assert!(
        glued_ids.contains(&workspace),
        "不传 expand_labels 时 depth=1 仍会扩到 workspace"
    );

    let scoped = db
        .search_advanced(
            &query,
            &SearchConfig {
                top_k: 10,
                expand_depth: 1,
                min_score: 0.1,
                enable_advanced_pipeline: false,
                expand_labels: Some(labels),
                ..Default::default()
            },
        )
        .unwrap();
    let scoped_ids: Vec<u64> = scoped.iter().map(|h| h.id).collect();
    assert!(scoped_ids.contains(&preference));
    assert!(scoped_ids.contains(&entity), "about 应把 entity 扩进结果");
    assert!(
        !scoped_ids.contains(&workspace),
        "expand_labels 白名单不应把 workspace 粘进结果"
    );

    drop(db);
    cleanup(&path);
}
