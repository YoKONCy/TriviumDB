//! 候选文档事实与不可变正文批次。提交前投影共享批次，发布时回收候选向量副本。

use crate::document::*;
use crate::{Result, TriviumError, VectorType};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

pub(crate) const MAX_RECORD_BYTES: usize = 32 * 1024 * 1024;
const MAX_PENDING: usize = 128;
const MAX_ARCHIVES: usize = 1_000_000;
const MAX_BATCHES: usize = 4096;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Header {
    pub upload_id: String,
    pub document_id: u64,
    pub revision: String,
    pub source_id: String,
    pub source_revision: String,
    pub parser_id: String,
    pub content_hash: String,
    pub source: Option<DocumentSourceDescriptor>,
    pub expected_revision: Option<String>,
    pub fingerprint: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Chunk {
    pub content: String,
    pub original_text: String,
    pub mappings: Vec<DocumentSourceMapEntry>,
    pub blocks: Vec<DocumentBlockInfo>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Change {
    Begin {
        header: Header,
        root: Option<String>,
    },
    Append {
        upload_id: String,
        sequence: usize,
        fingerprint: String,
        chunk: Chunk,
        nodes: Option<String>,
    },
    Publish {
        upload_id: String,
        batches: usize,
    },
    Forget {
        upload_id: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub sequence: u64,
    pub change: Change,
}

#[derive(Clone)]
pub(crate) struct Upload {
    pub header: Header,
    pub root: Option<Arc<String>>,
    pub chunks: Vec<Arc<Chunk>>,
    pub nodes: BTreeMap<usize, Arc<String>>,
    pub fingerprints: Vec<String>,
    pub published: bool,
    pub content_bytes: usize,
    pub original_bytes: usize,
    pub block_count: usize,
    pub map_count: usize,
    bytes: usize,
    metadata_bytes: usize,
}

#[derive(Clone, Default)]
pub(crate) struct PublicationStore {
    pub uploads: rpds::RedBlackTreeMapSync<String, Arc<Upload>>,
    by_document: rpds::RedBlackTreeMapSync<u64, rpds::RedBlackTreeSetSync<String>>,
    pending: usize,
    bytes: usize,
    pub clock: u64,
}

pub(crate) fn missing(id: &str) -> TriviumError {
    TriviumError::DocumentUploadNotFound {
        upload_id: id.into(),
    }
}
pub(crate) fn conflict(field: &str) -> TriviumError {
    TriviumError::DocumentUploadConflict {
        field: field.into(),
    }
    .at_field(field)
}
pub(crate) fn corrupt(error: impl std::fmt::Display) -> TriviumError {
    TriviumError::CorruptedFile(format!(
        "文档批次事实无效 (Invalid document batch facts): {error}"
    ))
}

impl PublicationStore {
    pub fn document_upload_ids(&self, id: u64) -> Option<&rpds::RedBlackTreeSetSync<String>> {
        self.by_document.get(&id)
    }
    pub fn memory_bytes(&self) -> usize {
        self.bytes.saturating_add(if self.uploads.is_empty() {
            0
        } else {
            16 * 1024
        })
    }
    pub fn projection_bytes(&self) -> usize {
        if self.uploads.is_empty() {
            0
        } else {
            // 按对数树高预留双索引路径复制，正文与身份另行计量。
            let levels = usize::BITS - self.uploads.size().leading_zeros();
            (levels as usize).saturating_mul(32 * 1024)
        }
    }
    pub fn projection_for_changes(&self, count: usize) -> usize {
        self.projection_bytes().saturating_mul(count).min(
            self.uploads
                .size()
                .saturating_add(1)
                .saturating_mul(512)
                .saturating_add(64 * 1024),
        )
    }
    pub fn deletion_workspace(&self, id: u64) -> usize {
        self.by_document.get(&id).map_or(0, |ids| {
            self.projection_for_changes(ids.size().saturating_add(1))
        })
    }
    pub fn change_workspace(&self, change: &Change) -> usize {
        match change {
            Change::Publish { upload_id, .. } => self
                .uploads
                .get(upload_id)
                .map_or(0, |upload| upload.workspace_bytes()),
            Change::Append { upload_id, .. } => self.uploads.get(upload_id).map_or(0, |upload| {
                upload
                    .metadata_bytes
                    .saturating_add(upload.chunks.len().saturating_mul(512))
            }),
            _ => 0,
        }
    }
    pub fn encoding_workspace(&self) -> Result<usize> {
        let mut maximum = 0;
        for upload in self.uploads.values() {
            maximum = maximum.max(
                crate::storage::payload_store::serialized_json_len(&upload.header)?
                    .saturating_mul(4)
                    .saturating_add(
                        upload
                            .root
                            .as_ref()
                            .map_or(0, |root| root.len().saturating_mul(8)),
                    ),
            );
            for (sequence, chunk) in upload.chunks.iter().enumerate() {
                maximum = maximum.max(
                    crate::storage::payload_store::serialized_json_len(chunk.as_ref())?
                        .saturating_mul(4)
                        .saturating_add(
                            upload
                                .nodes
                                .get(&sequence)
                                .map_or(0, |nodes| nodes.len().saturating_mul(8)),
                        ),
                );
            }
        }
        Ok(maximum)
    }
    pub fn get(&self, id: &str) -> Result<&Upload> {
        uuid::Uuid::parse_str(id).map_err(|_| conflict("upload_id"))?;
        self.uploads
            .get(id)
            .map(AsRef::as_ref)
            .ok_or_else(|| missing(id))
    }
    fn remove(&mut self, id: &str) {
        if let Some(upload) = self.uploads.get(id).cloned() {
            self.uploads.remove_mut(id);
            self.bytes = self.bytes.saturating_sub(upload.bytes);
            self.pending = self.pending.saturating_sub(usize::from(!upload.published));
            if let Some(ids) = self.by_document.get_mut(&upload.header.document_id) {
                ids.remove_mut(&upload.header.upload_id);
                if ids.is_empty() {
                    self.by_document.remove_mut(&upload.header.document_id);
                }
            }
        }
    }
    pub fn delete_node(&mut self, id: u64) {
        let removed = self.by_document.get(&id).cloned().unwrap_or_default();
        for key in &removed {
            self.remove(key);
        }
    }
    pub fn stamp(&self, change: Change) -> Result<Record> {
        Ok(Record {
            sequence: self
                .clock
                .checked_add(1)
                .ok_or_else(|| conflict("upload.sequence"))?,
            change,
        })
    }
    pub fn apply_record<T: VectorType>(
        &mut self,
        record: &Record,
        dim: usize,
        replay: bool,
    ) -> Result<()> {
        if record.sequence == 0 {
            return Err(conflict("upload.sequence"));
        }
        if replay && record.sequence <= self.clock {
            match &record.change {
                Change::Begin {
                    root: Some(raw), ..
                } => validate_nodes::<T>(raw, dim, true)?,
                Change::Append {
                    nodes: Some(raw), ..
                } => validate_nodes::<T>(raw, dim, false)?,
                _ => {}
            }
            return Ok(());
        }
        if Some(record.sequence) != self.clock.checked_add(1) {
            return Err(conflict("upload.sequence"));
        }
        self.apply::<T>(&record.change, dim)?;
        self.clock = record.sequence;
        Ok(())
    }
    fn apply<T: VectorType>(&mut self, record: &Change, dim: usize) -> Result<()> {
        match record {
            Change::Begin { header, root } => {
                uuid::Uuid::parse_str(&header.upload_id).map_err(|_| conflict("upload_id"))?;
                uuid::Uuid::parse_str(&header.revision).map_err(|_| conflict("revision"))?;
                crate::document::valid_id(header.document_id, "root.id")?;
                crate::document::source::validate_hash(&header.content_hash, "content_hash")?;
                if header.source_id.is_empty()
                    || header.source_revision.is_empty()
                    || header.parser_id.is_empty()
                {
                    return Err(conflict("source_id"));
                }
                if let Some(source) = &header.source {
                    crate::document::source::validate_hash(
                        &source.content_hash,
                        "source.content_hash",
                    )?;
                    if source.media_type.is_empty() || source.media_type.len() > 256 {
                        return Err(conflict("source.media_type"));
                    }
                    if let Some(hash) = &source.artifact_hash {
                        crate::document::source::validate_hash(hash, "source.artifact_hash")?;
                    }
                }
                if let Some(raw) = root {
                    validate_nodes::<T>(raw, dim, true)?;
                }
                if self.uploads.contains_key(&header.upload_id) {
                    return Err(conflict("upload_id"));
                }
                crate::document::budget(
                    "document_pending_uploads",
                    self.pending.saturating_add(1),
                    MAX_PENDING,
                )?;
                crate::document::budget(
                    "document_archives",
                    self.uploads.size().saturating_add(1),
                    MAX_ARCHIVES,
                )?;
                let metadata_bytes = crate::storage::payload_store::serialized_json_len(header)?
                    .saturating_mul(4)
                    .saturating_add(4096);
                let bytes = metadata_bytes.saturating_add(root.as_ref().map_or(0, String::len));
                self.pending += 1;
                let mut ids = self
                    .by_document
                    .get(&header.document_id)
                    .cloned()
                    .unwrap_or_default();
                ids.insert_mut(header.upload_id.clone());
                self.by_document.insert_mut(header.document_id, ids);
                self.bytes = self.bytes.saturating_add(bytes);
                self.uploads.insert_mut(
                    header.upload_id.clone(),
                    Arc::new(Upload {
                        header: header.clone(),
                        root: root.clone().map(Arc::new),
                        chunks: Vec::new(),
                        nodes: BTreeMap::new(),
                        fingerprints: Vec::new(),
                        published: false,
                        content_bytes: 0,
                        original_bytes: 0,
                        block_count: 0,
                        map_count: 0,
                        bytes,
                        metadata_bytes,
                    }),
                );
            }
            Change::Append {
                upload_id,
                sequence,
                fingerprint,
                chunk,
                nodes,
            } => {
                let upload = self.get(upload_id)?;
                if upload.published || *sequence != upload.chunks.len() {
                    return Err(conflict("sequence"));
                }
                crate::document::budget(
                    "document_batches",
                    sequence.saturating_add(1),
                    MAX_BATCHES,
                )?;
                crate::document::budget(
                    "document_content_bytes",
                    upload.content_bytes.saturating_add(chunk.content.len()),
                    64 * 1024 * 1024,
                )?;
                crate::document::budget(
                    "document_source_bytes",
                    upload
                        .original_bytes
                        .saturating_add(chunk.original_text.len()),
                    64 * 1024 * 1024,
                )?;
                crate::document::budget(
                    "document_blocks",
                    upload.block_count.saturating_add(chunk.blocks.len()),
                    100_000,
                )?;
                crate::document::budget(
                    "document_source_mappings",
                    upload.map_count.saturating_add(chunk.mappings.len()),
                    1_000_000,
                )?;
                if let Some(raw) = nodes {
                    validate_nodes::<T>(raw, dim, false)?;
                }
                let bytes = crate::storage::payload_store::serialized_json_len(chunk)?
                    .saturating_mul(4)
                    .saturating_add(nodes.as_ref().map_or(0, String::len))
                    .saturating_add(fingerprint.len())
                    .saturating_add(256);
                let upload = Arc::make_mut(
                    self.uploads
                        .get_mut(upload_id)
                        .ok_or_else(|| missing(upload_id))?,
                );
                upload.content_bytes += chunk.content.len();
                upload.original_bytes += chunk.original_text.len();
                upload.block_count += chunk.blocks.len();
                upload.map_count += chunk.mappings.len();
                upload.chunks.push(Arc::new(chunk.clone()));
                upload.fingerprints.push(fingerprint.clone());
                if let Some(raw) = nodes {
                    upload.nodes.insert(*sequence, Arc::new(raw.clone()));
                }
                upload.bytes = upload.bytes.saturating_add(bytes);
                self.bytes = self.bytes.saturating_add(bytes);
            }
            Change::Publish { upload_id, batches } => {
                let upload = self.get(upload_id)?;
                if *batches != upload.chunks.len() {
                    return Err(conflict("sequence"));
                }
                upload.snapshot()?.validate()?;
                let document_id = upload.header.document_id;
                let old = self
                    .by_document
                    .get(&document_id)
                    .into_iter()
                    .flat_map(|ids| ids.iter())
                    .filter(|id| {
                        id.as_str() != upload_id
                            && self
                                .uploads
                                .get(*id)
                                .is_some_and(|previous| previous.published)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                for id in old {
                    self.remove(&id);
                }
                let upload = Arc::make_mut(
                    self.uploads
                        .get_mut(upload_id)
                        .ok_or_else(|| missing(upload_id))?,
                );
                let freed = upload.root.as_ref().map_or(0, |raw| raw.len())
                    + upload.nodes.values().map(|raw| raw.len()).sum::<usize>();
                upload.root = None;
                upload.nodes.clear();
                if !upload.published {
                    self.pending = self.pending.saturating_sub(1);
                }
                upload.published = true;
                upload.bytes = upload.bytes.saturating_sub(freed);
                self.bytes = self.bytes.saturating_sub(freed);
            }
            Change::Forget { upload_id } => self.remove(upload_id),
        }
        Ok(())
    }
    pub fn finish_restore(&self) -> Result<()> {
        for upload in self.uploads.values() {
            if !upload.published
                && (upload.root.is_none() || upload.nodes.len() != upload.chunks.len())
            {
                return Err(corrupt("候选表示缺失 (Missing staged representations)"));
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_nodes<T: VectorType>(raw: &str, dim: usize, root: bool) -> Result<()> {
    let nodes = if root {
        vec![
            serde_json::from_str::<DocumentNode<T>>(raw).map_err(|error| {
                crate::document::invalid(
                    "root",
                    &format!("候选节点编码无效 (Invalid staged node encoding): {error}"),
                )
            })?,
        ]
    } else {
        serde_json::from_str::<Vec<DocumentNode<T>>>(raw).map_err(|error| {
            crate::document::invalid(
                "nodes",
                &format!("候选节点编码无效 (Invalid staged node encoding): {error}"),
            )
        })?
    };
    for node in nodes {
        crate::document::valid_id(node.id, "node.id")?;
        if node.vector.len() != dim {
            return Err(TriviumError::DimensionMismatch {
                expected: dim,
                got: node.vector.len(),
            });
        }
        if node.vector.iter().any(|value| !value.to_f32().is_finite()) {
            return Err(conflict("node.vector"));
        }
        let bytes = crate::storage::payload_store::json_encoded_len(&node.payload)?;
        if bytes > 8 * 1024 * 1024 {
            return Err(TriviumError::PayloadTooLarge {
                size_bytes: bytes,
                max_bytes: 8 * 1024 * 1024,
            });
        }
    }
    Ok(())
}

impl Upload {
    pub fn snapshot(&self) -> Result<DocumentSnapshot> {
        let mut content = String::with_capacity(self.content_bytes);
        let mut original = String::with_capacity(self.original_bytes);
        let mut blocks = Vec::with_capacity(self.block_count);
        let mut mappings = Vec::with_capacity(self.map_count);
        for chunk in &self.chunks {
            crate::request::check_current()?;
            content.push_str(&chunk.content);
            original.push_str(&chunk.original_text);
            blocks.extend(chunk.blocks.iter().cloned());
            mappings.extend(chunk.mappings.iter().cloned());
        }
        if crate::document::content_hash(&content) != self.header.content_hash {
            return Err(conflict("content_hash"));
        }
        if self
            .header
            .source
            .as_ref()
            .is_some_and(|source| !source.has_original_text)
            && self.original_bytes != 0
        {
            return Err(conflict("original_text"));
        }
        let source = self
            .header
            .source
            .as_ref()
            .map(|descriptor| DocumentSource {
                media_type: descriptor.media_type.clone(),
                content_hash: descriptor.content_hash.clone(),
                original_text: descriptor.has_original_text.then_some(original),
                artifact_hash: descriptor.artifact_hash.clone(),
                mappings,
            });
        if self.header.source.is_none() && (self.original_bytes != 0 || self.map_count != 0) {
            return Err(conflict("source"));
        }
        let snapshot = DocumentSnapshot {
            schema_version: 2,
            document_id: self.header.document_id,
            revision: self.header.revision.clone(),
            source_id: self.header.source_id.clone(),
            source_revision: self.header.source_revision.clone(),
            parser_id: self.header.parser_id.clone(),
            content_hash: self.header.content_hash.clone(),
            content,
            source,
            blocks,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
    pub fn status(&self, visible: bool) -> DocumentUploadStatus {
        DocumentUploadStatus {
            upload_id: self.header.upload_id.clone(),
            document_id: self.header.document_id,
            revision: self.header.revision.clone(),
            state: if self.published {
                DocumentUploadState::Published
            } else {
                DocumentUploadState::Pending
            },
            next_sequence: self.chunks.len(),
            content_bytes: self.content_bytes,
            original_bytes: self.original_bytes,
            blocks: self.block_count,
            mappings: self.map_count,
            visible,
        }
    }
    pub fn storage_bytes(&self) -> usize {
        self.bytes / 4
    }
    pub fn workspace_bytes(&self) -> usize {
        self.bytes.saturating_mul(4)
    }
}

pub(crate) fn split_checkpoint(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let footer = bytes
        .len()
        .checked_sub(16)
        .ok_or_else(|| corrupt("footer"))?;
    if &bytes[footer + 12..] != b"DARC" {
        return Err(corrupt("magic"));
    }
    let len = usize::try_from(u64::from_le_bytes(
        bytes[footer..footer + 8].try_into().map_err(corrupt)?,
    ))
    .map_err(corrupt)?;
    let start = footer.checked_sub(len).ok_or_else(|| corrupt("length"))?;
    let body = &bytes[start..footer];
    if crc32fast::hash(body)
        != u32::from_le_bytes(bytes[footer + 8..footer + 12].try_into().map_err(corrupt)?)
    {
        return Err(corrupt("CRC32"));
    }
    Ok((&bytes[..start], body))
}

pub(crate) fn save<W: std::io::Write>(store: &PublicationStore, writer: &mut W) -> Result<()> {
    let mut crc = crc32fast::Hasher::new();
    let mut total = 0u64;
    let count = store
        .uploads
        .values()
        .map(|upload| 1 + upload.chunks.len() + usize::from(upload.published))
        .sum::<usize>();
    let clock = store.clock.to_le_bytes();
    writer.write_all(&clock)?;
    crc.update(&clock);
    total += 8;
    let count = (count as u64).to_le_bytes();
    writer.write_all(&count)?;
    crc.update(&count);
    total += 8;
    let mut write = |record: Change| -> Result<()> {
        crate::request::check_current()?;
        let raw = serde_json::to_vec(&record).map_err(corrupt)?;
        crate::document::budget("document_record_bytes", raw.len(), MAX_RECORD_BYTES)?;
        let len = (raw.len() as u64).to_le_bytes();
        writer.write_all(&len)?;
        writer.write_all(&raw)?;
        crc.update(&len);
        crc.update(&raw);
        total += 8 + raw.len() as u64;
        Ok(())
    };
    for upload in store.uploads.values() {
        write(Change::Begin {
            header: upload.header.clone(),
            root: upload.root.as_ref().map(|root| root.as_ref().clone()),
        })?;
        for (sequence, chunk) in upload.chunks.iter().enumerate() {
            write(Change::Append {
                upload_id: upload.header.upload_id.clone(),
                sequence,
                fingerprint: upload.fingerprints[sequence].clone(),
                chunk: chunk.as_ref().clone(),
                nodes: upload
                    .nodes
                    .get(&sequence)
                    .map(|nodes| nodes.as_ref().clone()),
            })?;
        }
        if upload.published {
            write(Change::Publish {
                upload_id: upload.header.upload_id.clone(),
                batches: upload.chunks.len(),
            })?;
        }
    }
    writer.write_all(&total.to_le_bytes())?;
    writer.write_all(&crc.finalize().to_le_bytes())?;
    writer.write_all(b"DARC")?;
    Ok(())
}

pub(crate) fn restore<T: VectorType>(
    mt: &mut super::memtable::MemTable<T>,
    mut bytes: &[u8],
) -> Result<()> {
    fn number(bytes: &mut &[u8]) -> Result<usize> {
        let raw = bytes.get(..8).ok_or_else(|| corrupt("truncated length"))?;
        let value = usize::try_from(u64::from_le_bytes(raw.try_into().map_err(corrupt)?))
            .map_err(corrupt)?;
        *bytes = &bytes[8..];
        Ok(value)
    }
    let clock_raw = bytes.get(..8).ok_or_else(|| corrupt("clock"))?;
    let clock = u64::from_le_bytes(clock_raw.try_into().map_err(corrupt)?);
    bytes = &bytes[8..];
    let count = number(&mut bytes)?;
    if count > MAX_ARCHIVES * (MAX_BATCHES + 2) || count > bytes.len() / 8 {
        return Err(corrupt("count"));
    }
    for _ in 0..count {
        let len = number(&mut bytes)?;
        if len > MAX_RECORD_BYTES {
            return Err(corrupt("record length"));
        }
        let raw = bytes
            .get(..len)
            .ok_or_else(|| corrupt("truncated record"))?;
        mt.check_load_allocation(
            len.saturating_mul(64)
                .saturating_add(mt.document_publications.projection_bytes()),
        )?;
        let record: Change = serde_json::from_slice(raw).map_err(corrupt)?;
        if let Change::Begin { header, .. } = &record
            && mt
                .document_publications
                .uploads
                .contains_key(&header.upload_id)
        {
            return Err(corrupt("duplicate identity"));
        }
        let extra = mt.document_publications.change_workspace(&record);
        mt.check_load_allocation(extra)?;
        let _workspace = mt.resource_runtime().reserve_workspace(extra)?;
        mt.document_publications
            .apply::<T>(&record, mt.dim())
            .map_err(|error| {
                if matches!(
                    error.underlying(),
                    TriviumError::QueryCancelled | TriviumError::QueryDeadlineExceeded
                ) {
                    error
                } else {
                    corrupt(error)
                }
            })?;
        bytes = &bytes[len..];
    }
    if !bytes.is_empty() {
        return Err(corrupt("trailing data"));
    }
    mt.document_publications.clock = clock;
    mt.document_publications.finish_restore()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(body: &[u8]) -> Vec<u8> {
        let mut result = body.to_vec();
        result.extend_from_slice(&(body.len() as u64).to_le_bytes());
        result.extend_from_slice(&crc32fast::hash(body).to_le_bytes());
        result.extend_from_slice(b"DARC");
        result
    }

    #[test]
    fn 独立空归档编码及逐字节损坏与恶意长度拒绝() {
        let body = [0u8; 16];
        let expected = block(&body);
        let mut actual = Vec::new();
        save(&PublicationStore::default(), &mut actual).unwrap();
        assert_eq!(actual, expected);
        let mut mt = crate::storage::memtable::MemTable::<f32>::new(2);
        let (_, decoded) = split_checkpoint(&expected).unwrap();
        restore(&mut mt, decoded).unwrap();
        assert!(mt.document_publications.uploads.is_empty());
        for end in 0..expected.len() {
            assert_eq!(
                split_checkpoint(&expected[..end]).unwrap_err().code(),
                "TDB_CORRUPTED_FILE"
            );
        }
        for index in 0..expected.len() {
            let mut bad = expected.clone();
            bad[index] ^= 1;
            assert_eq!(
                split_checkpoint(&bad).unwrap_err().code(),
                "TDB_CORRUPTED_FILE",
                "字节{index}"
            );
        }
        for body in [
            vec![0; 8],
            [0u64.to_le_bytes(), u64::MAX.to_le_bytes()].concat(),
            [
                0u64.to_le_bytes(),
                1u64.to_le_bytes(),
                u64::MAX.to_le_bytes(),
            ]
            .concat(),
            [vec![0; 16], vec![1]].concat(),
        ] {
            let encoded = block(&body);
            let (_, body) = split_checkpoint(&encoded).unwrap();
            let mut mt = crate::storage::memtable::MemTable::<f32>::new(2);
            assert_eq!(
                restore(&mut mt, body).unwrap_err().code(),
                "TDB_CORRUPTED_FILE"
            );
            assert!(mt.document_publications.uploads.is_empty());
        }
    }
}
