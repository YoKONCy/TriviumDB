//! 双曲向量检索的共享 JSON 表示，度量和执行交给 Core。

use crate::index::pelorus::{
    PelorusConfig, PelorusHit, PelorusRuntimeOptions, WireSearchOptions, convert_query,
};
use crate::resources::WorkspaceReservation;
use crate::{Database, Result, TriviumError, VectorType};
use serde::{Deserialize, Serialize, Serializer, de::DeserializeOwned};
use serde_json::{Number, Value};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Build {
        #[serde(default)]
        config: PelorusConfig,
    },
    Configure {
        #[serde(default)]
        options: PelorusRuntimeOptions,
    },
    Search {
        vector: Vec<Number>,
        #[serde(default)]
        options: WireSearchOptions,
        #[serde(default)]
        profile_io: bool,
    },
    SearchBatch {
        vectors: Vec<Vec<Number>>,
        #[serde(default)]
        options: WireSearchOptions,
    },
}

#[derive(Serialize)]
struct WireHit<'a> {
    #[serde(serialize_with = "serialize_id")]
    id: u64,
    distance: f64,
    payload: &'a Value,
}

fn serialize_id<S: Serializer>(id: &u64, serializer: S) -> std::result::Result<S::Ok, S::Error> {
    serializer.collect_str(id)
}

struct WireRows<'a>(&'a [PelorusHit]);

impl Serialize for WireRows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(|hit| WireHit {
            id: hit.id,
            distance: hit.distance,
            payload: &hit.payload,
        }))
    }
}

fn serialize_groups<S: Serializer>(
    groups: &[Vec<PelorusHit>],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.collect_seq(groups.iter().map(|hits| WireRows(hits)))
}

pub fn request<T: VectorType + Serialize + DeserializeOwned>(
    db: &Database<T>,
    json: &str,
) -> Result<String> {
    db.check_request_admission()?;
    crate::request::check_current()?;
    let runtime = crate::database::read_or_recover(&db.memtable)
        .resource_runtime()
        .clone();
    let _workspace = runtime.reserve_workspace(json.len().saturating_mul(32))?;
    let command = serde_json::from_str(json).map_err(|error| {
        TriviumError::InvalidInput(format!(
            "Pelorus 请求无效 (Invalid Pelorus request): {error}"
        ))
        .at_field("request_json")
    })?;
    match command {
        Command::Build { config } => {
            db.build_pelorus_index(config)?;
            Ok("{}".to_owned())
        }
        Command::Configure { options } => {
            db.configure_pelorus_runtime(options)?;
            Ok("{}".to_owned())
        }
        Command::Search {
            vector,
            options,
            profile_io,
        } => {
            let vector = convert_query(&vector)?;
            let options = options.into_core()?;
            let search = || db.search_pelorus_with_workspace(&vector, &options);
            let ((hits, workspace), io) = if profile_io {
                let (hits, io) = super::quiver::measure_io(search);
                (hits?, Some(io))
            } else {
                (search()?, None)
            };
            #[derive(Serialize)]
            struct Response<'a> {
                hits: WireRows<'a>,
                #[serde(skip_serializing_if = "Option::is_none")]
                io: Option<super::quiver::QuiverIoStats>,
            }
            encode(
                db,
                workspace,
                &Response {
                    hits: WireRows(&hits),
                    io,
                },
            )
        }
        Command::SearchBatch { vectors, options } => {
            let vectors = vectors
                .iter()
                .map(|v| convert_query(v))
                .collect::<Result<Vec<_>>>()?;
            let (groups, workspace) =
                db.search_pelorus_batch_with_workspace(&vectors, &options.into_core()?)?;
            #[derive(Serialize)]
            struct BatchResponse<'a> {
                #[serde(serialize_with = "serialize_groups")]
                results: &'a [Vec<PelorusHit>],
            }
            encode(db, workspace, &BatchResponse { results: &groups })
        }
    }
}

fn encode<T: VectorType + Serialize + DeserializeOwned>(
    db: &Database<T>,
    mut workspace: WorkspaceReservation,
    response: &impl Serialize,
) -> Result<String> {
    crate::request::check_current()?;
    let bytes = crate::storage::payload_store::serialized_json_len(response)?;
    crate::request::check_current()?;
    let peak = workspace.bytes().saturating_add(bytes);
    db.ensure_incremental_memory_budget(peak)?;
    workspace.resize(peak)?;
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(bytes)
        .map_err(|error| TriviumError::CapacityAllocationFailed {
            reason: format!("Pelorus 响应分配失败 (Pelorus response allocation failed): {error}"),
        })?;
    serde_json::to_writer(&mut buffer, response).map_err(|error| {
        TriviumError::InvalidInput(format!(
            "Pelorus 响应编码失败 (Failed to encode Pelorus response): {error}"
        ))
    })?;
    crate::request::check_current()?;
    String::from_utf8(buffer).map_err(|error| {
        TriviumError::QueryExecution(format!(
            "Pelorus 响应不是有效 UTF-8 (Pelorus response is not valid UTF-8): {error}"
        ))
    })
}
