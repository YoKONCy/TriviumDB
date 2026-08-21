//! 数据库核心模块
//!
//! 重构后按职责拆分为 4 个子文件：
//! - `config.rs`: 配置类型（StorageMode, Config, SearchConfig）
//! - `pipeline.rs`: 混合检索管线（L0-L9 + 6 个 Hook 调用点）
//! - `transaction.rs`: 轻量级事务（Dry-Run + WAL-first 语义）+ WAL 回放
//! - `mod.rs`（本文件）: Database 结构体 + CRUD + 生命周期管理

pub mod config;
pub(crate) mod pipeline;
pub mod transaction;

// 从子模块重导出公开类型，保持对外 API 不变
pub use config::{Config, SearchConfig, StorageMode};
pub use transaction::{Transaction, TxBuilder};

use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::hook::{HookContext, NoopHook, SearchHook};

use crate::node::{NodeId, SearchHit};
use crate::storage::compaction::CompactionThread;
use crate::storage::file_format;
use crate::storage::memtable::MemTable;
use crate::storage::wal::{SyncMode, Wal, WalEntry};
use fs2::FileExt;

use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DatabaseLifecycle {
    Open,
    Closing,
    Closed,
}

struct LifecycleState {
    lifecycle: DatabaseLifecycle,
    active_operations: usize,
}

struct OperationGuard {
    lifecycle: Arc<(Mutex<LifecycleState>, Condvar)>,
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

/// 数据库核心入口实例
pub struct Database<T: VectorType> {
    pub(crate) db_path: String,
    pub(crate) memtable: Arc<RwLock<MemTable<T>>>,
    pub(crate) wal: Arc<Mutex<Wal>>,
    pub(crate) compaction: Option<CompactionThread>,
    /// 文件锁：防止多进程同时打开同一个数据库
    /// Option 化以便 close() 时显式 take 释放（否则锁要等对象 Drop，JS GC 时机不可控）
    _lock_file: Option<std::fs::File>,
    /// 内存上限（字节），0 = 无限制
    memory_limit: usize,
    /// 存储模式
    pub(crate) storage_mode: StorageMode,
    /// 检索管线 Hook（默认 NoopHook，零开销）
    hook: Arc<dyn SearchHook>,
    /// 有状态 fatigue 查询保持数据库级顺序语义。
    stateful_search: Arc<Mutex<()>>,
    /// 正在锁外构建的 QuIVer 向量代际，避免重复构建。
    quiver_builds: Arc<(Mutex<std::collections::HashSet<u64>>, std::sync::Condvar)>,
    /// 生命周期和已进入操作计数；Closing 阻止新操作并等待现有操作退出。
    lifecycle: Arc<(Mutex<LifecycleState>, Condvar)>,
}

impl<T: VectorType + serde::Serialize + serde::de::DeserializeOwned> Database<T> {
    fn enter_operation(&self) -> Result<OperationGuard> {
        let mut state = lock_or_recover(&self.lifecycle.0);
        if state.lifecycle != DatabaseLifecycle::Open {
            return Err(TriviumError::DatabaseClosed);
        }
        state.active_operations = state
            .active_operations
            .checked_add(1)
            .ok_or_else(|| TriviumError::InvalidInput("数据库活动操作计数溢出".into()))?;
        drop(state);
        Ok(OperationGuard {
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

    /// 打开或创建数据库，指定 WAL 同步模式 (向后兼容)
    pub fn open_with_sync(path: &str, dim: usize, sync_mode: SyncMode) -> Result<Self> {
        let config = Config {
            dim,
            sync_mode,
            ..Default::default()
        };
        Self::open_with_config(path, config)
    }

    /// 打开或创建数据库（高级配置入口）
    pub fn open_with_config(path: &str, config: Config) -> Result<Self> {
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
        if let Some(parent_dir) = std::path::Path::new(path).parent()
            && !parent_dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent_dir)?;
        }

        // ═══ 文件锁：防止多进程并发写同一个数据库 ═══
        let lock_path = format!("{}.lock", path);
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)?;
        lock_file.try_lock_exclusive().map_err(|_| {
            TriviumError::DatabaseLocked(format!(
                "Database '{}' is already opened by another process. \
                 If this is unexpected, delete '{}'",
                path, lock_path
            ))
        })?;

        let mut memtable = if std::path::Path::new(path).exists() {
            file_format::load(path, config.storage_mode, config.load_text_index)?
        } else {
            MemTable::new(dim)
        };

        if Wal::needs_recovery(path) {
            let (entries, valid_offset) = Wal::read_entries::<T>(path)?;

            // 执行极其关键的物理防串局截断（Truncation）！
            let wal_path = format!("{}.wal", path);
            let wal_file = std::fs::OpenOptions::new().write(true).open(&wal_path)?;
            wal_file.set_len(valid_offset)?;
            wal_file.sync_all()?;

            if !entries.is_empty() {
                tracing::info!(
                    "正在从 WAL 恢复 {} 条记录，安全截断至偏移 {} (Recovering {} entries from WAL, truncated at offset {})",
                    entries.len(),
                    valid_offset,
                    entries.len(),
                    valid_offset
                );
                for entry in entries {
                    transaction::replay_entry(&mut memtable, entry);
                }
            } else {
                tracing::info!(
                    "已清除损坏/未提交的 WAL 数据，回退至偏移 {} (Cleared corrupt/uncommitted WAL, truncated to {})",
                    valid_offset,
                    valid_offset
                );
            }
        }

        // TextIndex 由 sidecar 精确恢复；缺失时保持为空，避免打开数据库时
        // 无条件扫描全部 Payload 并改变用户显式 index_text() 的索引语义。
        memtable.set_auto_build_quiver(config.auto_build_quiver);

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

        let wal = Wal::open_with_sync(path, config.sync_mode)?;
        Ok(Self {
            db_path: path.to_string(),
            memtable: Arc::new(RwLock::new(memtable)),
            wal: Arc::new(Mutex::new(wal)),
            compaction: None,
            _lock_file: Some(lock_file),
            memory_limit: config.memory_limit,
            storage_mode: config.storage_mode,
            hook: Arc::new(NoopHook),
            stateful_search: Arc::new(Mutex::new(())),
            quiver_builds: Arc::new((
                Mutex::new(std::collections::HashSet::new()),
                std::sync::Condvar::new(),
            )),
            lifecycle: Arc::new((
                Mutex::new(LifecycleState {
                    lifecycle: DatabaseLifecycle::Open,
                    active_operations: 0,
                }),
                Condvar::new(),
            )),
        })
    }

