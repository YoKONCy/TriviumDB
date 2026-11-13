//! 分批接入契约。批次范围使用完整解析文本的UTF-8字节偏移。

use super::{DocumentBlock, DocumentNode, DocumentSourceMapEntry};
use crate::VectorType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSourceDescriptor {
    pub media_type: String,
    pub content_hash: String,
    #[serde(default)]
    pub artifact_hash: Option<String>,
    #[serde(default)]
    pub has_original_text: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound = "")]
pub struct DocumentUploadBegin<T: VectorType> {
    /// 调用方生成UUID可在请求结果不确定时恢复同一上传。
    pub upload_id: String,
    pub source_id: String,
    pub source_revision: String,
    pub parser_id: String,
    pub content_hash: String,
    pub root: DocumentNode<T>,
    #[serde(default)]
    pub source: Option<DocumentSourceDescriptor>,
    #[serde(default)]
    pub expected_revision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound = "")]
pub struct DocumentUploadBatch<T: VectorType> {
    pub sequence: usize,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub original_text: String,
    #[serde(default)]
    pub mappings: Vec<DocumentSourceMapEntry>,
    #[serde(default)]
    pub blocks: Vec<DocumentBlock<T>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocumentUploadState {
    Pending,
    Published,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocumentUploadStatus {
    pub upload_id: String,
    #[serde(with = "super::decimal_id")]
    pub document_id: u64,
    pub revision: String,
    pub state: DocumentUploadState,
    pub next_sequence: usize,
    pub content_bytes: usize,
    pub original_bytes: usize,
    pub blocks: usize,
    pub mappings: usize,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound = "")]
pub struct DocumentRepresentation<T: VectorType> {
    pub key: String,
    pub node: DocumentNode<T>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound = "")]
pub struct ParsedDocumentWrite<T: VectorType> {
    pub parsed: super::parsing::ParsedDocument,
    pub root: DocumentNode<T>,
    #[serde(default)]
    pub representations: Vec<DocumentRepresentation<T>>,
}
