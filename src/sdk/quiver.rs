//! 压缩冷热运行时 JSON 协议。绑定负责传输，来源、预算、I/O 与排空由内核执行。

use crate::index::quiver::{QuIVerSearchConfig, runtime::QuiverRuntimeOptions};
use crate::{Database, Result, TriviumError, VectorType};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub use crate::storage::quiver_io::QuiverIoStats;

/// 计量一次同步请求的压缩 QuIVer 冷读。即使操作失败，仍返回已完成的计量。
pub fn measure_io<T>(operation: impl FnOnce() -> Result<T>) -> (Result<T>, QuiverIoStats) {
    crate::request::measure_quiver_io(operation)
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Configure {
        #[serde(default)]
        options: QuiverRuntimeOptions,
    },
    Search {
        vector: Vec<f32>,
        top_k: usize,
        ef_search: usize,
        rerank_limit: Option<usize>,
        #[serde(default)]
        profile_io: bool,
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
    Stats(Option<Box<crate::index::quiver::runtime::QuiverRuntimeStats>>),
    Hits {
        hits: Vec<Hit>,
        #[serde(skip_serializing_if = "Option::is_none")]
        io: Option<QuiverIoStats>,
    },
    Collected {
        removed: usize,
    },
    Closed,
}

/// 统一的压缩检索协议，NodeId以十进制字符串返回，避免JavaScript数值精度损失。
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
        Command::Configure { options } => {
            Response::Stats(Some(Box::new(db.configure_quiver(options)?)))
        }
        Command::Search {
            vector,
            top_k,
            ef_search,
            rerank_limit,
            profile_io,
        } => {
            let rows = crate::database::read_or_recover(&db.memtable).node_count();
            let output_bytes = top_k.min(rows).checked_mul(256).ok_or_else(|| {
                TriviumError::CapacityAllocationFailed {
                    reason: "SSD响应容量溢出 (SSD response capacity overflow)".into(),
                }
            })?;
            output_workspace = Some(runtime.reserve_workspace(output_bytes)?);
            let config = QuIVerSearchConfig {
                top_k,
                ef_search,
                rerank_limit,
            };
            let search = || db.search_quiver(&vector, &config);
            let (hits, io) = if profile_io {
                let (hits, io) = measure_io(search);
                (hits?, Some(io))
            } else {
                (search()?, None)
            };
            Response::Hits {
                hits: hits
                    .into_iter()
                    .map(|(id, score)| Hit {
                        id: id.to_string(),
                        score,
                    })
                    .collect(),
                io,
            }
        }
        Command::Stats {} => Response::Stats(db.quiver_stats()?.map(Box::new)),
        Command::Close {} => {
            db.close_quiver()?;
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
