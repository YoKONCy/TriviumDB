//! 数据库核心模块
//!
//! 按职责划分为以下模块：
//! - `config.rs`: 配置类型（StorageMode, Config, SearchConfig）
//! - `facade.rs`: Reader / Writer 公共门面
//! - `lifecycle.rs`: 可观测关闭、排空与超时等待
//! - `pipeline.rs`: 混合检索管线（L0-L9 + 6 个 Hook 调用点）
//! - `transaction.rs`: 轻量级事务（Dry-Run + WAL-first 语义）+ WAL 回放
//! - `mod.rs`（本文件）: Database 结构体 + CRUD + 生命周期管理

pub mod config;
pub mod facade;
mod lifecycle;
mod migration;
pub use lifecycle::{InstanceMemoryStats, InstanceStats, ShutdownPhase, ShutdownStatus};
pub(crate) mod pipeline;
pub mod transaction;

// 从子模块重导出公开类型，保持对外 API 不变
pub use config::{
    AccessMode, BatchSearchConfig, Config, EdgeDirection, MissingIndexPolicy, RowOverflowPolicy,
    SearchConfig, StorageMode,
};
pub use facade::{DatabaseReader, DatabaseWriter};
pub use transaction::{Transaction, TxBuilder};

use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::hook::{HookContext, NoopHook, SearchHook};

use crate::node::{NodeId, SearchHit};
use crate::storage::compaction::CompactionThread;
use crate::storage::file_format;
use crate::storage::memtable::MemTable;
use crate::storage::wal::{SyncMode, Wal, WalEntry, WalStats};
use fs2::FileExt;
use rayon::prelude::*;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

type HookSlot = Arc<RwLock<Arc<dyn SearchHook>>>;

fn clear_hook_slot(slot: &HookSlot) {
    let previous = {
        let mut hook = write_or_recover(slot);
        std::mem::replace(&mut *hook, Arc::new(NoopHook))
    };
    // 用户 Hook 的析构可重入诊断接口，析构时不持有 Hook 槽锁。
    drop(previous);
}

/// 安全获取 Mutex 锁：如果锁中毒（某个线程 panic 持有锁），
/// 则恢复内部数据继续运行，而不是 panic 整个进程。
pub(crate) fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("互斥锁中毒，正在恢复 (Mutex was poisoned, recovering...)");
        poisoned.into_inner()
    })
}

pub(crate) fn read_or_recover<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| {
        tracing::warn!("读写锁中毒，正在恢复只读访问 (RwLock was poisoned, recovering read...)");
        poisoned.into_inner()
    })
}

pub(crate) fn write_or_recover<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|poisoned| {
        tracing::warn!("读写锁中毒，正在恢复写访问 (RwLock was poisoned, recovering write...)");
        poisoned.into_inner()
    })
}

fn normalized_database_path(path: &str) -> Result<PathBuf> {
    let requested = Path::new(path);
    if requested.exists() {
        let metadata = requested.metadata().map_err(TriviumError::Io)?;
        reject_hard_link_alias(path, &metadata)?;
        return requested.canonicalize().map_err(TriviumError::Io);
    }
    let file_name = requested.file_name().ok_or_else(|| {
        TriviumError::InvalidInput(format!(
            "数据库路径缺少文件名 (Database path has no file name): {path}"
        ))
    })?;
    let parent = requested
        .parent()
        .filter(|value| !value.as_os_str().is_empty());
    let parent = match parent {
        Some(parent) => parent.canonicalize().map_err(TriviumError::Io)?,
        None => std::env::current_dir().map_err(TriviumError::Io)?,
    };
    Ok(parent.join(file_name))
}

