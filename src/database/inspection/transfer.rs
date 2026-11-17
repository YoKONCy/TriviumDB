//! 工具使用的逻辑事实协议。目标始终通过当前内核写出当前持久化格式。

use super::FactFingerprint;
use crate::database::{Config, Database, StorageMode, write_or_recover};
use crate::{Result, TriviumError, VectorType};
use bincode::Options;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub protocol: u32,
    pub dimension: usize,
    pub nodes: usize,
    pub next_id: u64,
    pub storage_id: [u8; 16],
    pub indexes: Vec<(String, Vec<String>)>,
    pub unique: Vec<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRecord {
    pub id: u64,
    pub vector: Vec<u8>,
    pub payload: String,
    pub edges: String,
}

fn invalid(message: &str) -> TriviumError {
    TriviumError::CorruptedFile(format!(
        "逻辑事实交付无效 (Invalid logical fact transfer): {message}"
    ))
}

pub fn write_node(writer: &mut impl Write, node: &NodeRecord) -> Result<()> {
    let length = bincode::serialized_size(node).map_err(TriviumError::Serialization)?;
    writer.write_all(&length.to_le_bytes())?;
    bincode::serialize_into(writer, node).map_err(TriviumError::Serialization)
}

pub fn create(path: &Path) -> Result<BufWriter<File>> {
    Ok(BufWriter::new(
        OpenOptions::new().create_new(true).write(true).open(path)?,
    ))
}

pub fn finish(mut writer: BufWriter<File>) -> Result<()> {
    writer.flush()?;
    writer.get_ref().sync_all()?;
    Ok(())
}

fn bounded_read(path: &Path, memory: usize) -> Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let length = usize::try_from(file.metadata()?.len()).map_err(|_| invalid("length"))?;
    if memory != 0 && length.saturating_mul(32) > memory {
        return Err(TriviumError::CapacityReservationRejected {
            requested_nodes: 0,
            estimated_bytes: length.saturating_mul(32),
            current_bytes: 0,
            memory_limit: memory,
        });
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|e| TriviumError::CapacityAllocationFailed {
            reason: format!("事实读取分配失败 (Fact read allocation failed): {e}"),
        })?;
    Read::by_ref(&mut file)
        .take(length as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() != length {
        return Err(invalid("component changed during read"));
    }
    Ok(bytes)
}

fn component_workspace<T: VectorType>(
    mt: &crate::storage::memtable::MemTable<T>,
    path: &Path,
    memory: usize,
) -> Result<crate::resources::WorkspaceReservation> {
    let length =
        usize::try_from(std::fs::metadata(path)?.len()).map_err(|_| invalid("component length"))?;
    let peak = length.saturating_mul(32).saturating_add(64 * 1024);
    mt.ensure_additional_memory(memory, peak)?;
    mt.resource_runtime().reserve_workspace(peak)
}

