//! 四类属性索引 CRUD、统计、持久化与重启一致性测试。
//! 索引命中必须与权威 Payload 扫描结果等价，删除和 slot 复用不得产生幽灵结果。

use serde_json::json;
use triviumdb::database::{AccessMode, Config, MissingIndexPolicy};
use triviumdb::{Database, TriviumError};

const DIM: usize = 4;
const SUFFIXES: &[&str] = &[
    "",
    ".vec",
    ".wal",
    ".lock",
    ".flush_ok",
    ".pidx",
    ".pidx.tmp",
    ".gidx",
    ".gidx.tmp",
    ".manifest.json",
];

fn database_path(name: &str) -> String {
    let directory = std::env::temp_dir().join("triviumdb_property_index_tests");
    std::fs::create_dir_all(&directory).unwrap();
    directory
        .join(format!("{name}_{}", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

fn cleanup(path: &str) {
    for suffix in SUFFIXES {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

fn seed_database(path: &str) {
    let mut db = Database::<f32>::open(path, DIM).unwrap();
    db.insert(&[1.0, 0.0, 0.0, 0.0], json!({"kind": "person", "rank": 1}))
        .unwrap();
    db.insert(&[0.0, 1.0, 0.0, 0.0], json!({"kind": "person", "rank": 2}))
        .unwrap();
    db.insert(&[0.0, 0.0, 1.0, 0.0], json!({"kind": "event", "rank": 3}))
        .unwrap();
    db.create_index("kind").unwrap();
    db.create_index("rank").unwrap();
    db.close().unwrap();
}

#[test]
fn 复合与_bitmap_索引_v4_重启更新删除无幽灵命中() {
    let path = database_path("v4_composite_bitmap");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    let first = db
        .insert(
            &[1.0, 0.0, 0.0, 0.0],
            json!({"tenant": "a", "kind": "person", "state": "active"}),
        )
        .unwrap();
    db.insert(
        &[0.0, 1.0, 0.0, 0.0],
        json!({"tenant": "a", "kind": "event", "state": "active"}),
    )
    .unwrap();
    db.create_composite_index(&["tenant".into(), "kind".into()])
        .unwrap();
    db.create_bitmap_index("state").unwrap();
    db.update_payload(
        first,
        json!({"tenant": "b", "kind": "person", "state": "deleted"}),
    )
    .unwrap();
    db.close().unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        db.tql_nodes(r#"FIND {tenant: "a", kind: "event"} RETURN *"#)
            .unwrap()
            .len(),
        1
    );
    assert!(
        db.tql_nodes(r#"FIND {tenant: "a", kind: "person"} RETURN *"#)
            .unwrap()
            .is_empty()
    );
    assert!(db.list_indexes().contains(&"state".to_owned()));
    drop(db);
    cleanup(&path);
}

#[test]
fn 属性索引重启后直接恢复并保持负命中语义() {
    let path = database_path("restart");
    cleanup(&path);
    seed_database(&path);

    assert!(std::path::Path::new(&format!("{path}.pidx")).exists());
    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(db.list_indexes(), vec!["kind", "rank"]);
    assert_eq!(
        db.tql_nodes(r#"FIND {kind: "person"} RETURN *"#)
            .unwrap()
            .len(),
        2
    );
    assert!(
        db.tql_nodes(r#"MATCH (a {kind: "missing"}) RETURN a"#)
            .unwrap()
            .is_empty()
    );

    drop(db);
    cleanup(&path);
}

#[test]
fn 属性索引在更新删除和槽位复用后无幽灵命中() {
    let path = database_path("crud");
    cleanup(&path);
    let mut db = Database::<f32>::open(&path, DIM).unwrap();
    db.create_index("state").unwrap();
    let first = db
        .insert(&[1.0, 0.0, 0.0, 0.0], json!({"state": "old"}))
        .unwrap();
    db.update_payload(first, json!({"state": "new"})).unwrap();
    assert!(
        db.tql_nodes(r#"FIND {state: "old"} RETURN *"#)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        db.tql_nodes(r#"FIND {state: "new"} RETURN *"#)
            .unwrap()
            .len(),
        1
    );
    db.delete(first).unwrap();
    db.insert(&[0.0, 1.0, 0.0, 0.0], json!({"state": "replacement"}))
        .unwrap();
    assert!(
        db.tql_nodes(r#"FIND {state: "new"} RETURN *"#)
            .unwrap()
            .is_empty()
    );
    db.close().unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert!(
        db.tql_nodes(r#"FIND {state: "new"} RETURN *"#)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        db.tql_nodes(r#"FIND {state: "replacement"} RETURN *"#)
            .unwrap()
            .len(),
        1
    );
    drop(db);
    cleanup(&path);
}

#[test]
fn 旧数据库没有属性索引_sidecar_仍可兼容打开() {
    let path = database_path("legacy_missing");
    cleanup(&path);
    seed_database(&path);
    std::fs::remove_file(format!("{path}.pidx")).unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert!(db.list_indexes().is_empty());
    assert_eq!(
        db.tql_nodes(r#"FIND {kind: "person"} RETURN *"#)
            .unwrap()
            .len(),
        2
    );
    drop(db);
    cleanup(&path);
}

#[test]
fn 损坏属性索引按策略回退或报错() {
    let path = database_path("corruption_policy");
    cleanup(&path);
    seed_database(&path);
    let sidecar = format!("{path}.pidx");
    let mut bytes = std::fs::read(&sidecar).unwrap();
    bytes[8] ^= 0x5a;
    std::fs::write(&sidecar, bytes).unwrap();

    let db = Database::<f32>::open(&path, DIM).unwrap();
    assert!(db.list_indexes().is_empty());
    assert_eq!(
        db.tql_nodes(r#"FIND {kind: "event"} RETURN *"#)
            .unwrap()
            .len(),
        1
    );
    drop(db);

    seed_database(&path);
    let mut bytes = std::fs::read(&sidecar).unwrap();
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&sidecar, bytes).unwrap();
    let error = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            missing_index_policy: MissingIndexPolicy::Error,
            ..Default::default()
        },
    )
    .err()
    .expect("Error 策略必须拒绝损坏 sidecar");
    assert!(
        matches!(error, TriviumError::CorruptedFile(_)),
        "实际错误: {error:?}"
    );
    cleanup(&path);
}

#[test]
fn 属性索引_v4_posting块_crc_独立拒绝损坏() {
    let path = database_path("posting_block_crc");
    cleanup(&path);
    seed_database(&path);
    let sidecar = format!("{path}.pidx");
    let mut bytes = std::fs::read(&sidecar).unwrap();
    assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 4);

    let field_len = u32::from_le_bytes(bytes[37..41].try_into().unwrap()) as usize;
    let entry_count_offset = 41 + field_len;
    let key_len_offset = entry_count_offset + 8;
    let key_len = u32::from_le_bytes(
        bytes[key_len_offset..key_len_offset + 4]
            .try_into()
            .unwrap(),
    ) as usize;
    let id_count_offset = key_len_offset + 4 + key_len;
    let block_len_offset = id_count_offset + 8;
    let block_len = u64::from_le_bytes(
        bytes[block_len_offset..block_len_offset + 8]
            .try_into()
            .unwrap(),
    ) as usize;
    let block_start = block_len_offset + 16;
    assert!(block_len >= 8);
    bytes[block_start] ^= 0x5a;
    let whole_crc = crc32fast::hash(&bytes[..bytes.len() - 4]);
    let end = bytes.len();
    bytes[end - 4..].copy_from_slice(&whole_crc.to_le_bytes());
    std::fs::write(&sidecar, bytes).unwrap();

    let error = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            access_mode: AccessMode::ReadOnly,
            missing_index_policy: MissingIndexPolicy::Error,
            ..Default::default()
        },
    )
    .err()
    .expect("posting block CRC 损坏必须拒绝");
    assert!(error.to_string().contains("posting block CRC32"));
    cleanup(&path);
}

#[test]
fn 属性索引逐字节截断均不触发_panic() {
    let path = database_path("truncation");
    cleanup(&path);
    seed_database(&path);
    let sidecar = format!("{path}.pidx");
    let original = std::fs::read(&sidecar).unwrap();

    for length in 0..original.len() {
        std::fs::write(&sidecar, &original[..length]).unwrap();
        let result = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: DIM,
                missing_index_policy: MissingIndexPolicy::Error,
                ..Default::default()
            },
        );
        assert!(result.is_err(), "截断到 {length} 字节时必须拒绝");
    }
    cleanup(&path);
}

#[test]
fn 只读与不可变模式加载属性索引且零副作用() {
    let path = database_path("access_modes");
    cleanup(&path);
    seed_database(&path);
    let mut writer = Database::<f32>::open(&path, DIM).unwrap();
    writer.flush().unwrap();
    writer
        .publish_generation_manifest("property-index-generation")
        .unwrap();
    drop(writer);

    let sidecar = format!("{path}.pidx");
    let before = std::fs::read(&sidecar).unwrap();
    let read_only = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            access_mode: AccessMode::ReadOnly,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(read_only.list_indexes(), vec!["kind", "rank"]);
    let memory = read_only.index_memory_stats();
    assert!(memory.mapped_bytes > (3 * DIM * std::mem::size_of::<f32>()) as u64);
    assert!(memory.posting_entries >= 6);
    assert_eq!(
        read_only
            .tql_nodes(r#"FIND {kind: "person"} RETURN *"#)
            .unwrap()
            .len(),
        2
    );
    drop(read_only);

    let immutable = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: DIM,
            access_mode: AccessMode::Immutable,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(immutable.list_indexes(), vec!["kind", "rank"]);
    drop(immutable);
    assert_eq!(std::fs::read(&sidecar).unwrap(), before);
    cleanup(&path);
}

#[test]
fn 属性索引与全扫描随机数据结果一致() {
    let indexed_path = database_path("differential_indexed");
    let scanned_path = database_path("differential_scanned");
    cleanup(&indexed_path);
    cleanup(&scanned_path);
    let mut indexed = Database::<f32>::open(&indexed_path, DIM).unwrap();
    let mut scanned = Database::<f32>::open(&scanned_path, DIM).unwrap();
    indexed.create_index("bucket").unwrap();

    for sequence in 0..2_000usize {
        let payload = json!({"bucket": format!("bucket_{}", sequence % 37), "sequence": sequence});
        let vector = [sequence as f32, 0.0, 0.0, 0.0];
        indexed.insert(&vector, payload.clone()).unwrap();
        scanned.insert(&vector, payload).unwrap();
    }
    for bucket in 0..50usize {
        let query = format!("FIND {{bucket: \"bucket_{bucket}\"}} RETURN *");
        let mut indexed_ids: Vec<_> = indexed
            .tql_nodes(&query)
            .unwrap()
            .into_iter()
            .map(|row| row["_"].id)
            .collect();
        let mut scanned_ids: Vec<_> = scanned
            .tql_nodes(&query)
            .unwrap()
            .into_iter()
            .map(|row| row["_"].id)
            .collect();
        indexed_ids.sort_unstable();
        scanned_ids.sort_unstable();
        assert_eq!(indexed_ids, scanned_ids, "bucket_{bucket} 结果不一致");
    }

    drop(indexed);
    drop(scanned);
    cleanup(&indexed_path);
    cleanup(&scanned_path);
}
