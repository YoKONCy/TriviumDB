//! 索引定义编码、历史加速层回退及图副本的事实一致性边界。
use super::support;

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use triviumdb::database::{AccessMode, Config, MissingIndexPolicy, StorageMode};
use triviumdb::storage::graph_blocks::MappedGraphStore;
use triviumdb::{Database, TriviumError};

fn config(mode: StorageMode, access_mode: AccessMode) -> Config {
    Config {
        dim: 2,
        storage_mode: mode,
        access_mode,
        auto_build_quiver: false,
        ..Default::default()
    }
}
fn group(root: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}
fn ids(db: &Database<f32>, query: &str) -> Vec<u64> {
    let mut result = db
        .tql_nodes(query)
        .unwrap()
        .into_iter()
        .flat_map(|row| row.into_values().map(|node| node.id))
        .collect::<Vec<_>>();
    result.sort_unstable();
    result
}
fn contract() -> Value {
    serde_json::from_str(include_str!("../contracts/index_schema_boundaries.json")).unwrap()
}

#[test]
fn 复合字段列表逐字节往返且不同边界定义并存() {
    let case = contract();
    let definitions: Vec<Vec<String>> =
        serde_json::from_value(case["definitions"].clone()).unwrap();
    let mut expected_definitions = definitions.clone();
    expected_definitions.sort();
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for unique in [false, true] {
            let root = support::TestDir::new("composite_field_identity");
            let path = root.file("sample.tdb");
            let mut db =
                Database::<f32>::open_with_config(&path, config(mode, AccessMode::ReadWrite))
                    .unwrap();
            db.insert_with_id(1, &[1., 0.], case["payload"].clone())
                .unwrap();
            for fields in &definitions {
                if unique {
                    db.create_unique_composite_index(fields).unwrap();
                } else {
                    db.create_composite_index(fields).unwrap();
                }
            }
            assert_eq!(ids(&db, case["query"].as_str().unwrap()), [1]);
            db.close().unwrap();
            drop(db);
            for access in [AccessMode::ReadOnly, AccessMode::ReadWrite] {
                let before = group(&root.0);
                let mut db =
                    Database::<f32>::open_with_config(&path, config(mode, access)).unwrap();
                assert_eq!(db.get_payload(1).unwrap(), Some(case["payload"].clone()));
                assert_eq!(ids(&db, case["query"].as_str().unwrap()), [1]);
                let mut actual = db
                    .index_info()
                    .unwrap()
                    .into_iter()
                    .map(|index| index.fields)
                    .collect::<Vec<_>>();
                actual.sort();
                assert_eq!(actual, expected_definitions);
                for fields in &definitions {
                    let equalities = fields
                        .iter()
                        .map(|field| (field.clone(), case["payload"][field].clone()))
                        .collect::<Vec<_>>();
                    assert_eq!(db.indexed_lookup(&equalities, 10).unwrap(), [1]);
                }
                db.close().unwrap();
                if access == AccessMode::ReadOnly {
                    assert_eq!(group(&root.0), before);
                }
            }
        }
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> usize {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize
}
fn u64_at(bytes: &[u8], offset: usize) -> usize {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()) as usize
}
fn sign(bytes: &mut [u8], offset: usize, range: std::ops::Range<usize>) {
    let crc = crc32fast::hash(&bytes[range]);
    bytes[offset..offset + 4].copy_from_slice(&crc.to_le_bytes());
}

