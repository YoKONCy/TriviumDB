//! Core 请求身份与执行控制，SDK 传递上下文，存储内核提供提交及持久化事实。

use crate::error::WriteErrorInfo;
use crate::query::tql_executor::QueryControl;
use crate::storage::quiver_io::{IoMeter, QuiverIoStats};
use crate::{Result, TriviumError};
use std::cell::RefCell;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
};

thread_local! {
    static ACTIVE: RefCell<Vec<RequestContext>> = const { RefCell::new(Vec::new()) };
    static PAYLOAD_METER: RefCell<Option<Arc<PayloadMeter>>> = const { RefCell::new(None) };
}

pub(crate) struct PayloadMeter {
    lookups: AtomicU64,
    parsed_bytes: AtomicU64,
    max_lookups: u64,
    max_parsed_bytes: u64,
    exhausted: OnceLock<PayloadBudgetFailure>,
}

#[derive(Clone, Copy)]
struct PayloadBudgetFailure {
    dimension: &'static str,
    used: u64,
    budget: u64,
}

impl Default for PayloadMeter {
    fn default() -> Self {
        Self::new(u64::MAX, u64::MAX)
    }
}

impl PayloadMeter {
    pub(crate) fn new(max_lookups: u64, max_parsed_bytes: u64) -> Self {
        Self {
            lookups: AtomicU64::new(0),
            parsed_bytes: AtomicU64::new(0),
            max_lookups,
            max_parsed_bytes,
            exhausted: OnceLock::new(),
        }
    }

    pub(crate) fn check(&self) -> Result<()> {
        if let Some(failure) = self.exhausted.get() {
            return Err(TriviumError::PayloadQueryBudgetExceeded {
                dimension: failure.dimension,
                used: failure.used,
                budget: failure.budget,
            });
        }
        Ok(())
    }

    fn charge(
        &self,
        counter: &AtomicU64,
        bytes: u64,
        budget: u64,
        dimension: &'static str,
    ) -> Result<()> {
        self.check()?;
        if let Err(used) = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(bytes).filter(|used| *used <= budget)
        }) {
            self.exhausted.get_or_init(|| PayloadBudgetFailure {
                dimension,
                used: used.saturating_add(bytes),
                budget,
            });
            return self.check();
        }
        Ok(())
    }

    pub(crate) fn usage(&self) -> (u64, u64) {
        (
            self.lookups.load(Ordering::Relaxed),
            self.parsed_bytes.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn enter(self: &Arc<Self>) -> PayloadScope {
        PayloadScope {
            previous: PAYLOAD_METER.with(|active| active.replace(Some(Arc::clone(self)))),
            _thread: std::marker::PhantomData,
        }
    }
}

pub(crate) struct PayloadScope {
    previous: Option<Arc<PayloadMeter>>,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl Drop for PayloadScope {
    fn drop(&mut self) {
        PAYLOAD_METER.with(|active| active.replace(self.previous.take()));
    }
}

pub(crate) fn payload_meter() -> Option<Arc<PayloadMeter>> {
    PAYLOAD_METER.with(|active| active.borrow().clone())
}

pub(crate) fn record_payload_lookup() -> Result<()> {
    PAYLOAD_METER.with(|active| {
        active.borrow().as_ref().map_or(Ok(()), |meter| {
            meter.charge(&meter.lookups, 1, meter.max_lookups, "lookups")
        })
    })
}

pub(crate) fn record_payload_parse(bytes: u64) -> Result<()> {
    PAYLOAD_METER.with(|active| {
        active.borrow().as_ref().map_or(Ok(()), |meter| {
            meter.charge(
                &meter.parsed_bytes,
                bytes,
                meter.max_parsed_bytes,
                "parsed_bytes",
            )
        })
    })
}

// TLS 归还必须在创建作用域的线程执行。
pub(crate) struct ContextScope {
    _payload: Option<PayloadScope>,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

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

/// 请求结束后只完成已开始操作的清理，退出时归还原执行控制。
pub(crate) fn finish_cleanup<T>(cleanup: impl FnOnce() -> Result<T>) -> Result<T> {
    let mut context = match current() {
        Some(context) => context,
        None => return cleanup(),
    };
    context.control = QueryControl::default();
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        active
            .try_reserve(1)
            .map_err(|error| TriviumError::CapacityAllocationFailed {
                reason: format!("清理上下文分配失败 (Cleanup context allocation failed): {error}"),
            })?;
        active.push(context);
        Ok::<_, TriviumError>(())
    })?;
    let _scope = ContextScope {
        _payload: None,
        _thread: std::marker::PhantomData,
    };
    cleanup()
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

pub(crate) fn quiver_io_meter() -> Option<Arc<IoMeter>> {
    ACTIVE.with(|active| {
        active
            .borrow()
            .last()
            .and_then(|context| context.quiver_io_meter.clone())
    })
}

pub(crate) fn measure_quiver_io<T>(
    operation: impl FnOnce() -> Result<T>,
) -> (Result<T>, QuiverIoStats) {
    let meter = Arc::new(IoMeter::default());
    let mut context = current().unwrap_or_else(|| RequestContext {
        request_id: "quiver_io".into(),
        control: QueryControl::default(),
        write: false,
        payload_meter: None,
        quiver_io_meter: None,
    });
    context.quiver_io_meter = Some(meter.clone());
    let result = context.run(operation);
    (result, meter.snapshot())
}

#[derive(Clone)]
pub struct RequestContext {
    request_id: String,
    control: QueryControl,
    write: bool,
    payload_meter: Option<Arc<PayloadMeter>>,
    quiver_io_meter: Option<Arc<IoMeter>>,
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
                if context.payload_meter.is_none() {
                    context.payload_meter = parent.payload_meter.clone();
                }
                if context.quiver_io_meter.is_none() {
                    context.quiver_io_meter = parent.quiver_io_meter.clone();
                }
            }
            active
                .try_reserve(1)
                .map_err(|error| TriviumError::CapacityAllocationFailed {
                    reason: format!(
                        "请求上下文分配失败 (Request context allocation failed): {error}"
                    ),
                })?;
            let payload = context.payload_meter.as_ref().map(PayloadMeter::enter);
            active.push(context);
            Ok(ContextScope {
                _payload: payload,
                _thread: std::marker::PhantomData,
            })
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
            payload_meter: None,
            quiver_io_meter: None,
        })
    }

    pub(crate) fn with_payload_meter(mut self, meter: Arc<PayloadMeter>) -> Self {
        self.payload_meter = Some(meter);
        self
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
