//! ReadOnly 模式的锁、恢复门禁与字节级零写契约测试。
//! 覆盖共享 Reader、Writer 互斥、缺失/旧 WAL、sidecar 策略及失败前后文件不变性。

use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use triviumdb::TriviumError;
use triviumdb::database::{AccessMode, Config, Database, MissingIndexPolicy, SearchConfig};

fn tmp_db(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("triviumdb_read_only_{name}_{}", std::process::id()))
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
    ] {
        std::fs::remove_file(format!("{path}{suffix}")).ok();
    }
}

#[test]
fn 只读跨进程辅助进程() {
    let Ok(path) = std::env::var("TRIVIUM_READ_ONLY_CHILD_PATH") else {
        return;
    };
    let ready = std::env::var("TRIVIUM_READ_ONLY_CHILD_READY").unwrap();
    let db = Database::<f32>::open_read_only(&path, 2).unwrap();
    std::fs::write(&ready, b"ready").unwrap();
    std::thread::sleep(Duration::from_secs(3));
    assert!(!db.search(&[1.0, 0.0], 1, 0, 0.0).unwrap().is_empty());
}

#[test]
fn 两个独立进程可以共享只读锁并查询() {
    let path = tmp_db("process_shared");
    create_clean_database(&path);
    let ready = format!("{path}.child-ready");
    std::fs::remove_file(&ready).ok();
    let executable = std::env::current_exe().unwrap();
    let mut command = if std::env::var_os("TRIVIUM_TEST_QEMU_AARCH64").is_some() {
        let mut command = Command::new("qemu-aarch64");
        command
            .arg("-L")
            .arg("/usr/aarch64-linux-gnu")
            .arg(executable);
        command
    } else {
        Command::new(executable)
    };
    let mut child = command
        .arg("--exact")
        .arg("只读跨进程辅助进程")
        .arg("--nocapture")
        .env("TRIVIUM_READ_ONLY_CHILD_PATH", &path)
        .env("TRIVIUM_READ_ONLY_CHILD_READY", &ready)
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if Path::new(&ready).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(Path::new(&ready).exists(), "辅助 Reader 未成功打开数据库");
    let reader = Database::<f32>::open_read_only(&path, 2).unwrap();
    assert!(!reader.search(&[1.0, 0.0], 1, 0, 0.0).unwrap().is_empty());
    assert!(child.wait().unwrap().success());
    std::fs::remove_file(ready).ok();
    cleanup(&path);
}

fn create_clean_database(path: &str) -> u64 {
    cleanup(path);
    let mut db = Database::<f32>::open(path, 2).unwrap();
    let id = db.insert(&[1.0, 0.0], json!({"kind": "seed"})).unwrap();
    db.flush().unwrap();
    drop(db);
    id
}

#[test]
fn 多个只读句柄共享锁并可并发查询() {
    let path = tmp_db("shared");
    let id = create_clean_database(&path);
    let reader1 = Database::<f32>::open_read_only(&path, 2).unwrap();
    let reader2 = Database::<f32>::open_read_only(&path, 2).unwrap();
    assert_eq!(reader1.search(&[1.0, 0.0], 1, 0, 0.0).unwrap()[0].id, id);
    assert_eq!(reader2.search(&[1.0, 0.0], 1, 0, 0.0).unwrap()[0].id, id);
    cleanup(&path);
}

#[test]
fn 只读与读写锁双向互斥() {
    let path = tmp_db("lock_matrix");
    create_clean_database(&path);
    let reader = Database::<f32>::open_read_only(&path, 2).unwrap();
    assert!(matches!(
        Database::<f32>::open(&path, 2),
        Err(TriviumError::DatabaseLocked(_))
    ));
    drop(reader);
    let writer = Database::<f32>::open(&path, 2).unwrap();
    assert!(matches!(
        Database::<f32>::open_read_only(&path, 2),
        Err(TriviumError::DatabaseLocked(_))
    ));
    drop(writer);
    cleanup(&path);
}