    // ════════════════════════════════════════════════════════
    //  配置管理
    // ════════════════════════════════════════════════════════

    /// 运行时切换 WAL 同步模式
    pub fn set_sync_mode(&mut self, mode: SyncMode) {
        let mut w = lock_or_recover(&self.wal);
        w.set_sync_mode(mode);
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
    /// db.set_hook(MyHook);
    /// ```
    pub fn set_hook(&mut self, hook: impl SearchHook + 'static) {
        self.hook = Arc::new(hook);
    }

    /// 移除当前 Hook，恢复为默认的 NoopHook
    pub fn clear_hook(&mut self) {
        self.hook = Arc::new(NoopHook);
    }

    /// 获取当前 Hook 的引用（主要用于测试和调试）
    pub fn hook(&self) -> &dyn SearchHook {
        self.hook.as_ref()
    }

    // ════════════════════════════════════════════════════════
    //  QuIVer 索引管理
    // ════════════════════════════════════════════════════════

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
        let cfg = config.unwrap_or_default();
        {
            let mt = read_or_recover(&self.memtable);
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
        }
        let snapshot = read_or_recover(&self.memtable).quiver_build_snapshot();
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
            let index = MemTable::<T>::build_quiver_snapshot(snapshot, &cfg);
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

    // ════════════════════════════════════════════════════════
    //  内存管理
    // ════════════════════════════════════════════════════════

    /// 设置内存上限（字节）
    ///
    /// 当 MemTable 估算内存超过此值时，写操作后会自动触发 flush 落盘。
    /// 设为 0 表示无限制（默认）。
    pub fn set_memory_limit(&mut self, bytes: usize) {
        self.memory_limit = bytes;
    }

    /// 为后续插入主动预留额外节点容量。
    ///
    /// 预留受内核内存预算约束；失败不会修改 WAL、节点、ID 或 generation。
    pub fn reserve_nodes(&self, additional: usize) -> Result<()> {
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

    /// 内部方法：检查内存压力，超出上限时自动 flush
    fn check_memory_pressure(&mut self) {
        if self.memory_limit > 0 {
            let usage = write_or_recover(&self.memtable).estimated_memory_bytes();
            if usage > self.memory_limit {
                tracing::info!(
                    "内存压力: {}MB > 上限 {}MB，自动落盘中 (Memory pressure: {}MB > limit {}MB, auto-flushing)",
                    usage / (1024 * 1024),
                    self.memory_limit / (1024 * 1024),
                    usage / (1024 * 1024),
                    self.memory_limit / (1024 * 1024)
                );
                if let Err(e) = self.flush() {
                    tracing::error!("自动落盘失败 (Auto-flush failed): {}", e);
                }
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
    pub fn clear_search_state(&self) {
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeClearSearchStateLock);
        let _stateful_guard = lock_or_recover(&self.stateful_search);
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::ClearSearchStateEntered);
        read_or_recover(&self.memtable).clear_fatigue();
    }

    pub fn set_auto_build_quiver(&mut self, enabled: bool) {
        write_or_recover(&self.memtable).set_auto_build_quiver(enabled);
    }

    /// 启动后台自动 Compaction 线程
    pub fn enable_auto_compaction(&mut self, interval: Duration) -> Result<()> {
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
        );
        self.compaction = Some(ct);
        Ok(())
    }

    pub fn disable_auto_compaction(&mut self) {
        self.compaction.take();
    }

    /// 主动触发全量重写与压实（Manual Compaction）
    pub fn compact(&mut self) -> Result<()> {
        reject_hook_reentrant_write()?;
        {
            let mut mt = write_or_recover(&self.memtable);
            tracing::info!("手动压实开始 (Manual compaction started): {}", self.db_path);
            // 压实只准备持久化需要的 BQ；ANN 构建由查询或显式 API 负责。
            mt.prepare_persistence_cache(false);
        }

        {
            let mut mt = write_or_recover(&self.memtable);
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeCompactionSave);
            file_format::save(&mut mt, &self.db_path, self.storage_mode)?;
            let mut w = lock_or_recover(&self.wal);
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeWalClear);
            w.clear()?;
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
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        let payload_str = payload.to_string();
        if payload_str.len() > 8 * 1024 * 1024 {
            return Err(crate::error::TriviumError::PayloadTooLarge {
                size_bytes: payload_str.len(),
                max_bytes: 8 * 1024 * 1024,
            });
        }

        let id = {
            let mut mt = write_or_recover(&self.memtable);
            mt.validate_insert(vector)?;
            let id = mt.next_id_value();
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
        let _operation = self.enter_operation()?;
        let payload_str = payload.to_string();
        reject_hook_reentrant_write()?;
        if payload_str.len() > 8 * 1024 * 1024 {
            return Err(crate::error::TriviumError::PayloadTooLarge {
                size_bytes: payload_str.len(),
                max_bytes: 8 * 1024 * 1024,
            });
        }

        {
            let mut mt = write_or_recover(&self.memtable);
            mt.validate_insert_with_id(id, vector)?;
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

    pub fn link(&mut self, src: NodeId, dst: NodeId, label: &str, weight: f32) -> Result<()> {
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        {
            let mut mt = write_or_recover(&self.memtable);
            mt.validate_link(src, dst)?;
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
            })?;
            mt.link(src, dst, label.to_string(), weight)?;
        }
        Ok(())
    }

    pub fn delete(&mut self, id: NodeId) -> Result<()> {
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        {
            let mut mt = write_or_recover(&self.memtable);
            mt.validate_delete(id)?;
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::Delete::<T> { id })?;
            mt.delete(id)?;
        }

        Ok(())
    }

    pub fn unlink(&mut self, src: NodeId, dst: NodeId) -> Result<()> {
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
        reject_hook_reentrant_write()?;
        let payload_str = payload.to_string();
        if payload_str.len() > 8 * 1024 * 1024 {
            return Err(crate::error::TriviumError::PayloadTooLarge {
                size_bytes: payload_str.len(),
                max_bytes: 8 * 1024 * 1024,
            });
        }

        {
            let mut mt = write_or_recover(&self.memtable);
            mt.validate_update_payload(id)?;
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
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        let final_payload = {
            let mt = read_or_recover(&self.memtable);
            mt.preview_patch_payload(id, &patch)?
        };

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
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
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
        let mut result = crate::graph::leiden::run_leiden(&snapshot, &config);

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
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        let mut mt = write_or_recover(&self.memtable);
        mt.index_keyword(id, keyword);
        Ok(())
    }

    pub fn index_text(&mut self, id: NodeId, text: &str) -> Result<()> {
        reject_hook_reentrant_write()?;
        let mut mt = write_or_recover(&self.memtable);
        mt.index_text(id, text);
        Ok(())
    }

    pub fn build_text_index(&mut self) -> Result<()> {
        let _operation = self.enter_operation()?;
        reject_hook_reentrant_write()?;
        let mut mt = write_or_recover(&self.memtable);
        mt.build_text_index();
        Ok(())
    }

    pub fn get_payload(&self, id: NodeId) -> Option<serde_json::Value> {
        let mt = read_or_recover(&self.memtable);
        mt.get_payload(id).cloned()
    }

    pub fn get_edges(&self, id: NodeId) -> Vec<crate::node::Edge> {
        let mt = read_or_recover(&self.memtable);
        mt.get_edges(id).map(|e| e.to_vec()).unwrap_or_default()
    }

    pub fn get_incoming_edges(
        &self,
        id: NodeId,
        label: Option<&str>,
    ) -> Vec<crate::node::IncomingEdge> {
        read_or_recover(&self.memtable).get_incoming_edges(id, label)
    }

    pub fn get_all_ids(&self) -> Vec<NodeId> {
        let mt = read_or_recover(&self.memtable);
        mt.get_all_ids()
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
        let config = SearchConfig {
            top_k,
            expand_depth,
            min_score,
            enable_advanced_pipeline: false,
            ..Default::default()
        };
        self.search_hybrid(None, Some(query_vector), &config)
    }

    pub fn search_advanced(
        &self,
        query_vector: &[T],
        config: &SearchConfig,
    ) -> Result<Vec<SearchHit>> {
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
            &self.hook,
            query_text,
            query_vector,
            config,
            &mut ctx,
        )?;
        Ok((results, ctx))
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
            &self.hook,
            query_text,
            query_vector,
            config,
            &mut ctx,
        )
    }

