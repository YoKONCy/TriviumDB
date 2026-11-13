//! 文档事实与普通节点共享事务、预算、WAL及文件生命周期。

use super::{Database, read_or_recover};
use crate::document::*;
use crate::document::{DOCUMENT_FIELD, block_tag, content_hash, invalid, node_id};
use crate::error::WritePhase;
use crate::storage::memtable::MemTable;
use crate::{NodeId, Result, TriviumError, VectorType};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

pub(super) fn stored_error(error: impl std::fmt::Display) -> TriviumError {
    TriviumError::CorruptedFile(format!(
        "文档事实不完整 (Incomplete document facts): {error}"
    ))
}

pub(super) fn read_snapshot<T: VectorType>(
    mt: &MemTable<T>,
    id: NodeId,
) -> Result<Option<DocumentSnapshot>> {
    read_snapshot_checked(mt, id, None)
}

pub(super) fn read_snapshot_checked<T: VectorType>(
    mt: &MemTable<T>,
    id: NodeId,
    mut budget: Option<&mut super::document_query::ReadBudget<'_>>,
) -> Result<Option<DocumentSnapshot>> {
    if let Some(budget) = budget.as_deref_mut() {
        budget.node(mt, id)?;
    }
    let Some(payload) = mt.try_get_payload(id)? else {
        return Ok(None);
    };
    let raw = payload.get(DOCUMENT_FIELD).ok_or_else(|| {
        invalid(
            "document_id",
            "目标节点未登记文档来源 (Target node has no document source)",
        )
    })?;
    if raw.get("key").is_some() {
        return Err(invalid(
            "document_id",
            "文档读取要求根节点ID (Document reads require the root node ID)",
        ));
    }
    let _archive_workspace;
    let snapshot: DocumentSnapshot = if let Some(archive) =
        raw.get("archive").and_then(Value::as_str)
    {
        let upload = mt
            .document_publications
            .get(archive)
            .map_err(stored_error)?;
        if !upload.published
            || raw.get("revision").and_then(Value::as_str) != Some(upload.header.revision.as_str())
            || raw.get("schema_version").and_then(Value::as_u64) != Some(2)
            || raw.get("document_id").and_then(Value::as_str) != Some(id.to_string().as_str())
        {
            return Err(stored_error(
                "文档发布指针无效 (Invalid document publication pointer)",
            ));
        }
        if let Some(budget) = budget.as_deref_mut() {
            budget.payload(mt, upload.storage_bytes(), upload.workspace_bytes())?;
        }
        mt.check_load_allocation(upload.workspace_bytes())?;
        _archive_workspace = Some(
            mt.resource_runtime()
                .reserve_workspace(upload.workspace_bytes())?,
        );
        upload.snapshot()?
    } else {
        _archive_workspace = None;
        serde_json::from_value(raw.clone()).map_err(stored_error)?
    };
    snapshot.validate().map_err(|error| {
        if matches!(
            error.underlying(),
            TriviumError::QueryCancelled | TriviumError::QueryDeadlineExceeded
        ) {
            error
        } else {
            stored_error(error)
        }
    })?;
    if snapshot.document_id != id || snapshot.content_hash != content_hash(&snapshot.content) {
        return Err(stored_error(
            "来源身份或正文摘要不匹配 (Source identity or content hash mismatch)",
        ));
    }
    for block in &snapshot.blocks {
        crate::request::check_current()?;
        if let Some(raw) = &block.node_id {
            let id = node_id(raw, "document.blocks.node_id").map_err(stored_error)?;
            if let Some(budget) = budget.as_deref_mut() {
                budget.node(mt, id).map_err(|error| match error {
                    TriviumError::NodeNotFound(_) => {
                        stored_error("片段节点缺失 (Fragment node is missing)")
                    }
                    error => error,
                })?;
            }
            let payload = mt
                .try_get_payload(id)?
                .ok_or_else(|| stored_error("片段节点缺失 (Fragment node is missing)"))?;
            if payload.get(DOCUMENT_FIELD) != Some(&block_tag(&snapshot, block))
                || payload.get("text").and_then(Value::as_str)
                    != snapshot.content.get(block.start_byte..block.end_byte)
            {
                return Err(stored_error(
                    "片段修订或正文不匹配 (Fragment revision or text mismatch)",
                ));
            }
        }
    }
    let mut expected = snapshot
        .edges()?
        .into_iter()
        .map(|(src, dst, metadata)| ((src, dst), metadata))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut ids = snapshot.node_ids()?.into_iter().collect::<Vec<_>>();
    ids.sort_unstable();
    // 每个邻接表只遍历一次，目录包含大量子级时保持线性边访问。
    for id in ids {
        let edges = mt.get_edges(id).unwrap_or(&[]);
        if let Some(budget) = budget.as_deref_mut() {
            budget.edges(edges.len())?;
        }
        for edge in edges {
            crate::request::check_current()?;
            if edge.label == DOCUMENT_EDGE_LABEL
                && (edge.weight != 1.
                    || expected.remove(&(id, edge.target_id)).as_ref() != Some(&edge.metadata))
            {
                return Err(stored_error(
                    "文档结构边内容不匹配 (Document hierarchy edge content mismatch)",
                ));
            }
        }
    }
    if !expected.is_empty() {
        return Err(stored_error(
            "文档结构边集合不匹配 (Document hierarchy edge set mismatch)",
        ));
    }
    Ok(Some(snapshot))
}

