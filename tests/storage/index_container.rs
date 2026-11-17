//! 统一容器的完整事实、独立字节变异及只读文件契约。
use super::support::TestDir;
use serde_json::json;
use triviumdb::Database;
use triviumdb::database::{Config, StorageMode};

fn seed(path: &str, mode: StorageMode) {
    let mut db = Database::<f32>::open_with_config(
        path,
        Config {
            dim: 2,
            storage_mode: mode,
            auto_build_quiver: false,
            ..Default::default()
        },
    )
    .unwrap();
    db.insert_with_id(7, &[1., 0.], json!({"kind":"source"}))
        .unwrap();
    db.insert_with_id(11, &[0., 1.], json!({"kind":"target"}))
        .unwrap();
    db.upsert_edge(7, 11, "引用", 0.5, json!({"page":3}))
        .unwrap();
    db.index_text(7, "alpha evidence").unwrap();
    db.index_keyword(11, "beta").unwrap();
    db.create_index("kind").unwrap();
    db.build_quiver_index(None).unwrap();
    db.close().unwrap();
}

fn facts(db: &Database<f32>) {
    assert_eq!(db.node_count().unwrap(), 2);
    let node = db.get(7).unwrap().unwrap();
    assert_eq!(node.vector, [1., 0.]);
    assert_eq!(node.payload, json!({"kind":"source"}));
    assert_eq!(node.edges.len(), 1);
    assert_eq!(
        (
            node.edges[0].target_id,
            node.edges[0].label.as_str(),
            node.edges[0].weight
        ),
        (11, "引用", 0.5)
    );
    assert_eq!(node.edges[0].metadata, json!({"page":3}));
    assert_eq!(db.get(11).unwrap().unwrap().vector, [0., 1.]);
    assert_eq!(db.index_info().unwrap().len(), 1);
    for (query, expected) in [
        ("TEXT BM25 \"alpha\" TOP 10 AS h WITH h RETURN h", 7),
        ("TEXT AC \"beta\" TOP 10 AS h WITH h RETURN h", 11),
    ] {
        assert_eq!(
            db.tql_values(query)
                .unwrap()
                .iter()
                .map(|row| row["h"].as_node().unwrap().id)
                .collect::<Vec<_>>(),
            [expected]
        );
    }
    let result = db.search(&[1., 0.], 2, 0, -1.).unwrap();
    assert_eq!(
        result
            .iter()
            .map(|hit| (hit.id, hit.score))
            .collect::<Vec<_>>(),
        [(7, 1.), (11, 0.)]
    );
}

#[test]
fn 统一容器保留全部组件且旧映射关闭后可移动重开() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("index_container");
        let path = root.file("data.tdb");
        seed(&path, mode);
        assert!(std::path::Path::new(&format!("{path}.idx")).is_file());
        for suffix in [
            ".text",
            ".text.meta",
            ".quiver",
            ".quiver.meta",
            ".pidx",
            ".gidx",
        ] {
            assert!(!std::path::Path::new(&format!("{path}{suffix}")).exists());
            assert!(
                !super::artifact::read(format!("{path}{suffix}"))
                    .unwrap()
                    .is_empty()
            );
        }
        let before = std::fs::read(format!("{path}.idx")).unwrap();
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 2,
                access_mode: triviumdb::database::AccessMode::ReadOnly,
                load_text_index: true,
                ..Default::default()
            },
        )
        .unwrap();
        facts(&db);
        let retained = db.get(7).unwrap().unwrap();
        db.close().unwrap();
        assert_eq!(std::fs::read(format!("{path}.idx")).unwrap(), before);
        std::fs::rename(format!("{path}.idx"), format!("{path}.idx.moved")).unwrap();
        std::fs::rename(format!("{path}.idx.moved"), format!("{path}.idx")).unwrap();
        assert_eq!(retained.payload, json!({"kind":"source"}));
        let mut writer = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 2,
                storage_mode: mode,
                load_text_index: true,
                ..Default::default()
            },
        )
        .unwrap();
        writer
            .update_payload(11, json!({"kind":"updated"}))
            .unwrap();
        writer.flush().unwrap();
        writer.close().unwrap();
        let mut reopened = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 2,
                access_mode: triviumdb::database::AccessMode::ReadOnly,
                load_text_index: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            reopened.get_payload(11).unwrap(),
            Some(json!({"kind":"updated"}))
        );
        facts(&reopened);
        reopened.close().unwrap();
    }
}

