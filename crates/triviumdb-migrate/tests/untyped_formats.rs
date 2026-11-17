//! 无类型头旧布局的独立字节样本，以及真实 0.8.10 文本制品。

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use triviumdb::{Database, VectorType};
use triviumdb_migrate::Options;

fn options(source: PathBuf, destination: PathBuf, mmap: bool) -> Options {
    Options {
        source,
        source_dtype: None,
        destination,
        check: false,
        memory_bytes: 128 * 1024 * 1024,
        max_source_bytes: 1024 * 1024,
        max_work_bytes: 2048 * 1024 * 1024,
        mmap,
    }
}

fn files(path: &Path) -> std::collections::BTreeMap<std::ffi::OsString, Vec<u8>> {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect()
}

fn checkpoint<T: VectorType>(version: u16, vectors: &[Vec<T>]) -> Vec<u8> {
    let mut payloads = Vec::new();
    for (index, _) in vectors.iter().enumerate() {
        let payload = json!({"rank":index+1}).to_string();
        payloads.extend_from_slice(&(7 + index as u64).to_le_bytes());
        payloads.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        payloads.extend_from_slice(payload.as_bytes());
    }
    let vector_offset = 58 + payloads.len() as u64;
    let vector_bytes = vectors
        .iter()
        .flat_map(|v| bytemuck::cast_slice::<T, u8>(v).iter().copied())
        .collect::<Vec<_>>();
    let edge_offset = vector_offset + vector_bytes.len() as u64;
    let mut edges = Vec::new();
    if !vectors.is_empty() {
        edges.extend_from_slice(&7u64.to_le_bytes());
        edges.extend_from_slice(&8u64.to_le_bytes());
        edges.extend_from_slice(&4u16.to_le_bytes());
        edges.extend_from_slice(b"next");
        edges.extend_from_slice(&0.5f32.to_le_bytes());
        if version >= 7 {
            let metadata = br#"{"from":"fixture"}"#;
            edges.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
            edges.extend_from_slice(metadata);
        }
    }
    let bq_offset = edge_offset + edges.len() as u64;
    let mut output = b"TVDB".to_vec();
    output.extend_from_slice(&version.to_le_bytes());
    output.extend_from_slice(&2u32.to_le_bytes());
    output.extend_from_slice(&(if vectors.is_empty() { 1u64 } else { 10u64 }).to_le_bytes());
    output.extend_from_slice(&(vectors.len() as u64).to_le_bytes());
    for offset in [58u64, vector_offset, edge_offset, bq_offset] {
        output.extend_from_slice(&offset.to_le_bytes());
    }
    output.extend_from_slice(&payloads);
    output.extend_from_slice(&vector_bytes);
    output.extend_from_slice(&edges);
    if version == 5 {
        output.extend_from_slice(&0u64.to_le_bytes());
    } else {
        output.extend_from_slice(b"TBQF");
        output.extend_from_slice(&1u16.to_le_bytes());
        output.extend_from_slice(&48u16.to_le_bytes());
        output.extend_from_slice(&0u64.to_le_bytes());
    }
    if version >= 8 {
        let mut unique = b"TUQC".to_vec();
        unique.extend_from_slice(&1u16.to_le_bytes());
        unique.extend_from_slice(&0u32.to_le_bytes());
        output.extend_from_slice(&unique);
        output.extend_from_slice(&crc32fast::hash(&unique).to_le_bytes());
    }
    output
}

fn run<T: VectorType + Serialize + DeserializeOwned>(vectors: [Vec<T>; 2]) {
    for version in 5..=9 {
        for mmap in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("source");
            std::fs::create_dir(&source).unwrap();
            let database = source.join("sample.tdb");
            std::fs::write(&database, checkpoint(version, &vectors)).unwrap();
            std::fs::write(source.join("sample.tdb.lock"), []).unwrap();
            let before = files(&source);
            let opts = options(database, root.path().join("target"), mmap);
            let report = triviumdb_migrate::run(&opts).unwrap();
            assert_eq!(report.source_format, version);
            assert_eq!(report.facts.next_id, 10);
            for _ in 0..2 {
                let mut db =
                    Database::<T>::open_read_only(report.database.to_str().unwrap(), 2).unwrap();
                assert_eq!(db.all_node_ids().unwrap(), [7, 8]);
                for (i, expected) in vectors.iter().enumerate() {
                    let node = db.get(7 + i as u64).unwrap().unwrap();
                    assert_eq!(
                        bytemuck::cast_slice::<T, u8>(&node.vector),
                        bytemuck::cast_slice::<T, u8>(expected)
                    );
                    assert_eq!(node.payload, json!({"rank":i+1}));
                }
                assert_eq!(
                    serde_json::to_value(db.get_edges(7).unwrap()).unwrap(),
                    json!([{
                        "target_id":8,"label":"next","weight":0.5,
                        "metadata":if version>=7 {json!({"from":"fixture"})} else {Value::Null}
                    }])
                );
                assert!(db.get_edges(8).unwrap().is_empty());
                db.close().unwrap();
            }
            assert_eq!(files(&source), before);
        }
    }
}

