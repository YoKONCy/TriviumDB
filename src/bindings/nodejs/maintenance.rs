//! 持久化、后台维护、容量与资源管理入口。

use super::arguments::{
    node_id_to_js, parse_management_string, parse_management_usize, parse_positive_usize,
    write_number, write_string,
};
use super::database::{DbBackend, TriviumDB};
use super::errors::{to_napi_error, with_napi_write_info};
use super::json::{NodeJson, node_json_value};
use napi::bindgen_prelude::Either;
use napi::{Env, JsUnknown};
use napi_derive::napi;

#[napi]
impl TriviumDB {
    #[napi]
    pub fn storage_info(&self) -> napi::Result<NodeJson> {
        dispatch!(self, db => node_json_value(db.storage_info()))
            .map_err(|error| napi::Error::from_reason(error.to_string()))
    }

    // ── 持久化与管理 ──

    /// 手动落盘
    #[napi]
    pub fn flush(&mut self, env: Env) -> napi::Result<()> {
        dispatch!(self, mut db => db.flush()).map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn publish_generation_manifest(
        &mut self,
        env: Env,
        generation_id: Either<String, JsUnknown>,
    ) -> napi::Result<NodeJson> {
        let generation_id = parse_management_string(env, generation_id, "generationId")?;
        let manifest = dispatch!(self, mut db => db.publish_generation_manifest(&generation_id))
            .map_err(|error| to_napi_error(env, error))?;
        node_json_value(manifest).map_err(|error| napi::Error::from_reason(error.to_string()))
    }

    /// 运行时切换 WAL 同步模式
    #[napi]
    pub fn set_sync_mode(&mut self, env: Env, mode: Either<String, JsUnknown>) -> napi::Result<()> {
        let mode = parse_management_string(env, mode, "mode")?;
        let sm = crate::storage::wal::SyncMode::parse(&mode).map_err(|reason| {
            to_napi_error(
                env,
                crate::TriviumError::InvalidInput(reason)
                    .at_field("mode")
                    .with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
            )
        })?;
        dispatch!(self, mut db => db.set_sync_mode(sm)).map_err(|error| to_napi_error(env, error))
    }

    /// 启动后台自动压缩（每 interval_secs 秒落盘一次，默认 2 小时=7200秒）
    #[napi]
    pub fn enable_auto_compaction(
        &mut self,
        env: Env,
        interval_secs: Option<Either<f64, JsUnknown>>,
    ) -> napi::Result<()> {
        let secs = parse_management_usize(
            env,
            interval_secs.unwrap_or(Either::A(7200.0)),
            "intervalSecs",
        )? as u64;
        dispatch!(self, mut db => db.enable_auto_compaction(std::time::Duration::from_secs(secs)))
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn set_auto_build_quiver(
        &mut self,
        env: Env,
        enabled: Either<bool, JsUnknown>,
    ) -> napi::Result<()> {
        let enabled = match enabled {
            Either::A(enabled) => enabled,
            Either::B(_) => {
                return Err(to_napi_error(
                    env,
                    crate::TriviumError::InvalidInput(
                        "enabled 必须是布尔值 (enabled must be a boolean)".into(),
                    )
                    .at_field("enabled")
                    .with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                ));
            }
        };
        dispatch!(self, mut db => db.set_auto_build_quiver(enabled))
            .map_err(|error| to_napi_error(env, error))
    }

    /// 停止后台自动压缩
    #[napi]
    pub fn clear_search_state(&self, env: Env) -> napi::Result<()> {
        dispatch!(self, db => db.clear_search_state()).map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn disable_auto_compaction(&mut self) {
        dispatch!(self, mut db => db.disable_auto_compaction());
    }

    /// 手动触发全量压实（阻塞当前线程）
    #[napi]
    pub fn compact(&mut self, env: Env) -> napi::Result<()> {
        dispatch!(self, mut db => db.compact()).map_err(|error| to_napi_error(env, error))
    }

    /// 为后续插入主动预留额外节点容量。
    #[napi]
    pub fn reserve_nodes(&self, env: Env, additional: Either<f64, JsUnknown>) -> napi::Result<()> {
        let additional = parse_management_usize(env, additional, "additional")?;
        dispatch!(self, db => db.reserve_nodes(additional))
            .map_err(|error| to_napi_error(env, error))
    }

    /// 设置内存上限（MiB），0 = 无限制。
    #[napi]
    pub fn set_memory_limit(&mut self, env: Env, mb: Either<f64, JsUnknown>) -> napi::Result<()> {
        let mb = parse_management_usize(env, mb, "mb")?;
        let bytes = mb.checked_mul(1024 * 1024).ok_or_else(|| {
            to_napi_error(
                env,
                crate::TriviumError::InvalidInput(
                    "内存上限换算字节时溢出 (Memory limit overflows when converted to bytes)"
                        .into(),
                )
                .at_field("mb")
                .with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
            )
        })?;
        dispatch!(self, mut db => db.set_memory_limit(bytes)).map_err(|error| {
            let error = if error.info().target.is_some() {
                error.at_field("mb")
            } else {
                error
            };
            to_napi_error(env, error)
        })
    }

    /// 读取多个数据库共享的配额、工作线程和任务统计。
    #[napi]
    pub fn resource_stats(&self) -> napi::Result<NodeJson> {
        node_json_value(dispatch!(self, db => db.resource_stats()))
            .map_err(|error| napi::Error::from_reason(error.to_string()))
    }

    #[napi]
    pub fn instance_stats(&self) -> napi::Result<NodeJson> {
        node_json_value(dispatch!(self, db => db.instance_stats()))
            .map_err(|error| napi::Error::from_reason(error.to_string()))
    }

    /// 估算当前内存占用（字节）
    #[napi]
    pub fn estimated_memory(&self) -> f64 {
        dispatch!(self, db => db.estimated_memory()) as f64
    }

    /// 维度迁移：结构复制到新维度数据库，返回需要更新向量的节点 ID 列表
    #[napi]
    pub fn migrate(
        &self,
        env: Env,
        new_path: JsUnknown,
        new_dim: JsUnknown,
    ) -> napi::Result<Vec<f64>> {
        let new_path = write_string(env, new_path, "newPath")?;
        let new_dim = write_number(env, new_dim, "newDim").and_then(|value| {
            parse_positive_usize(env, value, "newDim").map_err(|error| {
                with_napi_write_info(
                    env,
                    error,
                    crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION,
                )
            })
        })?;
        self.all_node_ids(env)?;
        match &self.inner {
            DbBackend::F32(db) => {
                let (_, ids) = db
                    .migrate_to(&new_path, new_dim)
                    .map_err(|error| to_napi_error(env, error))?;
                ids.into_iter()
                    .map(|value| node_id_to_js(env, value))
                    .collect::<napi::Result<_>>()
            }
            DbBackend::F16(db) => {
                let (_, ids) = db
                    .migrate_to(&new_path, new_dim)
                    .map_err(|error| to_napi_error(env, error))?;
                ids.into_iter()
                    .map(|value| node_id_to_js(env, value))
                    .collect::<napi::Result<_>>()
            }
            DbBackend::U64(db) => {
                let (_, ids) = db
                    .migrate_to(&new_path, new_dim)
                    .map_err(|error| to_napi_error(env, error))?;
                ids.into_iter()
                    .map(|value| node_id_to_js(env, value))
                    .collect::<napi::Result<_>>()
            }
        }
    }
}
