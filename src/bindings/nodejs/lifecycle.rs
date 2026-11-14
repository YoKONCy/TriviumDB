//! 关闭、排空、取消及上下文管理器入口。

use super::arguments::{parse_safe_usize, read_number};
use super::database::TriviumDB;
use super::errors::to_napi_error;
use super::json::{NodeJson, node_json_value};
use napi::bindgen_prelude::This;
use napi::{Env, JsObject, JsUnknown};
use napi_derive::napi;

#[napi]
impl TriviumDB {
    #[napi]
    pub fn request_shutdown(&mut self, env: Env, this: This<JsObject>) -> napi::Result<NodeJson> {
        let status =
            dispatch!(self, mut db => db.request_shutdown()).map_err(|e| to_napi_error(env, e))?;
        crate::bindings::nodejs::hook_refs::flush(env, &this)?;
        node_json_value(status).map_err(|e| napi::Error::from_reason(e.to_string()))
    }

    #[napi]
    pub fn shutdown_status(&self, env: Env, this: This<JsObject>) -> napi::Result<NodeJson> {
        crate::bindings::nodejs::hook_refs::flush(env, &this)?;
        node_json_value(dispatch!(self, db => db.shutdown_status()))
            .map_err(|e| napi::Error::from_reason(e.to_string()))
    }

    #[napi]
    pub fn close_timeout(
        &mut self,
        env: Env,
        this: This<JsObject>,
        timeout_ms: JsUnknown,
    ) -> napi::Result<bool> {
        let milliseconds =
            parse_safe_usize(env, read_number(env, timeout_ms, "timeoutMs")?, "timeoutMs")?;
        let closed = dispatch!(self, mut db => db.close_timeout(std::time::Duration::from_millis(milliseconds as u64)))
                .map_err(|e| to_napi_error(env, e))?;
        crate::bindings::nodejs::hook_refs::flush(env, &this)?;
        Ok(closed)
    }

    /// 显式关闭数据库（落盘后释放资源）
    #[napi]
    pub fn close(&mut self, env: Env, this: This<JsObject>) -> napi::Result<()> {
        let result = match dispatch!(self, mut db => db.close()) {
            Ok(()) | Err(crate::error::TriviumError::DatabaseClosed) => Ok(()),
            Err(error) => Err(to_napi_error(env, error)),
        };
        crate::bindings::nodejs::hook_refs::flush(env, &this)?;
        result
    }
}
