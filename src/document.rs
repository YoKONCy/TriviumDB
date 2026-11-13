//! 文档来源、修订与UTF-8证据范围。结构清单复用Payload，检索表示复用普通节点。

use crate::{NodeId, Result, TriviumError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub const DOCUMENT_SCHEMA_VERSION: u32 = 1;
pub const DOCUMENT_EDGE_LABEL: &str = "tdb.document.contains";
pub(crate) const DOCUMENT_FIELD: &str = "_tdb_document";
mod publication;
pub(crate) mod source;
pub use publication::*;
pub mod parsing;
pub use source::{
    DocumentRegion, DocumentRegionUnit, DocumentSource, DocumentSourceLocation,
    DocumentSourceMapEntry, DocumentSourceSpan, SourceMapKind,
};

/// 文档或片段的真实检索表示；向量沿用数据库的维度及dtype契约。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound = "")]
pub struct DocumentNode<T: crate::VectorType> {
    #[serde(with = "decimal_id")]
    pub id: NodeId,
    #[serde(with = "vector_values")]
    pub vector: Vec<T>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// 按父级在前排列的逻辑结构；node为空时仅在结构清单中保存。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound = "")]
pub struct DocumentBlock<T: crate::VectorType> {
    pub key: String,
    #[serde(default)]
    pub parent: Option<String>,
    pub kind: String,
    pub start_byte: usize,
    pub end_byte: usize,
    #[serde(default)]
    pub node: Option<DocumentNode<T>>,
}

/// 宿主完成解析和向量生成后，提交同一来源修订的全部结构与表示。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound = "")]
pub struct DocumentWrite<T: crate::VectorType> {
    pub source_id: String,
    pub source_revision: String,
    pub parser_id: String,
    pub content: String,
    pub root: DocumentNode<T>,
    pub blocks: Vec<DocumentBlock<T>>,
}

impl<T: crate::VectorType> DocumentWrite<T> {
    pub(crate) fn workspace_bytes(&self) -> Result<usize> {
        let node_bytes = |node: &DocumentNode<T>| -> Result<usize> {
            Ok(
                crate::storage::payload_store::json_encoded_len(&node.payload)?
                    .saturating_mul(16)
                    .saturating_add(
                        node.vector
                            .len()
                            .saturating_mul(std::mem::size_of::<T>())
                            .saturating_mul(4),
                    ),
            )
        };
        let mut bytes = self
            .content
            .len()
            .saturating_add(self.source_id.len())
            .saturating_add(self.source_revision.len())
            .saturating_add(self.parser_id.len())
            .saturating_mul(64)
            .saturating_add(node_bytes(&self.root)?);
        for block in &self.blocks {
            crate::request::check_current()?;
            bytes = bytes.saturating_add(2048).saturating_add(
                block
                    .key
                    .len()
                    .saturating_add(block.kind.len())
                    .saturating_add(block.parent.as_ref().map_or(0, String::len))
                    .saturating_mul(32),
            );
            if let Some(node) = &block.node {
                bytes = bytes.saturating_add(node_bytes(node)?).saturating_add(
                    self.content
                        .get(block.start_byte..block.end_byte)
                        .map_or(0, str::len)
                        .saturating_mul(64),
                );
            }
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentBlockInfo {
    pub key: String,
    pub parent: Option<String>,
    pub kind: String,
    pub start_byte: usize,
    pub end_byte: usize,
    /// 十进制字符串保留跨语言u64身份。
    pub node_id: Option<String>,
}

/// 拥有型来源快照，关闭数据库后仍可解释其中的正文及范围。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSnapshot {
    pub schema_version: u32,
    #[serde(with = "decimal_id")]
    pub document_id: NodeId,
    pub revision: String,
    pub source_id: String,
    pub source_revision: String,
    pub parser_id: String,
    pub content_hash: String,
    pub content: String,
    pub blocks: Vec<DocumentBlockInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DocumentSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentReference {
    #[serde(with = "decimal_id")]
    pub document_id: NodeId,
    pub revision: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentEvidence {
    pub reference: DocumentReference,
    pub source_id: String,
    pub source_revision: String,
    pub parser_id: String,
    pub content_hash: String,
    pub text: String,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_spans: Vec<DocumentSourceSpan>,
}

/// 文档检索与上下文展开共享的完整性预算，零值表示零额度。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentQueryLimits {
    pub max_scan_nodes: usize,
    pub max_payload_bytes: usize,
    pub max_text_bytes: usize,
    pub max_edges: usize,
    pub max_postings: usize,
    pub max_candidates: usize,
    pub max_return_bytes: usize,
    pub max_positions: usize,
    pub max_query_terms: usize,
    pub max_position_visits: usize,
    pub max_matches: usize,
    pub max_member_values: usize,
    pub max_filter_candidates: usize,
    pub max_vector_scores: usize,
    pub max_ann_visits: usize,
}

impl Default for DocumentQueryLimits {
    fn default() -> Self {
        Self {
            max_scan_nodes: 1_000_000,
            max_payload_bytes: 256 * 1024 * 1024,
            max_text_bytes: 64 * 1024 * 1024,
            max_edges: 1_000_000,
            max_postings: 1_000_000,
            max_candidates: 100_000,
            max_return_bytes: 1024 * 1024,
            max_positions: 4_000_000,
            max_query_terms: 256,
            max_position_visits: 1_000_000,
            max_matches: 10_000,
            max_member_values: 1_000_000,
            max_filter_candidates: 100_000,
            max_vector_scores: 1_000_000,
            max_ann_visits: 1_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentSearchLevel {
    Document,
    #[default]
    Block,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentMatchMode {
    #[default]
    Any,
    Phrase,
    OrderedNear,
}

/// 字段复用具有节点表示的块类型，分别统计长度和词频。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSearchField {
    pub kind: String,
    #[serde(default = "unit_weight")]
    pub weight: f32,
}

fn unit_weight() -> f32 {
    1.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentMetadataScope {
    Document,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentMemberMode {
    Any,
    All,
}

/// 精确匹配调用方data中顶层数组的标量成员，多条条件取交集。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentMemberFilter {
    pub scope: DocumentMetadataScope,
    pub field: String,
    pub mode: DocumentMemberMode,
    pub values: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentSearchQuery {
    pub text: String,
    pub level: DocumentSearchLevel,
    pub match_mode: DocumentMatchMode,
    pub max_gap: usize,
    pub include_matches: bool,
    pub analyzer_id: Option<String>,
    /// JSON沿用十进制字符串ID，Rust使用原生NodeId。
    #[serde(with = "decimal_ids")]
    pub document_ids: Vec<NodeId>,
    pub source_ids: Vec<String>,
    pub kinds: Vec<String>,
    pub fields: Vec<DocumentSearchField>,
    pub document_filter: Option<serde_json::Value>,
    pub block_filter: Option<serde_json::Value>,
    pub members: Vec<DocumentMemberFilter>,
    #[serde(with = "revision_map")]
    pub expected_revisions: std::collections::BTreeMap<NodeId, String>,
    pub max_documents: usize,
    pub hits_per_document: usize,
    pub use_cache: bool,
    pub limits: DocumentQueryLimits,
}

impl Default for DocumentSearchQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            level: Default::default(),
            match_mode: Default::default(),
            max_gap: 0,
            include_matches: false,
            analyzer_id: None,
            document_ids: Vec::new(),
            source_ids: Vec::new(),
            kinds: Vec::new(),
            fields: Vec::new(),
            document_filter: None,
            block_filter: None,
            members: Vec::new(),
            expected_revisions: Default::default(),
            max_documents: 10,
            hits_per_document: 2,
            use_cache: true,
            limits: Default::default(),
        }
    }
}

impl DocumentSearchQuery {
    pub(crate) fn uses_positions(&self) -> bool {
        self.match_mode != DocumentMatchMode::Any || self.include_matches
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentVectorStrategy {
    #[default]
    Auto,
    Exact,
    Ann,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentVectorOptions {
    pub strategy: DocumentVectorStrategy,
    pub min_score: Option<f32>,
    pub initial_candidates: usize,
    pub candidate_limit: usize,
    pub ef_search: usize,
}

impl Default for DocumentVectorOptions {
    fn default() -> Self {
        Self {
            strategy: Default::default(),
            min_score: None,
            initial_candidates: 64,
            candidate_limit: 4096,
            ef_search: 128,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentHybridOptions {
    pub vector: DocumentVectorOptions,
    pub text_weight: f32,
}

impl Default for DocumentHybridOptions {
    fn default() -> Self {
        Self {
            vector: Default::default(),
            text_weight: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentMatchScores {
    pub vector_score: Option<f32>,
    pub vector_rank: Option<usize>,
    pub text_score: Option<f32>,
    pub text_rank: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentRecallStop {
    ExactComplete,
    GroupsSatisfied,
    CandidateLimit,
    IndexExhausted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentRetrievalInfo {
    pub hybrid: bool,
    pub strategy: DocumentVectorStrategy,
    pub approximate: bool,
    pub eligible_units: usize,
    pub vector_candidates: usize,
    pub vector_scores: usize,
    pub ann_visits: usize,
    pub ann_rounds: usize,
    pub stop: DocumentRecallStop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentMatch {
    #[serde(with = "decimal_id")]
    pub node_id: NodeId,
    pub block_key: Option<String>,
    pub kind: String,
    pub score: f32,
    pub evidence: DocumentEvidence,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<DocumentTextMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scores: Option<DocumentMatchScores>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentTokenMatch {
    pub term: String,
    /// 词项序号从当前整篇或块的零位置开始，字节范围使用来源正文坐标。
    pub position: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentTextMatch {
    pub evidence: DocumentEvidence,
    pub terms: Vec<DocumentTokenMatch>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentSearchGroup {
    #[serde(with = "decimal_id")]
    pub document_id: NodeId,
    pub revision: String,
    pub source_id: String,
    pub source_revision: String,
    pub score: f32,
    pub hits: Vec<DocumentMatch>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentSearchResult {
    #[serde(default)]
    pub analyzer_id: String,
    pub level: DocumentSearchLevel,
    pub corpus_documents: usize,
    pub corpus_units: usize,
    pub matched_units: usize,
    pub groups: Vec<DocumentSearchGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_corpora: Vec<DocumentFieldCorpus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval: Option<DocumentRetrievalInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentFieldCorpus {
    pub kind: String,
    pub weight: f32,
    pub corpus_units: usize,
    pub average_length: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentContextSeed {
    pub reference: DocumentReference,
    #[serde(default)]
    pub block_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentContextQuery {
    pub seeds: Vec<DocumentContextSeed>,
    pub before: usize,
    pub after: usize,
    pub ancestor_depth: usize,
    pub limits: DocumentQueryLimits,
}

impl Default for DocumentContextQuery {
    fn default() -> Self {
        Self {
            seeds: Vec::new(),
            before: 1,
            after: 1,
            ancestor_depth: 0,
            limits: Default::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentContext {
    #[serde(with = "decimal_id")]
    pub document_id: NodeId,
    pub revision: String,
    pub source_id: String,
    pub source_revision: String,
    pub block_keys: Vec<String>,
    pub windows: Vec<DocumentEvidence>,
}

pub(crate) fn budget(resource: &'static str, requested: usize, limit: usize) -> Result<()> {
    if requested > limit {
        return Err(TriviumError::ResourceBudgetExceeded {
            resource,
            requested,
            used: 0,
            limit,
        });
    }
    Ok(())
}

mod revision_map {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S: Serializer>(
        values: &BTreeMap<u64, String>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        values.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<u64, String>, D::Error> {
        BTreeMap::<String, String>::deserialize(deserializer)?
            .into_iter()
            .map(|(raw, revision)| {
                super::node_id(&raw, "query.expected_revisions")
                    .map(|id| (id, revision))
                    .map_err(serde::de::Error::custom)
            })
            .collect()
    }
}

pub(crate) mod decimal_ids {
    use serde::{Deserialize, Deserializer, Serializer, ser::SerializeSeq};
    pub fn serialize<S: Serializer>(ids: &[u64], serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(ids.len()))?;
        for id in ids {
            seq.serialize_element(&id.to_string())?;
        }
        seq.end()
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u64>, D::Error> {
        Vec::<String>::deserialize(deserializer)?
            .iter()
            .map(|raw| super::node_id(raw, "document_ids").map_err(serde::de::Error::custom))
            .collect()
    }
}

pub(crate) fn invalid(field: &str, reason: &str) -> TriviumError {
    TriviumError::InvalidInput(reason.into()).at_field(field)
}

pub(crate) fn content_hash(content: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(content.as_bytes()))
}

pub(crate) fn valid_id(id: NodeId, field: &str) -> Result<()> {
    if id == 0 || id == u64::MAX {
        return Err(invalid(
            field,
            "节点ID超出有效范围 (Node ID is out of range)",
        ));
    }
    Ok(())
}

pub(crate) fn node_id(raw: &str, field: &str) -> Result<NodeId> {
    let id = raw.parse().map_err(|_| {
        invalid(
            field,
            "节点ID要求十进制字符串 (Node ID requires a decimal string)",
        )
    })?;
    valid_id(id, field)?;
    if id.to_string() != raw {
        return Err(invalid(
            field,
            "节点ID要求规范十进制字符串 (Node ID requires a canonical decimal string)",
        ));
    }
    Ok(id)
}

pub(crate) fn check_range(content: &str, start: usize, end: usize, field: &str) -> Result<()> {
    if content.get(start..end).is_none() {
        return Err(invalid(
            field,
            "文档范围须为有效UTF-8半开字节范围 (Document range must be a valid UTF-8 half-open byte range)",
        ));
    }
    Ok(())
}

impl DocumentSnapshot {
    pub(crate) fn from_write<T: crate::VectorType>(input: &DocumentWrite<T>) -> Self {
        Self {
            schema_version: DOCUMENT_SCHEMA_VERSION,
            document_id: input.root.id,
            revision: uuid::Uuid::new_v4().to_string(),
            source_id: input.source_id.clone(),
            source_revision: input.source_revision.clone(),
            parser_id: input.parser_id.clone(),
            content_hash: content_hash(&input.content),
            content: input.content.clone(),
            source: None,
            blocks: input
                .blocks
                .iter()
                .map(|block| DocumentBlockInfo {
                    key: block.key.clone(),
                    parent: block.parent.clone(),
                    kind: block.kind.clone(),
                    start_byte: block.start_byte,
                    end_byte: block.end_byte,
                    node_id: block.node.as_ref().map(|node| node.id.to_string()),
                })
                .collect(),
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        valid_id(self.document_id, "document.root.id")?;
        if !(1..=2).contains(&self.schema_version) || uuid::Uuid::parse_str(&self.revision).is_err()
        {
            return Err(invalid(
                "document.revision",
                "文档结构版本或修订无效 (Invalid document schema version or revision)",
            ));
        }
        budget(
            "document_content_bytes",
            self.content.len(),
            64 * 1024 * 1024,
        )?;
        budget("document_blocks", self.blocks.len(), 100_000)?;
        if let Some(source) = &self.source {
            if self.schema_version != 2 {
                return Err(invalid(
                    "document.schema_version",
                    "解析来源要求文档结构版本2 (Parsed sources require document schema version two)",
                ));
            }
            source.validate(&self.content)?;
        }
        for (field, value) in [
            ("source_id", &self.source_id),
            ("source_revision", &self.source_revision),
            ("parser_id", &self.parser_id),
        ] {
            if value.is_empty() {
                return Err(invalid(
                    &format!("document.{field}"),
                    "文档来源身份字段不能为空 (Document source identity fields must be nonempty)",
                ));
            }
        }
        validate_blocks(&self.content, &self.blocks, Some(self.document_id))?;
        Ok(())
    }

    pub(crate) fn node_ids(&self) -> Result<HashSet<NodeId>> {
        let mut ids = HashSet::from([self.document_id]);
        for block in &self.blocks {
            if let Some(id) = &block.node_id {
                ids.insert(node_id(id, "document.blocks.node_id")?);
            }
        }
        Ok(ids)
    }

    /// 逻辑结构完整保留；图边连接最近的具有节点表示的祖先。
    pub(crate) fn edges(&self) -> Result<Vec<(NodeId, NodeId, serde_json::Value)>> {
        let mut ancestors = HashMap::<&str, NodeId>::new();
        let mut edges = Vec::new();
        for (position, block) in self.blocks.iter().enumerate() {
            let parent = match &block.parent {
                Some(key) => *ancestors.get(key.as_str()).ok_or_else(|| {
                    invalid(
                        "document.blocks.parent",
                        "文档父级缺失 (Document parent is missing)",
                    )
                })?,
                None => self.document_id,
            };
            let mut ancestor = parent;
            if let Some(raw) = &block.node_id {
                let id = node_id(raw, "document.blocks.node_id")?;
                edges.push((
                    parent,
                    id,
                    serde_json::json!({
                        "document_id": self.document_id.to_string(), "revision": self.revision,
                        "key": block.key, "parent": block.parent, "position": position,
                    }),
                ));
                ancestor = id;
            }
            ancestors.insert(&block.key, ancestor);
        }
        Ok(edges)
    }
}

pub(crate) fn validate_blocks(
    content: &str,
    input: &[DocumentBlockInfo],
    root_id: Option<NodeId>,
) -> Result<()> {
    let mut blocks = HashMap::<&str, &DocumentBlockInfo>::new();
    let mut ids = root_id.into_iter().collect::<HashSet<_>>();
    for (index, block) in input.iter().enumerate() {
        crate::request::check_current()?;
        let field = format!("/document/blocks/{index}");
        if block.key.is_empty() || block.kind.is_empty() || blocks.contains_key(block.key.as_str())
        {
            return Err(invalid(
                &field,
                "文档块的键和类型须非空且键须唯一 (Document block keys and kinds must be nonempty and keys unique)",
            ));
        }
        check_range(content, block.start_byte, block.end_byte, &field)?;
        if let Some(parent) = &block.parent {
            let parent = blocks.get(parent.as_str()).ok_or_else(|| {
                invalid(
                    &format!("{field}/parent"),
                    "文档父级须在子级之前定义 (Document parents must precede their children)",
                )
            })?;
            if block.start_byte < parent.start_byte || block.end_byte > parent.end_byte {
                return Err(invalid(
                    &field,
                    "文档子级范围超出父级 (Document child range exceeds its parent)",
                ));
            }
        }
        if let Some(raw) = &block.node_id {
            let id = node_id(raw, &format!("{field}/node/id"))?;
            if !ids.insert(id) {
                return Err(invalid(
                    &format!("{field}/node/id"),
                    "文档检索表示的节点ID重复 (Duplicate document representation node ID)",
                ));
            }
        }
        blocks.insert(&block.key, block);
    }
    Ok(())
}

pub(crate) fn block_tag(
    snapshot: &DocumentSnapshot,
    block: &DocumentBlockInfo,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": snapshot.schema_version, "document_id": snapshot.document_id.to_string(),
        "revision": snapshot.revision, "key": block.key,
        "start_byte": block.start_byte, "end_byte": block.end_byte,
    })
}

/// JSON边界统一使用十进制字符串，Rust公共类型继续使用NodeId。
pub(crate) mod decimal_id {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(id: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(id)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let raw = String::deserialize(deserializer)?;
        super::node_id(&raw, "id").map_err(serde::de::Error::custom)
    }
}

/// 公共文档JSON使用数值语义，f16保持数值且u64保持完整整数。
pub(crate) mod vector_values {
    use crate::VectorType;
    use serde::{Deserialize, Deserializer, Serializer, ser::SerializeSeq};
    pub fn serialize<T: VectorType, S: Serializer>(
        values: &[T],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(values.len()))?;
        for value in values {
            sequence
                .serialize_element(&value.to_json_number().map_err(serde::ser::Error::custom)?)?;
        }
        sequence.end()
    }
    pub fn deserialize<'de, T: VectorType, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<T>, D::Error> {
        let values = Vec::<serde_json::Number>::deserialize(deserializer)?;
        values
            .iter()
            .map(|value| T::from_json_number(value).map_err(serde::de::Error::custom))
            .collect()
    }
}
