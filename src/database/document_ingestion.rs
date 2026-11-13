//! 自动解析与分批发布入口。

use super::{Database, read_or_recover};
use crate::document::{
    parsing::{DocumentParseInput, ParsedDocument},
    *,
};
use crate::storage::document_publications::{self as stored, Change as Record, Chunk, Header};
use crate::{NodeId, Result, TriviumError, VectorType};
use serde::{Serialize, de::DeserializeOwned};

impl<T: VectorType + Serialize + DeserializeOwned> Database<T> {
    pub fn parse_document(&self, input: &DocumentParseInput) -> Result<ParsedDocument> {
        self.read_operation(|| {
            let extra = input.content.len().saturating_mul(64).saturating_add(4096);
            self.ensure_incremental_memory_budget(extra)?;
            let _workspace = self.resource_runtime.reserve_workspace(extra)?;
            parsing::parse(input)
        })?
    }

    pub fn begin_document_upload(
        &mut self,
        input: DocumentUploadBegin<T>,
    ) -> Result<DocumentUploadStatus> {
        Self::write_operation(|write| {
            let _operation = self.enter_operation()?;
            self.check_transaction_admission()?;
            write.phase = crate::error::WritePhase::Validation;
            let size = crate::storage::payload_store::serialized_json_len(&input)?;
            budget("document_record_bytes", size, stored::MAX_RECORD_BYTES / 2)?;
            self.ensure_incremental_memory_budget(size.saturating_mul(64))?;
            let _workspace = self
                .resource_runtime
                .reserve_workspace(size.saturating_mul(64))?;
            let fingerprint = content_hash(
                &serde_json::to_string(&input).map_err(super::documents::stored_error)?,
            );
            {
                let mt = read_or_recover(&self.memtable);
                if let Some(upload) = mt.document_publications.uploads.get(&input.upload_id) {
                    if upload.header.fingerprint != fingerprint {
                        return Err(stored::conflict("upload_id"));
                    }
                    return Ok(upload.status(visible(&mt, upload)?));
                }
            }
            self.check_document_target(
                input.root.id,
                &input.source_id,
                input.expected_revision.as_deref(),
            )?;
            let root =
                serde_json::to_string(&input.root).map_err(super::documents::stored_error)?;
            let upload_id = input.upload_id.clone();
            let header = Header {
                upload_id: input.upload_id,
                document_id: input.root.id,
                revision: uuid::Uuid::new_v4().to_string(),
                source_id: input.source_id,
                source_revision: input.source_revision,
                parser_id: input.parser_id,
                content_hash: input.content_hash,
                source: input.source,
                expected_revision: input.expected_revision,
                fingerprint,
            };
            self.commit_document_record(Record::Begin {
                header,
                root: Some(root),
            })?;
            self.document_upload_status(&upload_id)
        })
    }

    pub fn append_document_upload(
        &mut self,
        upload_id: &str,
        batch: DocumentUploadBatch<T>,
    ) -> Result<DocumentUploadStatus> {
        Self::write_operation(|write| {
            let _operation = self.enter_operation()?;
            self.check_transaction_admission()?;
            write.phase = crate::error::WritePhase::Validation;
            let size = crate::storage::payload_store::serialized_json_len(&batch)?;
            budget("document_batch_bytes", size, stored::MAX_RECORD_BYTES / 2)?;
            self.ensure_incremental_memory_budget(size.saturating_mul(64))?;
            let _workspace = self
                .resource_runtime
                .reserve_workspace(size.saturating_mul(64))?;
            let fingerprint = content_hash(
                &serde_json::to_string(&batch).map_err(super::documents::stored_error)?,
            );
            {
                let mt = read_or_recover(&self.memtable);
                let upload = mt.document_publications.get(upload_id)?;
                if let Some(previous) = upload.fingerprints.get(batch.sequence) {
                    if previous != &fingerprint {
                        return Err(stored::conflict("sequence"));
                    }
                    return Ok(upload.status(visible(&mt, upload)?));
                }
                if upload.published || batch.sequence != upload.chunks.len() {
                    return Err(stored::conflict("sequence"));
                }
            }
            let mut blocks = Vec::with_capacity(batch.blocks.len());
            let mut nodes = Vec::new();
            for block in batch.blocks {
                blocks.push(DocumentBlockInfo {
                    key: block.key,
                    parent: block.parent,
                    kind: block.kind,
                    start_byte: block.start_byte,
                    end_byte: block.end_byte,
                    node_id: block.node.as_ref().map(|node| node.id.to_string()),
                });
                if let Some(node) = block.node {
                    nodes.push(node);
                }
            }
            let nodes = serde_json::to_string(&nodes).map_err(super::documents::stored_error)?;
            self.commit_document_record(Record::Append {
                upload_id: upload_id.into(),
                sequence: batch.sequence,
                fingerprint,
                chunk: Chunk {
                    content: batch.content,
                    original_text: batch.original_text,
                    mappings: batch.mappings,
                    blocks,
                },
                nodes: Some(nodes),
            })?;
            self.document_upload_status(upload_id)
        })
    }

