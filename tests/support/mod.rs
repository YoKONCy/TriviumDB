use std::sync::atomic::{AtomicU64, Ordering};

pub struct TestDir(pub std::path::PathBuf);

impl TestDir {
    pub fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "tdb_{name}_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub fn file(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }

    /// 在隔离目录复制同一检查点，保留主库、索引和日志之间的身份关系。
    #[allow(dead_code)]
    pub fn copy_checkpoint(&self, source: &str, target: &str) {
        let base = std::path::Path::new(source)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        for entry in std::fs::read_dir(&self.0).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let Some(suffix) = name.to_str().unwrap().strip_prefix(base) else {
                continue;
            };
            if (suffix.is_empty() || suffix.starts_with('.')) && suffix != ".lock" {
                std::fs::copy(entry.path(), format!("{target}{suffix}")).unwrap();
            }
        }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
