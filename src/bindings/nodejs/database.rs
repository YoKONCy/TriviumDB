//! 数据库句柄、dtype 分发、构造及基础信息。

use super::arguments::{
    constructor_bool, constructor_mib, constructor_options, constructor_string, node_id_to_js,
    parse_node_id, parse_search_usize, parse_usize_argument, raw_string,
};
use super::errors::{
    argument_input_error, invalid_constructor_field, to_napi_error, with_napi_write_info,
};
use super::options::{
    parse_access_mode, parse_missing_index_policy, parse_row_overflow, parse_storage_mode,
    parse_sync_mode,
};
use super::runtime::parse_runtime;
use super::types::JsDatabaseOptions;
use crate::database::Database as GenericDatabase;
use napi::bindgen_prelude::Either;
use napi::{Env, JsUnknown, ValueType};
use napi_derive::napi;

// ════════ 后端枚举：封装三种泛型特化 ════════

pub(super) enum DbBackend {
    F32(GenericDatabase<f32>),
    F16(GenericDatabase<half::f16>),
    U64(GenericDatabase<u64>),
}

// ════════ TriviumDB 主类 ════════

#[napi(js_name = "TriviumDB")]
pub struct TriviumDB {
    pub(super) inner: DbBackend,
    pub(super) dtype: String,
}

impl TriviumDB {
    pub(super) fn check_generated_ids(&self, env: Env, count: usize) -> napi::Result<()> {
        if count == 0 {
            return Ok(());
        }
        let next = dispatch!(self,db=>db.next_id_for_binding()).map_err(|error| {
            to_napi_error(
                env,
                error.with_write_info(crate::error::WriteErrorInfo {
                    transaction_consumed: None,
                    ..crate::error::WriteErrorInfo::TRANSACTION_ADMISSION
                }),
            )
        })?;
        let last = next.checked_add(count as u64 - 1).ok_or_else(|| {
            to_napi_error(
                env,
                crate::TriviumError::InvalidInput(
                    "节点 ID 分配溢出 (Node ID allocation overflow)".into(),
                )
                .with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
            )
        })?;
        node_id_to_js(env, last).map_err(|error| {
            with_napi_write_info(
                env,
                error,
                crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION,
            )
        })?;
        Ok(())
    }

