#![allow(non_snake_case)]
//! 压实引擎、数据库核心 API 与向量运算集成测试
//!
//! 验证范围：
//! - `storage/compaction.rs`: 后台自动压实、手动压实、空库压实、连续压实幂等性
//! - `database/mod.rs`: 同步模式、内存限制自动落盘、Hook 生命周期、混合搜索、文本索引
//! - `vector.rs`: 余弦相似度边界（零向量、尾部标量路径）、u64 汉明相似度、f32 SIMD 分发

use triviumdb::database::{Config, Database, SearchConfig, StorageMode};
use triviumdb::node::NodeId;

const DIM: usize = 4;

#[derive(Clone)]
struct ExpectedNode {
    id: NodeId,
    idx: u32,
    vector: [f32; DIM],
}

fn vector_for_idx(idx: u32) -> [f32; DIM] {
    [idx as f32 + 1.0, idx as f32 + 2.0, idx as f32 + 3.0, 1.0]
}

fn seed_committed_graph(db: &mut Database<f32>, count: u32) -> Vec<ExpectedNode> {
    let start = db.node_count() as u32;
    let mut expected = Vec::new();
    for offset in 0..count {
        let idx = start + offset;
        let vector = vector_for_idx(idx);
        let id = db
            .insert(
                &vector,
                serde_json::json!({"idx": idx, "kind": "committed"}),
            )
            .unwrap();
        expected.push(ExpectedNode { id, idx, vector });
    }
    for pair in expected.windows(2) {
        db.link(pair[0].id, pair[1].id, "next", 1.0).unwrap();
    }
    expected
}

fn link_expected_batches(db: &mut Database<f32>, left: &[ExpectedNode], right: &[ExpectedNode]) {
    if let (Some(prev), Some(next)) = (left.last(), right.first()) {
        db.link(prev.id, next.id, "next", 1.0).unwrap();
    }
}

fn assert_committed_graph(db: &Database<f32>, expected: &[ExpectedNode]) {
    assert_eq!(
        db.node_count(),
        expected.len(),
        "恢复后不能丢失或额外生成已提交节点"
    );
    for item in expected {
        let node = db.get(item.id).expect("已提交节点必须能按 ID 读取");
        assert_eq!(node.vector, item.vector, "节点向量必须完整恢复");
        assert_eq!(
            node.payload.get("idx").and_then(|value| value.as_u64()),
            Some(item.idx as u64),
            "节点 payload.idx 必须完整恢复"
        );
        assert_eq!(
            node.payload.get("kind").and_then(|value| value.as_str()),
            Some("committed"),
            "节点 payload.kind 必须完整恢复"
        );
    }
    for pair in expected.windows(2) {
        let edges = db.get_edges(pair[0].id);
        assert!(
            edges.iter().any(|edge| {
                edge.target_id == pair[1].id && edge.label == "next" && edge.weight == 1.0
            }),
            "已提交边 {} -> {} 必须完整恢复",
            pair[0].id,
            pair[1].id
        );
    }
    for item in expected {
        let hits = db.search(&item.vector, 1, 0, 0.0).unwrap();
        assert_eq!(hits.len(), 1, "按原始向量搜索必须返回 Top1");
        assert_eq!(hits[0].id, item.id, "按原始向量搜索必须命中原节点");
    }
}

/// metadata-only 降级恢复的断言：校验节点数、payload、边，但不校验向量
/// （marker 无效时不信任 .vec，向量由 WAL 回放补齐，可能为零向量）
fn assert_committed_graph_metadata_only(db: &Database<f32>, expected: &[ExpectedNode]) {
    assert_eq!(
        db.node_count(),
        expected.len(),
        "降级恢复后不能丢失或额外生成已提交节点"
    );
    for item in expected {
        let node = db.get(item.id).expect("已提交节点必须能按 ID 读取");
        assert_eq!(
            node.payload.get("idx").and_then(|value| value.as_u64()),
            Some(item.idx as u64),
            "节点 payload.idx 必须完整恢复"
        );
        assert_eq!(
            node.payload.get("kind").and_then(|value| value.as_str()),
            Some("committed"),
            "节点 payload.kind 必须完整恢复"
        );
    }
    for pair in expected.windows(2) {
        let edges = db.get_edges(pair[0].id);
        assert!(
            edges.iter().any(|edge| {
                edge.target_id == pair[1].id && edge.label == "next" && edge.weight == 1.0
            }),
            "已提交边 {} -> {} 必须完整恢复",
            pair[0].id,
            pair[1].id
        );
    }
}

