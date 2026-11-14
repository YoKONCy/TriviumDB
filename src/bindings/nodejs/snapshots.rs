//! 快照校验、恢复及数据库快照发布入口。

use super::arguments::{read_string, write_string};
use super::database::TriviumDB;
use super::errors::to_napi_error;
use super::json::{NodeJson, node_json_value};
use super::runtime::parse_runtime;
use napi::{Env, JsUnknown};
use napi_derive::napi;

#[napi]
pub fn verify_snapshot(
    env: Env,
    directory: JsUnknown,
    runtime: Option<JsUnknown>,
) -> napi::Result<NodeJson> {
    let directory = read_string(env, directory, "directory")?;
    let runtime = parse_runtime(env, runtime, None)?;
    let manifest = crate::storage::snapshot::verify_snapshot_with_runtime(&directory, &runtime)
        .map_err(|e| to_napi_error(env, e))?;
    node_json_value(manifest).map_err(|e| napi::Error::from_reason(e.to_string()))
}

#[napi]
pub fn restore_snapshot(
    env: Env,
    source: JsUnknown,
    destination: JsUnknown,
    runtime: Option<JsUnknown>,
) -> napi::Result<NodeJson> {
    let source = write_string(env, source, "source")?;
    let destination = write_string(env, destination, "destination")?;
    let runtime = parse_runtime(
        env,
        runtime,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )?;
    let manifest =
        crate::storage::snapshot::restore_snapshot_with_runtime(&source, &destination, &runtime)
            .map_err(|e| to_napi_error(env, e))?;
    node_json_value(manifest).map_err(|e| napi::Error::from_reason(e.to_string()))
}

#[napi]
impl TriviumDB {
    #[napi]
    pub fn create_snapshot(
        &mut self,
        env: Env,
        destination: JsUnknown,
        generation_id: JsUnknown,
    ) -> napi::Result<NodeJson> {
        let destination = write_string(env, destination, "destination")?;
        let generation_id = write_string(env, generation_id, "generationId")?;
        let manifest = dispatch!(self, mut db => db.create_snapshot(&destination, &generation_id))
            .map_err(|e| to_napi_error(env, e))?;
        node_json_value(manifest).map_err(|e| napi::Error::from_reason(e.to_string()))
    }
}
