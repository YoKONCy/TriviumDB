//! 补齐 error.rs / node.rs / database/config.rs 的单元测试
//!
//! 这些模块此前完全没有任何测试覆盖

use triviumdb::database::config::{Config, SearchConfig, StorageMode};
use triviumdb::error::TriviumError;
use triviumdb::node::{Edge, NodeView, SearchHit};
use triviumdb::storage::wal::SyncMode;

// ════════════════════════════════════════════════════════════════
//  TriviumError — Display + From 转换
// ════════════════════════════════════════════════════════════════

#[test]
fn error_display_格式正确() {
    let e = TriviumError::DimensionMismatch {
        expected: 128,
        got: 64,
    };
    let msg = format!("{}", e);
    assert!(msg.contains("128"), "应包含期望维度");
    assert!(msg.contains("64"), "应包含实际维度");
}

#[test]
fn error_node_not_found() {
    let e = TriviumError::NodeNotFound(42);
    assert!(format!("{}", e).contains("42"));
}

#[test]
fn error_invalid_vector() {
    let e = TriviumError::InvalidVector {
        reason: "contains NaN".into(),
    };
    assert!(format!("{}", e).contains("NaN"));
}

#[test]
fn error_payload_too_large() {
    let e = TriviumError::PayloadTooLarge {
        size_bytes: 2_000_000,
        max_bytes: 1_000_000,
    };
    let msg = format!("{}", e);
    assert!(msg.contains("2000000"));
    assert!(msg.contains("1000000"));
}

#[test]
fn error_node_already_exists() {
    let e = TriviumError::NodeAlreadyExists(99);
    assert!(format!("{}", e).contains("99"));
}

#[test]
fn error_database_locked() {
    let e = TriviumError::DatabaseLocked("test.tdb".into());
    assert!(format!("{}", e).contains("test.tdb"));
}

#[test]
fn error_corrupted_file() {
    let e = TriviumError::CorruptedFile("bad header".into());
    assert!(format!("{}", e).contains("bad header"));
}

#[test]
fn error_query_parse() {
    let e = TriviumError::QueryParse("unexpected token".into());
    assert!(format!("{}", e).contains("unexpected token"));
}

#[test]
fn error_query_execution() {
    let e = TriviumError::QueryExecution("timeout".into());
    assert!(format!("{}", e).contains("timeout"));
}

#[test]
fn error_hook_load() {
    let e = TriviumError::HookLoadError("libhook.so not found".into());
    assert!(format!("{}", e).contains("libhook.so"));
}

#[test]
fn error_wal_closed() {
    let e = TriviumError::WalClosed;
    assert!(format!("{}", e).contains("WAL"));
}

#[test]
fn error_invalid_input() {
    let e = TriviumError::InvalidInput("dim must be > 0".into());
    assert!(format!("{}", e).contains("dim"));
}

#[test]
fn error_from_io() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let e: TriviumError = io_err.into();
    assert!(format!("{}", e).contains("file not found"));
}

// ════════════════════════════════════════════════════════════════
//  Edge / NodeView / SearchHit — 结构体完整性
// ════════════════════════════════════════════════════════════════

#[test]
fn edge_创建和克隆() {
    let edge = Edge {
        target_id: 42,
        label: "knows".to_string(),
        weight: 0.8,
        metadata: serde_json::Value::Null,
    };
    assert_eq!(edge.target_id, 42);
    assert_eq!(edge.label, "knows");
    assert_eq!(edge.weight, 0.8);

    let cloned = edge.clone();
    assert_eq!(edge, cloned);
}

#[test]
fn edge_序列化反序列化() {
    let edge = Edge {
        target_id: 1,
        label: "likes".into(),
        weight: 1.0,
        metadata: serde_json::json!({"source": "test"}),
    };
    let json = serde_json::to_string(&edge).unwrap();
    let restored: Edge = serde_json::from_str(&json).unwrap();
    assert_eq!(edge, restored);
}

#[test]
fn nodeview_创建() {
    let node = NodeView::<f32> {
        id: 1,
        vector: vec![1.0, 2.0, 3.0],
        payload: serde_json::json!({"name": "test"}),
        edges: vec![Edge {
            target_id: 2,
            label: "knows".into(),
            weight: 0.5,
            metadata: serde_json::Value::Null,
        }],
    };
    assert_eq!(node.id, 1);
    assert_eq!(node.vector.len(), 3);
    assert_eq!(node.edges.len(), 1);
}

#[test]
fn searchhit_创建和排序() {
    let mut hits = [
        SearchHit {
            id: 1,
            score: 0.5,
            payload: serde_json::Value::Null,
        },
        SearchHit {
            id: 2,
            score: 0.9,
            payload: serde_json::Value::Null,
        },
        SearchHit {
            id: 3,
            score: 0.7,
            payload: serde_json::Value::Null,
        },
    ];
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    assert_eq!(hits[0].id, 2);
    assert_eq!(hits[1].id, 3);
    assert_eq!(hits[2].id, 1);
}

// ════════════════════════════════════════════════════════════════
//  Config / SearchConfig — 默认值和构建
// ════════════════════════════════════════════════════════════════

#[test]
fn config_default() {
    let cfg = Config::default();
    assert_eq!(cfg.dim, 1536);
    assert_eq!(cfg.sync_mode, SyncMode::Normal);
    assert_eq!(cfg.storage_mode, StorageMode::Mmap);
}

#[test]
fn search_config_default() {
    let cfg = SearchConfig::default();
    assert_eq!(cfg.top_k, 5);
    assert_eq!(cfg.expand_depth, 2);
    assert!(!cfg.enable_advanced_pipeline);
    assert!(!cfg.enable_dpp);
    assert!(!cfg.enable_text_hybrid_search);
    assert!(cfg.payload_filter.is_none());
}

#[test]
fn storage_mode_variants() {
    assert_eq!(StorageMode::default(), StorageMode::Mmap);
    let _rom = StorageMode::Rom;
}

#[test]
fn sync_mode_variants() {
    assert_eq!(SyncMode::default(), SyncMode::Normal);
    let _full = SyncMode::Full;
    let _off = SyncMode::Off;
}
