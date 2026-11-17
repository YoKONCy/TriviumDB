//! 固定来源后端恢复旧数据，统一受管理文档结构，再交付逻辑事实。

use super::{Config, Database, MissingIndexPolicy, write_or_recover};
use crate::{Result, TriviumError, VectorType};
use std::io::Write;
use std::path::Path;
use target::database::inspection::transfer;

pub enum ConversionError {
    Source(TriviumError),
    Target(target::TriviumError),
}
impl From<TriviumError> for ConversionError {
    fn from(error: TriviumError) -> Self {
        Self::Source(error)
    }
}
impl From<target::TriviumError> for ConversionError {
    fn from(error: target::TriviumError) -> Self {
        Self::Target(error)
    }
}
impl From<std::io::Error> for ConversionError {
    fn from(error: std::io::Error) -> Self {
        Self::Source(error.into())
    }
}

fn normalize_documents<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    database: &mut Database<T>,
) -> Result<()> {
    let _workspace = reserve_workspace(database)?;
    for id in database.all_node_ids()? {
        let payload = database
            .get_payload(id)?
            .ok_or(TriviumError::NodeNotFound(id))?;
        let Some(tag) = payload.get(crate::document::DOCUMENT_FIELD) else {
            continue;
        };
        if tag.get("key").is_some()
            || tag
                .get("schema_version")
                .and_then(serde_json::Value::as_u64)
                != Some(1)
        {
            continue;
        }
        // 先按来源契约验证正文、片段和修订，再更新版本字段及匹配的受管理片段标签。
        let mut snapshot = database
            .get_document(id)?
            .ok_or(TriviumError::NodeNotFound(id))?;
        snapshot.schema_version = 2;
        let mut root = payload;
        root[crate::document::DOCUMENT_FIELD] = serde_json::to_value(&snapshot)
            .map_err(|e| TriviumError::InvalidInput(e.to_string()))?;
        let mut mt = write_or_recover(&database.memtable);
        mt.update_payload(id, root)?;
        for block in &snapshot.blocks {
            if let Some(node_id) = &block.node_id {
                let node_id = crate::document::node_id(node_id, "document.blocks.node_id")?;
                let mut payload = mt
                    .try_get_payload(node_id)?
                    .ok_or(TriviumError::NodeNotFound(node_id))?
                    .as_ref()
                    .clone();
                payload[crate::document::DOCUMENT_FIELD] =
                    crate::document::block_tag(&snapshot, block);
                mt.update_payload(node_id, payload)?;
            }
        }
    }
    Ok(())
}

fn reserve_workspace<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    database: &Database<T>,
) -> Result<crate::resources::WorkspaceReservation> {
    let mt = super::read_or_recover(&database.memtable);
    let peak = mt
        .estimated_memory_bytes()
        .saturating_mul(3)
        .saturating_add(1024 * 1024);
    mt.ensure_additional_memory(database.memory_limit, peak)?;
    database.resource_runtime.reserve_workspace(peak)
}

pub fn export<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    path: &str,
    dimension: usize,
    memory: usize,
    directory: &Path,
) -> std::result::Result<super::inspection::FactFingerprint, ConversionError> {
    let mut database = Database::<T>::open_with_config(
        path,
        Config {
            dim: dimension,
            memory_limit: memory,
            auto_build_quiver: false,
            load_text_index: true,
            missing_index_policy: MissingIndexPolicy::Fallback,
            ..Default::default()
        },
    )?;
    if crate::storage::index_container::open(path, ".text")?.is_none()
        && crate::storage::index_container::open(path, ".text.meta")?.is_some()
    {
        return Err(TriviumError::CorruptedFile("来源文本事实缺失而绑定记录仍存在 (Source text facts are missing while their binding record exists)".into()).into());
    }
    normalize_documents(&mut database)?;
    let expected = super::inspection::fingerprint(&database)?;
    {
        let _workspace = reserve_workspace(&database)?;
        let mt = super::read_or_recover(&database.memtable);
        let manifest: transfer::Manifest = serde_json::from_value(serde_json::json!({
            "protocol":1,"dimension":mt.dim(),"nodes":mt.node_count(),"next_id":mt.next_id_value(),
            "storage_id":mt.storage_id,"indexes":mt.property_indexes().index_definitions(),
            "unique":mt.unique_index_definitions()
        }))
        .map_err(|e| TriviumError::CorruptedFile(e.to_string()))?;
        let mut writer = transfer::create(&directory.join("manifest.json"))?;
        serde_json::to_writer(&mut writer, &manifest)
            .map_err(|e| TriviumError::CorruptedFile(e.to_string()))?;
        transfer::finish(writer)?;
        let mut writer = transfer::create(&directory.join("nodes.bin"))?;
        for id in mt.active_node_ids() {
            let payload = mt.get_payload(id).ok_or(TriviumError::NodeNotFound(id))?;
            let vector = mt.get_vector(id).ok_or_else(|| {
                TriviumError::CorruptedFile(
                    "节点向量事实缺失 (Node vector facts are missing)".into(),
                )
            })?;
            let record = transfer::NodeRecord {
                id,
                vector: bytemuck::cast_slice(vector).to_vec(),
                payload: payload.to_string(),
                edges: serde_json::to_string(mt.get_edges(id).unwrap_or(&[]))
                    .map_err(|e| TriviumError::CorruptedFile(e.to_string()))?,
            };
            transfer::write_node(&mut writer, &record)?;
        }
        transfer::finish(writer)?;
        mt.text_engine().save_to_file(&directory.join("text.bin"))?;
        let mut writer = transfer::create(&directory.join("graphs.bin"))?;
        crate::storage::named_graphs::save(&mt.named_graphs, &mut writer)?;
        transfer::finish(writer)?;
        let mut writer = transfer::create(&directory.join("documents.bin"))?;
        crate::storage::document_publications::save(&mt.document_publications, &mut writer)?;
        transfer::finish(writer)?;
        let mut writer = transfer::create(&directory.join("receipts.bin"))?;
        writer.write_all(&mt.editing.checkpoint()?)?;
        transfer::finish(writer)?;
    }
    database.close()?;
    Ok(expected)
}
