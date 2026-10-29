//! CRC 合法帧仍须满足恢复语义；失败必须保持原文件组并允许再次诊断。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use triviumdb::database::{Config, StorageMode};
use triviumdb::storage::wal::{SyncMode, Wal, WalEntry};
use triviumdb::{Database, TriviumError, VectorType};

struct TestDir(std::path::PathBuf);
impl TestDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "tdb_wal_semantics_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> String {
        self.0.join("sample.tdb").to_str().unwrap().to_owned()
    }
    fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(&self.0)
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
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(mode: StorageMode) -> Config {
    Config {
        dim: 4,
        storage_mode: mode,
        auto_build_quiver: false,
        ..Default::default()
    }
}
fn vector<T: VectorType>() -> Vec<T> {
    [1., 0., 0., 0.].into_iter().map(T::from_f32).collect()
}
fn insert<T: VectorType>(id: u64, payload: &str) -> WalEntry<T> {
    WalEntry::Insert {
        id,
        vector: vector(),
        payload: payload.to_owned(),
    }
}

fn reject<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    entries: Vec<(&str, WalEntry<T>)>,
) {
    for (name, invalid) in entries {
        let invalid_bytes = bincode::serialize(&invalid).unwrap();
        for mode in [StorageMode::Rom, StorageMode::Mmap] {
            for checkpoint in [false, true] {
                for batch in [false, true] {
                    let dir = TestDir::new();
                    let path = dir.path();
                    if checkpoint {
                        let mut db = Database::<T>::open_with_config(&path, config(mode)).unwrap();
                        db.insert_with_id(10, &vector(), serde_json::json!({"state":"checkpoint"}))
                            .unwrap();
                        db.insert_with_id(20, &vector(), serde_json::Value::Null)
                            .unwrap();
                        db.close().unwrap();
                    }
                    if !std::path::Path::new(&path).exists() {
                        Database::<T>::open_with_config(&path, config(mode))
                            .unwrap()
                            .close()
                            .unwrap();
                    }
                    {
                        let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
                        if !checkpoint {
                            wal.append(&insert::<T>(10, "{\"state\":\"checkpoint\"}"))
                                .unwrap();
                            wal.append(&insert::<T>(20, "null")).unwrap();
                        }
                        // 合法前缀先改变恢复中的内存，后续拒绝仍不得发布该半成品。
                        let operations: [WalEntry<T>; 2] = [
                            WalEntry::UpdatePayload {
                                id: 10,
                                payload: "{\"state\":\"unpublished\"}".into(),
                            },
                            bincode::deserialize(&invalid_bytes).unwrap(),
                        ];
                        if batch {
                            wal.append_batch(7, &operations).unwrap();
                        } else {
                            for entry in &operations {
                                wal.append(entry).unwrap();
                            }
                        }
                    }
                    let (decoded, _) = Wal::read_entries::<T>(&path).unwrap();
                    assert_eq!(decoded.len(), if checkpoint { 2 } else { 4 });
                    // 可写打开使用锁文件；预先建立它，随后逐字节比较完整文件集合。
                    std::fs::OpenOptions::new()
                        .create(true)
                        .truncate(false)
                        .write(true)
                        .open(format!("{path}.lock"))
                        .unwrap();
                    let before = dir.snapshot();
                    for _ in 0..2 {
                        let result = Database::<T>::open_with_config(&path, config(mode));
                        assert!(
                            matches!(result, Err(TriviumError::CorruptedFile(_))),
                            "{name}, {mode:?}, 检查点={checkpoint}, 事务帧={batch}"
                        );
                        assert_eq!(dir.snapshot(), before, "{name}: 恢复失败不得修改原文件组");
                    }
                }
            }
        }
    }
}

fn malformed<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>() {
    reject(vec![
        ("插入 JSON 不完整", insert::<T>(30, "{")),
        ("重复 ID 的 JSON 也必须校验", insert::<T>(10, "{")),
        ("JSON 后缀非法", insert::<T>(30, "null trailing")),
        ("JSON 为空", insert::<T>(30, "")),
        (
            "更新 JSON 非法",
            WalEntry::UpdatePayload {
                id: 10,
                payload: "{".into(),
            },
        ),
        (
            "跳过不存在节点前校验 JSON",
            WalEntry::UpdatePayload {
                id: 99,
                payload: "{".into(),
            },
        ),
        (
            "边元数据非法",
            WalEntry::Link {
                src: 10,
                dst: 20,
                label: "knows".into(),
                weight: 1.,
                metadata: "{".into(),
            },
        ),
        (
            "缺失端点也校验元数据",
            WalEntry::Link {
                src: 10,
                dst: 99,
                label: "knows".into(),
                weight: 1.,
                metadata: "{".into(),
            },
        ),
        (
            "插入维度错误",
            WalEntry::Insert {
                id: 30,
                vector: vec![T::from_f32(1.)],
                payload: "null".into(),
            },
        ),
        (
            "重复 ID 也校验维度",
            WalEntry::Insert {
                id: 10,
                vector: vec![T::from_f32(1.)],
                payload: "null".into(),
            },
        ),
        (
            "更新维度错误",
            WalEntry::UpdateVector {
                id: 10,
                vector: vec![T::from_f32(1.)],
            },
        ),
        (
            "缺失 ID 也校验维度",
            WalEntry::UpdateVector {
                id: 99,
                vector: vec![T::from_f32(1.)],
            },
        ),
        ("保留 ID", insert::<T>(0, "null")),
        ("ID 推进溢出", insert::<T>(u64::MAX, "null")),
        (
            "非法边权",
            WalEntry::Link {
                src: 10,
                dst: 20,
                label: "knows".into(),
                weight: f32::NAN,
                metadata: "null".into(),
            },
        ),
    ]);
}