#[test]
fn 只读句柄拒绝写入事务索引和持久化操作() {
    let path = tmp_db("guards");
    let id = create_clean_database(&path);
    let mut db = Database::<f32>::open_read_only(&path, 2).unwrap();
    assert!(matches!(
        db.insert(&[0.0, 1.0], json!({})),
        Err(TriviumError::ReadOnlyViolation { .. })
    ));
    assert!(matches!(
        db.update_vector(id, &[0.0, 1.0]),
        Err(TriviumError::ReadOnlyViolation { .. })
    ));
    assert!(matches!(
        db.flush(),
        Err(TriviumError::ReadOnlyViolation { .. })
    ));
    assert!(matches!(
        db.compact(),
        Err(TriviumError::ReadOnlyViolation { .. })
    ));
    assert!(matches!(
        db.create_index("kind"),
        Err(TriviumError::ReadOnlyViolation { .. })
    ));
    assert!(matches!(
        db.set_sync_mode(triviumdb::storage::wal::SyncMode::Full),
        Err(TriviumError::ReadOnlyViolation { .. })
    ));
    assert!(matches!(
        db.set_auto_build_quiver(true),
        Err(TriviumError::ReadOnlyViolation { .. })
    ));
    let mut tx = db.begin_tx();
    tx.insert(&[0.0, 1.0], json!({}));
    assert!(matches!(
        tx.commit(),
        Err(TriviumError::ReadOnlyViolation { .. })
    ));
    assert_eq!(db.node_count(), 1);
    assert_eq!(db.search(&[1.0, 0.0], 1, 0, 0.0).unwrap()[0].id, id);
    db.close().unwrap();
    cleanup(&path);
}

#[test]
fn 只读打开遇到空旧版本_wal拒绝且字节不变() {
    let path = tmp_db("legacy_empty_wal");
    create_clean_database(&path);
    let wal_path = format!("{path}.wal");
    let legacy_header = [b'T', b'V', b'W', b'L', 2, 0];
    std::fs::write(&wal_path, legacy_header).unwrap();

    assert!(matches!(
        Database::<f32>::open_read_only(&path, 2),
        Err(TriviumError::RecoveryRequired { .. })
    ));
    assert_eq!(std::fs::read(&wal_path).unwrap(), legacy_header);
    cleanup(&path);
}

#[test]
fn 只读打开遇到待恢复_wal明确拒绝且不修改文件() {
    let path = tmp_db("wal");
    create_clean_database(&path);
    {
        let mut db = Database::<f32>::open(&path, 2).unwrap();
        db.insert(&[0.0, 1.0], json!({})).unwrap();
    }
    let wal_path = format!("{path}.wal");
    let before = std::fs::read(&wal_path).unwrap();
    assert!(matches!(
        Database::<f32>::open_read_only(&path, 2),
        Err(TriviumError::RecoveryRequired { .. })
    ));
    assert_eq!(std::fs::read(&wal_path).unwrap(), before);
    cleanup(&path);
}

#[test]
fn 只读加载损坏sidecar保持所有文件字节不变() {
    let path = tmp_db("sidecar");
    create_clean_database(&path);
    let quiver_path = format!("{path}.quiver");
    let meta_path = format!("{path}.quiver.meta");
    std::fs::write(&quiver_path, b"corrupt-quiver").unwrap();
    std::fs::write(&meta_path, b"corrupt-meta").unwrap();
    let before_quiver = std::fs::read(&quiver_path).unwrap();
    let before_meta = std::fs::read(&meta_path).unwrap();
    let config = Config {
        dim: 2,
        access_mode: AccessMode::ReadOnly,
        auto_build_quiver: false,
        ..Default::default()
    };
    let db = Database::<f32>::open_with_config(&path, config).unwrap();
    let search_config = SearchConfig {
        force_brute_force: true,
        ..Default::default()
    };
    assert!(
        !db.search_advanced(&[1.0, 0.0], &search_config)
            .unwrap()
            .is_empty()
    );
    drop(db);
    assert_eq!(std::fs::read(&quiver_path).unwrap(), before_quiver);
    assert_eq!(std::fs::read(&meta_path).unwrap(), before_meta);
    assert!(Path::new(&quiver_path).exists());
    cleanup(&path);
}

