//! 文档JSON适配。来源、位置、事务和修订判定由存储内核提供。

use crate::document::{
    DocumentContextQuery, DocumentHybridOptions, DocumentReference, DocumentSearchQuery,
    DocumentVectorOptions, DocumentWrite,
};
use crate::{Database, Result, TriviumError, VectorType};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    deny_unknown_fields,
    bound = ""
)]
enum Command<T: VectorType> {
    Parse {
        input: crate::document::parsing::DocumentParseInput,
    },
    PutParsed {
        document: crate::document::ParsedDocumentWrite<T>,
        expected_revision: Option<String>,
    },
    Ingest {
        input: crate::document::parsing::DocumentParseInput,
        root: crate::document::DocumentNode<T>,
        #[serde(default)]
        representations: Vec<crate::document::DocumentRepresentation<T>>,
        expected_revision: Option<String>,
    },
    BeginUpload {
        upload: crate::document::DocumentUploadBegin<T>,
    },
    AppendUpload {
        upload_id: String,
        batch: crate::document::DocumentUploadBatch<T>,
    },
    UploadStatus {
        upload_id: String,
    },
    ListUploads {
        document_id: Option<String>,
    },
    PublishUpload {
        upload_id: String,
    },
    AbortUpload {
        upload_id: String,
    },
    Put {
        document: DocumentWrite<T>,
        expected_revision: Option<String>,
    },
    Get {
        document_id: String,
    },
    Search {
        query: DocumentSearchQuery,
    },
    SearchVector {
        query: DocumentSearchQuery,
        #[serde(with = "crate::document::vector_values")]
        vector: Vec<T>,
        #[serde(default)]
        options: DocumentVectorOptions,
    },
    SearchHybrid {
        query: DocumentSearchQuery,
        #[serde(with = "crate::document::vector_values")]
        vector: Vec<T>,
        #[serde(default)]
        options: DocumentHybridOptions,
    },
    Context {
        query: DocumentContextQuery,
    },
    ClearIndex,
    Resolve {
        reference: DocumentReference,
    },
    Delete {
        document_id: String,
        expected_revision: String,
    },
}

