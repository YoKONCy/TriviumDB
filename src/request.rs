//! Core 请求身份与执行控制，SDK 传递上下文，存储内核提供提交及持久化事实。

use crate::error::WriteErrorInfo;
use crate::query::tql_executor::QueryControl;
use crate::{Result, TriviumError};
use std::cell::RefCell;

thread_local! {
    static ACTIVE: RefCell<Vec<RequestContext>> = const { RefCell::new(Vec::new()) };
}

// TLS 归还必须在创建作用域的线程执行。
pub(crate) struct ContextScope(std::marker::PhantomData<std::rc::Rc<()>>);

impl Drop for ContextScope {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            active.borrow_mut().pop();
        });
    }
}

pub(crate) fn current() -> Option<RequestContext> {
    ACTIVE.with(|active| active.borrow().last().cloned())
}

pub(crate) fn check_current() -> Result<()> {
    ACTIVE.with(|active| {
        active
            .borrow()
            .last()
            .map_or(Ok(()), |context| context.control.check())
    })
}

pub(crate) fn control() -> Option<QueryControl> {
    ACTIVE.with(|active| {
        active
            .borrow()
            .last()
            .map(|context| context.control.clone())
    })
}

#[derive(Clone)]
pub struct RequestContext {
    request_id: String,
    control: QueryControl,
    write: bool,
}

impl RequestContext {
    pub fn with_timeout(
        request_id: &str,
        timeout: std::time::Duration,
        write: bool,
    ) -> Result<Self> {
        let control = if timeout.is_zero() {
            QueryControl::default()
        } else {
            QueryControl::with_deadline(std::time::Instant::now().checked_add(timeout).ok_or_else(
                || {
                    TriviumError::InvalidInput(
                        "请求期限超出范围 (Request deadline is out of range)".into(),
                    )
                    .at_field("timeout_ms")
                },
            )?)
        };
        Self::new(request_id, control, write)
    }

    /// 同步执行当前请求，嵌套作用域保留外层取消和较早期限。
    pub fn run<R>(&self, operation: impl FnOnce() -> Result<R>) -> Result<R> {
        let _scope = self.enter().map_err(|error| self.admission_error(error))?;
        operation()
    }

    pub(crate) fn enter(&self) -> Result<ContextScope> {
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            // 为 SDK 的 32 层用户作用域预留适配与工作线程的内部帧。
            if active.len() >= 64 {
                return Err(TriviumError::InvalidInput(
                    "请求上下文嵌套超过 64 层 (Request context nesting exceeds 64 levels)".into(),
                ));
            }
            let mut context = self.clone();
            if let Some(parent) = active.last() {
                context.control = context.control.combined_with(&parent.control);
            }
            active
                .try_reserve(1)
                .map_err(|error| TriviumError::CapacityAllocationFailed {
                    reason: format!(
                        "请求上下文分配失败 (Request context allocation failed): {error}"
                    ),
                })?;
            active.push(context);
            Ok(ContextScope(std::marker::PhantomData))
        })
    }

    pub fn cancel(&self) {
        self.control.cancel();
    }

    pub fn new(request_id: &str, control: QueryControl, write: bool) -> Result<Self> {
        if request_id.is_empty() || request_id.len() > 128 {
            return Err(TriviumError::InvalidInput(
                "请求 ID 长度必须为 1..=128 字节 (Request ID length must be 1..=128 bytes)".into(),
            )
            .at_field("request_id"));
        }
        Ok(Self {
            request_id: request_id.into(),
            control,
            write,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    pub fn control(&self) -> &QueryControl {
        &self.control
    }
    pub fn is_write(&self) -> bool {
        self.write
    }

    /// 准入前检查共享取消及期限，执行结果由实际内核操作返回。
    pub fn check_admission(&self) -> Result<()> {
        self.control
            .check()
            .map_err(|error| self.admission_error(error))
    }

    pub(crate) fn admission_error(&self, error: TriviumError) -> TriviumError {
        if self.write {
            error.with_write_info(WriteErrorInfo::MAINTENANCE_ADMISSION)
        } else {
            error
        }
    }
}
