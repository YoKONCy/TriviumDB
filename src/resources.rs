//! 多个数据库实例共享的资源准入、工作线程和诊断。
//!
//! 内存按显式数据库配额和临时工作区预留，不以 RSS、缓存命中率或瞬时系统负载改变
//! 查询语义。工作线程在运行时内统一计数；闲置线程池回收完成后才创建替代线程池。

use crate::error::{Result, TriviumError};
use rayon::ThreadPool;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::{JoinHandle, ThreadId};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

/// 共享运行时的显式资源上限。内存与活动任务的零值表示不限。
#[derive(Debug, Clone, Copy, Default)]
pub struct ResourceLimits {
    pub memory_limit: usize,
    /// 从总预算中保护的维护工作区；零值在有限总预算下自动保护八分之一。
    pub maintenance_memory_limit: usize,
    /// 零值按当前可用并行度选择，最多 64 个工作线程。
    pub max_threads: usize,
    pub max_active_tasks: usize,
}

/// 配额与任务是可执行的准入约束；进程 RSS 不属于本统计的内存承诺。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResourceStats {
    pub memory_limit: usize,
    pub maintenance_memory_limit: usize,
    pub reserved_maintenance_bytes: usize,
    pub active_maintenance_tasks: usize,
    pub reserved_database_bytes: usize,
    pub reserved_workspace_bytes: usize,
    pub peak_reserved_bytes: usize,
    pub attached_databases: usize,
    pub max_threads: usize,
    pub allocated_worker_threads: usize,
    pub live_worker_threads: usize,
    pub peak_worker_threads: usize,
    pub max_active_tasks: usize,
    pub active_tasks: usize,
    pub peak_active_tasks: usize,
    pub rejected_memory_requests: u64,
    pub rejected_task_requests: u64,
    pub rejected_thread_requests: u64,
    pub maintenance_jobs: usize,
    pub maintenance_completed: u64,
    pub maintenance_failed: u64,
    /// 当前账本可准入的前台字节；None 表示未设置共享内存上限。
    pub available_foreground_bytes: Option<usize>,
    /// 当前账本可准入的维护字节；与前台余额共享总预算，不能相加。
    pub available_maintenance_bytes: Option<usize>,
    pub available_task_slots: Option<usize>,
}

/// 一次预算拒绝的结构化字段，单位由 resource 标识。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ResourceBudgetDetails {
    pub resource: &'static str,
    pub requested: usize,
    pub used: usize,
    pub limit: usize,
}

impl ResourceBudgetDetails {
    pub(crate) fn from_error(error: &TriviumError) -> Option<Self> {
        match error {
            TriviumError::ResourceBudgetExceeded {
                resource,
                requested,
                used,
                limit,
            } => Some(Self {
                resource,
                requested: *requested,
                used: *used,
                limit: *limit,
            }),
            _ => None,
        }
    }
}

/// 后台故障的可序列化诊断；宿主按 kind 和 budget 分类，不解析 message。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResourceFailure {
    pub kind: &'static str,
    pub message: String,
    pub budget: Option<ResourceBudgetDetails>,
}

impl ResourceFailure {
    pub(crate) fn from_error(error: &TriviumError) -> Self {
        let kind = match error {
            TriviumError::ResourceBudgetExceeded { .. } => "resource_budget",
            TriviumError::Io(_) => "io",
            TriviumError::CapacityAllocationFailed { .. } => "capacity",
            TriviumError::DatabaseClosed => "database_closed",
            TriviumError::ShutdownFailed { .. } => "shutdown",
            _ => "other",
        };
        Self {
            kind,
            message: error.to_string(),
            budget: ResourceBudgetDetails::from_error(error),
        }
    }
}

#[derive(Default)]
struct State {
    database_bytes: usize,
    workspace_bytes: usize,
    maintenance_bytes: usize,
    maintenance_tasks: HashSet<u64>,
    peak_bytes: usize,
    databases: usize,
    tasks: HashMap<u64, usize>,
    bindings: HashMap<ThreadId, Vec<u64>>,
    next_task: u64,
    peak_tasks: usize,
    rejected_memory: u64,
    rejected_tasks: u64,
    rejected_threads: u64,
    maintenance_jobs: usize,
    maintenance_completed: u64,
    maintenance_failed: u64,
    pools: Vec<PoolEntry>,
}

