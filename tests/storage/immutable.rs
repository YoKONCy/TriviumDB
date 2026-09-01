use serde_json::json;
use std::path::Path;
use triviumdb::TriviumError;
use triviumdb::database::Database;

fn tmp_db(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("triviumdb_immutable_{name}_{}", std::process::id()))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for suffix in [
        "",
        ".vec",
        ".wal",
        ".lock",
        ".flush_ok",
        ".quiver",
        ".quiver.meta",
        ".text",
        ".text.meta",
        ".manifest.json",
        ".manifest.json.tmp",
    ] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

fn publish(path: &str) -> u64 {
    cleanup(path);
    let mut db = Database::<f32>::open(path, 2).unwrap();
    let id = db.insert(&[1.0, 0.0], json!({"generation": 1})).unwrap();
    let manifest = db.publish_generation_manifest("generation-1").unwrap();
    assert!(manifest.complete);
    drop(db);
    id
}

#[test]
fn 不可变模式无锁打开并保持可查询() {
    let path = tmp_db("open");
    let id = publish(&path);
    std::fs::remove_file(format!("{path}.lock")).unwrap();
    std::fs::remove_file(format!("{path}.wal")).unwrap();
    let db1 = Database::<f32>::open_immutable(&path, 2).unwrap();
    let db2 = Database::<f32>::open_immutable(&path, 2).unwrap();
    assert_eq!(db1.search(&[1.0, 0.0], 1, 0, 0.0).unwrap()[0].id, id);
    assert_eq!(db2.search(&[1.0, 0.0], 1, 0, 0.0).unwrap()[0].id, id);
    for query in [
        "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed degree seed AS scored WITH scored RETURN scored, graph_score(scored) AS score",
        "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed iterate seed EXPAND [*1..1] times 2 fixed AS reached WITH reached RETURN reached",
    ] {
        db1.tql_values(query).unwrap();
    }
    assert!(!Path::new(&format!("{path}.lock")).exists());
    assert!(!Path::new(&format!("{path}.wal")).exists());
    cleanup(&path);
}

#[test]
fn 不可变模式拒绝写入且关闭不产生文件() {
    let path = tmp_db("guards");
    publish(&path);
    std::fs::remove_file(format!("{path}.wal")).unwrap();
    let mut db = Database::<f32>::open_immutable(&path, 2).unwrap();
    assert!(matches!(
        db.insert(&[0.0, 1.0], json!({})),
        Err(TriviumError::ReadOnlyViolation { .. })
    ));
    db.close().unwrap();
    assert!(!Path::new(&format!("{path}.wal")).exists());
    cleanup(&path);
}

#[test]
fn 不可变模式拒绝缺失或未完成manifest() {
    let path = tmp_db("manifest_missing");
    publish(&path);
    let manifest_path = format!("{path}.manifest.json");
    let bytes = std::fs::read(&manifest_path).unwrap();
    std::fs::remove_file(&manifest_path).unwrap();
    assert!(matches!(
        Database::<f32>::open_immutable(&path, 2),
        Err(TriviumError::ImmutableArtifactInvalid { .. })
    ));
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["complete"] = serde_json::Value::Bool(false);
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    assert!(matches!(
        Database::<f32>::open_immutable(&path, 2),
        Err(TriviumError::ImmutableArtifactInvalid { .. })
    ));
    cleanup(&path);
}

#[test]
fn 不可变模式拒绝文件篡改和文件缺失() {
    let path = tmp_db("tamper");
    publish(&path);
    let vec_path = format!("{path}.vec");
    let original = std::fs::read(&vec_path).unwrap();
    let mut tampered = original.clone();
    tampered[0] ^= 0xff;
    std::fs::write(&vec_path, &tampered).unwrap();
    assert!(matches!(
        Database::<f32>::open_immutable(&path, 2),
        Err(TriviumError::ImmutableArtifactInvalid { .. })
    ));
    std::fs::write(&vec_path, &original).unwrap();
    std::fs::remove_file(&vec_path).unwrap();
    assert!(matches!(
        Database::<f32>::open_immutable(&path, 2),
        Err(TriviumError::ImmutableArtifactInvalid { .. })
    ));
    cleanup(&path);
}

#[test]
fn 不可变模式拒绝待恢复wal且不修改() {
    let path = tmp_db("wal");
    publish(&path);
    let wal_path = format!("{path}.wal");
    std::fs::write(&wal_path, b"pending-wal").unwrap();
    let before = std::fs::read(&wal_path).unwrap();
    assert!(matches!(
        Database::<f32>::open_immutable(&path, 2),
        Err(TriviumError::RecoveryRequired { .. })
    ));
    assert_eq!(std::fs::read(&wal_path).unwrap(), before);
    cleanup(&path);
}

#[test]
fn 不可变模式拒绝manifest外新增sidecar和非法后缀() {
    let path = tmp_db("extra_file");
    publish(&path);
    std::fs::write(format!("{path}.text"), b"unexpected").unwrap();
    assert!(matches!(
        Database::<f32>::open_immutable(&path, 2),
        Err(TriviumError::ImmutableArtifactInvalid { .. })
    ));
    std::fs::remove_file(format!("{path}.text")).unwrap();

    let manifest_path = format!("{path}.manifest.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    value["files"][0]["suffix"] = serde_json::Value::String("../../secret".into());
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    assert!(matches!(
        Database::<f32>::open_immutable(&path, 2),
        Err(TriviumError::ImmutableArtifactInvalid { .. })
    ));
    cleanup(&path);
}
