//! 命名图JSON传输，成员、边、修订和事务语义由Core提供。

use crate::named_graph::*;
use crate::{Database, Result, TriviumError, VectorType};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Put {
        graph: NamedGraphWrite,
        expected_revision: Option<String>,
    },
    Edit {
        graph_id: String,
        expected_revision: String,
        edits: Vec<NamedGraphEdit>,
    },
    Get {
        graph_id: String,
        #[serde(default)]
        limits: NamedGraphLimits,
    },
    List {
        #[serde(default)]
        limits: NamedGraphLimits,
    },
    Scope {
        scope: NamedGraphScope,
        #[serde(default)]
        limits: NamedGraphLimits,
    },
    Traverse {
        scope: NamedGraphScope,
        start: String,
        max_depth: usize,
        #[serde(default)]
        limits: NamedGraphLimits,
    },
    Delete {
        graph_id: String,
        expected_revision: String,
    },
}

/// ID使用规范十进制字符串，修订令牌保持原样传递。
pub fn request<T: VectorType + Serialize + DeserializeOwned>(
    db: &mut Database<T>,
    request_json: &str,
) -> Result<String> {
    db.check_request_admission()?;
    crate::request::check_current()?;
    bound(
        "named_graph_request_bytes",
        request_json.len(),
        MAX_GRAPH_BYTES,
    )?;
    let runtime = crate::database::read_or_recover(&db.memtable)
        .resource_runtime()
        .clone();
    let request_bytes = request_json.len().saturating_mul(64);
    db.ensure_incremental_memory_budget(request_bytes)?;
    let _workspace = runtime.reserve_workspace(request_bytes)?;
    #[derive(Deserialize)]
    struct Header {
        operation: String,
    }
    let decode_error = |error| {
        TriviumError::InvalidInput(format!(
            "图请求JSON无效 (Invalid graph request JSON): {error}"
        ))
        .at_field("request_json")
    };
    let header: Header = serde_json::from_str(request_json).map_err(decode_error)?;
    let write = matches!(header.operation.as_str(), "put" | "edit" | "delete");
    let mut execute = || -> Result<String> {
        if write {
            db.check_transaction_admission()?;
        }
        let command: Command = serde_json::from_str(request_json).map_err(decode_error)?;
        match command {
            Command::Put {
                graph,
                expected_revision,
            } => {
                let snapshot = db.put_named_graph(graph, expected_revision.as_deref())?;
                encode(db, &snapshot)
                    .map_err(|error| error.with_write_info(db.response_write_info()))
            }
            Command::Edit {
                graph_id,
                expected_revision,
                edits,
            } => {
                let snapshot = db.edit_named_graph(&graph_id, &expected_revision, edits)?;
                encode(db, &snapshot)
                    .map_err(|error| error.with_write_info(db.response_write_info()))
            }
            Command::Get { graph_id, limits } => {
                encode(db, &db.get_named_graph(&graph_id, &limits)?)
            }
            Command::List { limits } => encode(db, &db.list_named_graphs(&limits)?),
            Command::Scope { scope, limits } => encode(db, &db.named_graph_scope(&scope, &limits)?),
            Command::Traverse {
                scope,
                start,
                max_depth,
                limits,
            } => encode(
                db,
                &db.traverse_named_graph(
                    &scope,
                    crate::document::node_id(&start, "start")?,
                    max_depth,
                    &limits,
                )?,
            ),
            Command::Delete {
                graph_id,
                expected_revision,
            } => {
                db.delete_named_graph(&graph_id, &expected_revision)?;
                Ok("{\"deleted\":true}".into())
            }
        }
    };
    let result = execute();
    if write {
        result.map_err(|error| {
            super::errors::direct_write_error(error.with_write_info(crate::error::WriteErrorInfo {
                phase: crate::error::WritePhase::Validation,
                ..crate::error::WriteErrorInfo::MAINTENANCE_ADMISSION
            }))
        })
    } else {
        result
    }
}

fn encode<T: VectorType + Serialize + DeserializeOwned, V: Serialize>(
    db: &Database<T>,
    value: &V,
) -> Result<String> {
    crate::request::check_current()?;
    let bytes = crate::storage::payload_store::serialized_json_len(value)?;
    db.ensure_incremental_memory_budget(bytes.saturating_mul(2))?;
    let mt = crate::database::read_or_recover(&db.memtable);
    mt.check_load_allocation(bytes)?;
    let _workspace = mt
        .resource_runtime()
        .reserve_workspace(bytes.saturating_mul(2))?;
    serde_json::to_string(value).map_err(|error| {
        TriviumError::InvalidInput(format!(
            "图响应编码失败 (Graph response encoding failed): {error}"
        ))
    })
}
