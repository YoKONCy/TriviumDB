//! 独立迁移执行器：持有源与目标租约，在副本恢复，核对事实后发布新目录。

mod files;
use files::{Lease, SourceFile, Stage, inventory, utf8};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use triviumdb::database::inspection::FactFingerprint;

pub const PROTOCOL: u32 = 1;
pub const TOOL_ID: &str = "tdb-migrate-2";

#[derive(Debug, Clone)]
pub struct Options {
    pub source: PathBuf,
    pub source_dtype: Option<u16>,
    pub destination: PathBuf,
    pub check: bool,
    pub memory_bytes: usize,
    pub max_source_bytes: u64,
    pub max_work_bytes: u64,
    pub mmap: bool,
}

#[derive(Debug, Serialize)]
pub struct Failure {
    pub code: String,
    pub message: String,
    pub phase: &'static str,
    pub outcome: &'static str,
    pub details: serde_json::Value,
}
pub type Result<T> = std::result::Result<T, Failure>;
impl Failure {
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        let message = message.into();
        let details = serde_json::json!({"reason": message});
        Self {
            code: code.into(),
            message,
            phase: "admission",
            outcome: "not_committed",
            details,
        }
    }
    fn legacy(error: legacy::database::migration_conversion::ConversionError) -> Self {
        let error = match error {
            legacy::database::migration_conversion::ConversionError::Source(error) => error,
            legacy::database::migration_conversion::ConversionError::Target(error) => {
                return error.into();
            }
        };
        Self {
            code: error.code().into(),
            message: error.to_string(),
            phase: "conversion",
            outcome: "not_committed",
            details: error.details(),
        }
    }
}
impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        let details = serde_json::json!({"io_kind":format!("{:?}", error.kind()), "os_code":error.raw_os_error()});
        Self {
            details,
            ..Self::new(
                "TDB_IO",
                format!("迁移文件操作失败 (Migration file operation failed): {error}"),
            )
        }
    }
}
impl From<triviumdb::TriviumError> for Failure {
    fn from(error: triviumdb::TriviumError) -> Self {
        Self {
            code: error.code().into(),
            message: error.to_string(),
            phase: "validation",
            outcome: "not_committed",
            details: error.details(),
        }
    }
}
impl From<serde_json::Error> for Failure {
    fn from(error: serde_json::Error) -> Self {
        Self::new(
            "TDB_CORRUPTED_FILE",
            format!("迁移记录无效 (Invalid migration record): {error}"),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Owner {
    pub tool: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub files: BTreeMap<String, SourceFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub protocol: u32,
    pub tool: String,
    pub source_format: u16,
    pub target_format: u16,
    pub dtype: String,
    pub checked: bool,
    pub committed: bool,
    pub reused: bool,
    pub database: PathBuf,
    pub facts: FactFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Leased,
    Converted,
    BeforePublish,
    AfterPublish,
}

fn inspect<T: triviumdb::VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    path: &Path,
    dim: usize,
    memory: usize,
) -> Result<FactFingerprint> {
    let mut db = triviumdb::Database::<T>::open_with_config(
        utf8(path)?,
        triviumdb::database::Config {
            dim,
            access_mode: triviumdb::database::AccessMode::ReadOnly,
            memory_limit: memory,
            auto_build_quiver: false,
            load_text_index: true,
            missing_index_policy: triviumdb::database::MissingIndexPolicy::Fallback,
            ..Default::default()
        },
    )?;
    let facts = triviumdb::database::inspection::fingerprint(&db)?;
    db.close()?;
    Ok(facts)
}

fn inspect_typed(path: &Path, dtype: u16, dim: usize, memory: usize) -> Result<FactFingerprint> {
    match dtype {
        1 => inspect::<f32>(path, dim, memory),
        2 => inspect::<half::f16>(path, dim, memory),
        3 => inspect::<u64>(path, dim, memory),
        _ => Err(Failure::new(
            "TDB_INVALID_INPUT",
            "向量类型无效 (Invalid vector dtype)",
        )),
    }
}

pub fn run(options: &Options) -> Result<Report> {
    run_controlled(options, |_| Ok(()))
}

/// 受控阶段用于进程中断和租约交错验收，生产调用使用 run。
#[doc(hidden)]
pub fn run_controlled(
    options: &Options,
    mut checkpoint: impl FnMut(Phase) -> Result<()>,
) -> Result<Report> {
    if options.memory_bytes < 16 * 1024 * 1024
        || options.max_source_bytes == 0
        || options.max_work_bytes == 0
    {
        return Err(Failure::new(
            "TDB_INVALID_INPUT",
            "迁移预算必须有效，内存至少 16 MiB (Valid migration budgets and at least 16 MiB memory are required)",
        ));
    }
    let source = options.source.canonicalize()?;
    let parent = options
        .destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()?;
    let leaf = options.destination.file_name().ok_or_else(|| {
        Failure::new(
            "TDB_INVALID_INPUT",
            "目标必须是新的目录名 (Destination must name a new directory)",
        )
    })?;
    let destination = parent.join(leaf);
    if std::fs::symlink_metadata(&destination)
        .is_ok_and(|metadata| !metadata.is_dir() || metadata.file_type().is_symlink())
    {
        return Err(Failure::new(
            "TDB_MIGRATION_TARGET_EXISTS",
            "目标必须是新的普通目录 (Destination must be a new regular directory)",
        ));
    }
    if source.starts_with(&destination) {
        return Err(Failure::new(
            "TDB_INVALID_INPUT",
            "目标目录包含源数据库 (Destination contains the source database)",
        ));
    }
    let _source_lease = Lease::source(&source, !options.check)?;
    triviumdb::database::inspection::validate_source_path(utf8(&source)?)?;
    let _target_lease = if options.check {
        None
    } else {
        Some(Lease::target(&destination)?)
    };
    checkpoint(Phase::Leased)?;
    let files = inventory(&source, options.max_source_bytes)?;
    let source_bytes = files
        .values()
        .try_fold(0u64, |sum, file| sum.checked_add(file.bytes))
        .ok_or_else(|| {
            Failure::new(
                "TDB_MIGRATION_BUDGET",
                "来源总长度溢出 (Source total size overflow)",
            )
        })?;
    let required = source_bytes
        .checked_mul(6)
        .and_then(|n| n.checked_add((options.memory_bytes as u64).saturating_mul(8)))
        .and_then(|n| n.checked_add(64 * 1024 * 1024))
        .ok_or_else(|| {
            Failure::new(
                "TDB_MIGRATION_BUDGET",
                "迁移工作区估算溢出 (Migration workspace estimate overflow)",
            )
        })?;
    if required > options.max_work_bytes || required > fs2::available_space(&parent)? {
        return Err(Failure::new(
            "TDB_MIGRATION_BUDGET",
            "迁移工作空间或可用磁盘不足 (Migration workspace budget or available disk space is insufficient)",
        ));
    }
    let (format, dtype, dimension) = files::identity(&source, options.source_dtype)?;
    let owner = Owner {
        tool: TOOL_ID.into(),
        source: source.clone(),
        destination: destination.clone(),
        files,
    };
    // 已有目标只有在包含本工具的同源交付记录时才进入内容复核；任何其他目标均保留。
    let previous = if destination.try_exists()? {
        if !destination.join("migration-source.json").is_file() {
            return Err(Failure::new(
                "TDB_MIGRATION_TARGET_EXISTS",
                "目标已存在且缺少迁移归属记录 (Destination exists without a migration ownership record)",
            ));
        }
        let prior: Owner = files::read_json(&destination.join("migration-source.json"))?;
        if prior != owner {
            return Err(Failure::new(
                "TDB_MIGRATION_TARGET_EXISTS",
                "目标归属与本次迁移不一致 (Destination ownership differs from this migration)",
            ));
        }
        true
    } else {
        false
    };
    let stage = Stage::prepare(&parent, leaf, &owner, options.check)?;
    let staged = stage.database.join("database.tdb");
    let copied = stage.source.join("database.tdb");
    files::copy_source(&source, &copied, &owner.files)?;
    let mode = if options.mmap {
        triviumdb::database::StorageMode::Mmap
    } else {
        triviumdb::database::StorageMode::Rom
    };
    let convert = |dtype| -> Result<FactFingerprint> {
        let path = utf8(&copied)?;
        let expected = match dtype {
            1 => legacy::database::migration_conversion::export::<f32>(
                path,
                dimension,
                options.memory_bytes,
                &stage.facts,
            ),
            2 => legacy::database::migration_conversion::export::<half::f16>(
                path,
                dimension,
                options.memory_bytes,
                &stage.facts,
            ),
            3 => legacy::database::migration_conversion::export::<u64>(
                path,
                dimension,
                options.memory_bytes,
                &stage.facts,
            ),
            _ => {
                return Err(Failure::new(
                    "TDB_INVALID_INPUT",
                    "向量类型无效 (Invalid vector dtype)",
                ));
            }
        }
        .map_err(Failure::legacy)?;
        let restored = match dtype {
            1 => triviumdb::database::inspection::transfer::restore::<f32>(
                &stage.facts,
                utf8(&staged)?,
                mode,
                options.memory_bytes,
            ),
            2 => triviumdb::database::inspection::transfer::restore::<half::f16>(
                &stage.facts,
                utf8(&staged)?,
                mode,
                options.memory_bytes,
            ),
            3 => triviumdb::database::inspection::transfer::restore::<u64>(
                &stage.facts,
                utf8(&staged)?,
                mode,
                options.memory_bytes,
            ),
            _ => unreachable!(),
        }?;
        let expected: FactFingerprint = serde_json::from_value(serde_json::to_value(expected)?)?;
        if restored != expected {
            return Err(Failure {
                details: serde_json::json!({"expected":expected,"actual":restored}),
                ..Failure::new(
                    "TDB_CORRUPTED_FILE",
                    "逻辑事实导入不一致 (Imported logical facts differ)",
                )
            });
        }
        Ok(expected)
    };
    let expected = convert(dtype)?;
    checkpoint(Phase::Converted)?;
    let actual = inspect_typed(&staged, dtype, dimension, options.memory_bytes)?;
    if expected != actual {
        return Err(Failure {
            details: serde_json::json!({"reason":"迁移前后事实不一致 (Facts differ after conversion)","expected":expected,"actual":actual}),
            ..Failure::new(
                "TDB_CORRUPTED_FILE",
                "迁移前后完整事实不一致 (Complete facts differ after migration)",
            )
        });
    }
    if inventory(&source, options.max_source_bytes)? != owner.files {
        return Err(Failure::new(
            "TDB_MIGRATION_CONFLICT",
            "租约期间来源发生变化 (Source changed while the migration lease was held)",
        ));
    }
    files::retain_current_group(&staged)?;
    let mut report = Report {
        protocol: PROTOCOL,
        tool: TOOL_ID.into(),
        source_format: format,
        target_format: triviumdb::storage::file_format::CURRENT_VERSION,
        dtype: match dtype {
            1 => "f32",
            2 => "f16",
            _ => "u64",
        }
        .into(),
        checked: true,
        committed: false,
        reused: false,
        database: destination.join("database.tdb"),
        facts: actual,
    };
    if options.check {
        stage.cleanup(&owner)?;
        return Ok(report);
    }
    if previous {
        let _published_lease = Lease::published(&report.database)?;
        files::finish_publication(&report.database, &owner)?;
        if inspect_typed(&report.database, dtype, dimension, options.memory_bytes)? != expected {
            return Err(Failure::new(
                "TDB_MIGRATION_CONFLICT",
                "已有目标事实已改变 (Previously published destination facts have changed)",
            ));
        }
        report.committed = true;
        report.reused = true;
        stage.cleanup(&owner)?;
        return Ok(report);
    }
    files::write_json(&stage.database.join("migration-source.json"), &owner)?;
    // 此记录随目录提交点同时可见；SDK 准入仍须等待发布标记完成确认。
    files::write_json(
        &stage.database.join("migration-report.json"),
        &Report {
            committed: true,
            ..report.clone()
        },
    )?;
    files::prepare_publication(&staged, &owner)?;
    checkpoint(Phase::BeforePublish)?;
    // 原子排他重命名是唯一提交点；之后的错误均明确记录目标已经提交。
    let published =
        triviumdb::database::inspection::publish_directory(&stage.database, &destination);
    let committed = destination.is_dir() && !stage.database.exists();
    let after = || -> Result<()> {
        published?;
        checkpoint(Phase::AfterPublish)?;
        let _published_lease = Lease::published(&report.database)?;
        files::finish_publication(&report.database, &owner)?;
        if inspect_typed(&report.database, dtype, dimension, options.memory_bytes)? != expected {
            return Err(Failure::new(
                "TDB_CORRUPTED_FILE",
                "已发布目标事实校验失败 (Published destination fact validation failed)",
            ));
        }
        stage.cleanup(&owner)?;
        Ok(())
    };
    after().map_err(|mut error| {
        if committed {
            error.outcome = "committed";
            error.phase = "publish";
        }
        error
    })?;
    report.committed = true;
    Ok(report)
}
