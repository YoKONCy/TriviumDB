//! Immutable generation 的 manifest 构建与完整性校验。
//!
//! manifest 记录主文件及所有可发布 sidecar 的长度和内容摘要，发布前与打开时均按
//! generation ID、dtype、维度和节点数校验。写入使用临时文件、fsync 和原子替换；
//! 校验路径只读，确保 Immutable Reader 不修复、不补建、也不改变制品字节。

use crate::error::{Result, TriviumError};
use crate::storage::fs::robust_rename_and_sync;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
mod transfer;
use std::io::Read;
use std::path::{Path, PathBuf};
pub(crate) use transfer::copy_database_snapshot;
pub use transfer::{
    restore_snapshot, restore_snapshot_with_runtime, verify_snapshot, verify_snapshot_with_runtime,
};

pub const MANIFEST_VERSION: u16 = 3;
const LEGACY_MANIFEST_VERSION: u16 = 2;
use crate::storage::artifacts::CHECKPOINT_SUFFIXES as GENERATION_SUFFIXES;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationFile {
    pub suffix: String,
    pub size: u64,
    pub crc32: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationManifest {
    pub format_version: u16,
    pub generation_id: String,
    pub dtype: String,
    pub dim: usize,
    pub node_count: usize,
    pub files: Vec<GenerationFile>,
    pub complete: bool,
}

pub fn manifest_path(db_path: &str) -> PathBuf {
    PathBuf::from(format!("{db_path}.manifest.json"))
}

#[derive(Default)]
struct FileDigest {
    crc: crc32fast::Hasher,
    sha: Sha256,
}

impl FileDigest {
    fn update(&mut self, bytes: &[u8]) {
        self.crc.update(bytes);
        self.sha.update(bytes);
    }

    fn finish(self, size: u64, suffix: String) -> GenerationFile {
        GenerationFile {
            suffix,
            size,
            crc32: self.crc.finalize(),
            sha256: Some(format!("{:x}", self.sha.finalize())),
        }
    }
}

fn checksum_file(path: &Path, suffix: String) -> std::io::Result<GenerationFile> {
    let mut digest = FileDigest::default();
    let mut file = std::fs::File::open(path)?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(digest.finish(size, suffix));
        }
        size += read as u64;
        digest.update(&buffer[..read]);
    }
}

impl GenerationFile {
    fn matches_digest(&self, actual: &Self) -> bool {
        self.size == actual.size
            && self.crc32 == actual.crc32
            && self
                .sha256
                .as_ref()
                .is_none_or(|sha| Some(sha) == actual.sha256.as_ref())
    }
}

fn payload_generation_suffix(db_path: &str) -> Option<String> {
    let generation = super::file_format::read_marker_generation(&format!("{db_path}.flush_ok"))?;
    Some(format!(".pld.{generation}"))
}

pub(crate) fn generation_suffixes(db_path: &str) -> Vec<String> {
    let mut suffixes = GENERATION_SUFFIXES
        .into_iter()
        .filter(|suffix| Path::new(&format!("{db_path}{suffix}")).exists())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if let Some(payload) = payload_generation_suffix(db_path)
        && Path::new(&format!("{db_path}{payload}")).exists()
    {
        suffixes.push(payload);
    }
    suffixes
}

pub fn write_manifest(
    db_path: &str,
    generation_id: &str,
    dtype: &str,
    dim: usize,
    node_count: usize,
) -> Result<GenerationManifest> {
    let mut write = crate::error::WriteErrorInfo::MAINTENANCE_ADMISSION;
    write_manifest_with_write_info(db_path, generation_id, dtype, dim, node_count, &mut write)
}

pub(crate) fn validate_generation_id(generation_id: &str) -> Result<()> {
    if generation_id.trim().is_empty() || generation_id.len() > 256 {
        return Err(TriviumError::InvalidInput(
            "generation_id 必须为 1 到 256 字节 (generation_id must contain 1 to 256 bytes)".into(),
        ));
    }
    Ok(())
}

pub(crate) fn write_manifest_with_write_info(
    db_path: &str,
    generation_id: &str,
    dtype: &str,
    dim: usize,
    node_count: usize,
    write: &mut crate::error::WriteErrorInfo,
) -> Result<GenerationManifest> {
    validate_generation_id(generation_id)?;
    let files = generation_suffixes(db_path)
        .into_iter()
        .map(|suffix| checksum_file(Path::new(&format!("{db_path}{suffix}")), suffix))
        .collect::<std::io::Result<Vec<_>>>()?;
    if !files.iter().any(|file| file.suffix.is_empty()) {
        return Err(TriviumError::ImmutableArtifactInvalid {
            reason: "manifest 缺少主数据库文件".into(),
        });
    }
    let manifest = GenerationManifest {
        format_version: MANIFEST_VERSION,
        generation_id: generation_id.to_string(),
        dtype: dtype.to_string(),
        dim,
        node_count,
        files,
        complete: true,
    };
    let path = manifest_path(db_path);
    let tmp = PathBuf::from(format!("{}.tmp", path.to_string_lossy()));
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| {
        TriviumError::ImmutableArtifactInvalid {
            reason: error.to_string(),
        }
    })?;
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    write.phase = crate::error::WritePhase::Publish;
    write.outcome = crate::error::WriteOutcome::Unknown;
    robust_rename_and_sync(&tmp, &path)?;
    write.outcome = crate::error::WriteOutcome::Committed;
    Ok(manifest)
}

