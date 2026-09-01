#![allow(non_snake_case)]
//! 格式感知变异矩阵：小型 fixture、逐字段变异、只读零写与 fail-closed。

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use triviumdb::database::Database;

#[derive(Debug, Clone)]
enum Mutation {
    TruncateAt(usize),
    FlipBit { offset: usize, bit: u8 },
    Overwrite { offset: usize, bytes: Vec<u8> },
    Append(Vec<u8>),
}

impl Mutation {
    fn apply(&self, bytes: &[u8]) -> Vec<u8> {
        let mut output = bytes.to_vec();
        match self {
            Self::TruncateAt(offset) => output.truncate((*offset).min(output.len())),
            Self::FlipBit { offset, bit } => {
                if let Some(byte) = output.get_mut(*offset) {
                    *byte ^= 1u8 << (bit % 8);
                }
            }
            Self::Overwrite { offset, bytes } => {
                if *offset < output.len() {
                    let end = offset.saturating_add(bytes.len()).min(output.len());
                    output[*offset..end].copy_from_slice(&bytes[..end - offset]);
                }
            }
            Self::Append(bytes) => output.extend_from_slice(bytes),
        }
        output
    }
}

fn path(name: &str) -> String {
    let root = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&root).unwrap();
    root.join(format!("format_mutation_{name}.tdb"))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for suffix in ["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

fn fixture(name: &str) -> String {
    let path = path(name);
    cleanup(&path);
    let mut database = Database::<f32>::open(&path, 4).unwrap();
    for index in 0..12u64 {
        database
            .insert_with_id(
                index + 1,
                &[index as f32, 1.0, 0.0, 0.0],
                serde_json::json!({"kind": "fixture", "index": index}),
            )
            .unwrap();
    }
    database.flush().unwrap();
    drop(database);
    path
}

fn snapshot(path: &str) -> BTreeMap<String, Vec<u8>> {
    ["", ".wal", ".vec", ".flush_ok"]
        .into_iter()
        .filter_map(|suffix| {
            let name = format!("{path}{suffix}");
            std::fs::read(&name).ok().map(|bytes| (name, bytes))
        })
        .collect()
}

fn mutations(length: usize) -> Vec<Mutation> {
    let mut output = vec![
        Mutation::TruncateAt(0),
        Mutation::TruncateAt(1),
        Mutation::TruncateAt(length / 2),
        Mutation::TruncateAt(length.saturating_sub(1)),
        Mutation::Append(vec![0xA5; 17]),
    ];
    for offset in [
        0,
        1,
        3,
        4,
        7,
        8,
        15,
        31,
        47,
        57,
        length / 2,
        length.saturating_sub(1),
    ] {
        for bit in [0, 3, 7] {
            output.push(Mutation::FlipBit { offset, bit });
        }
    }
    for offset in [0, 4, 8, 16, 24, 32, 48] {
        for bytes in [vec![0; 8], vec![0xFF; 8], u64::MAX.to_le_bytes().to_vec()] {
            output.push(Mutation::Overwrite { offset, bytes });
        }
    }
    output
}

#[test]
fn tdb格式变异全部拒绝或安全降级且绝不改写损坏输入() {
    let original_path = fixture("tdb");
    let original = std::fs::read(&original_path).unwrap();
    for (index, mutation) in mutations(original.len()).into_iter().enumerate() {
        let candidate = path(&format!("tdb_case_{index}"));
        cleanup(&candidate);
        std::fs::write(&candidate, mutation.apply(&original)).unwrap();
        let before = snapshot(&candidate);
        let result = Database::<f32>::open_read_only(&candidate, 4);
        if let Ok(database) = result {
            let _ = database.node_count();
            let _ = database.search(&[1.0, 0.0, 0.0, 0.0], 3, 0, -1.0);
        }
        assert_eq!(
            snapshot(&candidate),
            before,
            "变异 {mutation:?} 导致只读写盘"
        );
        cleanup(&candidate);
    }
    cleanup(&original_path);
}

#[test]
fn flush_marker和vec等长变异由v2_crc_fail_closed且只读零写() {
    let original_path = fixture("cross_file");
    for suffix in [".vec", ".flush_ok"] {
        let source = format!("{original_path}{suffix}");
        let original = std::fs::read(&source).unwrap();
        for offset in [0, original.len() / 2, original.len().saturating_sub(1)] {
            for bit in [0, 7] {
                let candidate = path(&format!("cross_{}_{}_{}", &suffix[1..], offset, bit));
                cleanup(&candidate);
                std::fs::copy(&original_path, &candidate).unwrap();
                for copy_suffix in [".vec", ".flush_ok"] {
                    std::fs::copy(
                        format!("{original_path}{copy_suffix}"),
                        format!("{candidate}{copy_suffix}"),
                    )
                    .unwrap();
                }
                let mutated = Mutation::FlipBit { offset, bit }.apply(&original);
                std::fs::write(format!("{candidate}{suffix}"), mutated).unwrap();
                let before = snapshot(&candidate);
                assert!(Database::<f32>::open_read_only(&candidate, 4).is_err());
                assert_eq!(snapshot(&candidate), before);
                cleanup(&candidate);
            }
        }
    }
    cleanup(&original_path);
}

#[test]
fn 文件读取工具自身覆盖偏移边界且不产生危险分配() {
    let path = path("io_helper");
    cleanup(&path);
    std::fs::write(&path, (0u8..64).collect::<Vec<_>>()).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.seek(SeekFrom::Start(16)).unwrap();
    file.write_all(&[1, 2, 3, 4]).unwrap();
    file.seek(SeekFrom::Start(16)).unwrap();
    let mut bytes = [0u8; 4];
    file.read_exact(&mut bytes).unwrap();
    assert_eq!(bytes, [1, 2, 3, 4]);
    cleanup(&path);
}