#[cfg(unix)]
fn reject_hard_link_alias(path: &str, metadata: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    if metadata.nlink() > 1 {
        return Err(TriviumError::InvalidInput(format!(
            "数据库主文件不能是硬链接 (Database file must not have hard links): {path}"
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn reject_hard_link_alias(_path: &str, _metadata: &std::fs::Metadata) -> Result<()> {
    Ok(())
}

thread_local! {
    static HOOK_READ_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) struct HookReadScope;

impl HookReadScope {
    pub(crate) fn enter() -> Self {
        HOOK_READ_DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self
    }
}

impl Drop for HookReadScope {
    fn drop(&mut self) {
        HOOK_READ_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

pub(crate) fn reject_hook_reentrant_write() -> Result<()> {
    if HOOK_READ_DEPTH.with(|depth| depth.get() > 0) {
        return Err(TriviumError::InvalidInput(
            "Hook 在检索读锁内禁止重入同一线程的数据库写操作".into(),
        ));
    }
    Ok(())
}

// 锁顺序不变量：
// 1. 有状态查询：stateful_search → MemTable read；
// 2. 持久化写入：MemTable write → WAL Mutex；
// 3. QuIVer 构建注册表不得在持有 MemTable guard 时等待 Condvar。
// 禁止反向获取，避免 stateful/MemTable/WAL 之间形成 ABBA 死锁。

pub(crate) struct QuiverBuildGuard {
    generation: u64,
    state: Arc<(Mutex<std::collections::HashSet<u64>>, std::sync::Condvar)>,
}

impl Drop for QuiverBuildGuard {
    fn drop(&mut self) {
        let (building, changed) = &*self.state;
        lock_or_recover(building).remove(&self.generation);
        changed.notify_all();
    }
}

pub(crate) fn try_start_quiver_build(
    state: &Arc<(Mutex<std::collections::HashSet<u64>>, std::sync::Condvar)>,
    generation: u64,
) -> Option<QuiverBuildGuard> {
    let mut building = lock_or_recover(&state.0);
    if !building.insert(generation) {
        return None;
    }
    Some(QuiverBuildGuard {
        generation,
        state: Arc::clone(state),
    })
}

/// 已完成资格检查和快照捕获、可在线程间移动的 QuIVer 后台构建任务。
pub struct PreparedQuiverBuild<T: VectorType> {
    snapshot: crate::storage::memtable::QuiverBuildSnapshot,
    config: crate::index::quiver::QuIVerConfig,
    memtable: Arc<RwLock<MemTable<T>>>,
    _build_guard: QuiverBuildGuard,
    _operation_guard: OperationGuard,
    _workspace: crate::resources::WorkspaceReservation,
    resource_runtime: crate::resources::ResourceRuntime,
}

impl<T: VectorType> PreparedQuiverBuild<T> {
    /// 在不持有调用方 Database guard 的情况下构建，并仅在向量代际未变化时发布。
    pub fn execute(self) -> Result<bool> {
        let source_generation = self.snapshot.generation;
        let pool = self
            .resource_runtime
            .pool(self.resource_runtime.parallelism())?;
        let index = pool.install_task(Some(self._operation_guard.resource_task.id()), || {
            MemTable::<T>::build_quiver_snapshot(self.snapshot, &self.config)
        });
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeQuiverPublish);
        Ok(write_or_recover(&self.memtable).publish_quiver_if_current(source_generation, index))
    }
}

/// 可脱离 `Database` 借用执行资格检查、快照和后台构建的轻量句柄。
#[derive(Clone)]
pub struct QuiverBuildHandle<T: VectorType> {
    memtable: Arc<RwLock<MemTable<T>>>,
    memory_limit: usize,
    resource_runtime: crate::resources::ResourceRuntime,
    quiver_builds: Arc<(Mutex<std::collections::HashSet<u64>>, Condvar)>,
    lifecycle: Arc<(Mutex<LifecycleState>, Condvar)>,
    access_mode: AccessMode,
}

impl<T: VectorType> QuiverBuildHandle<T> {
    /// 若当前配置和数据状态满足自动构建条件，捕获后台 QuIVer 构建任务。
    pub fn prepare_auto_build(&self) -> Result<Option<PreparedQuiverBuild<T>>> {
        if self.access_mode != AccessMode::ReadWrite {
            return Err(TriviumError::ReadOnlyViolation {
                operation: "prepare_auto_quiver_build",
            });
        }
        let mut resource_task = self.resource_runtime.enter_task()?;
        resource_task.detach();
        let operation_guard = {
            let mut state = lock_or_recover(&self.lifecycle.0);
            if state.recovery_required.load(Ordering::Acquire) {
                return Err(TriviumError::WalClosed);
            }
            if state.lifecycle != DatabaseLifecycle::Open {
                return Err(TriviumError::DatabaseClosed);
            }
            state.active_operations = state
                .active_operations
                .checked_add(1)
                .ok_or_else(|| TriviumError::InvalidInput("数据库活动操作计数溢出".into()))?;
            OperationGuard {
                resource_task,
                lifecycle: Arc::clone(&self.lifecycle),
            }
        };
        let config = crate::index::quiver::QuIVerConfig::default();
        let (snapshot, workspace) = {
            let mt = read_or_recover(&self.memtable);
            if !mt.auto_quiver_build_needed() {
                return Ok(None);
            }
            let projected = mt
                .estimated_memory_bytes()
                .saturating_add(mt.quiver_build_peak_bytes(&config));
            if self.memory_limit > 0 && projected > self.memory_limit {
                return Err(TriviumError::InvalidInput(format!(
                    "QuIVer 自动构建预计峰值 {}MB 超过内存上限 {}MB",
                    projected / (1024 * 1024),
                    self.memory_limit / (1024 * 1024)
                )));
            }
            let workspace = self
                .resource_runtime
                .reserve_workspace(mt.quiver_build_peak_bytes(&config))?;
            (mt.quiver_build_snapshot(), workspace)
        };
        let Some(snapshot) = snapshot else {
            return Ok(None);
        };
        let Some(build_guard) = try_start_quiver_build(&self.quiver_builds, snapshot.generation)
        else {
            return Ok(None);
        };
        Ok(Some(PreparedQuiverBuild {
            snapshot,
            config,
            memtable: Arc::clone(&self.memtable),
            _build_guard: build_guard,
            _operation_guard: operation_guard,
            _workspace: workspace,
            resource_runtime: self.resource_runtime.clone(),
        }))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DatabaseLifecycle {
    Open,
    Closing,
    Closed,
}

struct LifecycleState {
    recovery_required: Arc<AtomicBool>,
    lifecycle: DatabaseLifecycle,
    active_operations: usize,
    group_commit_owner: Option<std::thread::ThreadId>,
    shutdown_phase: ShutdownPhase,
    shutdown_error: Option<TriviumError>,
    shutdown_last_error: Option<String>,
    shutdown_last_failure: Option<crate::resources::ResourceFailure>,
    shutdown_job: Option<crate::storage::compaction::ScheduledMaintenance>,
}

struct OperationGuard {
    resource_task: crate::resources::TaskLease,
    lifecycle: Arc<(Mutex<LifecycleState>, Condvar)>,
}

/// 闭包异常展开时也结束同步组，避免后续普通写入沿用延期同步状态。
struct GroupCommitGuard {
    wal: Arc<Mutex<Wal>>,
    lifecycle: Arc<(Mutex<LifecycleState>, Condvar)>,
    active: bool,
}

impl GroupCommitGuard {
    fn finish(&mut self) -> Result<bool> {
        let mut wal = lock_or_recover(&self.wal);
        let result = wal.finish_group_commit();
        self.active = false;
        if result.is_err() {
            wal.release_writer();
        }
        lock_or_recover(&self.lifecycle.0).group_commit_owner = None;
        result
    }
}

impl Drop for GroupCommitGuard {
    fn drop(&mut self) {
        if self.active
            && let Err(error) = self.finish()
        {
            tracing::error!(%error, "分组提交异常展开期间同步失败，已关闭 WAL 写入能力");
        }
    }
}

impl OperationGuard {
    fn check_recovery(&self) -> Result<()> {
        if lock_or_recover(&self.lifecycle.0)
            .recovery_required
            .load(Ordering::Acquire)
        {
            Err(TriviumError::WalClosed)
        } else {
            Ok(())
        }
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        let mut state = lock_or_recover(&self.lifecycle.0);
        state.active_operations = state.active_operations.saturating_sub(1);
        if state.active_operations == 0 {
            self.lifecycle.1.notify_all();
        }
    }
}

/// 持久化与索引状态的公共诊断快照。
#[derive(Debug, Clone, serde::Serialize)]
pub struct StorageInfo {
    pub package_version: &'static str,
    pub database_format_current: u16,
    pub database_format_minimum: u16,
    pub wal_format: u16,
    pub property_index_format: u16,
    pub graph_index_format: u16,
    pub quiver_format: u32,
    pub text_index_format: u32,
    pub manifest_format: u16,
    pub dim: usize,
    pub node_count: usize,
    pub storage_mode: &'static str,
    pub access_mode: &'static str,
    pub estimated_memory_bytes: usize,
    pub sidecars: std::collections::BTreeMap<String, bool>,
}

/// 数据库核心入口实例
pub struct Database<T: VectorType> {
    pub(crate) db_path: String,
    pub(crate) memtable: Arc<RwLock<MemTable<T>>>,
    pub(crate) wal: Arc<Mutex<Wal>>,
    pub(crate) compaction: Option<CompactionThread>,
    /// 文件锁：防止多进程同时打开同一个数据库
    /// Option 化以便 close() 时显式 take 释放（否则锁要等对象 Drop，JS GC 时机不可控）
    _lock_file: Arc<Mutex<Option<std::fs::File>>>,
    /// 内存上限（字节），0 = 无限制
    memory_limit: usize,
    resource_runtime: crate::resources::ResourceRuntime,
    /// TQL 默认行上限与溢出策略。
    tql_limits: crate::query::tql_executor::TqlLimits,
    /// 存储模式
    pub(crate) storage_mode: StorageMode,
    /// 检索管线共享 Hook 槽，默认使用空实现 NoopHook
    hook: HookSlot,
    /// 有状态 fatigue 查询保持数据库级顺序语义。
    stateful_search: Arc<Mutex<()>>,
    /// 正在锁外构建的 QuIVer 向量代际，避免重复构建。
    quiver_builds: Arc<(Mutex<std::collections::HashSet<u64>>, std::sync::Condvar)>,
    /// 生命周期和已进入操作计数；Closing 阻止新操作并等待现有操作退出。
    lifecycle: Arc<(Mutex<LifecycleState>, Condvar)>,
    access_mode: AccessMode,
}

pub(crate) struct SearchHandle<T: VectorType> {
    memtable: Arc<RwLock<MemTable<T>>>,
    memory_limit: usize,
    resource_runtime: crate::resources::ResourceRuntime,
    hook: HookSlot,
    quiver_builds: Arc<(Mutex<std::collections::HashSet<u64>>, Condvar)>,
    lifecycle: Arc<(Mutex<LifecycleState>, Condvar)>,
}

impl<T: VectorType> Clone for SearchHandle<T> {
    fn clone(&self) -> Self {
        Self {
            memtable: Arc::clone(&self.memtable),
            memory_limit: self.memory_limit,
            resource_runtime: self.resource_runtime.clone(),
            hook: Arc::clone(&self.hook),
            quiver_builds: Arc::clone(&self.quiver_builds),
            lifecycle: Arc::clone(&self.lifecycle),
        }
    }
}

impl<T: VectorType> SearchHandle<T> {
    fn hook(&self) -> Arc<dyn SearchHook> {
        read_or_recover(&self.hook).clone()
    }

    fn enter_operation(&self) -> Result<OperationGuard> {
        let mut state = lock_or_recover(&self.lifecycle.0);
        if state.recovery_required.load(Ordering::Acquire) {
            return Err(TriviumError::WalClosed);
        }
        if state.lifecycle != DatabaseLifecycle::Open {
            return Err(TriviumError::DatabaseClosed);
        }
        let resource_task = self.resource_runtime.enter_task()?;
        state.active_operations = state
            .active_operations
            .checked_add(1)
            .ok_or_else(|| TriviumError::InvalidInput("数据库活动操作计数溢出".into()))?;
        drop(state);
        Ok(OperationGuard {
            resource_task,
            lifecycle: Arc::clone(&self.lifecycle),
        })
    }

    pub(crate) fn search_batch(
        &self,
        query_vectors: &[Vec<T>],
        search_config: &SearchConfig,
        batch_config: &BatchSearchConfig,
    ) -> Result<Vec<Vec<SearchHit>>> {
        let _operation = self.enter_operation()?;
        if search_config.enable_refractory_fatigue {
            return Err(TriviumError::InvalidInput(
                "批量并行查询不支持 enable_refractory_fatigue".into(),
            ));
        }
        if search_config.top_k == 0 {
            return Err(TriviumError::InvalidInput(
                "批量查询 top_k 必须大于 0".into(),
            ));
        }
        if batch_config.parallelism > 64 {
            return Err(TriviumError::InvalidInput(
                "批量查询 parallelism 不得超过 64".into(),
            ));
        }
        if query_vectors.is_empty() {
            return Ok(Vec::new());
        }
        let dim = read_or_recover(&self.memtable).dim();
        for query in query_vectors {
            if query.len() != dim {
                return Err(TriviumError::DimensionMismatch {
                    expected: dim,
                    got: query.len(),
                });
            }
            if query.iter().any(|value| !value.to_f32().is_finite()) {
                return Err(TriviumError::InvalidVector {
                    reason: "查询向量包含 NaN 或 Infinity (Query vector contains NaN or Infinity)"
                        .into(),
                });
            }
        }

        let search_one = |query: &Vec<T>| {
            let mut ctx = HookContext::new();
            pipeline::execute_pipeline_with_limit(
                &self.memtable,
                &self.quiver_builds,
                self.memory_limit,
                &self.hook(),
                None,
                Some(query),
                search_config,
                &mut ctx,
            )
            .map(|output| output.combined_hits)
        };
        let first = search_one(&query_vectors[0])?;
        if query_vectors.len() == 1 {
            _operation.check_recovery()?;
            return Ok(vec![first]);
        }

        let available = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1);
        let parallelism = if batch_config.parallelism == 0 {
            available
        } else {
            batch_config.parallelism
        }
        .min(query_vectors.len() - 1)
        .max(1);
        let pool = self.resource_runtime.pool(parallelism)?;
        let batch_task = self.resource_runtime.current_task();
        let remaining_count = query_vectors.len() - 1;
        let chunk_size = remaining_count.div_ceil(parallelism);
        let chunks = pool.install(|| {
            query_vectors[1..]
                .par_chunks(chunk_size)
                .map(|chunk| {
                    let _binding = self.resource_runtime.bind_task(batch_task);
                    chunk.iter().map(search_one).collect::<Result<Vec<_>>>()
                })
                .collect::<Result<Vec<_>>>()
        })?;
        let mut results = Vec::with_capacity(query_vectors.len());
        results.push(first);
        results.extend(chunks.into_iter().flatten());
        _operation.check_recovery()?;
        Ok(results)
    }
}

impl<T: VectorType + serde::Serialize + serde::de::DeserializeOwned> Database<T> {
    fn enter_operation(&self) -> Result<OperationGuard> {
        self.enter_operation_kind(false)
    }

    fn enter_maintenance_operation(&self) -> Result<OperationGuard> {
        self.enter_operation_kind(true)
    }

    fn enter_operation_kind(&self, maintenance: bool) -> Result<OperationGuard> {
        let mut state = lock_or_recover(&self.lifecycle.0);
        if state.recovery_required.load(Ordering::Acquire) {
            return Err(TriviumError::WalClosed);
        }
        if state.lifecycle != DatabaseLifecycle::Open {
            return Err(TriviumError::DatabaseClosed);
        }
        let resource_task = if maintenance {
            self.resource_runtime.enter_maintenance_task()?
        } else {
            self.resource_runtime.enter_task()?
        };
        state.active_operations = state
            .active_operations
            .checked_add(1)
            .ok_or_else(|| TriviumError::InvalidInput("数据库活动操作计数溢出".into()))?;
        drop(state);
        Ok(OperationGuard {
            resource_task,
            lifecycle: Arc::clone(&self.lifecycle),
        })
    }

    // ════════════════════════════════════════════════════════
    //  打开 / 创建
    // ════════════════════════════════════════════════════════

    /// 打开或创建数据库（默认：Mmap 模式，SyncMode::Normal）
    pub fn open(path: &str, dim: usize) -> Result<Self> {
        let config = Config {
            dim,
            ..Default::default()
        };
        Self::open_with_config(path, config)
    }

    /// 打开或创建数据库（高级配置入口）
    pub fn open_with_config(path: &str, config: Config) -> Result<Self> {
        Self::open_with_runtime(
            path,
            config,
            crate::resources::ResourceRuntime::shared_default(),
        )
    }

    /// 使用可由多个实例共享的资源运行时；内存配额在创建数据库制品前准入。
    pub fn open_with_runtime(
        path: &str,
        config: Config,
        runtime: crate::resources::ResourceRuntime,
    ) -> Result<Self> {
        let _task = runtime.enter_task()?;
        let reservation = runtime.reserve_database(config.memory_limit)?;
        Self::open_reserved(path, config, runtime.clone(), reservation)
    }

    fn open_reserved(
        path: &str,
        config: Config,
        runtime: crate::resources::ResourceRuntime,
        reservation: crate::resources::DatabaseReservation,
    ) -> Result<Self> {
        Self::open_reserved_with_lock(path, config, runtime, reservation, None)
    }

    fn open_reserved_with_lock(
        path: &str,
        config: Config,
        runtime: crate::resources::ResourceRuntime,
        reservation: crate::resources::DatabaseReservation,
        reserved_lock: Option<std::fs::File>,
    ) -> Result<Self> {
        let dim = config.dim;

        // ═══ 维度安全校验 ═══
        if dim == 0 {
            return Err(TriviumError::InvalidInput(
                "Vector dimension must be at least 1".into(),
            ));
        }
        // 上限 65536：单向量 65536×f32 = 256KB，已远超实际使用范围。
        // 超过此值会导致内存分配溢出或 32 位平台上的指针寻址越界。
        const MAX_DIM: usize = 65536;
        if dim > MAX_DIM {
            return Err(TriviumError::InvalidInput(format!(
                "Vector dimension {} exceeds maximum allowed {} (would require {}MB per vector pool page)",
                dim,
                MAX_DIM,
                dim * 4 / 1024 / 1024
            )));
        }

        // ═══ 自动递归创建上层目录 ═══
        if config.access_mode == AccessMode::ReadWrite
            && let Some(parent_dir) = std::path::Path::new(path).parent()
            && !parent_dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent_dir)?;
        }

        let normalized_path = normalized_database_path(path)?;
        let path = normalized_path.to_str().ok_or_else(|| {
            TriviumError::InvalidInput(
                "数据库规范路径不是有效 UTF-8 (Canonical database path is not valid UTF-8)".into(),
            )
        })?;

        // ═══ 文件锁：防止多进程并发写同一个数据库 ═══
        let lock_path = format!("{}.lock", path);
        if config.access_mode != AccessMode::ReadWrite && !std::path::Path::new(path).exists() {
            return Err(TriviumError::InvalidInput(format!(
                "只读模式要求数据库文件已存在: {path}"
            )));
        }
        let reserved_lock_held = reserved_lock.is_some();
        let lock_file = if let Some(lock) = reserved_lock {
            Some(lock)
        } else {
            match config.access_mode {
                AccessMode::ReadWrite => Some(
                    std::fs::OpenOptions::new()
                        .create(true)
                        .read(true)
                        .write(true)
                        .open(&lock_path)?,
                ),
                AccessMode::ReadOnly => {
                    Some(std::fs::OpenOptions::new().read(true).open(&lock_path)?)
                }
                AccessMode::Immutable => None,
            }
        };
        let locked = reserved_lock_held
            || match (config.access_mode, lock_file.as_ref()) {
                (AccessMode::ReadWrite, Some(file)) => file.try_lock_exclusive().is_ok(),
                (AccessMode::ReadOnly, Some(file)) => file.try_lock_shared().is_ok(),
                (AccessMode::Immutable, None) => true,
                _ => false,
            };
        if !locked {
            return Err(TriviumError::DatabaseLocked(format!(
                "Database '{}' is already opened with an incompatible access mode",
                path
            )));
        }

        file_format::recover_publication::<T>(path, config.access_mode == AccessMode::ReadWrite)?;

        if config.access_mode == AccessMode::Immutable {
            crate::storage::snapshot::validate_manifest(path, std::any::type_name::<T>(), dim)?;
        }
        if !std::path::Path::new(path).exists() && Wal::needs_recovery(path) {
            return Err(TriviumError::CorruptedFile("缺少带类型的基础检查点，请用原版本恢复并导出 (Typed checkpoint missing; recover and export with the original version)".into()));
        }
        let mut memtable = if std::path::Path::new(path).exists() {
            file_format::load_with_memory_limit(
                path,
                config.storage_mode,
                config.load_text_index,
                config.access_mode == AccessMode::ReadWrite,
                config.missing_index_policy,
                config.access_mode != AccessMode::ReadWrite,
                config.memory_limit,
            )?
        } else {
            MemTable::new(dim)
        };

        if memtable.dim() != dim {
            return Err(TriviumError::DimensionMismatch {
                expected: memtable.dim(),
                got: dim,
            });
        }
        memtable.set_text_memory_limit(config.memory_limit);
        // 恢复必须从第一条记录开始使用调用方预算，避免先填充默认解析缓存再淘汰。
        memtable.begin_loading(config.memory_limit);
        memtable.attach_resources(runtime.clone(), reservation);

        if Wal::needs_recovery(path) && config.access_mode != AccessMode::ReadWrite {
            return Err(TriviumError::RecoveryRequired {
                wal_path: format!("{}.wal", path),
            });
        }
        let mut recovered_offset = None;
        if Wal::needs_recovery(path) {
            let mut entry_count = 0usize;
            let restored_bytes = std::cell::Cell::new(memtable.estimated_memory_bytes());
            let valid_offset =
                Wal::replay_for_open::<T>(path, config.memory_limit, &restored_bytes, |entry| {
                    transaction::replay_entry(&mut memtable, entry)?;
                    memtable.check_load_allocation(0)?;
                    restored_bytes.set(memtable.estimated_memory_bytes());
                    entry_count = entry_count.saturating_add(1);
                    Ok(())
                })?;

            // 回放按完整提交后的事实重建唯一所有者，验证通过后再修改 WAL。
            let unique_definitions = memtable.unique_index_definitions();
            memtable.restore_unique_index_definitions(&unique_definitions)?;

            memtable.check_load_allocation(0)?;

            recovered_offset = Some((valid_offset, entry_count));
        }

        // TextIndex 由 sidecar 精确恢复；缺失时保持为空，避免打开数据库时
        // 无条件扫描全部 Payload 并改变用户显式 index_text() 的索引语义。
        memtable.set_auto_build_quiver(
            config.auto_build_quiver
                && (config.access_mode == AccessMode::ReadWrite
                    || config.missing_index_policy == MissingIndexPolicy::BuildInMemory),
        );

        if let Some(expected_nodes) = config.expected_nodes {
            let additional = expected_nodes.saturating_sub(memtable.node_count());
            let estimated_bytes = memtable.estimate_reserve_bytes(additional)?;
            let current_bytes = memtable.estimated_memory_bytes();
            if config.memory_limit > 0
                && current_bytes.saturating_add(estimated_bytes) > config.memory_limit
            {
                return Err(TriviumError::CapacityReservationRejected {
                    requested_nodes: additional,
                    estimated_bytes,
                    current_bytes,
                    memory_limit: config.memory_limit,
                });
            }
            memtable.try_reserve_for_insert(additional)?;
        }

        memtable.check_load_allocation(0)?;
        memtable.finish_loading(config.payload_cache_bytes, config.payload_cache_entry_bytes)?;
        // 所有加载、恢复与容量准入完成后才修改 WAL 尾部或迁移空检查点。
        if config.access_mode == AccessMode::ReadWrite {
            if std::path::Path::new(path).exists() {
                file_format::upgrade_empty_checkpoint(&mut memtable, path, config.storage_mode)?;
            } else {
                file_format::initialize::<T>(path, dim)?;
            }
        }
        if config.access_mode == AccessMode::ReadWrite {
            Wal::admit_legacy_migration(
                path,
                config.memory_limit,
                memtable.estimated_memory_bytes(),
            )?;
            Wal::upgrade_legacy_wal::<T>(path)?;
        }
        if let Some((offset, entry_count)) = recovered_offset {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(format!("{path}.wal"))?;
            file.set_len(offset)?;
            file.sync_all()?;
            tracing::info!(
                entry_count,
                offset,
                "WAL 恢复与尾部截断完成 (WAL recovery and tail truncation completed)"
            );
        }

        let wal = match config.access_mode {
            AccessMode::ReadWrite => Wal::open_with_sync(path, config.sync_mode)?,
            AccessMode::ReadOnly | AccessMode::Immutable => Wal::disabled(path, config.sync_mode),
        };
        let recovery_required = memtable.recovery_latch();
        Ok(Self {
            db_path: normalized_path.to_string_lossy().into_owned(),
            memtable: Arc::new(RwLock::new(memtable)),
            wal: Arc::new(Mutex::new(wal)),
            compaction: None,
            _lock_file: Arc::new(Mutex::new(lock_file)),
            memory_limit: config.memory_limit,
            resource_runtime: runtime,
            tql_limits: crate::query::tql_executor::TqlLimits {
                max_query_rows: config.max_query_rows,
                row_overflow: config.row_overflow,
                memory_limit: config.memory_limit,
                query_control: None,
            },
            storage_mode: config.storage_mode,
            hook: Arc::new(RwLock::new(Arc::new(NoopHook))),
            stateful_search: Arc::new(Mutex::new(())),
            quiver_builds: Arc::new((
                Mutex::new(std::collections::HashSet::new()),
                std::sync::Condvar::new(),
            )),
            lifecycle: Arc::new((
                Mutex::new(LifecycleState {
                    lifecycle: DatabaseLifecycle::Open,
                    recovery_required,
                    active_operations: 0,
                    group_commit_owner: None,
                    shutdown_phase: ShutdownPhase::Open,
                    shutdown_error: None,
                    shutdown_last_error: None,
                    shutdown_last_failure: None,
                    shutdown_job: None,
                }),
                Condvar::new(),
            )),
            access_mode: config.access_mode,
        })
    }

    pub fn open_read_only(path: &str, dim: usize) -> Result<Self> {
        Self::open_with_config(
            path,
            Config {
                dim,
                access_mode: AccessMode::ReadOnly,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
    }

    pub fn open_immutable(path: &str, dim: usize) -> Result<Self> {
        Self::open_with_config(
            path,
            Config {
                dim,
                access_mode: AccessMode::Immutable,
                auto_build_quiver: false,
                ..Default::default()
            },
        )
    }

    pub(crate) fn require_write_access(&self, operation: &'static str) -> Result<()> {
        if self.access_mode != AccessMode::ReadWrite {
            return Err(TriviumError::ReadOnlyViolation { operation });
        }
        Ok(())
    }

    fn require_reader_determinism(&self, config: &SearchConfig) -> Result<()> {
        if self.access_mode != AccessMode::ReadWrite && config.enable_refractory_fatigue {
            return Err(TriviumError::InvalidInput(
                "ReadOnly/Immutable Reader 禁止启用进程本地 refractory fatigue".into(),
            ));
        }
        Ok(())
    }

    // ════════════════════════════════════════════════════════
    //  配置管理
    // ════════════════════════════════════════════════════════

    /// 运行时切换 WAL 同步模式
    pub fn set_sync_mode(&mut self, mode: SyncMode) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("set_sync_mode")?;
        let mut w = lock_or_recover(&self.wal);
        w.set_sync_mode(mode);
        Ok(())
    }

    /// 数据读取与查询共享准入和生命周期检查。
    fn read_operation<R>(&self, operation: impl FnOnce() -> R) -> Result<R> {
        let _operation = self.enter_operation()?;
        let result = operation();
        _operation.check_recovery()?;
        Ok(result)
    }

    /// 返回当前进程内累计的 WAL 写入与物理同步计数。
    pub fn wal_stats(&self) -> WalStats {
        lock_or_recover(&self.wal).stats()
    }

    /// 在一个 WAL 同步组内执行多个独立事务，并在返回前统一同步。
    ///
    /// 闭包内仍通过普通事务 API 提交；每个事务保留独立 Begin/Commit 边界。
    /// 返回值中的布尔量表示本组是否执行了一次物理 fsync。
    pub fn group_commit<R>(&mut self, operation: impl FnOnce(&mut Self) -> R) -> Result<(R, bool)> {
        let _operation = self.enter_operation()?;
        self.require_write_access("group_commit")?;
        lock_or_recover(&self.wal).begin_group_commit()?;
        lock_or_recover(&self.lifecycle.0).group_commit_owner = Some(std::thread::current().id());
        let mut group = GroupCommitGuard {
            wal: self.wal.clone(),
            lifecycle: self.lifecycle.clone(),
            active: true,
        };
        let result = operation(self);
        let synced = group.finish()?;
        Ok((result, synced))
    }

    // ════════════════════════════════════════════════════════
    //  Hook 管理
    // ════════════════════════════════════════════════════════

    /// 注册自定义检索管线 Hook
    ///
    /// Hook 允许开发者在检索管线的 6 个关键阶段插入自定义逻辑：
    /// 1. `on_pre_search` — 查询预处理
    /// 2. `on_custom_recall` — 自定义召回（可替代内置）
    /// 3. `on_post_recall` — 召回后处理
    /// 4. `on_pre_graph_expand` — 图扩散前拦截
    /// 5. `on_rerank` — 自定义重排序
    /// 6. `on_post_search` — 最终后处理
    ///
    /// # 示例
    /// ```rust,ignore
    /// struct MyHook;
    /// impl SearchHook for MyHook {
    ///     fn on_post_recall(&self, hits: &mut Vec<SearchHit>, _ctx: &mut HookContext) {
    ///         hits.retain(|h| h.score > 0.5);
    ///     }
    /// }
    /// db.set_hook(MyHook)?;
    /// ```
    pub fn set_hook(&mut self, hook: impl SearchHook + 'static) -> Result<()> {
        self.set_hook_with(|| Ok(hook), std::convert::identity)
    }

    /// 准入成功后才构造语言绑定的 Hook，失败时保留旧 Hook 及其宿主引用。
    pub(crate) fn set_hook_with<H: SearchHook + 'static, E>(
        &mut self,
        create: impl FnOnce() -> std::result::Result<H, E>,
        map_error: impl FnOnce(TriviumError) -> E,
    ) -> std::result::Result<(), E> {
        let _operation = self.enter_operation().map_err(map_error)?;
        let hook = create()?;
        let previous = {
            let mut slot = write_or_recover(&self.hook);
            std::mem::replace(&mut *slot, Arc::new(hook))
        };
        drop(previous);
        Ok(())
    }

    /// 移除当前 Hook，恢复为默认的 NoopHook
    pub fn clear_hook(&mut self) {
        clear_hook_slot(&self.hook);
    }

    /// 获取当前 Hook 的引用（主要用于测试和调试）
    pub fn hook(&self) -> Arc<dyn SearchHook> {
        read_or_recover(&self.hook).clone()
    }

    #[cfg(feature = "python")]
    pub(crate) fn try_with_hook<R>(
        &self,
        visit: impl FnOnce(&Arc<dyn SearchHook>) -> R,
    ) -> Option<R> {
        self.hook.try_read().ok().map(|hook| visit(&hook))
    }

    // ════════════════════════════════════════════════════════
    //  QuIVer 索引管理
    // ════════════════════════════════════════════════════════

    /// 克隆一个可在线程池中完成自动 QuIVer 预热的轻量句柄。
    pub fn quiver_build_handle(&self) -> QuiverBuildHandle<T> {
        QuiverBuildHandle {
            memtable: Arc::clone(&self.memtable),
            memory_limit: self.memory_limit,
            resource_runtime: self.resource_runtime.clone(),
            quiver_builds: Arc::clone(&self.quiver_builds),
            lifecycle: Arc::clone(&self.lifecycle),
            access_mode: self.access_mode,
        }
    }

    /// 若当前配置和数据状态满足自动构建条件，捕获后台 QuIVer 构建任务。
    pub fn prepare_auto_quiver_build(&self) -> Result<Option<PreparedQuiverBuild<T>>> {
        let _operation = self.enter_operation()?;
        self.quiver_build_handle().prepare_auto_build()
    }

    /// 构建 QuIVer BQ-native Vamana 图索引
    ///
    /// 从当前所有活跃数据构建 ANN 图索引，替代默认的三级火箭管线。
    /// 构建后的搜索将自动使用 QuIVer 的 O(log N) 图搜索。
    ///
    /// **冷热分离**：
    /// - Hot: 2-bit BQ 签名 + Vamana 图拓扑 (~2 bits/dim/node)
    /// - Cold: f32 原始向量（仅精排阶段按需访问）
    ///
    /// **事务安全**: delete / update_vector 会使索引自动失效，管线回退到三级火箭。
    ///
    /// ```rust,ignore
    /// use triviumdb::index::quiver::QuIVerConfig;
    /// db.build_quiver_index(None); // 使用默认配置 (m=16, ef_c=128, α=1.2)
    /// ```
    pub fn build_quiver_index(
        &self,
        config: Option<crate::index::quiver::QuIVerConfig>,
    ) -> Result<()> {
        let _operation = self.enter_operation()?;

        self.require_write_access("build_quiver_index")?;
        if crate::vector::is_binary_vector::<T>() {
            return Err(TriviumError::InvalidInput("u64 使用汉明精确检索，QuIVer 构建支持浮点向量 (u64 uses exact Hamming retrieval; QuIVer construction supports floating-point vectors)".into()));
        }
        let cfg = config.unwrap_or_default();
        let (_workspace, snapshot) = {
            let mt = read_or_recover(&self.memtable);
            if mt.dim() > crate::index::bq::MAX_BQ_DIM {
                return Err(TriviumError::InvalidInput(format!(
                    "QuIVer 最高支持 {} 维，当前数据库为 {} 维",
                    crate::index::bq::MAX_BQ_DIM,
                    mt.dim()
                )));
            }
            let projected = mt
                .estimated_memory_bytes()
                .saturating_add(mt.quiver_build_peak_bytes(&cfg));
            if self.memory_limit > 0 && projected > self.memory_limit {
                return Err(TriviumError::InvalidInput(format!(
                    "QuIVer 构建预计峰值 {}MB 超过内存上限 {}MB",
                    projected / (1024 * 1024),
                    self.memory_limit / (1024 * 1024)
                )));
            }
            let workspace = self
                .resource_runtime
                .reserve_workspace(mt.quiver_build_peak_bytes(&cfg))?;
            (workspace, mt.quiver_build_snapshot())
        };
        if let Some(snapshot) = snapshot {
            let source_generation = snapshot.generation;
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeQuiverBuildClaim);
            let Some(_build_guard) = try_start_quiver_build(&self.quiver_builds, source_generation)
            else {
                #[cfg(feature = "test-hooks")]
                crate::test_hooks::hit(
                    crate::test_hooks::ConcurrencyPoint::QuiverBuildFollowerWaiting,
                );
                let (building, changed) = &*self.quiver_builds;
                let mut active = lock_or_recover(building);
                while active.contains(&source_generation) {
                    active = changed
                        .wait(active)
                        .unwrap_or_else(|poison| poison.into_inner());
                }
                return Ok(());
            };
            let index = self
                .resource_runtime
                .pool(self.resource_runtime.parallelism())?
                .install(|| MemTable::<T>::build_quiver_snapshot(snapshot, &cfg));
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeQuiverPublish);
            if !write_or_recover(&self.memtable).publish_quiver_if_current(source_generation, index)
            {
                return Err(TriviumError::InvalidInput(
                    "QuIVer 构建期间向量数据已变化，请重试".into(),
                ));
            }
        }
        Ok(())
    }

    /// 在当前快速QuIVer图上安装按节点ID绑定的校正系数，不重新构图。
    /// 校正为运行期派生状态；增删、重建和重载后需重新安装，不改变旧BQ存储格式。
    pub fn enable_quiver_corrected_sm(&self, coefficients: &[(NodeId, f32)]) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("enable_quiver_corrected_sm")?;
        reject_hook_reentrant_write()?;
        let mut guard = write_or_recover(&self.memtable);
        let mt = &mut *guard;
        self.resource_runtime
            .pool(self.resource_runtime.parallelism())?
            .install(|| {
                let extra = mt
                    .quiver()
                    .map_or(0, |index| index.sm_install_peak_extra_bytes());
                if self.memory_limit > 0
                    && mt.estimated_memory_bytes().saturating_add(extra) > self.memory_limit
                {
                    return Err(TriviumError::InvalidInput(
                        "QuIVer校正安装预计峰值超过内核内存预算".into(),
                    ));
                }
                let _workspace = self.resource_runtime.reserve_workspace(extra)?;
                mt.enable_quiver_corrected_sm(coefficients)
                    .map_err(|error| {
                        TriviumError::InvalidInput(format!("安装QuIVer校正失败：{error}"))
                    })
            })
    }

    /// 返回校正是否仍与当前QuIVer图匹配。
    pub fn quiver_corrected_sm_enabled(&self) -> bool {
        read_or_recover(&self.memtable).quiver_corrected_sm_enabled()
    }

    /// 恢复默认无训练SM-L2；新构图、增量写入及数据库重载均自动维护默认状态。
    pub fn enable_quiver_sm_l2(&self) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("enable_quiver_sm_l2")?;
        reject_hook_reentrant_write()?;
        let mut guard = write_or_recover(&self.memtable);
        let mt = &mut *guard;
        self.resource_runtime
            .pool(self.resource_runtime.parallelism())?
            .install(|| {
                let extra = mt.quiver().map_or(0, |index| {
                    if index.default_sm_ready() {
                        0
                    } else {
                        index.sm_install_peak_extra_bytes()
                    }
                });
                if self.memory_limit > 0
                    && mt.estimated_memory_bytes().saturating_add(extra) > self.memory_limit
                {
                    return Err(TriviumError::InvalidInput(
                        "QuIVer重建L2安装预计峰值超过内核内存预算".into(),
                    ));
                }
                let _workspace = self.resource_runtime.reserve_workspace(extra)?;
                mt.enable_quiver_sm_l2().map_err(|error| {
                    TriviumError::InvalidInput(format!("安装QuIVer重建L2失败：{error}"))
                })
            })
    }

    pub fn quiver_sm_l2_enabled(&self) -> bool {
        read_or_recover(&self.memtable).quiver_sm_l2_enabled()
    }

    /// 显式选择原生欧氏查询，返回负平方距离；运行期模式在修改或重载后需重新安装。
    pub fn enable_quiver_sm_native_l2(&self) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("enable_quiver_sm_native_l2")?;
        reject_hook_reentrant_write()?;
        let mut guard = write_or_recover(&self.memtable);
        let mt = &mut *guard;
        self.resource_runtime
            .pool(self.resource_runtime.parallelism())?
            .install(|| {
                let extra = mt
                    .quiver()
                    .map_or(0, |index| index.sm_install_peak_extra_bytes());
                if self.memory_limit > 0
                    && mt.estimated_memory_bytes().saturating_add(extra) > self.memory_limit
                {
                    return Err(TriviumError::InvalidInput(
                        "QuIVer原生欧氏安装预计峰值超过内核内存预算".into(),
                    ));
                }
                let _workspace = self.resource_runtime.reserve_workspace(extra)?;
                mt.enable_quiver_sm_native_l2().map_err(|error| {
                    TriviumError::InvalidInput(format!("安装QuIVer原生欧氏失败：{error}"))
                })
            })
    }

    pub fn quiver_sm_native_l2_enabled(&self) -> bool {
        read_or_recover(&self.memtable).quiver_sm_native_l2_enabled()
    }

    // ════════════════════════════════════════════════════════
    //  内存管理
    // ════════════════════════════════════════════════════════

    /// 设置内存上限（字节）
    ///
    /// 达到上限时拒绝新增内存负载；不会用频繁全量 flush 换取表面 RSS 降低。
    /// 设为 0 表示无限制（默认）。
    pub fn set_memory_limit(&mut self, bytes: usize) -> Result<()> {
        let _operation = self.enter_operation()?;
        let mut mt = write_or_recover(&self.memtable);
        mt.resize_resource_reservation(bytes)?;
        self.memory_limit = bytes;
        self.tql_limits.memory_limit = bytes;
        mt.set_text_memory_limit(bytes);
        Ok(())
    }

    /// 返回共享运行时的配额、线程和任务统计。
    pub fn resource_stats(&self) -> crate::resources::ResourceStats {
        self.resource_runtime.stats()
    }

    /// 为后续插入主动预留额外节点容量。
    ///
    /// 预留受内核内存预算约束；失败不会修改 WAL、节点、ID 或 generation。
    pub fn reserve_nodes(&self, additional: usize) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("reserve_nodes")?;
        reject_hook_reentrant_write()?;
        if additional == 0 {
            return Err(TriviumError::InvalidInput(
                "reserve_nodes 的 additional 必须大于 0".into(),
            ));
        }
        let mut mt = write_or_recover(&self.memtable);
        let estimated_bytes = mt.estimate_reserve_bytes(additional)?;
        let current_bytes = mt.estimated_memory_bytes();
        if self.memory_limit > 0
            && current_bytes.saturating_add(estimated_bytes) > self.memory_limit
        {
            return Err(TriviumError::CapacityReservationRejected {
                requested_nodes: additional,
                estimated_bytes,
                current_bytes,
                memory_limit: self.memory_limit,
            });
        }
        mt.try_reserve_for_insert(additional)
    }

    /// 查询当前 MemTable 估算内存占用（字节）
    pub fn estimated_memory(&self) -> usize {
        read_or_recover(&self.memtable).estimated_memory_bytes()
    }

    #[cfg(feature = "nodejs")]
    pub(crate) fn next_id_for_binding(&self) -> Result<NodeId> {
        self.read_operation(|| read_or_recover(&self.memtable).next_id_value())
    }

    fn ensure_incremental_memory_budget(&self, additional_bytes: usize) -> Result<()> {
        if self.memory_limit == 0 {
            return Ok(());
        }
        let current_bytes = read_or_recover(&self.memtable).estimated_memory_bytes();
        if current_bytes.saturating_add(additional_bytes) > self.memory_limit {
            return Err(TriviumError::CapacityReservationRejected {
                requested_nodes: 1,
                estimated_bytes: additional_bytes,
                current_bytes,
                memory_limit: self.memory_limit,
            });
        }
        Ok(())
    }

    fn admit_payload(&self, payload: &serde_json::Value) -> Result<()> {
        let encoded = crate::storage::payload_store::json_encoded_len(payload)?;
        if encoded > 8 * 1024 * 1024 {
            return Err(TriviumError::PayloadTooLarge {
                size_bytes: encoded,
                max_bytes: 8 * 1024 * 1024,
            });
        }
        self.ensure_incremental_memory_budget(
            encoded
                .saturating_mul(2)
                .saturating_add(crate::storage::payload_store::estimate_json_memory(payload))
                .saturating_add(256),
        )
    }

    /// 内部方法：达到预算后只释放冷 mmap 页，不触发无法降低索引堆占用的全量重写。
    fn check_memory_pressure(&mut self) {
        if self.memory_limit > 0 {
            let usage = read_or_recover(&self.memtable).estimated_memory_bytes();
            if usage > self.memory_limit {
                tracing::warn!(
                    "内存压力超过预算，已释放冷 mmap 页且后续新增负载将被拒绝；不会自动全量重写 (Memory budget exceeded; cold mmap pages were released and subsequent growth will be rejected; automatic full rewrite is disabled): {}MB > {}MB",
                    usage / (1024 * 1024),
                    self.memory_limit / (1024 * 1024)
                );
                read_or_recover(&self.memtable).advise_cold_vectors_dontneed();
            }
        }
    }

    // ════════════════════════════════════════════════════════
    //  Compaction 管理
    // ════════════════════════════════════════════════════════

    /// 清除数据库级 fatigue 状态。
    ///
    /// 与有状态查询共用同一串行锁，保证清理操作在线性顺序上发生于两个完整查询之间，
    /// 不会与查询中的 fatigue consume/mark 阶段交错。
    pub fn clear_search_state(&self) -> Result<()> {
        self.read_operation(|| {
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeClearSearchStateLock);
            let _stateful_guard = lock_or_recover(&self.stateful_search);
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::ClearSearchStateEntered);
            read_or_recover(&self.memtable).clear_fatigue();
        })
    }

    pub fn set_auto_build_quiver(&mut self, enabled: bool) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("set_auto_build_quiver")?;
        write_or_recover(&self.memtable).set_auto_build_quiver(enabled);
        Ok(())
    }

    /// 启动后台自动 Compaction 线程
    pub fn enable_auto_compaction(&mut self, interval: Duration) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("enable_auto_compaction")?;
        if interval.is_zero() {
            return Err(TriviumError::InvalidInput(
                "自动压缩间隔必须大于 0 秒".into(),
            ));
        }
        self.compaction.take();
        let ct = CompactionThread::spawn(
            interval,
            Arc::clone(&self.memtable),
            Arc::clone(&self.wal),
            self.db_path.clone(),
            self.storage_mode,
        )?;
        self.compaction = Some(ct);
        Ok(())
    }

    pub fn disable_auto_compaction(&mut self) {
        self.compaction.take();
    }

    /// 主动触发全量重写与压实（Manual Compaction）
    pub fn compact(&mut self) -> Result<()> {
        let _operation = self.enter_maintenance_operation()?;
        self.require_write_access("compact")?;
        reject_hook_reentrant_write()?;
        {
            let mut mt = write_or_recover(&self.memtable);
            tracing::info!("手动压实开始: {}", self.db_path);
            let mut w = lock_or_recover(&self.wal);
            w.ensure_writable()?;
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeCompactionSave);
            file_format::save(&mut mt, &self.db_path, self.storage_mode)?;
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeWalClear);
            w.clear()?;
            mt.mark_checkpoint_clean();
            mt.advise_cold_vectors_dontneed();
        }

        tracing::info!(
            "手动压实完成 (Manual compaction completed): {}",
            self.db_path
        );
        Ok(())
    }

    // ════════════════════════════════════════════════════════
    //  写操作
    // ════════════════════════════════════════════════════════

    pub fn insert(&mut self, vector: &[T], payload: serde_json::Value) -> Result<NodeId> {
        self.require_write_access("insert")?;
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        self.admit_payload(&payload)?;
        let payload_str = payload.to_string();
        if payload_str.len() > 8 * 1024 * 1024 {
            return Err(crate::error::TriviumError::PayloadTooLarge {
                size_bytes: payload_str.len(),
                max_bytes: 8 * 1024 * 1024,
            });
        }
        self.ensure_incremental_memory_budget(
            vector
                .len()
                .saturating_mul(std::mem::size_of::<T>())
                .saturating_add(payload_str.len())
                .saturating_add(256),
        )?;

        let id = {
            let mut mt = write_or_recover(&self.memtable);
            mt.validate_insert(vector)?;
            let id = mt.next_id_value();
            mt.validate_insert_with_id(id, vector)?;
            mt.ensure_additional_memory(
                self.memory_limit,
                mt.estimate_reserve_bytes(1)?
                    .saturating_add(payload_str.len().saturating_mul(2))
                    .saturating_add(crate::storage::payload_store::estimate_json_memory(
                        &payload,
                    ))
                    .saturating_add(vector.len().saturating_mul(std::mem::size_of::<T>()))
                    .saturating_add(256),
            )?;
            mt.validate_unique_changes([(id, Some(&payload))])?;
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::Insert {
                id,
                vector: vector.to_vec(),
                payload: payload_str,
            })?;
            mt.insert_with_id(id, vector, payload)?;
            id
        };
        self.check_memory_pressure();
        Ok(id)
    }

    pub fn insert_with_id(
        &mut self,
        id: NodeId,
        vector: &[T],
        payload: serde_json::Value,
    ) -> Result<()> {
        self.require_write_access("insert_with_id")?;
        let _operation = self.enter_operation()?;
        self.admit_payload(&payload)?;
        let payload_str = payload.to_string();
        reject_hook_reentrant_write()?;
        if payload_str.len() > 8 * 1024 * 1024 {
            return Err(crate::error::TriviumError::PayloadTooLarge {
                size_bytes: payload_str.len(),
                max_bytes: 8 * 1024 * 1024,
            });
        }
        self.ensure_incremental_memory_budget(
            vector
                .len()
                .saturating_mul(std::mem::size_of::<T>())
                .saturating_add(payload_str.len())
                .saturating_add(256),
        )?;

        {
            let mut mt = write_or_recover(&self.memtable);
            mt.validate_insert_with_id(id, vector)?;
            mt.ensure_additional_memory(
                self.memory_limit,
                mt.estimate_reserve_bytes(1)?
                    .saturating_add(payload_str.len().saturating_mul(2))
                    .saturating_add(crate::storage::payload_store::estimate_json_memory(
                        &payload,
                    ))
                    .saturating_add(vector.len().saturating_mul(std::mem::size_of::<T>()))
                    .saturating_add(256),
            )?;
            mt.validate_unique_changes([(id, Some(&payload))])?;
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::Insert {
                id,
                vector: vector.to_vec(),
                payload: payload_str,
            })?;
            mt.insert_with_id(id, vector, payload)?;
        }
        self.check_memory_pressure();
        Ok(())
    }

    /// 使用自定义 ID 原子插入或覆盖节点的向量与 Payload。
    pub fn upsert_with_id(
        &mut self,
        id: NodeId,
        vector: &[T],
        payload: serde_json::Value,
    ) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("upsert_with_id")?;
        let mut tx = transaction::TxBuilder::new();
        if self.contains(id)? {
            tx.update_vector(id, vector);
            tx.update_payload(id, payload);
        } else {
            tx.insert_with_id(id, vector, payload);
        }
        self.commit_tx(tx).map(|_| ())
    }

    pub fn link(&mut self, src: NodeId, dst: NodeId, label: &str, weight: f32) -> Result<()> {
        self.require_write_access("link")?;
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        {
            let mut mt = write_or_recover(&self.memtable);
            mt.validate_link(src, dst)?;
            let growth = mt.estimate_edge_write_bytes(src, dst, label, &serde_json::Value::Null)?;
            mt.ensure_additional_memory(self.memory_limit, growth)?;
            if !weight.is_finite() {
                return Err(TriviumError::InvalidInput(
                    "边权重必须是有限浮点数 (Edge weight must be finite)".into(),
                ));
            }
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::Link::<T> {
                src,
                dst,
                label: label.to_string(),
                weight,
                metadata: "null".to_string(),
            })?;
            mt.link(src, dst, label.to_string(), weight)?;
        }
        Ok(())
    }

    pub fn get_edge(
        &self,
        src: NodeId,
        dst: NodeId,
        label: &str,
    ) -> Result<Option<crate::node::Edge>> {
        self.read_operation(|| {
            read_or_recover(&self.memtable)
                .get_edge(src, dst, label)
                .cloned()
        })
    }

    pub fn shortest_path(
        &self,
        source: NodeId,
        target: NodeId,
        max_depth: usize,
        label: Option<&str>,
    ) -> Result<Option<Vec<NodeId>>> {
        self.read_operation(|| {
            crate::graph::pathfinding::shortest_path(
                &read_or_recover(&self.memtable),
                source,
                target,
                max_depth,
                label,
            )
        })
    }

    pub fn shortest_path_bidirectional(
        &self,
        source: NodeId,
        target: NodeId,
        label: Option<&str>,
        budget: &crate::graph::budget::TraversalBudget,
    ) -> Result<crate::graph::pathfinding::ShortestPathOutput> {
        let _operation = self.enter_operation()?;
        crate::graph::pathfinding::shortest_path_bidirectional(
            &read_or_recover(&self.memtable),
            source,
            target,
            label,
            budget,
        )
    }

    pub fn graph_stats(&self) -> Result<crate::storage::memtable::GraphStats> {
        self.read_operation(|| read_or_recover(&self.memtable).graph_stats())
    }

    pub fn validate_graph(&self) -> Result<crate::storage::memtable::GraphIntegrityReport> {
        self.read_operation(|| read_or_recover(&self.memtable).validate_graph())
    }

    pub fn repair_graph_indexes(&mut self) -> Result<crate::storage::memtable::GraphRepairReport> {
        let _operation = self.enter_operation()?;
        self.require_write_access("repair_graph_indexes")?;
        let report = write_or_recover(&self.memtable).repair_graph_indexes();
        self.flush()?;
        Ok(report)
    }

    pub fn upsert_edge(
        &mut self,
        src: NodeId,
        dst: NodeId,
        label: &str,
        weight: f32,
        metadata: serde_json::Value,
    ) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("upsert_edge")?;
        let mut tx = transaction::TxBuilder::new();
        tx.upsert_edge(src, dst, label, weight, metadata);
        self.commit_tx(tx).map(|_| ())
    }

    pub fn update_edge(
        &mut self,
        src: NodeId,
        dst: NodeId,
        label: &str,
        weight: Option<f32>,
        metadata: Option<serde_json::Value>,
    ) -> Result<()> {
        let _operation = self.enter_operation()?;
        let edge = self
            .get_edge(src, dst, label)?
            .ok_or_else(|| TriviumError::InvalidInput("指定边不存在".into()))?;
        self.upsert_edge(
            src,
            dst,
            label,
            weight.unwrap_or(edge.weight),
            metadata.unwrap_or(edge.metadata),
        )
    }

    /// 当字段当前值等于 `expected` 时原子替换为 `replacement`。
    pub fn compare_and_set_payload_field(
        &mut self,
        id: NodeId,
        field: &str,
        expected: &serde_json::Value,
        replacement: serde_json::Value,
    ) -> Result<()> {
        self.require_write_access("compare_and_set_payload_field")?;
        if field.is_empty() {
            return Err(TriviumError::InvalidInput(
                "条件更新字段不能为空 (Conditional update field cannot be empty)".into(),
            ));
        }
        self.admit_payload(&replacement)?;
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        let mut mt = write_or_recover(&self.memtable);
        mt.validate_update_payload(id)?;
        let mut payload = mt
            .get_payload(id)
            .map(|payload| (*payload).clone())
            .ok_or(TriviumError::NodeNotFound(id))?;
        if payload.get(field) != Some(expected) {
            return Err(TriviumError::ConditionalUpdateNotMatched { id });
        }
        let object = payload.as_object_mut().ok_or_else(|| {
            TriviumError::InvalidInput(
                "条件更新要求 Payload 为 JSON 对象 (Conditional update requires an object payload)"
                    .into(),
            )
        })?;
        object.insert(field.to_owned(), replacement);
        let encoded = crate::storage::payload_store::json_encoded_len(&payload)?;
        let additional = encoded.saturating_mul(2).saturating_add(
            crate::storage::payload_store::estimate_json_memory(&payload),
        );
        let current_bytes = mt.estimated_memory_bytes();
        if self.memory_limit > 0 && current_bytes.saturating_add(additional) > self.memory_limit {
            return Err(TriviumError::CapacityReservationRejected {
                requested_nodes: 1,
                estimated_bytes: additional,
                current_bytes,
                memory_limit: self.memory_limit,
            });
        }
        let payload_str = payload.to_string();
        if payload_str.len() > 8 * 1024 * 1024 {
            return Err(TriviumError::PayloadTooLarge {
                size_bytes: payload_str.len(),
                max_bytes: 8 * 1024 * 1024,
            });
        }
        mt.validate_unique_changes([(id, Some(&payload))])?;
        lock_or_recover(&self.wal).append(&WalEntry::UpdatePayload::<T> {
            id,
            payload: payload_str,
        })?;
        mt.update_payload(id, payload)
    }

    /// 原子删除去重后的节点集合；任一节点不存在时不执行任何删除。
    pub fn delete_many_atomic(&mut self, ids: &[NodeId]) -> Result<usize> {
        let _operation = self.enter_operation()?;
        const MAX_DELETE_NODES: usize = 100_000;
        const MAX_DELETE_INCIDENT_EDGES: usize = 1_000_000;
        const MAX_DELETE_WAL_BYTES: usize = 8 * 1024 * 1024;
        let ids = ids
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        if ids.len() > MAX_DELETE_NODES {
            return Err(TriviumError::InvalidInput(format!(
                "批量删除超过节点预算 {MAX_DELETE_NODES} (Delete set exceeds node budget)"
            )));
        }
        let estimated_wal_bytes = ids.len().saturating_mul(32);
        if estimated_wal_bytes > MAX_DELETE_WAL_BYTES {
            return Err(TriviumError::InvalidInput(format!(
                "批量删除超过 WAL 字节预算 {MAX_DELETE_WAL_BYTES} (Delete set exceeds WAL byte budget)"
            )));
        }
        let incident_edges = {
            let mt = read_or_recover(&self.memtable);
            let mut total = 0usize;
            for id in &ids {
                if !mt.contains(*id) {
                    return Err(TriviumError::NodeNotFound(*id));
                }
                total = total.saturating_add(mt.incident_edge_count(*id));
                if total > MAX_DELETE_INCIDENT_EDGES {
                    return Err(TriviumError::InvalidInput(format!(
                        "批量删除超过关联边预算 {MAX_DELETE_INCIDENT_EDGES} (Delete set exceeds incident edge budget)"
                    )));
                }
            }
            total
        };
        self.ensure_incremental_memory_budget(
            estimated_wal_bytes
                .saturating_add(ids.len().saturating_mul(24))
                .saturating_add(incident_edges.saturating_mul(16)),
        )?;
        let count = ids.len();
        let mut tx = transaction::TxBuilder::new();
        for id in ids {
            tx.delete(id);
        }
        self.commit_tx(tx)?;
        Ok(count)
    }

    pub fn delete(&mut self, id: NodeId) -> Result<()> {
        self.require_write_access("delete")?;
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        {
            let mut mt = write_or_recover(&self.memtable);
            mt.validate_delete(id)?;
            mt.reserve_text_deletes(1)?;
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::Delete::<T> { id })?;
            mt.delete(id)?;
        }

        Ok(())
    }

    pub fn unlink(&mut self, src: NodeId, dst: NodeId) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("unlink")?;
        reject_hook_reentrant_write()?;
        {
            let mut mt = write_or_recover(&self.memtable);
            mt.validate_unlink(src)?;
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::Unlink::<T> { src, dst })?;
            mt.unlink(src, dst)?;
        }
        Ok(())
    }

    pub fn unlink_label(&mut self, src: NodeId, dst: NodeId, label: &str) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("unlink_label")?;
        reject_hook_reentrant_write()?;
        let mut mt = write_or_recover(&self.memtable);
        mt.validate_unlink(src)?;
        let mut w = lock_or_recover(&self.wal);
        w.append(&WalEntry::UnlinkLabel::<T> {
            src,
            dst,
            label: label.to_string(),
        })?;
        mt.unlink_label(src, dst, label)
    }

    pub fn update_payload(&mut self, id: NodeId, payload: serde_json::Value) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("update_payload")?;
        reject_hook_reentrant_write()?;
        self.admit_payload(&payload)?;
        let payload_str = payload.to_string();
        if payload_str.len() > 8 * 1024 * 1024 {
            return Err(crate::error::TriviumError::PayloadTooLarge {
                size_bytes: payload_str.len(),
                max_bytes: 8 * 1024 * 1024,
            });
        }

        self.ensure_incremental_memory_budget(payload_str.len().saturating_add(128))?;
        {
            let mut mt = write_or_recover(&self.memtable);
            mt.validate_update_payload(id)?;
            mt.validate_unique_changes([(id, Some(&payload))])?;
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::UpdatePayload::<T> {
                id,
                payload: payload_str,
            })?;
            mt.update_payload(id, payload)?;
        }
        Ok(())
    }

    /// 部分更新节点的 Payload（$set / $inc / $unset）
    ///
    /// 与 `update_payload` 的全量替换不同，只修改指定字段，其他字段保持不变。
    ///
    /// # 示例
    /// ```rust,ignore
    /// // 设置字段
    /// db.patch_payload(id, serde_json::json!({"$set": {"name": "Alice"}}))?;
    /// // 递增计数器
    /// db.patch_payload(id, serde_json::json!({"$inc": {"visits": 1}}))?;
    /// // 删除字段
    /// db.patch_payload(id, serde_json::json!({"$unset": {"old_field": true}}))?;
    /// // 简写模式（等价于 $set）
    /// db.patch_payload(id, serde_json::json!({"name": "Bob"}))?;
    /// ```
    pub fn patch_payload(&mut self, id: NodeId, patch: serde_json::Value) -> Result<()> {
        self.require_write_access("patch_payload")?;
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        self.admit_payload(&patch)?;
        let final_payload = {
            let mt = read_or_recover(&self.memtable);
            mt.preview_patch_payload(id, &patch)?
        };

        self.admit_payload(&final_payload)?;

        let payload_str = final_payload.to_string();
        if payload_str.len() > 8 * 1024 * 1024 {
            return Err(crate::error::TriviumError::PayloadTooLarge {
                size_bytes: payload_str.len(),
                max_bytes: 8 * 1024 * 1024,
            });
        }
        {
            let mut mt = write_or_recover(&self.memtable);
            mt.validate_update_payload(id)?;
            mt.validate_unique_changes([(id, Some(&final_payload))])?;
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::UpdatePayload::<T> {
                id,
                payload: payload_str,
            })?;
            mt.update_payload(id, final_payload)?;
        }
        Ok(())
    }

    pub fn update_vector(&self, id: NodeId, vector: &[T]) -> Result<()> {
        self.require_write_access("update_vector")?;
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        self.ensure_incremental_memory_budget(
            vector
                .len()
                .saturating_mul(std::mem::size_of::<T>())
                .saturating_add(128),
        )?;
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeUpdateVectorWriteLock);
        {
            let mut mt = write_or_recover(&self.memtable);
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit(
                crate::test_hooks::ConcurrencyPoint::UpdateVectorWriteLockAcquired,
            );
            mt.validate_update_vector(id, vector)?;
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::UpdateVector::<T> {
                id,
                vector: vector.to_vec(),
            })?;
            mt.update_vector(id, vector)?;
        }
        Ok(())
    }

    // ════════════════════════════════════════════════════════
    //  社区聚类
    // ════════════════════════════════════════════════════════

    /// 基于内存图谱进行 Leiden/Louvain 近似快速聚类（无锁设计）
    pub fn leiden_cluster(
        &self,
        min_community_size: usize,
        max_iterations: Option<usize>,
        with_centroids: Option<bool>,
    ) -> Result<crate::graph::leiden::LeidenResult> {
        let _operation = self.enter_operation()?;
        let config = crate::graph::leiden::LeidenConfig {
            min_community_size,
            max_iterations: max_iterations.unwrap_or(15),
            compute_centroids: with_centroids.unwrap_or(true),
        };

        // Step 1: 快照邻接表 (短暂持锁)
        let (snapshot, dim) = {
            let mt = read_or_recover(&self.memtable);
            let node_ids = mt.all_node_ids();
            let mut edges = std::collections::HashMap::new();
            for &id in &node_ids {
                if let Some(e) = mt.get_edges(id) {
                    edges.insert(
                        id,
                        e.iter().map(|edge| (edge.target_id, edge.weight)).collect(),
                    );
                }
            }
            (
                crate::graph::leiden::AdjacencySnapshot { edges, node_ids },
                mt.dim(),
            )
        };

        // Step 2: 纯计算聚类 (无锁)
        let mut result = self
            .resource_runtime
            .pool(self.resource_runtime.parallelism())?
            .install(|| crate::graph::leiden::run_leiden(&snapshot, &config));

        // Step 3: 可选质心计算
        if config.compute_centroids && !result.node_to_cluster.is_empty() {
            let vectors = {
                let mt = read_or_recover(&self.memtable);
                let mut vecs = std::collections::HashMap::new();
                for &node_id in result.node_to_cluster.keys() {
                    if let Some(v) = mt.get_vector(node_id) {
                        vecs.insert(node_id, v.iter().map(|x| x.to_f32()).collect::<Vec<f32>>());
                    }
                }
                vecs
            };
            crate::graph::leiden::compute_centroids(&mut result, &vectors, dim);
        }

        Ok(result)
    }

    // ════════════════════════════════════════════════════════
    //  读操作 / 文本索引
    // ════════════════════════════════════════════════════════

    pub fn index_keyword(&mut self, id: NodeId, keyword: &str) -> Result<()> {
        self.require_write_access("index_keyword")?;
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        if keyword.is_empty() {
            return Err(TriviumError::InvalidInput("关键词不能为空".into()));
        }
        let mut mt = write_or_recover(&self.memtable);
        mt.validate_update_payload(id)?;
        mt.prepare_text_index()?;
        mt.index_keyword(id, keyword);
        Ok(())
    }

    pub fn index_text(&mut self, id: NodeId, text: &str) -> Result<()> {
        self.require_write_access("index_text")?;
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        let mut mt = write_or_recover(&self.memtable);
        mt.validate_update_payload(id)?;
        mt.prepare_text_index()?;
        mt.index_text(id, text);
        Ok(())
    }

    pub fn build_text_index(&mut self) -> Result<()> {
        self.require_write_access("build_text_index")?;
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        let mut mt = write_or_recover(&self.memtable);
        mt.prepare_text_index()?;
        mt.try_build_text_index()
    }

    /// 根据提供的完整正文与关键词集合重建文本索引，成功后通过 flush 或 close 持久化。
    pub fn rebuild_text_index(
        &mut self,
        documents: &[crate::index::text::TextIndexDocument],
    ) -> Result<()> {
        self.require_write_access("rebuild_text_index")?;
        let _operation = self.enter_maintenance_operation()?;
        reject_hook_reentrant_write()?;
        write_or_recover(&self.memtable).rebuild_text_index(documents)
    }

    pub fn get_payload(&self, id: NodeId) -> Result<Option<serde_json::Value>> {
        self.read_operation(|| {
            let mt = read_or_recover(&self.memtable);
            mt.try_get_payload(id)
                .map(|payload| payload.map(|payload| (*payload).clone()))
        })?
    }

    pub fn get_edges(&self, id: NodeId) -> Result<Vec<crate::node::Edge>> {
        self.read_operation(|| {
            let mt = read_or_recover(&self.memtable);
            mt.get_edges(id).map(|e| e.to_vec()).unwrap_or_default()
        })
    }

    pub fn get_incoming_edges(
        &self,
        id: NodeId,
        label: Option<&str>,
    ) -> Result<Vec<crate::node::IncomingEdge>> {
        self.read_operation(|| read_or_recover(&self.memtable).get_incoming_edges(id, label))
    }

    pub fn get_all_ids(&self) -> Result<Vec<NodeId>> {
        self.read_operation(|| {
            let mt = read_or_recover(&self.memtable);
            mt.get_all_ids()
        })
    }

    // ════════════════════════════════════════════════════════
    //  检索（委托给 pipeline 子模块）
    // ════════════════════════════════════════════════════════

    pub fn search(
        &self,
        query_vector: &[T],
        top_k: usize,
        expand_depth: usize,
        min_score: f32,
    ) -> Result<Vec<SearchHit>> {
        let _operation = self.enter_operation()?;
        let config = SearchConfig {
            top_k,
            expand_depth,
            min_score,
            enable_advanced_pipeline: false,
            ..Default::default()
        };
        self.search_hybrid(None, Some(query_vector), &config)
    }

    pub fn search_batch(
        &self,
        query_vectors: &[Vec<T>],
        search_config: &SearchConfig,
        batch_config: &BatchSearchConfig,
    ) -> Result<Vec<Vec<SearchHit>>> {
        let _operation = self.enter_operation()?;
        self.require_reader_determinism(search_config)?;
        self.search_handle()
            .search_batch(query_vectors, search_config, batch_config)
    }

    pub(crate) fn search_handle(&self) -> SearchHandle<T> {
        SearchHandle {
            memtable: Arc::clone(&self.memtable),
            memory_limit: self.memory_limit,
            resource_runtime: self.resource_runtime.clone(),
            hook: Arc::clone(&self.hook),
            quiver_builds: Arc::clone(&self.quiver_builds),
            lifecycle: Arc::clone(&self.lifecycle),
        }
    }

    pub fn search_exact(&self, query_vector: &[T], top_k: usize) -> Result<Vec<SearchHit>> {
        let _operation = self.enter_operation()?;
        crate::index::exact::search(&read_or_recover(&self.memtable), query_vector, top_k)
    }

    pub fn search_advanced(
        &self,
        query_vector: &[T],
        config: &SearchConfig,
    ) -> Result<Vec<SearchHit>> {
        let _operation = self.enter_operation()?;
        self.search_hybrid(None, Some(query_vector), config)
    }

    /// 带 Hook 上下文的混合检索（完整版）
    ///
    /// 与 `search_hybrid` 相同，但额外返回 `HookContext`，
    /// 开发者可以从中读取 Hook 各阶段注入的自定义数据和计时统计。
    pub fn search_hybrid_with_context(
        &self,
        query_text: Option<&str>,
        query_vector: Option<&[T]>,
        config: &SearchConfig,
    ) -> Result<(Vec<SearchHit>, HookContext)> {
        self.require_reader_determinism(config)?;
        let _operation = self.enter_operation()?;
        #[cfg(feature = "test-hooks")]
        if config.enable_refractory_fatigue {
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeStatefulSearchLock);
        }
        let _stateful_guard = config
            .enable_refractory_fatigue
            .then(|| lock_or_recover(&self.stateful_search));
        #[cfg(feature = "test-hooks")]
        if _stateful_guard.is_some() {
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::StatefulSearchEntered);
        }
        let mut ctx = HookContext::new();
        let results = pipeline::execute_pipeline_with_limit(
            &self.memtable,
            &self.quiver_builds,
            self.memory_limit,
            &self.hook(),
            query_text,
            query_vector,
            config,
            &mut ctx,
        )?;
        Ok((results.combined_hits, ctx))
    }

    /// 全能混合检索核心引擎 (Hybrid Advanced Pipeline)
    ///
    /// 包含文本稀疏索引 + 稠密连续向量空间 + 图谱数学约束的真正完全体检索引擎。
    /// 具体实现委托给 `pipeline::execute_pipeline`。
    pub fn search_hybrid(
        &self,
        query_text: Option<&str>,
        query_vector: Option<&[T]>,
        config: &SearchConfig,
    ) -> Result<Vec<SearchHit>> {
        self.require_reader_determinism(config)?;
        let _operation = self.enter_operation()?;
        #[cfg(feature = "test-hooks")]
        if config.enable_refractory_fatigue {
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeStatefulSearchLock);
        }
        let _stateful_guard = config
            .enable_refractory_fatigue
            .then(|| lock_or_recover(&self.stateful_search));
        #[cfg(feature = "test-hooks")]
        if _stateful_guard.is_some() {
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::StatefulSearchEntered);
        }
        let mut ctx = HookContext::new();
        pipeline::execute_pipeline_with_limit(
            &self.memtable,
            &self.quiver_builds,
            self.memory_limit,
            &self.hook(),
            query_text,
            query_vector,
            config,
            &mut ctx,
        )
        .map(|output| output.combined_hits)
    }

    pub fn search_hybrid_grouped(
        &self,
        query_text: Option<&str>,
        query_vector: Option<&[T]>,
        config: &SearchConfig,
    ) -> Result<crate::node::GroupedSearchResult> {
        self.require_reader_determinism(config)?;
        let _operation = self.enter_operation()?;
        let _stateful_guard = config
            .enable_refractory_fatigue
            .then(|| lock_or_recover(&self.stateful_search));
        let mut ctx = HookContext::new();
        pipeline::execute_pipeline_with_limit(
            &self.memtable,
            &self.quiver_builds,
            self.memory_limit,
            &self.hook(),
            query_text,
            query_vector,
            config,
            &mut ctx,
        )
        .map(|output| crate::node::GroupedSearchResult {
            semantic_hits: output.semantic_hits,
            graph_hits: output.graph_hits,
        })
    }

    // ════════════════════════════════════════════════════════
    //  节点查询
    // ════════════════════════════════════════════════════════

    pub fn get(&self, id: NodeId) -> Result<Option<crate::node::NodeView<T>>> {
        self.read_operation(|| {
            let mt = read_or_recover(&self.memtable);
            let Some(payload) = mt.try_get_payload(id)? else {
                return Ok(None);
            };
            let vector = mt
                .get_vector(id)
                .ok_or_else(|| {
                    TriviumError::CorruptedFile("节点缺少向量 (Node vector is missing)".into())
                })?
                .to_vec();
            let edges = mt.get_edges(id).unwrap_or(&[]).to_vec();
            Ok(Some(crate::node::NodeView {
                id,
                vector,
                payload: (*payload).clone(),
                edges,
            }))
        })?
    }

    pub fn neighbors_with_labels(
        &self,
        id: NodeId,
        depth: usize,
        labels: Option<&[String]>,
    ) -> Result<Vec<NodeId>> {
        self.read_operation(|| -> Result<Vec<NodeId>> {
            use std::collections::{HashMap, VecDeque};
            let mt = read_or_recover(&self.memtable);
            if !mt.contains(id) || depth == 0 {
                return Ok(Vec::new());
            }
            let bytes = 4usize
                .checked_mul(192)
                .and_then(|bytes| bytes.checked_add(256))
                .ok_or_else(|| TriviumError::CapacityAllocationFailed {
                    reason: "邻域工作区估算溢出 (Neighborhood workspace estimate overflowed)"
                        .into(),
                })?;
            let mut workspace = mt.resource_runtime().reserve_workspace(bytes)?;
            let mut distances = HashMap::new();
            let mut queue = VecDeque::new();
            distances.insert(id, 0usize);
            queue.push_back(id);
            while let Some(current) = queue.pop_front() {
                let current_depth = distances[&current];
                if current_depth >= depth {
                    continue;
                }
                if let Some(edges) = mt.get_edges(current) {
                    for edge in edges {
                        if labels.is_some_and(|allowed| {
                            !allowed.iter().any(|label| label == &edge.label)
                        }) {
                            continue;
                        }
                        let projected = distances.len().saturating_add(1).max(4);
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            distances.entry(edge.target_id)
                        {
                            let bytes = projected
                                .checked_mul(192)
                                .and_then(|bytes| bytes.checked_add(256))
                                .ok_or_else(|| TriviumError::CapacityAllocationFailed {
                                    reason: "邻域工作区估算溢出 (Neighborhood workspace estimate overflowed)"
                                        .into(),
                                })?;
                            workspace.resize(bytes)?;
                            entry.insert(current_depth + 1);
                            queue.push_back(edge.target_id);
                        }
                    }
                }
            }
            distances.remove(&id);
            let mut result: Vec<(usize, NodeId)> = distances
                .into_iter()
                .map(|(node_id, distance)| (distance, node_id))
                .collect();
            result.sort_unstable();
            Ok(result.into_iter().map(|(_, node_id)| node_id).collect())
        })?
    }

    pub fn neighbors(&self, id: NodeId, depth: usize) -> Result<Vec<NodeId>> {
        self.neighbors_with_labels(id, depth, None)
    }

    pub fn reachable(
        &self,
        id: NodeId,
        config: &crate::graph::reachability::ReachabilityConfig,
    ) -> Result<Vec<crate::graph::reachability::ReachabilityResult>> {
        let _operation = self.enter_operation()?;
        crate::graph::reachability::traverse(&read_or_recover(&self.memtable), id, config)
    }

    pub fn reachable_detailed(
        &self,
        id: NodeId,
        config: &crate::graph::reachability::ReachabilityConfig,
    ) -> Result<crate::graph::reachability::ReachabilityOutput> {
        let _operation = self.enter_operation()?;
        crate::graph::reachability::traverse_detailed(&read_or_recover(&self.memtable), id, config)
    }

    pub fn query_subgraph(
        &self,
        id: NodeId,
        config: &crate::graph::reachability::ReachabilityConfig,
    ) -> Result<crate::graph::reachability::SubgraphResult> {
        let _operation = self.enter_operation()?;
        crate::graph::reachability::query_subgraph(&read_or_recover(&self.memtable), id, config)
    }

    pub fn tsng_ground_truth(
        &self,
        query: &crate::tsng::TsngQuery<'_, T>,
    ) -> Result<crate::tsng::TsngGroundTruth> {
        let _operation = self.enter_operation()?;
        let mt = read_or_recover(&self.memtable);
        self.resource_runtime
            .pool(self.resource_runtime.parallelism())?
            .install(|| crate::tsng::exact_ground_truth(&mt, query))
    }

    /// 实验性 TSNG 联合导航。纯向量权重会显式退化到原始 QuIVer scorer。
    pub fn search_tsng(
        &self,
        query: &crate::tsng::TsngQuery<'_, T>,
        config: crate::tsng::TsngSearchConfig,
    ) -> Result<crate::tsng::TsngSearchResult> {
        let _operation = self.enter_operation()?;
        let mt = read_or_recover(&self.memtable);
        self.resource_runtime
            .pool(self.resource_runtime.parallelism())?
            .install(|| crate::tsng::approximate_search(&mt, query, config))
    }

    /// TSNG 串行对照：原始 QuIVer ANN 后执行完整属性/图过滤和联合精排。
    pub fn search_tsng_post_filter(
        &self,
        query: &crate::tsng::TsngQuery<'_, T>,
        config: crate::tsng::TsngSearchConfig,
    ) -> Result<crate::tsng::TsngSearchResult> {
        let _operation = self.enter_operation()?;
        let mt = read_or_recover(&self.memtable);
        self.resource_runtime
            .pool(self.resource_runtime.parallelism())?
            .install(|| crate::tsng::post_filter_search(&mt, query, config))
    }

    /// 图候选并集对照：原始 ANN 候选与精确图候选直接合并后联合精排。
    pub fn search_tsng_graph_union(
        &self,
        query: &crate::tsng::TsngQuery<'_, T>,
        config: crate::tsng::TsngSearchConfig,
    ) -> Result<crate::tsng::TsngSearchResult> {
        let _operation = self.enter_operation()?;
        let mt = read_or_recover(&self.memtable);
        self.resource_runtime
            .pool(self.resource_runtime.parallelism())?
            .install(|| crate::tsng::graph_union_search(&mt, query, config))
    }

    /// BQ 预筛强基线：属性候选仅以 BQ2 粗排，随后读取有限原始向量精排。
    pub fn search_tsng_bq_prefilter(
        &self,
        query: &crate::tsng::TsngQuery<'_, T>,
        config: crate::tsng::IndustrialSearchConfig,
    ) -> Result<crate::tsng::TsngSearchResult> {
        let _operation = self.enter_operation()?;
        let mt = read_or_recover(&self.memtable);
        self.resource_runtime
            .pool(self.resource_runtime.parallelism())?
            .install(|| crate::tsng::bq_prefilter_search(&mt, query, config))
    }

    /// 工业混合查询：按选择性自动使用属性/图优先、候选并集或 ANN 后过滤。
    pub fn search_tsng_industrial(
        &self,
        query: &crate::tsng::TsngQuery<'_, T>,
        config: crate::tsng::IndustrialSearchConfig,
    ) -> Result<crate::tsng::TsngSearchResult> {
        let _operation = self.enter_operation()?;
        let mt = read_or_recover(&self.memtable);
        self.resource_runtime
            .pool(self.resource_runtime.parallelism())?
            .install(|| crate::tsng::industrial_search(&mt, query, config))
    }

    pub fn index_memory_stats(&self) -> crate::observability::IndexMemoryStats {
        read_or_recover(&self.memtable).index_memory_stats()
    }

    pub fn payload_memory_stats(&self) -> crate::observability::PayloadMemoryStats {
        read_or_recover(&self.memtable).payload_memory_stats()
    }

    pub fn storage_write_stats(&self) -> crate::observability::StorageWriteStats {
        let file_len = |suffix: &str| {
            std::fs::metadata(format!("{}{}", self.db_path, suffix))
                .map(|metadata| metadata.len())
                .unwrap_or_default()
        };
        let wal = lock_or_recover(&self.wal);
        crate::observability::StorageWriteStats {
            wal_bytes: wal.cumulative_written_bytes(),
            sidecar_bytes: file_len(".pidx")
                .saturating_add(file_len(".quiver"))
                .saturating_add(file_len(".text"))
                .saturating_add(file_len(".gidx")),
            checkpoint_bytes: file_len("").saturating_add(file_len(".vec")),
            temporary_spill_bytes: 0,
            logical_bytes: wal.cumulative_logical_bytes(),
        }
    }

    pub fn search_graph_first(
        &self,
        query: &[T],
        anchor_ids: &[NodeId],
        top_k: usize,
        max_anchor_nodes: usize,
    ) -> Result<Vec<SearchHit>> {
        let _operation = self.enter_operation()?;
        let mt = read_or_recover(&self.memtable);
        self.resource_runtime
            .pool(self.resource_runtime.parallelism())?
            .install(|| {
                crate::graph::constrained::rank_within(
                    &mt,
                    query,
                    anchor_ids,
                    top_k,
                    max_anchor_nodes,
                )
            })
    }

    // ════════════════════════════════════════════════════════
    //  属性二级索引管理
    // ════════════════════════════════════════════════════════

    pub fn indexed_lookup(
        &self,
        equalities: &[(String, serde_json::Value)],
        max_results: usize,
    ) -> Result<Vec<NodeId>> {
        let _operation = self.enter_operation()?;
        if equalities.is_empty() {
            return Err(TriviumError::InvalidInput(
                "索引等值查询至少需要一个字段 (Indexed equality lookup requires at least one field)"
                    .into(),
            ));
        }
        if max_results == 0 || max_results > 1_000_000 {
            return Err(TriviumError::InvalidInput(
                "索引等值查询 max_results 必须在 1..=1000000 (max_results must be within 1..=1000000)"
                    .into(),
            ));
        }
        let mt = read_or_recover(&self.memtable);
        let filter = if equalities.len() == 1 {
            crate::filter::Filter::Eq(equalities[0].0.clone(), equalities[0].1.clone())
        } else {
            crate::filter::Filter::And(
                equalities
                    .iter()
                    .map(|(field, value)| crate::filter::Filter::Eq(field.clone(), value.clone()))
                    .collect(),
            )
        };
        let plan = crate::query::planner::plan_filter(&filter, &mt);
        if matches!(
            plan.access_path,
            crate::query::planner::AccessPath::FullNodeScan
                | crate::query::planner::AccessPath::ColdPayloadScan
        ) {
            return Err(TriviumError::InvalidInput(
                "等值查询没有可用属性索引，已拒绝全扫描 (No usable property index; full scan rejected)"
                    .into(),
            ));
        }
        let mut ids = plan
            .candidates
            .into_iter()
            .filter(|id| {
                mt.get_payload(*id)
                    .is_some_and(|payload| filter.matches(&payload))
            })
            .collect::<Vec<_>>();
        ids.sort_unstable();
        if ids.len() > max_results {
            return Err(TriviumError::QueryRowBudgetExceeded {
                budget: max_results,
            });
        }
        Ok(ids)
    }

    pub fn create_ngram_index(&mut self, field: &str) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("create_ngram_index")?;
        if field.is_empty() {
            return Err(TriviumError::InvalidInput(
                "N-gram 索引字段不能为空 (N-gram index field cannot be empty)".into(),
            ));
        }
        write_or_recover(&self.memtable).build_property_index(
            crate::index::property::PropertyIndexKind::Ngram,
            &[field.to_owned()],
            false,
            self.memory_limit,
        )
    }

    pub fn drop_ngram_index(&mut self, field: &str) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("drop_ngram_index")?;
        write_or_recover(&self.memtable).drop_ngram_property_index(field);
        Ok(())
    }

    pub fn substring_lookup(
        &self,
        field: &str,
        needle: &str,
        max_results: usize,
    ) -> Result<Vec<NodeId>> {
        let _operation = self.enter_operation()?;
        if field.is_empty() || needle.is_empty() || max_results == 0 || max_results > 1_000_000 {
            return Err(TriviumError::InvalidInput(
                "子串查询参数无效 (Invalid substring lookup parameters)".into(),
            ));
        }
        let mt = read_or_recover(&self.memtable);
        let candidates = mt
            .find_by_ngram_property_index(field, needle)?
            .ok_or_else(|| {
                TriviumError::InvalidInput(
                    "字段没有 N-gram 索引，已拒绝全扫描 (Field has no N-gram index; full scan rejected)"
                        .into(),
                )
            })?;
        let mut ids = candidates
            .into_iter()
            .filter(|id| {
                mt.get_payload(*id).is_some_and(|payload| {
                    payload
                        .get(field)
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|text| text.contains(needle))
                })
            })
            .collect::<Vec<_>>();
        ids.sort_unstable();
        if ids.len() > max_results {
            return Err(TriviumError::QueryRowBudgetExceeded {
                budget: max_results,
            });
        }
        Ok(ids)
    }

    pub fn create_unique_index(&mut self, field: &str) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("create_unique_index")?;
        if field.is_empty() {
            return Err(TriviumError::InvalidInput(
                "唯一索引字段不能为空 (Unique index field cannot be empty)".into(),
            ));
        }
        self.change_property_schema(|mt| {
            mt.build_property_index(
                crate::index::property::PropertyIndexKind::Hash,
                &[field.to_owned()],
                true,
                self.memory_limit,
            )?;
            Ok(true)
        })
    }

    pub fn create_unique_composite_index(&mut self, fields: &[String]) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("create_unique_composite_index")?;
        self.change_property_schema(|mt| {
            mt.build_property_index(
                crate::index::property::PropertyIndexKind::Composite,
                fields,
                true,
                self.memory_limit,
            )?;
            Ok(true)
        })
    }

    pub fn list_unique_indexes(&self) -> Result<Vec<Vec<String>>> {
        self.read_operation(|| read_or_recover(&self.memtable).unique_index_definitions())
    }

    /// 创建属性索引：对指定 payload 字段建立倒排索引，加速 MATCH/FIND 查询
    ///
    /// ```ignore
    /// db.create_index("name");   // 之后 MATCH (a {name: "Alice"}) 将使用 O(1) 索引
    /// db.create_index("type");   // FIND {type: "event"} 同样受益
    /// ```
    pub fn create_index(&mut self, field: &str) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("create_index")?;
        let mut mt = write_or_recover(&self.memtable);
        mt.build_property_index(
            crate::index::property::PropertyIndexKind::Hash,
            &[field.to_owned()],
            false,
            self.memory_limit,
        )?;
        Ok(())
    }

    pub fn create_composite_index(&mut self, fields: &[String]) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("create_composite_index")?;
        if fields.len() < 2 {
            return Err(TriviumError::InvalidInput(
                "复合索引至少需要两个字段 (Composite index requires at least two fields)".into(),
            ));
        }
        write_or_recover(&self.memtable).build_property_index(
            crate::index::property::PropertyIndexKind::Composite,
            fields,
            false,
            self.memory_limit,
        )?;
        Ok(())
    }

    pub fn create_bitmap_index(&mut self, field: &str) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("create_bitmap_index")?;
        write_or_recover(&self.memtable).build_property_index(
            crate::index::property::PropertyIndexKind::Bitmap,
            &[field.to_owned()],
            false,
            self.memory_limit,
        )?;
        Ok(())
    }

    pub fn drop_composite_index(&mut self, fields: &[String]) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("drop_composite_index")?;
        self.change_property_schema(|mt| {
            let was_unique = mt
                .unique_index_definitions()
                .iter()
                .any(|definition| definition == fields);
            mt.drop_composite_property_index(fields);
            Ok(was_unique)
        })
    }

    pub fn drop_bitmap_index(&mut self, field: &str) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("drop_bitmap_index")?;
        write_or_recover(&self.memtable).drop_bitmap_property_index(field);
        Ok(())
    }

    pub fn create_ordered_index(&mut self, field: &str) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("create_ordered_index")?;
        let mut mt = write_or_recover(&self.memtable);
        mt.build_property_index(
            crate::index::property::PropertyIndexKind::Ordered,
            &[field.to_owned()],
            false,
            self.memory_limit,
        )?;
        Ok(())
    }

    pub fn drop_ordered_index(&mut self, field: &str) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("drop_ordered_index")?;
        let mut mt = write_or_recover(&self.memtable);
        mt.drop_ordered_property_index(field);
        Ok(())
    }

    /// 删除属性索引
    pub fn drop_index(&mut self, field: &str) -> Result<()> {
        let _operation = self.enter_operation()?;
        self.require_write_access("drop_index")?;
        self.change_property_schema(|mt| {
            let was_unique = mt
                .unique_index_definitions()
                .iter()
                .any(|definition| definition.as_slice() == [field]);
            mt.drop_property_index(field);
            Ok(was_unique)
        })
    }

    /// 约束变更与检查点共用写锁；发布结果不明确时隔离实例，由重开按提交点恢复。
    fn change_property_schema(
        &self,
        change: impl FnOnce(&mut MemTable<T>) -> Result<bool>,
    ) -> Result<()> {
        reject_hook_reentrant_write()?;
        let mut mt = write_or_recover(&self.memtable);
        mt.ensure_readable()?;
        let mut wal = lock_or_recover(&self.wal);
        wal.ensure_writable()?;
        if !change(&mut mt)? {
            return Ok(());
        }
        if let Err(error) =
            file_format::checkpoint(&mut mt, &mut wal, &self.db_path, self.storage_mode, false)
        {
            // 既覆盖提交前失败，也覆盖提交后 WAL 清理失败，禁止继续使用未确认的约束状态。
            mt.require_recovery();
            wal.release_writer();
            return Err(error);
        }
        Ok(())
    }

    /// 返回已注册属性索引字段，按字段名稳定排序。
    pub fn list_indexes(&self) -> Result<Vec<String>> {
        self.read_operation(|| {
            let mt = read_or_recover(&self.memtable);
            let mut fields: Vec<String> = mt
                .indexed_field_names()
                .into_iter()
                .map(str::to_owned)
                .collect();
            fields.sort();
            fields
        })
    }

    /// 返回四类属性索引的完整统计，按类型和字段稳定排序。
    pub fn index_info(&self) -> Result<Vec<crate::index::property::PropertyIndexStats>> {
        self.read_operation(|| {
            let mut stats = read_or_recover(&self.memtable).property_index_stats();
            stats.sort_by(|left, right| {
                format!("{:?}", left.kind)
                    .cmp(&format!("{:?}", right.kind))
                    .then_with(|| left.fields.cmp(&right.fields))
            });
            stats
        })
    }

    /// 返回当前数据库格式、模式和 sidecar 存在状态；该操作只读且不会修复文件。
    pub fn storage_info(&self) -> StorageInfo {
        let mt = read_or_recover(&self.memtable);
        let suffixes = [
            ".vec",
            ".wal",
            ".flush_ok",
            ".quiver",
            ".quiver.meta",
            ".text",
            ".text.meta",
            ".pidx",
            ".gidx",
            ".manifest.json",
        ];
        let sidecars = suffixes
            .into_iter()
            .map(|suffix| {
                (
                    suffix.to_owned(),
                    std::path::Path::new(&format!("{}{}", self.db_path, suffix)).exists(),
                )
            })
            .collect();
        StorageInfo {
            package_version: env!("CARGO_PKG_VERSION"),
            database_format_current: crate::storage::file_format::CURRENT_VERSION,
            database_format_minimum: crate::storage::file_format::MINIMUM_SUPPORTED_VERSION,
            wal_format: crate::storage::wal::WAL_VERSION,
            property_index_format: crate::index::property::FORMAT_VERSION,
            graph_index_format: crate::storage::graph_blocks::VERSION,
            quiver_format: crate::index::quiver::QuIVer::QUIVER_VERSION,
            text_index_format: crate::index::text::TEXT_INDEX_VERSION,
            manifest_format: crate::storage::snapshot::MANIFEST_VERSION,
            dim: mt.dim(),
            node_count: mt.node_count(),
            storage_mode: match self.storage_mode {
                StorageMode::Mmap => "mmap",
                StorageMode::Rom => "rom",
            },
            access_mode: match self.access_mode {
                AccessMode::ReadWrite => "read_write",
                AccessMode::ReadOnly => "read_only",
                AccessMode::Immutable => "immutable",
            },
            estimated_memory_bytes: mt.estimated_memory_bytes(),
            sidecars,
        }
    }

    // ════════════════════════════════════════════════════════
    //  TQL 统一查询接口
    // ════════════════════════════════════════════════════════

    /// TQL (Trivium Query Language) 统一查询入口
    ///
    /// 支持三种查询模式：
    /// - `FIND {type: "event"} RETURN *` — 文档过滤
    /// - `MATCH (a)-[:knows]->(b) WHERE b.age > 20 RETURN b` — 图遍历
    /// - `SEARCH VECTOR [...] TOP 10 RETURN *` — 向量检索
    ///
    /// ```ignore
    /// let results = db.tql("FIND {type: \"event\", heat: {$gte: 0.7}} RETURN * LIMIT 10")?;
    /// ```
    /// 执行 TQL，并返回可同时承载节点与标量列的一等值结果。
    pub fn tql(&self, input: &str) -> Result<crate::query::tql_executor::TqlValueResult<T>> {
        let _operation = self.enter_operation()?;
        let query = crate::query::tql_parser::parse_tql(input).map_err(TriviumError::QueryParse)?;
        let mt = read_or_recover(&self.memtable);
        crate::query::tql_executor::execute_tql_values_with_limits(
            &query,
            &mt,
            self.tql_limits.clone(),
        )
    }

    pub fn tql_with_control(
        &self,
        input: &str,
        control: crate::query::tql_executor::QueryControl,
    ) -> Result<crate::query::tql_executor::TqlValueResult<T>> {
        let _operation = self.enter_operation()?;
        let query = crate::query::tql_parser::parse_tql(input).map_err(TriviumError::QueryParse)?;
        let mt = read_or_recover(&self.memtable);
        let mut limits = self.tql_limits.clone();
        limits.query_control = Some(control);
        crate::query::tql_executor::execute_tql_values_with_limits(&query, &mt, limits)
    }

    /// 执行仅返回节点绑定的 TQL。
    pub fn tql_nodes(&self, input: &str) -> Result<crate::query::tql_executor::TqlResult<T>> {
        let _operation = self.enter_operation()?;
        let query = crate::query::tql_parser::parse_tql(input).map_err(TriviumError::QueryParse)?;
        let mt = read_or_recover(&self.memtable);
        crate::query::tql_executor::execute_tql_with_limits(&query, &mt, self.tql_limits.clone())
    }

    /// 执行 TQL，并返回可同时承载节点与标量列的一等值结果。
    pub fn query(&self, input: &str) -> Result<crate::query::tql_executor::TqlValueResult<T>> {
        let _operation = self.enter_operation()?;
        let query = crate::query::tql_parser::parse_tql(input).map_err(TriviumError::QueryParse)?;
        let mt = read_or_recover(&self.memtable);
        crate::query::tql_executor::execute_tql_values_with_limits(
            &query,
            &mt,
            self.tql_limits.clone(),
        )
    }

    /// `query` 的兼容别名。
    pub fn tql_values(&self, input: &str) -> Result<crate::query::tql_executor::TqlValueResult<T>> {
        let _operation = self.enter_operation()?;
        self.query(input)
    }

    /// 解析并准备可重复绑定执行的只读 TQL。
    pub fn prepare_tql(&self, input: &str) -> Result<crate::query::tql_prepared::PreparedTql> {
        let _operation = self.enter_operation()?;
        let query = crate::query::tql_parser::parse_tql(input).map_err(TriviumError::QueryParse)?;
        Ok(crate::query::tql_prepared::PreparedTql::from_query(query))
    }

    /// 执行已准备的 TQL，并返回一等值结果。
    pub fn execute_prepared_tql(
        &self,
        prepared: &crate::query::tql_prepared::PreparedTql,
        parameters: &std::collections::HashMap<String, crate::query::tql_prepared::TqlParamValue>,
    ) -> Result<crate::query::tql_executor::TqlValueResult<T>> {
        let _operation = self.enter_operation()?;
        let query = prepared.bind(parameters)?;
        let mt = read_or_recover(&self.memtable);
        crate::query::tql_executor::execute_tql_values_with_limits(
            &query,
            &mt,
            self.tql_limits.clone(),
        )
    }

    pub fn execute_prepared_tql_with_control(
        &self,
        prepared: &crate::query::tql_prepared::PreparedTql,
        parameters: &std::collections::HashMap<String, crate::query::tql_prepared::TqlParamValue>,
        control: crate::query::tql_executor::QueryControl,
    ) -> Result<crate::query::tql_executor::TqlValueResult<T>> {
        let _operation = self.enter_operation()?;
        let query = prepared.bind(parameters)?;
        let mt = read_or_recover(&self.memtable);
        let mut limits = self.tql_limits.clone();
        limits.query_control = Some(control);
        crate::query::tql_executor::execute_tql_values_with_limits(&query, &mt, limits)
    }

    /// TQL 写操作入口
    ///
    /// 支持三种写操作：
    /// - `CREATE ({name: "Alice", age: 30})` — 创建节点
    /// - `MATCH (a) WHERE a.name == "Alice" SET a.age == 31` — 更新字段
    /// - `MATCH (a) WHERE a.name == "Alice" DELETE a` — 删除节点
    /// - `MATCH (a) WHERE a.name == "Alice" DETACH DELETE a` — 删除节点及其边
    /// - `MATCH (a), (b) WHERE ... CREATE (a)-[:knows]->(b)` — 创建边
    ///
    /// 也兼容读查询（自动降级为 tql()），返回 affected=0。
    ///
    /// ```ignore
    /// let result = db.tql_mut(r#"CREATE ({name: "Alice", age: 30})"#)?;
    /// assert_eq!(result.created_ids.len(), 1);
    /// ```
    pub fn tql_mut(&mut self, input: &str) -> Result<crate::query::tql_executor::TqlMutResult> {
        self.tql_mut_with_id_limit(input, NodeId::MAX - 1)
    }

    pub(crate) fn tql_mut_with_id_limit(
        &mut self,
        input: &str,
        max_id: NodeId,
    ) -> Result<crate::query::tql_executor::TqlMutResult> {
        self.require_write_access("tql_mut")?;
        let _operation = self.enter_operation()?;
        use crate::query::tql_ast::TqlStatement;
        use crate::query::tql_executor::{MutationOp, TqlMutResult};

        let stmt = crate::query::tql_parser::parse_tql_statement(input)
            .map_err(TriviumError::QueryParse)?;

        match stmt {
            TqlStatement::Query(_) => Err(TriviumError::ApiMigrationRequired {
                removed_api: "tql_mut(read query)",
                replacement: "tql() 或 tql_values()",
            }),
            TqlStatement::Mutation(mutation) => {
                let (ops, mut next_id) = {
                    let mt = read_or_recover(&self.memtable);
                    (
                        crate::query::tql_executor::execute_tql_mutation_with_limits(
                            &mutation,
                            &mt,
                            self.tql_limits.clone(),
                        )?,
                        mt.next_id_value(),
                    )
                };

                let mut affected = 0usize;
                let mut var_id_map: std::collections::HashMap<String, u64> =
                    std::collections::HashMap::new();
                for op in &ops {
                    if let MutationOp::InsertNode { var, .. } = op {
                        if next_id > max_id {
                            return Err(TriviumError::InvalidInput("节点 ID 超出当前接口范围 (Node ID exceeds this API's supported range)".into()));
                        }
                        var_id_map.insert(var.clone(), next_id);
                        next_id = next_id.checked_add(1).ok_or_else(|| {
                            TriviumError::InvalidInput("节点 ID 已耗尽 (Node IDs exhausted)".into())
                        })?;
                    }
                }

                let mut tx_ops = Vec::with_capacity(ops.len());
                for op in ops {
                    match op {
                        MutationOp::InsertNode {
                            var,
                            vector,
                            payload,
                        } => {
                            let id = var_id_map[&var];
                            tx_ops.push(transaction::TxOp::InsertWithId {
                                id,
                                vector,
                                payload,
                            });
                            affected += 1;
                        }
                        MutationOp::LinkEdge {
                            src_id,
                            dst_id,
                            src_var,
                            dst_var,
                            label,
                            weight,
                        } => {
                            let src = if src_id == 0 {
                                var_id_map.get(&src_var).copied().ok_or_else(|| {
                                    TriviumError::QueryExecution(format!(
                                        "CREATE 边引用了未定义变量 {src_var}"
                                    ))
                                })?
                            } else {
                                src_id
                            };
                            let dst = if dst_id == 0 {
                                var_id_map.get(&dst_var).copied().ok_or_else(|| {
                                    TriviumError::QueryExecution(format!(
                                        "CREATE 边引用了未定义变量 {dst_var}"
                                    ))
                                })?
                            } else {
                                dst_id
                            };
                            tx_ops.push(transaction::TxOp::Link {
                                src,
                                dst,
                                label,
                                weight,
                                metadata: serde_json::Value::Null,
                            });
                            affected += 1;
                        }
                        MutationOp::UpdateVector { id, vector } => {
                            tx_ops.push(transaction::TxOp::UpdateVector { id, vector });
                            affected += 1;
                        }
                        MutationOp::UpdatePayload { id, payload } => {
                            tx_ops.push(transaction::TxOp::UpdatePayload { id, payload });
                            affected += 1;
                        }
                        MutationOp::DeleteNode { id, .. } => {
                            // 事务删除统一清理关联边；整批节点共享同一预检与 WAL 提交边界。
                            tx_ops.push(transaction::TxOp::Delete { id });
                            affected += 1;
                        }
                    }
                }

                let created_ids = self.commit_ops(tx_ops)?;

                Ok(TqlMutResult {
                    affected,
                    created_ids,
                })
            }
        }
    }

    // ════════════════════════════════════════════════════════
    //  持久化 / 关闭
    // ════════════════════════════════════════════════════════

    /// 将内存数据持久化到磁盘
    ///
    /// 安全顺序（防止崩溃丢数据）：
    ///   1. 原子写入 .tdb（写 .tmp → fsync → rename）
    ///   2. 确认 .tdb 写入成功后，才清除 WAL
    pub fn flush(&mut self) -> Result<()> {
        self.require_write_access("flush")?;
        let _operation = self.enter_maintenance_operation()?;
        self.flush_inner()
    }

    pub fn publish_generation_manifest(
        &mut self,
        generation_id: &str,
    ) -> Result<crate::storage::snapshot::GenerationManifest> {
        let _operation = self.enter_maintenance_operation()?;
        self.require_write_access("publish_generation_manifest")?;
        self.flush()?;
        crate::storage::snapshot::write_manifest(
            &self.db_path,
            generation_id,
            std::any::type_name::<T>(),
            self.dim(),
            self.node_count()?,
        )
    }

    /// 保存当前一致状态到独立的新快照目录；持有写锁直到复制和发布完成。
    pub fn create_snapshot(
        &mut self,
        destination: &str,
        generation_id: &str,
    ) -> Result<crate::storage::snapshot::GenerationManifest> {
        self.require_write_access("create_snapshot")?;
        reject_hook_reentrant_write()?;
        let _operation = self.enter_maintenance_operation()?;
        let mut mt = write_or_recover(&self.memtable);
        file_format::checkpoint(
            &mut mt,
            &mut lock_or_recover(&self.wal),
            &self.db_path,
            self.storage_mode,
            false,
        )?;
        crate::storage::snapshot::copy_database_snapshot(
            &self.db_path,
            destination,
            generation_id,
            std::any::type_name::<T>(),
            mt.dim(),
            mt.node_count(),
            &self.resource_runtime,
        )
    }

    fn flush_inner(&self) -> Result<()> {
        self.require_write_access("flush")?;
        reject_hook_reentrant_write()?;
        let mut mt = write_or_recover(&self.memtable);
        file_format::checkpoint(
            &mut mt,
            &mut lock_or_recover(&self.wal),
            &self.db_path,
            self.storage_mode,
            false,
        )?;
        drop(mt);
        read_or_recover(&self.memtable).advise_cold_vectors_dontneed();
        Ok(())
    }

    pub fn close(&mut self) -> Result<()> {
        reject_hook_reentrant_write()?;
        self.reject_group_shutdown()?;
        {
            let mut state = lock_or_recover(&self.lifecycle.0);
            if state.lifecycle == DatabaseLifecycle::Closing {
                drop(state);
                self.request_shutdown()?;
                return self.wait_for_shutdown(None).map(|_| ());
            }
            if state.lifecycle == DatabaseLifecycle::Closed {
                return Err(TriviumError::DatabaseClosed);
            }
            state.lifecycle = DatabaseLifecycle::Closing;
            state.shutdown_phase = ShutdownPhase::Draining;
            state.shutdown_last_error = None;
            state.shutdown_last_failure = None;
        }

        self.compaction.take();

        {
            let mut state = lock_or_recover(&self.lifecycle.0);
            while state.active_operations != 0 {
                state = self
                    .lifecycle
                    .1
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        }

        let _resource_task = match self.resource_runtime.enter_maintenance_task() {
            Ok(task) => task,
            Err(error) => {
                let mut state = lock_or_recover(&self.lifecycle.0);
                state.lifecycle = DatabaseLifecycle::Open;
                state.shutdown_phase = ShutdownPhase::Open;
                state.shutdown_last_error = Some(error.to_string());
                state.shutdown_last_failure =
                    Some(crate::resources::ResourceFailure::from_error(&error));
                self.lifecycle.1.notify_all();
                return Err(error);
            }
        };
        lock_or_recover(&self.lifecycle.0).shutdown_phase = ShutdownPhase::Flushing;
        let persisted = if self.access_mode == AccessMode::ReadWrite {
            let mut mt = write_or_recover(&self.memtable);
            let mut wal = lock_or_recover(&self.wal);
            file_format::checkpoint(&mut mt, &mut wal, &self.db_path, self.storage_mode, true)
                .and_then(|_| wal.close_writer())
        } else {
            Ok(())
        };
        if let Err(error) = persisted {
            let mut state = lock_or_recover(&self.lifecycle.0);
            state.lifecycle = DatabaseLifecycle::Open;
            state.shutdown_phase = ShutdownPhase::Open;
            state.shutdown_last_error = Some(error.to_string());
            state.shutdown_last_failure =
                Some(crate::resources::ResourceFailure::from_error(&error));
            self.lifecycle.1.notify_all();
            return Err(error);
        }

        write_or_recover(&self.memtable).release_resource_storage();
        clear_hook_slot(&self.hook);
        self.release_lock();
        let mut state = lock_or_recover(&self.lifecycle.0);
        state.lifecycle = DatabaseLifecycle::Closed;
        state.shutdown_phase = ShutdownPhase::Closed;
        self.lifecycle.1.notify_all();
        Ok(())
    }

    /// 仅在 close 成功完成持久化后释放文件锁。
    fn release_lock(&mut self) {
        lock_or_recover(&self._lock_file).take();
    }

    pub fn node_count(&self) -> Result<usize> {
        self.read_operation(|| read_or_recover(&self.memtable).node_count())
    }
    pub fn contains(&self, id: NodeId) -> Result<bool> {
        self.read_operation(|| read_or_recover(&self.memtable).contains(id))
    }
    pub fn dim(&self) -> usize {
        read_or_recover(&self.memtable).dim()
    }

    /// 获取所有活跃节点的 ID 列表
    pub fn all_node_ids(&self) -> Result<Vec<NodeId>> {
        self.read_operation(|| read_or_recover(&self.memtable).all_node_ids())
    }

    // ════════════════════════════════════════════════════════
    //  维度迁移
    // ════════════════════════════════════════════════════════

    /// 维度迁移：从当前数据库导出所有节点和边到一个新维度的数据库。
    pub fn migrate_to(&self, new_path: &str, new_dim: usize) -> Result<(Database<T>, Vec<NodeId>)>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        let _operation = self.enter_operation()?;
        migration::migrate(self, new_path, new_dim)
    }

    // ════════════════════════════════════════════════════════
    //  事务
    // ════════════════════════════════════════════════════════

    /// 开启一个轻量级事务
    ///
    /// 事务期间所有写操作仅缓冲在内存中，调用 commit() 后原子性写入。
    pub fn begin_tx(&mut self) -> Transaction<'_, T> {
        Transaction {
            db: self,
            ops: Vec::new(),
            committed: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn close_wal_writer_for_test(&mut self) {
        lock_or_recover(&self.wal).close_writer_for_test();
    }
}

