//! 使用上一批实际交付物产生的独立 v10/v3 样本验证兼容及截断恢复。
use super::support::TestDir;
use serde_json::json;
use std::collections::BTreeMap;
use triviumdb::database::{Config, StorageMode};
use triviumdb::{Database, TriviumError, VectorType};

fn files(path: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_str().unwrap().to_owned(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

fn facts<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(db: &Database<T>) {
    assert_eq!(db.all_node_ids().unwrap(), vec![7, 8]);
    for (id, vector, kind) in [(7, vec![1., 0.], "历史甲"), (8, vec![0., 1.], "历史乙")] {
        let node = db.get(id).unwrap().unwrap();
        assert_eq!(
            node.vector
                .into_iter()
                .map(|value| value.to_f32())
                .collect::<Vec<_>>(),
            vector
        );
        assert_eq!(node.payload, json!({"kind":kind}));
    }
    let edges = db.get_edges(7).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target_id, 8);
    assert_eq!(edges[0].label, "rel");
    assert_eq!(edges[0].weight, 0.5);
    assert_eq!(edges[0].metadata, json!({"source":"历史"}));
    assert!(db.get_edges(8).unwrap().is_empty());
}

fn matrix<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(dtype: &str) {
    for case in ["checkpoint", "wal", "tail"] {
        let root = TestDir::new(&format!("history-{dtype}-{case}"));
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/v0900_format10")
            .join(dtype)
            .join(if case == "tail" { "wal" } else { case });
        for (name, bytes) in files(&fixture) {
            std::fs::write(root.0.join(name), bytes).unwrap();
        }
        let path = root.file("sample.tdb");
        std::fs::write(format!("{path}.lock"), []).unwrap();
        if case == "tail" {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(format!("{path}.wal"))
                .unwrap()
                .write_all(&[4, 0, 0])
                .unwrap();
        }
        let before = files(&root.0);
        if case == "checkpoint" {
            let mut readonly = Database::<T>::open_read_only(&path, 2).unwrap();
            facts(&readonly);
            readonly.close().unwrap();
        } else {
            assert!(matches!(
                Database::<T>::open_read_only(&path, 2),
                Err(TriviumError::RecoveryRequired { .. })
            ));
        }
        assert_eq!(files(&root.0), before);
        let mut database = Database::<T>::open_with_config(
            &path,
            Config {
                dim: 2,
                storage_mode: StorageMode::Rom,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
        .unwrap();
        facts(&database);
        database.close().unwrap();
        assert_eq!(&std::fs::read(&path).unwrap()[4..6], &11u16.to_le_bytes());
        assert_eq!(
            &std::fs::read(format!("{path}.wal")).unwrap()[4..6],
            &4u16.to_le_bytes()
        );
        let before = files(&root.0);
        let mut reopened = Database::<T>::open_read_only(&path, 2).unwrap();
        facts(&reopened);
        reopened.close().unwrap();
        assert_eq!(files(&root.0), before);
    }
}

#[test]
fn 历史检查点和完整WAL及截断尾帧在三种dtype保持全部事实() {
    matrix::<f32>("f32");
    matrix::<half::f16>("f16");
    matrix::<u64>("u64");
}
