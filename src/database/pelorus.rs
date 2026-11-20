//! Pelorus 构建、发布和查询与数据库生命周期共用同一个 Core。

use super::{Database, MissingIndexPolicy, lock_or_recover, read_or_recover, write_or_recover};
use crate::error::{WriteErrorInfo, WriteOutcome, WritePhase};
use crate::index::pelorus::{
    self, PelorusConfig, PelorusHit, PelorusRuntimeOptions, PelorusSearchOptions, PelorusState,
};
use crate::{Result, TriviumError, VectorType};
use serde::{Serialize, de::DeserializeOwned};

impl<T: VectorType + Serialize + DeserializeOwned> Database<T> {
    /// 在当前向量代际上构建显式 Poincaré 索引。常规检索继续使用默认路由。
    pub fn build_pelorus_index(&self, config: PelorusConfig) -> Result<()> {
        Self::write_operation(|write| {
            let _operation = self.enter_maintenance_operation()?;
            self.require_write_access("build_pelorus_index")?;
            write.phase = WritePhase::Validation;
            self.prepare_pelorus(config, Some(write))
        })
    }

    fn prepare_pelorus(
        &self,
        config: PelorusConfig,
        write: Option<&mut WriteErrorInfo>,
    ) -> Result<()> {
        pelorus::validate_dtype::<T>()?;
        let (generation, previous, dim, ids, vectors, workspace) = {
            let mt = read_or_recover(&self.memtable);
            mt.ensure_readable()?;
            let dim = mt.dim();
            config.validate(dim)?;
            if write.is_some() {
                lock_or_recover(&self.wal).ensure_writable()?;
            }
            let count = mt.active_vector_count();
            let per_node = dim
                .saturating_mul(40)
                .saturating_add(config.m.saturating_mul(160))
                .saturating_add(2048);
            let parallelism = self.resource_runtime.parallelism();
            let peak = count.saturating_mul(per_node).saturating_add(
                count
                    .min(config.ef_construction)
                    .saturating_mul(parallelism)
                    .saturating_mul(256),
            );
            mt.ensure_additional_memory(self.memory_limit, peak)?;
            let workspace = self.resource_runtime.reserve_task_workspace(peak)?;
            let mut ids = Vec::new();
            let mut vectors = Vec::new();
            ids.try_reserve_exact(count).map_err(allocation_failed)?;
            vectors
                .try_reserve_exact(count.saturating_mul(dim))
                .map_err(allocation_failed)?;
            for slot in 0..mt.internal_slot_count() {
                crate::request::check_current()?;
                if let Some((id, vector)) = mt.active_vector_at_slot(slot)? {
                    ids.push(id);
                    vectors.extend(vector.iter().map(|x| x.to_f32()));
                }
            }
            (
                mt.vector_generation(),
                mt.pelorus.as_ref().map(|state| state.config),
                dim,
                ids,
                vectors,
                workspace,
            )
        };
        let state = self
            .resource_runtime
            .pool(self.resource_runtime.parallelism())?
            .install(|| PelorusState::build(config, dim, &ids, &vectors))?;
        drop(ids);
        drop(vectors);
        crate::request::check_current()?;

        let mut mt = write_or_recover(&self.memtable);
        mt.ensure_readable()?;
        if mt.vector_generation() != generation
            || mt.pelorus.as_ref().map(|state| state.config) != previous
        {
            return Err(TriviumError::DatabaseBusy {
                resource: "pelorus_source_generation",
            });
        }
        let Some(write) = write else {
            drop(workspace);
            Self::reserve_pelorus_install(&mt, &state)?;
            mt.pelorus = Some(state);
            return Ok(());
        };
        let mut wal = lock_or_recover(&self.wal);
        wal.ensure_writable()?;
        if previous.map(|old| old.curvature) != Some(config.curvature)
            && (mt.needs_checkpoint() || super::Wal::needs_recovery(&self.db_path))
        {
            // 先清除按旧曲率写入的 WAL。即使新检查点发布后清理失败，
            // 留下的日志也不会包含需按旧曲率验证的向量。
            super::file_format::checkpoint(
                &mut mt,
                &mut wal,
                &self.db_path,
                self.storage_mode,
                false,
            )?;
        }
        crate::request::check_current()?;
        drop(workspace);
        Self::reserve_pelorus_install(&mt, &state)?;
        let dirty = mt.needs_checkpoint();
        let previous = mt.pelorus.replace(state);
        mt.mark_checkpoint_dirty();
        let result = super::file_format::checkpoint_with_write_info(
            &mut mt,
            &mut wal,
            &self.db_path,
            self.storage_mode,
            false,
            write,
        );
        if result.is_err() && write.outcome != WriteOutcome::Committed {
            mt.pelorus = previous;
            if !dirty {
                mt.mark_checkpoint_clean();
            }
            if write.outcome == WriteOutcome::Unknown {
                mt.require_recovery();
                wal.release_writer();
            }
        }
        result
    }

