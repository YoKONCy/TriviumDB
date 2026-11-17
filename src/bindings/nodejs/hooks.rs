//! 宿主 Hook 回调、引用所有权与回调重入边界。

use super::arguments::read_string;
use super::database::TriviumDB;
use super::errors::to_napi_error;
use super::json::NodeJson;
use napi::bindgen_prelude::{FromNapiValue, Function, This};
use napi::{Env, JsObject, JsUnknown, NapiValue, ValueType, sys};
use napi_derive::napi;

struct HookReturn(Option<serde_json::Value>);

impl FromNapiValue for HookReturn {
    unsafe fn from_napi_value(env: sys::napi_env, value: sys::napi_value) -> napi::Result<Self> {
        // SAFETY: N-API 在所属 JavaScript 线程提供有效句柄，全部访问局限于本次同步调用。
        let (env, value) = unsafe {
            (
                Env::from_raw(env),
                JsUnknown::from_raw_unchecked(env, value),
            )
        };
        if value.is_promise()? {
            let promise = JsObject::try_from(value)?;
            let then: Function<'_, (napi::JsUndefined, napi::JsFunction), JsUnknown> =
                promise.get_named_property("then")?;
            // 同步拒绝异步结果，同时接收拒绝通知，避免普通 Promise 拒绝成为未处理异常。
            let rejected =
                env.create_function_from_closure("tdbRejectedHookPromise", |_| Ok(()))?;
            then.apply(promise, (env.get_undefined()?, rejected))?;
            return Err(napi::Error::from_reason(
                "Hook 必须同步返回，收到 Promise (Hook must return synchronously, received a Promise)",
            ));
        }
        if matches!(value.get_type()?, ValueType::Null | ValueType::Undefined) {
            return Ok(Self(None));
        }
        NodeJson::from_js(env, value).map(|value| Self(Some(value.0)))
    }
}

struct JsNativeHook {
    object: crate::bindings::nodejs::hook_refs::HookReference,
    owner_thread: std::thread::ThreadId,
}

impl JsNativeHook {
    fn invoke(
        &self,
        name: &str,
        value: serde_json::Value,
        ctx: &mut crate::hook::HookContext,
    ) -> Option<serde_json::Value> {
        if std::thread::current().id() != self.owner_thread {
            ctx.error = Some(format!(
                "Node Hook {name} 只能在所属 JavaScript 线程同步执行 (Node Hook {name} must run synchronously on its JavaScript owner thread)"
            ));
            return None;
        }
        let object: JsObject = match self.object.get() {
            Ok(value) => value,
            Err(error) => {
                ctx.error = Some(format!(
                    "Node Hook {name} 引用无效 (Invalid Node Hook {name} reference): {error}"
                ));
                return None;
            }
        };
        let function: Function<'_, NodeJson, HookReturn> = match object.get_named_property(name) {
            Ok(Some(value)) => value,
            Ok(None) => return None,
            Err(error) => {
                ctx.error = Some(format!(
                    "Node Hook {name} 回调读取失败 (Node Hook {name} callback lookup failed): {error}"
                ));
                return None;
            }
        };
        match function.call(value.into()) {
            Ok(result) => result.0,
            Err(error) => {
                ctx.error = Some(format!(
                    "Node Hook {name} 执行失败 (Node Hook {name} failed): {error}"
                ));
                None
            }
        }
    }

    fn hits_json(hits: &[crate::node::SearchHit]) -> serde_json::Value {
        serde_json::Value::Array(
            hits.iter()
                .map(|hit| {
                    serde_json::json!({
                        "id": hit.id.to_string(), "score": hit.score, "payload": hit.payload
                    })
                })
                .collect(),
        )
    }

    fn parse_hits(value: serde_json::Value) -> Result<Vec<crate::node::SearchHit>, String> {
        let items = value
            .as_array()
            .ok_or_else(|| "Hook 命中必须是数组 (Hook hits must be an array)".to_owned())?;
        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let invalid = |field: &str| {
                    format!("Hook 命中字段无效 (Invalid hook hit field): [{index}].{field}")
                };
                let id = item
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|id| *id != 0)
                    .ok_or_else(|| invalid("id"))?;
                let score = item
                    .get("score")
                    .and_then(serde_json::Value::as_f64)
                    .map(|value| value as f32)
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| invalid("score"))?;
                Ok(crate::node::SearchHit {
                    id,
                    score,
                    payload: item
                        .get("payload")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                })
            })
            .collect()
    }

    fn replace_hits(
        hits: &mut Vec<crate::node::SearchHit>,
        value: serde_json::Value,
        ctx: &mut crate::hook::HookContext,
        stage: &str,
    ) {
        match Self::parse_hits(value) {
            Ok(updated) => *hits = updated,
            Err(error) => ctx.error = Some(format!("Node Hook {stage}: {error}")),
        }
    }
}

