use serde_json::json;
use triviumdb::database::{Config, Database, SearchConfig, StorageMode};
use triviumdb::test_hooks::{IoPoint, fail_io_at};

#[test]
fn 干净历史文本在首次写入前补齐封条且中断后可重试() {
    let root = super::support::TestDir::new("legacy_text_seal");
    for eager in [false, true] {
        for interrupted in [false, true] {
            let path = root.file(&format!("{eager}-{interrupted}.tdb"));
            let cfg = Config {
                dim: 2,
                load_text_index: eager,
                auto_build_quiver: false,
                ..Default::default()
            };
            let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
            db.insert_with_id(1, &[1., 0.], json!({"version":"old"}))
                .unwrap();
            db.index_text(1, "oldtext").unwrap();
            db.index_keyword(1, "oldkeyword").unwrap();
            db.close().unwrap();
            let text = std::fs::read(format!("{path}.text")).unwrap();
            let mut metadata = std::fs::read(format!("{path}.text.meta")).unwrap();
            metadata.truncate(28);
            metadata[4..8].copy_from_slice(&1u32.to_le_bytes());
            std::fs::write(format!("{path}.text.meta"), metadata).unwrap();
            if interrupted {
                let _failure = fail_io_at(IoPoint::CheckpointWalClear);
                assert_eq!(
                    Database::<f32>::open_with_config(&path, cfg)
                        .err()
                        .unwrap()
                        .code(),
                    "TDB_IO"
                );
            }
            let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
            assert_eq!(
                &std::fs::read(format!("{path}.text.meta")).unwrap()[..8],
                b"TMET\x02\x00\x00\x00"
            );
            assert_eq!(std::fs::read(format!("{path}.text")).unwrap(), text);
            db.close().unwrap();
            assert!(!triviumdb::storage::wal::Wal::needs_recovery(&path));
            let mut wal = triviumdb::storage::wal::Wal::open_with_sync(
                &path,
                triviumdb::storage::wal::SyncMode::Full,
            )
            .unwrap();
            wal.append(&triviumdb::storage::wal::WalEntry::<f32>::Delete { id: 1 })
                .unwrap();
            wal.append(&triviumdb::storage::wal::WalEntry::<f32>::Insert {
                id: 1,
                vector: vec![0., 1.],
                payload: json!({"version":"new"}).to_string(),
            })
            .unwrap();
            drop(wal);
            let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
            db.build_text_index().unwrap();
            assert_eq!(db.get_payload(1).unwrap(), Some(json!({"version":"new"})));
            assert!(hits(&db, "oldtext oldkeyword").is_empty());
            db.close().unwrap();
        }
    }
}

#[test]
fn 检查点封条保护新文本且后续删除继续级联清理() {
    let root = super::support::TestDir::new("text_replay_seals");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for eager in [false, true] {
            for sync in [
                triviumdb::storage::wal::SyncMode::Normal,
                triviumdb::storage::wal::SyncMode::Off,
            ] {
                let path = root.file(&format!("{mode:?}-{eager}-{sync:?}.tdb"));
                let cfg = Config {
                    dim: 2,
                    storage_mode: mode,
                    load_text_index: eager,
                    sync_mode: sync,
                    auto_build_quiver: false,
                    ..Default::default()
                };
                let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
                db.insert_with_id(1, &[1., 0.], json!({"version":"old"}))
                    .unwrap();
                db.flush().unwrap();
                db.delete(1).unwrap();
                db.insert_with_id(1, &[0., 1.], json!({"version":"new"}))
                    .unwrap();
                db.index_text(1, "newtoken").unwrap();
                db.index_keyword(1, "newanchor").unwrap();
                {
                    let _failure = fail_io_at(IoPoint::CheckpointWalClear);
                    assert_eq!(db.flush().unwrap_err().code(), "TDB_IO");
                    drop(db);
                }
                let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
                db.build_text_index().unwrap();
                assert_eq!(db.get_payload(1).unwrap(), Some(json!({"version":"new"})));
                assert_eq!(hits(&db, "newtoken"), [1]);
                assert_eq!(hits(&db, "newanchor"), [1]);
                db.close().unwrap();
                let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
                db.build_text_index().unwrap();
                assert_eq!(hits(&db, "newtoken"), [1]);
                assert_eq!(hits(&db, "newanchor"), [1]);
                db.close().unwrap();
                // 独立写入已确认日志，保留关闭后的文本检查点作为恢复起点。
                let mut wal = triviumdb::storage::wal::Wal::open_with_sync(
                    &path,
                    triviumdb::storage::wal::SyncMode::Full,
                )
                .unwrap();
                wal.append(&triviumdb::storage::wal::WalEntry::<f32>::Delete { id: 1 })
                    .unwrap();
                wal.append(&triviumdb::storage::wal::WalEntry::<f32>::Insert {
                    id: 1,
                    vector: vec![1., 0.],
                    payload: json!({"version":"later"}).to_string(),
                })
                .unwrap();
                drop(wal);
                let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
                db.build_text_index().unwrap();
                assert_eq!(db.get_payload(1).unwrap(), Some(json!({"version":"later"})));
                assert!(hits(&db, "newtoken").is_empty());
                assert!(hits(&db, "newanchor").is_empty());
                db.close().unwrap();
                let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
                db.build_text_index().unwrap();
                assert!(hits(&db, "newtoken").is_empty());
                assert!(hits(&db, "newanchor").is_empty());
                db.close().unwrap();
            }
        }
    }
}

