#![allow(non_snake_case)]

use serde_json::json;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use triviumdb::TriviumError;
use triviumdb::database::{Config, Database, StorageMode};
use triviumdb::storage::file_format::{CURRENT_VERSION, MINIMUM_SUPPORTED_VERSION};

const V5_LEGACY_CHUNKS: usize = 32;
const V5_CURRENT_CHUNKS: usize = 48;

fn tmp_db(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "triviumdb_format_migration_{name}_{}",
        std::process::id()
    ))
}

fn version(path: &PathBuf) -> u16 {
    let bytes = std::fs::read(path).unwrap();
    u16::from_le_bytes([bytes[4], bytes[5]])
}

fn cleanup(path: &Path) {
    for suffix in [
        "",
        ".vec",
        ".pld",
        ".pld.tmp",
        ".wal",
        ".lock",
        ".flush_ok",
        ".quiver",
        ".quiver.meta",
        ".text",
        ".text.meta",
        ".pidx",
        ".gidx",
        ".manifest.json",
    ] {
        std::fs::remove_file(format!("{}{suffix}", path.display())).ok();
    }
}

fn seed_v6(path: &Path, mode: StorageMode, dim: usize) {
    cleanup(path);
    let mut db = Database::<f32>::open_with_config(
        path.to_str().unwrap(),
        Config {
            dim,
            storage_mode: mode,
            ..Default::default()
        },
    )
    .unwrap();
    let first = db
        .insert(
            &(0..dim)
                .map(|index| if index % 2 == 0 { 1.0 } else { -1.0 })
                .collect::<Vec<_>>(),
            json!({"name": "first"}),
        )
        .unwrap();
    let second = db
        .insert(&vec![0.5; dim], json!({"name": "second"}))
        .unwrap();
    db.link(first, second, "related", 0.75).unwrap();
    db.flush().unwrap();
}

fn current_payload_path(path: &Path) -> Option<PathBuf> {
    let marker = std::fs::read(format!("{}.flush_ok", path.display())).ok()?;
    let generation = u64::from_le_bytes(marker.get(5..13)?.try_into().ok()?);
    Some(PathBuf::from(format!(
        "{}.pld.{generation}",
        path.display()
    )))
}

fn mapped_payloads(path: &Path) -> std::collections::HashMap<u64, Vec<u8>> {
    let bytes = std::fs::read(current_payload_path(path).unwrap()).unwrap();
    let count = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) as usize;
    let mut payloads = std::collections::HashMap::new();
    for index in 0..count {
        let cursor = 40 + index * 32;
        let id = u64::from_le_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
        let offset =
            u64::from_le_bytes(bytes[cursor + 8..cursor + 16].try_into().unwrap()) as usize;
        let length =
            u32::from_le_bytes(bytes[cursor + 16..cursor + 20].try_into().unwrap()) as usize;
        payloads.insert(id, bytes[offset..offset + length].to_vec());
    }
    payloads
}

