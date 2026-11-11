//! 独立快照目录的流式复制、完整性校验与原子发布。

use super::*;
use crate::error::{WriteErrorInfo, WriteOutcome, WritePhase};
use crate::resources::ResourceRuntime;
use fs2::FileExt;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

const DATABASE_FILE: &str = "database.tdb";
const TRANSFER_BYTES: usize = 2 * 1024 * 1024;
static NONCE: AtomicU64 = AtomicU64::new(1);

fn invalid(reason: impl Into<String>) -> TriviumError {
    TriviumError::ImmutableArtifactInvalid {
        reason: reason.into(),
    }
}

/// 校验独立快照的 manifest、文件集合和 CRC；读取过程不修改快照。
pub fn verify_snapshot(directory: &str) -> Result<GenerationManifest> {
    verify_snapshot_with_runtime(directory, &ResourceRuntime::shared_default())
}

pub fn verify_snapshot_with_runtime(
    directory: &str,
    runtime: &ResourceRuntime,
) -> Result<GenerationManifest> {
    let _task = runtime.enter_maintenance_task()?;
    let _workspace = runtime.reserve_maintenance_workspace(TRANSFER_BYTES)?;
    verify(Path::new(directory))
}

fn regular(path: &Path) -> Result<()> {
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(invalid(format!(
            "快照制品必须是普通文件：{}",
            path.display()
        )));
    }
    Ok(())
}

fn verify(directory: &Path) -> Result<GenerationManifest> {
    if !std::fs::symlink_metadata(directory)?.file_type().is_dir() {
        return Err(invalid("快照必须是实际目录，不能是符号链接"));
    }
    // 在读取和校验任何制品前拒绝链接、管道和目录，同时限制可枚举文件数。
    let mut actual = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err(invalid("快照只能包含普通文件"));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("快照文件名不是有效文本"))?;
        actual.insert(name);
        if actual.len() > GENERATION_SUFFIXES.len() + 2 {
            return Err(invalid("快照文件数量超过格式上限"));
        }
    }
    let database = directory.join(DATABASE_FILE);
    let manifest_path = manifest_path(&database.to_string_lossy());
    regular(&manifest_path)?;
    let declared: GenerationManifest =
        serde_json::from_slice(&read_manifest_bytes(&manifest_path)?)
            .map_err(|error| invalid(format!("快照 manifest 无效：{error}")))?;
    if declared.generation_id.trim().is_empty()
        || declared.generation_id.len() > 256
        || !["f32", std::any::type_name::<half::f16>(), "u64"].contains(&declared.dtype.as_str())
    {
        return Err(invalid("快照 generation_id 或 dtype 无效"));
    }
    // 先校验受支持的后缀，再按声明访问文件，避免由 manifest 指定任意路径。
    let manifest = validate_manifest(&database.to_string_lossy(), &declared.dtype, declared.dim)?;
    let mut expected = std::collections::BTreeSet::from([format!("{DATABASE_FILE}.manifest.json")]);
    for file in &manifest.files {
        let name = format!("{DATABASE_FILE}{}", file.suffix);
        regular(&directory.join(&name))?;
        expected.insert(name);
    }
    if actual != expected {
        return Err(invalid("快照包含未声明文件、WAL 或临时制品"));
    }
    crate::storage::file_format::validate_snapshot_metadata(
        &database,
        manifest.dim,
        manifest.node_count,
    )?;
    Ok(manifest)
}

/// 将经过校验的快照恢复到新目录；原快照与已经存在的目标均不会被改写。
pub fn restore_snapshot(source: &str, destination: &str) -> Result<GenerationManifest> {
    restore_snapshot_with_runtime(source, destination, &ResourceRuntime::shared_default())
}

pub fn restore_snapshot_with_runtime(
    source: &str,
    destination: &str,
    runtime: &ResourceRuntime,
) -> Result<GenerationManifest> {
    WriteErrorInfo::capture_destination(|write| {
        let _task = runtime.enter_maintenance_task()?;
        write.phase = WritePhase::Validation;
        let _workspace = runtime.reserve_maintenance_workspace(TRANSFER_BYTES)?;
        let source = std::fs::canonicalize(source)?;
        let manifest = verify(&source)?;
        let stage = Stage::new(destination, Some(&source))?;
        for file in &manifest.files {
            let name = format!("{DATABASE_FILE}{}", file.suffix);
            let copied = copy_file(&source.join(&name), &stage.directory.join(&name))?;
            if copied != (file.size, file.crc32) {
                return Err(invalid("恢复复制期间源快照发生变化"));
            }
        }
        write_copied_manifest(&stage.directory, &manifest)?;
        verify(&stage.directory)?;
        stage.publish(write)?;
        Ok(manifest)
    })
}

