//! 可观测、可重试的关闭协议；超时只结束等待，不撤销正在持久化的任务。

use super::*;
use crate::storage::compaction::ScheduledMaintenance;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownPhase {
    Open,
    Draining,
    Flushing,
    Failed,
    Closed,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ShutdownStatus {
    pub phase: ShutdownPhase,
    pub active_operations: usize,
    pub last_error: Option<String>,
    pub last_failure: Option<crate::resources::ResourceFailure>,
    pub maintenance: Option<crate::storage::compaction::MaintenanceStatus>,
}

/// 本实例的内存估算和准入配额，不含共享运行时中其他实例。
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstanceMemoryStats {
    pub reserved_database_bytes: usize,
    pub estimated_memory_bytes: usize,
    pub node_count: usize,
}

/// 可在过载或关闭期间读取；写锁繁忙时 memory 为 None，不等待刷盘或构图。
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstanceStats {
    pub database_path: String,
    pub memory_limit_bytes: usize,
    pub memory: Option<InstanceMemoryStats>,
    pub shutdown: ShutdownStatus,
    pub compaction: Option<crate::storage::compaction::MaintenanceStatus>,
}

impl<T: VectorType> Database<T> {
    pub(super) fn reject_group_shutdown(&self) -> Result<()> {
        if lock_or_recover(&self.lifecycle.0).group_commit_owner
            == Some(std::thread::current().id())
        {
            return Err(TriviumError::ReentrantAccess {
                operation: "group_commit_shutdown",
            });
        }
        Ok(())
    }

    pub fn instance_stats(&self) -> InstanceStats {
        let shutdown = self.shutdown_status();
        // 不持有生命周期锁去获取 MemTable 锁，避免与完成回调形成锁序环。
        let memory = match self.memtable.try_read() {
            Ok(guard) => Some(guard),
            Err(std::sync::TryLockError::Poisoned(error)) => Some(error.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
        .map(|mt| InstanceMemoryStats {
            reserved_database_bytes: mt.reserved_database_bytes(),
            estimated_memory_bytes: mt.estimated_memory_bytes(),
            node_count: mt.node_count(),
        });
        InstanceStats {
            database_path: self.db_path.clone(),
            memory_limit_bytes: self.memory_limit,
            memory,
            shutdown,
            compaction: self.compaction.as_ref().map(CompactionThread::status),
        }
    }

    pub fn shutdown_status(&self) -> ShutdownStatus {
        let state = lock_or_recover(&self.lifecycle.0);
        ShutdownStatus {
            phase: state.shutdown_phase,
            active_operations: state.active_operations,
            last_error: state.shutdown_last_error.clone(),
            last_failure: state.shutdown_last_failure.clone(),
            maintenance: state
                .shutdown_job
                .as_ref()
                .map(ScheduledMaintenance::status),
        }
    }

    /// 拒绝新操作并登记关闭任务。已有操作和后台维护先排空，完成后才释放文件锁。
    pub fn request_shutdown(&mut self) -> Result<ShutdownStatus> {
        reject_hook_reentrant_access(&self.memtable)?;
        self.reject_group_shutdown()?;
        let mut state = lock_or_recover(&self.lifecycle.0);
        if state.lifecycle == DatabaseLifecycle::Closed
            || state.shutdown_job.is_some()
            || state.shutdown_error.is_some()
        {
            drop(state);
            return Ok(self.shutdown_status());
        }
        let previous = state.lifecycle;
        state.lifecycle = DatabaseLifecycle::Closing;
        state.shutdown_phase = ShutdownPhase::Draining;
        state.shutdown_last_error = None;
        state.shutdown_last_failure = None;
        if let Some(background) = &self.compaction {
            background.request_stop();
        }
        let background = self.compaction.take();
        let work = Arc::new(CloseWork {
            background: Mutex::new(background),
            hook: self.hook.clone(),
            memtable: self.memtable.clone(),
            wal: self.wal.clone(),
            file_lock: self._lock_file.clone(),
            lifecycle: self.lifecycle.clone(),
            path: self.db_path.clone(),
            mode: self.storage_mode,
            access: self.access_mode,
            result: Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            io_failure: crate::test_hooks::current_io_failure(),
        });
        let ready_work = work.clone();
        let completed_work = work.clone();
        let rollback_work = work.clone();
        let scheduled = ScheduledMaintenance::schedule(
            self.resource_runtime.clone(),
            move || {
                lock_or_recover(&ready_work.lifecycle.0).active_operations == 0
                    && !lock_or_recover(&ready_work.background)
                        .as_ref()
                        .is_some_and(|b| b.is_running())
            },
            move || work.execute(),
            move || completed_work.complete(),
        );
        match scheduled {
            Ok(job) => state.shutdown_job = Some(job),
            Err(error) => {
                self.compaction = lock_or_recover(&rollback_work.background).take();
                if let Some(background) = &mut self.compaction {
                    background.resume();
                }
                state.lifecycle = previous;
                state.shutdown_phase = if previous == DatabaseLifecycle::Open {
                    ShutdownPhase::Open
                } else {
                    ShutdownPhase::Failed
                };
                return Err(error);
            }
        }
        drop(state);
        Ok(self.shutdown_status())
    }

    /// 返回 true 表示关闭完成，false 表示等待期限耗尽；false 后任务仍持有数据和锁。
    pub fn close_timeout(&mut self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| TriviumError::InvalidInput("关闭等待期限超出时钟范围".into()))?;
        self.request_shutdown()?;
        self.wait_for_shutdown(Some(deadline))
    }

    pub(super) fn wait_for_shutdown(&self, deadline: Option<Instant>) -> Result<bool> {
        let mut state = lock_or_recover(&self.lifecycle.0);
        loop {
            if let Some(error) = state.shutdown_error.take() {
                return Err(error);
            }
            if state.lifecycle == DatabaseLifecycle::Closed {
                return Ok(true);
            }
            if let Some(deadline) = deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Ok(false);
                }
                state = self
                    .lifecycle
                    .1
                    .wait_timeout(state, remaining)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            } else {
                state = self
                    .lifecycle
                    .1
                    .wait(state)
                    .unwrap_or_else(|e| e.into_inner());
            }
        }
    }
}