fn corrupt_flush_marker_size(path: &str) {
    let marker_path = format!("{}.flush_ok", path);
    let mut marker = std::fs::read(&marker_path).expect("必须存在 flush_ok 标记");
    // 新格式：magic(4) + version(1) + generation(8) + tdb_size(8) + vec_size(8) = 29 字节
    assert!(marker.len() >= 29, "flush_ok 标记必须包含完整头部");
    // vec_size 位于 bytes[21..29]
    let stored_vec = u64::from_le_bytes(marker[21..29].try_into().unwrap());
    marker[21..29].copy_from_slice(&(stored_vec + 1).to_le_bytes());
    std::fs::write(marker_path, marker).unwrap();
}

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    let path = dir
        .join(format!("cov_{}", name))
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
//  compaction.rs 覆盖
// ════════════════════════════════════════════════════════════════

/// 后台自动 Compaction: 启动 → 等待一轮 → 停止
#[test]
fn COV_01_auto_compaction_启动停止() {
    let path = tmp_db("auto_compact");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 插入数据
    for i in 0..50u32 {
        db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"i": i}))
            .unwrap();
    }

    // 启动后台 compaction（1 秒间隔）
    db.enable_auto_compaction(std::time::Duration::from_secs(1));

    // 等待一轮 compaction 完成
    std::thread::sleep(std::time::Duration::from_millis(1500));

    // 停止 compaction
    db.disable_auto_compaction();

    // 验证数据完好
    assert_eq!(db.node_count(), 50);
    drop(db);

    // 重新打开验证持久化
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.node_count(), 50, "auto-compaction 后数据应完好");

    cleanup(&path);
}

/// 手动 compact: 删除后 compact 验证文件缩小
#[test]
fn COV_02_manual_compact_删除后压实() {
    let path = tmp_db("manual_compact");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..100u32 {
        db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
    }
    db.flush().unwrap();

    let size_before = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

    // 删除 80 个节点
    let ids = db.all_node_ids();
    for &id in ids.iter().take(80) {
        db.delete(id).unwrap();
    }

    db.compact().unwrap();

    let size_after = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    assert_eq!(db.node_count(), 20);
    assert!(
        size_after < size_before,
        "compact 后文件应缩小: before={}B, after={}B",
        size_before,
        size_after
    );

    cleanup(&path);
}

/// 空数据库 compact（边界）
#[test]
fn COV_03_empty_compact() {
    let path = tmp_db("empty_compact");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.compact().unwrap();
    assert_eq!(db.node_count(), 0);

    cleanup(&path);
}

/// 连续 compact 3 轮（幂等性）
#[test]
fn COV_04_triple_compact() {
    let path = tmp_db("triple_compact");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..20u32 {
        db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
    }

    db.compact().unwrap();
    db.compact().unwrap();
    db.compact().unwrap();

    assert_eq!(db.node_count(), 20, "3 轮 compact 后数据应不变");

    drop(db);
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.node_count(), 20, "重新打开后数据完好");

    cleanup(&path);
}