    pub(super) fn check_transaction_ids(
        &self,
        env: Env,
        operations: &[serde_json::Value],
    ) -> napi::Result<()> {
        let mut next = dispatch!(self,db=>db.next_id_for_binding())
            .map_err(|error| to_napi_error(env, error))?;
        for (index, operation) in operations.iter().enumerate() {
            match operation.get("type").and_then(serde_json::Value::as_str) {
                Some("insert") => {
                    node_id_to_js(env, next)?;
                    next += 1;
                }
                Some("insertWithId") => {
                    let id = operation
                        .get("id")
                        .and_then(serde_json::Value::as_f64)
                        .ok_or_else(|| {
                            argument_input_error(
                                env,
                                &format!("/operations/{index}/id"),
                                "缺少事务 ID (Missing transaction ID)",
                                None,
                            )
                        })?;
                    next =
                        next.max(parse_node_id(env, id, &format!("/operations/{index}/id"))? + 1);
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[napi]
impl TriviumDB {
    /// 打开或创建数据库
    ///
    /// ```js
    /// const db = new TriviumDB("data.tdb", { dim: 1536, dtype: "f32", syncMode: "normal" })
    /// ```
    #[napi(constructor)]
    pub fn new(env: Env, path: JsUnknown, options: Option<JsUnknown>) -> napi::Result<Self> {
        let path = raw_string(env, path, "path", None)?;
        if let Some(value) = options.as_ref()
            && value.get_type()? == ValueType::Number
        {
            return Err(to_napi_error(
                env,
                crate::TriviumError::ApiMigrationRequired {
                    removed_api: "new TriviumDB(path, dim)",
                    replacement: "new TriviumDB(path, { dim })",
                }
                .at_field("options"),
            ));
        }
        let object = constructor_options(env, options)?;
        let options = JsDatabaseOptions {
            runtime: object.get("runtime")?,
            dim: object.get("dim")?,
            dtype: object.get("dtype")?,
            sync_mode: object.get("syncMode")?,
            storage_mode: object.get("storageMode")?,
            auto_build_quiver: object.get("autoBuildQuiver")?,
            load_text_index: object.get("loadTextIndex")?,
            expected_nodes: object.get("expectedNodes")?,
            memory_limit_mb: object.get("memoryLimitMb")?,
            payload_cache_mb: object.get("payloadCacheMb")?,
            payload_cache_entry_mb: object.get("payloadCacheEntryMb")?,
            access_mode: object.get("accessMode")?,
            missing_index_policy: object.get("missingIndexPolicy")?,
            max_query_rows: object.get("maxQueryRows")?,
            row_overflow: object.get("rowOverflow")?,
        };
        let dim = parse_search_usize(env, options.dim, 1536.0, "dim", true)?;
        let dtype = constructor_string(env, options.dtype, "f32", "dtype")?;
        let expected_nodes = options
            .expected_nodes
            .map(|value| parse_usize_argument(env, value, "expectedNodes", None))
            .transpose()?;
        let memory_limit_mb = options
            .memory_limit_mb
            .map(|value| parse_usize_argument(env, value, "memoryLimitMb", None))
            .transpose()?
            .unwrap_or(0);
        let memory_limit =
            constructor_mib(env, memory_limit_mb, "memoryLimitMb", usize::MAX as u128)?;
        let mb_to_bytes = |value: Option<Either<f64, JsUnknown>>, name: &str, default_mb: usize| {
            let mb = value
                .map(|value| parse_usize_argument(env, value, name, None))
                .transpose()?
                .unwrap_or(default_mb);
            constructor_mib(env, mb, name, usize::MAX as u128)
        };
        let payload_cache_bytes = mb_to_bytes(options.payload_cache_mb, "payloadCacheMb", 64)?;
        let payload_cache_entry_bytes =
            mb_to_bytes(options.payload_cache_entry_mb, "payloadCacheEntryMb", 8)?;
        let max_query_rows = options
            .max_query_rows
            .map(|value| parse_usize_argument(env, value, "maxQueryRows", None))
            .transpose()?;
        let config = crate::database::Config {
            dim,
            sync_mode: parse_sync_mode(
                env,
                &constructor_string(env, options.sync_mode, "normal", "syncMode")?,
            )?,
            storage_mode: parse_storage_mode(
                env,
                Some(&constructor_string(
                    env,
                    options.storage_mode,
                    "mmap",
                    "storageMode",
                )?),
            )?,
            auto_build_quiver: constructor_bool(
                env,
                options.auto_build_quiver,
                true,
                "autoBuildQuiver",
            )?,
            load_text_index: constructor_bool(
                env,
                options.load_text_index,
                false,
                "loadTextIndex",
            )?,
            expected_nodes,
            memory_limit,
            payload_cache_bytes,
            payload_cache_entry_bytes,
            access_mode: parse_access_mode(
                env,
                Some(&constructor_string(
                    env,
                    options.access_mode,
                    "readWrite",
                    "accessMode",
                )?),
            )?,
            missing_index_policy: parse_missing_index_policy(
                env,
                Some(&constructor_string(
                    env,
                    options.missing_index_policy,
                    "fallback",
                    "missingIndexPolicy",
                )?),
            )?,
            max_query_rows,
            row_overflow: parse_row_overflow(
                env,
                Some(&constructor_string(
                    env,
                    options.row_overflow,
                    "throw",
                    "rowOverflow",
                )?),
            )?,
        };
        let runtime = parse_runtime(env, options.runtime, None)?;
        let dtype_str = dtype.as_str();

        let inner = match dtype_str {
            "f32" => DbBackend::F32(
                GenericDatabase::<f32>::open_with_runtime(&path, config, runtime.clone())
                    .map_err(|error| to_napi_error(env, error))?,
            ),
            "f16" => DbBackend::F16(
                GenericDatabase::<half::f16>::open_with_runtime(&path, config, runtime.clone())
                    .map_err(|error| to_napi_error(env, error))?,
            ),
            "u64" => DbBackend::U64(
                GenericDatabase::<u64>::open_with_runtime(&path, config, runtime.clone())
                    .map_err(|error| to_napi_error(env, error))?,
            ),
            _ => {
                return Err(invalid_constructor_field(
                    env,
                    "dtype",
                    "dtype 必须是 f32 / f16 / u64 (dtype must be f32 / f16 / u64)",
                ));
            }
        };
        Ok(Self {
            inner,
            dtype: dtype_str.to_string(),
        })
    }

    /// 获取向量维度
    #[napi]
    pub fn dim(&self) -> u32 {
        dispatch!(self, db => db.dim()) as u32
    }

    /// 获取节点总数
    #[napi]
    pub fn node_count(&self, env: Env) -> napi::Result<u32> {
        Ok({
            dispatch!(self, db => db.node_count().map_err(|error| to_napi_error(env, error))?)
                as u32
        })
    }

    /// 获取所有活跃节点 ID
    #[napi]
    pub fn all_node_ids(&self, env: Env) -> napi::Result<Vec<f64>> {
        Ok({
            dispatch!(self, db => db.all_node_ids().map_err(|error| to_napi_error(env, error))?)
                .into_iter()
                .map(|value| node_id_to_js(env, value))
                .collect::<napi::Result<_>>()?
        })
    }

    /// 获取 dtype 字符串（"f32" / "f16" / "u64"）
    #[napi(getter)]
    pub fn dtype(&self) -> String {
        self.dtype.clone()
    }

    /// 检查节点是否存在
    #[napi]
    pub fn contains(&self, env: Env, id: Either<f64, JsUnknown>) -> napi::Result<bool> {
        let id = parse_usize_argument(env, id, "id", None)? as u64;
        Ok(dispatch!(self, db => db.contains(id).map_err(|error| to_napi_error(env, error))?))
    }
}
