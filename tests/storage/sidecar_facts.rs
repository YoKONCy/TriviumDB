//! 派生索引必须绑定事实内容；旧版无绑定索引通过事实重建或读取主文件保持一致。
use super::support;

use serde_json::json;
use triviumdb::database::{AccessMode, Config, MissingIndexPolicy, SearchConfig, StorageMode};
use triviumdb::{Database, TriviumError};

fn config(mode: StorageMode, access: AccessMode) -> Config {
    Config {
        dim: 2,
        storage_mode: mode,
        access_mode: access,
        auto_build_quiver: false,
        ..Default::default()
    }
}

#[test]
fn 多标签平行边在只读和可写模式保持完整性分数及排序() {
    let root = support::TestDir::new("mapped_graph_semantics");
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let path = root.file(&format!("{mode:?}.tdb"));
        let mut db =
            Database::<f32>::open_with_config(&path, config(mode, AccessMode::ReadWrite)).unwrap();
        for (id, v) in [(1, [1., 0.]), (2, [0.2, 0.98]), (3, [0.3, 0.95])] {
            db.insert_with_id(id, &v, json!({"id":id})).unwrap();
        }
        db.link(1, 2, "first", 1.).unwrap();
        db.link(1, 2, "second", 1.).unwrap();
        db.link(1, 3, "third", 1.).unwrap();
        db.close().unwrap();
        let search = SearchConfig {
            top_k: 3,
            expand_depth: 1,
            force_brute_force: true,
            enable_inverse_inhibition: true,
            min_score: 0.,
            ..Default::default()
        };
        let mut writable =
            Database::<f32>::open_with_config(&path, config(mode, AccessMode::ReadWrite)).unwrap();
        let expected = writable.search_advanced(&[1., 0.], &search).unwrap();
        assert_eq!(expected.iter().map(|h| h.id).collect::<Vec<_>>(), [1, 3, 2]);
        assert!(writable.validate_graph().unwrap().valid);
        writable.close().unwrap();
        let before = std::fs::read(format!("{path}.gidx")).unwrap();
        let mut readonly =
            Database::<f32>::open_with_config(&path, config(mode, AccessMode::ReadOnly)).unwrap();
        let actual = readonly.search_advanced(&[1., 0.], &search).unwrap();
        assert_eq!(actual.iter().map(|h| h.id).collect::<Vec<_>>(), [1, 3, 2]);
        assert_eq!(
            actual.iter().map(|h| h.score).collect::<Vec<_>>(),
            expected.iter().map(|h| h.score).collect::<Vec<_>>()
        );
        let edges = readonly.get_edges(1).unwrap();
        assert_eq!(
            edges
                .iter()
                .map(|e| (e.target_id, e.label.as_str(), e.weight))
                .collect::<Vec<_>>(),
            [(2, "first", 1.), (2, "second", 1.), (3, "third", 1.)]
        );
        assert_eq!(readonly.get_incoming_edges(2, None).unwrap().len(), 2);
        assert!(readonly.validate_graph().unwrap().valid);
        readonly.close().unwrap();
        assert_eq!(std::fs::read(format!("{path}.gidx")).unwrap(), before);
    }
}

#[test]
fn payload同长度更新后旧属性索引被拒绝且fallback查询事实正确() {
    let root = support::TestDir::new("property_fact_binding");
    for kind in ["hash", "ordered", "bitmap", "composite", "ngram"] {
        let path = root.file(&format!("{kind}.tdb"));
        let cfg = config(StorageMode::Mmap, AccessMode::ReadWrite);
        let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
        db.insert_with_id(10, &[1., 0.], json!({"value":"old", "tenant":"a"}))
            .unwrap();
        match kind {
            "hash" => db.create_index("value"),
            "ordered" => db.create_ordered_index("value"),
            "bitmap" => db.create_bitmap_index("value"),
            "ngram" => db.create_ngram_index("value"),
            _ => db.create_composite_index(&["tenant".into(), "value".into()]),
        }
        .unwrap();
        db.close().unwrap();
        let old = std::fs::read(format!("{path}.pidx")).unwrap();
        let main = std::fs::read(&path).unwrap();
        let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
        db.update_payload(10, json!({"value":"new", "tenant":"a"}))
            .unwrap();
        db.close().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), main);
        std::fs::write(format!("{path}.pidx"), &old).unwrap();
        for access in [AccessMode::ReadOnly, AccessMode::ReadWrite] {
            let result = Database::<f32>::open_with_config(
                &path,
                Config {
                    access_mode: access,
                    missing_index_policy: MissingIndexPolicy::Error,
                    ..cfg
                },
            );
            assert!(
                matches!(result, Err(TriviumError::CorruptedFile(_))),
                "{kind}/{access:?}"
            );
            let mut db = Database::<f32>::open_with_config(
                &path,
                Config {
                    access_mode: access,
                    ..cfg
                },
            )
            .unwrap();
            assert_eq!(
                db.get_payload(10).unwrap(),
                Some(json!({"value":"new", "tenant":"a"}))
            );
            let rows = db.tql_values("FIND {value: \"new\"} RETURN *").unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(
                rows[0]
                    .values()
                    .map(|value| value.as_node().unwrap().id)
                    .collect::<Vec<_>>(),
                [10]
            );
            assert!(
                db.tql_values("FIND {value: \"old\"} RETURN *")
                    .unwrap()
                    .is_empty()
            );
            db.close().unwrap();
            // 每种访问模式均面对同一旧代文件。
            std::fs::write(format!("{path}.pidx"), &old).unwrap();
        }
    }
}

