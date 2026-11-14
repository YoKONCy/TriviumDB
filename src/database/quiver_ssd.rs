//! 数据库绑定的压缩 QuIVer 运行时，来源身份和刷新由内核维护。

use super::{AccessMode, Database, read_or_recover, write_or_recover};
use crate::index::quiver::{
    QuIVerSearchConfig,
    ssd::{SsdOptions, SsdQuiver, SsdRuntimeOptions, SsdStats},
};
use crate::{Result, TriviumError, VectorType};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[cfg(test)]
#[path = "quiver_ssd/tests.rs"]
mod tests;

pub(crate) struct DatabaseSsd {
    pub generation: u64,
    pub options: SsdRuntimeOptions,
    pub reader: SsdQuiver,
    pub database_memory_limit: usize,
}

impl<T: VectorType + Serialize + DeserializeOwned> Database<T> {
    pub fn configure_quiver(&self, options: SsdRuntimeOptions) -> Result<SsdStats> {
        self.configure_quiver_int8(options)
    }
    pub fn search_quiver(
        &self,
        query: &[f32],
        config: &QuIVerSearchConfig,
    ) -> Result<Vec<(u64, f32)>> {
        self.search_quiver_int8(query, config)
    }
    pub fn quiver_stats(&self) -> Result<Option<SsdStats>> {
        self.quiver_int8_stats()
    }
    pub fn close_quiver(&self) -> Result<()> {
        self.close_quiver_int8()
    }

    pub(crate) fn prepare_quiver_rerank(&self) -> Result<()> {
        prepare_configured_rerank(
            &self.memtable,
            &self.wal,
            &self.db_path,
            self.storage_mode,
            self.access_mode,
            self.memory_limit,
        )
    }

    pub(crate) fn prepare_quiver_rerank_with_source(
        &self,
        query: &crate::query::tql_ast::TqlQuery,
        source: Option<&std::sync::Arc<crate::query::tql_source::TqlSource>>,
    ) -> Result<()> {
        if (query.explain && !query.analyze)
            || !matches!(
                query.entry,
                crate::query::tql_ast::QueryEntry::Search { .. }
            )
        {
            return Ok(());
        }
        self.prepare_quiver_rerank().map_err(|error| {
            let stage = crate::error::QueryErrorStage::Execute;
            match source {
                Some(source) => source
                    .origin(crate::query::tql_source::SourceSelector::Root)
                    .error(error, stage),
                None => error.at_query_source(stage, None),
            }
        })
    }
    /// 选择压缩候选精排与驻留策略。数据库自动绑定已确认事实，按需发布派生制品。
    pub fn configure_quiver_int8(&self, options: SsdRuntimeOptions) -> Result<SsdStats> {
        let _operation = self.enter_operation()?;
        options.validate()?;
        if crate::vector::is_binary_vector::<T>() {
            return Err(TriviumError::InvalidInput(
                "INT8 QuIVer支持浮点向量 (INT8 QuIVer supports floating-point vectors)".into(),
            )
            .at_field("dtype"));
        }
        {
            let mut mt = write_or_recover(&self.memtable);
            validate_ann_mode(&mt, options)?;
            let capacity = database_capacity(self.memory_limit, mt.base_memory_bytes())?;
            if let Some(stats) = mt.configure_quiver_ssd_runtime(options, capacity)? {
                return Ok(stats);
            }
        }
        if self.access_mode == AccessMode::ReadWrite {
            if read_or_recover(&self.memtable).quiver().is_none() {
                self.build_quiver_index(None)?;
            }
            Self::write_operation(|write| {
                let mut mt = write_or_recover(&self.memtable);
                let mut wal = super::lock_or_recover(&self.wal);
                crate::storage::file_format::checkpoint_with_write_info(
                    &mut mt,
                    &mut wal,
                    &self.db_path,
                    self.storage_mode,
                    true,
                    write,
                )
            })?;
        }
        let mut mt = write_or_recover(&self.memtable);
        mt.ensure_readable()?;
        if self.access_mode == AccessMode::ReadWrite && mt.needs_checkpoint() {
            return Err(TriviumError::DatabaseBusy {
                resource: "quiver_ssd_fact_generation",
            });
        }
        let generation = mt.vector_generation();
        let mut reader = load_reader(
            &mt,
            &self.db_path,
            self.memory_limit,
            self.access_mode == AccessMode::ReadWrite,
            options.encoding,
        )?;
        reader.configure_bound(options, mt.quiver().ok_or(TriviumError::DatabaseClosed)?)?;
        reader.set_database_capacity(database_capacity(
            self.memory_limit,
            mt.base_memory_bytes(),
        )?);
        let stats = reader.stats();
        mt.advise_cold_vectors_dontneed();
        crate::request::check_current()?;
        mt.quiver_ssd = Some(DatabaseSsd {
            generation,
            options,
            reader,
            database_memory_limit: self.memory_limit,
        });
        Ok(stats)
    }

