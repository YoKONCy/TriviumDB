#![cfg(feature = "test-hooks")]
//! 用真实文件替换和路径屏障验证打开入口的租约顺序。

#[path = "../support/mod.rs"]
mod support;

use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use support::TestDir;
use triviumdb::database::{AccessMode, Config};
use triviumdb::test_hooks::{ConcurrencyPoint, pause_at_path};
use triviumdb::{Database, TriviumError};

fn seed(path: &str) -> Database<f32> {
    let mut database = Database::open(path, 3).unwrap();
    database
        .insert_with_id(1, &[1., 2., 3.], json!({"state":"before"}))
        .unwrap();
    database.flush().unwrap();
    database
}

fn snapshot(root: &Path) -> BTreeMap<std::ffi::OsString, Option<Vec<u8>>> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            // Windows 字节范围锁禁止读取协调文件；仍核对成员，其余制品逐字节比较。
            let bytes = if entry.path().extension() == Some(std::ffi::OsStr::new("lock")) {
                assert_eq!(
                    entry.metadata().unwrap().len(),
                    0,
                    "协调锁不得写入数据库内容"
                );
                None
            } else {
                Some(std::fs::read(entry.path()).unwrap())
            };
            (entry.file_name(), bytes)
        })
        .collect()
}

fn open(
    path: String,
    mode: AccessMode,
) -> std::thread::JoinHandle<triviumdb::Result<Database<f32>>> {
    std::thread::spawn(move || {
        Database::open_with_config(
            &path,
            Config {
                dim: 3,
                access_mode: mode,
                ..Default::default()
            },
        )
    })
}

#[test]
fn 路径解析与发布交错时保留真实租约冲突且不改写文件() {
    for mode in [AccessMode::ReadWrite, AccessMode::ReadOnly] {
        let root = TestDir::new("open_path_lease");
        let path = root.file("sample.tdb");
        let displaced = root.file("sample.displaced");
        let mut owner = seed(&path);
        let before = snapshot(&root.0);
        let metadata = pause_at_path(ConcurrencyPoint::BeforeDatabasePathMetadata, &path);
        let failed = pause_at_path(ConcurrencyPoint::DatabasePathResolutionFailed, &path);
        let opening = open(path.clone(), mode);
        metadata.wait_until_arrived();
        std::fs::rename(&path, &displaced).unwrap();
        metadata.release();
        failed.wait_until_arrived();
        std::fs::rename(&displaced, &path).unwrap();
        failed.release();
        let result = opening.join().unwrap();
        assert!(
            matches!(result, Err(TriviumError::DatabaseLocked(_))),
            "租约仍被原实例独占，打开入口必须报告锁冲突"
        );
        assert_eq!(snapshot(&root.0), before);
        assert_eq!(owner.get(1).unwrap().unwrap().vector, vec![1., 2., 3.]);
        assert_eq!(
            owner.get_payload(1).unwrap(),
            Some(json!({"state":"before"}))
        );
        owner.close().unwrap();
        let mut reopened = Database::<f32>::open(&path, 3).unwrap();
        assert_eq!(
            reopened.get_payload(1).unwrap(),
            Some(json!({"state":"before"}))
        );
        reopened.close().unwrap();
    }
}

#[test]
fn 路径解析失败后租约已释放时持锁重读完整已提交事实() {
    for mode in [AccessMode::ReadWrite, AccessMode::ReadOnly] {
        let root = TestDir::new("open_path_retry");
        let path = root.file("sample.tdb");
        let displaced = root.file("sample.displaced");
        let mut owner = seed(&path);
        owner
            .update_payload(1, json!({"state":"confirmed"}))
            .unwrap();
        let metadata = pause_at_path(ConcurrencyPoint::BeforeDatabasePathMetadata, &path);
        let failed = pause_at_path(ConcurrencyPoint::DatabasePathResolutionFailed, &path);
        let opening = open(path.clone(), mode);
        metadata.wait_until_arrived();
        std::fs::rename(&path, &displaced).unwrap();
        metadata.release();
        failed.wait_until_arrived();
        std::fs::rename(&displaced, &path).unwrap();
        owner.close().unwrap();
        failed.release();
        let mut reopened = opening.join().unwrap().unwrap();
        assert_eq!(reopened.node_count().unwrap(), 1);
        let node = reopened.get(1).unwrap().unwrap();
        assert_eq!(node.vector, vec![1., 2., 3.]);
        assert_eq!(node.payload, json!({"state":"confirmed"}));
        assert!(
            matches!(
                Database::<f32>::open(&path, 3),
                Err(TriviumError::DatabaseLocked(_))
            ),
            "重读成功后必须继续持有新实例的租约"
        );
        reopened.close().unwrap();
        let mut persisted = Database::<f32>::open(&path, 3).unwrap();
        assert_eq!(
            persisted.get_payload(1).unwrap(),
            Some(json!({"state":"confirmed"}))
        );
        persisted.close().unwrap();
    }
}