#[derive(Default)]
struct WorkerCounts {
    live: AtomicUsize,
    peak: AtomicUsize,
}

struct WorkerExit(Arc<WorkerCounts>);

impl Drop for WorkerExit {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::SeqCst);
    }
}

struct PoolEntry {
    threads: usize,
    pool: Arc<ThreadPool>,
    joins: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl PoolEntry {
    fn retire(self) {
        drop(self.pool);
        for join in lock(&self.joins).drain(..) {
            let _ = join.join();
        }
    }
}

struct Inner {
    limits: ResourceLimits,
    state: Mutex<State>,
    workers: Arc<WorkerCounts>,
}

/// 可克隆的共享资源句柄；克隆只共享同一组额度，不建立第二套预算。
#[derive(Clone)]
pub struct ResourceRuntime(Arc<Inner>);

impl std::fmt::Debug for ResourceRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResourceRuntime")
            .field("stats", &self.stats())
            .finish()
    }
}

impl ResourceRuntime {
    pub fn new(mut limits: ResourceLimits) -> Result<Self> {
        if limits.max_threads > 64 {
            return Err(TriviumError::InvalidInput(
                "运行时工作线程上限不得超过 64".into(),
            ));
        }
        if limits.memory_limit > 0 {
            if limits.maintenance_memory_limit == 0 {
                limits.maintenance_memory_limit = (limits.memory_limit / 8).max(1);
            }
            if limits.maintenance_memory_limit > limits.memory_limit {
                return Err(TriviumError::InvalidInput(
                    "维护保护额度不能超过运行时总内存额度".into(),
                ));
            }
        }
        if limits.max_threads == 0 {
            limits.max_threads = std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(1)
                .min(64);
        }
        Ok(Self(Arc::new(Inner {
            limits,
            state: Mutex::new(State::default()),
            workers: Arc::new(WorkerCounts::default()),
        })))
    }

    pub(crate) fn shared_default() -> Self {
        static DEFAULT: OnceLock<ResourceRuntime> = OnceLock::new();
        DEFAULT
            .get_or_init(|| {
                Self(Arc::new(Inner {
                    // 保留旧入口允许不同查询并发度的行为，同时约束所有缓存池的总线程数。
                    limits: ResourceLimits {
                        memory_limit: 0,
                        max_threads: 64,
                        max_active_tasks: 0,
                        maintenance_memory_limit: 0,
                    },
                    state: Mutex::new(State::default()),
                    workers: Arc::new(WorkerCounts::default()),
                }))
            })
            .clone()
    }

    pub fn stats(&self) -> ResourceStats {
        let state = lock(&self.0.state);
        let foreground = state.database_bytes.saturating_add(state.workspace_bytes);
        let active_tasks = state.tasks.len() - state.maintenance_tasks.len();
        let memory_limit = self.0.limits.memory_limit;
        ResourceStats {
            memory_limit: self.0.limits.memory_limit,
            maintenance_memory_limit: self.0.limits.maintenance_memory_limit,
            reserved_maintenance_bytes: state.maintenance_bytes,
            active_maintenance_tasks: state.maintenance_tasks.len(),
            reserved_database_bytes: state.database_bytes,
            reserved_workspace_bytes: state.workspace_bytes,
            peak_reserved_bytes: state.peak_bytes,
            attached_databases: state.databases,
            max_threads: self.0.limits.max_threads,
            allocated_worker_threads: state.pools.iter().map(|entry| entry.threads).sum(),
            live_worker_threads: self.0.workers.live.load(Ordering::SeqCst),
            peak_worker_threads: self.0.workers.peak.load(Ordering::SeqCst),
            max_active_tasks: self.0.limits.max_active_tasks,
            active_tasks: state.tasks.len() - state.maintenance_tasks.len(),
            peak_active_tasks: state.peak_tasks,
            rejected_memory_requests: state.rejected_memory,
            rejected_task_requests: state.rejected_tasks,
            rejected_thread_requests: state.rejected_threads,
            maintenance_jobs: state.maintenance_jobs,
            maintenance_completed: state.maintenance_completed,
            maintenance_failed: state.maintenance_failed,
            available_foreground_bytes: (memory_limit > 0).then(|| {
                memory_limit
                    .saturating_sub(
                        self.0
                            .limits
                            .maintenance_memory_limit
                            .max(state.maintenance_bytes),
                    )
                    .saturating_sub(foreground)
            }),
            available_maintenance_bytes: (memory_limit > 0).then(|| {
                memory_limit
                    .saturating_sub(foreground)
                    .saturating_sub(state.maintenance_bytes)
            }),
            available_task_slots: (self.0.limits.max_active_tasks > 0)
                .then(|| self.0.limits.max_active_tasks.saturating_sub(active_tasks)),
        }
    }