/// compact 写 .vec 后、.tdb 仍旧时断电：必须用旧快照加 WAL 恢复全部已提交数据
#[test]
fn COV_04B_compact中断_vec已扩大_tdb未更新_已提交数据不丢() {
    let path = tmp_db("compact_crash_vec_appended");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let expected = seed_committed_graph(&mut db, 12);
    db.flush().unwrap();

    let more = seed_committed_graph(&mut db, 8);
    link_expected_batches(&mut db, &expected, &more);
    let mut all_expected = expected.clone();
    all_expected.extend(more);

    let old_tdb = std::fs::read(&path).unwrap();
    let old_flush_ok = std::fs::read(format!("{}.flush_ok", path)).unwrap();
    let wal_before = std::fs::read(format!("{}.wal", path)).unwrap();
    db.flush().unwrap();
    let new_vec = std::fs::read(format!("{}.vec", path)).unwrap();
    drop(db);

    std::fs::write(&path, old_tdb).unwrap();
    std::fs::write(format!("{}.vec", path), new_vec).unwrap();
    std::fs::write(format!("{}.flush_ok", path), old_flush_ok).unwrap();
    std::fs::write(format!("{}.wal", path), wal_before).unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_committed_graph_metadata_only(&db, &all_expected);

    cleanup(&path);
}

/// compact 写 .tdb 后、.flush_ok 未提交时断电：必须降级并通过 WAL 找回全部已提交数据
#[test]
fn COV_04C_compact中断_tdb已更新_flush_ok未更新_已提交数据不丢() {
    let path = tmp_db("compact_crash_tdb_renamed");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let expected = seed_committed_graph(&mut db, 10);
    db.flush().unwrap();

    let more = seed_committed_graph(&mut db, 6);
    link_expected_batches(&mut db, &expected, &more);
    let mut all_expected = expected.clone();
    all_expected.extend(more);

    let old_flush_ok = std::fs::read(format!("{}.flush_ok", path)).unwrap();
    let wal_before = std::fs::read(format!("{}.wal", path)).unwrap();
    db.flush().unwrap();
    drop(db);

    std::fs::write(format!("{}.flush_ok", path), old_flush_ok).unwrap();
    std::fs::write(format!("{}.wal", path), wal_before).unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_committed_graph_metadata_only(&db, &all_expected);

    cleanup(&path);
}

/// compact 写出 .flush_ok.tmp 但正式标记未提交时断电：残留临时标记不能导致已提交数据丢失
#[test]
fn COV_04D_compact中断_flush_ok_tmp残留_已提交数据不丢() {
    let path = tmp_db("compact_crash_flush_marker_tmp");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let expected = seed_committed_graph(&mut db, 14);
    db.flush().unwrap();
    db.update_payload(
        expected[0].id,
        serde_json::json!({"idx": expected[0].idx, "kind": "committed"}),
    )
    .unwrap();
    db.flush().unwrap();
    drop(db);

    std::fs::write(format!("{}.flush_ok.tmp", path), b"partial marker").unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_committed_graph(&db, &expected);

    cleanup(&path);
}

/// compact 成功写快照但 WAL 清理前断电：重复 WAL 回放必须幂等，不能丢失或复制已提交数据
#[test]
fn COV_04E_compact成功_wal清理前断电_已提交数据不丢() {
    let path = tmp_db("compact_crash_before_wal_clear");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let expected = seed_committed_graph(&mut db, 16);
    db.flush().unwrap();

    let more = seed_committed_graph(&mut db, 4);
    link_expected_batches(&mut db, &expected, &more);
    let mut all_expected = expected.clone();
    all_expected.extend(more);

    let wal_before = std::fs::read(format!("{}.wal", path)).unwrap();
    db.flush().unwrap();
    drop(db);
    std::fs::write(format!("{}.wal", path), wal_before).unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_committed_graph(&db, &all_expected);

    cleanup(&path);
}

/// compact 成功且 WAL 已清空后断电：快照本身必须足够完整地恢复全部已提交数据
#[test]
fn COV_04F_compact成功_wal已清空_快照完整恢复() {
    let path = tmp_db("compact_crash_after_wal_clear");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let expected = seed_committed_graph(&mut db, 18);
    db.flush().unwrap();
    drop(db);

    std::fs::write(format!("{}.wal", path), b"").unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_committed_graph(&db, &expected);

    cleanup(&path);
}

