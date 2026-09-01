#![allow(non_snake_case)]
//! 多端公共契约的 Rust 权威矩阵。
//!
//! Python/Node 绑定测试使用相同操作语义；此处验证稳定错误类别、ID、分页、索引和 Hook 契约。

use serde_json::json;
use std::collections::BTreeSet;
use triviumdb::database::{AccessMode, Config, Database, StorageMode};
use triviumdb::error::TriviumError;
use triviumdb::hook::{HookContext, SearchHook};
use triviumdb::node::SearchHit;

fn path(name: &str) -> String {
    let root = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&root).unwrap();
    root.join(format!("contract_matrix_{name}.tdb"))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for suffix in ["", ".wal", ".vec", ".lock", ".flush_ok", ".propidx"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

#[test]
fn CRUD_索引_分页和重开公共契约保持一致() {
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        let path = path(&format!("crud_{mode:?}"));
        cleanup(&path);
        let mut database = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 2,
                storage_mode: mode,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        for id in 1..=32 {
            database
                .insert_with_id(
                    id,
                    &[id as f32, 1.0],
                    json!({"kind": if id % 2 == 0 {"even"} else {"odd"}, "rank": id}),
                )
                .unwrap();
        }
        database.create_index("kind").unwrap();
        database.create_ordered_index("rank").unwrap();
        let page = database
            .tql("FIND {kind: \"even\"} RETURN * ORDER BY _.rank ASC LIMIT 5 OFFSET 3")
            .unwrap();
        assert_eq!(
            page.iter().map(|row| row["_"].id).collect::<Vec<_>>(),
            vec![8, 10, 12, 14, 16]
        );
        database.flush().unwrap();
        drop(database);
        let database = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 2,
                storage_mode: mode,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(database.node_count(), 32);
        assert_eq!(database.get_payload(16).unwrap()["kind"], "even");
        drop(database);
        cleanup(&path);
    }
}

#[test]
fn 结构化错误契约覆盖维度_节点_只读_查询预算和API迁移() {
    let path = path("errors");
    cleanup(&path);
    let mut database = Database::<f32>::open(&path, 2).unwrap();
    assert!(matches!(
        database.insert(&[1.0], json!({})),
        Err(TriviumError::DimensionMismatch { .. })
    ));
    assert!(matches!(
        database.update_payload(999, json!({})),
        Err(TriviumError::NodeNotFound(999))
    ));
    database.insert(&[1.0, 0.0], json!({})).unwrap();
    database.flush().unwrap();
    drop(database);

    let mut readonly = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: 2,
            access_mode: AccessMode::ReadOnly,
            memory_limit: 10 * (2 * std::mem::size_of::<f32>() + 256),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        readonly.insert(&[0.0, 1.0], json!({})),
        Err(TriviumError::ReadOnlyViolation { .. })
    ));
    assert!(matches!(
        readonly.tql("MATCH (n) RETURN n LIMIT 10 OFFSET 1"),
        Err(TriviumError::QueryRowBudgetExceeded { .. })
    ));
    drop(readonly);
    cleanup(&path);
}

struct InvalidRecall;

impl SearchHook for InvalidRecall {
    fn on_custom_recall(
        &self,
        _query: &[f32],
        _config: &triviumdb::database::SearchConfig,
        _context: &mut HookContext,
    ) -> Option<Vec<SearchHit>> {
        Some(vec![
            SearchHit {
                id: 0,
                score: 1.0,
                payload: json!({}),
            },
            SearchHit {
                id: 1,
                score: f32::NAN,
                payload: json!({}),
            },
            SearchHit {
                id: 2,
                score: 0.5,
                payload: json!({}),
            },
        ])
    }
}

#[test]
fn Hook非法命中不得越过最终公共结果契约() {
    let path = path("hook");
    cleanup(&path);
    let mut database = Database::<f32>::open(&path, 2).unwrap();
    for id in 1..=2 {
        database
            .insert_with_id(id, &[1.0, 0.0], json!({"id": id}))
            .unwrap();
    }
    database.set_hook(InvalidRecall);
    let result = database.search(&[1.0, 0.0], 10, 0, -1.0);
    match result {
        Ok(hits) => {
            let ids = hits.into_iter().map(|hit| hit.id).collect::<BTreeSet<_>>();
            assert!(!ids.contains(&0));
        }
        Err(error) => assert!(matches!(
            error,
            TriviumError::InvalidInput(_) | TriviumError::QueryExecution(_)
        )),
    }
    cleanup(&path);
}
