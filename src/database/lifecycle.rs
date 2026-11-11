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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{ResourceLimits, ResourceRuntime};
    use std::sync::mpsc;

    #[test]
    fn 关闭后待执行查询句柄不保留_hook() {
        struct Probe(Arc<std::sync::atomic::AtomicUsize>);
        impl SearchHook for Probe {}
        impl Drop for Probe {
            fn drop(&mut self) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let (mut db, _) = open();
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        db.set_hook(Probe(dropped.clone())).unwrap();
        let queued = db.search_handle();
        assert!(db.close_timeout(Duration::from_secs(5)).unwrap());
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(matches!(
            queued.search_batch(
                &[vec![1.0; 3]],
                &SearchConfig::default(),
                &BatchSearchConfig::default()
            ),
            Err(TriviumError::DatabaseClosed)
        ));
    }

    fn open() -> (Database<f32>, ResourceRuntime) {
        // Windows 会复用进程号，样本身份需跨运行唯一，避免读到旧测试留下的数据。
        let path = std::env::temp_dir().join(format!("tdb_shutdown_{}.tdb", uuid::Uuid::new_v4()));
        let runtime = ResourceRuntime::new(ResourceLimits {
            memory_limit: 64 * 1024 * 1024,
            maintenance_memory_limit: 8 * 1024 * 1024,
            max_threads: 1,
            max_active_tasks: 1,
        })
        .unwrap();
        let mut db = Database::open_with_runtime(
            &path.to_string_lossy(),
            Config {
                dim: 3,
                memory_limit: 8 * 1024 * 1024,
                auto_build_quiver: false,
                ..Default::default()
            },
            runtime.clone(),
        )
        .unwrap();
        db.insert_with_id(1, &[1.0, 0.0, 0.0], serde_json::json!({"state": "saved"}))
            .unwrap();
        (db, runtime)
    }

    struct Gate {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    #[test]
    fn 实例诊断在写锁占用时返回未知内存且不消耗准入名额() {
        let (mut db, runtime) = open();
        let task = runtime.enter_task().unwrap();
        let mt = write_or_recover(&db.memtable);
        let stats = db.instance_stats();
        assert!(stats.memory.is_none());
        assert_eq!(stats.memory_limit_bytes, 8 * 1024 * 1024);
        assert_eq!(runtime.stats().active_tasks, 1);
        drop(mt);
        drop(task);
        let stats = db.instance_stats();
        assert_eq!(stats.memory.unwrap().node_count, 1);
        db.close().unwrap();
        let stats = db.instance_stats();
        assert_eq!(stats.shutdown.phase, ShutdownPhase::Closed);
        assert_eq!(stats.memory.unwrap().reserved_database_bytes, 0);
    }

    #[test]
    fn 关闭等待预算名额时可观测原因且释放后完成() {
        let (mut db, runtime) = open();
        let mut occupied = runtime.enter_maintenance_task().unwrap();
        occupied.detach();
        db.request_shutdown().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = db.shutdown_status();
            assert_eq!(status.phase, ShutdownPhase::Draining);
            if let Some(failure) = status.maintenance.and_then(|status| status.last_failure) {
                assert_eq!(failure.kind, "resource_budget");
                assert_eq!(failure.budget.unwrap().resource, "maintenance_tasks");
                break;
            }
            assert!(Instant::now() < deadline, "关闭延期原因未及时发布");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!db.close_timeout(Duration::from_millis(10)).unwrap());
        assert_eq!(
            db.instance_stats().memory.unwrap().reserved_database_bytes,
            8 * 1024 * 1024
        );
        drop(occupied);
        assert!(db.close_timeout(Duration::from_secs(5)).unwrap());
        assert_eq!(runtime.stats().attached_databases, 0);
    }
    #[test]
    fn 注册_hook_准入或构造失败保留原扩展() {
        let (mut db, runtime) = open();
        let original = db.hook();
        let (entered, arrived) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _task = runtime.enter_task().unwrap();
            entered.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        arrived.recv_timeout(Duration::from_secs(10)).unwrap();
        let mut called = false;
        let result = db.set_hook_with(
            || {
                called = true;
                Ok(NoopHook)
            },
            std::convert::identity,
        );
        release.send(()).unwrap();
        holder.join().unwrap();
        assert!(matches!(
            result,
            Err(TriviumError::ResourceBudgetExceeded { .. })
        ));
        assert!(!called);
        assert!(Arc::ptr_eq(&original, &db.hook()));
        let result = db.set_hook_with(
            || Err::<NoopHook, _>(TriviumError::InvalidInput("模拟绑定创建失败".into())),
            std::convert::identity,
        );
        assert!(result.is_err());
        assert!(Arc::ptr_eq(&original, &db.hook()));
        db.close().unwrap();
    }

    #[test]
    fn 关闭请求登记不等待_wal_锁() {
        let (mut db, _) = open();
        let wal = db.wal.clone();
        let (entered, arrived) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _writer = lock_or_recover(&wal);
            entered.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        arrived.recv_timeout(Duration::from_secs(10)).unwrap();
        let (requested, completed) = mpsc::channel();
        let closer = std::thread::spawn(move || {
            let status = db.request_shutdown();
            requested.send(status).unwrap();
            db.close_timeout(Duration::from_secs(10))
        });
        let status = completed.recv_timeout(Duration::from_secs(2));
        // 先释放屏障再断言，即使回归出现阻塞，测试也能有界退出。
        release.send(()).unwrap();
        holder.join().unwrap();
        assert!(closer.join().unwrap().unwrap());
        assert!(status.unwrap().is_ok());
    }

    impl SearchHook for Gate {
        fn on_pre_search(&self, _: &mut Vec<f32>, _: &mut SearchConfig, _: &mut HookContext) {
            self.entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }
    }

    #[test]
    fn 关闭超时并丢弃包装对象仍保护排空任务与文件锁() {
        let (mut db, runtime) = open();
        let path = db.db_path.clone();
        let (entered, arrived) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        db.set_hook(Gate {
            entered,
            release: Mutex::new(wait),
        })
        .unwrap();
        let handle = db.search_handle();
        let query = std::thread::spawn(move || {
            handle.search_batch(
                &[vec![1.0, 0.0, 0.0]],
                &SearchConfig {
                    top_k: 1,
                    expand_depth: 0,
                    ..Default::default()
                },
                &BatchSearchConfig::default(),
            )
        });
        arrived.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(!db.close_timeout(Duration::from_millis(10)).unwrap());
        assert_eq!(db.shutdown_status().phase, ShutdownPhase::Draining);
        assert_eq!(db.instance_stats().shutdown.active_operations, 1);
        assert!(matches!(
            db.insert(&[0.0, 1.0, 0.0], serde_json::json!({}))
                .unwrap_err()
                .underlying(),
            TriviumError::DatabaseClosed
        ));
        assert!(matches!(
            Database::<f32>::open(&path, 3),
            Err(TriviumError::DatabaseLocked(_))
        ));
        drop(db);
        assert!(matches!(
            Database::<f32>::open(&path, 3),
            Err(TriviumError::DatabaseLocked(_))
        ));
        release.send(()).unwrap();
        assert_eq!(query.join().unwrap().unwrap()[0][0].id, 1);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match Database::<f32>::open(&path, 3) {
                Ok(mut reopened) => {
                    assert!(reopened.contains(1).unwrap());
                    reopened.close().unwrap();
                    break;
                }
                Err(TriviumError::DatabaseLocked(_)) => {
                    assert!(Instant::now() < deadline, "排空后未释放文件锁");
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("重新打开失败：{error}"),
            }
        }
        assert_eq!(runtime.stats().attached_databases, 0);
        assert_eq!(runtime.stats().active_maintenance_tasks, 0);
    }

    #[test]
    fn 未请求关闭直接析构仍保护正在执行的独立查询() {
        let (mut db, runtime) = open();
        let path = db.db_path.clone();
        db.flush().unwrap();
        let (entered, arrived) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        db.set_hook(Gate {
            entered,
            release: Mutex::new(wait),
        })
        .unwrap();
        let handle = db.search_handle();
        let idle = handle.clone();
        let query = std::thread::spawn(move || {
            handle.search_batch(
                &[vec![1., 0., 0.]],
                &SearchConfig {
                    top_k: 1,
                    expand_depth: 0,
                    ..Default::default()
                },
                &BatchSearchConfig::default(),
            )
        });
        arrived.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(db);
        let locked = matches!(
            Database::<f32>::open(&path, 3),
            Err(TriviumError::DatabaseLocked(_))
        );
        // 先放行工作线程，再做断言，失败时也能释放真实资源。
        release.send(()).unwrap();
        let hits = query.join().unwrap().unwrap();
        assert!(locked);
        assert_eq!(hits[0].len(), 1);
        assert_eq!(hits[0][0].id, 1);
        assert_eq!(hits[0][0].payload, serde_json::json!({"state":"saved"}));
        assert!(matches!(
            idle.enter_operation(),
            Err(TriviumError::DatabaseClosed)
        ));
        assert!(matches!(
            Database::<f32>::open(&path, 3),
            Err(TriviumError::DatabaseLocked(_))
        ));
        drop(idle);
        assert_eq!(runtime.stats().attached_databases, 0);
        let mut reopened = Database::<f32>::open(&path, 3).unwrap();
        assert_eq!(reopened.get(1).unwrap().unwrap().vector, vec![1., 0., 0.]);
        assert_eq!(
            reopened.get_payload(1).unwrap().unwrap(),
            serde_json::json!({"state":"saved"})
        );
        reopened.close().unwrap();
    }

    #[test]
    fn 其他库前台占满名额时仍可关闭空闲库() {
        let (mut first, runtime) = open();
        let second_path = format!("{}.second", first.db_path);
        let mut second = Database::<f32>::open_with_runtime(
            &second_path,
            Config {
                dim: 3,
                memory_limit: 8 * 1024 * 1024,
                ..Default::default()
            },
            runtime.clone(),
        )
        .unwrap();
        let (entered, arrived) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        first
            .set_hook(Gate {
                entered,
                release: Mutex::new(wait),
            })
            .unwrap();
        let handle = first.search_handle();
        let worker = std::thread::spawn(move || {
            handle.search_batch(
                &[vec![1.0, 0.0, 0.0]],
                &SearchConfig::default(),
                &BatchSearchConfig::default(),
            )
        });
        arrived.recv_timeout(Duration::from_secs(10)).unwrap();
        let closed = second.close_timeout(Duration::from_secs(2));
        release.send(()).unwrap();
        worker.join().unwrap().unwrap();
        assert!(closed.unwrap());
        assert_eq!(second.shutdown_status().phase, ShutdownPhase::Closed);
        assert_eq!(runtime.stats().active_maintenance_tasks, 0);
        first.close().unwrap();
    }
}