/// compact 提交点内容被撕裂时：不能信任不匹配的 .flush_ok，必须通过 WAL 恢复已提交增量
#[test]
fn COV_04G_compact中断_flush_ok大小不匹配_已提交数据不丢() {
    let path = tmp_db("compact_crash_flush_marker_mismatch");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let expected = seed_committed_graph(&mut db, 9);
    db.flush().unwrap();

    let more = seed_committed_graph(&mut db, 5);
    link_expected_batches(&mut db, &expected, &more);
    let mut all_expected = expected.clone();
    all_expected.extend(more);

    let wal_before = std::fs::read(format!("{}.wal", path)).unwrap();
    db.flush().unwrap();
    drop(db);
    corrupt_flush_marker_size(&path);
    std::fs::write(format!("{}.wal", path), wal_before).unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_committed_graph_metadata_only(&db, &all_expected);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  database/mod.rs 覆盖
// ════════════════════════════════════════════════════════════════

/// open_with_sync API 覆盖
#[test]
fn COV_05_open_with_sync() {
    let path = tmp_db("open_sync");

    let mut db =
        Database::<f32>::open_with_sync(&path, DIM, triviumdb::storage::wal::SyncMode::Full)
            .unwrap();
    db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    assert_eq!(db.node_count(), 1);

    cleanup(&path);
}

/// dim=0 拒绝
#[test]
fn COV_06_dim_zero_rejected() {
    let path = tmp_db("dim_zero");

    let config = Config {
        dim: 0,
        ..Default::default()
    };
    let result = Database::<f32>::open_with_config(&path, config);
    assert!(result.is_err(), "dim=0 应被拒绝");

    cleanup(&path);
}

/// memory limit 自动 flush
#[test]
fn COV_07_memory_limit_auto_flush() {
    let path = tmp_db("mem_limit");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.set_memory_limit(1024); // 1KB 极小限制

    // 插入足够数据触发自动 flush
    for i in 0..100u32 {
        db.insert(
            &[i as f32, 0.0, 0.0, 0.0],
            serde_json::json!({"data": "x".repeat(50)}),
        )
        .unwrap();
    }

    // 验证 estimated_memory 可调用
    let mem = db.estimated_memory();
    eprintln!("  estimated_memory: {} bytes", mem);

    // 文件应已被自动 flush
    assert!(
        std::path::Path::new(&path).exists(),
        "内存超限应触发自动 flush"
    );

    cleanup(&path);
}

/// set_sync_mode 运行时切换
#[test]
fn COV_08_set_sync_mode() {
    let path = tmp_db("sync_mode");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.set_sync_mode(triviumdb::storage::wal::SyncMode::Full);
    db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    db.set_sync_mode(triviumdb::storage::wal::SyncMode::Normal);
    db.insert(&[2.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    assert_eq!(db.node_count(), 2);

    cleanup(&path);
}

/// hook set/clear/get API
#[test]
fn COV_09_hook_lifecycle() {
    let path = tmp_db("hook_lc");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 默认 hook
    let _ = db.hook();

    // 自定义 hook
    struct TestHook;
    impl triviumdb::hook::SearchHook for TestHook {}
    db.set_hook(TestHook);
    let _ = db.hook();

    // 清除 hook
    db.clear_hook();
    let _ = db.hook();

    cleanup(&path);
}

/// search_hybrid_with_context 覆盖
#[test]
fn COV_10_search_hybrid_with_context() {
    let path = tmp_db("hybrid_ctx");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..10u32 {
        db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
    }

    let config = SearchConfig {
        top_k: 5,
        ..Default::default()
    };
    let (hits, ctx) = db
        .search_hybrid_with_context(None, Some(&[1.0, 0.0, 0.0, 0.0]), &config)
        .unwrap();
    assert!(!hits.is_empty());
    // 验证 ctx 有计时数据
    let _ = ctx;

    cleanup(&path);
}

/// index_keyword / index_text / build_text_index 覆盖
#[test]
fn COV_11_text_index_apis() {
    let path = tmp_db("text_idx");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id1 = db
        .insert(
            &[1.0, 0.0, 0.0, 0.0],
            serde_json::json!({"text": "hello world"}),
        )
        .unwrap();
    let id2 = db
        .insert(
            &[0.0, 1.0, 0.0, 0.0],
            serde_json::json!({"text": "foo bar"}),
        )
        .unwrap();

    db.index_keyword(id1, "hello").unwrap();
    db.index_text(id2, "foo bar baz").unwrap();
    db.build_text_index().unwrap();

    cleanup(&path);
}

/// get_edges 覆盖
#[test]
fn COV_12_get_edges() {
    let path = tmp_db("get_edges");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id1 = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    let id2 = db
        .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();

    db.link(id1, id2, "knows", 0.9).unwrap();

    let edges = db.get_edges(id1);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target_id, id2);

    // 不存在的节点
    let edges_none = db.get_edges(999);
    assert!(edges_none.is_empty());

    cleanup(&path);
}

/// unlink API 覆盖
#[test]
fn COV_13_unlink() {
    let path = tmp_db("unlink");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id1 = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    let id2 = db
        .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();

    db.link(id1, id2, "knows", 1.0).unwrap();
    assert_eq!(db.get_edges(id1).len(), 1);

    db.unlink(id1, id2).unwrap();
    assert_eq!(db.get_edges(id1).len(), 0);

    cleanup(&path);
}

/// update_vector API 覆盖
#[test]
fn COV_14_update_vector() {
    let path = tmp_db("update_vec");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();

    db.update_vector(id, &[0.0, 0.0, 0.0, 1.0]).unwrap();

    let hits = db.search(&[0.0, 0.0, 0.0, 1.0], 1, 0, 0.0).unwrap();
    assert_eq!(hits[0].id, id, "更新后向量应匹配新查询");

    cleanup(&path);
}

/// insert_with_id 错误路径
#[test]
fn COV_15_insert_with_id_errors() {
    let path = tmp_db("insert_id_err");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();

    // 重复 ID
    let result = db.insert_with_id(id, &[1.0, 0.0, 0.0, 0.0], serde_json::json!({}));
    assert!(result.is_err(), "重复 ID 应被拒绝");

    cleanup(&path);
}

/// payload too large 错误路径
#[test]
fn COV_16_payload_too_large() {
    let path = tmp_db("payload_big");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 超过 8MB 的 payload
    let big = "x".repeat(9 * 1024 * 1024);
    let result = db.insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({"data": big}));
    assert!(result.is_err(), "超大 payload 应被拒绝");

    // update_payload 也应拒绝
    let id = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    let result = db.update_payload(id, serde_json::json!({"data": big}));
    assert!(result.is_err(), "update 超大 payload 应被拒绝");

    cleanup(&path);
}

/// StorageMode::Rom 覆盖
#[test]
fn COV_17_in_memory_storage_mode() {
    let path = tmp_db("in_memory");

    let config = Config {
        dim: DIM,
        storage_mode: StorageMode::Rom,
        ..Default::default()
    };
    let mut db = Database::<f32>::open_with_config(&path, config).unwrap();
    for i in 0..20u32 {
        db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
    }
    db.flush().unwrap();
    drop(db);

    let config = Config {
        dim: DIM,
        storage_mode: StorageMode::Rom,
        ..Default::default()
    };
    let db = Database::<f32>::open_with_config(&path, config).unwrap();
    assert_eq!(db.node_count(), 20);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  vector.rs 覆盖
// ════════════════════════════════════════════════════════════════

/// f32 零向量距离
#[test]
fn COV_18_cosine_zero_vector() {
    let zero = [0.0f32; 4];
    let other = [1.0, 2.0, 3.0, 4.0];
    let sim = triviumdb::vector::cosine_similarity_f32(&zero, &other);
    assert_eq!(sim, 0.0, "零向量的余弦相似度应为 0");

    let sim2 = triviumdb::vector::cosine_similarity_f32(&zero, &zero);
    assert_eq!(sim2, 0.0, "两个零向量的余弦相似度应为 0");
}

/// f64 类型 database 覆盖（触达 VectorType for f64 分支 — 如果存在）
/// 这里测试 u64 的完整流程来覆盖 vector.rs 中的 u64 impl
#[test]
fn COV_19_u64_vector_similarity() {
    use triviumdb::VectorType;
    let a: Vec<u64> = vec![0xFF00FF00FF00FF00, 0x0F0F0F0F0F0F0F0F];
    let b: Vec<u64> = vec![0xFF00FF00FF00FF00, 0x0F0F0F0F0F0F0F0F];
    let sim = u64::similarity(&a, &b);
    assert_eq!(sim, 128.0, "完全相同的 u64 向量应有最大汉明相似度");

    let c: Vec<u64> = vec![0x0000000000000000, 0x0000000000000000];
    let sim2 = u64::similarity(&a, &c);
    assert!(sim2 < 128.0, "不同向量应有较低相似度");

    // zero / to_f32 / from_f32
    assert_eq!(u64::zero(), 0u64);
    assert_eq!(42u64.to_f32(), 42.0);
    assert_eq!(u64::from_f32(42.0), 42u64);
}

/// 标量路径覆盖（非 8 的倍数维度 → AVX2 尾部处理 + 非 4 的倍数 → 标量尾部）
#[test]
fn COV_20_cosine_scalar_tail() {
    // 维度 3: 不是 4 的倍数，触发标量尾部
    let a = [1.0f32, 2.0, 3.0];
    let b = [4.0f32, 5.0, 6.0];
    let sim = triviumdb::vector::cosine_similarity_f32(&a, &b);
    assert!(sim > 0.9, "同向向量应有高相似度");

    // 维度 1
    let a1 = [1.0f32];
    let b1 = [-1.0f32];
    let sim1 = triviumdb::vector::cosine_similarity_f32(&a1, &b1);
    assert!(sim1 < 0.0, "反向向量应有负相似度");

    // 维度 0 (空)
    let empty: [f32; 0] = [];
    let sim0 = triviumdb::vector::cosine_similarity_f32(&empty, &empty);
    assert_eq!(sim0, 0.0, "空向量相似度应为 0");
}

/// search_advanced 覆盖更多参数组合
#[test]
fn COV_21_search_advanced_params() {
    let path = tmp_db("adv_params");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    for i in 0..20u32 {
        db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"score": i}))
            .unwrap();
    }
    db.link(1, 2, "rel", 1.0).unwrap();
    db.link(2, 3, "rel", 1.0).unwrap();

    // 带 filter 的 search_advanced
    let config = SearchConfig {
        top_k: 5,
        expand_depth: 2,
        min_score: 0.0,
        payload_filter: Some(triviumdb::Filter::gt("score", 10.0)),
        ..Default::default()
    };
    let hits = db.search_advanced(&[15.0, 0.0, 0.0, 0.0], &config).unwrap();
    for h in &hits {
        let p = db.get_payload(h.id).unwrap();
        let score = p["score"].as_f64().unwrap();
        assert!(score > 10.0, "过滤后所有结果 score 应 > 10");
    }

    cleanup(&path);
}

