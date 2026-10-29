//! 完整坏帧必须保留日志并拒绝打开；物理未写完的尾帧只恢复已完成前缀。
use super::support::TestDir;
use serde_json::json;
use triviumdb::storage::wal::{SyncMode, Wal, WalEntry};
use triviumdb::{Database, TriviumError};

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
    std::fs::read(format!("{path}.wal")).unwrap()
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
            let frame = frames(&valid)[index].clone();
            if bad_body {
                // CRC 合法但枚举判别值无效，单独验证结构解码契约。
                corrupt[frame.start + 4..frame.start + 8].fill(255);
                let checksum = crc32fast::hash(&corrupt[frame.start + 4..frame.end - 4]);
                corrupt[frame.end - 4..frame.end].copy_from_slice(&checksum.to_le_bytes());
            } else {
                corrupt[frame.end - 1] ^= 1;
            }
            std::fs::write(format!("{path}.wal"), &corrupt).unwrap();
            for _ in 0..2 {
                assert!(matches!(
                    Wal::read_entries::<f32>(&path),
                    Err(TriviumError::CorruptedFile(_))
                ));
                assert!(matches!(
                    Database::<f32>::open(&path, 2),
                    Err(TriviumError::CorruptedFile(_))
                ));
                assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), corrupt);
            }
            // 恢复测试副本后，三条已确认事实都必须仍能回放并再次持久化。
            std::fs::write(format!("{path}.wal"), valid).unwrap();
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
    let original = sample(&root.file("original.tdb"));
    let last = frames(&original)[2].clone();
    for length in last.start..last.end {
        let path = root.file(&format!("tail-{length}.tdb"));
        Database::<f32>::open(&path, 2).unwrap().close().unwrap();
        std::fs::write(format!("{path}.wal"), &original[..length]).unwrap();
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
        std::fs::write(format!("{path}.wal"), &bytes).unwrap();
        assert!(matches!(
            Database::<f32>::open(&path, 2),
            Err(TriviumError::CorruptedFile(_))
        ));
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), bytes);
    }
}
