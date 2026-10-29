//! 仅测试构建可用的确定性交错控制器。
//!
//! 启用 `test-hooks` feature 后，测试可以在指定执行点阻塞目标线程，再通过
//! `WaitHandle` 精确等待到达并释放。默认生产构建不编译本模块，因而没有运行时开销。

use std::cell::Cell;
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConcurrencyPoint {
    BeforeSearchReadLock,
    BeforeSnapshotPublish,
    AfterSnapshotPublish,
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
    AfterVecPersisted,
    AfterTdbPersisted,
    BeforeFlushMarkerRename,
    AfterFlushMarkerRename,
    AfterPublicationPrepared,
    AfterPublicationRestore,
    AfterSidecarsPersisted,
    AfterPublicationCommitted,
    MigrationFilePublished,
    TqlPathStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoPoint {
    TransactionApply,
    SnapshotCopy,
    SnapshotSync,
    SnapshotPublish,
    SnapshotDirectorySync,
    MarkerCreate,
    MarkerWrite,
    MarkerSync,
    MarkerRename,
    MarkerMetadata,
    PublicationBackupCopy,
    PublicationJournalSync,
    PublicationJournalRename,
    PublicationRestoreCopy,
    PublicationCommitSync,
    PublicationCommitRename,
    TextMetadataWrite,
    PropertySidecarWrite,
    GraphSidecarWrite,
    CheckpointWalClear,
    MigrationPublishFile,
    MigrationAfterFirstFile,
    MigrationPublishMarker,
    MigrationAfterCommit,
    WalFlush,
    WalSync,
    WalMigrationCreate,
    WalMigrationWrite,
    WalMigrationSync,
    WalMigrationRename,
}

thread_local! {
    static IO_FAILURE: Cell<Option<IoPoint>> = const { Cell::new(None) };
}

pub struct IoFailureGuard {
    previous: Option<IoPoint>,
}

impl Drop for IoFailureGuard {
    fn drop(&mut self) {
        IO_FAILURE.set(self.previous);
    }
}

pub(crate) fn current_io_failure() -> Option<IoPoint> {
    IO_FAILURE.get()
}

pub fn fail_io_at(point: IoPoint) -> IoFailureGuard {
    let previous = IO_FAILURE.replace(Some(point));
    IoFailureGuard { previous }
}

pub fn io_result(point: IoPoint) -> io::Result<()> {
    if IO_FAILURE.get() == Some(point) {
        Err(io::Error::other(format!(
            "deterministic I/O failure at {point:?}"
        )))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueryExecutionStrategy {
    #[default]
    Auto,
    ForceSerial,
    ForceParallel,
    DisableFusion,
}

thread_local! {
    static QUERY_STRATEGY: Cell<QueryExecutionStrategy> = const { Cell::new(QueryExecutionStrategy::Auto) };
}

pub struct QueryStrategyGuard {
    previous: QueryExecutionStrategy,
}

impl Drop for QueryStrategyGuard {
    fn drop(&mut self) {
        QUERY_STRATEGY.set(self.previous);
    }
}

pub fn force_query_strategy(strategy: QueryExecutionStrategy) -> QueryStrategyGuard {
    let previous = QUERY_STRATEGY.replace(strategy);
    QueryStrategyGuard { previous }
}

pub fn query_strategy() -> QueryExecutionStrategy {
    QUERY_STRATEGY.get()
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

type GateKey = (ConcurrencyPoint, Option<String>);
type GateRegistry = Mutex<HashMap<GateKey, Arc<Gate>>>;

fn registry() -> &'static GateRegistry {
    static REGISTRY: OnceLock<GateRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 某个交错点的测试端控制句柄。
pub struct WaitHandle {
    key: GateKey,
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
        drop(self);
    }
}

impl Drop for WaitHandle {
    fn drop(&mut self) {
        let mut gates = registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if gates
            .get(&self.key)
            .is_some_and(|gate| Arc::ptr_eq(gate, &self.gate))
        {
            gates.remove(&self.key);
        }
        drop(gates);
        self.gate.release();
    }
}

/// 注册一次性阻塞点。同一交错点不能同时注册两次。
pub fn pause_at(point: ConcurrencyPoint) -> WaitHandle {
    register_gate((point, None))
}

/// 只阻塞指定数据库规范路径，允许相同阶段的多个实例并行测试。
pub fn pause_at_path(point: ConcurrencyPoint, path: &str) -> WaitHandle {
    register_gate((point, Some(path.to_owned())))
}

fn register_gate(key: GateKey) -> WaitHandle {
    let gate = Arc::new(Gate::default());
    let mut gates = registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if gates.contains_key(&key) {
        drop(gates);
        panic!("同一范围的并发交错点不能重复注册");
    }
    gates.insert(key.clone(), Arc::clone(&gate));
    WaitHandle { key, gate }
}

/// 由生产路径调用：未注册时快速返回，注册时通知测试并等待释放。
pub fn hit(point: ConcurrencyPoint) {
    let gate = registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(&(point, None))
        .cloned();
    if let Some(gate) = gate {
        gate.arrive_and_wait();
    }
}

/// 文件发布路径同时支持全局屏障与按数据库路径选择的屏障。
pub fn hit_for_path(point: ConcurrencyPoint, path: &str) {
    hit(point);
    let gate = registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(&(point, Some(path.to_owned())))
        .cloned();
    if let Some(gate) = gate {
        gate.arrive_and_wait();
    }
}
