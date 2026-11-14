//! 文档、图、QuIVer 和 Console 的共享 JSON 协议入口。

use super::arguments::{constructor_string, read_string};
use super::database::TriviumDB;
use super::errors::to_napi_error;
use napi::{Env, JsUnknown};
use napi_derive::napi;

/// 会话只保存策略与取消状态，数据库仍由宿主持有。
#[napi(js_name = "ConsoleSession")]
pub struct JsConsoleSession {
    inner: crate::ConsoleSession,
}

#[napi]
impl JsConsoleSession {
    #[napi(constructor)]
    pub fn new(env: Env, options_json: Option<JsUnknown>) -> napi::Result<Self> {
        let options_json = constructor_string(env, options_json, "{}", "optionsJson")?;
        crate::ConsoleSession::from_json(&options_json)
            .map(|inner| Self { inner })
            .map_err(|error| to_napi_error(env, error.at_field("optionsJson")))
    }

    #[napi]
    pub fn detach(&self) {
        self.inner.detach();
    }
    #[napi]
    pub fn is_detached(&self) -> bool {
        self.inner.is_detached()
    }
    #[napi]
    pub fn is_drained(&self) -> bool {
        self.inner.is_drained()
    }
    #[napi]
    pub fn cancel(&self, env: Env, request_id: JsUnknown) -> napi::Result<bool> {
        let request_id = read_string(env, request_id, "requestId")?;
        Ok(self.inner.cancel(&request_id))
    }
}

#[napi]
impl TriviumDB {
    /// 文档JSON适配复用Core的解析、来源映射、分批发布、修订、检索与证据上下文。
    #[napi]
    pub fn document_request(&mut self, env: Env, request_json: JsUnknown) -> napi::Result<String> {
        let request_json = read_string(env, request_json, "requestJson")?;
        dispatch!(self, mut db => crate::sdk::documents::request(db, &request_json))
            .map_err(|error| to_napi_error(env, error))
    }

    /// SSD运行时JSON适配复用Core的事实绑定、驻留、缓存与I/O生命周期。
    #[napi]
    pub fn quiver_request(&self, env: Env, request_json: JsUnknown) -> napi::Result<String> {
        let request_json = read_string(env, request_json, "requestJson")?;
        dispatch!(self, db => crate::sdk::quiver::request(db, &request_json))
            .map_err(|error| to_napi_error(env, error))
    }

    /// 命名图JSON适配复用Core的成员、独立边、修订及作用域契约。
    #[napi]
    pub fn graph_request(&mut self, env: Env, request_json: JsUnknown) -> napi::Result<String> {
        let request_json = read_string(env, request_json, "requestJson")?;
        dispatch!(self, mut db => crate::sdk::named_graphs::request(db, &request_json))
            .map_err(|error| to_napi_error(env, error))
    }

    /// 同步执行宿主控制台请求。
    #[napi]
    pub fn console_request(
        &mut self,
        env: Env,
        session: &JsConsoleSession,
        request_json: JsUnknown,
    ) -> napi::Result<String> {
        let request_json = read_string(env, request_json, "requestJson")?;
        dispatch!(self, mut db => db.console_request(&session.inner, &request_json))
            .map_err(|error| to_napi_error(env, error))
    }
}