    pub fn document_upload_status(&self, upload_id: &str) -> Result<DocumentUploadStatus> {
        self.read_operation(|| {
            let mt = read_or_recover(&self.memtable);
            let upload = mt.document_publications.get(upload_id)?;
            Ok(upload.status(visible(&mt, upload)?))
        })?
    }

    pub fn list_document_uploads(
        &self,
        document_id: Option<NodeId>,
    ) -> Result<Vec<DocumentUploadStatus>> {
        self.read_operation(|| {
            if let Some(id) = document_id {
                valid_id(id, "document_id")?;
            }
            let mt = read_or_recover(&self.memtable);
            let count = document_id.map_or(mt.document_publications.uploads.size(), |id| {
                mt.document_publications
                    .document_upload_ids(id)
                    .map_or(0, rpds::RedBlackTreeSetSync::size)
            });
            let extra = count.saturating_mul(1024);
            mt.ensure_additional_memory(self.memory_limit, extra)?;
            let _workspace = self.resource_runtime.reserve_workspace(extra)?;
            let selected = if let Some(id) = document_id {
                mt.document_publications
                    .document_upload_ids(id)
                    .into_iter()
                    .flat_map(|ids| ids.iter())
                    .filter_map(|id| mt.document_publications.uploads.get(id))
                    .collect::<Vec<_>>()
            } else {
                mt.document_publications
                    .uploads
                    .values()
                    .collect::<Vec<_>>()
            };
            selected
                .into_iter()
                .map(|upload| Ok(upload.status(visible(&mt, upload)?)))
                .collect()
        })?
    }

    pub fn publish_document_upload(&mut self, upload_id: &str) -> Result<DocumentSnapshot> {
        Self::write_operation(|write| {
            let _operation = self.enter_operation()?;
            self.check_transaction_admission()?;
            write.phase = crate::error::WritePhase::Validation;
            let (upload, _workspace) = {
                let mt = read_or_recover(&self.memtable);
                let upload = mt.document_publications.get(upload_id)?;
                let extra = upload.workspace_bytes();
                mt.ensure_additional_memory(self.memory_limit, extra)?;
                (
                    upload.clone(),
                    self.resource_runtime.reserve_workspace(extra)?,
                )
            };
            if upload.published {
                let mt = read_or_recover(&self.memtable);
                if !visible(&mt, &upload)? {
                    return Err(stored::conflict("upload_id"));
                }
                return super::documents::read_snapshot(&mt, upload.header.document_id)?
                    .ok_or(TriviumError::NodeNotFound(upload.header.document_id));
            }
            let snapshot = upload.snapshot()?;
            let root: DocumentNode<T> = serde_json::from_str(
                upload
                    .root
                    .as_deref()
                    .ok_or_else(|| stored::corrupt("root"))?,
            )
            .map_err(super::documents::stored_error)?;
            let mut nodes = std::collections::BTreeMap::new();
            for raw in upload.nodes.values() {
                for node in serde_json::from_str::<Vec<DocumentNode<T>>>(raw)
                    .map_err(super::documents::stored_error)?
                {
                    if nodes.insert(node.id, node).is_some() {
                        return Err(stored::conflict("blocks.node.id"));
                    }
                }
            }
            let mut blocks = Vec::with_capacity(snapshot.blocks.len());
            for block in &snapshot.blocks {
                let node = block
                    .node_id
                    .as_deref()
                    .map(|id| {
                        let id = node_id(id, "blocks.node.id")?;
                        nodes.remove(&id).ok_or_else(|| stored::corrupt("fragment"))
                    })
                    .transpose()?;
                blocks.push(DocumentBlock {
                    key: block.key.clone(),
                    parent: block.parent.clone(),
                    kind: block.kind.clone(),
                    start_byte: block.start_byte,
                    end_byte: block.end_byte,
                    node,
                });
            }
            if !nodes.is_empty() || root.id != snapshot.document_id {
                return Err(stored::corrupt("representation identity"));
            }
            let document = DocumentWrite {
                source_id: snapshot.source_id.clone(),
                source_revision: snapshot.source_revision.clone(),
                parser_id: snapshot.parser_id.clone(),
                content: snapshot.content.clone(),
                root,
                blocks,
            };
            self.write_document(
                document,
                Some(snapshot),
                upload.header.expected_revision.as_deref(),
                Some((
                    upload_id.into(),
                    Record::Publish {
                        upload_id: upload_id.into(),
                        batches: upload.chunks.len(),
                    },
                )),
            )
        })
    }

