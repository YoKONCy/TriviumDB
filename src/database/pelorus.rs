//! Pelorus 构建、发布和查询与数据库生命周期共用同一个 Core。

use super::{Database, MissingIndexPolicy, lock_or_recover, read_or_recover, write_or_recover};
use crate::error::WritePhase;
use crate::index::pelorus::{
    self, PelorusConfig, PelorusHit, PelorusRuntimeOptions, PelorusSearchOptions, PelorusState,
};
use crate::{Result, TriviumError, VectorType};
use serde::{Serialize, de::DeserializeOwned};

impl<T: VectorType + Serialize + DeserializeOwned> Database<T> {
    /// 在当前向量代际上构建显式 Poincaré 索引。常规检索继续使用默认路由。
    pub fn build_pelorus_index(&self, config: PelorusConfig) -> Result<()> {
        Self::write_operation(|write| {
            let _operation = self.enter_operation()?;
            self.require_write_access("build_pelorus_index")?;
            write.phase = WritePhase::Validation;
            self.prepare_pelorus(config, true)
        })
    }

    fn prepare_pelorus(&self, config: PelorusConfig, persist: bool) -> Result<()> {
        pelorus::validate_dtype::<T>()?;
        let (generation, previous, dim, ids, vectors, workspace) = {
            let mt = read_or_recover(&self.memtable);
            mt.ensure_readable()?;
            let dim = mt.dim();
            config.validate(dim)?;
            if persist {
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
            let workspace = self.resource_runtime.reserve_workspace(peak)?;
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
        if persist {
            lock_or_recover(&self.wal).ensure_writable()?;
        }
        if mt.vector_generation() != generation
            || mt.pelorus.as_ref().map(|state| state.config) != previous
        {
            return Err(TriviumError::DatabaseBusy {
                resource: "pelorus_source_generation",
            });
        }
        let current_bytes = mt.estimated_memory_bytes();
        let replaced_bytes = mt.pelorus.as_ref().map_or(0, PelorusState::heap_bytes);
        let required_bytes = current_bytes
            .saturating_sub(replaced_bytes)
            .saturating_add(state.heap_bytes());
        drop(workspace);
        if required_bytes > mt.reserved_database_bytes() {
            mt.resize_resource_reservation(required_bytes)?;
        }
        mt.pelorus = Some(state);
        if persist {
            mt.mark_checkpoint_dirty();
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
        search_locked(&read_or_recover(&self.memtable), query, options)
    }

    /// 在同一事实读视图内按输入顺序执行一批 Poincaré 查询。
    pub fn search_pelorus_batch(
        &self,
        queries: &[Vec<T>],
        options: &PelorusSearchOptions,
    ) -> Result<Vec<Vec<PelorusHit>>> {
        let _operation = self.enter_operation()?;
        self.prepare_pelorus_search(options)?;
        let mt = read_or_recover(&self.memtable);
        let row_bytes = options
            .top_k
            .min(mt.active_vector_count())
            .saturating_mul(std::mem::size_of::<PelorusHit>())
            .saturating_add(std::mem::size_of::<Vec<PelorusHit>>());
        let response_bytes = queries.len().saturating_mul(row_bytes);
        mt.ensure_additional_memory(self.memory_limit, response_bytes)?;
        let _workspace = self.resource_runtime.reserve_workspace(response_bytes)?;
        queries
            .iter()
            .map(|query| search_locked(&mt, query, options))
            .collect()
    }

    fn prepare_pelorus_search(&self, options: &PelorusSearchOptions) -> Result<()> {
        if !options.exact && self.missing_index_policy == MissingIndexPolicy::BuildInMemory {
            let pending = read_or_recover(&self.memtable)
                .pelorus
                .as_ref()
                .filter(|state| state.index.is_none())
                .map(|state| state.config);
            if let Some(config) = pending {
                self.prepare_pelorus(config, false)?;
            }
        }
        Ok(())
    }
}

fn search_locked<T: VectorType>(
    mt: &crate::storage::memtable::MemTable<T>,
    query: &[T],
    options: &PelorusSearchOptions,
) -> Result<Vec<PelorusHit>> {
    pelorus::search(
        mt,
        query,
        options,
        mt.pelorus_policy != MissingIndexPolicy::Error,
        |id| {
            Ok(options.payload_filter.as_ref().is_none_or(|filter| {
                mt.get_payload(id)
                    .is_some_and(|value| filter.matches(&value))
            }))
        },
    )?
    .into_iter()
    .map(|(id, distance)| {
        crate::request::check_current()?;
        let payload = mt.get_payload(id).ok_or_else(|| {
            TriviumError::CorruptedFile(
                "Pelorus 命中缺少 Payload (Pelorus hit has no payload)".into(),
            )
        })?;
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