#[test]
fn 目录越界重叠未知版本与缺失容器均结构化拒绝且保持源文件() {
    let root = TestDir::new("index_container_corruption");
    let path = root.file("data.tdb");
    seed(&path, StorageMode::Mmap);
    let container = format!("{path}.idx");
    let original = std::fs::read(&container).unwrap();
    let mut mutations = Vec::new();
    let mut version = original.clone();
    version[8..12].copy_from_slice(&999u32.to_le_bytes());
    mutations.push(version);
    let mut range = original.clone();
    range[40..48].copy_from_slice(&u64::MAX.to_le_bytes());
    mutations.push(range);
    let mut overlap = original.clone();
    overlap[72..80].copy_from_slice(&65536u64.to_le_bytes());
    mutations.push(overlap);
    let other = root.file("other.tdb");
    seed(&other, StorageMode::Mmap);
    let mut foreign = std::fs::read(format!("{other}.idx")).unwrap();
    // 保留本库主文件长度及 CRC，独立验证容器身份仍拒绝另一数据库的组件。
    foreign[16..28].copy_from_slice(&original[16..28]);
    foreign[272..288].copy_from_slice(&original[272..288]);
    mutations.push(foreign);
    for mut bytes in mutations {
        let crc = crc32fast::hash(&bytes[..508]);
        bytes[508..512].copy_from_slice(&crc.to_le_bytes());
        std::fs::write(&container, &bytes).unwrap();
        let error = Database::<f32>::open_read_only(&path, 2)
            .err()
            .expect("非法目录必须拒绝");
        assert_eq!(error.code(), "TDB_CORRUPTED_FILE");
        assert_eq!(std::fs::read(&container).unwrap(), bytes);
    }
    // 即使旧独立组件仍在，缺失的统一容器也必须拒绝，避免复用失配的事实。
    std::fs::write(&container, &original).unwrap();
    for suffix in [
        ".text",
        ".text.meta",
        ".quiver",
        ".quiver.meta",
        ".pidx",
        ".gidx",
    ] {
        let bytes = super::artifact::read(format!("{path}{suffix}")).unwrap();
        std::fs::write(format!("{path}{suffix}"), bytes).unwrap();
    }
    std::fs::remove_file(&container).unwrap();
    let error = Database::<f32>::open_read_only(&path, 2)
        .err()
        .expect("当前格式必须具有容器");
    assert_eq!(error.code(), "TDB_CORRUPTED_FILE");
    assert!(!std::path::Path::new(&container).exists());
    std::fs::write(container, original).unwrap();
    let mut db = Database::<f32>::open_with_config(
        &path,
        Config {
            dim: 2,
            access_mode: triviumdb::database::AccessMode::ReadOnly,
            load_text_index: true,
            ..Default::default()
        },
    )
    .unwrap();
    facts(&db);
    db.close().unwrap();
}

#[test]
fn 只改登记正文后旧容器不能回退同一数据库的检查点() {
    for mode in [StorageMode::Rom, StorageMode::Mmap] {
        let root = TestDir::new("container_checkpoint_identity");
        let path = root.file("data.tdb");
        seed(&path, mode);
        let old = std::fs::read(format!("{path}.idx")).unwrap();
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 2,
                storage_mode: mode,
                load_text_index: true,
                ..Default::default()
            },
        )
        .unwrap();
        db.rebuild_text_index(&[triviumdb::index::text::TextIndexDocument {
            id: 7,
            text: "replacement".into(),
            keywords: vec!["freshalias".into()],
        }])
        .unwrap();
        db.close().unwrap();
        let current = std::fs::read(format!("{path}.idx")).unwrap();
        assert_eq!(&old[256..272], &current[256..272]);
        assert_ne!(&old[272..288], &current[272..288]);
        std::fs::write(format!("{path}.idx"), &old).unwrap();
        let error = Database::<f32>::open_read_only(&path, 2)
            .err()
            .expect("旧容器必须拒绝");
        assert_eq!(error.code(), "TDB_CORRUPTED_FILE");
        assert_eq!(std::fs::read(format!("{path}.idx")).unwrap(), old);
        std::fs::write(format!("{path}.idx"), current).unwrap();
        let mut db = Database::<f32>::open_with_config(
            &path,
            Config {
                dim: 2,
                access_mode: triviumdb::database::AccessMode::ReadOnly,
                load_text_index: true,
                ..Default::default()
            },
        )
        .unwrap();
        for query in [
            "TEXT BM25 \"replacement\" TOP 10 AS h WITH h RETURN h",
            "TEXT AC \"freshalias\" TOP 10 AS h WITH h RETURN h",
        ] {
            assert_eq!(
                db.tql_values(query)
                    .unwrap()
                    .iter()
                    .map(|row| row["h"].as_node().unwrap().id)
                    .collect::<Vec<_>>(),
                [7]
            );
        }
        assert!(
            db.tql_values("TEXT BM25 \"alpha\" TOP 10 AS h WITH h RETURN h")
                .unwrap()
                .is_empty()
        );
        db.close().unwrap();
    }
}