    pub fn abort_document_upload(&mut self, upload_id: &str) -> Result<()> {
        Self::write_operation(|write| {
            let _operation = self.enter_operation()?;
            self.check_transaction_admission()?;
            write.phase = crate::error::WritePhase::Validation;
            {
                let mt = read_or_recover(&self.memtable);
                let upload = mt.document_publications.get(upload_id)?;
                if visible(&mt, upload)? {
                    return Err(stored::conflict("upload_id"));
                }
            }
            self.commit_document_record(Record::Forget {
                upload_id: upload_id.into(),
            })
        })
    }

    pub fn put_parsed_document(
        &mut self,
        input: ParsedDocumentWrite<T>,
        expected_revision: Option<&str>,
    ) -> Result<DocumentSnapshot> {
        Self::write_operation(|write| {
            let _operation = self.enter_operation()?;
            self.check_transaction_admission()?;
            write.phase = crate::error::WritePhase::Validation;
            let extra =
                crate::storage::payload_store::serialized_json_len(&input)?.saturating_mul(32);
            self.ensure_incremental_memory_budget(extra)?;
            let _workspace = self.resource_runtime.reserve_workspace(extra)?;
            input.parsed.validate()?;
            let mut representations = std::collections::BTreeMap::new();
            for representation in input.representations {
                if representations
                    .insert(representation.key, representation.node)
                    .is_some()
                {
                    return Err(stored::conflict("representations.key"));
                }
            }
            let mut blocks = Vec::with_capacity(input.parsed.blocks.len());
            for block in input.parsed.blocks {
                let node = representations.remove(&block.key);
                blocks.push(DocumentBlock {
                    key: block.key,
                    parent: block.parent,
                    kind: block.kind,
                    start_byte: block.start_byte,
                    end_byte: block.end_byte,
                    node,
                });
            }
            if !representations.is_empty() {
                return Err(invalid(
                    "representations.key",
                    "表示对应的逻辑块不存在 (Representation logical block does not exist)",
                ));
            }
            self.stage_document(
                DocumentWrite {
                    source_id: input.parsed.source_id,
                    source_revision: input.parsed.source_revision,
                    parser_id: input.parsed.parser_id,
                    content: input.parsed.text,
                    root: input.root,
                    blocks,
                },
                Some(input.parsed.source),
                expected_revision,
            )
        })
    }

    pub fn ingest_document(
        &mut self,
        input: &DocumentParseInput,
        root: DocumentNode<T>,
        representations: Vec<DocumentRepresentation<T>>,
        expected_revision: Option<&str>,
    ) -> Result<DocumentSnapshot> {
        Self::write_operation(|write| {
            let _operation = self.enter_operation()?;
            self.check_transaction_admission()?;
            write.phase = crate::error::WritePhase::Validation;
            let parsed = self.parse_document(input)?;
            self.put_parsed_document(
                ParsedDocumentWrite {
                    parsed,
                    root,
                    representations,
                },
                expected_revision,
            )
        })
    }

