use serde_json::json;
use std::path::{Path, PathBuf};
use triviumdb::database::{AccessMode, Config, StorageMode};
use triviumdb::resources::{ResourceLimits, ResourceRuntime};
use triviumdb::storage::wal::{SyncMode, Wal, WalEntry};
use triviumdb::{Database, TriviumError, VectorType};

struct Sample(PathBuf);
impl Sample {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "tdb-kernel-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> String {
        self.0.join("中文 数据.tdb").to_str().unwrap().to_owned()
    }
}
impl Drop for Sample {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn facts(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_str().unwrap().to_owned(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}
fn config(mode: StorageMode) -> Config {
    Config {
        dim: 2,
        storage_mode: mode,
        auto_build_quiver: false,
        ..Default::default()
    }
}

fn dtype_case<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    stored: &'static str,
    vector: &[T],
) {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let sample = Sample::new();
        let path = sample.path();
        let mut db = Database::<T>::open_with_config(&path, config(mode)).unwrap();
        db.insert_with_id(1, vector, json!({"x":1})).unwrap();
        db.close().unwrap();
        drop(db);
        let before = facts(&sample.0);
        for access_mode in [AccessMode::ReadOnly, AccessMode::ReadWrite] {
            let cfg = Config {
                access_mode,
                ..config(mode)
            };
            let assert_mismatch = |error: TriviumError, requested| {
                assert!(matches!(error, TriviumError::DtypeMismatch { .. }));
                assert_eq!(
                    error.details(),
                    json!({"stored":stored,"requested":requested})
                );
            };
            if stored != "f32" {
                assert_mismatch(
                    Database::<f32>::open_with_config(&path, cfg).err().unwrap(),
                    "f32",
                );
            }
            if stored != "f16" {
                assert_mismatch(
                    Database::<half::f16>::open_with_config(&path, cfg)
                        .err()
                        .unwrap(),
                    "f16",
                );
            }
            if stored != "u64" {
                assert_mismatch(
                    Database::<u64>::open_with_config(&path, cfg).err().unwrap(),
                    "u64",
                );
            }
            assert_eq!(facts(&sample.0), before, "拒绝错误类型时保持整个文件组");
        }
        let mut correct = Database::<T>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(correct.get(1).unwrap().unwrap().vector, vector);
        correct.close().unwrap();
    }
}
#[test]
fn persistent_dtype_pair_matrix() {
    dtype_case("f32", &[1.0f32, 2.0]);
    dtype_case("f16", &[half::f16::ONE, half::f16::from_f32(2.0)]);
    dtype_case("u64", &[9_007_199_254_740_993u64, u64::MAX]);
}

#[test]
fn typed_empty_checkpoint_protects_uncheckpointed_wal() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let sample = Sample::new();
        let path = sample.path();
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        db.close().unwrap();
        drop(db);
        let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
        wal.append(&WalEntry::Insert {
            id: 1,
            vector: vec![1.0f32, 2.0],
            payload: "{\"x\":7}".into(),
        })
        .unwrap();
        drop(wal);
        let before = facts(&sample.0);
        assert!(matches!(
            Database::<half::f16>::open_with_config(&path, config(mode)),
            Err(TriviumError::DtypeMismatch { .. })
        ));
        assert_eq!(facts(&sample.0), before);
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        let node = db.get(1).unwrap().unwrap();
        assert_eq!(node.vector, vec![1.0, 2.0]);
        assert_eq!(node.payload, json!({"x":7}));
        db.close().unwrap();
    }
}