#[test]
fn 历史复合字段边界不明确时查询扫描事实且唯一约束从主文件恢复() {
    let case = contract();
    let fields = vec!["a\0b".into(), "c".into()];
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        for unique in [false, true] {
            let root = support::TestDir::new("legacy_composite_fields");
            let path = root.file("sample.tdb");
            let mut db =
                Database::<f32>::open_with_config(&path, config(mode, AccessMode::ReadWrite))
                    .unwrap();
            db.insert_with_id(1, &[1., 0.], case["payload"].clone())
                .unwrap();
            if unique {
                db.create_unique_composite_index(&fields).unwrap();
            } else {
                db.create_composite_index(&fields).unwrap();
            }
            db.close().unwrap();
            drop(db);
            let mut bytes = std::fs::read(format!("{path}.pidx")).unwrap();
            // 独立 v7 布局：48 字节头、kind/flags、长度和 NUL 分隔的原始字段名。
            let old_len = u32_at(&bytes, 50);
            bytes.splice(54..54 + old_len, b"a\0b\0c".iter().copied());
            bytes[50..54].copy_from_slice(&5u32.to_le_bytes());
            bytes[4..6].copy_from_slice(&7u16.to_le_bytes());
            let end = bytes.len() - 4;
            sign(&mut bytes, end, 0..end);
            std::fs::write(format!("{path}.pidx"), bytes).unwrap();
            let before = group(&root.0);
            let mut reader =
                Database::<f32>::open_with_config(&path, config(mode, AccessMode::ReadOnly))
                    .unwrap();
            assert_eq!(ids(&reader, case["query"].as_str().unwrap()), [1]);
            assert_eq!(
                reader.list_unique_indexes().unwrap(),
                if unique { vec![fields.clone()] } else { vec![] }
            );
            reader.close().unwrap();
            drop(reader);
            assert_eq!(group(&root.0), before);
            let mut writer =
                Database::<f32>::open_with_config(&path, config(mode, AccessMode::ReadWrite))
                    .unwrap();
            if unique {
                assert!(matches!(
                    writer.insert_with_id(2, &[0., 1.], case["payload"].clone()),
                    Err(TriviumError::UniqueConstraintViolation { .. })
                ));
            } else {
                writer.create_composite_index(&fields).unwrap();
            }
            writer.close().unwrap();
            let mut reader =
                Database::<f32>::open_with_config(&path, config(mode, AccessMode::ReadOnly))
                    .unwrap();
            assert_eq!(ids(&reader, case["query"].as_str().unwrap()), [1]);
            assert_eq!(reader.index_info().unwrap()[0].fields, fields);
            reader.close().unwrap();
        }
    }
}

#[test]
fn 复合字段磁盘边界在构建与写日志前拒绝() {
    let root = support::TestDir::new("composite_field_limits");
    let path = root.file("sample.tdb");
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"a":1})).unwrap();
    // 总字段名超过 1 MiB，或数量超过主文件 u16 字段计数上限。
    let cases = [
        vec!["a".repeat(1024 * 1024), "b".into()],
        (0..=u16::MAX)
            .map(|index| format!("field{index}"))
            .collect::<Vec<_>>(),
    ];
    let before = std::fs::read(format!("{path}.wal")).unwrap();
    for fields in cases {
        assert!(matches!(
            db.create_composite_index(&fields),
            Err(TriviumError::InvalidInput(_))
        ));
        assert!(matches!(
            db.create_unique_composite_index(&fields),
            Err(TriviumError::InvalidInput(_))
        ));
        assert_eq!(std::fs::read(format!("{path}.wal")).unwrap(), before);
        assert_eq!(db.get_payload(1).unwrap(), Some(json!({"a":1})));
    }
    db.close().unwrap();
}

