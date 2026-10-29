//! 维度迁移在独占暂存库中构建，通过目标租约和发布日志安装全新文件组。

use super::{Config, Database, read_or_recover};
use crate::{Result, TriviumError, VectorType};
use fs2::FileExt;
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
    let prefix = format!("{leaf}.");
    let lock = format!("{leaf}.lock");
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == leaf || (name.starts_with(&prefix) && name != lock) {
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists,
                format!("迁移目标文件组已经存在 (Migration destination file group already exists): {target}")).into());
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
    // Windows 在目标映射建立前释放暂存硬链接，清理期间目标租约仍由当前调用持有。
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
