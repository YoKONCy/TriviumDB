//! 显式SSD QuIVer索引，复用既有图、SM导航及候选排序。
//!
//! 宿主提供不可变事实来源摘要和稳定slot读取。发布逐行编码，Reader只装载热区，
//! 精排使用位置读取与INT8评分。单个Reader固定一个文件代际，关闭释放其文件及热额度。
//!
//! ```
//! use triviumdb::index::quiver::{QuIVer, QuIVerConfig, QuIVerSearchConfig};
//! use triviumdb::index::quiver::ssd::{SsdOptions, SsdQuiver};
//! use triviumdb::{ResourceLimits, ResourceRuntime};
//!
//! let path = std::env::temp_dir().join(format!("tdb-ssd-example-{}.qssd", std::process::id()));
//! let vectors = [[1f32, 0.], [0., 1.]];
//! let mut index = QuIVer::new(2, &QuIVerConfig::default());
//! index.batch_build(&[1., 0., 0., 1.], &[1, 2], &[0, 1]);
//! let runtime = ResourceRuntime::new(ResourceLimits::default())?;
//! // 示例来源身份；宿主使用包含数据库身份和事实代际的不可变清单摘要。
//! let source = [3u8; 32];
//! SsdQuiver::publish(&index, &path, source, SsdOptions::default(), &runtime,
//!     |slot, output| { output.copy_from_slice(&vectors[slot]); Ok(()) })?;
//! drop(index);
//! let mut reader = SsdQuiver::open(&path, source, SsdOptions::default(), runtime)?;
//! let config = QuIVerSearchConfig { top_k: 2, ef_search: 2, rerank_limit: Some(2) };
//! let hits = reader.search(&[1., 0.], &config)?;
//! assert_eq!(hits.iter().map(|hit| hit.0).collect::<Vec<_>>(), [1, 2]);
//! assert!((hits[0].1 - 1.).abs() < 1e-6);
//! assert!(hits[1].1.abs() < 1e-6);
//! reader.close();
//! std::fs::remove_file(path)?;
//! # Ok::<(), triviumdb::TriviumError>(())
//! ```

use super::cold::{Int8Layout, Int8Query};
use super::corrected_sm::ExactSource;
use super::{FlatAdj, QuIVer, QuIVerSearchConfig};
use crate::error::{Result, TriviumError, WriteErrorInfo, WritePhase};
use crate::index::bq::MAX_BQ_DIM;
use crate::resources::{ResourceRuntime, WorkspaceReservation};
use crate::storage::quiver_cold::{self, ColdFile, RECORD_HEADER_BYTES, RowIdentity};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::sync::Condvar;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub use crate::storage::quiver_cold::SsdMetadata;
pub use crate::storage::quiver_io::IoMode;

#[path = "ssd/batch.rs"]
mod batch;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Residency {
    #[default]
    Ssd,
    Hot,
}

/// 运行时策略独立于磁盘编码；Hot预热和SSD缓存使用相同的INT8行及评分。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SsdRuntimeOptions {
    pub residency: Residency,
    pub cache_bytes: usize,
    pub io_mode: IoMode,
    pub io_depth: usize,
}
impl Default for SsdRuntimeOptions {
    fn default() -> Self {
        Self {
            residency: Residency::Ssd,
            cache_bytes: 0,
            io_mode: IoMode::Buffered,
            io_depth: 4,
        }
    }
}
impl SsdRuntimeOptions {
    pub(crate) fn validate(&self) -> Result<()> {
        if !(1..=32).contains(&self.io_depth) {
            return Err(
                invalid("I/O深度须在1至32之间 (I/O depth must be between 1 and 32)")
                    .at_field("io_depth"),
            );
        }
        Ok(())
    }
}

