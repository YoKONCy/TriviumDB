//! 存储模式切换、冷向量配置及固定候选精排。

use super::*;
use crate::index::pelorus::engine::{RerankConfig, VectorStorageStats};

impl Index {
    pub(in crate::index::pelorus) fn prepare_shared_ssd(
        &mut self,
        runtime: &crate::ResourceRuntime,
        options: crate::index::pelorus::PelorusRuntimeOptions,
        available: usize,
    ) -> crate::Result<Option<crate::index::pelorus::io::PelorusSsd>> {
        if self.is_empty() {
            return Ok(None);
        }
        if self.rerank.is_none() {
            self.rerank = Some(
                ColdStore::canonical(
                    self.dimensions,
                    &self.vectors,
                    self.metric,
                    &self.boundary_factors,
                )
                .map_err(crate::index::pelorus::map_error)?,
            );
        }
        let store = self.rerank.as_ref().expect("精排记录已准备");
        let (file, offset, length) = store.bytes.file_source().ok_or_else(|| {
            crate::TriviumError::CorruptedFile(
                "Pelorus 直读需要已发布的精排区间 (Pelorus direct reads need a published rerank range)".into(),
            )
        })?;
        let (records, delta) = store.bytes.slices();
        if records.len() != length || !delta.is_empty() {
            return Err(crate::TriviumError::CorruptedFile(
                "Pelorus 已发布精排区间不一致 (Pelorus published rerank range is inconsistent)"
                    .into(),
            ));
        }
        crate::index::pelorus::io::PelorusSsd::new(
            runtime,
            &file,
            offset as u64,
            records,
            store.stride,
            options,
            available,
        )
        .map(Some)
    }

    pub fn rerank_config(&self) -> RerankConfig {
        self.rerank
            .as_ref()
            .map_or(RerankConfig::default(), |store| store.config)
    }

    /// 内嵌 Core 的 Hot 构建不创建临时文件，原始坐标继续由当前内存快照持有。
    pub(crate) fn set_hot_rerank_config(&mut self, config: RerankConfig) -> Result<()> {
        config.validate()?;
        self.rerank = if config == RerankConfig::default() {
            None
        } else {
            Some(ColdStore::build(
                config,
                self.dimensions,
                &self.vectors,
                &self.active,
                self.metric,
            )?)
        };
        Ok(())
    }

    pub fn vector_storage_stats(&self) -> VectorStorageStats {
        VectorStorageStats {
            canonical_heap_bytes: self.vectors.heap_bytes(),
            canonical_mapped_bytes: self.vectors.mapped_bytes(),
            rerank_heap_bytes: self
                .rerank
                .as_ref()
                .map_or(0, |store| store.bytes.heap_bytes()),
            rerank_mapped_bytes: self
                .rerank
                .as_ref()
                .map_or(0, |store| store.bytes.mapped_bytes()),
            shared_metadata_bytes: self
                .rerank
                .as_ref()
                .map_or(0, |store| store.center.len() * 4),
            rerank_metadata_bytes: self
                .rerank
                .as_ref()
                .map_or(0, |store| std::mem::size_of_val(store.prepared.as_slice())),
            rerank_record_bytes: self
                .rerank
                .as_ref()
                .map_or(self.dimensions * 4, |store| store.stride),
        }
    }

    pub(crate) fn native_space(&self, curvature: f64, rerank: bool) -> NativeSpace<'_> {
        let (vectors, delta) = self.vectors.slices();
        NativeSpace {
            vectors,
            delta,
            rerank: if rerank { self.rerank.as_ref() } else { None },
            boundary_factors: &self.boundary_factors,
            dimensions: self.dimensions,
            curvature,
            meter: None,
        }
    }
}
