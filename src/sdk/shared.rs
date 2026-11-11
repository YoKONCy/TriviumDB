//! 共享宿主句柄；按请求借用内核，锁忙和同线程重入均在调用方返回。

use crate::database::{Config, TxBuilder};
use crate::{Database, NodeId, Result, ShutdownStatus, TriviumError, VectorType};
use std::cell::RefCell;
use std::sync::{Arc, RwLock, TryLockError, Weak};
use std::time::Duration;

thread_local! {
    static ACTIVE_INSTANCES: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// 多个线程和组件共享一个存储内核及文件租约。
///
/// `try_read`/`try_write` 的闭包应只包含本次数据库操作；耗时准备放在闭包外。
/// 普通争用返回 `TDB_BUSY`，同线程重入返回 `TDB_REENTRANT_ACCESS`。
/// 克隆与释放句柄只改变共享所有权；显式关闭作用于所有句柄。
pub struct SharedDatabase<T: VectorType> {
    inner: Arc<RwLock<Database<T>>>,
}

/// 回调或组件可持有弱句柄，避免与内核 Hook 形成所有权环。
pub struct WeakDatabase<T: VectorType> {
    inner: Weak<RwLock<Database<T>>>,
}

impl<T: VectorType> Clone for WeakDatabase<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: VectorType> WeakDatabase<T> {
    pub fn upgrade(&self) -> Option<SharedDatabase<T>> {
        self.inner.upgrade().map(|inner| SharedDatabase { inner })
    }
}

impl<T: VectorType> Clone for SharedDatabase<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: VectorType> SharedDatabase<T> {
    pub fn downgrade(&self) -> WeakDatabase<T> {
        WeakDatabase {
            inner: Arc::downgrade(&self.inner),
        }
    }
}

pub(crate) struct RequestScope(std::marker::PhantomData<std::rc::Rc<()>>);

impl RequestScope {
    #[cfg(feature = "python")]
    pub(crate) fn callback(identity: usize, operation: &'static str) -> Result<Option<Self>> {
        // 同步 Hook 复用外层登记；工作线程上的 Hook 在本线程补上同一实例身份。
        if ACTIVE_INSTANCES.with(|active| active.borrow().contains(&identity)) {
            Ok(None)
        } else {
            Self::enter(identity, operation).map(Some)
        }
    }

    pub(crate) fn enter(identity: usize, operation: &'static str) -> Result<Self> {
        ACTIVE_INSTANCES.with(|active| {
            let mut active = active.borrow_mut();
            if active.contains(&identity) {
                return Err(TriviumError::ReentrantAccess { operation });
            }
            active
                .try_reserve(1)
                .map_err(|error| TriviumError::CapacityAllocationFailed {
                    reason: format!(
                        "请求作用域分配失败 (Request scope allocation failed): {error}"
                    ),
                })?;
            active.push(identity);
            Ok(Self(std::marker::PhantomData))
        })
    }
}

impl Drop for RequestScope {
    fn drop(&mut self) {
        ACTIVE_INSTANCES.with(|active| {
            active.borrow_mut().pop();
        });
    }
}

pub(crate) fn lock_error<T>(error: TryLockError<T>, resource: &'static str) -> TriviumError {
    match error {
        TryLockError::WouldBlock => TriviumError::DatabaseBusy { resource },
        TryLockError::Poisoned(_) => TriviumError::HostLockPoisoned { resource },
    }
}

impl<T: VectorType + serde::Serialize + serde::de::DeserializeOwned> SharedDatabase<T> {
    pub fn try_read_with_context<R>(
        &self,
        context: &super::RequestContext,
        operation: impl FnOnce(&Database<T>) -> Result<R>,
    ) -> Result<R> {
        let _context = context
            .enter()
            .map_err(|error| context.admission_error(error))?;
        crate::request::check_current().map_err(|error| context.admission_error(error))?;
        let _scope = RequestScope::enter(Arc::as_ptr(&self.inner) as usize, "shared_read")
            .map_err(|error| context.admission_error(error))?;
        let guard = self
            .inner
            .try_read()
            .map_err(|error| context.admission_error(lock_error(error, "shared_database")))?;
        guard
            .check_request_admission()
            .map_err(|error| context.admission_error(error))?;
        crate::request::check_current().map_err(|error| context.admission_error(error))?;
        operation(&guard)
    }

    pub fn try_write_with_context<R>(
        &self,
        context: &super::RequestContext,
        operation: impl FnOnce(&mut Database<T>) -> Result<R>,
    ) -> Result<R> {
        let _context = context
            .enter()
            .map_err(|error| context.admission_error(error))?;
        crate::request::check_current().map_err(|error| context.admission_error(error))?;
        let _scope = RequestScope::enter(Arc::as_ptr(&self.inner) as usize, "shared_write")
            .map_err(|error| context.admission_error(error))?;
        let mut guard = self
            .inner
            .try_write()
            .map_err(|error| context.admission_error(lock_error(error, "shared_database")))?;
        guard
            .check_request_admission()
            .map_err(|error| context.admission_error(error))?;
        crate::request::check_current().map_err(|error| context.admission_error(error))?;
        operation(&mut guard)
    }

    /// 移入已打开的内核；文件租约保持原有所有权。
    pub fn new(database: Database<T>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(database)),
        }
    }

    pub fn try_read<R>(&self, operation: impl FnOnce(&Database<T>) -> Result<R>) -> Result<R> {
        let _scope = RequestScope::enter(Arc::as_ptr(&self.inner) as usize, "shared_read")?;
        let guard = self
            .inner
            .try_read()
            .map_err(|error| lock_error(error, "shared_database"))?;
        guard.check_request_admission()?;
        operation(&guard)
    }

    pub fn try_write<R>(&self, operation: impl FnOnce(&mut Database<T>) -> Result<R>) -> Result<R> {
        self.with_write(|database| {
            database.check_request_admission()?;
            operation(database)
        })
    }

    fn with_write<R>(&self, operation: impl FnOnce(&mut Database<T>) -> Result<R>) -> Result<R> {
        let _scope = RequestScope::enter(Arc::as_ptr(&self.inner) as usize, "shared_write")?;
        let mut guard = self
            .inner
            .try_write()
            .map_err(|error| lock_error(error, "shared_database"))?;
        operation(&mut guard)
    }
}