/// 单次构建或Reader的资源配置；零内存上限沿用共享Runtime准入。
#[derive(Debug, Clone, Copy)]
pub struct SsdOptions {
    pub memory_limit_bytes: usize,
    /// 发布时控制编码行步长；Reader按制品头确定行布局，物理I/O使用平台位置读取。
    pub row_alignment: usize,
}
impl Default for SsdOptions {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 0,
            row_alignment: 64,
        }
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct SsdStats {
    pub metadata: SsdMetadata,
    pub closed: bool,
    pub reserved_hot_bytes: usize,
    pub reserved_bytes: usize,
    pub cold_read_calls: u64,
    pub cold_read_bytes: u64,
    pub physical_read_calls: u64,
    pub physical_read_bytes: u64,
    pub io_alignment_bytes: usize,
    pub io_memory_alignment_bytes: usize,
    pub io_alignment_detected: bool,
    pub io_window_bytes: u64,
    pub window_reuse_hits: u64,
    pub window_reuse_bytes: u64,
    pub residency: Residency,
    pub io_mode: IoMode,
    pub io_active: bool,
    pub cache_limit_bytes: usize,
    pub cache_used_bytes: usize,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub coalesced_reads: u64,
}

struct State {
    index: RoutingIndex,
    file: ColdFile,
    _hot: MemoryLease,
    hot_bytes: usize,
}

/// 只保留冷查询所需的路由及身份，加载校验后回收BQ构图数据、上层图和维护工作区。
struct RoutingIndex {
    dim: usize,
    n: usize,
    layer0: FlatAdj,
    model: super::corrected_sm::CorrectedSmIndex,
    ids: Vec<u64>,
    slot_indices: Vec<usize>,
    id_to_internal: std::collections::HashMap<u64, u32>,
    tombstones: Vec<bool>,
}

impl RoutingIndex {
    fn identity(&self, row: usize) -> Result<RowIdentity> {
        identity(
            &self.ids,
            &self.slot_indices,
            &self.tombstones,
            &self.id_to_internal,
            row,
        )
    }
}

/// 显式量化检索入口，持有热索引和同一代际的只读文件句柄。
pub struct SsdQuiver {
    state: Option<State>,
    metadata: SsdMetadata,
    memory: Memory,
    cache: Mutex<Cache>,
    hot_rows: Option<Vec<SsdRow>>,
    hot_directory: Option<MemoryLease>,
    runtime_options: SsdRuntimeOptions,
    io_memory: Option<MemoryLease>,
}

#[derive(Default)]
struct Cache {
    rows: BTreeMap<usize, (SsdRow, u128)>,
    lru: BTreeSet<(u128, usize)>,
    clock: u128,
    pending: BTreeMap<usize, Arc<Flight>>,
    bytes: usize,
    hits: u64,
    misses: u64,
    coalesced: u64,
}
#[derive(Default)]
struct Flight {
    state: Mutex<(bool, Option<SsdRow>)>,
    ready: Condvar,
}
impl Cache {
    fn trim(&mut self, limit: usize) {
        while self.bytes > limit {
            let Some((_, row)) = self.lru.pop_first() else {
                break;
            };
            if let Some((value, _)) = self.rows.remove(&row) {
                self.bytes -= value.data.charge;
            }
        }
    }
}

/// 自有冷行租约，继续持有行缓冲额度；文件句柄归Reader管理。
#[derive(Clone)]
pub struct SsdRow {
    layout: Int8Layout,
    data: Arc<RowData>,
}
struct RowData {
    bytes: Vec<u8>,
    charge: usize,
    _memory: MemoryLease,
}
impl SsdRow {
    pub fn decode_into(&self, output: &mut [f32]) -> Result<()> {
        self.layout
            .row(&self.data.bytes[RECORD_HEADER_BYTES..])?
            .decode_into(output)
    }
    pub fn score(&self, query: &Int8Query) -> Result<f32> {
        query.score(self.layout.row(&self.data.bytes[RECORD_HEADER_BYTES..])?)
    }
}

impl SsdQuiver {
    pub(crate) fn reserved_bytes(&self) -> usize {
        *crate::database::lock_or_recover(&self.memory.local.used)
    }