#[test]
fn 路径持续缺失且租约空闲时保留原始io错误并归还探测租约() {
    for mode in [
        AccessMode::ReadWrite,
        AccessMode::ReadOnly,
        AccessMode::Immutable,
    ] {
        let root = TestDir::new("open_path_failure");
        let path = root.file("sample.tdb");
        let displaced = root.file("sample.displaced");
        seed(&path).close().unwrap();
        let before = snapshot(&root.0);
        let metadata = pause_at_path(ConcurrencyPoint::BeforeDatabasePathMetadata, &path);
        let opening = open(path.clone(), mode);
        metadata.wait_until_arrived();
        std::fs::rename(&path, &displaced).unwrap();
        metadata.release();
        let result = opening.join().unwrap();
        std::fs::rename(&displaced, &path).unwrap();
        assert!(
            matches!(result, Err(TriviumError::Io(error)) if error.kind()==std::io::ErrorKind::NotFound)
        );
        assert_eq!(snapshot(&root.0), before);
        let mut reopened = Database::<f32>::open(&path, 3).unwrap();
        assert_eq!(
            reopened.get_payload(1).unwrap(),
            Some(json!({"state":"before"}))
        );
        reopened.close().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn 规范路径取得后仍须验证调用路径未失效() {
    let root = TestDir::new("open_path_identity");
    let path = root.file("sample.tdb");
    let entry = root.0.join("入口");
    let displaced = root.0.join("已移除入口");
    std::fs::create_dir(&entry).unwrap();
    let requested = root.file("入口/../sample.tdb");
    let mut owner = seed(&path);
    let before = std::fs::read(&path).unwrap();
    let identity = pause_at_path(ConcurrencyPoint::BeforeDatabasePathIdentity, &requested);
    let opening = open(requested, AccessMode::ReadWrite);
    identity.wait_until_arrived();
    std::fs::rename(&entry, &displaced).unwrap();
    identity.release();
    let result = opening.join().unwrap();
    std::fs::rename(&displaced, &entry).unwrap();
    assert!(
        matches!(result, Err(TriviumError::Io(error)) if error.kind()==std::io::ErrorKind::NotFound)
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        owner.get_payload(1).unwrap(),
        Some(json!({"state":"before"}))
    );
    owner.close().unwrap();
    let mut reopened = Database::<f32>::open(&path, 3).unwrap();
    assert_eq!(reopened.get(1).unwrap().unwrap().vector, vec![1., 2., 3.]);
    assert_eq!(
        reopened.get_payload(1).unwrap(),
        Some(json!({"state":"before"}))
    );
    reopened.close().unwrap();
}

#[cfg(unix)]
#[test]
fn 符号链接解析后不能把别名的空闲租约用于目标数据库() {
    use fs2::FileExt;
    let root = TestDir::new("open_path_alias");
    let path = root.file("target.tdb");
    let alias = root.file("alias.tdb");
    let displaced = root.file("alias.displaced");
    let alias_lock = root.file("alias.tdb.lock");
    let mut owner = seed(&path);
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    std::fs::write(&alias_lock, []).unwrap();
    let metadata = pause_at_path(ConcurrencyPoint::BeforeDatabasePathMetadata, &alias);
    let failed = pause_at_path(ConcurrencyPoint::DatabasePathResolutionFailed, &alias);
    let opening = open(alias.clone(), AccessMode::ReadWrite);
    metadata.wait_until_arrived();
    std::fs::rename(&alias, &displaced).unwrap();
    metadata.release();
    failed.wait_until_arrived();
    std::fs::rename(&displaced, &alias).unwrap();
    failed.release();
    assert!(
        matches!(opening.join().unwrap(), Err(TriviumError::Io(error)) if error.kind()==std::io::ErrorKind::NotFound)
    );
    let probe = std::fs::File::open(&alias_lock).unwrap();
    FileExt::try_lock_exclusive(&probe).unwrap();
    drop(probe);
    assert!(matches!(
        Database::<f32>::open(&path, 3),
        Err(TriviumError::DatabaseLocked(_))
    ));
    assert_eq!(
        owner.get_payload(1).unwrap(),
        Some(json!({"state":"before"}))
    );
    owner.close().unwrap();
    let mut reopened = Database::<f32>::open(&alias, 3).unwrap();
    assert_eq!(reopened.get(1).unwrap().unwrap().vector, vec![1., 2., 3.]);
    assert_eq!(
        reopened.get_payload(1).unwrap(),
        Some(json!({"state":"before"}))
    );
    reopened.close().unwrap();
}