fn rewrite_v6_as_v5(path: &Path, chunks: usize) {
    let bytes = std::fs::read(path).unwrap();
    let node_count = u64::from_le_bytes(bytes[18..26].try_into().unwrap()) as usize;
    let payload_offset = u64::from_le_bytes(bytes[26..34].try_into().unwrap()) as usize;
    let edge_offset = u64::from_le_bytes(bytes[42..50].try_into().unwrap()) as usize;
    let bq_offset = u64::from_le_bytes(bytes[50..58].try_into().unwrap()) as usize;
    assert_eq!(&bytes[bq_offset..bq_offset + 4], b"TBQF");
    let count = u64::from_le_bytes(bytes[bq_offset + 8..bq_offset + 16].try_into().unwrap());

    let sidecar_path = current_payload_path(path);
    let mut rewritten = bytes[..payload_offset].to_vec();
    if sidecar_path.as_ref().is_some_and(|path| path.exists()) {
        let payloads = mapped_payloads(path);
        for index in 0..node_count {
            let slot = payload_offset + index * 12;
            let id = u64::from_le_bytes(bytes[slot..slot + 8].try_into().unwrap());
            rewritten.extend_from_slice(&id.to_le_bytes());
            if id == 0 {
                rewritten.extend_from_slice(&0u32.to_le_bytes());
            } else {
                let payload = payloads.get(&id).unwrap();
                rewritten.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                rewritten.extend_from_slice(payload);
            }
        }
    } else {
        // Rom 当前格式仍内嵌 raw Payload，可直接保留旧式变长记录。
        rewritten.extend_from_slice(&bytes[payload_offset..edge_offset]);
    }
    let legacy_edge_offset = rewritten.len() as u64;
    let mut cursor = edge_offset;
    while cursor < bq_offset {
        let record_start = cursor;
        cursor += 16;
        let label_len = u16::from_le_bytes(bytes[cursor..cursor + 2].try_into().unwrap()) as usize;
        cursor += 2 + label_len + 4;
        rewritten.extend_from_slice(&bytes[record_start..cursor]);
        let metadata_len =
            u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4 + metadata_len;
    }

    let legacy_bq_offset = rewritten.len() as u64;
    rewritten[4..6].copy_from_slice(&5u16.to_le_bytes());
    rewritten[42..50].copy_from_slice(&legacy_edge_offset.to_le_bytes());
    rewritten[50..58].copy_from_slice(&legacy_bq_offset.to_le_bytes());
    rewritten.extend_from_slice(&count.to_le_bytes());
    for index in 0..count as usize {
        let source = bq_offset + 16 + index * V5_CURRENT_CHUNKS * 8;
        rewritten.extend_from_slice(&bytes[source..source + chunks * 8]);
    }
    std::fs::write(path, rewritten).unwrap();
    refresh_flush_marker(path);
}

fn refresh_flush_marker(path: &Path) {
    let marker_path = PathBuf::from(format!("{}.flush_ok", path.display()));
    if !marker_path.exists() {
        return;
    }
    let mut marker = std::fs::read(&marker_path).unwrap();
    marker[13..21].copy_from_slice(&std::fs::metadata(path).unwrap().len().to_le_bytes());
    let tdb_crc = crc32fast::hash(&std::fs::read(path).unwrap());
    let (tdb_crc_offset, marker_crc_offset) = match marker.len() {
        41 => (29, 37),
        53 => (37, 49),
        _ => panic!("未知 flush marker 长度"),
    };
    marker[tdb_crc_offset..tdb_crc_offset + 4].copy_from_slice(&tdb_crc.to_le_bytes());
    let marker_crc = crc32fast::hash(&marker[..marker_crc_offset]);
    marker[marker_crc_offset..marker_crc_offset + 4].copy_from_slice(&marker_crc.to_le_bytes());
    std::fs::write(marker_path, marker).unwrap();
}

#[test]
fn v5_三十二chunk从零点七零可读并在flush后升级v6() {
    let path = tmp_db("v5_32");
    seed_v6(&path, StorageMode::Mmap, 128);
    rewrite_v6_as_v5(&path, V5_LEGACY_CHUNKS);
    assert_eq!(version(&path), 5);
    let mut db = Database::<f32>::open(path.to_str().unwrap(), 128).unwrap();
    assert_eq!(db.node_count(), 2);
    assert_eq!(db.get_edges(1).len(), 1);
    assert_eq!(db.get_payload(2).unwrap()["name"], "second");
    db.flush().unwrap();
    assert_eq!(version(&path), CURRENT_VERSION);
    drop(db);
    let reopened = Database::<f32>::open(path.to_str().unwrap(), 128).unwrap();
    assert_eq!(reopened.node_count(), 2);
}

