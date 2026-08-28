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
fn wal明确拒绝不支持的版本() {
    let path = tmp_db("unsupported_version");
    cleanup(&path);
    // 头部（版本 0x7FFF）+ 一段条目负载，确保这是一个「非空」WAL：
    // 非空意味着存在待回放数据，任何版本不兼容都必须拒绝，不能放行。
    // 空 WAL 的向后兼容处理见 wal_空WAL_版本不兼容_应放行。
    std::fs::write(
        format!("{}.wal", path),
        [
            b'T', b'V', b'W', b'L', 0xFF, 0x7F, 0x10, 0x00, 0x00, 0x00, 0xAA, 0xBB, 0xCC, 0xDD,
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
fn wal兼容历史无头记录() {
    let path = tmp_db("legacy_headerless");
    cleanup(&path);
    let entry = WalEntry::Delete::<f32> { id: 9 };
    let data = bincode::serialize(&entry).unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(&crc32fast::hash(&data).to_le_bytes());
    std::fs::write(format!("{}.wal", path), bytes).unwrap();

    let (entries, offset) = Wal::read_entries::<f32>(&path).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(offset > 0);

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

// ════════════════════════════════════════════════════════════════
//  WAL 版本向后兼容
// ════════════════════════════════════════════════════════════════

/// 构造一个指定版本的 WAL 文件（可选附加伪造条目），写到 `<path>.wal`
fn write_wal_with_version(path: &str, version: u16, extra: &[u8]) {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"TVWL");
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(extra);
    std::fs::write(format!("{}.wal", path), &bytes).unwrap();
}

/// 空 WAL（只有头部、零条目）版本不兼容时应放行 —— 没有待回放数据，
/// 放行不会有数据完整性风险，却能让旧版本创建的库正常升级。
#[test]
fn wal_空WAL_版本不兼容_应放行() {
    let path = tmp_db("legacy_empty_wal");
    cleanup(&path);
    write_wal_with_version(&path, 2, &[]);

    let (entries, offset) = Wal::read_entries::<f32>(&path).unwrap();
    assert!(entries.is_empty(), "空 WAL 应解析出 0 条记录");
    assert_eq!(offset, 6, "空 WAL 应只消费头部 6 字节");

    cleanup(&path);
}

/// WAL 里还有条目时，版本不兼容必须继续拒绝 —— 不能用新格式回放旧格式条目。
#[test]
fn wal_非空WAL_版本不兼容_应拒绝() {
    let path = tmp_db("legacy_nonempty_wal");
    cleanup(&path);
    // 伪造 32 字节条目负载，使 WAL 长度超过头部
    write_wal_with_version(&path, 2, &[0xAA; 32]);

    let err = Wal::read_entries::<f32>(&path).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("WAL 版本") || msg.contains("WAL version"),
        "应当报 WAL 版本不兼容，实际错误：{msg}"
    );

    cleanup(&path);
}