/// 三端共享JSON入口。所有NodeId以规范十进制字符串表示。
pub fn request<T: VectorType + Serialize + DeserializeOwned>(
    db: &mut Database<T>,
    request_json: &str,
) -> Result<String> {
    db.check_request_admission()?;
    crate::request::check_current()?;
    let runtime = crate::database::read_or_recover(&db.memtable)
        .resource_runtime()
        .clone();
    let _request_workspace = runtime.reserve_workspace(request_json.len().saturating_mul(64))?;
    #[derive(Deserialize)]
    struct Header {
        operation: String,
    }
    let header: Header = serde_json::from_str(request_json).map_err(|error| {
        TriviumError::InvalidInput(format!(
            "文档请求JSON无效 (Invalid document request JSON): {error}"
        ))
        .at_field("request_json")
    })?;
    let is_write = matches!(
        header.operation.as_str(),
        "put"
            | "delete"
            | "put_parsed"
            | "ingest"
            | "begin_upload"
            | "append_upload"
            | "publish_upload"
            | "abort_upload"
    );
    if is_write {
        db.check_transaction_admission().map_err(|error| {
            error.with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_ADMISSION)
        })?;
    }
    let result = (|| {
        if is_write || matches!(header.operation.as_str(), "search_vector" | "search_hybrid") {
            let value: serde_json::Value = serde_json::from_str(request_json).map_err(|error| {
                crate::document::invalid(
                    "request_json",
                    &format!("文档请求JSON无效 (Invalid document request JSON): {error}"),
                )
            })?;
            for path in [
                "/vector",
                "/document/root/vector",
                "/root/vector",
                "/upload/root/vector",
            ] {
                if let Some(vector) = value.pointer(path) {
                    validate_vector::<T>(vector, path)?;
                }
            }
            for path in [
                "/document/blocks",
                "/batch/blocks",
                "/document/representations",
                "/representations",
            ] {
                if let Some(blocks) = value.pointer(path).and_then(serde_json::Value::as_array) {
                    for (index, block) in blocks.iter().enumerate() {
                        if let Some(vector) = block.pointer("/node/vector") {
                            validate_vector::<T>(vector, &format!("{path}/{index}/node/vector"))?;
                        }
                    }
                }
            }
        }
        let command: Command<T> = serde_json::from_str(request_json).map_err(|error| {
            TriviumError::InvalidInput(format!(
                "文档请求JSON无效 (Invalid document request JSON): {error}"
            ))
            .at_field("request_json")
        })?;
        match command {
            Command::Parse { input } => encode(db, &db.parse_document(&input)?),
            Command::PutParsed {
                document,
                expected_revision,
            } => {
                let result = db
                    .put_parsed_document(document, expected_revision.as_deref())
                    .map_err(super::errors::direct_write_error)?;
                encode(db, &result).map_err(|error| error.with_write_info(db.response_write_info()))
            }
            Command::Ingest {
                input,
                root,
                representations,
                expected_revision,
            } => {
                let result = db
                    .ingest_document(&input, root, representations, expected_revision.as_deref())
                    .map_err(super::errors::direct_write_error)?;
                encode(db, &result).map_err(|error| error.with_write_info(db.response_write_info()))
            }
            Command::BeginUpload { upload } => {
                let result = db
                    .begin_document_upload(upload)
                    .map_err(super::errors::direct_write_error)?;
                encode(db, &result).map_err(|error| error.with_write_info(db.response_write_info()))
            }
            Command::AppendUpload { upload_id, batch } => {
                let result = db
                    .append_document_upload(&upload_id, batch)
                    .map_err(super::errors::direct_write_error)?;
                encode(db, &result).map_err(|error| error.with_write_info(db.response_write_info()))
            }
            Command::UploadStatus { upload_id } => {
                encode(db, &db.document_upload_status(&upload_id)?)
            }
            Command::ListUploads { document_id } => encode(
                db,
                &db.list_document_uploads(
                    document_id
                        .as_deref()
                        .map(|id| crate::document::node_id(id, "document_id"))
                        .transpose()?,
                )?,
            ),
            Command::PublishUpload { upload_id } => {
                let result = db
                    .publish_document_upload(&upload_id)
                    .map_err(super::errors::direct_write_error)?;
                encode(db, &result).map_err(|error| error.with_write_info(db.response_write_info()))
            }
            Command::AbortUpload { upload_id } => {
                db.abort_document_upload(&upload_id)
                    .map_err(super::errors::direct_write_error)?;
                encode(db, &serde_json::json!({"aborted":true}))
                    .map_err(|error| error.with_write_info(db.response_write_info()))
            }

            Command::Put {
                document,
                expected_revision,
            } => {
                let result = db
                    .put_document(document, expected_revision.as_deref())
                    .map_err(super::errors::direct_write_error)?;
                encode(db, &result).map_err(|error| error.with_write_info(db.response_write_info()))
            }
            Command::Get { document_id } => encode(
                db,
                &db.get_document(crate::document::node_id(&document_id, "document_id")?)?,
            ),
            Command::Resolve { reference } => {
                encode(db, &db.resolve_document_reference(reference)?)
            }
            Command::Search { query } => encode(db, &db.search_documents(&query)?),
            Command::SearchVector {
                query,
                vector,
                options,
            } => encode(db, &db.search_documents_vector(&query, &vector, &options)?),
            Command::SearchHybrid {
                query,
                vector,
                options,
            } => encode(db, &db.search_documents_hybrid(&query, &vector, &options)?),
            Command::Context { query } => encode(db, &db.document_context(&query)?),
            Command::ClearIndex => {
                db.clear_document_index()?;
                Ok("{\"cleared\":true}".into())
            }
            Command::Delete {
                document_id,
                expected_revision,
            } => {
                db.delete_document(
                    crate::document::node_id(&document_id, "document_id")?,
                    &expected_revision,
                )
                .map_err(super::errors::direct_write_error)?;
                Ok("{\"deleted\":true}".into())
            }
        }
    })();
    if is_write {
        result.map_err(|error| {
            error.with_write_info(crate::error::WriteErrorInfo {
                phase: crate::error::WritePhase::Validation,
                ..crate::error::WriteErrorInfo::MAINTENANCE_ADMISSION
            })
        })
    } else {
        result
    }
}

fn validate_vector<T: VectorType>(value: &serde_json::Value, field: &str) -> Result<()> {
    let values = value.as_array().ok_or_else(|| {
        crate::document::invalid(
            field,
            "文档向量须为数值数组 (Document vector must be a numeric array)",
        )
    })?;
    for (index, value) in values.iter().enumerate() {
        crate::request::check_current()?;
        let path = format!("{field}/{index}");
        let number = value.as_number().ok_or_else(|| {
            crate::document::invalid(
                &path,
                "向量分量须为数值 (Vector components must be numeric)",
            )
        })?;
        T::from_json_number(number).map_err(|error| error.at_field(&path))?;
    }
    Ok(())
}

fn encode<T: VectorType, V: Serialize>(db: &Database<T>, value: &V) -> Result<String> {
    crate::request::check_current()?;
    let bytes = crate::storage::payload_store::serialized_json_len(value)?;
    let mt = crate::database::read_or_recover(&db.memtable);
    mt.check_load_allocation(bytes)?;
    let _workspace = mt.resource_runtime().reserve_workspace(bytes)?;
    drop(mt);
    serde_json::to_string(value).map_err(|error| {
        TriviumError::InvalidInput(format!(
            "文档响应编码失败 (Document response encoding failed): {error}"
        ))
    })
}