#[test]
fn v5_四十八chunk从零点七一后可读并在close后升级v6() {
    let path = tmp_db("v5_48");
    seed_v6(&path, StorageMode::Rom, 384);
    rewrite_v6_as_v5(&path, V5_CURRENT_CHUNKS);
    let mut db = Database::<f32>::open_with_config(
        path.to_str().unwrap(),
        Config {
            dim: 384,
            storage_mode: StorageMode::Rom,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(db.node_count(), 2);
    db.close().unwrap();
    assert_eq!(version(&path), CURRENT_VERSION);
}

#[test]
fn v6_BQ块使用显式自描述布局() {
    let path = tmp_db("v6_layout");
    seed_v6(&path, StorageMode::Rom, 64);
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), CURRENT_VERSION);
    let offset = u64::from_le_bytes(bytes[50..58].try_into().unwrap()) as usize;
    assert_eq!(&bytes[offset..offset + 4], b"TBQF");
    assert_eq!(
        u16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]),
        1
    );
    assert_eq!(
        u16::from_le_bytes([bytes[offset + 6], bytes[offset + 7]]) as usize,
        V5_CURRENT_CHUNKS
    );
    assert_eq!(
        u64::from_le_bytes(bytes[offset + 8..offset + 16].try_into().unwrap()),
        2
    );
}

#[test]
fn 早于零点七零与未来版本均明确拒绝且文件不变() {
    for candidate in [MINIMUM_SUPPORTED_VERSION - 1, CURRENT_VERSION + 1] {
        let path = tmp_db(&format!("unsupported_{candidate}"));
        seed_v6(&path, StorageMode::Rom, 4);
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.seek(SeekFrom::Start(4)).unwrap();
        file.write_all(&candidate.to_le_bytes()).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            Database::<f32>::open(path.to_str().unwrap(), 4),
            Err(TriviumError::UnsupportedDatabaseVersion { found, .. }) if found == candidate
        ));
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}

#[test]
fn v5_未知签名布局和v6损坏格式头均安全拒绝() {
    let legacy = tmp_db("invalid_v5");
    seed_v6(&legacy, StorageMode::Rom, 64);
    rewrite_v6_as_v5(&legacy, V5_CURRENT_CHUNKS);
    let mut bytes = std::fs::read(&legacy).unwrap();
    bytes.push(0);
    std::fs::write(&legacy, bytes).unwrap();
    assert!(matches!(
        Database::<f32>::open(legacy.to_str().unwrap(), 64),
        Err(TriviumError::CorruptedFile(_))
    ));

    let current = tmp_db("invalid_v6");
    seed_v6(&current, StorageMode::Rom, 64);
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&current)
        .unwrap();
    let mut header = [0u8; 58];
    file.read_exact(&mut header).unwrap();
    let offset = u64::from_le_bytes(header[50..58].try_into().unwrap());
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(b"FAIL").unwrap();
    file.sync_all().unwrap();
    assert!(matches!(
        Database::<f32>::open(current.to_str().unwrap(), 64),
        Err(TriviumError::CorruptedFile(_))
    ));
}

#[test]
fn v5_readonly可读但不会原地升级或改写() {
    let path = tmp_db("v5_read_only");
    seed_v6(&path, StorageMode::Mmap, 128);
    rewrite_v6_as_v5(&path, V5_LEGACY_CHUNKS);
    let before_tdb = std::fs::read(&path).unwrap();
    let before_vec = std::fs::read(format!("{}.vec", path.display())).unwrap();
    let before_marker = std::fs::read(format!("{}.flush_ok", path.display())).unwrap();
    let reader = Database::<f32>::open_read_only(path.to_str().unwrap(), 128).unwrap();
    assert_eq!(reader.node_count(), 2);
    drop(reader);
    assert_eq!(std::fs::read(&path).unwrap(), before_tdb);
    assert_eq!(
        std::fs::read(format!("{}.vec", path.display())).unwrap(),
        before_vec
    );
    assert_eq!(
        std::fs::read(format!("{}.flush_ok", path.display())).unwrap(),
        before_marker
    );
}