    pub(crate) fn set_database_capacity(&self, limit: usize) {
        // 已有租约按自身生命周期排空，新分配与预算更新使用同一把准入锁。
        let _used = crate::database::lock_or_recover(&self.memory.local.used);
        self.memory.local.limit.store(limit, Ordering::Relaxed);
    }
    /// 在目标发布租约下回收上次进程遗留的私有暂存制品。
    pub fn collect_temporary(path: &Path) -> Result<usize> {
        quiver_cold::collect(path)
    }
    /// 发布一个完整不可变SSD索引。source应为包含数据库身份及事实代际的清单摘要。
    /// fill必须在整个调用期间读取同一事实快照，并完整填充给定slot的浮点向量。
    pub fn publish(
        index: &QuIVer,
        path: &Path,
        source: [u8; 32],
        options: SsdOptions,
        runtime: &ResourceRuntime,
        mut fill: impl FnMut(usize, &mut [f32]) -> Result<()>,
    ) -> Result<SsdMetadata> {
        WriteErrorInfo::capture(|write| {
            crate::request::check_current()?;
            write.phase = WritePhase::Validation;
            if index.n == 0
                || index.n > u32::MAX as usize
                || index.default_sm.is_none()
                || index.m == 0
                || index.m0 == 0
                || !index.alpha.is_finite()
                || index.alpha <= 0.
            {
                return Err(invalid(
                    "SSD构建需要非空图及默认SM状态 (SSD build requires a nonempty graph and default SM state)",
                ));
            }
            let layout = Int8Layout::new(index.dim, options.row_alignment)?;
            let bytes = layout
                .row_stride_bytes()
                .checked_add(RECORD_HEADER_BYTES + 64 * 1024 + 4096 + MAX_BQ_DIM * 4)
                .and_then(|bytes| bytes.checked_add(index.dim * 4))
                .ok_or_else(capacity)?;
            let memory = Memory::new(runtime.clone(), options.memory_limit_bytes);
            let _memory = memory.reserve(bytes)?;
            let _task = runtime.enter_task()?;
            write.phase = WritePhase::Apply;
            quiver_cold::publish(
                path,
                source,
                layout,
                index.n,
                |writer| index.write_snapshot(writer).map_err(TriviumError::Io),
                |row, vector| {
                    let identity = identity(
                        &index.ids,
                        &index.slot_indices,
                        &index.tombstones,
                        &index.id_to_internal,
                        row,
                    )?;
                    if !identity.deleted {
                        fill(index.slot_indices[row], vector)?;
                    }
                    Ok(identity)
                },
                write,
            )
        })
    }

    /// 只读打开并绑定预期事实来源；启动读取头和热区，冷行在请求时验证。
    pub fn open(
        path: &Path,
        expected_source: [u8; 32],
        options: SsdOptions,
        runtime: ResourceRuntime,
    ) -> Result<Self> {
        Self::open_bound(path, expected_source, options, runtime, None)
    }

    pub(crate) fn open_bound(
        path: &Path,
        expected_source: [u8; 32],
        options: SsdOptions,
        runtime: ResourceRuntime,
        binding: Option<(u64, u32)>,
    ) -> Result<Self> {
        crate::request::check_current()?;
        let _task = runtime.enter_task()?;
        let file = match binding {
            Some(binding) => ColdFile::open_bound(path, expected_source, Some(binding))?,
            None => ColdFile::open(path, expected_source)?,
        };
        let metadata = file.metadata();
        let serialized = usize::try_from(metadata.hot_bytes).map_err(|_| capacity())?;
        let memory = Memory::new(runtime.clone(), options.memory_limit_bytes);
        // 先准入热区读取缓冲，再按已校验热区的节点和层数准入解码峰值。
        let initial = serialized.checked_add(64 * 1024).ok_or_else(capacity)?;
        let mut hot_memory = memory.reserve(initial)?;
        let hot = file.read_hot()?;
        let peak = loading_bytes(&hot, metadata)?;
        hot_memory.resize(peak)?;
        let index = {
            let mut decoded = QuIVer::load_from_bytes(&hot).map_err(|error| {
                TriviumError::CorruptedFile(format!(
                    "SSD热索引无效 (Invalid SSD hot index): {error}"
                ))
            })?;
            if decoded.n != metadata.rows || decoded.dim != metadata.dimension {
                return Err(TriviumError::CorruptedFile(
                    "SSD热区与目录不匹配 (SSD hot section and directory mismatch)".into(),
                ));
            }
            let model = decoded.default_sm.take().ok_or_else(|| {
                TriviumError::CorruptedFile(
                    "SSD默认SM状态缺失 (SSD default SM state is missing)".into(),
                )
            })?;
            RoutingIndex {
                dim: decoded.dim,
                n: decoded.n,
                layer0: decoded.layer0,
                model,
                ids: decoded.ids,
                slot_indices: decoded.slot_indices,
                id_to_internal: decoded.id_to_internal,
                tombstones: decoded.tombstones,
            }
        };
        drop(hot);
        crate::request::check_current()?;
        let retained = retained_bytes(&index)?;
        if retained > peak {
            return Err(capacity());
        }
        hot_memory.resize(retained)?;
        crate::request::check_current()?;
        Ok(Self {
            state: Some(State {
                index,
                file,
                _hot: hot_memory,
                hot_bytes: retained,
            }),
            metadata,
            memory,
            cache: Mutex::new(Cache::default()),
            hot_rows: None,
            hot_directory: None,
            runtime_options: SsdRuntimeOptions::default(),
            io_memory: None,
        })
    }

