//! 共享资源运行时的构造、句柄校验与统计。

use super::arguments::{constructor_mib, constructor_options, parse_search_usize};
use super::errors::to_napi_error;
use super::json::{NodeJson, node_json_value};
use napi::bindgen_prelude::Either;
use napi::{Env, JsObject, JsUnknown, ValueType};
use napi_derive::napi;

pub(super) fn parse_runtime(
    env: Env,
    runtime: Option<JsUnknown>,
    write: Option<crate::error::WriteErrorInfo>,
) -> napi::Result<crate::resources::ResourceRuntime> {
    let invalid = || {
        let error = crate::TriviumError::InvalidInput(
                "runtime 必须是有效的 ResourceRuntime 句柄 (runtime must contain a valid ResourceRuntime handle)".into(),
            ).at_field("runtime");
        to_napi_error(
            env,
            match write {
                Some(write) => error.with_write_info(write),
                None => error,
            },
        )
    };
    Ok(if let Some(value) = runtime {
        if value.get_type()? != ValueType::Object {
            return Err(invalid());
        }
        let object = JsObject::try_from(value)?;
        if !object.has_own_property("__tdbResourceRuntime")? {
            return Err(invalid());
        }
        let handle: napi::bindgen_prelude::External<crate::resources::ResourceRuntime> = object
            .get_named_property("__tdbResourceRuntime")
            .map_err(|error| {
                // 已知句柄类型拒绝附加字段，getter 或 Proxy 的宿主异常原样传播。
                if error.status == napi::Status::InvalidArg {
                    invalid()
                } else {
                    error
                }
            })?;
        (*handle).clone()
    } else {
        crate::resources::ResourceRuntime::shared_default()
    })
}

/// 多个数据库共享的运行时资源配置，字节统计不等于进程 RSS。
#[napi(object)]
pub struct JsResourceLimits {
    pub maintenance_memory_limit_mb: Option<Either<f64, JsUnknown>>,
    pub memory_limit_mb: Option<Either<f64, JsUnknown>>,
    pub max_threads: Option<Either<f64, JsUnknown>>,
    pub max_active_tasks: Option<Either<f64, JsUnknown>>,
}

#[napi(js_name = "ResourceRuntime")]
pub struct JsResourceRuntime {
    inner: crate::resources::ResourceRuntime,
}

#[napi]
impl JsResourceRuntime {
    #[napi(constructor)]
    pub fn new(
        env: Env,
        mut this: napi::bindgen_prelude::This<JsObject>,
        options: Option<JsUnknown>,
    ) -> napi::Result<Self> {
        let object = constructor_options(env, options)?;
        let options = JsResourceLimits {
            maintenance_memory_limit_mb: object.get("maintenanceMemoryLimitMb")?,
            memory_limit_mb: object.get("memoryLimitMb")?,
            max_threads: object.get("maxThreads")?,
            max_active_tasks: object.get("maxActiveTasks")?,
        };
        let memory_limit = constructor_mib(
            env,
            parse_search_usize(env, options.memory_limit_mb, 0.0, "memoryLimitMb", false)?,
            "memoryLimitMb",
            9_007_199_254_740_991,
        )?;
        let inner = crate::resources::ResourceRuntime::new(crate::resources::ResourceLimits {
            memory_limit,
            maintenance_memory_limit: constructor_mib(
                env,
                parse_search_usize(
                    env,
                    options.maintenance_memory_limit_mb,
                    0.0,
                    "maintenanceMemoryLimitMb",
                    false,
                )?,
                "maintenanceMemoryLimitMb",
                9_007_199_254_740_991,
            )?,
            max_threads: parse_search_usize(env, options.max_threads, 0.0, "maxThreads", false)?,
            max_active_tasks: parse_search_usize(
                env,
                options.max_active_tasks,
                0.0,
                "maxActiveTasks",
                false,
            )?,
        })
        .map_err(|error| {
            let error = match error.info().target {
                Some(crate::error::ErrorTarget::Field { field }) if field == "max_threads" => {
                    error.at_field("maxThreads")
                }
                Some(crate::error::ErrorTarget::Field { field })
                    if field == "maintenance_memory_limit" =>
                {
                    error.at_field("maintenanceMemoryLimitMb")
                }
                _ => error,
            };
            to_napi_error(env, error)
        })?;
        // 使用带类型标记的外部句柄，避免把其他原生类的指针误解为资源运行时。
        let handle = env.create_external(inner.clone(), None)?;
        this.define_properties(
            &[napi::Property::new("__tdbResourceRuntime")?.with_value(&handle)],
        )?;
        Ok(Self { inner })
    }

    #[napi]
    pub fn stats(&self) -> napi::Result<NodeJson> {
        node_json_value(self.inner.stats())
            .map_err(|error| napi::Error::from_reason(error.to_string()))
    }
}