    fn reserve_pelorus_install(
        mt: &crate::storage::memtable::MemTable<T>,
        state: &PelorusState,
    ) -> Result<()> {
        let current_bytes = mt.estimated_memory_bytes();
        let replaced_bytes = mt.pelorus.as_ref().map_or(0, PelorusState::heap_bytes);
        let required_bytes = current_bytes
            .saturating_sub(replaced_bytes)
            .saturating_add(state.heap_bytes());
        if required_bytes > mt.reserved_database_bytes() {
            mt.resize_resource_reservation(required_bytes)?;
        }
        Ok(())
    }

    /// 从已发布的 Pelorus 制品切换重排行的驻留方式；Reader 也可配置。
    pub fn configure_pelorus_runtime(&self, options: PelorusRuntimeOptions) -> Result<()> {
        let _operation = self.enter_operation()?;
        let mut mt = write_or_recover(&self.memtable);
        mt.ensure_readable()?;
        if mt.needs_checkpoint() {
            return Err(TriviumError::DatabaseBusy {
                resource: "pelorus_pending_checkpoint",
            });
        }
        pelorus::persistence::configure_runtime(&mut mt, &self.db_path, options, self.memory_limit)
    }

    /// 使用已配置的 Poincaré 曲率查询，返回由近到远的测地距离。
    pub fn search_pelorus(
        &self,
        query: &[T],
        options: &PelorusSearchOptions,
    ) -> Result<Vec<PelorusHit>> {
        let _operation = self.enter_operation()?;
        self.prepare_pelorus_search(options)?;
        let mt = read_or_recover(&self.memtable);
        let mut response = self.resource_runtime.reserve_workspace(0)?;
        search_locked(&mt, query, options, self.memory_limit, &mut response)
    }

    /// 在同一事实读视图内按输入顺序执行一批 Poincaré 查询。
    pub fn search_pelorus_batch(
        &self,
        queries: &[Vec<T>],
        options: &PelorusSearchOptions,
    ) -> Result<Vec<Vec<PelorusHit>>> {
        let _operation = self.enter_operation()?;
        pelorus::validate_dtype::<T>()?;
        self.prepare_pelorus_search(options)?;
        let mt = read_or_recover(&self.memtable);
        let response_bytes = queries
            .len()
            .saturating_mul(std::mem::size_of::<Vec<PelorusHit>>());
        mt.ensure_additional_memory(self.memory_limit, response_bytes)?;
        let mut response = self.resource_runtime.reserve_workspace(response_bytes)?;
        queries
            .iter()
            .map(|query| search_locked(&mt, query, options, self.memory_limit, &mut response))
            .collect()
    }

    fn prepare_pelorus_search(&self, options: &PelorusSearchOptions) -> Result<()> {
        options.validate()?;
        if !options.exact && self.missing_index_policy == MissingIndexPolicy::BuildInMemory {
            let pending = read_or_recover(&self.memtable)
                .pelorus
                .as_ref()
                .filter(|state| state.index.is_none())
                .map(|state| state.config);
            if let Some(config) = pending {
                self.prepare_pelorus(config, None)?;
            }
        }
        Ok(())
    }
}

fn search_locked<T: VectorType>(
    mt: &crate::storage::memtable::MemTable<T>,
    query: &[T],
    options: &PelorusSearchOptions,
    memory_limit: usize,
    response: &mut crate::resources::WorkspaceReservation,
) -> Result<Vec<PelorusHit>> {
    let hits = pelorus::search(
        mt,
        query,
        options,
        mt.pelorus_policy != MissingIndexPolicy::Error,
        |id| match &options.payload_filter {
            Some(filter) => Ok(mt
                .try_get_payload(id)?
                .is_some_and(|value| filter.matches(&value))),
            None => Ok(true),
        },
    )?;
    let bytes = response
        .bytes()
        .saturating_add(hits.len().saturating_mul(std::mem::size_of::<PelorusHit>()));
    mt.ensure_additional_memory(memory_limit, bytes)?;
    response.resize(bytes)?;
    hits.into_iter()
        .map(|(id, distance)| {
            crate::request::check_current()?;
            let payload = mt.try_get_payload(id)?.ok_or_else(|| {
                TriviumError::CorruptedFile(
                    "Pelorus 命中缺少 Payload (Pelorus hit has no payload)".into(),
                )
            })?;
            let bytes = response.bytes().saturating_add(
                crate::storage::payload_store::estimate_json_memory(&payload),
            );
            mt.ensure_additional_memory(memory_limit, bytes)?;
            response.resize(bytes)?;
            Ok(PelorusHit {
                id,
                distance,
                payload: (*payload).clone(),
            })
        })
        .collect()
}

fn allocation_failed(_: std::collections::TryReserveError) -> TriviumError {
    TriviumError::CapacityAllocationFailed {
        reason: "无法预留 Pelorus 构建输入 (Could not reserve Pelorus build input)".into(),
    }
}