    /// 预热成功后替换驻留策略，失败或取消保持既有策略；调用方独占Reader完成切换。
    pub fn configure(&mut self, options: SsdRuntimeOptions) -> Result<()> {
        self.active()?;
        crate::request::check_current()?;
        options.validate()?;
        let _task = self.memory.runtime.enter_task()?;
        let change_io = options.residency == Residency::Ssd
            && (!self.active()?.file.has_async_io()
                || options.io_mode != self.runtime_options.io_mode
                || options.io_depth != self.runtime_options.io_depth);
        let io_bytes = options
            .io_depth
            .min(self.memory.runtime.max_threads().saturating_sub(1))
            .max(1)
            .checked_mul(256 * 1024)
            .ok_or_else(capacity)?;
        let mut io_memory = if change_io {
            Some(
                self.memory
                    .reserve(io_bytes.checked_add(131072).ok_or_else(capacity)?)?,
            )
        } else {
            None
        };
        let mut warm = None;
        let mut directory = None;
        if options.residency == Residency::Hot && self.hot_rows.is_none() {
            directory = Some(
                self.memory.reserve(
                    self.metadata
                        .rows
                        .checked_mul(std::mem::size_of::<SsdRow>())
                        .ok_or_else(capacity)?,
                )?,
            );
            let mut rows = Vec::new();
            rows.try_reserve_exact(self.metadata.rows)
                .map_err(|_| capacity())?;
            for start in (0..self.metadata.rows).step_by(4) {
                crate::request::check_current()?;
                let count = (self.metadata.rows - start).min(4);
                let mut batch = [start; 4];
                for (offset, row) in batch[..count].iter_mut().enumerate() {
                    *row += offset;
                }
                rows.extend(self.load_records(&batch[..count])?);
            }
            warm = Some(rows);
        }
        crate::request::check_current()?;
        if let Some(mut memory) = io_memory.take() {
            self.state
                .as_mut()
                .ok_or(TriviumError::DatabaseClosed)?
                .file
                .configure_io(options.io_mode, options.io_depth, &self.memory.runtime)?;
            memory.resize(io_bytes)?;
            self.io_memory = Some(memory);
        }
        if options.residency == Residency::Hot {
            self.state
                .as_mut()
                .ok_or(TriviumError::DatabaseClosed)?
                .file
                .close_io();
            self.io_memory.take();
        }
        if let Some(rows) = warm {
            self.hot_rows = Some(rows);
            self.hot_directory = directory;
        }
        if options.residency == Residency::Ssd {
            self.hot_rows = None;
            self.hot_directory = None;
        }
        crate::database::lock_or_recover(&self.cache).trim(options.cache_bytes);
        self.runtime_options = options;
        Ok(())
    }