pub fn validate_manifest(
    db_path: &str,
    expected_dtype: &str,
    expected_dim: usize,
) -> Result<GenerationManifest> {
    let manifest = read_manifest(db_path)?;
    if manifest.dtype != expected_dtype || manifest.dim != expected_dim {
        return Err(TriviumError::ImmutableArtifactInvalid {
            reason: "清单 dtype 或维度不匹配 (Manifest dtype or dimension mismatch)".into(),
        });
    }
    validate_manifest_files(db_path, manifest)
}

fn read_manifest(db_path: &str) -> Result<GenerationManifest> {
    let path = manifest_path(db_path);
    let bytes =
        read_bounded_metadata(&path).map_err(|error| TriviumError::ImmutableArtifactInvalid {
            reason: format!(
                "无法读取 {} (Could not read {}): {error}",
                path.display(),
                path.display()
            ),
        })?;
    serde_json::from_slice(&bytes).map_err(|error| TriviumError::ImmutableArtifactInvalid {
        reason: format!("清单 JSON 无效 (Invalid manifest JSON): {error}"),
    })
}

fn validate_manifest_files(
    db_path: &str,
    manifest: GenerationManifest,
) -> Result<GenerationManifest> {
    if !matches!(
        manifest.format_version,
        MANIFEST_VERSION | LEGACY_MANIFEST_VERSION
    ) || !manifest.complete
    {
        return Err(TriviumError::ImmutableArtifactInvalid {
            reason: "清单版本或完成状态无效 (Invalid manifest version or completion state)".into(),
        });
    }
    if !manifest.files.iter().any(|file| file.suffix.is_empty()) {
        return Err(TriviumError::ImmutableArtifactInvalid {
            reason: "manifest 未声明主数据库文件".into(),
        });
    }
    let mut declared = std::collections::HashSet::new();
    let payload_suffix = payload_generation_suffix(db_path);
    for file in &manifest.files {
        let valid_payload = payload_suffix.as_deref() == Some(&file.suffix);
        if (!GENERATION_SUFFIXES.contains(&file.suffix.as_str()) && !valid_payload)
            || !declared.insert(file.suffix.as_str())
            || manifest.format_version == MANIFEST_VERSION && file.sha256.is_none()
        {
            return Err(TriviumError::ImmutableArtifactInvalid {
                reason: format!("manifest 包含非法或重复文件后缀: {}", file.suffix),
            });
        }
    }
    let mut expected = GENERATION_SUFFIXES
        .into_iter()
        .filter(|suffix| Path::new(&format!("{db_path}{suffix}")).exists())
        .map(str::to_string)
        .collect::<std::collections::HashSet<_>>();
    if let Some(payload) = payload_suffix
        && Path::new(&format!("{db_path}{payload}")).exists()
    {
        expected.insert(payload);
    }
    if expected != declared.into_iter().map(str::to_string).collect() {
        return Err(TriviumError::ImmutableArtifactInvalid {
            reason: "generation 文件集合与 manifest 不一致".into(),
        });
    }
    for file in &manifest.files {
        let file_path = PathBuf::from(format!("{db_path}{}", file.suffix));
        let actual = checksum_file(&file_path, file.suffix.clone()).map_err(|error| {
            TriviumError::ImmutableArtifactInvalid {
                reason: format!("无法验证 {}: {error}", file_path.display()),
            }
        })?;
        if !file.matches_digest(&actual) {
            return Err(TriviumError::ImmutableArtifactInvalid {
                reason: format!("{} 大小或校验和不匹配", file_path.display()),
            });
        }
    }
    Ok(manifest)
}

pub fn validate_manifest_for_generation(
    db_path: &str,
    expected_generation_id: &str,
) -> Result<GenerationManifest> {
    let declared = read_manifest(db_path)?;
    if declared.generation_id != expected_generation_id {
        return Err(TriviumError::ImmutableArtifactInvalid {
            reason: format!(
                "manifest generation_id {} 与目标 {} 不一致",
                declared.generation_id, expected_generation_id
            ),
        });
    }
    validate_manifest_files(db_path, declared)
}

pub(crate) fn read_bounded_metadata(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "元数据超过 64 KiB 上限 (Metadata exceeds 64 KiB limit)",
        ));
    }
    Ok(bytes)
}