    pub(crate) fn stage_document(
        &mut self,
        document: DocumentWrite<T>,
        source: Option<DocumentSource>,
        expected_revision: Option<&str>,
    ) -> Result<DocumentSnapshot> {
        Self::write_operation(|write| {
            let _operation = self.enter_operation()?;
            self.check_transaction_admission()?;
            write.phase = crate::error::WritePhase::Validation;
            let extra = document.workspace_bytes()?.saturating_add(
                source
                    .as_ref()
                    .map(crate::storage::payload_store::serialized_json_len)
                    .transpose()?
                    .unwrap_or(0)
                    .saturating_mul(16),
            );
            self.ensure_incremental_memory_budget(extra)?;
            let _workspace = self.resource_runtime.reserve_workspace(extra)?;
            let mut snapshot = DocumentSnapshot::from_write(&document);
            snapshot.schema_version = 2;
            snapshot.source = source.clone();
            snapshot.validate()?;
            self.check_document_target(document.root.id, &document.source_id, expected_revision)?;
            let upload_id = uuid::Uuid::new_v4().to_string();
            let descriptor = source.as_ref().map(|source| DocumentSourceDescriptor {
                media_type: source.media_type.clone(),
                content_hash: source.content_hash.clone(),
                artifact_hash: source.artifact_hash.clone(),
                has_original_text: source.original_text.is_some(),
            });
            self.begin_document_upload(DocumentUploadBegin {
                upload_id: upload_id.clone(),
                source_id: document.source_id,
                source_revision: document.source_revision,
                parser_id: document.parser_id,
                content_hash: snapshot.content_hash,
                root: document.root,
                source: descriptor,
                expected_revision: expected_revision.map(str::to_owned),
            })?;
            let mut sequence = 0;
            for (original, text) in [
                (false, document.content.as_str()),
                (
                    true,
                    source
                        .as_ref()
                        .and_then(|source| source.original_text.as_deref())
                        .unwrap_or(""),
                ),
            ] {
                let mut first = 0;
                while first < text.len() {
                    let mut end = first.saturating_add(256 * 1024).min(text.len());
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    let mut batch = empty_batch(sequence);
                    if original {
                        batch.original_text = text[first..end].into();
                    } else {
                        batch.content = text[first..end].into();
                    }
                    self.append_document_upload(&upload_id, batch)?;
                    sequence += 1;
                    first = end;
                }
            }
            if let Some(source) = source {
                for mappings in source.mappings.chunks(1024) {
                    let mut batch = empty_batch(sequence);
                    batch.mappings = mappings.to_vec();
                    self.append_document_upload(&upload_id, batch)?;
                    sequence += 1;
                }
            }
            let mut batch = empty_batch(sequence);
            let mut bytes = 0usize;
            for block in document.blocks {
                let size = crate::storage::payload_store::serialized_json_len(&block)?;
                if !batch.blocks.is_empty()
                    && (bytes.saturating_add(size) > 4 * 1024 * 1024 || batch.blocks.len() >= 512)
                {
                    self.append_document_upload(&upload_id, batch)?;
                    sequence += 1;
                    batch = empty_batch(sequence);
                    bytes = 0;
                }
                bytes = bytes.saturating_add(size);
                batch.blocks.push(block);
            }
            if !batch.blocks.is_empty() {
                self.append_document_upload(&upload_id, batch)?;
            }
            self.publish_document_upload(&upload_id)
        })
    }

    fn check_document_target(
        &self,
        id: NodeId,
        source_id: &str,
        expected: Option<&str>,
    ) -> Result<()> {
        {
            let mt = read_or_recover(&self.memtable);
            if mt.contains(id)
                && mt
                    .try_get_payload(id)?
                    .is_some_and(|payload| payload.get("_tdb_document").is_none())
            {
                return Err(TriviumError::NodeAlreadyExists(id));
            }
        }
        match self.get_document(id) {
            Ok(Some(snapshot)) => {
                super::documents::check_revision(&snapshot, expected)?;
                if snapshot.source_id != source_id {
                    return Err(stored::conflict("source_id"));
                }
            }
            Ok(None) if expected.is_none() => {}
            Ok(None) => return Err(TriviumError::NodeNotFound(id)),
            Err(error) => return Err(error),
        }
        Ok(())
    }
    fn commit_document_record(&mut self, record: Record) -> Result<()> {
        let size = crate::storage::payload_store::serialized_json_len(&record)?;
        budget("document_record_bytes", size, stored::MAX_RECORD_BYTES)?;
        self.ensure_incremental_memory_budget(size.saturating_mul(64))?;
        let _workspace = self
            .resource_runtime
            .reserve_workspace(size.saturating_mul(64))?;
        let record = read_or_recover(&self.memtable)
            .document_publications
            .stamp(record)?;
        let bytes = serde_json::to_string(&record).map_err(super::documents::stored_error)?;
        self.commit_ops(vec![super::transaction::TxOp::DocumentPublication {
            bytes,
        }])?;
        Ok(())
    }
}

fn visible<T: VectorType>(
    mt: &crate::storage::memtable::MemTable<T>,
    upload: &stored::Upload,
) -> Result<bool> {
    if let Some(raw) = mt.get_payload_raw(upload.header.document_id) {
        let extra = raw.len().saturating_mul(32);
        mt.check_load_allocation(extra)?;
        let _workspace = mt.resource_runtime().reserve_workspace(extra)?;
        return Ok(upload.published
            && mt
                .try_get_payload(upload.header.document_id)?
                .is_some_and(|payload| {
                    payload
                        .pointer("/_tdb_document/archive")
                        .and_then(serde_json::Value::as_str)
                        == Some(upload.header.upload_id.as_str())
                        && payload
                            .pointer("/_tdb_document/revision")
                            .and_then(serde_json::Value::as_str)
                            == Some(upload.header.revision.as_str())
                }));
    }
    Ok(false)
}

fn empty_batch<T: VectorType>(sequence: usize) -> DocumentUploadBatch<T> {
    DocumentUploadBatch {
        sequence,
        content: String::new(),
        original_text: String::new(),
        mappings: Vec::new(),
        blocks: Vec::new(),
    }
}