    pub fn stats(&self) -> SsdStats {
        let cache = crate::database::lock_or_recover(&self.cache);
        let alignment = self
            .state
            .as_ref()
            .map_or_else(Default::default, |state| state.file.io_alignment());
        SsdStats {
            metadata: self.metadata,
            closed: self.state.is_none(),
            reserved_hot_bytes: self.state.as_ref().map_or(0, |state| state.hot_bytes),
            reserved_bytes: *crate::database::lock_or_recover(&self.memory.local.used),
            cold_read_calls: self
                .state
                .as_ref()
                .map_or(0, |state| state.file.counters.calls.load(Ordering::Relaxed)),
            cold_read_bytes: self
                .state
                .as_ref()
                .map_or(0, |state| state.file.counters.bytes.load(Ordering::Relaxed)),
            physical_read_calls: self
                .state
                .as_ref()
                .map_or(0, |state| state.file.physical.calls.load(Ordering::Relaxed)),
            physical_read_bytes: self
                .state
                .as_ref()
                .map_or(0, |state| state.file.physical.bytes.load(Ordering::Relaxed)),
            io_alignment_bytes: alignment.offset,
            io_memory_alignment_bytes: alignment.memory,
            io_alignment_detected: alignment.detected,
            io_window_bytes: self.state.as_ref().map_or(0, |state| {
                state.file.physical.window_bytes.load(Ordering::Relaxed)
            }),
            window_reuse_hits: self.state.as_ref().map_or(0, |state| {
                state.file.physical.window_hits.load(Ordering::Relaxed)
            }),
            window_reuse_bytes: self.state.as_ref().map_or(0, |state| {
                state.file.physical.window_hit_bytes.load(Ordering::Relaxed)
            }),
            residency: self.runtime_options.residency,
            io_mode: self
                .state
                .as_ref()
                .map_or(IoMode::Buffered, |state| state.file.io_mode()),
            io_active: self
                .state
                .as_ref()
                .is_some_and(|state| state.file.has_async_io()),
            cache_limit_bytes: self.runtime_options.cache_bytes,
            cache_used_bytes: cache.bytes,
            cache_hits: cache.hits,
            cache_misses: cache.misses,
            coalesced_reads: cache.coalesced,
        }
    }

    /// 取得活跃节点的自有行租约；后续Reader关闭和新版发布不改变该行内容。
    pub fn row(&self, node_id: u64) -> Result<SsdRow> {
        let state = self.active()?;
        crate::request::check_current()?;
        let row = state
            .index
            .id_to_internal
            .get(&node_id)
            .copied()
            .filter(|&row| !state.index.tombstones[row as usize])
            .ok_or(TriviumError::NodeNotFound(node_id))? as usize;
        self.load_record(row)
    }

    /// 复用默认SM图导航，以codec 1近似余弦精排并按分数、NodeId稳定排序。
    pub fn search(&self, query: &[f32], config: &QuIVerSearchConfig) -> Result<Vec<(u64, f32)>> {
        let state = self.active()?;
        crate::vector::validate_query_vector(query, state.index.dim)?;
        crate::request::check_current()?;
        if config.top_k == 0 {
            return Ok(Vec::new());
        }
        let top_k = config.top_k.min(state.index.n);
        let ef = config.ef_search.max(top_k).min(state.index.n);
        let budget = config.rerank_limit.unwrap_or(64).max(top_k).min(ef);
        // 导航候选队列在最坏情况下可覆盖全图，准入同时覆盖堆扩容和结果工作区。
        let bytes = state
            .index
            .n
            .checked_mul(96)
            .and_then(|bytes| bytes.checked_add(self.metadata.record_bytes))
            .and_then(|bytes| bytes.checked_add(state.index.n.div_ceil(64) * 8))
            .and_then(|bytes| bytes.checked_add(64 * 1024))
            .ok_or_else(capacity)?;
        let _memory = self.memory.reserve(bytes)?;
        let _task = self.memory.runtime.enter_task()?;
        let quantized = Int8Query::new(query)?;
        let mut source = Source {
            state,
            reader: self,
            query: quantized,
            error: None,
        };
        let squared = query
            .iter()
            .map(|&value| f64::from(value).powi(2))
            .sum::<f64>();
        let norm = if squared == 0. { 1. } else { squared.sqrt() };
        let mut normalized = [0f32; MAX_BQ_DIM];
        for (target, &value) in normalized.iter_mut().zip(query) {
            *target = (f64::from(value) / norm) as f32;
        }
        let config = QuIVerSearchConfig {
            top_k,
            ef_search: ef,
            rerank_limit: Some(budget),
        };
        let output = state.index.model.search_routed_source(
            &normalized[..query.len()],
            &state.index.layer0,
            &config,
            &mut source,
        );
        if let Some(error) = source.error {
            return Err(error);
        }
        let result = output.map_err(TriviumError::Io)?;
        crate::request::check_current()?;
        Ok(result.neighbors)
    }