#[test]
fn historical_checkpoint_geometry_establishes_dtype_without_rewriting() {
    let sample = Sample::new();
    let path = sample.path();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v0810_text_mmap");
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if let Some(suffix) = name.strip_prefix("sample.tdb") {
            std::fs::copy(entry.path(), format!("{path}{suffix}")).unwrap();
        }
    }
    std::fs::File::create(format!("{path}.lock")).unwrap();
    let before = facts(&sample.0);
    for access_mode in [AccessMode::ReadOnly, AccessMode::ReadWrite] {
        let cfg = Config {
            dim: 4,
            access_mode,
            auto_build_quiver: false,
            ..Default::default()
        };
        assert!(matches!(
            Database::<half::f16>::open_with_config(&path, cfg),
            Err(TriviumError::DtypeMismatch {
                stored: "f32",
                requested: "f16"
            })
        ));
        assert!(matches!(
            Database::<u64>::open_with_config(&path, cfg),
            Err(TriviumError::DtypeMismatch {
                stored: "f32",
                requested: "u64"
            })
        ));
        assert_eq!(facts(&sample.0), before);
    }
    let mut db = Database::<f32>::open_read_only(&path, 4).unwrap();
    assert_eq!(db.node_count().unwrap(), 4);
    db.close().unwrap();
    assert_eq!(facts(&sample.0), before);
}

fn write_legacy_empty(path: &str) {
    // 独立构造无 dtype 的 v9 空 Rom 检查点。
    let mut bytes = vec![0u8; 58];
    bytes[..4].copy_from_slice(b"TVDB");
    bytes[4..6].copy_from_slice(&9u16.to_le_bytes());
    bytes[6..10].copy_from_slice(&2u32.to_le_bytes());
    bytes[10..18].copy_from_slice(&1u64.to_le_bytes());
    for offset in [26, 34, 42, 50] {
        bytes[offset..offset + 8].copy_from_slice(&58u64.to_le_bytes());
    }
    bytes.extend_from_slice(b"TBQF\x01\x00\x30\x00\x00\x00\x00\x00\x00\x00\x00\x00");
    let unique = b"TUQC\x01\x00\x00\x00\x00\x00";
    bytes.extend_from_slice(unique);
    bytes.extend_from_slice(&crc32fast::hash(unique).to_le_bytes());
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn untyped_wal_requires_original_version_recovery() {
    for legacy_checkpoint in [false, true] {
        let sample = Sample::new();
        let path = sample.path();
        if legacy_checkpoint {
            write_legacy_empty(&path);
        }
        let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
        wal.append(&WalEntry::Insert {
            id: 1,
            vector: vec![1.0f32, 2.0],
            payload: "{}".into(),
        })
        .unwrap();
        drop(wal);
        std::fs::File::create(format!("{path}.lock")).unwrap();
        let before = facts(&sample.0);
        assert!(matches!(
            Database::<f32>::open(&path, 2),
            Err(TriviumError::CorruptedFile(_))
        ));
        assert!(matches!(
            Database::<half::f16>::open(&path, 2),
            Err(TriviumError::CorruptedFile(_))
        ));
        assert_eq!(facts(&sample.0), before);
    }
}

#[cfg(windows)]
#[test]
fn empty_vector_file_delete_failure_can_retry_without_false_publication() {
    use std::os::windows::fs::OpenOptionsExt;
    let sample = Sample::new();
    let path = sample.path();
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    std::fs::write(format!("{path}.vec"), [0u8; 8]).unwrap();
    let blocker = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(format!("{path}.vec"))
        .unwrap();
    assert!(matches!(db.flush(), Err(TriviumError::Io(_))));
    assert!(!Path::new(&format!("{path}.flush_ok")).exists());
    drop(blocker);
    db.flush().unwrap();
    db.close().unwrap();
    drop(db);
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    assert_eq!(db.node_count().unwrap(), 0);
    db.close().unwrap();
}

#[test]
fn opening_existing_facts_obeys_instance_and_shared_admission() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let sample = Sample::new();
        let path = sample.path();
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        for id in 1..=128 {
            db.insert_with_id(id, &[1.0, 2.0], json!({"id":id}))
                .unwrap();
        }
        db.publish_generation_manifest("budget-base").unwrap();
        db.close().unwrap();
        drop(db);
        for access_mode in [
            AccessMode::ReadWrite,
            AccessMode::ReadOnly,
            AccessMode::Immutable,
        ] {
            let runtime = ResourceRuntime::new(ResourceLimits {
                memory_limit: 32_768,
                max_threads: 1,
                ..Default::default()
            })
            .unwrap();
            let before = facts(&sample.0);
            // 1 KiB 小于 128 条记录的 ID/slot 目录，拒绝发生于加载而非后续写入。
            for _ in 0..3 {
                assert!(matches!(
                    Database::<f32>::open_with_runtime(
                        &path,
                        Config {
                            memory_limit: 1024,
                            access_mode,
                            ..config(mode)
                        },
                        runtime.clone()
                    ),
                    Err(TriviumError::CapacityReservationRejected { .. })
                ));
                assert_eq!(runtime.stats().reserved_database_bytes, 0);
                assert_eq!(runtime.stats().attached_databases, 0);
                assert_eq!(facts(&sample.0), before);
            }
        }
        let mut adequate = Database::<f32>::open_with_config(
            &path,
            Config {
                memory_limit: 1024 * 1024,
                ..config(mode)
            },
        )
        .unwrap();
        assert_eq!(adequate.node_count().unwrap(), 128);
        for id in 1..=128 {
            assert_eq!(adequate.get(id).unwrap().unwrap().payload, json!({"id":id}));
        }
        adequate.close().unwrap();
    }
}

