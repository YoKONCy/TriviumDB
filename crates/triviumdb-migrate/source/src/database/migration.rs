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
    write: &mut crate::error::WriteErrorInfo,
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
    if mt
        .document_publications
        .uploads
        .values()
        .any(|upload| !upload.published)
    {
        return Err(crate::document::invalid(
            "document_uploads",
            "维度迁移前须发布或终止候选文档 (Publish or abort pending documents before dimension migration)",
        ));
    }

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
        {
            let mut target = super::write_or_recover(&stage.memtable);
            let bytes = mt
                .named_graphs
                .projection_bytes()
                .saturating_add(mt.document_publications.projection_bytes());
            target.ensure_additional_memory(config.memory_limit, bytes)?;
            let _workspace = source.resource_runtime.reserve_workspace(bytes)?;
            // 暂存目标保留图身份、修订和共享引用，随目标检查点统一发布。
            target.named_graphs = mt.named_graphs.clone();
            target.document_publications = mt.document_publications.clone();
            target.mark_changed(false);
        }
        stage.flush()?;
        stage.close()?;
    }
    drop(mt);
    // 暂存库释放后先预留最终实例配额，再公开目标，避免发布后被其他实例抢走额度。
    let _task = source.resource_runtime.enter_task()?;
    let reservation = source
        .resource_runtime
        .reserve_database(config.memory_limit)?;
    ensure_empty(&destination.target)?;
    crate::storage::file_format::publish_new_database::<T>(
        &stage_path,
        &destination.target,
        write,
    )?;
    // 文件已经排他移动，清理剩余暂存目录期间目标租约仍由当前调用持有。
    std::fs::remove_dir_all(&destination.directory)?;
    // 目标租约直接交给新实例，发布与打开之间保持排他性。
    let database = Database::<T>::open_reserved_with_lock(
        &destination.target,
        config,
        source.resource_runtime.clone(),
        reservation,
        destination.lease.take(),
    )?;
    Ok((database, ids))
}