pub(crate) fn copy_database_snapshot(
    source: &str,
    destination: &str,
    generation_id: &str,
    dtype: &str,
    dim: usize,
    node_count: usize,
    runtime: &ResourceRuntime,
    write: &mut WriteErrorInfo,
) -> Result<GenerationManifest> {
    validate_generation_id(generation_id)?;
    let _workspace = runtime.reserve_maintenance_workspace(TRANSFER_BYTES)?;
    let stage = Stage::new(destination, None)?;
    let files = generation_suffixes(source)
        .into_iter()
        .map(|suffix| {
            let (size, crc32) = copy_file(
                Path::new(&format!("{source}{suffix}")),
                &stage.directory.join(format!("{DATABASE_FILE}{suffix}")),
            )?;
            Ok(GenerationFile {
                suffix,
                size,
                crc32,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let manifest = GenerationManifest {
        format_version: MANIFEST_VERSION,
        generation_id: generation_id.into(),
        dtype: dtype.into(),
        dim,
        node_count,
        files,
        complete: true,
    };
    write_copied_manifest(&stage.directory, &manifest)?;
    verify(&stage.directory)?;
    stage.publish(write)?;
    Ok(manifest)
}

fn copy_file(source: &Path, target: &Path) -> Result<(u64, u32)> {
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::SnapshotCopy)?;
    regular(source)?;
    let input = std::fs::File::open(source)?;
    let length = input.metadata()?.len();
    let mut input = input.take(length);
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    let mut buffer = [0u8; 64 * 1024];
    let mut hasher = crc32fast::Hasher::new();
    let mut size = 0u64;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| invalid("快照文件长度溢出"))?;
        output.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
    }
    let mut extra = [0u8; 1];
    if size != length || input.into_inner().read(&mut extra)? != 0 {
        return Err(invalid("复制期间源文件长度发生变化"));
    }
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::io_result(crate::test_hooks::IoPoint::SnapshotSync)?;
    output.sync_all()?;
    Ok((size, hasher.finalize()))
}

fn write_copied_manifest(directory: &Path, manifest: &GenerationManifest) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|e| invalid(e.to_string()))?;
    let temporary = directory.join("manifest.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    robust_rename_and_sync(
        &temporary,
        &directory.join(format!("{DATABASE_FILE}.manifest.json")),
    )?;
    Ok(())
}

struct Stage {
    directory: PathBuf,
    target: PathBuf,
    _lease: std::fs::File,
}

impl Stage {
    fn new(destination: &str, source: Option<&Path>) -> Result<Self> {
        let requested = Path::new(destination);
        let leaf = requested
            .file_name()
            .ok_or_else(|| TriviumError::InvalidInput("快照目标必须有目录名称".into()))?;
        let parent = std::fs::canonicalize(
            requested
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        if source.is_some_and(|source| parent.starts_with(source)) {
            return Err(invalid("恢复目标不能位于源快照内部"));
        }
        let target = parent.join(leaf);
        let exists = || match std::fs::symlink_metadata(&target) {
            Ok(_) => Err(TriviumError::SnapshotDestinationExists {
                path: target.to_string_lossy().into_owned(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        };
        exists()?;
        let lease_path = parent.join(format!(".{}.snapshot.lock", leaf.to_string_lossy()));
        let lease = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lease_path)?;
        lease
            .try_lock_exclusive()
            .map_err(|_| TriviumError::DatabaseLocked("快照目标正在发布".into()))?;
        exists()?;
        let directory = parent.join(format!(
            ".{}.{}.{}.snapshot.tmp",
            leaf.to_string_lossy(),
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory)?;
        Ok(Self {
            directory,
            target,
            _lease: lease,
        })
    }

    fn publish(self, write: &mut WriteErrorInfo) -> Result<()> {
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::io_result(crate::test_hooks::IoPoint::SnapshotPublish)?;
        if std::fs::symlink_metadata(&self.target).is_ok() {
            return Err(TriviumError::SnapshotDestinationExists {
                path: self.target.to_string_lossy().into_owned(),
            });
        }
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeSnapshotPublish);
        write.phase = WritePhase::Publish;
        write.outcome = WriteOutcome::Unknown;
        let published = (|| -> std::io::Result<()> {
            crate::storage::fs::robust_rename(&self.directory, &self.target)?;
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::io_result(crate::test_hooks::IoPoint::SnapshotDirectorySync)?;
            crate::storage::fs::sync_parent_directory(&self.target)
        })();
        if let Err(error) = published {
            if self.target.is_dir() && !self.directory.exists() {
                return Err(TriviumError::SnapshotPublicationUncertain {
                    path: self.target.to_string_lossy().into_owned(),
                    reason: error.to_string(),
                });
            }
            return Err(error.into());
        }
        write.outcome = WriteOutcome::Committed;
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::AfterSnapshotPublish);
        Ok(())
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        // 只删除本次通过 create_dir 独占创建的暂存目录；发布后的目标永不由清理路径删除。
        if self.directory.is_dir() {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
}