struct CloseWork<T: VectorType> {
    hook: HookSlot,
    background: Mutex<Option<CompactionThread>>,
    memtable: Arc<RwLock<MemTable<T>>>,
    wal: Arc<Mutex<Wal>>,
    file_lock: Arc<Mutex<Option<std::fs::File>>>,
    lifecycle: Arc<(Mutex<LifecycleState>, Condvar)>,
    path: String,
    mode: StorageMode,
    access: AccessMode,
    result: Mutex<Option<CloseOutcome>>,
    #[cfg(feature = "test-hooks")]
    io_failure: Option<crate::test_hooks::IoPoint>,
}

struct CloseOutcome {
    resources_released: bool,
    result: Result<()>,
}

impl<T: VectorType> CloseWork<T> {
    fn execute(&self) -> Result<bool> {
        lock_or_recover(&self.lifecycle.0).shutdown_phase = ShutdownPhase::Flushing;
        #[cfg(feature = "test-hooks")]
        let _injection = self.io_failure.map(crate::test_hooks::fail_io_at);
        let mut resources_released = false;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
            let mut mt = write_or_recover(&self.memtable);
            if self.access == AccessMode::ReadWrite {
                let mut wal = lock_or_recover(&self.wal);
                file_format::checkpoint(&mut mt, &mut wal, &self.path, self.mode, true)?;
                wal.close_writer()?;
            }
            mt.release_resource_storage();
            drop(mt);
            let result = clear_hook_for_close(&self.hook);
            lock_or_recover(&self.file_lock).take();
            resources_released = true;
            result
        }))
        .unwrap_or_else(|_| {
            Err(TriviumError::ShutdownFailed {
                reason: "关闭工作发生异常展开 (Shutdown work panicked)".into(),
            })
        });
        let failed = result.is_err();
        *lock_or_recover(&self.result) = Some(CloseOutcome {
            resources_released,
            result,
        });
        if failed {
            Err(TriviumError::ShutdownFailed {
                reason: "关闭发生错误，请检查关闭阶段与故障详情 (Shutdown reported an error; inspect its phase and failure details)".into(),
            })
        } else {
            Ok(true)
        }
    }

    fn complete(&self) {
        let outcome = lock_or_recover(&self.result)
            .take()
            .unwrap_or_else(|| CloseOutcome {
                resources_released: false,
                result: Err(TriviumError::ShutdownFailed {
                    reason: "关闭任务未提供完成结果 (Shutdown task supplied no completion result)"
                        .into(),
                }),
            });
        let mut state = lock_or_recover(&self.lifecycle.0);
        if outcome.resources_released {
            state.lifecycle = DatabaseLifecycle::Closed;
            state.shutdown_phase = ShutdownPhase::Closed;
        }
        match outcome.result {
            Ok(()) => {}
            Err(error) => {
                if !outcome.resources_released {
                    state.shutdown_phase = ShutdownPhase::Failed;
                }
                state.shutdown_last_error = Some(error.to_string());
                state.shutdown_last_failure =
                    Some(crate::resources::ResourceFailure::from_error(&error));
                state.shutdown_error = Some(error);
            }
        }
        state.shutdown_job.take();
        self.lifecycle.1.notify_all();
    }
}
