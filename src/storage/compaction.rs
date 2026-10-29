//! 共享定时调度与受资源预算约束的后台压实。
//!
//! 全进程复用一个仅负责计时的线程，实际任务在数据库所属的资源运行时中执行。
//! 注册项仅弱引用数据库；停止会撤销登记并等待已经派发的任务结束，不提前清除 WAL。

use crate::error::{Result, TriviumError};
use crate::resources::ResourceRuntime;
use crate::storage::{file_format, memtable::MemTable, wal::Wal};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenancePhase {
    Scheduled,
    Running,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceDeferReason {
    NotReady,
    ResourceBudget,
    DatabaseBusy,
}

/// 单个维护登记的诊断，不包含其他实例的任务，也不触发资源准入。
#[derive(Debug, Clone, serde::Serialize)]
pub struct MaintenanceStatus {
    pub phase: MaintenancePhase,
    pub deferred_runs: u64,
    pub completed_runs: u64,
    pub failed_runs: u64,
    pub last_defer_reason: Option<MaintenanceDeferReason>,
    pub last_failure: Option<crate::resources::ResourceFailure>,
}

impl Default for MaintenanceStatus {
    fn default() -> Self {
        Self {
            phase: MaintenancePhase::Scheduled,
            deferred_runs: 0,
            completed_runs: 0,
            failed_runs: 0,
            last_defer_reason: None,
            last_failure: None,
        }
    }
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

struct JobState {
    next: Instant,
    running: bool,
    cancelled: bool,
    registered: bool,
    diagnostics: MaintenanceStatus,
}

struct Job {
    runtime: ResourceRuntime,
    interval: Duration,
    state: Mutex<JobState>,
    idle: Condvar,
    operation: Box<dyn Fn() -> Result<bool> + Send + Sync>,
    ready: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    repeat: bool,
    completed: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Job {
    fn status(&self) -> MaintenanceStatus {
        let state = lock(&self.state);
        let mut status = state.diagnostics.clone();
        status.phase = if state.running {
            MaintenancePhase::Running
        } else if state.cancelled {
            MaintenancePhase::Stopped
        } else {
            MaintenancePhase::Scheduled
        };
        status
    }

    fn dispatch(self: &Arc<Self>, now: Instant) {
        {
            let state = lock(&self.state);
            if state.cancelled || state.running || state.next > now {
                return;
            }
        }
        if self.ready.as_ref().is_some_and(|ready| !ready()) {
            let mut state = lock(&self.state);
            state.next = now.checked_add(self.interval).unwrap_or(now);
            state.diagnostics.deferred_runs = state.diagnostics.deferred_runs.saturating_add(1);
            state.diagnostics.last_defer_reason = Some(MaintenanceDeferReason::NotReady);
            return;
        }
        {
            let mut state = lock(&self.state);
            if state.cancelled || state.running || state.next > now {
                return;
            }
            state.next = now.checked_add(self.interval).unwrap_or(now);
            state.running = true;
        }
        let admitted = self
            .runtime
            .enter_maintenance_task()
            .and_then(|task| self.runtime.maintenance_pool().map(|pool| (task, pool)));
        match admitted {
            Ok((task, pool)) => {
                let job = Arc::clone(self);
                let finished = Arc::clone(self);
                pool.spawn_task(
                    task,
                    move || {
                        if lock(&job.state).cancelled {
                            return;
                        }
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            (job.operation)()
                        }))
                        .unwrap_or_else(|_| {
                            Err(TriviumError::ShutdownFailed {
                                reason: "维护任务发生异常展开".into(),
                            })
                        });
                        if !job.repeat {
                            job.request_stop();
                        }
                        match &result {
                            Ok(true) => job.runtime.record_maintenance(true),
                            Err(_) => job.runtime.record_maintenance(false),
                            Ok(false) => {}
                        }
                        {
                            let mut state = lock(&job.state);
                            let diagnostics = &mut state.diagnostics;
                            match &result {
                                Ok(true) => {
                                    diagnostics.completed_runs =
                                        diagnostics.completed_runs.saturating_add(1);
                                    diagnostics.last_defer_reason = None;
                                    diagnostics.last_failure = None;
                                }
                                Ok(false) => {
                                    diagnostics.deferred_runs =
                                        diagnostics.deferred_runs.saturating_add(1);
                                    diagnostics.last_defer_reason =
                                        Some(MaintenanceDeferReason::DatabaseBusy);
                                }
                                Err(error) => {
                                    diagnostics.failed_runs =
                                        diagnostics.failed_runs.saturating_add(1);
                                    diagnostics.last_defer_reason = None;
                                    diagnostics.last_failure =
                                        Some(crate::resources::ResourceFailure::from_error(error));
                                }
                            }
                        }
                        if let Err(error) = result {
                            tracing::error!(%error, "维护任务失败，保留未完成状态以便重试");
                        }
                    },
                    move || {
                        lock(&finished.state).running = false;
                        finished.idle.notify_all();
                        if let Some(completed) = &finished.completed {
                            completed();
                        }
                    },
                );
            }
            Err(error) => {
                let mut state = lock(&self.state);
                state.running = false;
                state.diagnostics.deferred_runs = state.diagnostics.deferred_runs.saturating_add(1);
                state.diagnostics.last_defer_reason =
                    crate::resources::ResourceBudgetDetails::from_error(&error)
                        .map(|_| MaintenanceDeferReason::ResourceBudget);
                state.diagnostics.last_failure =
                    Some(crate::resources::ResourceFailure::from_error(&error));
                drop(state);
                self.idle.notify_all();
                tracing::debug!(%error, "后台压实资源不足，延期到下一周期");
            }
        }
    }

    fn request_stop(&self) {
        let mut state = lock(&self.state);
        state.cancelled = true;
        if state.registered {
            state.registered = false;
            self.runtime.unregister_maintenance();
        }
    }

    fn stop(&self) {
        self.request_stop();
        let mut state = lock(&self.state);
        while state.running {
            state = self
                .idle
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

struct SchedulerState {
    jobs: Vec<Weak<Job>>,
    revision: u64,
}
struct Scheduler {
    state: Mutex<SchedulerState>,
    changed: Condvar,
}

impl Scheduler {
    fn shared() -> Result<Arc<Self>> {
        static SHARED: OnceLock<Mutex<Option<Arc<Scheduler>>>> = OnceLock::new();
        let mut shared = lock(SHARED.get_or_init(|| Mutex::new(None)));
        if let Some(scheduler) = shared.as_ref() {
            return Ok(Arc::clone(scheduler));
        }
        let scheduler = Arc::new(Self {
            state: Mutex::new(SchedulerState {
                jobs: Vec::new(),
                revision: 0,
            }),
            changed: Condvar::new(),
        });
        let worker = Arc::clone(&scheduler);
        std::thread::Builder::new()
            .name("triviumdb-maintenance-clock".into())
            .spawn(move || worker.run())
            .map_err(TriviumError::Io)?;
        *shared = Some(Arc::clone(&scheduler));
        Ok(scheduler)
    }

    fn notify(&self) {
        let mut state = lock(&self.state);
        state.revision = state.revision.wrapping_add(1);
        self.changed.notify_all();
    }

    fn run(&self) {
        loop {
            let (mut jobs, revision) = {
                let mut state = lock(&self.state);
                state.jobs.retain(|job| job.strong_count() > 0);
                (
                    state
                        .jobs
                        .iter()
                        .filter_map(Weak::upgrade)
                        .collect::<Vec<_>>(),
                    state.revision,
                )
            };
            // 排空关闭优先于周期压实，避免短周期维护反复抢占关闭所需的名额。
            jobs.sort_by_key(|job| job.repeat);
            let now = Instant::now();
            let mut delay = Duration::from_secs(3600);
            for job in &jobs {
                job.dispatch(now);
                let state = lock(&job.state);
                if !state.cancelled {
                    delay = delay.min(state.next.saturating_duration_since(Instant::now()));
                }
            }
            // 计时等待期间不能强引用注册项，否则关闭后的运行时无法释放。
            drop(jobs);
            let state = lock(&self.state);
            if state.revision != revision {
                continue;
            }
            let _wait = self
                .changed
                .wait_timeout(state, delay.max(Duration::from_millis(1)))
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

/// 后台压实注册句柄；不再为每个数据库单独创建计时线程。
pub struct CompactionThread {
    job: Arc<Job>,
    scheduler: Arc<Scheduler>,
}

impl CompactionThread {
    pub fn spawn<T: crate::VectorType>(
        interval: Duration,
        memtable: Arc<RwLock<MemTable<T>>>,
        wal: Arc<Mutex<Wal>>,
        db_path: String,
        storage_mode: crate::database::StorageMode,
    ) -> Result<Self> {
        if interval.is_zero() {
            return Err(TriviumError::InvalidInput("后台压实周期必须大于零".into()));
        }
        let next = Instant::now()
            .checked_add(interval)
            .ok_or_else(|| TriviumError::InvalidInput("后台压实周期超出时钟范围".into()))?;
        let runtime = memtable
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .resource_runtime()
            .clone();
        let scheduler = Scheduler::shared()?;
        let memtable = Arc::downgrade(&memtable);
        let wal = Arc::downgrade(&wal);
        let job = Arc::new(Job {
            runtime: runtime.clone(),
            interval,
            state: Mutex::new(JobState {
                next,
                running: false,
                cancelled: false,
                registered: true,
                diagnostics: MaintenanceStatus::default(),
            }),
            idle: Condvar::new(),
            ready: None,
            repeat: true,
            completed: None,
            operation: Box::new(move || {
                let (Some(memtable), Some(wal)) = (memtable.upgrade(), wal.upgrade()) else {
                    return Ok(false);
                };
                let mut mt = match memtable.try_write() {
                    Ok(guard) => guard,
                    Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
                    Err(std::sync::TryLockError::WouldBlock) => return Ok(false),
                };
                #[cfg(feature = "test-hooks")]
                crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeCompactionSave);
                let mut wal = lock(&wal);
                wal.ensure_writable()?;
                if !mt.needs_checkpoint() {
                    return Ok(true);
                }
                if let Err(error) = file_format::save(&mut mt, &db_path, storage_mode) {
                    if mt.ensure_readable().is_err() {
                        wal.release_writer();
                    }
                    return Err(error);
                }
                #[cfg(feature = "test-hooks")]
                crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeWalClear);
                wal.clear()?;
                mt.mark_checkpoint_clean();
                mt.advise_cold_vectors_dontneed();
                Ok(true)
            }),
        });
        runtime.register_maintenance();
        lock(&scheduler.state).jobs.push(Arc::downgrade(&job));
        scheduler.notify();
        Ok(Self { job, scheduler })
    }

    pub(crate) fn request_stop(&self) {
        self.job.request_stop();
        self.scheduler.notify();
    }

    pub(crate) fn is_running(&self) -> bool {
        lock(&self.job.state).running
    }

    pub fn status(&self) -> MaintenanceStatus {
        self.job.status()
    }

    pub fn stop(&mut self) {
        self.job.stop();
        self.scheduler.notify();
    }
}

impl Drop for CompactionThread {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 单次维护登记。撤销不阻塞；派发后的任务自行持有完成操作所需的全部对象。
pub(crate) struct ScheduledMaintenance {
    job: Arc<Job>,
    scheduler: Arc<Scheduler>,
}

impl ScheduledMaintenance {
    pub(crate) fn schedule(
        runtime: ResourceRuntime,
        ready: impl Fn() -> bool + Send + Sync + 'static,
        operation: impl Fn() -> Result<bool> + Send + Sync + 'static,
        completed: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        let scheduler = Scheduler::shared()?;
        let job = Arc::new(Job {
            runtime: runtime.clone(),
            interval: Duration::from_millis(2),
            state: Mutex::new(JobState {
                next: Instant::now(),
                running: false,
                cancelled: false,
                registered: true,
                diagnostics: MaintenanceStatus::default(),
            }),
            idle: Condvar::new(),
            ready: Some(Box::new(ready)),
            repeat: false,
            completed: Some(Box::new(completed)),
            operation: Box::new(operation),
        });
        runtime.register_maintenance();
        lock(&scheduler.state).jobs.push(Arc::downgrade(&job));
        scheduler.notify();
        Ok(Self { job, scheduler })
    }

    pub(crate) fn status(&self) -> MaintenanceStatus {
        self.job.status()
    }
}

impl Drop for ScheduledMaintenance {
    fn drop(&mut self) {
        self.job.request_stop();
        self.scheduler.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[test]
    fn 维护诊断区分排空延期资源拒绝锁忙失败和恢复() {
        let runtime = ResourceRuntime::new(crate::resources::ResourceLimits {
            max_threads: 1,
            ..Default::default()
        })
        .unwrap();
        let ready = Arc::new(AtomicBool::new(false));
        let ready_check = ready.clone();
        let mode = Arc::new(AtomicUsize::new(0));
        let operation_mode = mode.clone();
        let (sent, received) = std::sync::mpsc::channel();
        let job = Arc::new(Job {
            runtime: runtime.clone(),
            interval: Duration::from_millis(2),
            state: Mutex::new(JobState {
                next: Instant::now(),
                running: false,
                cancelled: false,
                registered: false,
                diagnostics: MaintenanceStatus::default(),
            }),
            idle: Condvar::new(),
            repeat: true,
            ready: Some(Box::new(move || ready_check.load(Ordering::SeqCst))),
            completed: Some(Box::new(move || {
                sent.send(()).unwrap();
            })),
            operation: Box::new(move || match operation_mode.load(Ordering::SeqCst) {
                0 => Ok(true),
                1 => Ok(false),
                _ => Err(TriviumError::Io(std::io::Error::other("模拟维护失败"))),
            }),
        });
        job.dispatch(Instant::now());
        assert_eq!(
            job.status().last_defer_reason,
            Some(MaintenanceDeferReason::NotReady)
        );
        assert_eq!(runtime.stats().active_maintenance_tasks, 0);
        ready.store(true, Ordering::SeqCst);
        let mut occupied = runtime.enter_maintenance_task().unwrap();
        occupied.detach();
        lock(&job.state).next = Instant::now();
        job.dispatch(Instant::now());
        let status = job.status();
        assert_eq!(status.deferred_runs, 2);
        assert_eq!(
            status.last_defer_reason,
            Some(MaintenanceDeferReason::ResourceBudget)
        );
        let failure = status.last_failure.unwrap();
        assert_eq!(failure.kind, "resource_budget");
        assert_eq!(failure.budget.unwrap().resource, "maintenance_tasks");
        drop(occupied);
        for outcome in [1, 2, 0] {
            mode.store(outcome, Ordering::SeqCst);
            lock(&job.state).next = Instant::now();
            job.dispatch(Instant::now());
            received.recv_timeout(Duration::from_secs(10)).unwrap();
            let status = job.status();
            assert_eq!(status.phase, MaintenancePhase::Scheduled);
            match outcome {
                1 => assert_eq!(
                    status.last_defer_reason,
                    Some(MaintenanceDeferReason::DatabaseBusy)
                ),
                2 => {
                    assert_eq!(status.failed_runs, 1);
                    assert_eq!(status.last_failure.unwrap().kind, "io");
                }
                _ => {
                    assert_eq!(status.completed_runs, 1);
                    assert!(status.last_failure.is_none());
                    assert!(status.last_defer_reason.is_none());
                }
            }
            assert_eq!(runtime.stats().active_maintenance_tasks, 0);
        }
        job.stop();
        assert_eq!(job.status().phase, MaintenancePhase::Stopped);
    }
}