    /// 显式INT8检索入口，写入后的首次调用同步刷新派生代际；错误保留结构化信息。
    pub fn search_quiver_int8(
        &self,
        query: &[f32],
        config: &QuIVerSearchConfig,
    ) -> Result<Vec<(u64, f32)>> {
        let _operation = self.enter_operation()?;
        crate::vector::validate_query_vector(query, self.dim())?;
        if config.top_k == 0 {
            return Ok(Vec::new());
        }
        let refresh = {
            let mt = read_or_recover(&self.memtable);
            if mt.active_vector_count() == 0 {
                return Ok(Vec::new());
            }
            let active = mt.quiver_ssd.as_ref().ok_or_else(|| {
                TriviumError::InvalidInput(
                    "请先配置INT8 QuIVer (Configure INT8 QuIVer first)".into(),
                )
                .at_field("quiver")
            })?;
            if mt.quiver().is_some_and(|index| index.active_count() == 0) {
                return Ok(Vec::new());
            }
            (active.generation != mt.vector_generation()
                || mt
                    .quiver()
                    .is_none_or(|index| !active.reader.routing_matches(index)))
            .then_some(active.options)
        };
        if let Some(options) = refresh {
            self.configure_quiver_int8(options)?;
        }
        let mt = read_or_recover(&self.memtable);
        let active = mt.quiver_ssd.as_ref().ok_or(TriviumError::DatabaseClosed)?;
        active.reader.set_database_capacity(database_capacity(
            self.memory_limit,
            mt.base_memory_bytes(),
        )?);
        if active.generation != mt.vector_generation() {
            return Err(TriviumError::InvalidInput(
                "事实代际已变化，请重试查询 (Fact generation changed; retry query)".into(),
            )
            .at_field("quiver"));
        }
        active.reader.search_bound(
            query,
            config,
            &|_| true,
            mt.quiver().ok_or(TriviumError::DatabaseClosed)?,
        )
    }

    pub fn quiver_int8_stats(&self) -> Result<Option<SsdStats>> {
        let _operation = self.enter_operation()?;
        Ok(read_or_recover(&self.memtable)
            .quiver_ssd
            .as_ref()
            .map(|active| active.reader.stats()))
    }

    pub fn close_quiver_int8(&self) -> Result<()> {
        let _operation = self.enter_operation()?;
        write_or_recover(&self.memtable).quiver_ssd.take();
        Ok(())
    }

    pub fn collect_quiver_temporary(&self) -> Result<usize> {
        let _operation = self.enter_operation()?;
        self.require_write_access("collect_quiver_temporary")?;
        SsdQuiver::collect_temporary(&self.quiver_ssd_path())
    }

    fn quiver_ssd_path(&self) -> PathBuf {
        PathBuf::from(format!("{}.qssd", self.db_path))
    }
}

