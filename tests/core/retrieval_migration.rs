//! 查询位模式、度量、浮点尺度与迁移目标的独立行为边界。
use super::support::TestDir;
use serde_json::json;
use std::collections::BTreeMap;
use triviumdb::database::{BatchSearchConfig, Config, SearchConfig, StorageMode};
use triviumdb::{Database, HookContext, SearchHook, TriviumError};

fn config(mode: StorageMode, dim: usize) -> Config {
    Config {
        dim,
        storage_mode: mode,
        auto_build_quiver: false,
        ..Default::default()
    }
}
fn search_config(k: usize) -> SearchConfig {
    SearchConfig {
        top_k: k,
        recall_k: 8,
        rerank_k: 8,
        min_score: -1.,
        expand_depth: 0,
        ..Default::default()
    }
}
fn hits(hits: Vec<triviumdb::node::SearchHit>) -> Vec<(u64, f32)> {
    hits.into_iter().map(|hit| (hit.id, hit.score)).collect()
}

#[test]
fn 汉明检索各入口保持完整整数位模式及固定分数() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for query in [16_777_217, 9_007_199_254_740_993, 1u64 << 63, u64::MAX] {
            let root = TestDir::new("hamming_routes");
            let path = root.file("data.tdb");
            let mut db = Database::<u64>::open_with_config(&path, config(mode, 1)).unwrap();
            db.insert_with_id(1, &[query], json!({"exact":true}))
                .unwrap();
            db.insert_with_id(2, &[query ^ 1], json!({"exact":false}))
                .unwrap();
            for reopened in [false, true] {
                if reopened {
                    db.close().unwrap();
                    db = Database::open_with_config(&path, config(mode, 1)).unwrap();
                }
                let expected = vec![(1, 64.), (2, 63.)];
                assert_eq!(hits(db.search_exact(&[query], 2).unwrap()), expected);
                assert_eq!(hits(db.search(&[query], 2, 0, -1.).unwrap()), expected);
                for forced in [false, true] {
                    let cfg = SearchConfig {
                        force_brute_force: forced,
                        ..search_config(2)
                    };
                    assert_eq!(hits(db.search_advanced(&[query], &cfg).unwrap()), expected);
                    let batch = db
                        .search_batch(
                            &[vec![query], vec![query ^ 1]],
                            &cfg,
                            &BatchSearchConfig::default(),
                        )
                        .unwrap();
                    assert_eq!(hits(batch[0].clone()), expected);
                    assert_eq!(hits(batch[1].clone()), [(2, 64.), (1, 63.)]);
                }
                let rows = db
                    .tql_nodes(&format!("SEARCH VECTOR [{query}] TOP 1 RETURN *"))
                    .unwrap();
                assert_eq!(rows[0].values().next().unwrap().id, 1);
                assert!(matches!(
                    db.build_quiver_index(None),
                    Err(TriviumError::InvalidInput(_))
                ));
                assert!(db.prepare_auto_quiver_build().unwrap().is_none());
                let (result, ctx) = db
                    .search_hybrid_with_context(None, Some(&[query]), &search_config(2))
                    .unwrap();
                assert_eq!(hits(result), expected);
                assert!(
                    ctx.observations
                        .contains(&("vector_route_quiver".into(), 0))
                );
            }
            db.close().unwrap();
        }
    }
}

struct QueryHook(bool);
impl SearchHook for QueryHook {
    fn on_pre_search(
        &self,
        query: &mut Vec<f32>,
        config: &mut SearchConfig,
        _ctx: &mut HookContext,
    ) {
        config.top_k = 1;
        if self.0 {
            query[0] = 1.;
        }
    }
}
#[test]
fn 二进制hook保留观察视图并拒绝浮点改写() {
    let root = TestDir::new("hamming_hook");
    let mut db = Database::<u64>::open(&root.file("data.tdb"), 1).unwrap();
    db.insert_with_id(1, &[16_777_217], json!(null)).unwrap();
    db.insert_with_id(2, &[16_777_216], json!(null)).unwrap();
    db.set_hook(QueryHook(false)).unwrap();
    assert_eq!(
        hits(db.search(&[16_777_217], 2, 0, -1.).unwrap()),
        [(1, 64.)]
    );
    db.set_hook(QueryHook(true)).unwrap();
    assert!(matches!(
        db.search(&[16_777_217], 1, 0, -1.),
        Err(TriviumError::InvalidInput(_))
    ));
    db.clear_hook();
    for (sparse, dpp) in [(true, false), (false, true)] {
        assert!(matches!(
            db.search_advanced(
                &[16_777_217],
                &SearchConfig {
                    enable_advanced_pipeline: true,
                    enable_sparse_residual: sparse,
                    enable_dpp: dpp,
                    ..search_config(1)
                }
            ),
            Err(TriviumError::InvalidInput(_))
        ));
    }
    db.close().unwrap();
}