#[test]
fn malformed_semantics_f32() {
    malformed::<f32>();
}
#[test]
fn malformed_semantics_f16() {
    malformed::<half::f16>();
}
#[test]
fn malformed_semantics_u64() {
    malformed::<u64>();
}

#[test]
fn nonfinite_vectors_are_rejected() {
    fn check<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>() {
        let mut entries = Vec::new();
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let vector = vec![T::from_f32(invalid); 4];
            entries.push((
                "非有限插入向量",
                WalEntry::Insert {
                    id: 30,
                    vector: vector.clone(),
                    payload: "null".into(),
                },
            ));
            entries.push(("非有限更新向量", WalEntry::UpdateVector { id: 10, vector }));
        }
        reject(entries);
    }
    check::<f32>();
    check::<half::f16>();
}

#[test]
fn valid_null_and_checkpoint_idempotence_remain_supported() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let dir = TestDir::new();
        let path = dir.path();
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        db.insert_with_id(
            10,
            &[0., 1., 0., 0.],
            serde_json::json!({"state":"checkpoint"}),
        )
        .unwrap();
        db.close().unwrap();
        Database::<f32>::open_with_config(&path, config(mode))
            .unwrap()
            .close()
            .unwrap();
        {
            let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
            for entry in [
                insert::<f32>(10, "{\"state\":\"stale\"}"),
                insert(30, "null"),
                WalEntry::UpdatePayload {
                    id: 30,
                    payload: "null".into(),
                },
                WalEntry::Link {
                    src: 10,
                    dst: 30,
                    label: "null-metadata".into(),
                    weight: 2.,
                    metadata: "null".into(),
                },
                WalEntry::Delete { id: 99 },
                WalEntry::UpdatePayload {
                    id: 99,
                    payload: "{}".into(),
                },
                WalEntry::UpdateVector {
                    id: 99,
                    vector: vec![1., 0., 0., 0.],
                },
                WalEntry::Unlink { src: 99, dst: 10 },
                WalEntry::UnlinkLabel {
                    src: 99,
                    dst: 10,
                    label: "missing".into(),
                },
            ] {
                wal.append(&entry).unwrap();
            }
        }
        for _ in 0..2 {
            let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
            assert_eq!(db.node_count().unwrap(), 2);
            let node = db.get(10).unwrap().unwrap();
            assert_eq!(node.vector, [0., 1., 0., 0.]);
            assert_eq!(node.payload, serde_json::json!({"state":"checkpoint"}));
            assert_eq!(db.get_payload(30).unwrap(), Some(serde_json::Value::Null));
            let edges = db.get_edges(10).unwrap();
            assert_eq!(edges.len(), 1);
            assert_eq!(
                (edges[0].target_id, edges[0].label.as_str(), edges[0].weight),
                (30, "null-metadata", 2.)
            );
            assert_eq!(edges[0].metadata, serde_json::Value::Null);
            db.close().unwrap();
        }
    }
}

#[test]
fn uncommitted_semantically_invalid_entry_is_not_replayed() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let dir = TestDir::new();
        let path = dir.path();
        let committed_offset;
        Database::<f32>::open_with_config(&path, config(mode))
            .unwrap()
            .close()
            .unwrap();
        {
            let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
            wal.append(&insert::<f32>(10, "{\"state\":\"committed\"}"))
                .unwrap();
            committed_offset = std::fs::metadata(format!("{path}.wal")).unwrap().len();
            wal.append(&WalEntry::<f32>::TxBegin { tx_id: 7 }).unwrap();
            wal.append(&insert::<f32>(30, "{")).unwrap();
        }
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        assert_eq!(db.node_count().unwrap(), 1);
        let node = db.get(10).unwrap().unwrap();
        assert_eq!(node.vector, [1., 0., 0., 0.]);
        assert_eq!(node.payload, serde_json::json!({"state":"committed"}));
        assert!(db.get(30).unwrap().is_none());
        assert_eq!(
            std::fs::metadata(format!("{path}.wal")).unwrap().len(),
            committed_offset
        );
        db.close().unwrap();
    }
}

#[test]
fn last_valid_node_id_recovers_without_wrapping_allocator() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let dir = TestDir::new();
        let path = dir.path();
        Database::<f32>::open_with_config(&path, config(mode))
            .unwrap()
            .close()
            .unwrap();
        {
            let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
            wal.append(&insert::<f32>(u64::MAX - 1, "null")).unwrap();
        }
        let mut db = Database::<f32>::open_with_config(&path, config(mode)).unwrap();
        let before = std::fs::read(format!("{path}.wal")).unwrap();
        let mut tx = db.begin_tx();
        tx.insert(&[1., 0., 0., 0.], serde_json::Value::Null);
        assert!(matches!(tx.commit(), Err(TriviumError::InvalidInput(_))));
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
        assert_eq!(db.node_count().unwrap(), 1);
        let node = db.get(u64::MAX - 1).unwrap().unwrap();
        assert_eq!(node.id, u64::MAX - 1);
        assert_eq!(node.vector, [1., 0., 0., 0.]);
        assert_eq!(node.payload, serde_json::Value::Null);
        db.close().unwrap();
    }
}