#[test]
fn 只读拒绝缺失一致性标记的跨文件代际且不修改文件() {
    let path = tmp_db("marker");
    create_clean_database(&path);
    let marker = format!("{path}.flush_ok");
    std::fs::remove_file(&marker).unwrap();
    let tdb_before = std::fs::read(&path).unwrap();
    let vec_before = std::fs::read(format!("{path}.vec")).unwrap();
    assert!(matches!(
        Database::<f32>::open_read_only(&path, 2),
        Err(TriviumError::CorruptedFile(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), tdb_before);
    assert_eq!(std::fs::read(format!("{path}.vec")).unwrap(), vec_before);
    assert!(!Path::new(&marker).exists());
    cleanup(&path);
}

#[test]
fn 只读和不可变reader拒绝进程本地fatigue() {
    let path = tmp_db("fatigue");
    create_clean_database(&path);
    let reader = Database::<f32>::open_read_only(&path, 2).unwrap();
    let config = SearchConfig {
        enable_refractory_fatigue: true,
        ..Default::default()
    };
    assert!(matches!(
        reader.search_advanced(&[1.0, 0.0], &config),
        Err(TriviumError::InvalidInput(_))
    ));
    drop(reader);
    cleanup(&path);
}

fn file_state(path: &str) -> Vec<(String, u64, std::time::SystemTime, u32)> {
    let mut state = Vec::new();
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
    ] {
        let file = format!("{path}{suffix}");
        if let Ok(bytes) = std::fs::read(&file) {
            let metadata = std::fs::metadata(&file).unwrap();
            state.push((
                suffix.to_string(),
                metadata.len(),
                metadata.modified().unwrap(),
                crc32fast::hash(&bytes),
            ));
        }
    }
    state
}

#[test]
fn 只读查询close和drop保持完整文件状态不变() {
    let path = tmp_db("all_file_state");
    create_clean_database(&path);
    let before = file_state(&path);
    {
        let mut reader = Database::<f32>::open_read_only(&path, 2).unwrap();
        assert!(!reader.search(&[1.0, 0.0], 1, 0, 0.0).unwrap().is_empty());
        let corpus = [
            "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed EXPAND seed [*1..1] AS reached WITH reached RETURN reached",
            "FIND {kind: 'seed'} AS seed WITH seed degree seed AS scored WITH scored RETURN scored, graph_score(scored) AS score",
            "SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed iterate seed EXPAND [*1..1] times 2 fixed AS reached WITH reached RETURN reached",
        ];
        for query in corpus {
            reader.tql_values(query).unwrap();
        }
        reader.close().unwrap();
    }
    assert_eq!(file_state(&path), before);
    cleanup(&path);
}

#[test]
fn reader严格索引策略拒绝缺失quiver且fallback可查询() {
    let path = tmp_db("missing_index_policy");
    create_clean_database(&path);
    let strict = Config {
        dim: 2,
        access_mode: AccessMode::ReadOnly,
        missing_index_policy: MissingIndexPolicy::Error,
        ..Default::default()
    };
    assert!(matches!(
        Database::<f32>::open_with_config(&path, strict),
        Err(TriviumError::ImmutableArtifactInvalid { .. })
    ));
    let fallback = Config {
        dim: 2,
        access_mode: AccessMode::ReadOnly,
        missing_index_policy: MissingIndexPolicy::Fallback,
        ..Default::default()
    };
    let db = Database::<f32>::open_with_config(&path, fallback).unwrap();
    assert!(!db.search(&[1.0, 0.0], 1, 0, 0.0).unwrap().is_empty());
    cleanup(&path);
}

#[test]
fn reader严格索引策略拒绝损坏quiver且不删除() {
    let path = tmp_db("strict_corrupt_quiver");
    create_clean_database(&path);
    let quiver = format!("{path}.quiver");
    let meta = format!("{path}.quiver.meta");
    std::fs::write(&quiver, b"broken").unwrap();
    std::fs::write(&meta, b"broken").unwrap();
    let before = (
        std::fs::read(&quiver).unwrap(),
        std::fs::read(&meta).unwrap(),
    );
    let strict = Config {
        dim: 2,
        access_mode: AccessMode::ReadOnly,
        missing_index_policy: MissingIndexPolicy::Error,
        ..Default::default()
    };
    assert!(Database::<f32>::open_with_config(&path, strict).is_err());
    assert_eq!(std::fs::read(&quiver).unwrap(), before.0);
    assert_eq!(std::fs::read(&meta).unwrap(), before.1);
    cleanup(&path);
}