    pub(crate) fn max_threads(&self) -> usize {
        self.0.limits.max_threads
    }

    pub(crate) fn parallelism(&self) -> usize {
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1)
            .min(self.max_threads())
    }

    pub(crate) fn register_maintenance(&self) {
        lock(&self.0.state).maintenance_jobs += 1;
    }

    pub(crate) fn unregister_maintenance(&self) {
        lock(&self.0.state).maintenance_jobs -= 1;
    }

    pub(crate) fn record_maintenance(&self, success: bool) {
        let mut state = lock(&self.0.state);
        if success {
            state.maintenance_completed = state.maintenance_completed.saturating_add(1);
        } else {
            state.maintenance_failed = state.maintenance_failed.saturating_add(1);
        }
    }

    fn check_memory(&self, state: &mut State, additional: usize) -> Result<()> {
        let used = state
            .database_bytes
            .checked_add(state.workspace_bytes)
            .ok_or_else(|| TriviumError::CapacityAllocationFailed {
                reason: "共享资源计数溢出".into(),
            })?;
        let projected =
            used.checked_add(additional)
                .ok_or_else(|| TriviumError::CapacityAllocationFailed {
                    reason: "共享资源预留溢出".into(),
                })?;
        let limit = self.0.limits.memory_limit.saturating_sub(
            self.0
                .limits
                .maintenance_memory_limit
                .max(state.maintenance_bytes),
        );
        if self.0.limits.memory_limit > 0 && projected > limit {
            state.rejected_memory = state.rejected_memory.saturating_add(1);
            return Err(TriviumError::ResourceBudgetExceeded {
                resource: "memory_bytes",
                requested: additional,
                used,
                limit,
            });
        }
        let total = projected
            .checked_add(state.maintenance_bytes)
            .ok_or_else(|| TriviumError::CapacityAllocationFailed {
                reason: "共享总预留计数溢出".into(),
            })?;
        state.peak_bytes = state.peak_bytes.max(total);
        Ok(())
    }

    pub(crate) fn reserve_database(&self, bytes: usize) -> Result<DatabaseReservation> {
        if self.0.limits.memory_limit > 0 && bytes == 0 {
            return Err(TriviumError::InvalidInput(
                "有共享内存上限时，数据库必须指定非零 memory_limit".into(),
            ));
        }
        let mut state = lock(&self.0.state);
        self.check_memory(&mut state, bytes)?;
        let databases = state.databases.checked_add(1).ok_or_else(|| {
            TriviumError::CapacityAllocationFailed {
                reason: "数据库资源登记数量溢出".into(),
            }
        })?;
        state.database_bytes += bytes;
        state.databases = databases;
        Ok(DatabaseReservation {
            runtime: self.clone(),
            bytes: AtomicUsize::new(bytes),
        })
    }

    pub(crate) fn reserve_workspace(&self, bytes: usize) -> Result<WorkspaceReservation> {
        let mut state = lock(&self.0.state);
        self.check_memory(&mut state, bytes)?;
        state.workspace_bytes += bytes;
        Ok(WorkspaceReservation {
            runtime: self.clone(),
            bytes,
            maintenance: false,
        })
    }

    pub(crate) fn reserve_maintenance_workspace(
        &self,
        bytes: usize,
    ) -> Result<WorkspaceReservation> {
        let mut state = lock(&self.0.state);
        let used = state
            .database_bytes
            .checked_add(state.workspace_bytes)
            .and_then(|n| n.checked_add(state.maintenance_bytes))
            .ok_or_else(|| TriviumError::CapacityAllocationFailed {
                reason: "维护资源计数溢出".into(),
            })?;
        let projected =
            used.checked_add(bytes)
                .ok_or_else(|| TriviumError::CapacityAllocationFailed {
                    reason: "维护工作区预留溢出".into(),
                })?;
        if self.0.limits.memory_limit > 0 && projected > self.0.limits.memory_limit {
            state.rejected_memory = state.rejected_memory.saturating_add(1);
            return Err(TriviumError::ResourceBudgetExceeded {
                resource: "maintenance_memory_bytes",
                requested: bytes,
                used,
                limit: self.0.limits.memory_limit,
            });
        }
        state.maintenance_bytes += bytes;
        state.peak_bytes = state.peak_bytes.max(projected);
        Ok(WorkspaceReservation {
            runtime: self.clone(),
            bytes,
            maintenance: true,
        })
    }

    pub(crate) fn is_maintenance_task(&self) -> bool {
        let state = lock(&self.0.state);
        state
            .bindings
            .get(&std::thread::current().id())
            .and_then(|s| s.last())
            .is_some_and(|id| state.maintenance_tasks.contains(id))
    }

    pub(crate) fn enter_task(&self) -> Result<TaskLease> {
        self.enter_task_kind(false)
    }

    pub(crate) fn enter_maintenance_task(&self) -> Result<TaskLease> {
        self.enter_task_kind(true)
    }

    fn enter_task_kind(&self, maintenance: bool) -> Result<TaskLease> {
        let owner = std::thread::current().id();
        let mut state = lock(&self.0.state);
        let inherited = state
            .bindings
            .get(&owner)
            .and_then(|s| s.last())
            .copied()
            .filter(|id| !maintenance || state.maintenance_tasks.contains(id));
        let id = if let Some(id) = inherited {
            let refs = state
                .tasks
                .get_mut(&id)
                .ok_or_else(|| TriviumError::InvalidInput("资源任务继承状态无效".into()))?;
            *refs = refs
                .checked_add(1)
                .ok_or_else(|| TriviumError::CapacityAllocationFailed {
                    reason: "任务引用计数溢出".into(),
                })?;
            id
        } else {
            let used = if maintenance {
                state.maintenance_tasks.len()
            } else {
                state.tasks.len() - state.maintenance_tasks.len()
            };
            let limit = if maintenance {
                self.max_threads()
            } else {
                self.0.limits.max_active_tasks
            };
            if limit > 0 && used >= limit {
                state.rejected_tasks = state.rejected_tasks.saturating_add(1);
                return Err(TriviumError::ResourceBudgetExceeded {
                    resource: if maintenance {
                        "maintenance_tasks"
                    } else {
                        "active_tasks"
                    },
                    requested: 1,
                    used,
                    limit,
                });
            }
            state.next_task = state.next_task.checked_add(1).ok_or_else(|| {
                TriviumError::CapacityAllocationFailed {
                    reason: "资源任务编号溢出".into(),
                }
            })?;
            let id = state.next_task;
            state.tasks.insert(id, 1);
            if maintenance {
                state.maintenance_tasks.insert(id);
            }
            state.peak_tasks = state
                .peak_tasks
                .max(state.tasks.len() - state.maintenance_tasks.len());
            id
        };
        state.bindings.entry(owner).or_default().push(id);
        Ok(TaskLease {
            runtime: self.clone(),
            id,
            owner: Some(owner),
        })
    }

    pub(crate) fn maintenance_pool(&self) -> Result<ResourcePool> {
        if let Some(pool) = lock(&self.0.state).pools.first() {
            return Ok(ResourcePool {
                runtime: self.clone(),
                pool: Arc::clone(&pool.pool),
            });
        }
        self.pool(1)
    }

    pub(crate) fn current_task(&self) -> Option<u64> {
        lock(&self.0.state)
            .bindings
            .get(&std::thread::current().id())
            .and_then(|stack| stack.last())
            .copied()
    }

    pub(crate) fn bind_task(&self, id: Option<u64>) -> TaskBinding {
        let owner = std::thread::current().id();
        if let Some(id) = id {
            lock(&self.0.state)
                .bindings
                .entry(owner)
                .or_default()
                .push(id);
        }
        TaskBinding {
            runtime: self.clone(),
            id,
            owner,
        }
    }

    /// 仅回收没有调用方持有的池；资源不足立即返回，不在数据库锁内等待其他任务。
    pub(crate) fn pool(&self, requested: usize) -> Result<ResourcePool> {
        let threads = requested.clamp(1, self.max_threads());
        let mut state = lock(&self.0.state);
        // 同一运行时的嵌套工作可以复用更小的当前池，保证不会扩张其线程上限。
        if let Some(entry) = state
            .pools
            .iter()
            .find(|entry| entry.threads <= threads && entry.pool.current_thread_index().is_some())
            .or_else(|| state.pools.iter().find(|entry| entry.threads == threads))
        {
            return Ok(ResourcePool {
                runtime: self.clone(),
                pool: Arc::clone(&entry.pool),
            });
        }
        let mut allocated: usize = state.pools.iter().map(|entry| entry.threads).sum();
        while allocated.saturating_add(threads) > self.max_threads() {
            let Some(index) = state
                .pools
                .iter()
                .position(|entry| Arc::strong_count(&entry.pool) == 1)
            else {
                state.rejected_threads = state.rejected_threads.saturating_add(1);
                return Err(TriviumError::ResourceBudgetExceeded {
                    resource: "worker_threads",
                    requested: threads,
                    used: allocated,
                    limit: self.max_threads(),
                });
            };
            let entry = state.pools.remove(index);
            allocated -= entry.threads;
            // join 不获取资源状态锁，确保旧线程退出后才释放可供新池使用的线程额度。
            entry.retire();
        }
        let joins = Arc::new(Mutex::new(Vec::new()));
        let thread_joins = Arc::clone(&joins);
        let workers = Arc::clone(&self.0.workers);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(move |index| format!("triviumdb-resource-{threads}-{index}"))
            .spawn_handler(move |thread| {
                let mut builder = std::thread::Builder::new();
                if let Some(name) = thread.name() {
                    builder = builder.name(name.to_owned());
                }
                if let Some(size) = thread.stack_size() {
                    builder = builder.stack_size(size);
                }
                let workers = Arc::clone(&workers);
                let join = builder.spawn(move || {
                    let live = workers.live.fetch_add(1, Ordering::SeqCst) + 1;
                    workers.peak.fetch_max(live, Ordering::SeqCst);
                    let _exit = WorkerExit(workers);
                    thread.run();
                })?;
                lock(&thread_joins).push(join);
                Ok(())
            })
            .build();
        let pool = match pool {
            Ok(pool) => Arc::new(pool),
            Err(error) => {
                for join in lock(&joins).drain(..) {
                    let _ = join.join();
                }
                return Err(TriviumError::CapacityAllocationFailed {
                    reason: format!("资源线程池创建失败：{error}"),
                });
            }
        };
        state.pools.push(PoolEntry {
            threads,
            pool: Arc::clone(&pool),
            joins,
        });
        Ok(ResourcePool {
            runtime: self.clone(),
            pool,
        })
    }
}