#[test]
fn 节点数不变的边权及元数据换代拒绝旧图索引() {
    let root = support::TestDir::new("graph_fact_binding");
    let path = root.file("graph.tdb");
    let cfg = config(StorageMode::Mmap, AccessMode::ReadWrite);
    let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
    db.insert_with_id(1, &[1., 0.], json!(null)).unwrap();
    db.insert_with_id(2, &[0., 1.], json!(null)).unwrap();
    db.upsert_edge(1, 2, "edge", 1., json!({"generation":"old"}))
        .unwrap();
    db.close().unwrap();
    let old = std::fs::read(format!("{path}.gidx")).unwrap();
    let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
    db.upsert_edge(1, 2, "edge", 2., json!({"generation":"new"}))
        .unwrap();
    db.close().unwrap();
    std::fs::write(format!("{path}.gidx"), &old).unwrap();
    assert!(matches!(
        Database::<f32>::open_with_config(
            &path,
            Config {
                access_mode: AccessMode::ReadOnly,
                ..cfg
            }
        ),
        Err(TriviumError::CorruptedFile(_))
    ));
    assert_eq!(std::fs::read(format!("{path}.gidx")).unwrap(), old);
    let mut writer = Database::<f32>::open_with_config(&path, cfg).unwrap();
    let edge = writer.get_edge(1, 2, "edge").unwrap().unwrap();
    assert_eq!(edge.weight, 2.);
    assert_eq!(edge.metadata, json!({"generation":"new"}));
    writer.repair_graph_indexes().unwrap();
    writer.close().unwrap();
    let mut reader = Database::<f32>::open_read_only(&path, 2).unwrap();
    assert_eq!(reader.get_edge(1, 2, "edge").unwrap(), Some(edge));
    assert!(reader.validate_graph().unwrap().valid);
    reader.close().unwrap();
}

#[test]
fn 无事实绑定的旧索引通过重建或主文件回退保持语义和只读零写() {
    let root = support::TestDir::new("legacy_sidecar_facts");
    let path = root.file("legacy.tdb");
    let cfg = config(StorageMode::Mmap, AccessMode::ReadWrite);
    let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
    db.insert_with_id(1, &[1., 0.], json!({"value":"old"}))
        .unwrap();
    db.insert_with_id(2, &[0., 1.], json!({"value":"two"}))
        .unwrap();
    db.create_index("value").unwrap();
    db.upsert_edge(1, 2, "edge", 1., json!(null)).unwrap();
    db.close().unwrap();
    let mut property = std::fs::read(format!("{path}.pidx")).unwrap();
    property.drain(36..48);
    property[4..6].copy_from_slice(&6u16.to_le_bytes());
    let end = property.len() - 4;
    let crc = crc32fast::hash(&property[..end]);
    property[end..].copy_from_slice(&crc.to_le_bytes());
    let mut graph = std::fs::read(format!("{path}.gidx")).unwrap();
    graph.drain(24..36);
    graph[4..6].copy_from_slice(&2u16.to_le_bytes());
    let end = graph.len() - 4;
    let crc = crc32fast::hash(&graph[..end]);
    graph[end..].copy_from_slice(&crc.to_le_bytes());
    let mut db = Database::<f32>::open_with_config(&path, cfg).unwrap();
    db.update_payload(1, json!({"value":"new"})).unwrap();
    db.upsert_edge(1, 2, "edge", 2., json!(null)).unwrap();
    db.close().unwrap();
    // 属性 v6 还绑定主文件 CRC，使用当前主文件身份只保留旧 Payload posting。
    property[8..16].copy_from_slice(&std::fs::metadata(&path).unwrap().len().to_le_bytes());
    let crc = crc32fast::hash(&std::fs::read(&path).unwrap());
    property[16..20].copy_from_slice(&crc.to_le_bytes());
    let end = property.len() - 4;
    let crc = crc32fast::hash(&property[..end]);
    property[end..].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(format!("{path}.pidx"), &property).unwrap();
    std::fs::write(format!("{path}.gidx"), &graph).unwrap();
    let mut reader = Database::<f32>::open_read_only(&path, 2).unwrap();
    assert_eq!(
        reader
            .indexed_lookup(&[("value".into(), json!("new"))], 10)
            .unwrap(),
        [1]
    );
    assert!(
        reader
            .indexed_lookup(&[("value".into(), json!("old"))], 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(reader.get_edge(1, 2, "edge").unwrap().unwrap().weight, 2.);
    assert!(reader.validate_graph().unwrap().valid);
    reader.close().unwrap();
    assert_eq!(std::fs::read(format!("{path}.pidx")).unwrap(), property);
    assert_eq!(std::fs::read(format!("{path}.gidx")).unwrap(), graph);
}
