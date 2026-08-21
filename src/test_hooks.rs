//! 仅测试构建可用的确定性交错控制器。
//!
//! 启用 `test-hooks` feature 后，测试可以在指定执行点阻塞目标线程，再通过
//! `WaitHandle` 精确等待到达并释放。默认生产构建不编译本模块，因而没有运行时开销。

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConcurrencyPoint {
    BeforeStatefulSearchLock,
    StatefulSearchEntered,
    BeforeClearSearchStateLock,
    ClearSearchStateEntered,
    BeforeUpdateVectorWriteLock,
    UpdateVectorWriteLockAcquired,
    BeforeQuiverBuildClaim,
    QuiverBuildFollowerWaiting,
    QuiverBuildStarted,
    SearchLockAcquired,
    QuiverCandidateProduced,
    BeforeVectorRerank,
    BeforeQuiverPublish,
    BeforeMergedCachePublish,
    AfterWalAppend,
    BeforeMemtableApply,
    BeforeCompactionSave,
    BeforeWalClear,
}

#[derive(Default)]
struct GateState {
    arrivals: usize,
    released: bool,
}

#[derive(Default)]
struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
}

impl Gate {
    fn arrive_and_wait(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.arrivals += 1;
        self.changed.notify_all();
        while !state.released {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }

    fn wait_until_arrived(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        while state.arrivals == 0 {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }

    fn wait_until_arrivals(&self, expected: usize) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        while state.arrivals < expected {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }

    fn arrivals(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .arrivals
    }

    fn is_arrived(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .arrivals
            > 0
    }

    fn release(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.released = true;
        self.changed.notify_all();
    }
}

fn registry() -> &'static Mutex<HashMap<ConcurrencyPoint, Arc<Gate>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<ConcurrencyPoint, Arc<Gate>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 某个交错点的测试端控制句柄。
pub struct WaitHandle {
    point: ConcurrencyPoint,
    gate: Arc<Gate>,
}

impl WaitHandle {
    /// 等待目标线程确定到达交错点。
    pub fn wait_until_arrived(&self) {
        self.gate.wait_until_arrived();
    }

    pub fn wait_until_arrivals(&self, expected: usize) {
        self.gate.wait_until_arrivals(expected);
    }

    pub fn arrivals(&self) -> usize {
        self.gate.arrivals()
    }

    pub fn is_arrived(&self) -> bool {
        self.gate.is_arrived()
    }

    /// 释放目标线程并注销交错点。
    pub fn release(self) {
        self.gate.release();
        registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&self.point);
    }
}

impl Drop for WaitHandle {
    fn drop(&mut self) {
        self.gate.release();
        registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&self.point);
    }
}

/// 注册一次性阻塞点。同一交错点不能同时注册两次。
pub fn pause_at(point: ConcurrencyPoint) -> WaitHandle {
    let gate = Arc::new(Gate::default());
    let previous = registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(point, Arc::clone(&gate));
    assert!(previous.is_none(), "同一个并发交错点不能重复注册");
    WaitHandle { point, gate }
}

/// 由生产路径调用：未注册时快速返回，注册时通知测试并等待释放。
pub fn hit(point: ConcurrencyPoint) {
    let gate = registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(&point)
        .cloned();
    if let Some(gate) = gate {
        gate.arrive_and_wait();
    }
}