    /// 关闭当前Reader，释放热索引、文件和额度；自有行租约按各自生命周期归还缓冲额度。
    pub fn close(&mut self) {
        self.hot_rows.take();
        self.hot_directory.take();
        *crate::database::lock_or_recover(&self.cache) = Cache::default();
        self.state.take();
        self.io_memory.take();
    }

    fn active(&self) -> Result<&State> {
        self.state.as_ref().ok_or(TriviumError::DatabaseClosed)
    }
}

struct Source<'a> {
    state: &'a State,
    reader: &'a SsdQuiver,
    query: Int8Query,
    error: Option<TriviumError>,
}
impl Source<'_> {
    fn score(&mut self, row: usize) -> Result<Option<f32>> {
        let identity = self.state.index.identity(row)?;
        if identity.deleted {
            return Ok(None);
        }
        self.reader.load_record(row)?.score(&self.query).map(Some)
    }
    fn capture<T>(&mut self, result: Result<T>) -> io::Result<T> {
        result.map_err(|error| {
            self.error = Some(error);
            io::Error::other("SSD读取或请求控制失败 (SSD read or request control failed)")
        })
    }
}
impl ExactSource for Source<'_> {
    fn owns_workspace(&self) -> bool {
        true
    }
    fn checkpoint(&mut self) -> io::Result<()> {
        self.capture(crate::request::check_current())
    }
    fn one(&mut self, _query: &[f32], row: usize) -> io::Result<Option<f32>> {
        let result = self.score(row);
        self.capture(result)
    }
    fn four(&mut self, _query: &[f32], rows: [usize; 4]) -> io::Result<[Option<f32>; 4]> {
        let result = self.score_four(rows);
        self.capture(result)
    }
}

impl Source<'_> {
    fn score_four(&self, rows: [usize; 4]) -> Result<[Option<f32>; 4]> {
        let mut scores = [None; 4];
        let mut active = Vec::with_capacity(4);
        for (position, row) in rows.into_iter().enumerate() {
            if !self.state.index.identity(row)?.deleted {
                active.push((position, row));
            }
        }
        let indices = active.iter().map(|&(_, row)| row).collect::<Vec<_>>();
        let loaded = self.reader.load_records(&indices)?;
        for ((position, _), value) in active.into_iter().zip(loaded) {
            scores[position] = Some(value.score(&self.query)?);
        }
        Ok(scores)
    }
}

fn identity(
    ids: &[u64],
    slots: &[usize],
    tombstones: &[bool],
    id_to_internal: &std::collections::HashMap<u64, u32>,
    row: usize,
) -> Result<RowIdentity> {
    if row >= ids.len() || row >= slots.len() || row >= tombstones.len() {
        return Err(TriviumError::CorruptedFile(
            "SSD内部行越界 (SSD internal row out of bounds)".into(),
        ));
    }
    if ids[row] == 0
        || (!tombstones[row] && id_to_internal.get(&ids[row]).copied() != Some(row as u32))
    {
        return Err(invalid(
            "SSD活跃节点身份无效 (Invalid SSD active node identity)",
        ));
    }
    Ok(RowIdentity {
        node_id: ids[row],
        slot: slots[row] as u64,
        deleted: tombstones[row],
    })
}

fn retained_bytes(index: &RoutingIndex) -> Result<usize> {
    index
        .layer0
        .data
        .capacity()
        .checked_mul(4)
        .ok_or_else(capacity)?
        .checked_add(index.model.routing_owned_bytes())
        .ok_or_else(capacity)?
        .checked_add(index.ids.capacity().checked_mul(8).ok_or_else(capacity)?)
        .and_then(|bytes| {
            bytes.checked_add(
                index
                    .slot_indices
                    .capacity()
                    .checked_mul(std::mem::size_of::<usize>())?,
            )
        })
        .and_then(|bytes| bytes.checked_add(index.tombstones.capacity()))
        // HashMap预算覆盖桶、控制字节和容量取整；其余加载后的连续数组按实际容量计量。
        .and_then(|bytes| bytes.checked_add(index.id_to_internal.capacity().checked_mul(64)?))
        // 固定目录覆盖Reader缓存账本、空BTree根节点及读取计数器。
        .and_then(|bytes| bytes.checked_add(4096))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<State>()))
        .ok_or_else(capacity)
}