#[test]
fn untyped_layouts_preserve_all_dtypes_and_edges() {
    run([vec![1f32, -0.0], vec![0., 1.]]);
    run([
        vec![half::f16::ONE, half::f16::NEG_ZERO],
        vec![half::f16::ZERO, half::f16::ONE],
    ]);
    run([vec![u64::MAX - 1, 9007199254740993], vec![0, u64::MAX]]);
}

#[test]
fn empty_untyped_checkpoint_requires_dtype_and_replays_confirmed_wal() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let database = source.join("sample.tdb");
    std::fs::write(&database, checkpoint::<f32>(9, &[])).unwrap();
    std::fs::write(source.join("sample.tdb.lock"), []).unwrap();
    let mut frame = 2u32.to_le_bytes().to_vec();
    frame.extend_from_slice(&7u64.to_le_bytes());
    frame.extend_from_slice(&2u64.to_le_bytes());
    frame.extend_from_slice(&1f32.to_le_bytes());
    frame.extend_from_slice(&(-0f32).to_le_bytes());
    frame.extend_from_slice(&2u64.to_le_bytes());
    frame.extend_from_slice(b"{}");
    let mut wal = b"TVWL\x03\x00".to_vec();
    wal.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    wal.extend_from_slice(&frame);
    wal.extend_from_slice(&crc32fast::hash(&frame).to_le_bytes());
    std::fs::write(source.join("sample.tdb.wal"), wal).unwrap();
    let before = files(&source);
    let mut opts = options(database, root.path().join("target"), true);
    assert_eq!(
        triviumdb_migrate::run(&opts).unwrap_err().code,
        "TDB_MIGRATION_DTYPE_REQUIRED"
    );
    assert!(!opts.destination.exists());
    assert_eq!(files(&source), before);
    opts.source_dtype = Some(1);
    let report = triviumdb_migrate::run(&opts).unwrap();
    assert_eq!((report.facts.nodes, report.facts.next_id), (1, 8));
    let mut db = Database::<f32>::open_read_only(report.database.to_str().unwrap(), 2).unwrap();
    let node = db.get(7).unwrap().unwrap();
    assert_eq!(node.payload, json!({}));
    assert_eq!(
        node.vector.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        [1f32.to_bits(), (-0f32).to_bits()]
    );
    db.close().unwrap();
    assert_eq!(files(&source), before);
}

#[test]
fn native_0810_registered_text_and_keywords_survive_migration() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/v0810_text_mmap");
    for entry in std::fs::read_dir(&fixture).unwrap() {
        let entry = entry.unwrap();
        if entry
            .file_name()
            .to_str()
            .unwrap()
            .starts_with("sample.tdb")
        {
            std::fs::copy(entry.path(), source.join(entry.file_name())).unwrap();
        }
    }
    std::fs::write(source.join("sample.tdb.lock"), []).unwrap();
    let before = files(&source);
    let report = triviumdb_migrate::run(&options(
        source.join("sample.tdb"),
        root.path().join("target"),
        true,
    ))
    .unwrap();
    let mut db = Database::<f32>::open_with_config(
        report.database.to_str().unwrap(),
        triviumdb::database::Config {
            dim: 4,
            access_mode: triviumdb::database::AccessMode::ReadOnly,
            load_text_index: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(db.all_node_ids().unwrap(), [1, 2, 3, 4]);
    for i in 0..4 {
        let node = db.get(i + 1).unwrap().unwrap();
        assert_eq!(node.vector, [1., i as f32, 0., 0.]);
        assert_eq!(node.payload, json!({"content":"测试正文 alpha","index":i}));
        let rows = db
            .tql_nodes(&format!("TEXT AC \"测试别名{i}\" TOP 10 AS n RETURN n"))
            .unwrap();
        assert_eq!(rows.iter().map(|r| r["n"].id).collect::<Vec<_>>(), [i + 1]);
    }
    db.close().unwrap();
    assert_eq!(files(&source), before);
}