pub(super) fn check_revision(snapshot: &DocumentSnapshot, expected: Option<&str>) -> Result<()> {
    if expected != Some(snapshot.revision.as_str()) {
        return Err(TriviumError::DocumentRevisionMismatch {
            document_id: snapshot.document_id,
            expected: expected.unwrap_or("absent").into(),
            actual: snapshot.revision.clone(),
        }
        .at_field("expected_revision"));
    }
    Ok(())
}

impl<T: VectorType + Serialize + DeserializeOwned> Database<T> {
    /// 以预期修订原子替换整个文档。None要求目标文档尚未创建。
    pub fn put_document(
        &mut self,
        document: DocumentWrite<T>,
        expected_revision: Option<&str>,
    ) -> Result<DocumentSnapshot> {
        Self::write_operation(|write| {
            let _operation = self.enter_operation()?;
            self.check_transaction_admission()?;
            write.phase = crate::error::WritePhase::Validation;

            if document.content.len() > 4 * 1024 * 1024
                || document.blocks.len() > 4096
                || crate::storage::payload_store::serialized_json_len(&document)? > 4 * 1024 * 1024
            {
                return self.stage_document(document, None, expected_revision);
            }
            self.write_document(document, None, expected_revision, None)
        })
    }

    pub(crate) fn write_document(
        &mut self,
        document: DocumentWrite<T>,
        prepared: Option<DocumentSnapshot>,
        expected_revision: Option<&str>,
        publication: Option<(String, crate::storage::document_publications::Change)>,
    ) -> Result<DocumentSnapshot> {
        Self::write_operation(|write| {
            let _operation = self.enter_operation()?;
            self.check_transaction_admission()?;
            write.phase = WritePhase::Validation;
            self.clear_document_index()?;
            super::reject_hook_reentrant_access(&self.memtable)?;
            let previous_bytes = {
                let mt = read_or_recover(&self.memtable);
                let payload = mt.try_get_payload(document.root.id)?;
                let archive = payload
                    .as_ref()
                    .and_then(|payload| payload.pointer("/_tdb_document/archive"))
                    .and_then(Value::as_str)
                    .and_then(|id| mt.document_publications.uploads.get(id))
                    .map_or(0, |upload| upload.storage_bytes());
                payload
                    .map(|payload| crate::storage::payload_store::json_encoded_len(&payload))
                    .transpose()?
                    .unwrap_or(0)
                    .saturating_add(archive)
            };
            let extra = document
                .workspace_bytes()?
                .saturating_add(previous_bytes.saturating_mul(16));
            self.ensure_incremental_memory_budget(extra)?;
            let _workspace = self.resource_runtime.reserve_workspace(extra)?;
            let snapshot = prepared.unwrap_or_else(|| DocumentSnapshot::from_write(&document));
            snapshot.validate()?;
            let mt = read_or_recover(&self.memtable);
            let previous = if mt.contains(document.root.id) {
                if mt
                    .try_get_payload(document.root.id)?
                    .is_some_and(|payload| payload.get(DOCUMENT_FIELD).is_none())
                {
                    return Err(TriviumError::NodeAlreadyExists(document.root.id));
                }
                let previous = read_snapshot(&mt, document.root.id)?
                    .ok_or(TriviumError::NodeNotFound(document.root.id))?;
                check_revision(&previous, expected_revision)?;
                if previous.source_id != document.source_id {
                    return Err(invalid(
                        "document.source_id",
                        "文档更新须保留来源身份 (Document updates must preserve source identity)",
                    ));
                }
                Some(previous)
            } else {
                if expected_revision.is_some() {
                    return Err(TriviumError::NodeNotFound(document.root.id));
                }
                None
            };
            let old_ids = previous
                .as_ref()
                .map(DocumentSnapshot::node_ids)
                .transpose()?
                .unwrap_or_default();
            let new_ids = snapshot.node_ids()?;
            mt.validate_insert(&document.root.vector)
                .map_err(|error| error.at_field("document.root.vector"))?;
            for (index, block) in document.blocks.iter().enumerate() {
                if let Some(node) = &block.node {
                    mt.validate_insert(&node.vector).map_err(|error| {
                        error.at_field(&format!("/document/blocks/{index}/node/vector"))
                    })?;
                }
            }
            let mut checked_ids = new_ids.iter().copied().collect::<Vec<_>>();
            checked_ids.sort_unstable();
            for id in &checked_ids {
                if mt.contains(*id) && !old_ids.contains(id) {
                    return Err(TriviumError::NodeAlreadyExists(*id));
                }
            }
            let mut operations = Vec::new();
            if let Some(previous) = &previous {
                for (src, dst, _) in previous.edges()? {
                    operations.push(super::transaction::TxOp::UnlinkLabel {
                        src,
                        dst,
                        label: DOCUMENT_EDGE_LABEL.into(),
                    });
                }
                let mut removed = old_ids.difference(&new_ids).copied().collect::<Vec<_>>();
                removed.sort_unstable();
                for id in removed {
                    operations.push(super::transaction::TxOp::Delete { id });
                }
            }
            let root_tag = if let Some((archive, _)) = &publication {
                json!({"schema_version":2,"document_id":snapshot.document_id.to_string(),"revision":snapshot.revision,"archive":archive})
            } else {
                serde_json::to_value(&snapshot).map_err(stored_error)?
            };
            let previous_archive = mt.try_get_payload(document.root.id)?.and_then(|payload| {
                payload
                    .get(DOCUMENT_FIELD)
                    .and_then(|tag| tag.get("archive"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            if publication.is_none()
                && let Some(upload_id) = previous_archive
            {
                operations.push(super::transaction::TxOp::DocumentPublication {
                    bytes: serde_json::to_string(&mt.document_publications.stamp(
                        crate::storage::document_publications::Change::Forget { upload_id },
                    )?)
                    .map_err(stored_error)?,
                });
            }
            let root_payload = json!({DOCUMENT_FIELD: root_tag, "data": document.root.payload});
            append_node(
                &mut operations,
                &old_ids,
                &mt,
                document.root.id,
                document.root.vector,
                root_payload,
            );
            for (block, info) in document.blocks.into_iter().zip(&snapshot.blocks) {
                crate::request::check_current()?;
                if let Some(node) = block.node {
                    let text = snapshot
                        .content
                        .get(info.start_byte..info.end_byte)
                        .ok_or_else(|| {
                            invalid("document.blocks", "文档范围无效 (Invalid document range)")
                        })?;
                    let payload = json!({DOCUMENT_FIELD: block_tag(&snapshot, info), "text": text, "data": node.payload});
                    append_node(
                        &mut operations,
                        &old_ids,
                        &mt,
                        node.id,
                        node.vector,
                        payload,
                    );
                }
            }
            for (src, dst, metadata) in snapshot.edges()? {
                operations.push(super::transaction::TxOp::Link {
                    src,
                    dst,
                    label: DOCUMENT_EDGE_LABEL.into(),
                    weight: 1.,
                    metadata,
                });
            }
            if let Some((_, record)) = publication {
                operations.push(super::transaction::TxOp::DocumentPublication {
                    bytes: serde_json::to_string(&mt.document_publications.stamp(record)?)
                        .map_err(stored_error)?,
                });
            }
            drop(mt);
            // &mut Database隔离事实写入；后台维护仅重排存储，事务复用既有WAL前控制检查。
            self.commit_ops(operations)?;
            Ok(snapshot)
        })
    }

    /// 在一个事实读锁内验证来源、片段和完整结构边，返回拥有型快照。
    pub fn get_document(&self, id: NodeId) -> Result<Option<DocumentSnapshot>> {
        self.read_operation(|| {
            crate::document::valid_id(id, "document_id")?;
            super::reject_hook_reentrant_access(&self.memtable)?;
            let mt = read_or_recover(&self.memtable);
            // 在克隆和解析清单前，使用实际Payload尺寸预留工作区。
            let bytes = match mt.get_payload_raw(id) {
                Some(payload) => payload.len(),
                None => return Ok(None),
            };
            let extra = bytes.saturating_mul(32).saturating_add(4096);
            mt.ensure_additional_memory(self.memory_limit, extra)?;
            let _workspace = self.resource_runtime.reserve_workspace(extra)?;
            read_snapshot(&mt, id)
        })?
    }

    /// 仅在来源修订匹配时解释引用；范围单位为原始UTF-8字节。
    pub fn resolve_document_reference(
        &self,
        reference: DocumentReference,
    ) -> Result<DocumentEvidence> {
        let _operation = self.enter_operation()?;
        let snapshot = self
            .get_document(reference.document_id)?
            .ok_or(TriviumError::NodeNotFound(reference.document_id))?;
        check_revision(&snapshot, Some(&reference.revision))
            .map_err(|error| error.at_field("reference.revision"))?;
        crate::request::check_current()?;
        let extra = snapshot
            .content
            .len()
            .saturating_mul(2)
            .saturating_add(snapshot.source_id.len())
            .saturating_add(snapshot.source_revision.len())
            .saturating_add(snapshot.parser_id.len())
            .saturating_add(
                snapshot
                    .source
                    .as_ref()
                    .map(|source| {
                        source.span_charge(
                            &snapshot.content,
                            reference.start_byte,
                            reference.end_byte,
                        )
                    })
                    .transpose()?
                    .unwrap_or(0)
                    .saturating_mul(4),
            );
        self.ensure_incremental_memory_budget(extra)?;
        let _workspace = self.resource_runtime.reserve_workspace(extra)?;
        super::document_query::evidence(&snapshot, reference.start_byte, reference.end_byte)
    }

    /// 删除文档拥有的表示节点及相连边；修订检查与整个删除批次复用单写事务。
    pub fn delete_document(&mut self, id: NodeId, expected_revision: &str) -> Result<()> {
        Self::write_operation(|write| {
            let _operation = self.enter_operation()?;
            self.check_transaction_admission()?;
            write.phase = WritePhase::Validation;
            self.clear_document_index()?;
            let snapshot = self
                .get_document(id)?
                .ok_or(TriviumError::NodeNotFound(id))?;
            check_revision(&snapshot, Some(expected_revision))?;
            let extra =
                crate::storage::payload_store::serialized_json_len(&snapshot)?.saturating_mul(16);
            self.ensure_incremental_memory_budget(extra)?;
            let _workspace = self.resource_runtime.reserve_workspace(extra)?;
            let mut ids = snapshot.node_ids()?.into_iter().collect::<Vec<_>>();
            ids.sort_unstable();
            let operations = ids
                .into_iter()
                .map(|id| super::transaction::TxOp::Delete { id })
                .collect();
            self.commit_ops(operations)?;
            Ok(())
        })
    }
}

fn append_node<T: VectorType>(
    operations: &mut Vec<super::transaction::TxOp<T>>,
    old_ids: &std::collections::HashSet<NodeId>,
    mt: &MemTable<T>,
    id: NodeId,
    vector: Vec<T>,
    payload: Value,
) {
    use super::transaction::TxOp;
    if old_ids.contains(&id) {
        operations.push(TxOp::UpdatePayload { id, payload });
        if mt.get_vector(id).is_none_or(|previous| {
            bytemuck::cast_slice::<T, u8>(previous) != bytemuck::cast_slice::<T, u8>(&vector)
        }) {
            operations.push(TxOp::UpdateVector { id, vector });
        }
    } else {
        operations.push(TxOp::InsertWithId {
            id,
            vector,
            payload,
        });
    }
}