/// leiden_cluster 覆盖
#[test]
fn COV_22_leiden_cluster() {
    let path = tmp_db("leiden");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();

    // 创建两个小社区
    let mut ids = Vec::new();
    for i in 0..10u32 {
        let id = db
            .insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({}))
            .unwrap();
        ids.push(id);
    }

    // 社区 1: 0-4 全连接
    for i in 0..5 {
        for j in i + 1..5 {
            db.link(ids[i], ids[j], "intra", 1.0).unwrap();
        }
    }
    // 社区 2: 5-9 全连接
    for i in 5..10 {
        for j in i + 1..10 {
            db.link(ids[i], ids[j], "intra", 1.0).unwrap();
        }
    }

    let result = db.leiden_cluster(2, Some(10), Some(true)).unwrap();
    assert!(!result.node_to_cluster.is_empty(), "应检测到社区结构");

    cleanup(&path);
}

/// get_all_ids 覆盖
#[test]
fn COV_23_get_all_ids() {
    let path = tmp_db("all_ids");

    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let id1 = db
        .insert(&[1.0, 0.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();
    let id2 = db
        .insert(&[0.0, 1.0, 0.0, 0.0], serde_json::json!({}))
        .unwrap();

    let all_ids = db.get_all_ids();
    assert!(all_ids.contains(&id1));
    assert!(all_ids.contains(&id2));
    assert_eq!(all_ids.len(), 2);

    cleanup(&path);
}