/// 同步入口和排队查询在实际执行时共同确认事实代际，存储锁内完成checkpoint与派生刷新。
pub(super) fn prepare_configured_rerank<T: VectorType>(
    memtable: &std::sync::RwLock<crate::storage::memtable::MemTable<T>>,
    wal: &std::sync::Mutex<crate::storage::wal::Wal>,
    path: &str,
    mode: super::StorageMode,
    access: AccessMode,
    limit: usize,
) -> Result<()> {
    crate::request::check_current()?;
    let needs_refresh = |mt: &crate::storage::memtable::MemTable<T>| {
        mt.active_vector_count() > 0
            && mt.quiver_ssd.as_ref().is_some_and(|active| {
                active.options.use_for_ann
                    && (active.generation != mt.vector_generation()
                        || active.reader.stats().closed
                        || mt
                            .quiver()
                            .is_none_or(|index| !active.reader.routing_matches(index)))
            })
    };
    if !needs_refresh(&read_or_recover(memtable)) {
        return Ok(());
    }
    let mut mt = write_or_recover(memtable);
    mt.ensure_readable()?;
    if !needs_refresh(&mt) {
        return Ok(());
    }
    if access == AccessMode::ReadWrite && mt.needs_checkpoint() {
        let mut wal = super::lock_or_recover(wal);
        crate::error::WriteErrorInfo::capture(|write| {
            crate::storage::file_format::checkpoint_with_write_info(
                &mut mt, &mut wal, path, mode, true, write,
            )
        })?;
        if !needs_refresh(&mt) {
            mt.advise_cold_vectors_dontneed();
            return Ok(());
        }
    }
    let options = mt
        .quiver_ssd
        .as_ref()
        .ok_or(TriviumError::DatabaseClosed)?
        .options;
    let mut reader = load_reader(
        &mt,
        path,
        limit,
        access == AccessMode::ReadWrite,
        options.encoding,
    )?;
    reader.configure_bound(options, mt.quiver().ok_or(TriviumError::DatabaseClosed)?)?;
    reader.set_database_capacity(database_capacity(limit, mt.base_memory_bytes())?);
    mt.quiver_ssd = Some(DatabaseSsd {
        generation: mt.vector_generation(),
        options,
        reader,
        database_memory_limit: limit,
    });
    mt.advise_cold_vectors_dontneed();
    Ok(())
}

fn database_capacity(limit: usize, used: usize) -> Result<usize> {
    if limit == 0 {
        return Ok(0);
    }
    let available = limit.saturating_sub(used);
    if available == 0 {
        return Err(TriviumError::ResourceBudgetExceeded {
            resource: "database_heap_bytes",
            requested: 1,
            used,
            limit,
        });
    }
    Ok(available)
}

fn source<T: VectorType>(path: &str) -> Result<([u8; 32], u64, u32)> {
    use std::io::Read;
    // 固定上限读取，避免损坏或被替换的元数据诱发无界分配。
    let mut metadata = Vec::new();
    std::fs::File::open(format!("{path}.quiver.meta"))?
        .take(4097)
        .read_to_end(&mut metadata)?;
    if metadata.len() != 60
        || &metadata[..4] != b"QMET"
        || u32::from_le_bytes(metadata[4..8].try_into().unwrap()) != 2
        || crc32fast::hash(&metadata[..56])
            != u32::from_le_bytes(metadata[56..].try_into().unwrap())
    {
        return Err(TriviumError::CorruptedFile(
            "QuIVer元数据无效 (Invalid QuIVer metadata)".into(),
        ));
    }
    let mut hash = Sha256::new();
    let hot_bytes = u64::from_le_bytes(metadata[24..32].try_into().unwrap());
    let hot_crc = u32::from_le_bytes(metadata[44..48].try_into().unwrap());
    hash.update(b"TDB-QVSSD-FACTS-1");
    hash.update(
        crate::storage::file_format::vector_dtype::<T>()?
            .0
            .to_le_bytes(),
    );
    hash.update(metadata);
    Ok((hash.finalize().into(), hot_bytes, hot_crc))
}