#[test]
fn graph_and_derived_index_loads_require_peak_headroom() {
    for component in ["graph", "property", "text", "quiver"] {
        let sample = Sample::new();
        let path = sample.path();
        let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
        for id in 1..=128 {
            db.insert_with_id(
                id,
                &[1.0, 0.0],
                json!({"key":format!("{id}-{}", "k".repeat(256))}),
            )
            .unwrap();
        }
        db.close().unwrap();
        drop(db);
        // 相同事实在无派生索引、无长标签时可在该预算内重开。
        let mut baseline = Database::<f32>::open_with_config(
            &path,
            Config {
                memory_limit: 64 * 1024,
                ..config(StorageMode::Mmap)
            },
        )
        .unwrap();
        assert_eq!(baseline.node_count().unwrap(), 128);
        baseline.close().unwrap();
        drop(baseline);
        let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
        match component {
            // 标签本身在各图目录中持有多份，超过 256 KiB 准入预算。
            "graph" => {
                db.link(1, 2, &"边".repeat(20_000), 1.0).unwrap();
            }
            "property" => {
                db.create_index("key").unwrap();
            }
            "text" => {
                for id in 1..=128 {
                    db.index_text(id, &format!("{} {}", "word".repeat(128), id))
                        .unwrap();
                }
                db.build_text_index().unwrap();
            }
            _ => {
                db.build_quiver_index(None).unwrap();
            }
        }
        db.close().unwrap();
        drop(db);
        let before = facts(&sample.0);
        let limit = if component == "graph" {
            256 * 1024
        } else if component == "property" {
            1024 * 1024
        } else {
            64 * 1024
        };
        assert!(
            matches!(
                Database::<f32>::open_with_config(
                    &path,
                    Config {
                        memory_limit: limit,
                        load_text_index: component == "text",
                        ..config(StorageMode::Mmap)
                    }
                ),
                Err(TriviumError::CapacityReservationRejected { .. })
            ),
            "{component}"
        );
        assert_eq!(facts(&sample.0), before, "{component}");
        let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
        assert_eq!(
            db.get(128).unwrap().unwrap().payload,
            json!({"key":format!("128-{}","k".repeat(256))})
        );
        db.close().unwrap();
    }
}

#[test]
fn exact_search_rejects_missing_live_vectors_instead_of_empty_hits() {
    let mut mt = triviumdb::storage::memtable::MemTable::<f32>::new(2);
    mt.register_node_raw(1, b"{}").unwrap();
    assert!(matches!(
        triviumdb::index::exact::search(&mt, &[1.0, 0.0], 1),
        Err(TriviumError::CorruptedFile(_))
    ));
}

