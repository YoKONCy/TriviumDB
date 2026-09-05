//! WAL 模块的单元测试（从 wal.rs 补齐）
//!
//! 覆盖: Wal append/read/clear/SyncMode 完整生命周期
//! 与 wal_midwrite.rs 的区别: wal_midwrite 测试截断容错，这里测试正常路径

use std::io::Cursor;
use triviumdb::storage::wal::{SyncMode, Wal, WalEntry};

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("unit_wal_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

// ════════════════════════════════════════════════════════════════
//  基础 append + read 往返
// ════════════════════════════════════════════════════════════════

#[test]
fn wal_append_和_read_entries_往返() {
    let path = tmp_db("roundtrip");
    cleanup(&path);

    {
        let mut wal = Wal::open(&path).unwrap();
        wal.append(&WalEntry::Insert::<f32> {
            id: 1,
            vector: vec![1.0, 2.0, 3.0],
            payload: r#"{"name":"alice"}"#.to_string(),
        })
        .unwrap();
        wal.append(&WalEntry::Insert::<f32> {
            id: 2,
            vector: vec![4.0, 5.0, 6.0],
            payload: r#"{"name":"bob"}"#.to_string(),
        })
        .unwrap();
    }

    let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
    assert_eq!(entries.len(), 2);

    match &entries[0] {
        WalEntry::Insert {
            id,
            vector,
            payload,
        } => {
            assert_eq!(*id, 1);
            assert_eq!(*vector, vec![1.0, 2.0, 3.0]);
            assert!(payload.contains("alice"));
        }
        _ => panic!("第一条应为 Insert"),
    }

    cleanup(&path);
}

#[test]
fn wal_所有Entry变体往返() {
    let path = tmp_db("all_variants");
    cleanup(&path);

    {
        let mut wal = Wal::open(&path).unwrap();
        wal.append(&WalEntry::Insert::<f32> {
            id: 1,
            vector: vec![1.0],
            payload: "{}".to_string(),
        })
        .unwrap();
        wal.append(&WalEntry::Link::<f32> {
            src: 1,
            dst: 2,
            label: "knows".to_string(),
            weight: 0.5,
            metadata: "null".to_string(),
        })
        .unwrap();
        wal.append(&WalEntry::Delete::<f32> { id: 3 }).unwrap();
        wal.append(&WalEntry::Unlink::<f32> { src: 1, dst: 2 })
            .unwrap();
        wal.append(&WalEntry::UpdatePayload::<f32> {
            id: 1,
            payload: r#"{"updated":true}"#.to_string(),
        })
        .unwrap();
        wal.append(&WalEntry::UpdateVector::<f32> {
            id: 1,
            vector: vec![9.0],
        })
        .unwrap();
        wal.append(&WalEntry::UnlinkLabel::<f32> {
            src: 1,
            dst: 2,
            label: "knows".to_string(),
        })
        .unwrap();
    }

    let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
    assert_eq!(entries.len(), 7);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  事务批量写入
// ════════════════════════════════════════════════════════════════

#[test]
fn wal_append_batch_事务完整性() {
    let path = tmp_db("batch");
    cleanup(&path);

    {
        let mut wal = Wal::open(&path).unwrap();
        let entries = vec![
            WalEntry::Insert::<f32> {
                id: 10,
                vector: vec![1.0],
                payload: "{}".to_string(),
            },
            WalEntry::Insert::<f32> {
                id: 11,
                vector: vec![2.0],
                payload: "{}".to_string(),
            },
        ];
        wal.append_batch(42, &entries).unwrap();
    }

    let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
    // 事务过滤后应包含 2 条 Insert（TxBegin/TxCommit 被过滤掉）
    assert_eq!(entries.len(), 2);

    cleanup(&path);
}

#[test]
fn wal_group_commit保留独立事务且仅同步一次() {
    let path = tmp_db("group_commit");
    cleanup(&path);

    let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
    wal.begin_group_commit().unwrap();
    for id in 1..=3u64 {
        wal.append_batch(
            id,
            &[WalEntry::Insert::<f32> {
                id,
                vector: vec![id as f32],
                payload: "{}".to_string(),
            }],
        )
        .unwrap();
    }
    assert_eq!(wal.stats().sync_count, 0);
    assert!(wal.finish_group_commit().unwrap());
    assert_eq!(wal.stats().sync_count, 1);
    drop(wal);

    let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
    assert_eq!(entries.len(), 3);
    cleanup(&path);
}

#[test]
fn wal_group_commit空组不执行同步且拒绝嵌套() {
    let path = tmp_db("group_commit_empty");
    cleanup(&path);
    let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
    wal.begin_group_commit().unwrap();
    assert!(wal.begin_group_commit().is_err());
    assert!(!wal.finish_group_commit().unwrap());
    assert_eq!(wal.stats().sync_count, 0);
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  WAL clear
// ════════════════════════════════════════════════════════════════

#[test]
fn wal_clear_后读取为空() {
    let path = tmp_db("clear");
    cleanup(&path);

    let mut wal = Wal::open(&path).unwrap();
    wal.append(&WalEntry::Insert::<f32> {
        id: 1,
        vector: vec![1.0],
        payload: "{}".to_string(),
    })
    .unwrap();
    wal.clear().unwrap();

    let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
    assert!(entries.is_empty(), "clear 后应无条目");

    // clear 后还能继续追加
    wal.append(&WalEntry::Insert::<f32> {
        id: 2,
        vector: vec![2.0],
        payload: "{}".to_string(),
    })
    .unwrap();
    drop(wal);

    let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
    assert_eq!(entries.len(), 1);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  SyncMode 切换
// ════════════════════════════════════════════════════════════════

#[test]
fn wal_sync_mode_切换() {
    let path = tmp_db("sync_mode");
    cleanup(&path);

    let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
    wal.append(&WalEntry::Insert::<f32> {
        id: 1,
        vector: vec![1.0],
        payload: "{}".to_string(),
    })
    .unwrap();

    wal.set_sync_mode(SyncMode::Off);
    wal.append(&WalEntry::Insert::<f32> {
        id: 2,
        vector: vec![2.0],
        payload: "{}".to_string(),
    })
    .unwrap();
    wal.flush_writer();
    drop(wal);

    let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
    assert_eq!(entries.len(), 2);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  needs_recovery
// ════════════════════════════════════════════════════════════════

#[test]
fn wal_needs_recovery_空文件() {
    let path = tmp_db("needs_rec_empty");
    cleanup(&path);

    // 无 WAL 文件
    assert!(!Wal::needs_recovery(&path));

    // 创建空 WAL
    let wal = Wal::open(&path).unwrap();
    drop(wal);
    assert!(!Wal::needs_recovery(&path), "空 WAL 不需要恢复");

    cleanup(&path);
}

#[test]
fn wal_needs_recovery_非空文件() {
    let path = tmp_db("needs_rec_data");
    cleanup(&path);

    let mut wal = Wal::open(&path).unwrap();
    wal.append(&WalEntry::Insert::<f32> {
        id: 1,
        vector: vec![1.0],
        payload: "{}".to_string(),
    })
    .unwrap();
    drop(wal);

    assert!(Wal::needs_recovery(&path), "非空 WAL 应该需要恢复");

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  read_entries_from_reader — CRC 校验
// ════════════════════════════════════════════════════════════════

#[test]
fn wal明确拒绝不支持的非空版本() {
    let path = tmp_db("unsupported_version");
    cleanup(&path);
    std::fs::write(
        format!("{}.wal", path),
        [
            b'T', b'V', b'W', b'L', 0xFF, 0x7F, 0x01, 0x00, 0x00, 0x00, 0xAA,
        ],
    )
    .unwrap();

    let result = Wal::read_entries::<f32>(&path);
    assert!(
        matches!(
            result,
            Err(triviumdb::TriviumError::UnsupportedWalVersion { .. })
        ),
        "未知 WAL 版本必须明确拒绝"
    );

    cleanup(&path);
}

#[test]
fn wal空旧版本头只能显式升级() {
    let path = tmp_db("legacy_empty_header");
    cleanup(&path);
    std::fs::write(format!("{}.wal", path), [b'T', b'V', b'W', b'L', 2, 0]).unwrap();

    assert!(matches!(
        Wal::read_entries::<f32>(&path),
        Err(triviumdb::TriviumError::UnsupportedWalVersion { found: 2, .. })
    ));
    assert!(Wal::upgrade_empty_legacy_wal(&path).unwrap());
    assert!(!Wal::upgrade_empty_legacy_wal(&path).unwrap());

    let bytes = std::fs::read(format!("{}.wal", path)).unwrap();
    assert_eq!(&bytes[0..4], b"TVWL");
    assert_eq!(
        u16::from_le_bytes([bytes[4], bytes[5]]),
        triviumdb::storage::wal::WAL_VERSION
    );
    assert!(Wal::read_entries::<f32>(&path).unwrap().0.is_empty());
    cleanup(&path);
}

#[test]
fn wal非空v2可安全迁移并补齐link元数据() {
    #[derive(serde::Serialize)]
    enum WalEntryV2<T> {
        TxBegin {
            tx_id: u64,
        },
        TxCommit {
            tx_id: u64,
        },
        Insert {
            id: u64,
            vector: Vec<T>,
            payload: String,
        },
        Link {
            src: u64,
            dst: u64,
            label: String,
            weight: f32,
        },
    }

    let path = tmp_db("legacy_nonempty_v2");
    cleanup(&path);
    let entries = [
        WalEntryV2::TxBegin::<f32> { tx_id: 7 },
        WalEntryV2::Insert {
            id: 1,
            vector: vec![1.0, 0.0],
            payload: "{}".into(),
        },
        WalEntryV2::Insert {
            id: 2,
            vector: vec![0.0, 1.0],
            payload: "{}".into(),
        },
        WalEntryV2::Link {
            src: 1,
            dst: 2,
            label: "old".into(),
            weight: 0.5,
        },
        WalEntryV2::TxCommit { tx_id: 7 },
    ];
    let mut bytes = vec![b'T', b'V', b'W', b'L', 2, 0];
    for entry in entries {
        let data = bincode::serialize(&entry).unwrap();
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes.extend_from_slice(&crc32fast::hash(&data).to_le_bytes());
    }
    std::fs::write(format!("{}.wal", path), bytes).unwrap();

    assert!(Wal::upgrade_legacy_wal::<f32>(&path).unwrap());
    let migrated = std::fs::read(format!("{}.wal", path)).unwrap();
    assert_eq!(u16::from_le_bytes([migrated[4], migrated[5]]), 3);
    let (entries, _) = Wal::read_entries::<f32>(&path).unwrap();
    assert_eq!(entries.len(), 3);
    assert!(matches!(
        &entries[2],
        WalEntry::Link { metadata, .. } if metadata == "null"
    ));
    cleanup(&path);
}

#[test]
fn wal_v2全部历史变体迁移后保持事务过滤和字段语义() {
    #[derive(serde::Serialize)]
    enum V2<T> {
        TxBegin {
            tx_id: u64,
        },
        TxCommit {
            tx_id: u64,
        },
        Insert {
            id: u64,
            vector: Vec<T>,
            payload: String,
        },
        Link {
            src: u64,
            dst: u64,
            label: String,
            weight: f32,
        },
        Delete {
            id: u64,
        },
        Unlink {
            src: u64,
            dst: u64,
        },
        UpdatePayload {
            id: u64,
            payload: String,
        },
        UpdateVector {
            id: u64,
            vector: Vec<T>,
        },
        UnlinkLabel {
            src: u64,
            dst: u64,
            label: String,
        },
    }
    let committed = vec![
        V2::TxBegin::<f32> { tx_id: 1 },
        V2::Insert {
            id: 1,
            vector: vec![1.0, 0.0],
            payload: "{\"v\":1}".into(),
        },
        V2::Link {
            src: 1,
            dst: 2,
            label: "x".into(),
            weight: 0.5,
        },
        V2::Delete { id: 3 },
        V2::Unlink { src: 1, dst: 2 },
        V2::UpdatePayload {
            id: 1,
            payload: "{\"v\":2}".into(),
        },
        V2::UpdateVector {
            id: 1,
            vector: vec![0.0, 1.0],
        },
        V2::UnlinkLabel {
            src: 1,
            dst: 2,
            label: "x".into(),
        },
        V2::TxCommit { tx_id: 1 },
        V2::TxBegin { tx_id: 2 },
        V2::Delete { id: 99 },
    ];
    let path = tmp_db("legacy_v2_all_variants");
    cleanup(&path);
    let mut bytes = vec![b'T', b'V', b'W', b'L', 2, 0];
    for entry in committed {
        let data = bincode::serialize(&entry).unwrap();
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes.extend_from_slice(&crc32fast::hash(&data).to_le_bytes());
    }
    std::fs::write(format!("{path}.wal"), bytes).unwrap();
    assert!(Wal::upgrade_legacy_wal::<f32>(&path).unwrap());
    let (entries, valid_offset) = Wal::read_entries::<f32>(&path).unwrap();
    assert_eq!(entries.len(), 7, "未提交事务中的 Delete 必须被过滤");
    assert!(valid_offset > 6);
    assert!(
        matches!(&entries[0], WalEntry::Insert { id: 1, payload, .. } if payload == "{\"v\":1}")
    );
    assert!(
        matches!(&entries[1], WalEntry::Link { metadata, weight, .. } if metadata == "null" && *weight == 0.5)
    );
    assert!(matches!(&entries[2], WalEntry::Delete { id: 3 }));
    assert!(matches!(&entries[3], WalEntry::Unlink { src: 1, dst: 2 }));
    assert!(
        matches!(&entries[4], WalEntry::UpdatePayload { id: 1, payload } if payload == "{\"v\":2}")
    );
    assert!(
        matches!(&entries[5], WalEntry::UpdateVector { id: 1, vector } if vector == &vec![0.0, 1.0])
    );
    assert!(matches!(&entries[6], WalEntry::UnlinkLabel { src: 1, dst: 2, label } if label == "x"));
    cleanup(&path);
}

#[test]
fn wal损坏v2迁移失败且原文件不变() {
    let path = tmp_db("legacy_v2_corrupt");
    cleanup(&path);
    let entry = WalEntry::Delete::<f32> { id: 9 };
    let data = bincode::serialize(&entry).unwrap();
    let mut bytes = vec![b'T', b'V', b'W', b'L', 2, 0];
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(&0xDEADBEEFu32.to_le_bytes());
    std::fs::write(format!("{}.wal", path), &bytes).unwrap();

    assert!(Wal::upgrade_legacy_wal::<f32>(&path).is_err());
    assert_eq!(std::fs::read(format!("{}.wal", path)).unwrap(), bytes);
    assert!(!std::path::Path::new(&format!("{}.wal.upgrade.tmp", path)).exists());
    cleanup(&path);
}

#[test]
fn wal非空旧版本不能升级且字节不变() {
    let path = tmp_db("legacy_nonempty_header");
    cleanup(&path);
    let bytes = [b'T', b'V', b'W', b'L', 2, 0, 1, 2, 3, 4];
    std::fs::write(format!("{}.wal", path), bytes).unwrap();

    assert!(!Wal::upgrade_empty_legacy_wal(&path).unwrap());
    assert_eq!(std::fs::read(format!("{}.wal", path)).unwrap(), bytes);
    assert!(matches!(
        Wal::read_entries::<f32>(&path),
        Err(triviumdb::TriviumError::UnsupportedWalVersion { found: 2, .. })
    ));
    cleanup(&path);
}

#[test]
fn wal拒绝历史无头记录并提示迁移() {
    let path = tmp_db("legacy_headerless");
    cleanup(&path);
    let entry = WalEntry::Delete::<f32> { id: 9 };
    let data = bincode::serialize(&entry).unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(&crc32fast::hash(&data).to_le_bytes());
    std::fs::write(format!("{}.wal", path), bytes).unwrap();

    assert!(matches!(
        Wal::read_entries::<f32>(&path),
        Err(triviumdb::TriviumError::UnsupportedWalVersion { found: 0, .. })
    ));

    cleanup(&path);
}

#[test]
fn wal_crc_错误时停止恢复() {
    // 构造一条有效记录
    let entry = WalEntry::Insert::<f32> {
        id: 1,
        vector: vec![1.0, 2.0],
        payload: "{}".to_string(),
    };
    let data = bincode::serialize(&entry).unwrap();
    let crc = crc32fast::hash(&data);

    let mut buf = Vec::new();
    // 正确的第一条
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&data);
    buf.extend_from_slice(&crc.to_le_bytes());

    // 错误 CRC 的第二条
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&data);
    buf.extend_from_slice(&0xDEADBEEFu32.to_le_bytes()); // 坏 CRC

    let (entries, _) = Wal::read_entries_from_reader::<f32>(Cursor::new(&buf)).unwrap();
    assert_eq!(entries.len(), 1, "CRC 错误后应只恢复第一条");
}

#[test]
fn wal_len过大_合理性检查() {
    let mut buf = Vec::new();
    // 写一个超过 256MB 的 len 值
    buf.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());
    buf.extend_from_slice(&[0xAA; 100]);

    let (entries, _) = Wal::read_entries_from_reader::<f32>(Cursor::new(&buf)).unwrap();
    assert!(entries.is_empty(), "超大 len 应触发安全停止");
}