fn load_reader<T: VectorType>(
    mt: &crate::storage::memtable::MemTable<T>,
    db_path: &str,
    limit: usize,
    writable: bool,
    encoding: crate::index::quiver::ssd::RerankEncoding,
) -> Result<SsdQuiver> {
    let index =
        mt.quiver().ok_or_else(|| {
            TriviumError::InvalidInput(
        "INT8 QuIVer需要非空浮点索引 (INT8 QuIVer requires a nonempty floating-point index)".into())
        })?;
    let path = PathBuf::from(format!("{db_path}.qssd"));
    let (source, hot_bytes, hot_crc) = source::<T>(db_path)?;
    let options = SsdOptions {
        encoding,
        memory_limit_bytes: database_capacity(limit, mt.estimated_memory_bytes())?,
        ..Default::default()
    };
    let runtime = mt.resource_runtime();
    if writable {
        SsdQuiver::collect_temporary(&path)?;
    }
    let open = || {
        let reader = SsdQuiver::open_bound(
            &path,
            source,
            options,
            runtime.clone(),
            (hot_bytes, hot_crc),
            index,
        )?;
        if reader.stats().metadata.encoding != encoding {
            return Err(TriviumError::ImmutableArtifactInvalid {
                reason: "重排制品编码不匹配 (Reranking artifact encoding mismatch)".into(),
            });
        }
        Ok(reader)
    };
    match open() {
        Ok(reader) => Ok(reader),
        Err(error)
            if writable
                && (matches!(&error, TriviumError::Io(cause) if cause.kind() == std::io::ErrorKind::NotFound)
                    || matches!(&error, TriviumError::ImmutableArtifactInvalid { .. })) =>
        {
            SsdQuiver::publish(index, &path, source, options, runtime, |slot, output| {
                let (_, vector) = mt.active_vector_at_slot(slot)?.ok_or_else(|| {
                    TriviumError::CorruptedFile(
                        "SSD候选向量缺失 (SSD candidate vector is missing)".into(),
                    )
                })?;
                for (target, value) in output.iter_mut().zip(vector) {
                    *target = value.to_f32();
                }
                Ok(())
            })?;
            open()
        }
        Err(error) => Err(error),
    }
}

impl<T: VectorType> crate::storage::memtable::MemTable<T> {
    pub(crate) fn configured_quiver_search(
        &self,
        query: &[f32],
        config: &QuIVerSearchConfig,
        eligible: &dyn Fn(u64) -> bool,
    ) -> Result<Option<Vec<(u64, f32)>>> {
        let Some(active) = self
            .quiver_ssd
            .as_ref()
            .filter(|active| active.options.use_for_ann)
        else {
            return Ok(None);
        };
        validate_ann_mode(self, active.options)?;
        if active.generation != self.vector_generation() || active.reader.stats().closed {
            return Err(TriviumError::DatabaseBusy {
                resource: "quiver_rerank_generation",
            });
        }
        active
            .reader
            .search_bound(
                query,
                config,
                eligible,
                self.quiver().ok_or(TriviumError::DatabaseClosed)?,
            )
            .map(Some)
    }
    /// 在事实文件组提交之后刷新启用的派生索引；失败保留原Reader并允许重试checkpoint。
    pub(crate) fn refresh_quiver_ssd_after_checkpoint(&mut self, path: &str) -> Result<()> {
        if self.quiver().is_none() {
            let generation = self.vector_generation();
            if let Some(active) = &mut self.quiver_ssd {
                active.reader.close();
                active.generation = generation;
            }
            return Ok(());
        }
        let Some(active) = &self.quiver_ssd else {
            return Ok(());
        };
        let (source, _, _) = source::<T>(path)?;
        if active.reader.stats().metadata.source == source {
            return Ok(());
        }
        let options = active.options;
        let limit = active.database_memory_limit;
        let mut reader = load_reader(self, path, limit, true, options.encoding)?;
        reader.configure_bound(options, self.quiver().ok_or(TriviumError::DatabaseClosed)?)?;
        reader.set_database_capacity(database_capacity(limit, self.base_memory_bytes())?);
        self.quiver_ssd = Some(DatabaseSsd {
            generation: self.vector_generation(),
            options,
            reader,
            database_memory_limit: limit,
        });
        Ok(())
    }
}

fn validate_ann_mode<T: VectorType>(
    mt: &crate::storage::memtable::MemTable<T>,
    options: SsdRuntimeOptions,
) -> Result<()> {
    if options.use_for_ann
        && mt
            .quiver()
            .is_some_and(|index| index.sm_native_l2_enabled() || index.corrected_sm_enabled())
    {
        return Err(TriviumError::InvalidInput("压缩ANN重排使用默认SM余弦模式 (Compressed ANN reranking requires default SM cosine mode)".into()).at_field("use_for_ann"));
    }
    Ok(())
}
