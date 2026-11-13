//! 数据库绑定的INT8 QuIVer运行时，来源身份和刷新由内核维护。

use super::{AccessMode, Database, read_or_recover, write_or_recover};
use crate::index::quiver::{
    QuIVerSearchConfig,
    ssd::{SsdOptions, SsdQuiver, SsdRuntimeOptions, SsdStats},
};
use crate::{Result, TriviumError, VectorType};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub(crate) struct DatabaseSsd {
    pub generation: u64,
    pub options: SsdRuntimeOptions,
    pub reader: SsdQuiver,
    pub database_memory_limit: usize,
}

impl<T: VectorType + Serialize + DeserializeOwned> Database<T> {
    /// 选择INT8候选精排与驻留策略。数据库自动绑定已确认事实，按需发布派生制品。
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
            let generation = mt.vector_generation();
            let capacity = database_capacity(self.memory_limit, mt.base_memory_bytes())?;
            if let Some(active) = &mut mt.quiver_ssd
                && active.generation == generation
                && !active.reader.stats().closed
            {
                active.reader.set_database_capacity(capacity);
                active.reader.configure(options)?;
                active.options = options;
                return Ok(active.reader.stats());
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
        )?;
        reader.configure(options)?;
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
            (active.generation != mt.vector_generation()).then_some(active.options)
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
        active.reader.search(query, config)
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
) -> Result<SsdQuiver> {
    let index =
        mt.quiver().ok_or_else(|| {
            TriviumError::InvalidInput(
        "INT8 QuIVer需要非空浮点索引 (INT8 QuIVer requires a nonempty floating-point index)".into())
        })?;
    let path = PathBuf::from(format!("{db_path}.qssd"));
    let (source, hot_bytes, hot_crc) = source::<T>(db_path)?;
    let options = SsdOptions {
        memory_limit_bytes: database_capacity(limit, mt.estimated_memory_bytes())?,
        ..Default::default()
    };
    let runtime = mt.resource_runtime();
    if writable {
        SsdQuiver::collect_temporary(&path)?;
    }
    let open = || {
        SsdQuiver::open_bound(
            &path,
            source,
            options,
            runtime.clone(),
            Some((hot_bytes, hot_crc)),
        )
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
        let mut reader = load_reader(self, path, limit, true)?;
        reader.configure(options)?;
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
