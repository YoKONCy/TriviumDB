//! 通过统一 `.idx` 容器发布双曲度量和可重建的图映像。

use super::{
    PelorusConfig, PelorusResidency, PelorusRuntimeOptions, PelorusState, engine, map_error,
    validate_dtype,
};
use crate::database::MissingIndexPolicy;
use crate::storage::index_container::{self, Part};
use crate::storage::memtable::MemTable;
use crate::{Result, TriviumError, VectorType};
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

const CONFIG_PART: &str = ".pelorus.meta";
const INDEX_PART: &str = ".pelorus";
const MAX_CONFIG_BYTES: u64 = 2 * 1024 * 1024;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    config: PelorusConfig,
    changed: Vec<crate::NodeId>,
}

pub(crate) fn stage<T: VectorType>(mt: &MemTable<T>, path: &str) -> Result<[Option<Part>; 2]> {
    let Some(state) = &mt.pelorus else {
        return Ok([None, None]);
    };
    let parent = Path::new(path)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut metadata = tempfile::tempfile_in(parent)?;
    let _metadata_workspace = mt
        .resource_runtime()
        .reserve_workspace(state.changed.len().saturating_mul(16))?;
    let mut changed = state.changed.iter().copied().collect::<Vec<_>>();
    changed.sort_unstable();
    serde_json::to_writer(
        &mut metadata,
        &Metadata {
            config: state.config,
            changed,
        },
    )
    .map_err(|e| {
        TriviumError::CorruptedFile(format!(
            "Pelorus 配置编码失败 (Pelorus config encoding failed): {e}"
        ))
    })?;
    let metadata = Part::staged(metadata)?;
    let Some(index) = &state.index else {
        return Ok([Some(metadata), None]);
    };
    let _workspace = mt.resource_runtime().reserve_workspace(
        index
            .navigation_storage_bytes()
            .saturating_mul(4)
            .saturating_add(index.len().saturating_mul(32)),
    )?;
    let mut image = tempfile::tempfile_in(parent)?;
    index.write_image(&mut image).map_err(map_error)?;
    Ok([Some(metadata), Some(Part::staged(image)?)])
}

pub(crate) fn load<T: VectorType>(
    mt: &mut MemTable<T>,
    path: &str,
    policy: MissingIndexPolicy,
) -> Result<()> {
    let Some(mut metadata) = index_container::open(path, CONFIG_PART)? else {
        return Ok(());
    };
    validate_dtype::<T>().map_err(|e| invalid_config(&e))?;
    if metadata.length > MAX_CONFIG_BYTES {
        return Err(invalid_config(&"metadata length"));
    }
    mt.check_load_allocation((metadata.length as usize).saturating_mul(8))?;
    metadata.checksum().map_err(|e| invalid_config(&e))?;
    let mut bytes = Vec::new();
    metadata.reader()?.read_to_end(&mut bytes)?;
    let Metadata { config, changed } =
        serde_json::from_slice(&bytes).map_err(|e| invalid_config(&e))?;
    if changed.len() > super::MAX_CHANGED_NODES
        || changed.first() == Some(&0)
        || changed.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(invalid_config(&"changed vector IDs"));
    }
    let mut changed = changed.into_iter().collect::<HashSet<_>>();
    config.validate(mt.dim()).map_err(|e| invalid_config(&e))?;
    let index = match index_container::open(path, INDEX_PART)? {
        None => None,
        Some(part) => match open_index(mt, config, &changed, part, engine::StorageMode::Memory) {
            Ok(index) => Some(index),
            Err(error)
                if policy != MissingIndexPolicy::Error && error.code() == "TDB_CORRUPTED_FILE" =>
            {
                tracing::warn!(error = %error, "拒绝损坏的 Pelorus 派生映像 (Rejected corrupt Pelorus derived image)");
                None
            }
            Err(error) => return Err(error),
        },
    };
    if index.is_none() {
        changed = HashSet::new();
    }
    mt.pelorus = Some(PelorusState {
        config,
        index,
        ssd: None,
        changed,
    });
    Ok(())
}

fn open_index<T: VectorType>(
    mt: &MemTable<T>,
    config: PelorusConfig,
    changed: &HashSet<crate::NodeId>,
    part: Part,
    mode: engine::StorageMode,
) -> Result<engine::Index> {
    let len = usize::try_from(part.length).map_err(|_| invalid_config(&"image length"))?;
    mt.check_load_allocation(len.saturating_mul(3))?;
    let file = part.file.try_clone()?;
    let offset = usize::try_from(part.offset).map_err(|_| invalid_config(&"image offset"))?;
    let mapping = part.map().map_err(|e| invalid_config(&e))?;
    let index =
        engine::Index::open_image(&file, offset, Arc::new(mapping), mode).map_err(map_error)?;
    let mut unchanged = 0;
    for id in index.ids().filter(|id| !changed.contains(id)) {
        if mt.internal_slot_of(id).is_none() {
            return Err(invalid_config(&"source vector IDs"));
        }
        unchanged += 1;
    }
    let active_changes = changed
        .iter()
        .filter(|&&id| mt.internal_slot_of(id).is_some())
        .count();
    if index.dimensions() != mt.dim()
        || unchanged + active_changes != mt.active_vector_count()
        || index.metric()
            != (engine::Metric::Poincare {
                curvature: config.curvature,
            })
        || index.config() != config.build_config()
        || index.poincare_navigation() != config.navigation
        || index.rerank_config() != config.rerank
    {
        return Err(invalid_config(&"source facts or index options"));
    }
    Ok(index)
}

pub(crate) fn configure_runtime<T: VectorType>(
    mt: &mut MemTable<T>,
    path: &str,
    options: PelorusRuntimeOptions,
    memory_limit: usize,
) -> Result<()> {
    let state = mt
        .pelorus
        .as_ref()
        .ok_or_else(|| TriviumError::ImmutableArtifactInvalid {
            reason: "Pelorus 度量配置缺失 (Pelorus metric configuration missing)".into(),
        })?;
    let part = index_container::open(path, INDEX_PART)?.ok_or_else(|| {
        TriviumError::ImmutableArtifactInvalid {
            reason: "Pelorus 需要已发布的图索引 (Pelorus requires a published graph index)".into(),
        }
    })?;
    let peak = usize::try_from(part.length)
        .unwrap_or(usize::MAX)
        .saturating_mul(3);
    mt.ensure_additional_memory(memory_limit, peak)?;
    let _workspace = mt.resource_runtime().reserve_workspace(peak)?;
    let mode = match options.residency {
        PelorusResidency::Hot => engine::StorageMode::Memory,
        PelorusResidency::Mmap | PelorusResidency::Ssd => engine::StorageMode::Mmap,
    };
    let mut index = open_index(mt, state.config, &state.changed, part, mode)?;
    let ssd = if options.residency == PelorusResidency::Ssd {
        let available = if memory_limit == 0 {
            usize::MAX
        } else {
            memory_limit.saturating_sub(mt.estimated_memory_bytes().saturating_add(peak))
        };
        index.prepare_shared_ssd(mt.resource_runtime(), options, available)?
    } else {
        None
    };
    crate::request::check_current()?;
    mt.pelorus = Some(PelorusState {
        config: state.config,
        index: Some(index),
        ssd,
        changed: state.changed.clone(),
    });
    Ok(())
}

fn invalid_config(reason: &impl std::fmt::Display) -> TriviumError {
    TriviumError::CorruptedFile(format!(
        "Pelorus 组件无效 (Invalid Pelorus component): {reason}"
    ))
}
