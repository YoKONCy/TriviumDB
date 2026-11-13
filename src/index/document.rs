//! 从文档事实派生的文本索引，整篇与片段分别维护统计。
use crate::document::{DocumentQueryLimits, DocumentSearchLevel, budget};
use crate::index::text::TextIndex;
use crate::{NodeId, Result};
use std::collections::BTreeMap;

pub(crate) mod members;

#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentReadCost {
    pub nodes: usize,
    pub payload_bytes: usize,
    pub edges: usize,
    pub text_bytes: usize,
    pub positions: usize,
    pub member_values: usize,
}

impl DocumentReadCost {
    pub fn check(&self, limits: &DocumentQueryLimits) -> Result<()> {
        budget("document_scan_nodes", self.nodes, limits.max_scan_nodes)?;
        budget(
            "document_payload_bytes",
            self.payload_bytes,
            limits.max_payload_bytes,
        )?;
        budget("document_edges", self.edges, limits.max_edges)?;
        budget(
            "document_text_bytes",
            self.text_bytes,
            limits.max_text_bytes,
        )
    }
}

pub(crate) struct DocumentHeader {
    pub revision: String,
    pub source_id: String,
    pub source_revision: String,
}

#[derive(Clone)]
pub(crate) struct DocumentUnit {
    pub node_id: NodeId,
    pub document_id: NodeId,
    pub block_key: Option<String>,
    pub kind: String,
}

#[derive(Default)]
pub(crate) struct DocumentTextIndex {
    pub documents: BTreeMap<NodeId, DocumentHeader>,
    pub units: BTreeMap<NodeId, DocumentUnit>,
    pub document_text: TextIndex,
    pub block_text: TextIndex,
    pub positions: Option<crate::index::text::positions::PositionIndex>,
    pub field_text: Option<BTreeMap<String, TextIndex>>,
    pub members: Option<members::MemberIndex>,
    pub cost: DocumentReadCost,
    metadata_bytes: usize,
    field_text_bytes: usize,
}

impl DocumentTextIndex {
    pub fn new(positions: bool, fields: bool, members: bool) -> Self {
        Self {
            positions: positions.then(Default::default),
            field_text: fields.then(Default::default),
            members: members.then(Default::default),
            ..Default::default()
        }
    }

    pub fn text(&self, level: DocumentSearchLevel) -> &TextIndex {
        match level {
            DocumentSearchLevel::Document => &self.document_text,
            DocumentSearchLevel::Block => &self.block_text,
        }
    }
    pub fn insert_header(&mut self, id: NodeId, header: DocumentHeader) {
        self.metadata_bytes = self.metadata_bytes.saturating_add(
            std::mem::size_of::<(NodeId, DocumentHeader)>()
                .saturating_add(64)
                .saturating_add(header.revision.capacity())
                .saturating_add(header.source_id.capacity())
                .saturating_add(header.source_revision.capacity()),
        );
        self.documents.insert(id, header);
    }

    pub fn insert_unit(&mut self, unit: DocumentUnit) -> Result<()> {
        if self.units.contains_key(&unit.node_id) {
            return Err(crate::TriviumError::CorruptedFile(
                "文档表示身份重复 (Duplicate document representation identity)".into(),
            ));
        }
        self.metadata_bytes = self.metadata_bytes.saturating_add(
            std::mem::size_of::<(NodeId, DocumentUnit)>()
                .saturating_add(64)
                .saturating_add(unit.block_key.as_ref().map_or(0, String::capacity))
                .saturating_add(unit.kind.capacity()),
        );
        self.units.insert(unit.node_id, unit);
        Ok(())
    }

    pub fn add_field(
        &mut self,
        unit: &DocumentUnit,
        frequencies: std::collections::HashMap<String, usize>,
        length: usize,
    ) {
        if let Some(fields) = &mut self.field_text {
            if !fields.contains_key(&unit.kind) {
                self.field_text_bytes = self
                    .field_text_bytes
                    .saturating_add(unit.kind.len())
                    .saturating_add(std::mem::size_of::<(String, TextIndex)>() + 64);
            }
            let text = fields.entry(unit.kind.clone()).or_default();
            let before = text.estimated_memory_bytes();
            text.add_term_frequencies(unit.node_id, frequencies, length);
            self.field_text_bytes = self
                .field_text_bytes
                .saturating_sub(before)
                .saturating_add(text.estimated_memory_bytes());
        }
    }

    pub fn heap_bytes(&self) -> usize {
        self.metadata_bytes
            .saturating_add(self.document_text.estimated_memory_bytes())
            .saturating_add(self.block_text.estimated_memory_bytes())
            .saturating_add(self.field_text_bytes)
            .saturating_add(
                self.members
                    .as_ref()
                    .map_or(0, members::MemberIndex::heap_bytes),
            )
            .saturating_add(
                self.positions
                    .as_ref()
                    .map_or(0, |positions| positions.heap_bytes()),
            )
    }
}