fn hits(db: &Database<f32>, query: &str) -> Vec<u64> {
    let config = SearchConfig {
        top_k: 10,
        recall_k: 10,
        expand_depth: 0,
        min_score: -1.,
        enable_text_hybrid_search: true,
        enable_advanced_pipeline: false,
        ..Default::default()
    };
    db.search_hybrid(Some(query), None, &config)
        .unwrap()
        .iter()
        .map(|hit| hit.id)
        .collect()
}

#[test]
fn 文本与派生索引发布故障支持重开恢复及同句柄重试() {
    for mode in [StorageMode::Mmap, StorageMode::Rom] {
        for point in [
            IoPoint::TextMetadataWrite,
            IoPoint::PropertySidecarWrite,
            IoPoint::GraphSidecarWrite,
            IoPoint::PublicationCommitSync,
            IoPoint::PublicationCommitRename,
        ] {
            if mode == StorageMode::Mmap
                && matches!(
                    point,
                    IoPoint::PublicationCommitSync | IoPoint::PublicationCommitRename
                )
            {
                continue;
            }
            for retry in [false, true] {
                let dir = std::env::temp_dir().join(format!(
                    "tdb-text-io-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir(&dir).unwrap();
                let path = dir.join("data.tdb").to_str().unwrap().to_owned();
                let open = || {
                    Database::<f32>::open_with_config(
                        &path,
                        Config {
                            dim: 3,
                            storage_mode: mode,
                            load_text_index: true,
                            auto_build_quiver: false,
                            ..Default::default()
                        },
                    )
                    .unwrap()
                };
                let mut db = open();
                db.insert_with_id(1, &[1., 0., 0.], json!({"version":"旧"}))
                    .unwrap();
                db.index_text(1, "original").unwrap();
                db.index_keyword(1, "旧别名").unwrap();
                db.close().unwrap();
                let mut db = open();
                db.index_text(1, "updated").unwrap();
                db.insert_with_id(2, &[0., 1., 0.], json!({"version":"新"}))
                    .unwrap();
                db.create_index("version").unwrap();
                db.link(1, 2, "关联", 1.).unwrap();
                {
                    let _failure = fail_io_at(point);
                    assert!(db.flush().is_err(), "故障应传递：{mode:?} {point:?}");
                }
                if retry {
                    db.flush().unwrap();
                }
                drop(db);
                let mut db = open();
                assert_eq!(db.node_count().unwrap(), 2);
                assert_eq!(db.get_payload(2).unwrap().unwrap(), json!({"version":"新"}));
                assert_eq!(db.get_edges(1).unwrap()[0].target_id, 2);
                assert_eq!(
                    hits(&db, if retry { "updated" } else { "original" }),
                    vec![1]
                );
                assert_eq!(hits(&db, "旧别名"), vec![1]);
                db.flush().unwrap();
                db.close().unwrap();
                let mut db = open();
                assert_eq!(
                    hits(&db, if retry { "updated" } else { "original" }),
                    vec![1]
                );
                db.close().unwrap();
                // 测试目录由本轮独占创建，仅清理直接子文件。
                for entry in std::fs::read_dir(&dir).unwrap() {
                    std::fs::remove_file(entry.unwrap().path()).unwrap();
                }
                std::fs::remove_dir(&dir).unwrap();
            }
        }
    }
}