fn damage_graph(original: &[u8], mutation: &str) -> Vec<u8> {
    let mut bytes = original.to_vec();
    assert_eq!(u64_at(&bytes, 16), 1);
    let block = 36;
    let start = block + 24;
    let end = start + u32_at(&bytes, block + 12);
    let directory = end + 16;
    let directory_end = directory + u64_at(&bytes, end);
    let label = directory + 48;
    match mutation {
        "json" => {
            let metadata = start + 20 + u32_at(&bytes, start + 8);
            bytes[metadata] = b'!';
        }
        "metadata" => {
            assert_eq!(bytes[end - 2], b'1');
            bytes[end - 2] = b'2';
        }
        "weight" => bytes[start + 16..start + 20].copy_from_slice(&9f32.to_le_bytes()),
        "count" => bytes[block + 8..block + 12].copy_from_slice(&2u32.to_le_bytes()),
        "incoming" => bytes[directory + 8..directory + 16].copy_from_slice(&3u64.to_le_bytes()),
        "label" => bytes[label..label + 3].copy_from_slice(b"bad"),
        "source" => {
            bytes[block..block + 8].copy_from_slice(&3u64.to_le_bytes());
            bytes[directory + 24..directory + 32].copy_from_slice(&3u64.to_le_bytes());
            bytes[label + 3..label + 11].copy_from_slice(&3u64.to_le_bytes());
        }
        "target" => {
            bytes[start..start + 8].copy_from_slice(&3u64.to_le_bytes());
            bytes[directory + 8..directory + 16].copy_from_slice(&3u64.to_le_bytes());
            bytes[label + 11..label + 19].copy_from_slice(&3u64.to_le_bytes());
        }
        "utf8" => bytes[start + 20] = 0xff,
        "reserved" => bytes[block + 20] = 1,
        _ => panic!("未知畸形样例"),
    }
    sign(&mut bytes, block + 16, start..end);
    sign(&mut bytes, end + 8, directory..directory_end);
    let payload_end = bytes.len() - 4;
    sign(&mut bytes, payload_end, 0..payload_end);
    bytes
}

#[test]
fn crc合法的畸形图副本在采用前拒绝且主文件事实保留() {
    for mutation in [
        "json", "metadata", "weight", "count", "incoming", "label", "source", "target", "utf8",
        "reserved",
    ] {
        let root = support::TestDir::new("graph_semantic_validation");
        let path = root.file("sample.tdb");
        let mut db = Database::<f32>::open_with_config(
            &path,
            config(StorageMode::Mmap, AccessMode::ReadWrite),
        )
        .unwrap();
        for id in 1..=3 {
            db.insert_with_id(id, &[1., 0.], json!({"id":id})).unwrap();
        }
        db.upsert_edge(1, 2, "rel", 2., json!({"ok":1})).unwrap();
        db.close().unwrap();
        drop(db);
        let damaged = damage_graph(&std::fs::read(format!("{path}.gidx")).unwrap(), mutation);
        std::fs::write(format!("{path}.gidx"), damaged).unwrap();
        // 清单自身覆盖当前副本校验和，再独立验证结构与主文件事实。
        triviumdb::storage::snapshot::write_manifest(&path, "graph-boundary", "f32", 2, 3).unwrap();
        let before = group(&root.0);
        assert!(
            matches!(
                MappedGraphStore::open(&path, 3),
                Err(TriviumError::CorruptedFile(_))
            ),
            "{mutation}"
        );
        for policy in [
            MissingIndexPolicy::Fallback,
            MissingIndexPolicy::Error,
            MissingIndexPolicy::BuildInMemory,
        ] {
            assert!(
                matches!(
                    Database::<f32>::open_with_config(
                        &path,
                        Config {
                            missing_index_policy: policy,
                            ..config(StorageMode::Mmap, AccessMode::ReadOnly)
                        }
                    ),
                    Err(TriviumError::CorruptedFile(_))
                ),
                "{mutation}/{policy:?}"
            );
            assert_eq!(group(&root.0), before);
        }
        assert!(
            matches!(
                Database::<f32>::open_immutable(&path, 2),
                Err(TriviumError::CorruptedFile(_))
            ),
            "{mutation}/Immutable"
        );
        assert_eq!(group(&root.0), before);
        let mut writer = Database::<f32>::open_with_config(
            &path,
            config(StorageMode::Mmap, AccessMode::ReadWrite),
        )
        .unwrap();
        let edge = writer.get_edge(1, 2, "rel").unwrap().unwrap();
        assert_eq!(
            (edge.target_id, edge.weight, edge.metadata),
            (2, 2., json!({"ok":1}))
        );
        assert_eq!(writer.get_incoming_edges(2, None).unwrap().len(), 1);
        writer.repair_graph_indexes().unwrap();
        writer.close().unwrap();
        let mut reader = Database::<f32>::open_with_config(
            &path,
            config(StorageMode::Mmap, AccessMode::ReadOnly),
        )
        .unwrap();
        assert_eq!(reader.get_edge(1, 2, "rel").unwrap().unwrap().weight, 2.);
        assert!(reader.validate_graph().unwrap().valid);
        reader.close().unwrap();
    }
}

