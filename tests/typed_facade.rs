use serde_json::json;
use triviumdb::database::{AccessMode, Config, DatabaseReader, DatabaseWriter, SearchConfig};

fn tmp_db(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("triviumdb_typed_{name}_{}", std::process::id()))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for suffix in ["", ".vec", ".wal", ".lock", ".flush_ok", ".manifest.json"] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

#[test]
fn writer门面保留完整写能力并可发布generation() {
    let path = tmp_db("writer");
    cleanup(&path);
    let mut writer = DatabaseWriter::<f32>::open(&path, 2).unwrap();
    let id = writer
        .insert(&[1.0, 0.0], json!({"kind": "typed"}))
        .unwrap();
    assert_eq!(writer.get(id).unwrap().payload["kind"], "typed");
    let manifest = writer
        .publish_generation_manifest("typed-generation")
        .unwrap();
    assert_eq!(manifest.generation_id, "typed-generation");
    cleanup(&path);
}

#[test]
fn reader门面支持共享只读和完整查询能力() {
    let path = tmp_db("reader");
    cleanup(&path);
    let id = {
        let mut writer = DatabaseWriter::<f32>::open(&path, 2).unwrap();
        let id = writer.insert(&[1.0, 0.0], json!({})).unwrap();
        writer.flush().unwrap();
        id
    };
    let reader1 = DatabaseReader::<f32>::open_read_only(&path, 2).unwrap();
    let reader2 = DatabaseReader::<f32>::open_read_only(&path, 2).unwrap();
    assert_eq!(reader1.search(&[1.0, 0.0], 1, 0, 0.0).unwrap()[0].id, id);
    assert_eq!(reader2.search_exact(&[1.0, 0.0], 1).unwrap()[0].id, id);
    assert_eq!(reader1.node_count(), 1);
    assert!(reader1.contains(id));
    cleanup(&path);
}

#[test]
fn reader门面支持immutable并验证manifest() {
    let path = tmp_db("immutable");
    cleanup(&path);
    {
        let mut writer = DatabaseWriter::<f32>::open(&path, 2).unwrap();
        writer.insert(&[1.0, 0.0], json!({})).unwrap();
        writer
            .publish_generation_manifest("typed-immutable")
            .unwrap();
    }
    std::fs::remove_file(format!("{path}.lock")).unwrap();
    std::fs::remove_file(format!("{path}.wal")).unwrap();
    let reader = DatabaseReader::<f32>::open_immutable(&path, 2).unwrap();
    let config = SearchConfig {
        force_brute_force: true,
        ..Default::default()
    };
    assert_eq!(
        reader.search_advanced(&[1.0, 0.0], &config).unwrap().len(),
        1
    );
    cleanup(&path);
}

#[test]
fn 类型化门面拒绝错误访问模式配置() {
    let path = tmp_db("mode");
    cleanup(&path);
    let reader_result = DatabaseReader::<f32>::open_with_config(
        &path,
        Config {
            dim: 2,
            access_mode: AccessMode::ReadWrite,
            ..Default::default()
        },
    );
    assert!(reader_result.is_err());
    let writer_result = DatabaseWriter::<f32>::open_with_config(
        &path,
        Config {
            dim: 2,
            access_mode: AccessMode::ReadOnly,
            ..Default::default()
        },
    );
    assert!(writer_result.is_err());
    assert!(!std::path::Path::new(&path).exists());
    cleanup(&path);
}