/// 调用方提供独占的新目标与已验证的事实目录；外部目录发布负责整个迁移的提交点。
pub fn restore<T: VectorType + Serialize + DeserializeOwned>(
    directory: &Path,
    destination: &str,
    mode: StorageMode,
    memory: usize,
) -> Result<FactFingerprint> {
    crate::database::migration::ensure_empty(destination)?;
    let manifest: Manifest =
        serde_json::from_slice(&bounded_read(&directory.join("manifest.json"), memory)?)
            .map_err(|e| invalid(&e.to_string()))?;
    if manifest.protocol != 1 || manifest.next_id == 0 || manifest.storage_id == [0; 16] {
        return Err(invalid("manifest"));
    }
    let mut db = Database::<T>::open_with_config(
        destination,
        Config {
            dim: manifest.dimension,
            storage_mode: mode,
            memory_limit: memory,
            auto_build_quiver: false,
            load_text_index: true,
            ..Default::default()
        },
    )?;
    let mut nodes = BufReader::new(File::open(directory.join("nodes.bin"))?);
    let mut last_id = 0;
    // 节点和边分两遍处理，前向引用在全部节点就绪后进入当前边校验。
    for edges_only in [false, true] {
        use std::io::Seek;
        nodes.rewind()?;
        for _ in 0..manifest.nodes {
            crate::request::check_current()?;
            let mut size = [0; 8];
            nodes.read_exact(&mut size)?;
            let length = u64::from_le_bytes(size);
            let length = usize::try_from(length).map_err(|_| invalid("node size"))?;
            let mt = crate::database::read_or_recover(&db.memtable);
            mt.ensure_additional_memory(memory, length.saturating_mul(64))?;
            let _workspace = db
                .resource_runtime
                .reserve_workspace(length.saturating_mul(64))?;
            drop(mt);
            let mut raw = Vec::new();
            raw.try_reserve_exact(length)
                .map_err(|e| TriviumError::CapacityAllocationFailed {
                    reason: format!("事实记录分配失败 (Fact record allocation failed): {e}"),
                })?;
            nodes.by_ref().take(length as u64).read_to_end(&mut raw)?;
            if raw.len() != length {
                return Err(invalid("truncated node"));
            }
            let node: NodeRecord = bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .with_limit(length as u64)
                .reject_trailing_bytes()
                .deserialize(&raw)
                .map_err(TriviumError::Serialization)?;
            if edges_only {
                let edges: Vec<crate::Edge> =
                    serde_json::from_str(&node.edges).map_err(|e| invalid(&e.to_string()))?;
                for edge in edges {
                    db.upsert_edge(
                        node.id,
                        edge.target_id,
                        &edge.label,
                        edge.weight,
                        edge.metadata,
                    )?;
                }
            } else {
                if node.id <= last_id || node.id >= manifest.next_id {
                    return Err(invalid("node identity"));
                }
                last_id = node.id;
                let payload =
                    serde_json::from_str(&node.payload).map_err(|e| invalid(&e.to_string()))?;
                let width = std::mem::size_of::<T>();
                if node.vector.len()
                    != manifest
                        .dimension
                        .checked_mul(width)
                        .ok_or_else(|| invalid("vector length"))?
                {
                    return Err(invalid("vector length"));
                }
                let vector = node
                    .vector
                    .chunks_exact(width)
                    .map(bytemuck::pod_read_unaligned::<T>)
                    .collect::<Vec<_>>();
                db.insert_with_id(node.id, &vector, payload)?;
            }
        }
        let mut trailing = [0];
        if nodes.read(&mut trailing)? != 0 {
            return Err(invalid("trailing nodes"));
        }
    }
    for (kind, fields) in &manifest.indexes {
        match (kind.as_str(), fields.as_slice()) {
            ("hash", [field]) => db.create_index(field)?,
            ("ordered", [field]) => db.create_ordered_index(field)?,
            ("bitmap", [field]) => db.create_bitmap_index(field)?,
            ("ngram", [field]) => db.create_ngram_index(field)?,
            ("composite", _) => db.create_composite_index(fields)?,
            _ => return Err(invalid("index definition")),
        }
    }
    {
        let mut mt = write_or_recover(&db.memtable);
        mt.advance_next_id(manifest.next_id);
        mt.storage_id = manifest.storage_id;
        mt.restore_unique_index_definitions(&manifest.unique)?;
        let text_path = directory.join("text.bin");
        if text_path.is_file() {
            let _workspace = component_workspace(&mt, &text_path, memory)?;
            mt.set_text_index(crate::index::text::TextIndex::load_from_file(&text_path)?);
        }
        {
            let path = directory.join("graphs.bin");
            let _workspace = component_workspace(&mt, &path, memory)?;
            let graphs = bounded_read(&path, memory)?;
            let (prefix, graphs) = crate::storage::named_graphs::split_checkpoint(&graphs)?;
            if !prefix.is_empty() {
                return Err(invalid("graph prefix"));
            }
            crate::storage::named_graphs::restore(&mut mt, graphs)?;
        }
        {
            let path = directory.join("documents.bin");
            let _workspace = component_workspace(&mt, &path, memory)?;
            let documents = bounded_read(&path, memory)?;
            let (prefix, documents) =
                crate::storage::document_publications::split_checkpoint(&documents)?;
            if !prefix.is_empty() {
                return Err(invalid("document prefix"));
            }
            crate::storage::document_publications::restore(&mut mt, documents)?;
        }
        {
            let path = directory.join("receipts.bin");
            let _workspace = component_workspace(&mt, &path, memory)?;
            let receipts = bounded_read(&path, memory)?;
            // 目标建库过程中产生的临时编辑版本由来源回执状态整体替换。
            mt.editing = Default::default();
            for raw in crate::storage::edit_receipts::records(&receipts)? {
                mt.editing.restore(raw)?;
            }
        }
        mt.mark_checkpoint_dirty();
    }
    let fingerprint = super::fingerprint(&db)?;
    db.close()?;
    Ok(fingerprint)
}