#[test]
fn 二进制只读重开保留旧式浮点索引副本并使用汉明分数() {
    use triviumdb::index::quiver::{QuIVer, QuIVerConfig};
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("binary_float_sidecar");
        let path = root.file("data.tdb");
        let mut db = Database::<u64>::open_with_config(&path, config(mode, 1)).unwrap();
        db.insert_with_id(1, &[2], json!({"id":1})).unwrap();
        db.insert_with_id(2, &[1], json!({"id":2})).unwrap();
        db.close().unwrap();
        drop(db);
        // 旧路径把整数转换为浮点后构建；独立 QMET 布局与合法副本模拟这种文件组。
        let mut index = QuIVer::new(1, &QuIVerConfig::default());
        index.batch_build(&[2., 1.], &[1, 2], &[0, 1]);
        let sidecar = format!("{path}.quiver");
        index.save_to_file(std::path::Path::new(&sidecar)).unwrap();
        let contents = std::fs::read(&sidecar).unwrap();
        let mut meta = b"QMET".to_vec();
        meta.extend_from_slice(&1u32.to_le_bytes());
        for size in [
            std::fs::metadata(&path).unwrap().len(),
            if mode == StorageMode::Mmap { 16 } else { 0 },
            contents.len() as u64,
            2,
        ] {
            meta.extend_from_slice(&size.to_le_bytes());
        }
        meta.extend_from_slice(&1u32.to_le_bytes());
        meta.extend_from_slice(&crc32fast::hash(&contents).to_le_bytes());
        std::fs::write(format!("{path}.quiver.meta"), meta).unwrap();
        let before = files(&root.0);
        let mut reader = Database::<u64>::open_read_only(&path, 1).unwrap();
        assert_eq!(
            hits(reader.search(&[1], 2, 0, -1.).unwrap()),
            [(2, 64.), (1, 62.)]
        );
        reader.close().unwrap();
        assert_eq!(files(&root.0), before);
    }
}

#[test]
fn 浮点极端有限幅值的精确及普通自检保持单位余弦() {
    let root = TestDir::new("float_scales");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for (index, scale) in [1e20f32, 1e-30, f32::MAX, f32::from_bits(1)]
            .into_iter()
            .enumerate()
        {
            let path = root.file(&format!("{mode:?}-{index}.tdb"));
            let mut db = Database::<f32>::open_with_config(&path, config(mode, 2)).unwrap();
            db.insert_with_id(1, &[scale, 0.], json!({"sign":1}))
                .unwrap();
            db.insert_with_id(2, &[-scale, 0.], json!({"sign":-1}))
                .unwrap();
            for reopen in [false, true] {
                if reopen {
                    db.close().unwrap();
                    db = Database::open_with_config(&path, config(mode, 2)).unwrap();
                }
                assert_eq!(
                    hits(db.search_exact(&[scale, 0.], 2).unwrap()),
                    [(1, 1.), (2, -1.)]
                );
                assert_eq!(
                    hits(db.search(&[scale, 0.], 2, 0, -1.).unwrap()),
                    [(1, 1.), (2, -1.)]
                );
            }
            db.close().unwrap();
        }
    }
}

fn files(root: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}
#[test]
fn 迁移拒绝已有或残留目标且完整保留原文件组() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let source_root = TestDir::new("migration_source");
        let target_root = TestDir::new("migration_target");
        let mut source =
            Database::<f32>::open_with_config(&source_root.file("source.tdb"), config(mode, 1))
                .unwrap();
        for id in [1, 2] {
            source
                .insert_with_id(id, &[1.], json!({"origin":"source","id":id}))
                .unwrap();
        }
        let target = target_root.file("target.tdb");
        let mut target_db = Database::<f32>::open_with_config(&target, config(mode, 2)).unwrap();
        target_db
            .insert_with_id(2, &[0., 1.], json!({"origin":"target"}))
            .unwrap();
        target_db.close().unwrap();
        drop(target_db);
        let before = files(&target_root.0);
        assert!(
            matches!(source.migrate_to(&target,2),Err(TriviumError::Io(error)) if error.kind()==std::io::ErrorKind::AlreadyExists)
        );
        assert_eq!(files(&target_root.0), before);
        let new_root = TestDir::new("migration_orphan");
        let new_path = new_root.file("target.tdb");
        std::fs::write(format!("{new_path}.wal"), b"existing recovery evidence").unwrap();
        let before = files(&new_root.0);
        assert!(
            matches!(source.migrate_to(&new_path,2),Err(TriviumError::Io(error)) if error.kind()==std::io::ErrorKind::AlreadyExists)
        );
        assert_eq!(files(&new_root.0), before);
        source.close().unwrap();
    }
}

#[test]
fn 新库迁移完整保存payload边元数据并可再次打开() {
    let source_root = TestDir::new("migration_complete_source");
    let target_root = TestDir::new("migration_complete_target");
    let mut source = Database::<f32>::open(&source_root.file("source.tdb"), 1).unwrap();
    for id in [1, 2] {
        source.insert_with_id(id, &[1.], json!({"id":id})).unwrap();
    }
    source
        .upsert_edge(1, 2, "关联", 2., json!({"source":"迁移"}))
        .unwrap();
    let target = target_root.file("target.tdb");
    let (mut db, ids) = source.migrate_to(&target, 3).unwrap();
    assert_eq!(ids, [1, 2]);
    for id in ids {
        let node = db.get(id).unwrap().unwrap();
        assert_eq!(node.vector, [0., 0., 0.]);
        assert_eq!(node.payload, json!({"id":id}));
    }
    assert_eq!(
        db.get_edge(1, 2, "关联").unwrap().unwrap().metadata,
        json!({"source":"迁移"})
    );
    db.close().unwrap();
    drop(db);
    let mut db = Database::<f32>::open_read_only(&target, 3).unwrap();
    assert_eq!(db.all_node_ids().unwrap(), [1, 2]);
    assert_eq!(db.get_edge(1, 2, "关联").unwrap().unwrap().weight, 2.);
    db.close().unwrap();
    assert!(
        std::fs::read_dir(&target_root.0).unwrap().all(|entry| entry
            .unwrap()
            .file_type()
            .unwrap()
            .is_file())
    );
    source.close().unwrap();
}
