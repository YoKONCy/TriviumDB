//! 不可变数据库代际的发布、解析与 Reader 租约管理。
//!
//! Writer 在独立 generation 目录完成构建和 manifest 校验后，才通过临时文件、
//! fsync 与原子 rename 发布 `current.json`。Reader 使用运行时目录中的租约保护
//! 正在读取的旧代际，使发布新版本与回收旧版本解耦；Immutable 数据目录本身始终
//! 保持零写。所有 generation/file 名称都按单一路径组件校验，避免目录穿越。

use crate::VectorType;
use crate::database::DatabaseReader;
use crate::error::{Result, TriviumError};
use crate::storage::fs::robust_rename_and_sync;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const CURRENT_VERSION: u16 = 1;
static CURRENT_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CurrentGeneration {
    pub format_version: u16,
    pub generation_id: String,
    pub database_file: String,
}

#[derive(Debug, Clone)]
enum RuntimePath {
    Explicit(PathBuf),
    DefaultBase(PathBuf),
}

#[derive(Debug, Clone)]
pub struct GenerationStore {
    root: PathBuf,
    runtime_root: RuntimePath,
    path_error: Option<(std::io::ErrorKind, String)>,
}

impl GenerationStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::from_paths(root.into(), None)
    }

    pub fn with_runtime_dir(root: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self::from_paths(root.into(), Some(runtime_root.into()))
    }

    fn from_paths(root: PathBuf, runtime_root: Option<PathBuf>) -> Self {
        let mut path_error = None;
        let mut anchor = |path: PathBuf| match std::path::absolute(&path) {
            Ok(absolute) => absolute,
            Err(error) => {
                path_error = Some((
                    error.kind(),
                    format!("代际路径解析失败 (Generation path resolution failed): {error}"),
                ));
                path
            }
        };
        let root = anchor(root);
        let runtime_root = match runtime_root {
            Some(path) => RuntimePath::Explicit(anchor(path)),
            None => {
                RuntimePath::DefaultBase(anchor(std::env::temp_dir().join("triviumdb-runtime")))
            }
        };
        Self {
            root,
            runtime_root,
            path_error,
        }
    }

    fn ensure_path(&self) -> Result<()> {
        if let Some((kind, reason)) = &self.path_error {
            return Err(std::io::Error::new(*kind, reason.clone()).into());
        }
        Ok(())
    }

    fn runtime_directory(&self) -> Result<PathBuf> {
        self.ensure_path()?;
        let base = match &self.runtime_root {
            RuntimePath::Explicit(directory) => return Ok(directory.clone()),
            RuntimePath::DefaultBase(base) => base,
        };
        // 根目录存在后按规范路径确定身份，构造时机和常规路径别名共享锁空间。
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(self.root.canonicalize()?.to_string_lossy().as_bytes());
        Ok(base.join(format!("store-{:08x}", hasher.finalize())))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn generation_dir(&self, generation_id: &str) -> Result<PathBuf> {
        self.ensure_path()?;
        validate_component(generation_id, "generation_id")?;
        Ok(self.root.join(generation_id))
    }

    pub fn database_path(&self, generation_id: &str, database_file: &str) -> Result<PathBuf> {
        validate_component(database_file, "database_file")?;
        Ok(self.generation_dir(generation_id)?.join(database_file))
    }

    pub fn prepare_generation(&self, generation_id: &str, database_file: &str) -> Result<PathBuf> {
        let path = self.database_path(generation_id, database_file)?;
        let directory = path
            .parent()
            .ok_or_else(|| TriviumError::InvalidInput("generation 数据库路径缺少父目录".into()))?;
        std::fs::create_dir_all(directory)?;
        Ok(path)
    }

    pub fn publish_current(
        &self,
        generation_id: &str,
        database_file: &str,
    ) -> Result<CurrentGeneration> {
        self.ensure_path()?;
        std::fs::create_dir_all(&self.root)?;
        let _management = self.lock_management_exclusive()?;
        let database_path = self.database_path(generation_id, database_file)?;
        let manifest = crate::storage::snapshot::validate_manifest_for_generation(
            &database_path.to_string_lossy(),
            generation_id,
        )?;
        let actual_node_count = match manifest.dtype.as_str() {
            "f32" => DatabaseReader::<f32>::open_immutable(
                &database_path.to_string_lossy(),
                manifest.dim,
            )?
            .node_count(),
            "half::binary16::f16" => DatabaseReader::<half::f16>::open_immutable(
                &database_path.to_string_lossy(),
                manifest.dim,
            )?
            .node_count(),
            "u64" => DatabaseReader::<u64>::open_immutable(
                &database_path.to_string_lossy(),
                manifest.dim,
            )?
            .node_count(),
            dtype => {
                return Err(TriviumError::ImmutableArtifactInvalid {
                    reason: format!("manifest dtype 不受支持: {dtype}"),
                });
            }
        }?;
        if actual_node_count != manifest.node_count {
            return Err(TriviumError::ImmutableArtifactInvalid {
                reason: format!(
                    "manifest node_count {} 与数据库 {} 不一致",
                    manifest.node_count, actual_node_count
                ),
            });
        }
        let current = CurrentGeneration {
            format_version: CURRENT_VERSION,
            generation_id: generation_id.to_string(),
            database_file: database_file.to_string(),
        };
        let target = self.root.join("current.json");
        let temporary = self.root.join(format!(
            ".current.{}.{}.tmp",
            std::process::id(),
            CURRENT_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        let bytes = serde_json::to_vec_pretty(&current).map_err(|error| {
            TriviumError::ImmutableArtifactInvalid {
                reason: error.to_string(),
            }
        })?;
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        robust_rename_and_sync(&temporary, &target)?;
        Ok(current)
    }

    pub fn resolve_current(&self) -> Result<CurrentGeneration> {
        self.ensure_path()?;
        let path = self.root.join("current.json");
        let bytes =
            std::fs::read(&path).map_err(|error| TriviumError::ImmutableArtifactInvalid {
                reason: format!("无法读取 {}: {error}", path.display()),
            })?;
        let current: CurrentGeneration = serde_json::from_slice(&bytes).map_err(|error| {
            TriviumError::ImmutableArtifactInvalid {
                reason: format!("current.json 无效: {error}"),
            }
        })?;
        if current.format_version != CURRENT_VERSION {
            return Err(TriviumError::ImmutableArtifactInvalid {
                reason: "current.json 版本不受支持".into(),
            });
        }
        validate_component(&current.generation_id, "generation_id")?;
        validate_component(&current.database_file, "database_file")?;
        Ok(current)
    }

    pub fn open_current<T>(&self, dim: usize) -> Result<GenerationReader<T>>
    where
        T: VectorType + serde::Serialize + serde::de::DeserializeOwned,
    {
        let _management = self.lock_management_shared()?;
        let current = self.resolve_current()?;
        let directory = self.generation_dir(&current.generation_id)?;
        let lease_path = self.reader_lock_path(&current.generation_id)?;
        std::fs::create_dir_all(self.runtime_directory()?)?;
        let lease = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lease_path)?;
        lease
            .try_lock_shared()
            .map_err(|_| TriviumError::GenerationBusy {
                generation_id: current.generation_id.clone(),
            })?;
        let path = directory.join(&current.database_file);
        let manifest = crate::storage::snapshot::validate_manifest_for_generation(
            &path.to_string_lossy(),
            &current.generation_id,
        )?;
        let reader = DatabaseReader::open_immutable(&path.to_string_lossy(), dim)?;
        let node_count = reader.node_count()?;
        if node_count != manifest.node_count {
            return Err(TriviumError::ImmutableArtifactInvalid {
                reason: format!(
                    "manifest node_count {} 与数据库 {} 不一致",
                    manifest.node_count, node_count
                ),
            });
        }
        Ok(GenerationReader {
            reader,
            current,
            _lease: lease,
        })
    }

    pub fn reclaim_generation(&self, generation_id: &str) -> Result<()> {
        let _management = self.lock_management_exclusive()?;
        let current = self.resolve_current()?;
        if current.generation_id == generation_id {
            return Err(TriviumError::GenerationBusy {
                generation_id: generation_id.to_string(),
            });
        }
        let directory = self.generation_dir(generation_id)?;
        if !directory.exists() {
            return Ok(());
        }
        let lease_path = self.reader_lock_path(generation_id)?;
        std::fs::create_dir_all(self.runtime_directory()?)?;
        let lease = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lease_path)?;
        lease
            .try_lock_exclusive()
            .map_err(|_| TriviumError::GenerationBusy {
                generation_id: generation_id.to_string(),
            })?;
        std::fs::remove_dir_all(&directory)?;
        Ok(())
    }

    fn lock_management_shared(&self) -> Result<std::fs::File> {
        let runtime_root = self.runtime_directory()?;
        std::fs::create_dir_all(&runtime_root)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(runtime_root.join("generation.lock"))?;
        file.lock_shared()?;
        Ok(file)
    }

    fn lock_management_exclusive(&self) -> Result<std::fs::File> {
        let runtime_root = self.runtime_directory()?;
        std::fs::create_dir_all(&runtime_root)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(runtime_root.join("generation.lock"))?;
        file.lock_exclusive()?;
        Ok(file)
    }

    fn reader_lock_path(&self, generation_id: &str) -> Result<PathBuf> {
        validate_component(generation_id, "generation_id")?;
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(self.root.canonicalize()?.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        hasher.update(generation_id.as_bytes());
        Ok(self
            .runtime_directory()?
            .join(format!("reader-{:08x}.lock", hasher.finalize())))
    }
}

pub struct GenerationReader<T: VectorType> {
    reader: DatabaseReader<T>,
    current: CurrentGeneration,
    _lease: std::fs::File,
}

impl<T: VectorType> GenerationReader<T> {
    pub fn generation_id(&self) -> &str {
        &self.current.generation_id
    }

    pub fn database_file(&self) -> &str {
        &self.current.database_file
    }
}

impl<T: VectorType> Deref for GenerationReader<T> {
    type Target = DatabaseReader<T>;

    fn deref(&self) -> &Self::Target {
        &self.reader
    }
}

fn validate_component(value: &str, field: &str) -> Result<()> {
    let path = Path::new(value);
    let mut components = path.components();
    let valid = !value.is_empty()
        && components
            .next()
            .is_some_and(|component| matches!(component, Component::Normal(_)))
        && components.next().is_none()
        && value != "."
        && value != "..";
    if !valid {
        return Err(TriviumError::InvalidInput(format!(
            "{field} 必须是不含路径分隔符的单一安全名称"
        )));
    }
    Ok(())
}
