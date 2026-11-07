//! 控制台宿主适配；只借用 SDK 或宿主已经拥有的实例。

use super::shared::lock_error;
use super::{ConsoleOptions, ConsoleSession, SharedDatabase, WeakDatabase};
use crate::query::tql_executor::QueryControl;
use crate::{Database, Result, TriviumError, VectorType};
use std::sync::{Arc, Mutex, Weak};

enum HostDatabase<T: VectorType> {
    External(Weak<Mutex<Database<T>>>),
    Shared(WeakDatabase<T>),
}

/// 组件持有实例的弱引用，卸载组件只释放会话。
pub struct ConsoleHost<T: VectorType> {
    database: HostDatabase<T>,
    session: ConsoleSession,
}

impl<T: VectorType> Clone for ConsoleHost<T> {
    fn clone(&self) -> Self {
        Self {
            database: match &self.database {
                HostDatabase::External(database) => HostDatabase::External(database.clone()),
                HostDatabase::Shared(database) => HostDatabase::Shared(database.clone()),
            },
            session: self.session.clone(),
        }
    }
}

impl<T: VectorType + serde::Serialize + serde::de::DeserializeOwned> ConsoleHost<T> {
    /// 兼容宿主自己的互斥锁；锁忙直接返回，避免跨 IPC 或 Hook 等待环。
    pub fn attach(database: &Arc<Mutex<Database<T>>>, options: ConsoleOptions) -> Result<Self> {
        let session = ConsoleSession::new(options)?;
        database
            .try_lock()
            .map_err(|error| lock_error(error, "host_database"))?
            .attach_console_session(&session)?;
        Ok(Self {
            database: HostDatabase::External(Arc::downgrade(database)),
            session,
        })
    }

    /// 与 SDK 调用者共用同一套请求准入和重入检查。
    pub fn attach_shared(database: &SharedDatabase<T>, options: ConsoleOptions) -> Result<Self> {
        let session = ConsoleSession::new(options)?;
        database.try_read(|core| core.attach_console_session(&session))?;
        Ok(Self {
            database: HostDatabase::Shared(database.downgrade()),
            session,
        })
    }

    pub fn session(&self) -> ConsoleSession {
        self.session.clone()
    }

    pub fn request(&self, request_json: &str) -> Result<String> {
        self.request_inner(request_json, None)
    }

    pub fn request_with_control(
        &self,
        request_json: &str,
        control: QueryControl,
    ) -> Result<String> {
        self.request_inner(request_json, Some(control))
    }

    fn request_inner(&self, request_json: &str, control: Option<QueryControl>) -> Result<String> {
        let prepared = self.session.prepare_request(request_json, control)?;
        let context = prepared.context.clone();
        let execute =
            |core: &mut Database<T>| core.execute_console_request(&self.session, prepared);
        match &self.database {
            HostDatabase::External(database) => {
                let database = database
                    .upgrade()
                    .ok_or_else(|| context.admission_error(TriviumError::DatabaseClosed))?;
                let mut guard = database
                    .try_lock()
                    .map_err(|error| context.admission_error(lock_error(error, "host_database")))?;
                context.check_admission()?;
                execute(&mut guard)
            }
            HostDatabase::Shared(database) => {
                let database = database
                    .upgrade()
                    .ok_or_else(|| context.admission_error(TriviumError::DatabaseClosed))?;
                database.try_write_with_context(&context, execute)
            }
        }
    }
}
