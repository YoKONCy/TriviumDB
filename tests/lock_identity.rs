#![allow(non_snake_case)]

use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;
use triviumdb::{Database, TriviumError};

fn tmp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "triviumdb_lock_identity_{name}_{}",
        std::process::id()
    ))
}

fn child_command() -> Command {
    let executable = std::env::current_exe().unwrap();
    if std::env::var_os("TRIVIUM_TEST_QEMU_AARCH64").is_some() {
        let mut command = Command::new("qemu-aarch64");
        command
            .arg("-L")
            .arg("/usr/aarch64-linux-gnu")
            .arg(executable);
        command
    } else {
        Command::new(executable)
    }
}

fn database_files(path: &Path) -> Vec<(String, Vec<u8>)> {
    ["", ".vec", ".wal", ".flush_ok", ".quiver", ".quiver.meta"]
        .into_iter()
        .filter_map(|suffix| {
            let file = PathBuf::from(format!("{}{suffix}", path.display()));
            file.exists()
                .then(|| (suffix.to_string(), std::fs::read(file).unwrap()))
        })
        .collect()
}

#[test]
fn 双写跨进程辅助进程() {
    let Ok(path) = std::env::var("TRIVIUM_WRITER_CHILD_PATH") else {
        return;
    };
    let result_path = std::env::var("TRIVIUM_WRITER_CHILD_RESULT").unwrap();
    let result = match Database::<f32>::open(&path, 2) {
        Err(TriviumError::DatabaseLocked(_)) => "locked",
        Ok(_) => "opened",
        Err(_) => "other-error",
    };
    std::fs::write(result_path, result).unwrap();
}

#[test]
fn 同一路径跨进程第二个writer被拒绝且文件字节不变() {
    let root = tmp_root("cross_process");
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("data.tdb");
    let mut writer = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    writer.insert(&[1.0, 0.0], json!({"owner": 1})).unwrap();
    writer.flush().unwrap();
    let before = database_files(&path);
    let result_path = root.join("child-result");
    let status = child_command()
        .arg("--exact")
        .arg("双写跨进程辅助进程")
        .arg("--nocapture")
        .env("TRIVIUM_WRITER_CHILD_PATH", &path)
        .env("TRIVIUM_WRITER_CHILD_RESULT", &result_path)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(std::fs::read_to_string(result_path).unwrap(), "locked");
    assert_eq!(database_files(&path), before);
    drop(writer);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn 相对路径绝对路径与父目录别名共享同一writer锁() {
    let root = tmp_root("aliases");
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(root.join("nested")).unwrap();
    let absolute = root.join("data.tdb");
    let writer = Database::<f32>::open(absolute.to_str().unwrap(), 2).unwrap();
    let parent_alias = root.join("nested").join("..").join("data.tdb");
    assert!(matches!(
        Database::<f32>::open(parent_alias.to_str().unwrap(), 2),
        Err(TriviumError::DatabaseLocked(_))
    ));
    let current = std::env::current_dir().unwrap();
    if let Ok(relative) = absolute.strip_prefix(&current) {
        assert!(matches!(
            Database::<f32>::open(relative.to_str().unwrap(), 2),
            Err(TriviumError::DatabaseLocked(_))
        ));
    }
    drop(writer);
    std::fs::remove_dir_all(root).ok();
}

#[cfg(unix)]
#[test]
fn 符号链接别名不能绕过writer锁() {
    use std::os::unix::fs::symlink;

    let root = tmp_root("symlink");
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("data.tdb");
    {
        let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
        db.flush().unwrap();
    }
    let alias = root.join("alias.tdb");
    symlink(&path, &alias).unwrap();
    let writer = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    assert!(matches!(
        Database::<f32>::open(alias.to_str().unwrap(), 2),
        Err(TriviumError::DatabaseLocked(_))
    ));
    drop(writer);
    std::fs::remove_dir_all(root).ok();
}

#[cfg(windows)]
#[test]
fn 符号链接别名在权限允许时不能绕过writer锁() {
    use std::os::windows::fs::symlink_file;

    let root = tmp_root("symlink");
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("data.tdb");
    {
        let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
        db.flush().unwrap();
    }
    let alias = root.join("alias.tdb");
    if symlink_file(&path, &alias).is_err() {
        std::fs::remove_dir_all(root).ok();
        return;
    }
    let writer = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
    assert!(matches!(
        Database::<f32>::open(alias.to_str().unwrap(), 2),
        Err(TriviumError::DatabaseLocked(_))
    ));
    drop(writer);
    std::fs::remove_dir_all(root).ok();
}

#[cfg(unix)]
#[test]
fn 硬链接主文件被安全拒绝以防独立sidecar锁身份() {
    let root = tmp_root("hard_link");
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("data.tdb");
    {
        let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
        db.flush().unwrap();
    }
    let alias = root.join("alias.tdb");
    std::fs::hard_link(&path, &alias).unwrap();
    assert!(matches!(
        Database::<f32>::open(path.to_str().unwrap(), 2),
        Err(TriviumError::InvalidInput(_))
    ));
    assert!(matches!(
        Database::<f32>::open(alias.to_str().unwrap(), 2),
        Err(TriviumError::InvalidInput(_))
    ));
    std::fs::remove_dir_all(root).ok();
}
