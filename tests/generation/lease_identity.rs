use super::{publish, support::TestDir};
use triviumdb::{GenerationStore, TriviumError};

fn artifacts(path: &std::path::Path) -> std::collections::BTreeMap<std::ffi::OsString, Vec<u8>> {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect()
}

#[test]
fn 根目录创建前后构造的默认store共享reader租约() {
    let parent = TestDir::new("generation_identity");
    let root = std::path::PathBuf::from(parent.file("代际 数据"));
    let before = GenerationStore::new(&root);
    publish(&before, "old", 1.);
    before.publish_current("old", "data.tdb").unwrap();
    let reader = before.open_current::<f32>(2).unwrap();
    let old_files = artifacts(&root.join("old"));
    let after = GenerationStore::new(root.canonicalize().unwrap());
    publish(&after, "new", 0.);
    after.publish_current("new", "data.tdb").unwrap();
    assert!(matches!(
        after.reclaim_generation("old"),
        Err(TriviumError::GenerationBusy { .. })
    ));
    assert!(root.join("old/data.tdb").exists());
    assert_eq!(artifacts(&root.join("old")), old_files);
    assert_eq!(reader.get(1).unwrap().unwrap().payload["generation"], "old");
    drop(reader);
    after.reclaim_generation("old").unwrap();
    assert!(!root.join("old").exists());
}

#[test]
fn 同一物理根目录的路径别名和显式运行目录共享租约() {
    for custom in [false, true] {
        let parent = TestDir::new("generation_alias");
        let root = parent.0.join("store");
        let runtime = parent.0.join("runtime");
        let first = if custom {
            GenerationStore::with_runtime_dir(&root, &runtime)
        } else {
            GenerationStore::new(&root)
        };
        publish(&first, "old", 1.);
        first.publish_current("old", "data.tdb").unwrap();
        let reader = first.open_current::<f32>(2).unwrap();
        let old_files = artifacts(&root.join("old"));
        let mut aliases = vec![root.join("."), root.canonicalize().unwrap()];
        #[cfg(windows)]
        aliases.push(std::path::PathBuf::from(
            root.to_str().unwrap().to_uppercase(),
        ));
        #[cfg(unix)]
        {
            let link = parent.0.join("symlink");
            std::os::unix::fs::symlink(&root, &link).unwrap();
            aliases.push(link);
        }
        for (index, alias) in aliases.drain(..).enumerate() {
            let second = if custom {
                GenerationStore::with_runtime_dir(alias, &runtime)
            } else {
                GenerationStore::new(alias)
            };
            let generation = format!("new-{index}");
            publish(&second, &generation, 0.);
            second.publish_current(&generation, "data.tdb").unwrap();
            assert!(matches!(
                second.reclaim_generation("old"),
                Err(TriviumError::GenerationBusy { .. })
            ));
            assert_eq!(artifacts(&root.join("old")), old_files);
        }
        assert_eq!(reader.get(1).unwrap().unwrap().payload["generation"], "old");
    }
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn 默认租约跨进程和构造后工作目录变化仍保护旧代() {
    let parent = TestDir::new("generation_default_process");
    let root = parent.0.join("store");
    let elsewhere = parent.0.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let before = GenerationStore::new(&root);
    publish(&before, "generation-1", 1.);
    before.publish_current("generation-1", "data.tdb").unwrap();
    let ready = parent.file("ready");
    let mut child = Child(
        super::generation_child_command()
            .args(["--exact", "generation跨进程租约辅助进程", "--nocapture"])
            .current_dir(&parent.0)
            .env("TRIVIUM_GENERATION_CHILD_ROOT", "store")
            .env_remove("TRIVIUM_GENERATION_CHILD_RUNTIME")
            .env("TRIVIUM_GENERATION_CHILD_CWD", &elsewhere)
            .env("TRIVIUM_GENERATION_CHILD_READY", &ready)
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !std::path::Path::new(&ready).exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "子进程在取得租约前退出"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "子进程未在期限内取得租约"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(std::fs::read(&ready).unwrap(), b"generation-1");
    let old_files = artifacts(&root.join("generation-1"));
    publish(&before, "generation-2", 0.);
    before.publish_current("generation-2", "data.tdb").unwrap();
    assert!(matches!(
        before.reclaim_generation("generation-1"),
        Err(TriviumError::GenerationBusy { .. })
    ));
    assert_eq!(artifacts(&root.join("generation-1")), old_files);
    drop(child);
    before.reclaim_generation("generation-1").unwrap();
    assert!(!root.join("generation-1").exists());
}