impl crate::hook::SearchHook for JsNativeHook {
    fn on_pre_search(
        &self,
        query: &mut Vec<f32>,
        _config: &mut crate::database::SearchConfig,
        ctx: &mut crate::hook::HookContext,
    ) {
        if let Some(value) = self.invoke("onPreSearch", serde_json::json!(query), ctx) {
            match serde_json::from_value::<Vec<f32>>(value) {
                Ok(updated) => *query = updated,
                Err(error) => {
                    ctx.error = Some(format!(
                        "Node Hook onPreSearch 返回向量无效 (Node Hook onPreSearch returned an invalid vector): {error}"
                    ))
                }
            }
        }
    }
    fn on_custom_recall(
        &self,
        query: &[f32],
        _config: &crate::database::SearchConfig,
        ctx: &mut crate::hook::HookContext,
    ) -> Option<Vec<crate::node::SearchHit>> {
        let value = self.invoke("onCustomRecall", serde_json::json!(query), ctx)?;
        match Self::parse_hits(value) {
            Ok(hits) => Some(hits),
            Err(error) => {
                ctx.error = Some(format!("Node Hook onCustomRecall: {error}"));
                None
            }
        }
    }
    fn on_post_recall(
        &self,
        hits: &mut Vec<crate::node::SearchHit>,
        ctx: &mut crate::hook::HookContext,
    ) {
        if let Some(value) = self.invoke("onPostRecall", Self::hits_json(hits), ctx) {
            Self::replace_hits(hits, value, ctx, "onPostRecall");
        }
    }
    fn on_pre_graph_expand(
        &self,
        hits: &mut Vec<crate::node::SearchHit>,
        ctx: &mut crate::hook::HookContext,
    ) {
        if let Some(value) = self.invoke("onPreGraphExpand", Self::hits_json(hits), ctx) {
            Self::replace_hits(hits, value, ctx, "onPreGraphExpand");
        }
    }
    fn on_rerank(
        &self,
        hits: &mut Vec<crate::node::SearchHit>,
        ctx: &mut crate::hook::HookContext,
    ) -> Option<Vec<crate::node::SearchHit>> {
        let value = self.invoke("onRerank", Self::hits_json(hits), ctx)?;
        match Self::parse_hits(value) {
            Ok(updated) => Some(updated),
            Err(error) => {
                ctx.error = Some(format!("Node Hook onRerank: {error}"));
                None
            }
        }
    }
    fn on_post_search(
        &self,
        hits: &mut Vec<crate::node::SearchHit>,
        ctx: &mut crate::hook::HookContext,
    ) {
        if let Some(value) = self.invoke("onPostSearch", Self::hits_json(hits), ctx) {
            Self::replace_hits(hits, value, ctx, "onPostSearch");
        }
    }
}

#[napi]
impl TriviumDB {
    // ── Hook 管理 ──

    #[napi]
    pub fn set_hook(&mut self, env: Env, this: This<JsObject>, hook: JsObject) -> napi::Result<()> {
        dispatch!(self, mut db => db.set_hook_with(
            || Ok(JsNativeHook {
                object: crate::bindings::nodejs::hook_refs::HookReference::new(env, this, hook)?,
                owner_thread: std::thread::current().id(),
            }),
            |error| to_napi_error(env, error),
        ))
    }

    /// 加载 C/C++ 动态库作为检索管线 Hook
    ///
    /// 动态库需导出 C ABI 符号（均可选）：
    /// - `trivium_recall`: 自定义召回
    /// - `trivium_rerank`: 自定义重排序
    ///
    /// ```js
    /// db.loadFfiHook('./libmy_plugin.so')
    /// const results = db.search(queryVec)  // 自动经过 C++ Hook
    /// ```
    #[napi]
    pub fn load_ffi_hook(&mut self, env: Env, lib_path: JsUnknown) -> napi::Result<()> {
        let lib_path = read_string(env, lib_path, "libPath")?;
        let ffi_hook =
            crate::hook::FfiHook::load(&lib_path).map_err(|error| to_napi_error(env, error))?;
        dispatch!(self, mut db => db.set_hook(ffi_hook)).map_err(|error| to_napi_error(env, error))
    }

    /// 清除当前已注册的 Hook，恢复为默认的零开销 NoopHook
    #[napi]
    pub fn clear_hook(&mut self) {
        dispatch!(self, mut db => db.clear_hook());
    }
}
