//! 请求上下文、调用形状与宿主执行桥接。

use super::arguments::{parse_search_usize, read_string};
use super::errors::to_napi_error;
use super::json::{NodeJson, node_json_value};
use napi::bindgen_prelude::{Either, Function};
use napi::{Env, JsUnknown};
use napi_derive::napi;

#[napi(js_name = "__callArgumentInfo")]
pub fn call_argument_info(
    env: Env,
    owner: String,
    operation: String,
    field: String,
    kind: String,
) -> napi::Result<NodeJson> {
    let info = crate::sdk::call_argument_info(&owner, &operation, &field, &kind)
        .map_err(|error| to_napi_error(env, error))?;
    node_json_value(info).map_err(|error| napi::Error::from_reason(error.to_string()))
}

#[napi(js_name = "RequestContext")]
pub struct JsRequestContext {
    inner: crate::sdk::RequestContext,
}

#[napi]
impl JsRequestContext {
    #[napi(constructor)]
    pub fn new(
        env: Env,
        request_id: JsUnknown,
        timeout_ms: Option<Either<f64, JsUnknown>>,
    ) -> napi::Result<Self> {
        let request_id = read_string(env, request_id, "requestId")?;
        let timeout_ms = parse_search_usize(env, timeout_ms, 0.0, "timeoutMs", false)?;
        crate::sdk::RequestContext::with_timeout(
            &request_id,
            std::time::Duration::from_millis(timeout_ms as u64),
            false,
        )
        .map(|inner| Self { inner })
        .map_err(|error| to_napi_error(env, error))
    }

    #[napi(getter)]
    pub fn request_id(&self) -> String {
        self.inner.request_id().to_owned()
    }
    #[napi]
    pub fn cancel(&self) {
        self.inner.cancel();
    }
    #[napi]
    pub fn check(&self, env: Env) -> napi::Result<()> {
        self.inner
            .control()
            .check()
            .map_err(|error| to_napi_error(env, error))
    }
}

#[napi(js_name = "__invokeRequest")]
pub fn invoke_request(
    env: Env,
    context: &JsRequestContext,
    owner: String,
    operation: String,
    callback: Function<(), JsUnknown>,
) -> napi::Result<JsUnknown> {
    let admission = |error| {
        to_napi_error(
            env,
            crate::sdk::request_admission(error, &owner, &operation),
        )
    };
    let _scope = context.inner.enter().map_err(admission)?;
    if operation != "console_request" {
        crate::request::check_current().map_err(admission)?;
    }
    callback.call(())
}

#[napi(js_name = "__requestOperation")]
pub fn request_operation(owner: String, operation: String) -> bool {
    crate::sdk::request_operation(&owner, &operation)
}
