use serde_json::json;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use triviumdb::{DatabaseWriter, GenerationStore, TriviumError};

fn tmp_store(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "triviumdb_generation_{name}_{}",
        std::process::id()
    ))
}

fn publish(store: &GenerationStore, generation: &str, value: f32) {
    let path = store.prepare_generation(generation, "data.tdb").unwrap();
    let mut writer = DatabaseWriter::<f32>::open(&path.to_string_lossy(), 2).unwrap();
    writer
        .insert(&[value, 1.0 - value], json!({"generation": generation}))
        .unwrap();
    writer.publish_generation_manifest(generation).unwrap();
    drop(writer);
    std::fs::remove_file(format!("{}.wal", path.to_string_lossy())).unwrap();
    std::fs::remove_file(format!("{}.lock", path.to_string_lossy())).unwrap();
}

#[test]
fn generation跨进程租约辅助进程() {
    let Ok(root) = std::env::var("TRIVIUM_GENERATION_CHILD_ROOT") else {
        return;
    };
    let runtime = std::env::var("TRIVIUM_GENERATION_CHILD_RUNTIME").unwrap();
    let ready = std::env::var("TRIVIUM_GENERATION_CHILD_READY").unwrap();
    let store = GenerationStore::with_runtime_dir(root, runtime);
    let reader = store.open_current::<f32>(2).unwrap();
    std::fs::write(ready, reader.generation_id()).unwrap();
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

#[test]
fn current原子切换后新reader读新代旧reader保持旧代() {
    let root = tmp_store("switch");
    std::fs::remove_dir_all(&root).ok();
    let store = GenerationStore::new(&root);
    publish(&store, "generation-1", 1.0);
    publish(&store, "generation-2", 0.0);
    store.publish_current("generation-1", "data.tdb").unwrap();
    let old = store.open_current::<f32>(2).unwrap();
    store.publish_current("generation-2", "data.tdb").unwrap();
    let new = store.open_current::<f32>(2).unwrap();
    assert_eq!(old.generation_id(), "generation-1");
    assert_eq!(new.generation_id(), "generation-2");
    assert_eq!(old.get(1).unwrap().payload["generation"], "generation-1");
    assert_eq!(new.get(1).unwrap().payload["generation"], "generation-2");
    drop(old);
    drop(new);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn 活跃reader阻止旧代回收释放后可以回收() {
    let root = tmp_store("lease");
    std::fs::remove_dir_all(&root).ok();
    let store = GenerationStore::new(&root);
    publish(&store, "generation-1", 1.0);
    publish(&store, "generation-2", 0.0);
    store.publish_current("generation-1", "data.tdb").unwrap();
    let reader = store.open_current::<f32>(2).unwrap();
    store.publish_current("generation-2", "data.tdb").unwrap();
    assert!(matches!(
        store.reclaim_generation("generation-1"),
        Err(TriviumError::GenerationBusy { .. })
    ));
    drop(reader);
    store.reclaim_generation("generation-1").unwrap();
    assert!(!root.join("generation-1").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn 当前代永远不能被回收() {
    let root = tmp_store("current_busy");
    std::fs::remove_dir_all(&root).ok();
    let store = GenerationStore::new(&root);
    publish(&store, "generation-1", 1.0);
    store.publish_current("generation-1", "data.tdb").unwrap();
    assert!(matches!(
        store.reclaim_generation("generation-1"),
        Err(TriviumError::GenerationBusy { .. })
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current拒绝路径穿越和损坏指针() {
    let root = tmp_store("traversal");
    std::fs::remove_dir_all(&root).ok();
    let store = GenerationStore::new(&root);
    assert!(store.prepare_generation("../escape", "data.tdb").is_err());
    assert!(
        store
            .prepare_generation("generation-1", "../data.tdb")
            .is_err()
    );
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("current.json"),
        br#"{"format_version":1,"generation_id":"../escape","database_file":"data.tdb"}"#,
    )
    .unwrap();
    assert!(store.resolve_current().is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current损坏缺失和未知版本时回收全部fail_closed() {
    let root = tmp_store("reclaim_fail_closed");
    std::fs::remove_dir_all(&root).ok();
    let store = GenerationStore::new(&root);
    publish(&store, "generation-1", 1.0);
    let current_path = root.join("current.json");
    assert!(store.reclaim_generation("generation-1").is_err());
    assert!(root.join("generation-1").exists());
    std::fs::write(&current_path, b"{").unwrap();
    assert!(store.reclaim_generation("generation-1").is_err());
    assert!(root.join("generation-1").exists());
    std::fs::write(
        &current_path,
        br#"{"format_version":999,"generation_id":"generation-2","database_file":"data.tdb"}"#,
    )
    .unwrap();
    assert!(store.reclaim_generation("generation-1").is_err());
    assert!(root.join("generation-1").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn publish_current拒绝损坏manifest和generation_id错配且保留旧current() {
    let root = tmp_store("publish_validate");
    std::fs::remove_dir_all(&root).ok();
    let store = GenerationStore::new(&root);
    publish(&store, "generation-1", 1.0);
    publish(&store, "generation-2", 0.0);
    store.publish_current("generation-1", "data.tdb").unwrap();
    let before = std::fs::read(root.join("current.json")).unwrap();
    let manifest = root.join("generation-2/data.tdb.manifest.json");
    let original = std::fs::read(&manifest).unwrap();
    std::fs::write(&manifest, b"{").unwrap();
    assert!(store.publish_current("generation-2", "data.tdb").is_err());
    assert_eq!(std::fs::read(root.join("current.json")).unwrap(), before);
    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    value["generation_id"] = serde_json::Value::String("generation-other".into());
    std::fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    assert!(store.publish_current("generation-2", "data.tdb").is_err());
    assert_eq!(std::fs::read(root.join("current.json")).unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn generation_reader不在不可变目录创建租约文件() {
    let root = tmp_store("external_runtime");
    let runtime = tmp_store("external_runtime_locks");
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&runtime).ok();
    let builder = GenerationStore::new(&root);
    publish(&builder, "generation-1", 1.0);
    builder.publish_current("generation-1", "data.tdb").unwrap();
    let store = GenerationStore::with_runtime_dir(&root, &runtime);
    let before: Vec<_> = std::fs::read_dir(root.join("generation-1"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    let reader = store.open_current::<f32>(2).unwrap();
    let after: Vec<_> = std::fs::read_dir(root.join("generation-1"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(before, after);
    assert!(runtime.exists());
    drop(reader);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(runtime).unwrap();
}

#[test]
fn manifest_node_count错配时reader拒绝打开() {
    let root = tmp_store("node_count");
    std::fs::remove_dir_all(&root).ok();
    let store = GenerationStore::new(&root);
    publish(&store, "generation-1", 1.0);
    let manifest = root.join("generation-1/data.tdb.manifest.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    value["node_count"] = serde_json::Value::from(999);
    std::fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    assert!(store.publish_current("generation-1", "data.tdb").is_err());
    assert!(!root.join("current.json").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current残留临时文件不影响旧指针和后续发布() {
    let root = tmp_store("current_tmp");
    std::fs::remove_dir_all(&root).ok();
    let store = GenerationStore::new(&root);
    publish(&store, "generation-1", 1.0);
    publish(&store, "generation-2", 0.0);
    store.publish_current("generation-1", "data.tdb").unwrap();
    std::fs::write(root.join(".current.crashed.tmp"), b"{").unwrap();
    assert_eq!(
        store.resolve_current().unwrap().generation_id,
        "generation-1"
    );
    store.publish_current("generation-2", "data.tdb").unwrap();
    assert_eq!(
        store.resolve_current().unwrap().generation_id,
        "generation-2"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn 不可变generation目录只读时仍可通过外部runtime打开() {
    use std::os::unix::fs::PermissionsExt;
    let root = tmp_store("readonly_root");
    let runtime = tmp_store("readonly_runtime");
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&runtime).ok();
    let builder = GenerationStore::new(&root);
    publish(&builder, "generation-1", 1.0);
    builder.publish_current("generation-1", "data.tdb").unwrap();
    let generation = root.join("generation-1");
    let old_mode = std::fs::metadata(&generation).unwrap().permissions().mode();
    std::fs::set_permissions(&generation, std::fs::Permissions::from_mode(0o555)).unwrap();
    let store = GenerationStore::with_runtime_dir(&root, &runtime);
    let reader = store.open_current::<f32>(2).unwrap();
    assert_eq!(reader.node_count(), 1);
    drop(reader);
    std::fs::set_permissions(&generation, std::fs::Permissions::from_mode(old_mode)).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(runtime).unwrap();
}

#[test]
fn 并发切代时current始终是完整json() {
    let root = tmp_store("concurrent");
    std::fs::remove_dir_all(&root).ok();
    let store = Arc::new(GenerationStore::new(&root));
    publish(&store, "generation-1", 1.0);
    publish(&store, "generation-2", 0.0);
    store.publish_current("generation-1", "data.tdb").unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let writer_store = Arc::clone(&store);
    let writer_barrier = Arc::clone(&barrier);
    let writer = thread::spawn(move || {
        writer_barrier.wait();
        for index in 0..100 {
            let generation = if index % 2 == 0 {
                "generation-1"
            } else {
                "generation-2"
            };
            writer_store
                .publish_current(generation, "data.tdb")
                .unwrap();
        }
    });
    let reader_store = Arc::clone(&store);
    let reader_barrier = Arc::clone(&barrier);
    let reader = thread::spawn(move || {
        reader_barrier.wait();
        for _ in 0..200 {
            let current = reader_store.resolve_current().unwrap();
            assert!(
                current.generation_id == "generation-1" || current.generation_id == "generation-2"
            );
        }
    });
    barrier.wait();
    writer.join().unwrap();
    reader.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn 并发open与回收不会删除刚解析到的generation() {
    let root = tmp_store("open_reclaim");
    std::fs::remove_dir_all(&root).ok();
    let store = Arc::new(GenerationStore::new(&root));
    publish(&store, "generation-1", 1.0);
    publish(&store, "generation-2", 0.0);
    store.publish_current("generation-1", "data.tdb").unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let open_store = Arc::clone(&store);
    let open_barrier = Arc::clone(&barrier);
    let opener = thread::spawn(move || {
        open_barrier.wait();
        for _ in 0..50 {
            let reader = open_store.open_current::<f32>(2).unwrap();
            assert!(!reader.search(&[1.0, 0.0], 1, 0, 0.0).unwrap().is_empty());
        }
    });
    let reclaim_store = Arc::clone(&store);
    let reclaim_barrier = Arc::clone(&barrier);
    let reclaimer = thread::spawn(move || {
        reclaim_barrier.wait();
        for _ in 0..50 {
            let _ = reclaim_store.reclaim_generation("generation-2");
        }
    });
    barrier.wait();
    opener.join().unwrap();
    reclaimer.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn 真实跨进程reader阻止回收且异常退出后租约自动释放() {
    let root = tmp_store("process_lease");
    let runtime = tmp_store("process_lease_runtime");
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&runtime).ok();
    let store = GenerationStore::with_runtime_dir(&root, &runtime);
    publish(&store, "generation-1", 1.0);
    publish(&store, "generation-2", 0.0);
    store.publish_current("generation-1", "data.tdb").unwrap();
    let ready = root.join("child-ready");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("generation跨进程租约辅助进程")
        .arg("--nocapture")
        .env("TRIVIUM_GENERATION_CHILD_ROOT", &root)
        .env("TRIVIUM_GENERATION_CHILD_RUNTIME", &runtime)
        .env("TRIVIUM_GENERATION_CHILD_READY", &ready)
        .spawn()
        .unwrap();
    for _ in 0..200 {
        if ready.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    assert!(ready.exists(), "子进程未取得 generation 租约");
    store.publish_current("generation-2", "data.tdb").unwrap();
    assert!(matches!(
        store.reclaim_generation("generation-1"),
        Err(TriviumError::GenerationBusy { .. })
    ));
    child.kill().unwrap();
    child.wait().unwrap();
    store.reclaim_generation("generation-1").unwrap();
    assert!(!root.join("generation-1").exists());
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(runtime).unwrap();
}