impl<T: VectorType + serde::Serialize + serde::de::DeserializeOwned> SharedDatabase<T> {
    pub fn open(path: &str, dim: usize) -> Result<Self> {
        Database::open(path, dim).map(Self::new)
    }

    pub fn open_with_config(path: &str, config: Config) -> Result<Self> {
        Database::open_with_config(path, config).map(Self::new)
    }

    /// 准入失败保留事务；准入成功后消费事务并由内核原子提交。
    pub fn commit_tx(&self, transaction: &mut TxBuilder<T>) -> Result<Vec<NodeId>> {
        self.try_write(|database| {
            database.check_transaction_admission()?;
            database.commit_tx(std::mem::take(transaction))
        })
        .map_err(|error| error.with_write_info(crate::error::WriteErrorInfo::TRANSACTION_ADMISSION))
    }

    /// 上下文准入与内核预检成功后再消费 builder，拒绝时保留原缓冲。
    pub fn commit_tx_with_context(
        &self,
        context: &super::RequestContext,
        transaction: &mut TxBuilder<T>,
    ) -> Result<Vec<NodeId>> {
        let consumed = std::cell::Cell::new(false);
        self.try_write_with_context(context, |database| {
            database.check_transaction_admission()?;
            let transaction = std::mem::take(transaction);
            consumed.set(true);
            database.commit_ops_guarded(transaction.into_ops(), None, Some(context.control()))
        })
        .map_err(|error| {
            if consumed.get() {
                error
            } else {
                TriviumError::WriteFailure {
                    source: Box::new(error),
                    write: crate::error::WriteErrorInfo::TRANSACTION_ADMISSION,
                }
            }
        })
    }

    pub fn request_shutdown(&self) -> Result<ShutdownStatus> {
        self.with_write(Database::request_shutdown)
    }

    /// 超时只结束本次等待；关闭任务继续持有文件租约直至完成。
    pub fn close_timeout(&self, timeout: Duration) -> Result<bool> {
        self.with_write(|database| database.close_timeout(timeout))
    }

    pub fn close(&self) -> Result<()> {
        self.with_write(Database::close)
    }

    pub fn shutdown_status(&self) -> Result<ShutdownStatus> {
        let _scope = RequestScope::enter(Arc::as_ptr(&self.inner) as usize, "shutdown_status")?;
        let guard = self
            .inner
            .try_read()
            .map_err(|error| lock_error(error, "shared_database"))?;
        Ok(guard.shutdown_status())
    }
}