    // ════════════════════════════════════════════════════════
    //  节点查询
    // ════════════════════════════════════════════════════════

    pub fn get(&self, id: NodeId) -> Option<crate::node::NodeView<T>> {
        let mt = read_or_recover(&self.memtable);
        let payload = mt.get_payload(id)?.clone();
        let vector = mt.get_vector(id)?.to_vec();
        let edges = mt.get_edges(id).unwrap_or(&[]).to_vec();
        Some(crate::node::NodeView {
            id,
            vector,
            payload,
            edges,
        })
    }

    pub fn neighbors_with_labels(
        &self,
        id: NodeId,
        depth: usize,
        labels: Option<&[String]>,
    ) -> Vec<NodeId> {
        use std::collections::{HashMap, VecDeque};
        let mt = read_or_recover(&self.memtable);
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
                    if labels
                        .is_some_and(|allowed| !allowed.iter().any(|label| label == &edge.label))
                    {
                        continue;
                    }
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        distances.entry(edge.target_id)
                    {
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
        result.into_iter().map(|(_, node_id)| node_id).collect()
    }

    pub fn neighbors(&self, id: NodeId, depth: usize) -> Vec<NodeId> {
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

    pub fn search_graph_first(
        &self,
        query: &[T],
        anchor_ids: &[NodeId],
        top_k: usize,
        max_anchor_nodes: usize,
    ) -> Result<Vec<SearchHit>> {
        let _operation = self.enter_operation()?;
        crate::graph::constrained::rank_within(
            &read_or_recover(&self.memtable),
            query,
            anchor_ids,
            top_k,
            max_anchor_nodes,
        )
    }

    // ════════════════════════════════════════════════════════
    //  属性二级索引管理
    // ════════════════════════════════════════════════════════

    /// 创建属性索引：对指定 payload 字段建立倒排索引，加速 MATCH/FIND 查询
    ///
    /// ```ignore
    /// db.create_index("name");   // 之后 MATCH (a {name: "Alice"}) 将使用 O(1) 索引
    /// db.create_index("type");   // FIND {type: "event"} 同样受益
    /// ```
    pub fn create_index(&mut self, field: &str) {
        let mut mt = write_or_recover(&self.memtable);
        mt.register_property_index(field);
    }

    /// 删除属性索引
    pub fn drop_index(&mut self, field: &str) {
        let mut mt = write_or_recover(&self.memtable);
        mt.drop_property_index(field);
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
    pub fn tql(&self, input: &str) -> Result<crate::query::tql_executor::TqlResult<T>> {
        let _operation = self.enter_operation()?;
        let query = crate::query::tql_parser::parse_tql(input).map_err(TriviumError::QueryParse)?;
        let mt = read_or_recover(&self.memtable);
        crate::query::tql_executor::execute_tql(&query, &mt)
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
        let _operation = self.enter_operation()?;
        use crate::query::tql_ast::TqlStatement;
        use crate::query::tql_executor::{MutationOp, TqlMutResult};

        let stmt = crate::query::tql_parser::parse_tql_statement(input)
            .map_err(TriviumError::QueryParse)?;

        match stmt {
            TqlStatement::Query(_) => {
                // 读查询降级：执行但不返回数据
                Ok(TqlMutResult {
                    affected: 0,
                    created_ids: Vec::new(),
                })
            }
            TqlStatement::Mutation(mutation) => {
                let (ops, mut next_id) = {
                    let mt = read_or_recover(&self.memtable);
                    (
                        crate::query::tql_executor::execute_tql_mutation(&mutation, &mt)?,
                        mt.next_id_value(),
                    )
                };

                let mut affected = 0usize;
                let mut var_id_map: std::collections::HashMap<String, u64> =
                    std::collections::HashMap::new();
                for op in &ops {
                    if let MutationOp::InsertNode { var, .. } = op {
                        var_id_map.insert(var.clone(), next_id);
                        next_id += 1;
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
                            });
                            affected += 1;
                        }
                        MutationOp::UpdatePayload { id, payload } => {
                            tx_ops.push(transaction::TxOp::UpdatePayload { id, payload });
                            affected += 1;
                        }
                        MutationOp::DeleteNode { id, detach } => {
                            if detach {
                                let edges_to_remove: Vec<(u64, u64)> = {
                                    let mt = read_or_recover(&self.memtable);
                                    let mut edges = Vec::new();
                                    if let Some(out_edges) = mt.get_edges(id) {
                                        for edge in out_edges {
                                            edges.push((id, edge.target_id));
                                        }
                                    }
                                    for &src_id in mt.get_incoming_sources(id) {
                                        edges.push((src_id, id));
                                    }
                                    edges
                                };
                                for (s, d) in edges_to_remove {
                                    tx_ops.push(transaction::TxOp::Unlink { src: s, dst: d });
                                }
                            }
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
        let _operation = self.enter_operation()?;
        self.flush_inner()
    }

    fn flush_inner(&self) -> Result<()> {
        reject_hook_reentrant_write()?;
        {
            let mut mt = write_or_recover(&self.memtable);
            file_format::save(&mut mt, &self.db_path, self.storage_mode)?;
        }
        {
            let mut w = lock_or_recover(&self.wal);
            w.clear()?;
        }
        Ok(())
    }

    pub fn close(&mut self) -> Result<()> {
        reject_hook_reentrant_write()?;
        {
            let mut state = lock_or_recover(&self.lifecycle.0);
            if state.lifecycle != DatabaseLifecycle::Open {
                return Err(TriviumError::DatabaseClosed);
            }
            state.lifecycle = DatabaseLifecycle::Closing;
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

        if let Err(error) = self.flush_inner() {
            let mut state = lock_or_recover(&self.lifecycle.0);
            state.lifecycle = DatabaseLifecycle::Open;
            self.lifecycle.1.notify_all();
            return Err(error);
        }

        self.release_lock();
        let mut state = lock_or_recover(&self.lifecycle.0);
        state.lifecycle = DatabaseLifecycle::Closed;
        self.lifecycle.1.notify_all();
        Ok(())
    }

    /// 仅在 close 成功完成持久化后释放文件锁。
    fn release_lock(&mut self) {
        self._lock_file.take();
    }

    pub fn node_count(&self) -> usize {
        read_or_recover(&self.memtable).node_count()
    }
    pub fn contains(&self, id: NodeId) -> bool {
        read_or_recover(&self.memtable).contains(id)
    }
    pub fn dim(&self) -> usize {
        read_or_recover(&self.memtable).dim()
    }

    /// 获取所有活跃节点的 ID 列表
    pub fn all_node_ids(&self) -> Vec<NodeId> {
        read_or_recover(&self.memtable).all_node_ids()
    }

    // ════════════════════════════════════════════════════════
    //  维度迁移
    // ════════════════════════════════════════════════════════

    /// 维度迁移：从当前数据库导出所有节点和边到一个新维度的数据库。
    pub fn migrate_to(&self, new_path: &str, new_dim: usize) -> Result<(Database<T>, Vec<NodeId>)>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        let mt = read_or_recover(&self.memtable);
        let mut node_ids = mt.all_node_ids();
        node_ids.sort();

        let mut new_db = Database::<T>::open(new_path, new_dim)?;

        let zero_vec = vec![T::zero(); new_dim];
        for &nid in &node_ids {
            if let Some(payload) = mt.get_payload(nid) {
                new_db.insert_with_id(nid, &zero_vec, payload.clone())?;
            }
        }

        for &nid in &node_ids {
            if let Some(edges) = mt.get_edges(nid) {
                for edge in edges {
                    if mt.get_payload(edge.target_id).is_some() {
                        new_db.link(nid, edge.target_id, &edge.label, edge.weight)?;
                    }
                }
            }
        }

        new_db.flush()?;
        tracing::info!(
            "维度迁移完成: {} → {}，共迁移 {} 个节点 (Dimension migration done: {} → {}, {} nodes migrated)",
            mt.dim(),
            new_dim,
            node_ids.len(),
            mt.dim(),
            new_dim,
            node_ids.len()
        );

        Ok((new_db, node_ids))
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
        if let Ok(mut w) = self.wal.lock() {
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
        assert_eq!(db.node_count(), 2);
        assert!(matches!(
            db.insert_with_id(42, &[0.0, 0.0, 1.0], json!({})),
            Err(TriviumError::WalClosed)
        ));
        assert!(!db.contains(42));
        assert!(matches!(
            db.link(second, first, "back", 1.0),
            Err(TriviumError::WalClosed)
        ));
        assert!(db.get_edges(second).is_empty());
        assert!(matches!(
            db.unlink(first, second),
            Err(TriviumError::WalClosed)
        ));
        assert_eq!(db.get_edges(first).len(), 1);
        assert!(matches!(
            db.update_payload(first, json!({"name": "changed"})),
            Err(TriviumError::WalClosed)
        ));
        assert_eq!(db.get_payload(first).unwrap()["name"], "first");
        assert!(matches!(
            db.patch_payload(first, json!({"name": "patched"})),
            Err(TriviumError::WalClosed)
        ));
        assert_eq!(db.get_payload(first).unwrap()["name"], "first");
        assert!(matches!(
            db.update_vector(first, &[9.0, 9.0, 9.0]),
            Err(TriviumError::WalClosed)
        ));
        assert_eq!(db.get(first).unwrap().vector, vec![1.0, 0.0, 0.0]);
        assert!(matches!(db.delete(first), Err(TriviumError::WalClosed)));
        assert!(db.contains(first));
    }
}