fn loading_bytes(hot: &[u8], metadata: SsdMetadata) -> Result<usize> {
    let word = |offset: usize| {
        u32::from_le_bytes([
            hot[offset],
            hot[offset + 1],
            hot[offset + 2],
            hot[offset + 3],
        ]) as usize
    };
    if hot.len() < 48
        || word(8) != metadata.dimension
        || word(12) != metadata.rows
        || word(36) > metadata.rows
    {
        return Err(TriviumError::CorruptedFile(
            "SSD热区计数与目录不匹配 (SSD hot counts and directory mismatch)".into(),
        ));
    }
    let layers = word(36);
    // 热区临时字节、解码数组、映射以及逐层Vec描述符在分配前统一准入。
    let per_node = layers
        .checked_mul(48)
        .and_then(|bytes| bytes.checked_add(256))
        .ok_or_else(capacity)?;
    hot.len()
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(metadata.rows.checked_mul(per_node)?))
        .and_then(|bytes| bytes.checked_add(layers.checked_mul(48)?))
        .and_then(|bytes| bytes.checked_add(64 * 1024))
        .ok_or_else(capacity)
}

struct LocalBudget {
    used: Mutex<usize>,
    limit: AtomicUsize,
}

impl LocalBudget {
    fn projected(&self, used: usize, bytes: usize) -> Result<usize> {
        let projected = used.checked_add(bytes).ok_or_else(capacity)?;
        let limit = self.limit.load(Ordering::Relaxed);
        if limit > 0 && projected > limit {
            return Err(TriviumError::ResourceBudgetExceeded {
                resource: "quiver_ssd_memory_bytes",
                requested: bytes,
                used,
                limit,
            }
            .at_field("memory_limit_bytes"));
        }
        Ok(projected)
    }
}

struct Memory {
    runtime: ResourceRuntime,
    local: Arc<LocalBudget>,
}

impl Memory {
    fn new(runtime: ResourceRuntime, limit: usize) -> Self {
        Self {
            runtime,
            local: Arc::new(LocalBudget {
                used: Mutex::new(0),
                limit: AtomicUsize::new(limit),
            }),
        }
    }

    fn reserve(&self, bytes: usize) -> Result<MemoryLease> {
        let global = self.runtime.reserve_workspace(bytes)?;
        let mut used = crate::database::lock_or_recover(&self.local.used);
        let projected = self.local.projected(*used, bytes)?;
        *used = projected;
        Ok(MemoryLease {
            global,
            local: self.local.clone(),
            bytes,
        })
    }
}

/// 同时持有Reader局部和共享Runtime额度，行对象可独立于Reader继续持有该凭证。
struct MemoryLease {
    global: WorkspaceReservation,
    local: Arc<LocalBudget>,
    bytes: usize,
}
impl MemoryLease {
    fn resize(&mut self, bytes: usize) -> Result<()> {
        if bytes > self.bytes {
            let mut used = crate::database::lock_or_recover(&self.local.used);
            let projected = self.local.projected(*used, bytes - self.bytes)?;
            self.global.resize(bytes)?;
            *used = projected;
        } else {
            self.global.resize(bytes)?;
            *crate::database::lock_or_recover(&self.local.used) -= self.bytes - bytes;
        }
        self.bytes = bytes;
        Ok(())
    }
}
impl Drop for MemoryLease {
    fn drop(&mut self) {
        *crate::database::lock_or_recover(&self.local.used) -= self.bytes;
    }
}
fn invalid(message: &str) -> TriviumError {
    TriviumError::InvalidInput(message.into())
}
fn capacity() -> TriviumError {
    TriviumError::CapacityAllocationFailed {
        reason: "SSD索引容量超出范围 (SSD index capacity exceeds range)".into(),
    }
}

#[cfg(test)]
#[path = "ssd/tests.rs"]
mod tests;
