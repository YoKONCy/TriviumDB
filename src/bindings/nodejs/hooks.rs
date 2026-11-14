//! 宿主 Hook 回调、引用所有权与回调重入边界。

use super::arguments::read_string;
use super::database::TriviumDB;
use super::errors::to_napi_error;
use super::json::NodeJson;
use napi::bindgen_prelude::{Function, This};
use napi::{Env, JsObject, JsUnknown};
use napi_derive::napi;

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
                "Node Hook {name} 只能在注册它的 JavaScript 线程同步执行"
            ));
            return None;
        }
        let object: JsObject = match self.object.get() {
            Ok(value) => value,
            Err(error) => {
                ctx.error = Some(format!("Node Hook {name} 引用无效: {error}"));
                return None;
            }
        };
        let function: Function<'_, NodeJson, NodeJson> = match object.get_named_property(name) {
            Ok(value) => value,
            Err(_) => return None,
        };
        match function.call(value.into()) {
            Ok(result) => Some(result.0),
            Err(error) => {
                ctx.error = Some(format!("Node Hook {name} 执行失败: {error}"));
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

    fn replace_hits(hits: &mut Vec<crate::node::SearchHit>, value: serde_json::Value) {
        let Some(items) = value.as_array() else {
            return;
        };
        *hits = items
            .iter()
            .filter_map(|item| {
                Some(crate::node::SearchHit {
                    id: item.get("id")?.as_str()?.parse().ok()?,
                    score: item.get("score")?.as_f64()? as f32,
                    payload: item
                        .get("payload")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                })
            })
            .filter(|hit| hit.id != 0 && hit.score.is_finite())
            .collect();
    }
}

impl crate::hook::SearchHook for JsNativeHook {
    fn on_pre_search(
        &self,
        query: &mut Vec<f32>,
        _config: &mut crate::database::SearchConfig,
        ctx: &mut crate::hook::HookContext,
    ) {
        if let Some(value) = self.invoke("onPreSearch", serde_json::json!(query), ctx)
            && let Ok(updated) = serde_json::from_value::<Vec<f32>>(value)
        {
            *query = updated;
        }
    }
    fn on_custom_recall(
        &self,
        query: &[f32],
        _config: &crate::database::SearchConfig,
        ctx: &mut crate::hook::HookContext,
    ) -> Option<Vec<crate::node::SearchHit>> {
        let value = self.invoke("onCustomRecall", serde_json::json!(query), ctx)?;
        if value.is_null() {
            return None;
        }
        let mut hits = Vec::new();
        Self::replace_hits(&mut hits, value);
        Some(hits)
    }
    fn on_post_recall(
        &self,
        hits: &mut Vec<crate::node::SearchHit>,
        ctx: &mut crate::hook::HookContext,
    ) {
        if let Some(value) = self.invoke("onPostRecall", Self::hits_json(hits), ctx) {
            Self::replace_hits(hits, value);
        }
    }
    fn on_pre_graph_expand(
        &self,
        hits: &mut Vec<crate::node::SearchHit>,
        ctx: &mut crate::hook::HookContext,
    ) {
        if let Some(value) = self.invoke("onPreGraphExpand", Self::hits_json(hits), ctx) {
            Self::replace_hits(hits, value);
        }
    }
    fn on_rerank(
        &self,
        hits: &mut Vec<crate::node::SearchHit>,
        ctx: &mut crate::hook::HookContext,
    ) -> Option<Vec<crate::node::SearchHit>> {
        let value = self.invoke("onRerank", Self::hits_json(hits), ctx)?;
        let mut updated = Vec::new();
        Self::replace_hits(&mut updated, value);
        Some(updated)
    }
    fn on_post_search(
        &self,
        hits: &mut Vec<crate::node::SearchHit>,
        ctx: &mut crate::hook::HookContext,
    ) {
        if let Some(value) = self.invoke("onPostSearch", Self::hits_json(hits), ctx) {
            Self::replace_hits(hits, value);
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
