//! 完整坏帧必须保留日志并拒绝打开；物理未写完的尾帧只恢复已完成前缀。
use super::support::TestDir;
use serde_json::json;
use triviumdb::storage::wal::{SyncMode, Wal, WalEntry};
use triviumdb::{Database, TriviumError};

#[test]
fn 文本检查点元数据按独立布局校验并拒绝损坏与截断() {
    let root = TestDir::new("text_metadata_integrity");
    let path = root.file("data.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"name":"saved"}))
        .unwrap();
    db.index_text(1, "preserved").unwrap();
    db.index_keyword(1, "keyword").unwrap();
    db.close().unwrap();
    let meta_path = format!("{path}.text.meta");
    let valid = crate::artifact::read(&meta_path).unwrap();
    let main = crate::artifact::read(&path).unwrap();
    let text = crate::artifact::read(format!("{path}.text")).unwrap();
    let base = crate::artifact::read(format!("{path}.wal")).unwrap();
    assert_eq!(&base[..6], b"TVWL\x07\x00");
    assert_eq!(&base[6..10], &20u32.to_le_bytes());
    assert_eq!(&base[10..14], &12u32.to_le_bytes());
    assert_eq!(base.len(), 34);
    let mut expected = b"TMET\x02\x00\x00\x00".to_vec();
    expected.extend_from_slice(&(main.len() as u64).to_le_bytes());
    expected.extend_from_slice(&(text.len() as u64).to_le_bytes());
    expected.extend_from_slice(&crc32fast::hash(&text).to_le_bytes());
    expected.extend_from_slice(&crc32fast::hash(&main).to_le_bytes());
    expected.extend_from_slice(&base[14..30]);
    expected.extend_from_slice(&crc32fast::hash(&expected).to_le_bytes());
    assert_eq!(valid, expected);
    let mut mismatched = base.clone();
    mismatched[14] ^= 1;
    let checksum = crc32fast::hash(&mismatched[10..30]);
    mismatched[30..34].copy_from_slice(&checksum.to_le_bytes());
    crate::artifact::write(format!("{path}.wal"), &mismatched).unwrap();
    assert_eq!(
        Database::<f32>::open(&path, 2).err().unwrap().code(),
        "TDB_CORRUPTED_FILE"
    );
    assert_eq!(
        crate::artifact::read(format!("{path}.wal")).unwrap(),
        mismatched
    );
    let mut old_version = base.clone();
    old_version[4..6].copy_from_slice(&6u16.to_le_bytes());
    crate::artifact::write(format!("{path}.wal"), &old_version).unwrap();
    assert_eq!(
        Database::<f32>::open(&path, 2).err().unwrap().code(),
        "TDB_MIGRATION_REQUIRED"
    );
    assert_eq!(
        crate::artifact::read(format!("{path}.wal")).unwrap(),
        old_version
    );
    let mut missing = b"TVWL\x07\x00".to_vec();
    let mut deletion = 4u32.to_le_bytes().to_vec();
    deletion.extend_from_slice(&1u64.to_le_bytes());
    missing.extend_from_slice(&(deletion.len() as u32).to_le_bytes());
    missing.extend_from_slice(&deletion);
    missing.extend_from_slice(&crc32fast::hash(&deletion).to_le_bytes());
    crate::artifact::write(format!("{path}.wal"), &missing).unwrap();
    assert_eq!(
        Database::<f32>::open(&path, 2).err().unwrap().code(),
        "TDB_RECOVERY_REQUIRED"
    );
    assert_eq!(
        crate::artifact::read(format!("{path}.wal")).unwrap(),
        missing
    );
    assert_eq!(crate::artifact::read(format!("{path}.text")).unwrap(), text);
    crate::artifact::write(format!("{path}.wal"), base).unwrap();
    let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
    wal.append(&WalEntry::<f32>::Delete { id: 1 }).unwrap();
    drop(wal);
    let wal_bytes = crate::artifact::read(format!("{path}.wal")).unwrap();
    for length in 0..valid.len() {
        crate::artifact::write(&meta_path, &valid[..length]).unwrap();
        assert_eq!(
            Database::<f32>::open(&path, 2).err().unwrap().code(),
            "TDB_CORRUPTED_FILE"
        );
        assert_eq!(crate::artifact::read(&meta_path).unwrap(), valid[..length]);
        assert_eq!(
            crate::artifact::read(format!("{path}.wal")).unwrap(),
            wal_bytes
        );
    }
    for offset in 0..valid.len() {
        let mut corrupt = valid.clone();
        corrupt[offset] ^= 1;
        crate::artifact::write(&meta_path, &corrupt).unwrap();
        assert_eq!(
            Database::<f32>::open(&path, 2).err().unwrap().code(),
            "TDB_CORRUPTED_FILE"
        );
        assert_eq!(crate::artifact::read(&meta_path).unwrap(), corrupt);
    }
    for (start, end) in [(8, 16), (28, 32)] {
        let mut corrupt = valid.clone();
        corrupt[start..end].fill(255);
        let checksum = crc32fast::hash(&corrupt[..48]);
        corrupt[48..52].copy_from_slice(&checksum.to_le_bytes());
        crate::artifact::write(&meta_path, &corrupt).unwrap();
        assert_eq!(
            Database::<f32>::open(&path, 2).err().unwrap().code(),
            "TDB_CORRUPTED_FILE"
        );
        assert_eq!(crate::artifact::read(&meta_path).unwrap(), corrupt);
        assert_eq!(
            crate::artifact::read(format!("{path}.wal")).unwrap(),
            wal_bytes
        );
    }
    crate::artifact::write(&meta_path, valid).unwrap();
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    assert!(db.get(1).unwrap().is_none());
    db.close().unwrap();
}

fn sample(path: &str) -> Vec<u8> {
    Database::<f32>::open(path, 2).unwrap().close().unwrap();
    let mut wal = Wal::open_with_sync(path, SyncMode::Full).unwrap();
    for id in 1..=3 {
        wal.append(&WalEntry::Insert {
            id,
            vector: vec![id as f32, 0.],
            payload: json!({"id":id}).to_string(),
        })
        .unwrap();
    }
    drop(wal);
    crate::artifact::read(format!("{path}.wal")).unwrap()
}

fn frames(bytes: &[u8]) -> Vec<std::ops::Range<usize>> {
    let mut offset = 6;
    let mut result = Vec::new();
    while offset < bytes.len() {
        let size = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        result.push(offset..offset + size + 8);
        offset += size + 8;
    }
    result
}

#[test]
fn 完整坏帧在首中尾均拒绝且反复打开保留原始日志() {
    for index in 0..3 {
        for bad_body in [false, true] {
            let root = TestDir::new("wal_complete_corruption");
            let path = root.file("data.tdb");
            let valid = sample(&path);
            let mut corrupt = valid.clone();
            let frame = frames(&valid)[index + 1].clone();
            if bad_body {
                // CRC 合法但枚举判别值无效，单独验证结构解码契约。
                corrupt[frame.start + 4..frame.start + 8].fill(255);
                let checksum = crc32fast::hash(&corrupt[frame.start + 4..frame.end - 4]);
                corrupt[frame.end - 4..frame.end].copy_from_slice(&checksum.to_le_bytes());
            } else {
                corrupt[frame.end - 1] ^= 1;
            }
            crate::artifact::write(format!("{path}.wal"), &corrupt).unwrap();
            for _ in 0..2 {
                assert!(matches!(
                    Wal::read_entries::<f32>(&path),
                    Err(TriviumError::CorruptedFile(_))
                ));
                assert!(matches!(
                    Database::<f32>::open(&path, 2),
                    Err(TriviumError::CorruptedFile(_))
                ));
                assert_eq!(
                    crate::artifact::read(format!("{path}.wal")).unwrap(),
                    corrupt
                );
            }
            // 恢复测试副本后，三条已确认事实都必须仍能回放并再次持久化。
            crate::artifact::write(format!("{path}.wal"), valid).unwrap();
            let mut db = Database::<f32>::open(&path, 2).unwrap();
            assert_eq!(db.all_node_ids().unwrap(), [1, 2, 3]);
            for id in 1..=3 {
                assert_eq!(db.get_payload(id).unwrap(), Some(json!({"id":id})));
            }
            db.close().unwrap();
        }
    }
}

#[test]
fn 尾帧任意物理截断仅保留两条完整记录() {
    let root = TestDir::new("wal_physical_tail");
    let source = root.file("original.tdb");
    let original = sample(&source);
    let last = frames(&original).last().unwrap().clone();
    for length in last.start..last.end {
        let path = root.file(&format!("tail-{length}.tdb"));
        root.copy_checkpoint(&source, &path);
        crate::artifact::write(format!("{path}.wal"), &original[..length]).unwrap();
        let mut db = Database::<f32>::open(&path, 2).unwrap();
        assert_eq!(db.all_node_ids().unwrap(), [1, 2]);
        assert_eq!(db.get_payload(2).unwrap(), Some(json!({"id":2})));
        db.close().unwrap();
    }
}

#[test]
fn 非法完整长度拒绝且保留已确认后缀() {
    for length in [0, u32::MAX] {
        let root = TestDir::new("wal_length_corruption");
        let path = root.file("data.tdb");
        let mut bytes = sample(&path);
        let middle = frames(&bytes)[1].start;
        bytes[middle..middle + 4].copy_from_slice(&length.to_le_bytes());
        crate::artifact::write(format!("{path}.wal"), &bytes).unwrap();
        assert!(matches!(
            Database::<f32>::open(&path, 2),
            Err(TriviumError::CorruptedFile(_))
        ));
        assert_eq!(crate::artifact::read(format!("{path}.wal")).unwrap(), bytes);
    }
}

#[test]
fn 普通检查点拒绝同维度外库的空封条和已确认写入() {
    for mode in [
        triviumdb::database::StorageMode::Rom,
        triviumdb::database::StorageMode::Mmap,
    ] {
        let dir = TestDir::new("foreign_wal");
        let a = dir.file("a.tdb");
        let b = dir.file("b.tdb");
        let config = || triviumdb::database::Config {
            dim: 2,
            storage_mode: mode,
            sync_mode: SyncMode::Full,
            ..Default::default()
        };
        for (path, value) in [(&a, 1), (&b, 2)] {
            let mut db = Database::<f32>::open_with_config(path, config()).unwrap();
            db.insert_with_id(7, &[1., 0.], json!(value)).unwrap();
            db.close().unwrap();
        }
        let wal = format!("{a}.wal");
        let clean = std::fs::read(&wal).unwrap();
        let mut other = Database::<f32>::open_with_config(&b, config()).unwrap();
        for dirty in [false, true] {
            if dirty {
                other.update_payload(7, json!(3)).unwrap();
            }
            let foreign = std::fs::read(format!("{b}.wal")).unwrap();
            std::fs::write(&wal, &foreign).unwrap();
            let error = Database::<f32>::open_with_config(&a, config())
                .err()
                .unwrap();
            assert_eq!(
                error.code(),
                if dirty {
                    "TDB_RECOVERY_REQUIRED"
                } else {
                    "TDB_CORRUPTED_FILE"
                }
            );
            assert_eq!(std::fs::read(&wal).unwrap(), foreign);
        }
        std::fs::write(&wal, clean).unwrap();
        let mut db = Database::<f32>::open_with_config(&a, config()).unwrap();
        assert_eq!(db.get_payload(7).unwrap(), Some(json!(1)));
        db.close().unwrap();
    }
}
