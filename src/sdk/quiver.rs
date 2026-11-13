//! SSD运行时JSON协议。绑定仅负责传输，来源、预算、I/O与排空均由内核执行。

use crate::index::quiver::{QuIVerSearchConfig, ssd::SsdRuntimeOptions};
use crate::{Database, Result, TriviumError, VectorType};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Configure {
        #[serde(default)]
        options: SsdRuntimeOptions,
    },
    Search {
        vector: Vec<f32>,
        top_k: usize,
        ef_search: usize,
        rerank_limit: Option<usize>,
    },
    Stats {},
    Close {},
    CollectTemporary {},
}

#[derive(Serialize)]
struct Hit {
    id: String,
    score: f32,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Response {
    Stats(Option<crate::index::quiver::ssd::SsdStats>),
    Hits { hits: Vec<Hit> },
    Collected { removed: usize },
    Closed,
}

/// 统一的INT8检索协议，NodeId以十进制字符串返回，避免JavaScript数值精度损失。
pub fn request<T: VectorType + Serialize + DeserializeOwned>(
    db: &Database<T>,
    json: &str,
) -> Result<String> {
    db.check_request_admission()?;
    crate::request::check_current()?;
    let runtime = crate::database::read_or_recover(&db.memtable)
        .resource_runtime()
        .clone();
    let bytes = json
        .len()
        .checked_mul(64)
        .and_then(|bytes| bytes.checked_add(16 * 1024))
        .ok_or_else(|| TriviumError::CapacityAllocationFailed {
            reason: "SSD请求容量溢出 (SSD request capacity overflow)".into(),
        })?;
    let _workspace = runtime.reserve_workspace(bytes)?;
    let command: Command = serde_json::from_str(json).map_err(|error| {
        TriviumError::InvalidInput(format!(
            "SSD请求JSON无效 (Invalid SSD request JSON): {error}"
        ))
        .at_field("request_json")
    })?;
    let writes = matches!(
        &command,
        Command::Configure { .. } | Command::CollectTemporary {}
    );
    let mut output_workspace = None;
    let response = match command {
        Command::Configure { options } => Response::Stats(Some(db.configure_quiver_int8(options)?)),
        Command::Search {
            vector,
            top_k,
            ef_search,
            rerank_limit,
        } => {
            let rows = crate::database::read_or_recover(&db.memtable).node_count();
            let output_bytes = top_k.min(rows).checked_mul(256).ok_or_else(|| {
                TriviumError::CapacityAllocationFailed {
                    reason: "SSD响应容量溢出 (SSD response capacity overflow)".into(),
                }
            })?;
            output_workspace = Some(runtime.reserve_workspace(output_bytes)?);
            let hits = db.search_quiver_int8(
                &vector,
                &QuIVerSearchConfig {
                    top_k,
                    ef_search,
                    rerank_limit,
                },
            )?;
            Response::Hits {
                hits: hits
                    .into_iter()
                    .map(|(id, score)| Hit {
                        id: id.to_string(),
                        score,
                    })
                    .collect(),
            }
        }
        Command::Stats {} => Response::Stats(db.quiver_int8_stats()?),
        Command::Close {} => {
            db.close_quiver_int8()?;
            Response::Closed
        }
        Command::CollectTemporary {} => Response::Collected {
            removed: db.collect_quiver_temporary()?,
        },
    };
    let encode = || {
        crate::request::check_current()?;
        serde_json::to_string(&response).map_err(|error| {
            TriviumError::InvalidInput(format!(
                "SSD响应编码失败 (SSD response encoding failed): {error}"
            ))
        })
    };
    let result = encode().map_err(|error| {
        if writes {
            error.with_write_info(db.response_write_info())
        } else {
            error
        }
    });
    drop(output_workspace);
    result
}