/// 安全析构：确保 WAL BufWriter 的缓冲数据在 Database 被 drop 时显式落盘。
impl<T: VectorType> Drop for Database<T> {
    fn drop(&mut self) {
        // 1. 停止自动压缩线程
        self.compaction.take();

        // 2. 显式 flush WAL BufWriter 到磁盘
        let pending_close = lock_or_recover(&self.lifecycle.0).shutdown_job.is_some();
        if !pending_close
            && self.access_mode == AccessMode::ReadWrite
            && let Ok(mut w) = self.wal.lock()
        {
            w.flush_writer();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Database, DatabaseLifecycle, lock_or_recover};
    use crate::error::TriviumError;
    use crate::graph::reachability::ReachabilityConfig;
    use serde_json::json;

    fn open_db(name: &str) -> Database<f32> {
        let dir = std::env::temp_dir().join(format!("tdb_wal_first_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Database::open(&dir.join("test.tdb").to_string_lossy(), 3).unwrap()
    }

    #[cfg(feature = "test-hooks")]
    #[test]
    fn 约束发布失败隔离已准入共享查询() {
        use crate::test_hooks::{IoPoint, fail_io_at};
        let mut db = open_db(&format!("schema_quarantine_{}", std::process::id()));
        db.insert_with_id(1, &[1., 0., 0.], json!({"email":"one"}))
            .unwrap();
        db.flush().unwrap();
        let handle = db.search_handle();
        let admitted = handle.enter_operation().unwrap();
        {
            let _failure = fail_io_at(IoPoint::PropertySidecarWrite);
            assert!(matches!(
                db.create_unique_index("email"),
                Err(TriviumError::Io(_))
            ));
        }
        assert!(matches!(
            admitted.check_recovery(),
            Err(TriviumError::WalClosed)
        ));
        assert!(matches!(
            handle.enter_operation(),
            Err(TriviumError::WalClosed)
        ));
        drop(admitted);
        drop(handle);
        let path = db.db_path.clone();
        drop(db);
        let mut recovered = Database::<f32>::open(&path, 3).unwrap();
        assert!(recovered.list_unique_indexes().unwrap().is_empty());
        assert_eq!(
            recovered.get_payload(1).unwrap(),
            Some(json!({"email":"one"}))
        );
        recovered.close().unwrap();
        drop(recovered);
        std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn 向量映射失效会隔离writer与已准入句柄() {
        use std::os::windows::fs::OpenOptionsExt;
        let mut db = open_db(&format!("mapping_quarantine_{}", std::process::id()));
        let path = db.db_path.clone();
        db.insert_with_id(1, &[1.0, 0.0, 0.0], json!({"state":"accepted"}))
            .unwrap();
        db.flush().unwrap();
        let handle = db.search_handle();
        let admitted = handle.enter_operation().unwrap();
        let blocker = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(3)
            .open(format!("{path}.vec"))
            .unwrap();
        db.update_vector(1, &[0.0, 1.0, 0.0]).unwrap();
        assert!(matches!(db.flush(), Err(TriviumError::Io(_))));
        assert!(matches!(
            admitted.check_recovery(),
            Err(TriviumError::WalClosed)
        ));
        assert!(matches!(
            lock_or_recover(&db.wal).ensure_writable(),
            Err(TriviumError::WalClosed)
        ));
        drop(blocker);
        drop(admitted);
        drop(handle);
        drop(db);
        let mut recovered = Database::<f32>::open(&path, 3).unwrap();
        let node = recovered.get(1).unwrap().unwrap();
        assert_eq!(node.vector, vec![0.0, 1.0, 0.0]);
        assert_eq!(node.payload, json!({"state":"accepted"}));
        recovered.close().unwrap();
        drop(recovered);
        std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
    }

    #[test]
    fn close成功后可失败公开操作拒绝旧句柄() {
        let mut db = open_db("close_rejects_stale_handle");
        let id = db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap();

        db.close().unwrap();
        assert_eq!(
            lock_or_recover(&db.lifecycle.0).lifecycle,
            DatabaseLifecycle::Closed
        );

        let insert_after_close = db.insert(&[0.0, 1.0, 0.0], json!({}));
        assert!(
            matches!(insert_after_close, Err(TriviumError::DatabaseClosed)),
            "关闭后插入结果异常: {insert_after_close:?}"
        );
        assert!(matches!(db.flush(), Err(TriviumError::DatabaseClosed)));
        assert!(matches!(
            db.update_vector(id, &[0.0, 0.0, 1.0]),
            Err(TriviumError::DatabaseClosed)
        ));
        assert!(matches!(
            db.search(&[1.0, 0.0, 0.0], 1, 0, 0.0),
            Err(TriviumError::DatabaseClosed)
        ));
        assert!(matches!(
            db.search_exact(&[1.0, 0.0, 0.0], 1),
            Err(TriviumError::DatabaseClosed)
        ));
        assert!(matches!(
            db.tql("FIND {} RETURN *"),
            Err(TriviumError::DatabaseClosed)
        ));
        assert!(matches!(
            db.reachable(id, &ReachabilityConfig::default()),
            Err(TriviumError::DatabaseClosed)
        ));
        assert!(matches!(
            db.search_graph_first(&[1.0, 0.0, 0.0], &[id], 1, 10),
            Err(TriviumError::DatabaseClosed)
        ));
        assert!(matches!(db.close(), Err(TriviumError::DatabaseClosed)));
    }

    #[test]
    fn close落盘失败恢复open并保留文件锁() {
        let mut db = open_db("close_failure_reopens");
        let path = db.db_path.clone();
        db.insert(&[1.0, 0.0, 0.0], json!({})).unwrap();
        db.close_wal_writer_for_test();
        std::fs::remove_file(format!("{path}.wal")).unwrap();

        let close_result = db.close();
        assert!(close_result.is_err(), "关闭 WAL 后 close 必须失败");
        assert!(matches!(
            Database::<f32>::open(&path, 3),
            Err(TriviumError::DatabaseLocked(_))
        ));
        assert!(matches!(
            db.insert(&[0.0, 1.0, 0.0], json!({})),
            Err(TriviumError::WalClosed)
        ));
    }

    #[test]
    fn 普通crud_wal失败不修改内存状态() {
        let mut db = open_db("crud");
        let first = db
            .insert(&[1.0, 0.0, 0.0], json!({"name": "first"}))
            .unwrap();
        let second = db
            .insert(&[0.0, 1.0, 0.0], json!({"name": "second"}))
            .unwrap();
        db.link(first, second, "rel", 1.0).unwrap();
        db.close_wal_writer_for_test();

        assert!(matches!(
            db.insert(&[0.0, 0.0, 1.0], json!({})),
            Err(TriviumError::WalClosed)
        ));
        assert_eq!(db.node_count().unwrap(), 2);
        assert!(matches!(
            db.insert_with_id(42, &[0.0, 0.0, 1.0], json!({})),
            Err(TriviumError::WalClosed)
        ));
        assert!(!db.contains(42).unwrap());
        assert!(matches!(
            db.link(second, first, "back", 1.0),
            Err(TriviumError::WalClosed)
        ));
        assert!(db.get_edges(second).unwrap().is_empty());
        assert!(matches!(
            db.unlink(first, second),
            Err(TriviumError::WalClosed)
        ));
        assert_eq!(db.get_edges(first).unwrap().len(), 1);
        assert!(matches!(
            db.update_payload(first, json!({"name": "changed"})),
            Err(TriviumError::WalClosed)
        ));
        assert_eq!(db.get_payload(first).unwrap().unwrap()["name"], "first");
        assert!(matches!(
            db.patch_payload(first, json!({"$set": {"name": "patched"}})),
            Err(TriviumError::WalClosed)
        ));
        assert_eq!(db.get_payload(first).unwrap().unwrap()["name"], "first");
        assert!(matches!(
            db.update_vector(first, &[9.0, 9.0, 9.0]),
            Err(TriviumError::WalClosed)
        ));
        assert_eq!(db.get(first).unwrap().unwrap().vector, vec![1.0, 0.0, 0.0]);
        assert!(matches!(db.delete(first), Err(TriviumError::WalClosed)));
        assert!(db.contains(first).unwrap());
    }
}