#[test]
fn 合法图副本保留平行标签自环大id及完整metadata() {
    let root = support::TestDir::new("valid_graph_copy");
    let path = root.file("sample.tdb");
    let large = u64::MAX - 2;
    let mut db = Database::<f32>::open(&path, 2).unwrap();
    for id in [1, 2, large] {
        db.insert_with_id(id, &[1., 0.], json!({"id":id})).unwrap();
    }
    let edges = [
        (1, 1, "自环", -0.0, json!(null)),
        (
            1,
            2,
            "a",
            0.5,
            json!({"array":[null, true, u64::MAX], "object":{"v":"中文"}}),
        ),
        (1, 2, "b", -2., json!([])),
        (large, 2, "a", 3., json!({})),
    ];
    for (source, target, label, weight, metadata) in &edges {
        db.upsert_edge(*source, *target, label, *weight, metadata.clone())
            .unwrap();
    }
    db.close().unwrap();
    drop(db);
    let before = group(&root.0);
    let graph = MappedGraphStore::open(&path, 3).unwrap().unwrap();
    assert_eq!(graph.edge_count(), 4);
    assert_eq!(graph.by_label("a"), [(1, 2), (large, 2)]);
    drop(graph);
    let mut reader = Database::<f32>::open_read_only(&path, 2).unwrap();
    for (source, target, label, weight, metadata) in edges {
        let edge = reader.get_edge(source, target, label).unwrap().unwrap();
        assert_eq!(edge.weight.to_bits(), weight.to_bits());
        assert_eq!(edge.metadata, metadata);
    }
    assert_eq!(reader.get_incoming_edges(2, None).unwrap().len(), 3);
    assert!(reader.validate_graph().unwrap().valid);
    reader.close().unwrap();
    drop(reader);
    assert_eq!(group(&root.0), before);
}

#[test]
fn 复合字段编码畸形时严格拒绝或扫描事实() {
    for definition in [
        r#"["a","a"]"#,
        r#"["", "c"]"#,
        r#"[1,"c"]"#,
        r#"{"a":"c"}"#,
        r#"["a"]"#,
    ] {
        let root = support::TestDir::new("malformed_composite_definition");
        let path = root.file("sample.tdb");
        let case = contract();
        let mut db = Database::<f32>::open(&path, 2).unwrap();
        db.insert_with_id(1, &[1., 0.], case["payload"].clone())
            .unwrap();
        db.create_composite_index(&["a\0b".into(), "c".into()])
            .unwrap();
        db.close().unwrap();
        drop(db);
        let mut bytes = std::fs::read(format!("{path}.pidx")).unwrap();
        let len = u32_at(&bytes, 50);
        bytes.splice(54..54 + len, definition.bytes());
        bytes[50..54].copy_from_slice(&(definition.len() as u32).to_le_bytes());
        let end = bytes.len() - 4;
        sign(&mut bytes, end, 0..end);
        std::fs::write(format!("{path}.pidx"), bytes).unwrap();
        let before = group(&root.0);
        assert!(matches!(
            Database::<f32>::open_with_config(
                &path,
                Config {
                    missing_index_policy: MissingIndexPolicy::Error,
                    ..config(StorageMode::Mmap, AccessMode::ReadOnly)
                }
            ),
            Err(TriviumError::CorruptedFile(_))
        ));
        let mut reader = Database::<f32>::open_read_only(&path, 2).unwrap();
        assert_eq!(ids(&reader, case["query"].as_str().unwrap()), [1]);
        reader.close().unwrap();
        drop(reader);
        assert_eq!(group(&root.0), before);
    }
}
