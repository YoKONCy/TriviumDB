//! Immutable generation 的 manifest 构建与完整性校验。
//!
//! manifest 记录主文件及所有可发布 sidecar 的长度和 CRC32，发布前与打开时均按
//! generation ID、dtype、维度和节点数校验。写入使用临时文件、fsync 和原子替换；
//! 校验路径只读，确保 Immutable Reader 不修复、不补建、也不改变制品字节。

use crate::error::{Result, TriviumError};
use crate::storage::fs::robust_rename_and_sync;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MANIFEST_VERSION: u16 = 2;
const GENERATION_SUFFIXES: [&str; 9] = [
    "",
    ".vec",
    ".flush_ok",
    ".quiver",
    ".quiver.meta",
    ".text",
    ".text.meta",
    ".pidx",
    ".gidx",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationFile {
    pub suffix: String,
    pub size: u64,
    pub crc32: u32,
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

fn checksum_file(path: &Path) -> std::io::Result<(u64, u32)> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok((size, hasher.finalize()))
}

fn payload_generation_suffix(db_path: &str) -> Option<String> {
    let marker = std::fs::read(format!("{db_path}.flush_ok")).ok()?;
    if marker.len() != 53 || marker.get(..4) != Some(b"TFMK") || marker.get(4) != Some(&3) {
        return None;
    }
    let generation = u64::from_le_bytes(marker.get(5..13)?.try_into().ok()?);
    Some(format!(".pld.{generation}"))
}

fn generation_suffixes(db_path: &str) -> Vec<String> {
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
    if generation_id.trim().is_empty() {
        return Err(TriviumError::InvalidInput("generation_id 不能为空".into()));
    }
    let files = generation_suffixes(db_path)
        .into_iter()
        .map(|suffix| {
            let (size, crc32) = checksum_file(Path::new(&format!("{db_path}{suffix}")))?;
            Ok(GenerationFile {
                suffix,
                size,
                crc32,
            })
        })
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
    robust_rename_and_sync(&tmp, &path)?;
    Ok(manifest)
}

pub fn validate_manifest(
    db_path: &str,
    expected_dtype: &str,
    expected_dim: usize,
) -> Result<GenerationManifest> {
    let path = manifest_path(db_path);
    let bytes = std::fs::read(&path).map_err(|error| TriviumError::ImmutableArtifactInvalid {
        reason: format!("无法读取 {}: {error}", path.display()),
    })?;
    let manifest: GenerationManifest =
        serde_json::from_slice(&bytes).map_err(|error| TriviumError::ImmutableArtifactInvalid {
            reason: format!("manifest JSON 无效: {error}"),
        })?;
    if manifest.format_version != MANIFEST_VERSION
        || !manifest.complete
        || manifest.dtype != expected_dtype
        || manifest.dim != expected_dim
    {
        return Err(TriviumError::ImmutableArtifactInvalid {
            reason: "manifest 版本、完成状态、dtype 或维度不匹配".into(),
        });
    }
    if !manifest.files.iter().any(|file| file.suffix.is_empty()) {
        return Err(TriviumError::ImmutableArtifactInvalid {
            reason: "manifest 未声明主数据库文件".into(),
        });
    }
    let mut declared = std::collections::HashSet::new();
    for file in &manifest.files {
        let valid_payload = payload_generation_suffix(db_path).as_deref() == Some(&file.suffix);
        if (!GENERATION_SUFFIXES.contains(&file.suffix.as_str()) && !valid_payload)
            || !declared.insert(file.suffix.as_str())
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
    if let Some(payload) = payload_generation_suffix(db_path)
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
        let (size, crc32) =
            checksum_file(&file_path).map_err(|error| TriviumError::ImmutableArtifactInvalid {
                reason: format!("无法验证 {}: {error}", file_path.display()),
            })?;
        if size != file.size || crc32 != file.crc32 {
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
    let path = manifest_path(db_path);
    let bytes = std::fs::read(&path).map_err(|error| TriviumError::ImmutableArtifactInvalid {
        reason: format!("无法读取 {}: {error}", path.display()),
    })?;
    let declared: GenerationManifest =
        serde_json::from_slice(&bytes).map_err(|error| TriviumError::ImmutableArtifactInvalid {
            reason: format!("manifest JSON 无效: {error}"),
        })?;
    if declared.generation_id != expected_generation_id {
        return Err(TriviumError::ImmutableArtifactInvalid {
            reason: format!(
                "manifest generation_id {} 与目标 {} 不一致",
                declared.generation_id, expected_generation_id
            ),
        });
    }
    validate_manifest(db_path, &declared.dtype, declared.dim)
}