#[test]
fn refused_wal_recovery_preserves_log_and_can_retry() {
    let sample = Sample::new();
    let path = sample.path();
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    db.close().unwrap();
    drop(db);
    let payload = json!({"text":"大正文".repeat(4096)});
    let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
    wal.append_batch(
        7,
        &[WalEntry::Insert {
            id: 1,
            vector: vec![1.0f32, 2.0],
            payload: payload.to_string(),
        }],
    )
    .unwrap();
    drop(wal);
    let before = facts(&sample.0);
    assert!(matches!(
        Database::<f32>::open_with_config(
            &path,
            Config {
                memory_limit: 32 * 1024,
                ..config(StorageMode::Mmap)
            }
        ),
        Err(TriviumError::CapacityReservationRejected { .. })
    ));
    assert_eq!(facts(&sample.0), before);
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            memory_limit: 8 * 1024 * 1024,
            ..config(StorageMode::Mmap)
        },
    )
    .unwrap();
    assert_eq!(db.get(1).unwrap().unwrap().payload, payload);
    db.close().unwrap();
    drop(db);
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    assert_eq!(db.get(1).unwrap().unwrap().payload, payload);
    db.close().unwrap();
}

#[cfg(windows)]
#[test]
fn blocked_vector_replacement_requires_reopen_and_recovers_wal() {
    let sample = Sample::new();
    let path = sample.path();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "kernel_boundaries::blocked_vector_worker",
            "--nocapture",
        ])
        .env("TDB_BLOCKED_VECTOR_SAMPLE", &path)
        .status()
        .unwrap();
    assert!(status.success(), "受控文件占用子进程失败：{status}");
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    assert_eq!(db.get(1).unwrap().unwrap().vector, vec![0.0, 1.0]);
    assert_eq!(db.search_exact(&[0.0, 1.0], 1).unwrap()[0].id, 1);
    db.close().unwrap();
    drop(db);
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    assert_eq!(db.get(1).unwrap().unwrap().vector, vec![0.0, 1.0]);
    db.close().unwrap();
}

#[cfg(windows)]
#[test]
#[ignore = "由父用例以独占样本执行并模拟进程退出"]
fn blocked_vector_worker() {
    use std::os::windows::fs::OpenOptionsExt;
    let path = std::env::var("TDB_BLOCKED_VECTOR_SAMPLE").unwrap();
    let mut db = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    db.insert_with_id(1, &[1.0, 0.0], json!({"x":1})).unwrap();
    db.flush().unwrap();
    let blocker = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(3)
        .open(format!("{path}.vec"))
        .unwrap();
    db.update_vector(1, &[0.0, 1.0]).unwrap();
    assert!(matches!(db.flush(), Err(TriviumError::Io(_))));
    assert!(matches!(db.get(1), Err(TriviumError::WalClosed)));
    assert!(matches!(
        db.search_exact(&[0.0, 1.0], 1),
        Err(TriviumError::WalClosed)
    ));
    assert!(matches!(
        db.insert(&[1.0, 0.0], json!({})),
        Err(TriviumError::WalClosed)
    ));
    assert!(matches!(
        db.search_batch(&[vec![0.0, 1.0]], &Default::default(), &Default::default()),
        Err(TriviumError::WalClosed)
    ));
    drop(blocker);
    assert!(matches!(db.flush(), Err(TriviumError::WalClosed)));
    drop(db);
    let mut reopened = Database::<f32>::open_with_config(&path, config(StorageMode::Mmap)).unwrap();
    assert_eq!(reopened.get(1).unwrap().unwrap().vector, vec![0.0, 1.0]);
    reopened.close().unwrap();
    std::process::exit(0);
}

#[test]
fn empty_legacy_type_upgrade_preserves_index_definitions_as_a_file_group() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let sample = Sample::new();
        let path = sample.path();
        write_legacy_empty(&path);
        let mut mt = triviumdb::storage::memtable::MemTable::<f32>::new(2);
        mt.register_property_index("key");
        triviumdb::index::property::save_sidecar(mt.property_indexes(), &path, 0, (0, 0)).unwrap();
        let db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(db.index_info().unwrap().len(), 1);
        // Drop 只排空 WAL，验证打开时的迁移已独立发布完整文件组。
        drop(db);
        let mut readonly = Database::<f32>::open_read_only(&path, 2).unwrap();
        let info = readonly.index_info().unwrap();
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].field, "key");
        assert_eq!(info[0].entry_count, 0);
        readonly.close().unwrap();
    }
}
