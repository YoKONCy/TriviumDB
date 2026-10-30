//! 维度迁移在独占暂存库中构建，通过目标租约和发布日志安装全新文件组。

use super::{Config, Database, read_or_recover};
use crate::{Result, TriviumError, VectorType};
use fs2::FileExt;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct Destination {
    target: String,
    directory: PathBuf,
    lease: Option<std::fs::File>,
}

fn ensure_empty(target: &str) -> Result<()> {
    let path = Path::new(target);
    let parent = path.parent().ok_or_else(|| {
        TriviumError::InvalidInput(
            "迁移目标缺少父目录 (Migration destination has no parent)".into(),
        )
    })?;
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            TriviumError::InvalidInput(
                "迁移目标缺少文件名 (Migration destination has no file name)".into(),
            )
        })?;
    let occupied = || {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "迁移目标文件组已经存在 (Migration destination file group already exists): {target}"
            ),
        )
    };
    // 由文件系统解析主文件别名；悬空链接和目录也属于已有目标。
    match std::fs::symlink_metadata(path) {
        Ok(_) => return Err(occupied().into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let lock = parent.join(format!("{leaf}.lock"));
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let mut remaining = Path::new(&name);
        let mut suffix = OsString::new();
        // 不转换大小写或有损解码；逐个点边界探测同名前缀，让卷及目录决定别名规则。
        while let (Some(stem), Some(extension)) = (remaining.file_stem(), remaining.extension()) {
            let mut next = OsString::from(".");
            next.push(extension);
            next.push(&suffix);
            suffix = next;
            remaining = Path::new(stem);
            let mut candidate = OsString::from(leaf);
            candidate.push(&suffix);
            let candidate = parent.join(candidate);
            let metadata = match std::fs::symlink_metadata(&candidate) {
                Ok(metadata) => metadata,
                // 无关长文件名的后缀可能使拼接路径超限；它不可能是有效的目标制品。
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidFilename
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            // 仅真实协调锁可复用；大小写不同的独立文件、符号链接及目录均不得豁免。
            if suffix
                .to_str()
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".lock"))
                && metadata.is_file()
            {
                match std::fs::symlink_metadata(&lock) {
                    Ok(lock_metadata)
                        if lock_metadata.is_file()
                            && same_file::is_same_file(&candidate, &lock)? =>
                    {
                        continue;
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            return Err(occupied().into());
        }
    }
    Ok(())
}

impl Destination {
    fn new(path: &str) -> Result<Self> {
        let requested = Path::new(path);
        let leaf = requested.file_name().and_then(|name| name.to_str()).ok_or_else(|| {
            TriviumError::InvalidInput("迁移目标必须有有效 UTF-8 文件名 (Migration destination requires a UTF-8 file name)".into())
        })?;
        let parent = requested
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let parent = parent.canonicalize()?;
        let target = parent
            .join(leaf)
            .to_str()
            .ok_or_else(|| {
                TriviumError::InvalidInput(
                    "迁移目标路径不是 UTF-8 (Migration destination path is not UTF-8)".into(),
                )
            })?
            .to_owned();
        ensure_empty(&target)?;
        let lease = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(format!("{target}.lock"))?;
        lease
            .try_lock_exclusive()
            .map_err(|_| TriviumError::DatabaseLocked(target.clone()))?;
        ensure_empty(&target)?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut nonce = NEXT.load(Ordering::Relaxed);
        loop {
            let next = nonce.checked_add(1).ok_or_else(|| {
                TriviumError::InvalidInput(
                    "迁移暂存标识已耗尽 (Migration staging identifiers exhausted)".into(),
                )
            })?;
            match NEXT.compare_exchange_weak(nonce, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(current) => nonce = current,
            }
        }
        let directory = parent.join(format!(
            ".{leaf}.{}.{nonce}.migration.tmp",
            std::process::id()
        ));
        std::fs::create_dir(&directory)?;
        Ok(Self {
            target,
            directory,
            lease: Some(lease),
        })
    }
}

impl Drop for Destination {
    fn drop(&mut self) {
        // 目录由本次 create_dir 独占创建，仅含引擎生成文件；目标文件组和租约文件均不在其中。
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

pub(super) fn migrate<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    source: &Database<T>,
    path: &str,
    dim: usize,
) -> Result<(Database<T>, Vec<u64>)> {
    if dim == 0 || dim > 65_536 {
        return Err(TriviumError::InvalidInput(
            "迁移维度必须介于 1 和 65536 (Migration dimension must be between 1 and 65536)".into(),
        ));
    }
    let mut destination = Destination::new(path)?;
    let stage_path = destination
        .directory
        .join("database.tdb")
        .to_str()
        .ok_or_else(|| {
            TriviumError::InvalidInput(
                "迁移暂存路径不是 UTF-8 (Migration staging path is not UTF-8)".into(),
            )
        })?
        .to_owned();
    let config = Config {
        dim,
        memory_limit: source.memory_limit,
        ..Default::default()
    };
    let mt = read_or_recover(&source.memtable);
    mt.ensure_readable()?;
    let mut ids = mt.all_node_ids();
    ids.sort_unstable();
    {
        let mut stage = Database::<T>::open_with_runtime(
            &stage_path,
            Config {
                auto_build_quiver: false,
                ..config
            },
            source.resource_runtime.clone(),
        )?;
        let _workspace = source
            .resource_runtime
            .reserve_workspace(dim.saturating_mul(std::mem::size_of::<T>()))?;
        let zero = vec![T::zero(); dim];
        for &id in &ids {
            let payload = mt.get_payload(id).ok_or_else(|| {
                TriviumError::CorruptedFile(
                    "迁移源节点缺少 Payload (Migration source node is missing its Payload)".into(),
                )
            })?;
            stage.insert_with_id(id, &zero, (*payload).clone())?;
        }
        for &id in &ids {
            for edge in mt.get_edges(id).unwrap_or(&[]) {
                stage.upsert_edge(
                    id,
                    edge.target_id,
                    &edge.label,
                    edge.weight,
                    edge.metadata.clone(),
                )?;
            }
        }
        stage.flush()?;
        stage.close()?;
    }
    drop(mt);
    ensure_empty(&destination.target)?;
    crate::storage::file_format::publish_new_database::<T>(&stage_path, &destination.target)?;
    // 文件已经排他移动，清理剩余暂存目录期间目标租约仍由当前调用持有。
    std::fs::remove_dir_all(&destination.directory)?;
    // 目标租约直接交给新实例，发布与打开之间保持排他性。
    let _task = source.resource_runtime.enter_task()?;
    let reservation = source
        .resource_runtime
        .reserve_database(config.memory_limit)?;
    let database = Database::<T>::open_reserved_with_lock(
        &destination.target,
        config,
        source.resource_runtime.clone(),
        reservation,
        destination.lease.take(),
    )?;
    Ok((database, ids))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "tdb_migration_preflight_{}_{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn files(&self) -> BTreeMap<OsString, Vec<u8>> {
            std::fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    (entry.file_name(), std::fs::read(entry.path()).unwrap())
                })
                .collect()
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn 迁移预检按实际文件系统识别主文件及多段侧文件别名() {
        for (stored, requested, lookup) in [
            ("TARGET.TDB", "target.tdb", "target.tdb"),
            ("TARGET.TDB.wal", "target.tdb", "target.tdb.wal"),
            ("TARGET.TDB.PLD.9", "target.tdb", "target.tdb.PLD.9"),
            (
                "TARGET.TDB.future.meta",
                "target.tdb",
                "target.tdb.future.meta",
            ),
            ("ÜBER.TDB.future.meta", "über.tdb", "über.tdb.future.meta"),
            (".TARGET.TDB.extra", ".target.tdb", ".target.tdb.extra"),
            ("target.tdb.", "target.tdb", "target.tdb."),
        ] {
            let root = TestDir::new();
            std::fs::write(root.0.join(stored), "保留既有文件内容".as_bytes()).unwrap();
            // 独立查询夹具路径是否实际解析为已有条目，不假定操作系统或卷的大小写规则。
            let occupied = root.0.join(lookup).try_exists().unwrap();
            let before = root.files();
            let result = Destination::new(root.0.join(requested).to_str().unwrap());
            if occupied {
                assert!(
                    matches!(result, Err(TriviumError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists),
                    "必须在创建租约或暂存目录前拒绝：{stored}/{requested}"
                );
                assert_eq!(root.files(), before);
            } else {
                // 区分大小写的文件系统允许独立目标，既有条目内容仍须保留。
                drop(result.unwrap());
                assert_eq!(
                    std::fs::read(root.0.join(stored)).unwrap(),
                    before[&OsString::from(stored)]
                );
            }
        }
    }

    #[test]
    fn 迁移预检仅复用实际协调锁且保留独立大小写文件() {
        for stored in [
            "target.tdb.lock",
            "TARGET.TDB.lock",
            "target.tdb.LOCK",
            "TARGET.TDB.LOCK",
        ] {
            let root = TestDir::new();
            std::fs::write(root.0.join(stored), "空闲协调锁".as_bytes()).unwrap();
            let lock_exists = root.0.join("target.tdb.lock").try_exists().unwrap();
            let in_group = root.0.join("target.tdb.LOCK").try_exists().unwrap();
            let before = root.files();
            let result = ensure_empty(root.0.join("target.tdb").to_str().unwrap());
            if in_group && !lock_exists {
                assert!(
                    matches!(result, Err(TriviumError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
                );
            } else {
                result.unwrap();
            }
            assert_eq!(root.files(), before);
        }
    }

    // macOS Runner 在创建此类文件名时即返回 EILSEQ；原始字节夹具由 Linux 验证。
    #[cfg(target_os = "linux")]
    #[test]
    fn 迁移预检保留非utf8侧文件() {
        use std::os::unix::ffi::OsStringExt;
        let root = TestDir::new();
        let mut name = b"target.tdb.".to_vec();
        name.push(0xff);
        std::fs::write(
            root.0.join(OsString::from_vec(name)),
            "非 UTF-8 侧文件".as_bytes(),
        )
        .unwrap();
        let before = root.files();
        assert!(
            matches!(ensure_empty(root.0.join("target.tdb").to_str().unwrap()), Err(TriviumError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
        );
        assert_eq!(root.files(), before);
    }

    #[cfg(unix)]
    #[test]
    fn 迁移预检拒绝悬空主文件及协调锁链接() {
        use std::os::unix::fs::symlink;
        let root = TestDir::new();
        symlink(root.0.join("missing"), root.0.join("dangling.tdb")).unwrap();
        assert!(
            matches!(ensure_empty(root.0.join("dangling.tdb").to_str().unwrap()), Err(TriviumError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
        );
        assert_eq!(
            std::fs::read_link(root.0.join("dangling.tdb")).unwrap(),
            root.0.join("missing")
        );
        std::fs::write(root.0.join("foreign"), b"foreign lock").unwrap();
        symlink(root.0.join("foreign"), root.0.join("target.tdb.lock")).unwrap();
        assert!(
            matches!(ensure_empty(root.0.join("target.tdb").to_str().unwrap()), Err(TriviumError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
        );
        assert_eq!(
            std::fs::read(root.0.join("foreign")).unwrap(),
            b"foreign lock"
        );
        assert_eq!(
            std::fs::read_link(root.0.join("target.tdb.lock")).unwrap(),
            root.0.join("foreign")
        );
    }

    #[test]
    fn 迁移预检允许无关长文件名() {
        let root = TestDir::new();
        let stored = format!("x.{}", "a".repeat(180));
        let requested = format!("{}.tdb", "t".repeat(100));
        std::fs::write(root.0.join(&stored), b"unrelated").unwrap();
        let before = root.files();
        ensure_empty(root.0.join(&requested).to_str().unwrap()).unwrap();
        assert_eq!(root.files(), before);
    }
}
