use serde_json::json;
use triviumdb::database::{AccessMode, Config, MissingIndexPolicy};
use triviumdb::{Database, TriviumError};

const DIM: usize = 4;

fn path(name: &str) -> String {
    std::env::temp_dir()
        .join(format!(
            "triviumdb_graph_blocks_{name}_{}",
            std::process::id()
        ))
        .to_string_lossy()
        .into_owned()
}

fn cleanup(path: &str) {
    for suffix in [
        "",
        ".vec",
        ".wal",
        ".lock",
        ".flush_ok",
        ".gidx",
        ".gidx.tmp",
        ".manifest.json",
    ] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

fn seed(path: &str) {
    let mut db = Database::<f32>::open(path, DIM).unwrap();
    for id in 1..=6 {
        db.insert_with_id(id, &[id as f32, 1.0, 0.0, 0.0], json!({"id": id}))
            .unwrap();
    }
    db.upsert_edge(1, 2, "road", 0.75, json!({"lane": 1}))
        .unwrap();
    db.upsert_edge(1, 3, "road", 1.0, json!({"lane": 2}))
        .unwrap();
    db.upsert_edge(4, 2, "rail", 0.5, json!({"line": "A"}))
        .unwrap();
    db.close().unwrap();
}

#[test]
fn 图块_readonly_mmap_与_rw_结果差分一致() {
    let path = path("readonly");
    cleanup(&path);
    seed(&path);
    assert!(std::path::Path::new(&format!("{path}.gidx")).exists());

    let rw = Database::<f32>::open(&path, DIM).unwrap();
    let expected = rw.get_edges(1);
    drop(rw);
    let ro = Database::<f32>::open_read_only(&path, DIM).unwrap();
    assert_eq!(ro.get_edges(1), expected);
    assert_eq!(ro.get_incoming_edges(2, None).len(), 2);
    assert!(ro.index_memory_stats().mapped_bytes > (6 * DIM * 4) as u64);
    drop(ro);
    cleanup(&path);
}

#[test]
fn 图块_crc_与逐字节截断全部_fail_closed() {
    let path = path("corruption");
    cleanup(&path);
    seed(&path);
    let sidecar = format!("{path}.gidx");
    let original = std::fs::read(&sidecar).unwrap();

    let mut corrupted = original.clone();
    let block_start = 24 + 24;
    corrupted[block_start] ^= 0x5a;
    let whole_crc = crc32fast::hash(&corrupted[..corrupted.len() - 4]);
    let end = corrupted.len();
    corrupted[end - 4..].copy_from_slice(&whole_crc.to_le_bytes());
    std::fs::write(&sidecar, corrupted).unwrap();
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
    .expect("图块 CRC 损坏必须拒绝");
    assert!(matches!(error, TriviumError::CorruptedFile(_)));

    for length in 0..original.len() {
        std::fs::write(&sidecar, &original[..length]).unwrap();
        assert!(
            Database::<f32>::open_read_only(&path, DIM).is_err(),
            "截断到 {length} 字节必须拒绝"
        );
    }
    cleanup(&path);
}

#[test]
fn 图块_v2_目录独立_crc_与超大计数_fail_closed() {
    let path = path("directory_corruption");
    cleanup(&path);
    seed(&path);
    let sidecar = format!("{path}.gidx");
    let original = std::fs::read(&sidecar).unwrap();

    let block_count = u64::from_le_bytes(original[16..24].try_into().unwrap()) as usize;
    let mut cursor = 24usize;
    for _ in 0..block_count {
        let block_len =
            u32::from_le_bytes(original[cursor + 12..cursor + 16].try_into().unwrap()) as usize;
        cursor += 24 + block_len;
    }
    let directory_len =
        u64::from_le_bytes(original[cursor..cursor + 8].try_into().unwrap()) as usize;
    let directory_start = cursor + 16;
    assert!(directory_len > 8);

    let mut damaged = original.clone();
    damaged[directory_start + 8] ^= 0x5a;
    let end = damaged.len();
    let whole_crc = crc32fast::hash(&damaged[..end - 4]);
    damaged[end - 4..].copy_from_slice(&whole_crc.to_le_bytes());
    std::fs::write(&sidecar, damaged).unwrap();
    assert!(Database::<f32>::open_read_only(&path, DIM).is_err());

    let mut oversized = original.clone();
    oversized[directory_start..directory_start + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    let directory_crc =
        crc32fast::hash(&oversized[directory_start..directory_start.saturating_add(directory_len)]);
    oversized[cursor + 8..cursor + 12].copy_from_slice(&directory_crc.to_le_bytes());
    let end = oversized.len();
    let whole_crc = crc32fast::hash(&oversized[..end - 4]);
    oversized[end - 4..].copy_from_slice(&whole_crc.to_le_bytes());
    std::fs::write(&sidecar, oversized).unwrap();
    assert!(Database::<f32>::open_read_only(&path, DIM).is_err());
    cleanup(&path);
}

#[test]
fn 图块_immutable_manifest_覆盖_sidecar_且只读零写() {
    let path = path("manifest");
    cleanup(&path);
    seed(&path);
    let mut writer = Database::<f32>::open(&path, DIM).unwrap();
    writer
        .publish_generation_manifest("graph-block-generation")
        .unwrap();
    drop(writer);
    let manifest = std::fs::read_to_string(format!("{path}.manifest.json")).unwrap();
    assert!(manifest.contains(".gidx"));
    let before = std::fs::read(format!("{path}.gidx")).unwrap();
    let immutable = Database::<f32>::open_immutable(&path, DIM).unwrap();
    assert_eq!(immutable.get_edges(1).len(), 2);
    drop(immutable);
    assert_eq!(std::fs::read(format!("{path}.gidx")).unwrap(), before);
    cleanup(&path);
}
