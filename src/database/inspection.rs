//! 为离线工具生成完整持久事实的有界摘要；调用方持有数据库租约。

use super::{Database, write_or_recover};
use crate::{Result, VectorType};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;

pub mod transfer;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FactFingerprint {
    pub protocol: u32,
    pub nodes: usize,
    pub edges: usize,
    pub next_id: u64,
    pub dimension: usize,
    pub sha256: String,
    pub components: std::collections::BTreeMap<String, String>,
}

pub(crate) struct DigestWriter(pub Sha256);

impl Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn record(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *writer, value).map_err(|error| {
        crate::TriviumError::CorruptedFile(format!(
            "持久事实编码失败 (Persistent fact encoding failed): {error}"
        ))
    })?;
    writer.write_all(b"\n")?;
    Ok(())
}

pub fn fingerprint<T: VectorType + serde::Serialize + serde::de::DeserializeOwned>(
    database: &Database<T>,
) -> Result<FactFingerprint> {
    let _operation = database.enter_operation()?;
    let mut mt = write_or_recover(&database.memtable);
    mt.ensure_readable()?;
    // 摘要包含延迟加载的文本事实；加载完成后在同一借用内核对完整快照。
    mt.prepare_text_index()?;
    let workspace = mt
        .estimated_memory_bytes()
        .saturating_mul(3)
        .saturating_add(1024 * 1024);
    mt.ensure_additional_memory(database.memory_limit, workspace)?;
    let _workspace = database.resource_runtime.reserve_workspace(workspace)?;
    let mut hash = DigestWriter(Sha256::new());
    let mut components = std::collections::BTreeMap::new();
    let mut seal = |name: &str, hash: &mut DigestWriter| {
        components.insert(name.to_owned(), format!("{:x}", hash.0.finalize_reset()));
    };
    let mut edges = 0usize;
    record(
        &mut hash,
        &(
            1u32,
            std::any::type_name::<T>(),
            mt.dim(),
            mt.next_id_value(),
        ),
    )?;
    for id in mt.active_node_ids() {
        crate::request::check_current()?;
        let payload = mt.get_payload(id).ok_or_else(|| {
            crate::TriviumError::CorruptedFile("节点缺少 Payload (Node is missing Payload)".into())
        })?;
        record(&mut hash, &(id, payload.as_ref()))?;
        if let Some(vector) = mt.get_vector(id) {
            hash.write_all(&(vector.len() as u64).to_le_bytes())?;
            hash.write_all(bytemuck::cast_slice(vector))?;
        } else {
            hash.write_all(&0u64.to_le_bytes())?;
        }
        let mut outgoing = mt.get_edges(id).unwrap_or(&[]).iter().collect::<Vec<_>>();
        outgoing.sort_unstable_by(|a, b| (a.target_id, &a.label).cmp(&(b.target_id, &b.label)));
        edges = edges.checked_add(outgoing.len()).ok_or_else(|| {
            crate::TriviumError::InvalidInput("边计数溢出 (Edge count overflow)".into())
        })?;
        record(&mut hash, &outgoing)?;
    }
    seal("nodes_edges", &mut hash);
    record(&mut hash, &mt.property_indexes().index_definitions())?;
    record(&mut hash, &mt.unique_index_definitions())?;
    seal("index_definitions", &mut hash);
    mt.text_engine().write_facts(&mut hash)?;
    seal("text", &mut hash);
    crate::storage::named_graphs::save(&mt.named_graphs, &mut hash)?;
    seal("named_graphs", &mut hash);
    crate::storage::document_publications::save(&mt.document_publications, &mut hash)?;
    seal("documents", &mut hash);
    hash.write_all(&mt.editing.checkpoint()?)?;
    seal("receipts", &mut hash);
    if let Some(pelorus) = &mt.pelorus {
        record(&mut hash, &pelorus.config.curvature)?;
        seal("pelorus_curvature", &mut hash);
    }
    record(&mut hash, &components)?;
    Ok(FactFingerprint {
        protocol: 1,
        nodes: mt.node_count(),
        edges,
        next_id: mt.next_id_value(),
        dimension: mt.dim(),
        sha256: format!("{:x}", hash.0.finalize()),
        components,
    })
}

/// 租约由调用方持有；目录内全部制品须在调用前完成同步及事实核验。
pub fn publish_directory(source: &std::path::Path, target: &std::path::Path) -> Result<()> {
    crate::error::WriteErrorInfo::capture_destination(|write| {
        crate::storage::fs::sync_parent_directory(&source.join("database.tdb"))?;
        crate::storage::fs::rename_new(source, target)?;
        write.phase = crate::error::WritePhase::Publish;
        write.outcome = crate::error::WriteOutcome::Committed;
        crate::storage::fs::sync_parent_directory(target)?;
        Ok(())
    })
}

pub fn sync_directory(directory: &std::path::Path) -> Result<()> {
    crate::storage::fs::sync_parent_directory(&directory.join("database.tdb"))?;
    Ok(())
}

/// 离线工具复用主文件硬链接准入；协调租约仍由调用方持有。
pub fn validate_source_path(path: &str) -> Result<()> {
    super::reject_hard_link_alias(path, &std::fs::metadata(path)?)
}
