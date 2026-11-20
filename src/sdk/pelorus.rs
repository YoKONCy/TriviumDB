//! 双曲向量检索的共享 JSON 表示，度量和执行交给 Core。

use crate::index::pelorus::{
    PelorusConfig, PelorusHit, PelorusRuntimeOptions, WireSearchOptions, convert_query,
};
use crate::{Database, Result, TriviumError, VectorType};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
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
struct Hit {
    id: String,
    distance: f64,
    payload: Value,
}

impl From<PelorusHit> for Hit {
    fn from(hit: PelorusHit) -> Self {
        Self {
            id: hit.id.to_string(),
            distance: hit.distance,
            payload: hit.payload,
        }
    }
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
            let search = || db.search_pelorus(&vector, &options);
            let (hits, io) = if profile_io {
                let (hits, io) = super::quiver::measure_io(search);
                (hits?, Some(io))
            } else {
                (search()?, None)
            };
            let hits = hits.into_iter().map(Hit::from).collect::<Vec<_>>();
            #[derive(Serialize)]
            struct Response {
                hits: Vec<Hit>,
                #[serde(skip_serializing_if = "Option::is_none")]
                io: Option<super::quiver::QuiverIoStats>,
            }
            encode(&Response { hits, io })
        }
        Command::SearchBatch { vectors, options } => {
            let vectors = vectors
                .iter()
                .map(|v| convert_query(v))
                .collect::<Result<Vec<_>>>()?;
            let groups = db
                .search_pelorus_batch(&vectors, &options.into_core()?)?
                .into_iter()
                .map(|hits| hits.into_iter().map(Hit::from).collect::<Vec<_>>())
                .collect::<Vec<_>>();
            #[derive(Serialize)]
            struct BatchResponse {
                results: Vec<Vec<Hit>>,
            }
            encode(&BatchResponse { results: groups })
        }
    }
}

fn encode(response: &impl Serialize) -> Result<String> {
    crate::request::check_current()?;
    serde_json::to_string(response).map_err(|error| {
        TriviumError::InvalidInput(format!(
            "Pelorus 响应编码失败 (Failed to encode Pelorus response): {error}"
        ))
    })
}
