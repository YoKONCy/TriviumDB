use super::fixture::*;
use super::mutation::{BoundaryValue, Mutation, field_boundary_mutations, truncation_mutations};
use super::spec::WAL_HEADER;
use triviumdb::database::Database;
use triviumdb::storage::wal::{SyncMode, Wal, WalEntry};

fn wal_fixture(name: &str) -> (String, Vec<u8>) {
    let path = path(name);
    cleanup(&path);
    let mut wal = Wal::open_with_sync(&path, SyncMode::Full).unwrap();
    wal.append_batch(
        7,
        &[
            WalEntry::Insert {
                id: 1,
                vector: vec![1.0f32, 0.0, 0.0, 0.0],
                payload: r#"{"kind":"a"}"#.into(),
            },
            WalEntry::UpdatePayload {
                id: 1,
                payload: r#"{"kind":"b"}"#.into(),
            },
        ],
    )
    .unwrap();
    drop(wal);
    let bytes = std::fs::read(format!("{path}.wal")).unwrap();
    (path, bytes)
}

fn frame_offsets(bytes: &[u8]) -> Vec<(usize, usize, usize)> {
    let mut output = Vec::new();
    let mut offset = 6;
    while offset + 8 <= bytes.len() {
        let length = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let payload = offset + 4;
        let crc = payload.saturating_add(length);
        if crc + 4 > bytes.len() {
            break;
        }
        output.push((offset, payload, crc));
        offset = crc + 4;
    }
    output
}

#[test]
fn wal头部字段边界与所有帧边界逐字节截断均安全停止() {
    let (source, bytes) = wal_fixture("wal_fields");
    let mut mutations = field_boundary_mutations(WAL_HEADER);
    mutations.extend(truncation_mutations(bytes.len(), WAL_HEADER));
    for (frame, payload, crc) in frame_offsets(&bytes) {
        mutations.extend([
            Mutation::TruncateAt(frame),
            Mutation::TruncateAt(frame + 1),
            Mutation::TruncateAt(payload),
            Mutation::TruncateAt(payload + 1),
            Mutation::TruncateAt(crc),
            Mutation::TruncateAt(crc + 1),
            Mutation::FlipBit {
                offset: payload,
                bit: 0,
            },
            Mutation::FlipBit {
                offset: crc,
                bit: 7,
            },
        ]);
    }
    for (index, mutation) in mutations.into_iter().enumerate() {
        let candidate = path(&format!("wal_field_case_{index}"));
        cleanup(&candidate);
        std::fs::write(format!("{candidate}.wal"), mutation.apply(&bytes)).unwrap();
        let _ = Wal::read_entries::<f32>(&candidate);
        cleanup(&candidate);
    }
    cleanup(&source);
}

#[test]
fn wal长度零_最大值_超出剩余和尾部垃圾不越过损坏边界() {
    let (source, bytes) = wal_fixture("wal_lengths");
    let first_length = super::spec::FieldSpec {
        name: "frame_len",
        offset: 6,
        width: 4,
        encoding: super::spec::FieldEncoding::U32Le,
    };
    for (value, corrupt) in [
        (BoundaryValue::Zero, true),
        (BoundaryValue::One, true),
        (BoundaryValue::Max, true),
        // 16 MiB 合法长度遇到物理 EOF，属于尚未写完的尾帧。
        (BoundaryValue::BigEndianOne, false),
    ] {
        let candidate = path(&format!("wal_length_{value:?}"));
        cleanup(&candidate);
        let changed = Mutation::SetField {
            field: first_length,
            value,
        }
        .apply(&bytes);
        std::fs::write(format!("{candidate}.wal"), &changed).unwrap();
        let recovered = Wal::read_entries::<f32>(&candidate);
        if corrupt {
            assert!(
                matches!(recovered, Err(triviumdb::TriviumError::CorruptedFile(_))),
                "{value:?}"
            );
        } else {
            let (entries, offset) = recovered.unwrap();
            assert!(entries.is_empty());
            assert_eq!(offset, 6);
        }
        assert_eq!(std::fs::read(format!("{candidate}.wal")).unwrap(), changed);
        cleanup(&candidate);
    }
    let candidate = path("wal_tail_garbage");
    cleanup(&candidate);
    std::fs::write(
        format!("{candidate}.wal"),
        Mutation::Append(vec![0xa5; 31]).apply(&bytes),
    )
    .unwrap();
    assert!(matches!(
        Wal::read_entries::<f32>(&candidate),
        Err(triviumdb::TriviumError::CorruptedFile(_))
    ));
    cleanup(&candidate);
    cleanup(&source);
}

#[test]
fn 未提交事务_缺commit与commit后残帧均保持原子恢复() {
    let source = path("wal_transaction_source");
    cleanup(&source);
    Database::<f32>::open(&source, DIM)
        .unwrap()
        .close()
        .unwrap();
    let mut wal = Wal::open_with_sync(&source, SyncMode::Full).unwrap();
    wal.append_batch(
        1,
        &[WalEntry::Insert {
            id: 1,
            vector: vec![1.0f32; DIM],
            payload: "{}".into(),
        }],
    )
    .unwrap();
    wal.append_batch(
        2,
        &[WalEntry::Insert {
            id: 2,
            vector: vec![2.0f32; DIM],
            payload: "{}".into(),
        }],
    )
    .unwrap();
    drop(wal);
    let bytes = std::fs::read(format!("{source}.wal")).unwrap();
    let frames = frame_offsets(&bytes);
    let second_commit_start = frames.last().unwrap().0;
    std::fs::write(format!("{source}.wal"), &bytes[..second_commit_start]).unwrap();
    let database = Database::<f32>::open(&source, DIM).unwrap();
    assert!(database.get_payload(1).unwrap().is_some());
    assert!(database.get_payload(2).unwrap().is_none());
    drop(database);
    cleanup(&source);
}

#[test]
fn wal未知版本只读打开零写拒绝() {
    let source = seed("wal_version_source");
    let mut wal = std::fs::read(format!("{source}.wal")).unwrap();
    wal[4..6].copy_from_slice(&u16::MAX.to_le_bytes());
    std::fs::write(format!("{source}.wal"), wal).unwrap();
    let before = directory_snapshot(&source);
    assert!(Database::<f32>::open_read_only(&source, DIM).is_err());
    assert_eq!(directory_snapshot(&source), before);
    cleanup(&source);
}