pub(crate) struct DatabaseReservation {
    runtime: ResourceRuntime,
    bytes: AtomicUsize,
}

impl DatabaseReservation {
    pub(crate) fn bytes(&self) -> usize {
        self.bytes.load(Ordering::Relaxed)
    }

    pub(crate) fn resize(&self, bytes: usize) -> Result<()> {
        if self.runtime.0.limits.memory_limit > 0 && bytes == 0 {
            return Err(TriviumError::InvalidInput(
                "共享内存受限时不能取消数据库配额".into(),
            ));
        }
        let mut state = lock(&self.runtime.0.state);
        let previous = self.bytes.load(Ordering::Relaxed);
        if bytes > previous {
            self.runtime.check_memory(&mut state, bytes - previous)?;
        }
        state.database_bytes = state.database_bytes - previous + bytes;
        self.bytes.store(bytes, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for DatabaseReservation {
    fn drop(&mut self) {
        let mut state = lock(&self.runtime.0.state);
        state.database_bytes -= self.bytes.load(Ordering::Relaxed);
        state.databases -= 1;
    }
}

pub(crate) struct WorkspaceReservation {
    runtime: ResourceRuntime,
    bytes: usize,
    maintenance: bool,
}

impl WorkspaceReservation {
    /// 扩容先准入，再把新增预留转入当前凭证；缩容和异常退出均准确归还账本。
    pub(crate) fn resize(&mut self, bytes: usize) -> Result<()> {
        if bytes > self.bytes {
            let extra = bytes - self.bytes;
            let mut addition = if self.maintenance {
                self.runtime.reserve_maintenance_workspace(extra)?
            } else {
                self.runtime.reserve_workspace(extra)?
            };
            self.bytes = bytes;
            addition.bytes = 0;
        } else if bytes < self.bytes {
            let mut state = lock(&self.runtime.0.state);
            if self.maintenance {
                state.maintenance_bytes -= self.bytes - bytes;
            } else {
                state.workspace_bytes -= self.bytes - bytes;
            }
            self.bytes = bytes;
        }
        Ok(())
    }
}

impl Drop for WorkspaceReservation {
    fn drop(&mut self) {
        let mut state = lock(&self.runtime.0.state);
        if self.maintenance {
            state.maintenance_bytes -= self.bytes;
        } else {
            state.workspace_bytes -= self.bytes;
        }
    }
}

fn unbind(state: &mut State, owner: ThreadId, id: u64) {
    if let Some(stack) = state.bindings.get_mut(&owner) {
        if let Some(index) = stack.iter().rposition(|candidate| *candidate == id) {
            stack.remove(index);
        }
        if stack.is_empty() {
            state.bindings.remove(&owner);
        }
    }
}

pub(crate) struct TaskLease {
    runtime: ResourceRuntime,
    id: u64,
    owner: Option<ThreadId>,
}

impl TaskLease {
    pub(crate) fn detach(&mut self) {
        if let Some(owner) = self.owner.take() {
            unbind(&mut lock(&self.runtime.0.state), owner, self.id);
        }
    }
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for TaskLease {
    fn drop(&mut self) {
        self.detach();
        let mut state = lock(&self.runtime.0.state);
        if let Some(refs) = state.tasks.get_mut(&self.id) {
            *refs -= 1;
            if *refs == 0 {
                state.tasks.remove(&self.id);
                state.maintenance_tasks.remove(&self.id);
            }
        }
    }
}

pub(crate) struct TaskBinding {
    runtime: ResourceRuntime,
    id: Option<u64>,
    owner: ThreadId,
}

impl Drop for TaskBinding {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            unbind(&mut lock(&self.runtime.0.state), self.owner, id);
        }
    }
}

pub(crate) struct ResourcePool {
    runtime: ResourceRuntime,
    pool: Arc<ThreadPool>,
}

impl ResourcePool {
    pub(crate) fn spawn_task(
        &self,
        mut task: TaskLease,
        operation: impl FnOnce() + Send + 'static,
        completed: impl FnOnce() + Send + 'static,
    ) {
        task.detach();
        let runtime = self.runtime.clone();
        let keep_alive = Arc::clone(&self.pool);
        self.pool.spawn(move || {
            let _pool = keep_alive;
            {
                let _binding = runtime.bind_task(Some(task.id()));
                operation();
            }
            drop(task);
            completed();
        });
    }
    pub(crate) fn install<R: Send>(&self, operation: impl FnOnce() -> R + Send) -> R {
        self.install_task(self.runtime.current_task(), operation)
    }
    pub(crate) fn install_task<R: Send>(
        &self,
        task: Option<u64>,
        operation: impl FnOnce() -> R + Send,
    ) -> R {
        self.pool.install(|| {
            let _binding = self.runtime.bind_task(task);
            operation()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;

    #[test]
    fn 工作区扩缩容拒绝与释放准确维护两类账本() {
        let runtime = ResourceRuntime::new(ResourceLimits {
            memory_limit: 1024,
            maintenance_memory_limit: 256,
            max_threads: 1,
            ..Default::default()
        })
        .unwrap();
        let mut foreground = runtime.reserve_workspace(512).unwrap();
        assert!(foreground.resize(800).is_err());
        assert_eq!(runtime.stats().reserved_workspace_bytes, 512);
        foreground.resize(128).unwrap();
        let mut maintenance = runtime.reserve_maintenance_workspace(512).unwrap();
        foreground.resize(300).unwrap();
        assert_eq!(runtime.stats().reserved_workspace_bytes, 300);
        assert!(foreground.resize(600).is_err());
        assert_eq!(runtime.stats().reserved_workspace_bytes, 300);
        assert!(maintenance.resize(800).is_err());
        assert_eq!(runtime.stats().reserved_maintenance_bytes, 512);
        maintenance.resize(128).unwrap();
        drop(maintenance);
        drop(foreground);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        assert_eq!(runtime.stats().reserved_maintenance_bytes, 0);
    }

    #[test]
    fn 前台用满额度仍保留维护工作区与独立任务名额() {
        let runtime = ResourceRuntime::new(ResourceLimits {
            memory_limit: 512,
            maintenance_memory_limit: 64,
            max_threads: 1,
            max_active_tasks: 1,
        })
        .unwrap();
        let _database = runtime.reserve_database(448).unwrap();
        assert_eq!(runtime.stats().available_foreground_bytes, Some(0));
        assert_eq!(runtime.stats().available_maintenance_bytes, Some(64));
        let _foreground = runtime.enter_task().unwrap();
        assert_eq!(runtime.stats().available_task_slots, Some(0));
        assert!(runtime.reserve_workspace(1).is_err());
        let maintenance = runtime.enter_maintenance_task().unwrap();
        let workspace = runtime.reserve_maintenance_workspace(64).unwrap();
        assert!(runtime.reserve_maintenance_workspace(1).is_err());
        assert_eq!(runtime.stats().active_tasks, 1);
        assert_eq!(runtime.stats().active_maintenance_tasks, 1);
        assert_eq!(runtime.stats().peak_reserved_bytes, 512);
        assert_eq!(runtime.stats().available_maintenance_bytes, Some(0));
        drop(workspace);
        drop(maintenance);
        assert_eq!(runtime.stats().reserved_maintenance_bytes, 0);
        assert_eq!(runtime.stats().active_maintenance_tasks, 0);
        assert_eq!(runtime.stats().available_maintenance_bytes, Some(64));
    }

    #[test]
    fn 无上限余额使用空值而不是伪造零余额() {
        let runtime = ResourceRuntime::new(ResourceLimits::default()).unwrap();
        let stats = runtime.stats();
        assert_eq!(stats.available_foreground_bytes, None);
        assert_eq!(stats.available_maintenance_bytes, None);
        assert_eq!(stats.available_task_slots, None);
    }

    #[test]
    fn 配额按独立账本核对且失败不消耗额度() {
        let runtime = ResourceRuntime::new(ResourceLimits {
            memory_limit: 512,
            max_threads: 2,
            max_active_tasks: 0,
            maintenance_memory_limit: 0,
        })
        .unwrap();
        let first = runtime.reserve_database(128).unwrap();
        let second = runtime.reserve_database(192).unwrap();
        let workspace = runtime.reserve_workspace(64).unwrap();
        assert_eq!(
            runtime.stats().reserved_database_bytes + runtime.stats().reserved_workspace_bytes,
            384
        );
        assert!(matches!(
            runtime.reserve_database(129),
            Err(TriviumError::ResourceBudgetExceeded {
                resource: "memory_bytes",
                ..
            })
        ));
        assert!(first.resize(257).is_err());
        assert_eq!(runtime.stats().reserved_database_bytes, 320);
        drop(workspace);
        first.resize(256).unwrap();
        assert_eq!(runtime.stats().reserved_database_bytes, 448);
        drop(second);
        drop(first);
        assert_eq!(runtime.stats().reserved_database_bytes, 0);
        assert_eq!(runtime.stats().attached_databases, 0);
    }

    #[test]
    fn 不同并发度的真实工作线程总数受同一上限约束() {
        let runtime = ResourceRuntime::new(ResourceLimits {
            max_threads: 4,
            ..Default::default()
        })
        .unwrap();
        for threads in [1, 2, 3, 4, 1, 4, 2] {
            let pool = runtime.pool(threads).unwrap();
            let sum = pool.install(|| (0..128usize).into_par_iter().sum::<usize>());
            assert_eq!(sum, 127 * 128 / 2);
            let stats = runtime.stats();
            assert!(stats.allocated_worker_threads <= 4);
            assert!(stats.live_worker_threads <= 4);
            assert!(stats.peak_worker_threads <= 4);
        }
    }

    #[test]
    fn 活跃池不足时拒绝新池且不等待数据库锁() {
        let runtime = ResourceRuntime::new(ResourceLimits {
            max_threads: 2,
            ..Default::default()
        })
        .unwrap();
        let active = runtime.pool(2).unwrap();
        assert!(matches!(
            runtime.pool(1),
            Err(TriviumError::ResourceBudgetExceeded {
                resource: "worker_threads",
                ..
            })
        ));
        assert_eq!(active.install(|| 7), 7);
        drop(active);
        runtime.pool(1).unwrap().install(|| ());
        assert_eq!(runtime.stats().rejected_thread_requests, 1);
    }

    #[test]
    fn 嵌套调用和工作线程继承不重复占用任务额度() {
        let runtime = ResourceRuntime::new(ResourceLimits {
            max_threads: 2,
            max_active_tasks: 1,
            maintenance_memory_limit: 0,
            memory_limit: 0,
        })
        .unwrap();
        let root = runtime.enter_task().unwrap();
        let nested = runtime.enter_task().unwrap();
        runtime.pool(2).unwrap().install(|| {
            let _nested = runtime.enter_task().unwrap();
            assert_eq!(runtime.stats().active_tasks, 1);
        });
        let another = runtime.clone();
        assert!(
            std::thread::spawn(move || another.enter_task().is_err())
                .join()
                .unwrap()
        );
        drop(nested);
        drop(root);
        assert_eq!(runtime.stats().active_tasks, 0);
        assert_eq!(runtime.stats().peak_active_tasks, 1);
    }

    #[test]
    fn 跨线程任务移交后原线程不能借用其额度() {
        let runtime = ResourceRuntime::new(ResourceLimits {
            max_threads: 2,
            max_active_tasks: 1,
            maintenance_memory_limit: 0,
            memory_limit: 0,
        })
        .unwrap();
        let mut task = runtime.enter_task().unwrap();
        task.detach();
        assert!(runtime.enter_task().is_err());
        let destination = runtime.clone();
        std::thread::spawn(move || {
            destination
                .pool(2)
                .unwrap()
                .install_task(Some(task.id()), || {
                    let _nested = destination.enter_task().unwrap();
                    assert_eq!(destination.stats().active_tasks, 1);
                });
            drop(task);
        })
        .join()
        .unwrap();
        assert_eq!(runtime.stats().active_tasks, 0);
        assert!(runtime.enter_task().is_ok());
    }

    #[test]
    fn 异常展开归还工作区和任务额度() {
        let runtime = ResourceRuntime::new(ResourceLimits {
            memory_limit: 256,
            max_threads: 1,
            max_active_tasks: 1,
            maintenance_memory_limit: 0,
        })
        .unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _task = runtime.enter_task().unwrap();
            let _workspace = runtime.reserve_workspace(128).unwrap();
            panic!("测试受控异常展开");
        }));
        assert!(result.is_err());
        assert_eq!(runtime.stats().active_tasks, 0);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }
}
