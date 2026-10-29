//! 轻量级事务支持 + WAL 回放
//!
//! 从 database.rs 独立拆分，包含：
//! - `TxOp`: 事务操作类型（内部缓冲用）
//! - `Transaction`: 轻量级事务（Dry-Run 预检 + WAL-first 语义）
//! - `replay_entry`: WAL 崩溃恢复回放

use crate::VectorType;
use crate::database::Database;
use crate::error::{Result, TriviumError};
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use crate::storage::wal::WalEntry;

use super::{lock_or_recover, write_or_recover};

/// 应用过程异常时保留 WAL，并让共享句柄统一等待重新打开恢复。
struct ApplicationGuard<'a, T: VectorType> {
    mt: &'a mut MemTable<T>,
    wal: &'a std::sync::Mutex<crate::storage::wal::Wal>,
    lifecycle: &'a std::sync::Mutex<super::LifecycleState>,
    previous_pause: bool,
    previous_unique_pause: bool,
    complete: bool,
}
impl<'a, T: VectorType> ApplicationGuard<'a, T> {
    fn new(
        mt: &'a mut MemTable<T>,
        wal: &'a std::sync::Mutex<crate::storage::wal::Wal>,
        lifecycle: &'a std::sync::Mutex<super::LifecycleState>,
    ) -> Self {
        let previous_unique_pause = mt.unique_updates_paused();
        mt.set_unique_updates_paused(true);
        let previous_pause = mt.quiver_sync_paused();
        mt.set_quiver_sync_paused(true);
        Self {
            mt,
            wal,
            lifecycle,
            previous_pause,
            previous_unique_pause,
            complete: false,
        }
    }
}
impl<T: VectorType> Drop for ApplicationGuard<'_, T> {
    fn drop(&mut self) {
        self.mt.set_quiver_sync_paused(self.previous_pause);
        self.mt
            .set_unique_updates_paused(self.previous_unique_pause);
        if !self.complete {
            self.mt.require_recovery();
            lock_or_recover(self.wal).release_writer();
            lock_or_recover(self.lifecycle)
                .recovery_required
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

static NEXT_TX_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn next_transaction_id(counter: &std::sync::atomic::AtomicU64) -> Result<u64> {
    use std::sync::atomic::Ordering;
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(1).ok_or_else(|| {
            TriviumError::InvalidInput("事务 ID 空间已耗尽 (Transaction ID space exhausted)".into())
        })?;
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(current),
            Err(actual) => current = actual,
        }
    }
}

/// WAL 崩溃恢复：回放单条 WAL 记录到 MemTable
///
/// 设计要点：
/// - 幂等性：已存在的 ID 会被跳过
/// - 无论是否跳过插入，都推进 `next_id` 防止 ID 复用
pub(crate) fn replay_entry<T: VectorType>(mt: &mut MemTable<T>, entry: WalEntry<T>) -> Result<()> {
    let extra = match &entry {
        WalEntry::Insert { payload, .. } => mt
            .estimate_reserve_bytes(1)?
            .saturating_add(payload.len().saturating_mul(64)),
        WalEntry::UpdatePayload { payload, .. } => payload.len().saturating_mul(64),
        WalEntry::Link {
            label, metadata, ..
        } => label
            .len()
            .saturating_mul(5)
            .saturating_add(metadata.len().saturating_mul(64)),
        _ => 0,
    };
    mt.check_load_allocation(extra)?;
    match entry {
        WalEntry::Insert {
            id,
            vector,
            payload,
        } => {
            // 即使检查点已包含此 ID，也要验证日志内容，防止幂等跳过掩盖非法记录。
            let payload_val = replay_json(&payload, "Insert.payload", id)?;
            let next_id = id.checked_add(1).filter(|_| id != 0).ok_or_else(|| {
                TriviumError::CorruptedFile(format!(
                    "WAL 插入 ID 超出有效范围 (WAL insert ID is out of range): id={id}"
                ))
            })?;
            mt.validate_insert(&vector).map_err(|error| {
                TriviumError::CorruptedFile(format!(
                    "WAL 插入向量无效 (Invalid WAL insert vector): id={id}, {error}"
                ))
            })?;
            if mt.contains(id) {
                // 幂等：该 ID 已存在（可能来自 .tdb 加载或重复回放），跳过
                tracing::debug!(
                    "WAL 回放跳过已存在的节点 (WAL replay skipped existing node) {}",
                    id
                );
            } else {
                mt.raw_insert(id, &vector, payload_val)?;
            }
            // 无论是否跳过，都必须推进 next_id 防止后续 insert 复用已物化的 ID
            mt.advance_next_id(next_id);
        }
        WalEntry::Link {
            src,
            dst,
            label,
            weight,
            metadata,
        } => {
            let metadata = replay_json(&metadata, "Link.metadata", src)?;
            if !weight.is_finite() {
                return Err(TriviumError::CorruptedFile(format!(
                    "WAL 边权重无效 (Invalid WAL edge weight): src={src}, dst={dst}"
                )));
            }
            if mt.contains(src) && mt.contains(dst) {
                mt.check_load_allocation(
                    mt.estimate_edge_write_bytes(src, dst, &label, &metadata)?,
                )?;
                mt.upsert_edge(src, dst, label, weight, metadata)?;
            }
        }
        WalEntry::Delete { id } => {
            if mt.contains(id) {
                mt.delete(id)?;
            }
        }
        WalEntry::Unlink { src, dst } => {
            if mt.contains(src) {
                mt.unlink(src, dst)?;
            }
        }
        WalEntry::UnlinkLabel { src, dst, label } => {
            if mt.contains(src) {
                mt.unlink_label(src, dst, &label)?;
            }
        }
        WalEntry::UpdatePayload { id, payload } => {
            let payload_val = replay_json(&payload, "UpdatePayload.payload", id)?;
            if mt.contains(id) {
                mt.update_payload(id, payload_val)?;
            }
        }
        WalEntry::UpdateVector { id, vector } => {
            mt.validate_insert(&vector).map_err(|error| {
                TriviumError::CorruptedFile(format!(
                    "WAL 更新向量无效 (Invalid WAL update vector): id={id}, {error}"
                ))
            })?;
            if mt.contains(id) {
                mt.update_vector(id, &vector)?;
            }
        }
        WalEntry::TxBegin { .. } | WalEntry::TxCommit { .. } => {
            // 已在 wal.rs 内的回放过滤环节处理，这里不应再收到，直接忽略
        }
    }
    Ok(())
}

/// 帧与 CRC 校验通过之后，仍须独立验证业务字段的 JSON 语义。
fn replay_json(raw: &str, field: &str, id: NodeId) -> Result<serde_json::Value> {
    serde_json::from_str(raw).map_err(|error| {
        TriviumError::CorruptedFile(format!(
            "WAL JSON 无效 (Invalid WAL JSON): field={field}, id={id}, {error}"
        ))
    })
}

/// 提交前确认 JSON 可被恢复端解码，包括 serde_json 的嵌套深度边界。
fn validate_transaction_json(raw: &str) -> Result<()> {
    serde_json::from_str::<serde_json::Value>(raw)
        .map(|_| ())
        .map_err(|error| {
            TriviumError::InvalidInput(format!(
                "事务 JSON 无法持久化并恢复 (Transaction JSON cannot be persisted and recovered): {error}"
            ))
        })
}

// ════════════════════════════════════════════════════════
//  事务操作类型
// ════════════════════════════════════════════════════════

/// 事务操作类型
pub enum TxOp<T> {
    Insert {
        vector: Vec<T>,
        payload: serde_json::Value,
    },
    InsertWithId {
        id: NodeId,
        vector: Vec<T>,
        payload: serde_json::Value,
    },
    Link {
        src: NodeId,
        dst: NodeId,
        label: String,
        weight: f32,
        metadata: serde_json::Value,
    },
    Delete {
        id: NodeId,
    },
    Unlink {
        src: NodeId,
        dst: NodeId,
    },
    UnlinkLabel {
        src: NodeId,
        dst: NodeId,
        label: String,
    },
    UpdatePayload {
        id: NodeId,
        payload: serde_json::Value,
    },
    UpdateVector {
        id: NodeId,
        vector: Vec<T>,
    },
}

// ════════════════════════════════════════════════════════
//  轻量级事务
// ════════════════════════════════════════════════════════

/// 轻量级事务
///
/// 所有操作在 commit() 前仅缓冲在内存中，不会影响数据库状态。
/// - `commit()` → 持锁完成顺序预检与预留，先提交 WAL，再按预检顺序应用到 memtable
/// - `rollback()` → 丢弃缓冲（或 drop 自动丢弃）
///
/// ```rust,ignore
/// let mut tx = db.begin_tx();
/// tx.insert(&vec, payload);
/// tx.link(1, 2, "knows", 1.0);
/// tx.commit()?;  // 原子提交
/// ```
pub struct Transaction<'a, T: VectorType + serde::Serialize + serde::de::DeserializeOwned> {
    pub(crate) db: &'a mut Database<T>,
    pub(crate) ops: Vec<TxOp<T>>,
    pub(crate) committed: bool,
}

impl<'a, T: VectorType + serde::Serialize + serde::de::DeserializeOwned> Transaction<'a, T> {
    /// 缓冲一个插入操作
    pub fn insert(&mut self, vector: &[T], payload: serde_json::Value) {
        self.ops.push(TxOp::Insert {
            vector: vector.to_vec(),
            payload,
        });
    }

    /// 缓冲一个带自定义 ID 的插入操作
    pub fn insert_with_id(&mut self, id: NodeId, vector: &[T], payload: serde_json::Value) {
        self.ops.push(TxOp::InsertWithId {
            id,
            vector: vector.to_vec(),
            payload,
        });
    }

    /// 缓冲一个连边操作
    pub fn link(&mut self, src: NodeId, dst: NodeId, label: &str, weight: f32) {
        self.upsert_edge(src, dst, label, weight, serde_json::Value::Null);
    }

    pub fn upsert_edge(
        &mut self,
        src: NodeId,
        dst: NodeId,
        label: &str,
        weight: f32,
        metadata: serde_json::Value,
    ) {
        self.ops.push(TxOp::Link {
            src,
            dst,
            label: label.to_string(),
            weight,
            metadata,
        });
    }

    /// 缓冲一个删除操作
    pub fn delete(&mut self, id: NodeId) {
        self.ops.push(TxOp::Delete { id });
    }

    /// 缓冲一个断边操作
    pub fn unlink(&mut self, src: NodeId, dst: NodeId) {
        self.ops.push(TxOp::Unlink { src, dst });
    }

    /// 缓冲一个指定标签断边操作
    pub fn unlink_label(&mut self, src: NodeId, dst: NodeId, label: &str) {
        self.ops.push(TxOp::UnlinkLabel {
            src,
            dst,
            label: label.to_string(),
        });
    }

    /// 缓冲一个更新 payload 操作
    pub fn update_payload(&mut self, id: NodeId, payload: serde_json::Value) {
        self.ops.push(TxOp::UpdatePayload { id, payload });
    }

    /// 缓冲一个更新向量操作
    pub fn update_vector(&mut self, id: NodeId, vector: &[T]) {
        self.ops.push(TxOp::UpdateVector {
            id,
            vector: vector.to_vec(),
        });
    }

    /// 当前事务中缓冲的操作数
    pub fn pending_count(&self) -> usize {
        self.ops.len()
    }

    /// 原子提交事务
    ///
    /// 流程（WAL-first 持久化语义）：
    ///   1. Dry-Run 预检：按顺序投影状态并确定 ID，预留核心容量
    ///   2. 构建 WAL 条目并检查 JSON 可恢复性，不修改事实数据
    ///   3. 先写 WAL，I/O 错误传播且该批内存数据尚未应用
    ///   4. 使用原始 JSON 值按预检顺序应用，再同步 QuIVer
    pub fn commit(mut self) -> Result<Vec<NodeId>> {
        let ops = std::mem::take(&mut self.ops);
        self.committed = true;
        self.db.commit_ops(ops)
    }

    /// 显式回滚（丢弃所有缓冲操作）
    pub fn rollback(mut self) {
        self.ops.clear();
        self.committed = true;
    }
}

impl<'a, T: VectorType + serde::Serialize + serde::de::DeserializeOwned> Drop
    for Transaction<'a, T>
{
    fn drop(&mut self) {
        if !self.committed && !self.ops.is_empty() {
            tracing::warn!(
                "事务未提交/回滚即被丢弃 (Transaction dropped without commit/rollback)，{} 个操作已放弃",
                self.ops.len()
            );
        }
    }
}

// ════════════════════════════════════════════════════════
//  TxBuilder — 无生命周期的事务操作收集器（FFI 友好）
// ════════════════════════════════════════════════════════

/// 事务操作收集器 — 不绑定 Database 引用，可自由跨 FFI 边界
///
/// 解决 Rust `Transaction<'a, T>` 持有 `&'a mut Database` 导致
/// 在 PyO3/napi-rs 等 FFI 绑定中无法暴露的结构性问题。
///
/// ```rust,ignore
/// let mut builder = TxBuilder::new();
/// builder.insert(&vec, payload);
/// builder.link(1, 2, "knows", 1.0);
/// let ids = db.commit_tx(builder)?;  // 原子提交
/// ```
pub struct TxBuilder<T> {
    ops: Vec<TxOp<T>>,
}

impl<T: VectorType> TxBuilder<T> {
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    pub fn insert(&mut self, vector: &[T], payload: serde_json::Value) {
        self.ops.push(TxOp::Insert {
            vector: vector.to_vec(),
            payload,
        });
    }

    pub fn insert_with_id(&mut self, id: NodeId, vector: &[T], payload: serde_json::Value) {
        self.ops.push(TxOp::InsertWithId {
            id,
            vector: vector.to_vec(),
            payload,
        });
    }

    pub fn link(&mut self, src: NodeId, dst: NodeId, label: &str, weight: f32) {
        self.upsert_edge(src, dst, label, weight, serde_json::Value::Null);
    }

    pub fn upsert_edge(
        &mut self,
        src: NodeId,
        dst: NodeId,
        label: &str,
        weight: f32,
        metadata: serde_json::Value,
    ) {
        self.ops.push(TxOp::Link {
            src,
            dst,
            label: label.to_string(),
            weight,
            metadata,
        });
    }

    pub fn delete(&mut self, id: NodeId) {
        self.ops.push(TxOp::Delete { id });
    }

    pub fn unlink(&mut self, src: NodeId, dst: NodeId) {
        self.ops.push(TxOp::Unlink { src, dst });
    }

    pub fn unlink_label(&mut self, src: NodeId, dst: NodeId, label: &str) {
        self.ops.push(TxOp::UnlinkLabel {
            src,
            dst,
            label: label.to_string(),
        });
    }

    pub fn update_payload(&mut self, id: NodeId, payload: serde_json::Value) {
        self.ops.push(TxOp::UpdatePayload { id, payload });
    }

    pub fn update_vector(&mut self, id: NodeId, vector: &[T]) {
        self.ops.push(TxOp::UpdateVector {
            id,
            vector: vector.to_vec(),
        });
    }

    pub fn pending_count(&self) -> usize {
        self.ops.len()
    }

    /// 消费 builder，返回内部操作列表
    pub(crate) fn into_ops(self) -> Vec<TxOp<T>> {
        self.ops
    }
}

impl<T: VectorType> Default for TxBuilder<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ════════════════════════════════════════════════════════
//  commit_ops — 共享的事务提交核心逻辑
// ════════════════════════════════════════════════════════

impl<T: VectorType + serde::Serialize + serde::de::DeserializeOwned> Database<T> {
    /// 原子提交一组事务操作（供 Transaction::commit 和 commit_tx 共用）
    pub(crate) fn commit_ops(&mut self, mut ops: Vec<TxOp<T>>) -> Result<Vec<NodeId>> {
        let _operation = self.enter_operation()?;
        self.require_write_access("transaction commit")?;
        if ops.is_empty() {
            return Ok(Vec::new());
        }

        let mut mt = write_or_recover(&self.memtable);

        // ════════ 第一阶段：预检前置 (Dry-Run) + 预分配 ID ════════
        let mut sim_next_id = mt.next_id_value();
        let dim = mt.dim();
        // 只投影本批触及的 ID；每个操作覆盖前一状态，支持同一 ID 多次删除和重建。
        let mut node_states = std::collections::HashMap::<NodeId, bool>::new();

        macro_rules! check_exists {
            ($id:expr) => {
                node_states
                    .get($id)
                    .copied()
                    .unwrap_or_else(|| mt.contains(*$id))
            };
        }

        for op in &mut ops {
            match op {
                TxOp::Insert { vector, payload } => {
                    if vector.len() != dim {
                        return Err(crate::error::TriviumError::DimensionMismatch {
                            expected: dim,
                            got: vector.len(),
                        });
                    }
                    for item in vector.iter() {
                        let f = item.to_f32();
                        if f.is_nan() || f.is_infinite() {
                            return Err(crate::error::TriviumError::InvalidVector {
                                reason:
                                    "向量包含 NaN 或 Infinity (Vector contains NaN or Infinity)"
                                        .into(),
                            });
                        }
                    }
                    let id = sim_next_id;
                    sim_next_id = sim_next_id.checked_add(1).ok_or_else(|| {
                        crate::error::TriviumError::InvalidInput("节点 ID 空间已耗尽".into())
                    })?;
                    node_states.insert(id, true);
                    // 自动 ID 在 WAL 前归一化，应用阶段直接使用已确定的 ID 与原始 JSON。
                    *op = TxOp::InsertWithId {
                        id,
                        vector: std::mem::take(vector),
                        payload: std::mem::take(payload),
                    };
                }
                TxOp::InsertWithId { id, vector, .. } => {
                    if *id == 0 {
                        return Err(crate::error::TriviumError::InvalidInput(
                            "节点 ID 0 为内部保留值".into(),
                        ));
                    }
                    if check_exists!(id) {
                        return Err(crate::error::TriviumError::NodeAlreadyExists(*id));
                    }
                    if vector.len() != dim {
                        return Err(crate::error::TriviumError::DimensionMismatch {
                            expected: dim,
                            got: vector.len(),
                        });
                    }
                    for item in vector.iter() {
                        let f = item.to_f32();
                        if f.is_nan() || f.is_infinite() {
                            return Err(crate::error::TriviumError::InvalidVector {
                                reason:
                                    "向量包含 NaN 或 Infinity (Vector contains NaN or Infinity)"
                                        .into(),
                            });
                        }
                    }
                    node_states.insert(*id, true);
                    if *id >= sim_next_id {
                        sim_next_id = id.checked_add(1).ok_or_else(|| {
                            crate::error::TriviumError::InvalidInput("节点 ID 空间已耗尽".into())
                        })?;
                    }
                }
                TxOp::Link {
                    src, dst, weight, ..
                } => {
                    if !check_exists!(src) {
                        return Err(crate::error::TriviumError::NodeNotFound(*src));
                    }
                    if !check_exists!(dst) {
                        return Err(crate::error::TriviumError::NodeNotFound(*dst));
                    }
                    if !weight.is_finite() {
                        return Err(crate::error::TriviumError::InvalidInput(
                            "边权重必须是有限浮点数 (Edge weight must be finite)".into(),
                        ));
                    }
                }
                TxOp::Delete { id } => {
                    if !check_exists!(id) {
                        return Err(crate::error::TriviumError::NodeNotFound(*id));
                    }
                    node_states.insert(*id, false);
                }
                TxOp::Unlink { src, .. } | TxOp::UnlinkLabel { src, .. } => {
                    if !check_exists!(src) {
                        return Err(crate::error::TriviumError::NodeNotFound(*src));
                    }
                }
                TxOp::UpdatePayload { id, .. } => {
                    if !check_exists!(id) {
                        return Err(crate::error::TriviumError::NodeNotFound(*id));
                    }
                }
                TxOp::UpdateVector { id, vector } => {
                    if !check_exists!(id) {
                        return Err(crate::error::TriviumError::NodeNotFound(*id));
                    }
                    if vector.len() != dim {
                        return Err(crate::error::TriviumError::DimensionMismatch {
                            expected: dim,
                            got: vector.len(),
                        });
                    }
                    for item in vector.iter() {
                        let f = item.to_f32();
                        if f.is_nan() || f.is_infinite() {
                            return Err(crate::error::TriviumError::InvalidVector {
                                reason:
                                    "向量包含 NaN 或 Infinity (Vector contains NaN or Infinity)"
                                        .into(),
                            });
                        }
                    }
                }
            }
        }

        let mut unique_changes =
            std::collections::BTreeMap::<NodeId, Option<&serde_json::Value>>::new();
        for op in &ops {
            match op {
                TxOp::InsertWithId { id, payload, .. } => {
                    unique_changes.insert(*id, Some(payload));
                }
                TxOp::UpdatePayload { id, payload } => {
                    unique_changes.insert(*id, Some(payload));
                }
                TxOp::Delete { id } => {
                    unique_changes.insert(*id, None);
                }
                _ => {}
            }
        }

        let delete_count = ops
            .iter()
            .filter(|op| matches!(op, TxOp::Delete { .. }))
            .count();
        let insert_count = ops
            .iter()
            .filter(|op| matches!(op, TxOp::InsertWithId { .. }))
            .count();
        let mut estimated_bytes = mt.estimate_reserve_bytes(insert_count)?;
        for op in &ops {
            if let TxOp::Link {
                src,
                dst,
                label,
                metadata,
                ..
            } = op
            {
                estimated_bytes = estimated_bytes
                    .saturating_add(mt.estimate_edge_write_bytes(*src, *dst, label, metadata)?);
            }
            let (value, vector_bytes) = match op {
                TxOp::InsertWithId {
                    payload, vector, ..
                } => (
                    Some(payload),
                    vector.len().saturating_mul(std::mem::size_of::<T>()),
                ),
                TxOp::UpdatePayload { payload, .. } => (Some(payload), 0),
                TxOp::Link { metadata, .. } => (Some(metadata), 0),
                TxOp::UpdateVector { vector, .. } => {
                    (None, vector.len().saturating_mul(std::mem::size_of::<T>()))
                }
                _ => (None, 0),
            };
            estimated_bytes = estimated_bytes.saturating_add(vector_bytes);
            if let Some(value) = value {
                let encoded = crate::storage::payload_store::json_encoded_len(value)?;
                if !matches!(op, TxOp::Link { .. }) && encoded > 8 * 1024 * 1024 {
                    return Err(TriviumError::PayloadTooLarge {
                        size_bytes: encoded,
                        max_bytes: 8 * 1024 * 1024,
                    });
                }
                // 同时计入 WAL 编码、原始存储和解析缓存的暂存峰值。
                estimated_bytes = estimated_bytes
                    .saturating_add(encoded.saturating_mul(2))
                    .saturating_add(crate::storage::payload_store::estimate_json_memory(value));
            }
        }
        let current_bytes = mt.estimated_memory_bytes();
        if self.memory_limit > 0
            && current_bytes.saturating_add(estimated_bytes) > self.memory_limit
        {
            return Err(crate::error::TriviumError::CapacityReservationRejected {
                requested_nodes: insert_count,
                estimated_bytes,
                current_bytes,
                memory_limit: self.memory_limit,
            });
        }
        let changes = unique_changes
            .iter()
            .map(|(id, payload)| (*id, *payload))
            .collect::<Vec<_>>();
        let unique_delta = mt.prepare_unique_delta(&changes)?;
        mt.ensure_additional_memory(
            self.memory_limit,
            estimated_bytes.saturating_add(unique_delta.workspace_bytes()),
        )?;
        mt.reserve_unique_delta(&unique_delta)?;
        drop(unique_changes);
        mt.reserve_text_deletes(delete_count)?;
        mt.try_reserve_for_insert(insert_count)?;

        // ════════ 第二阶段：构建 WAL 条目（不触碰 memtable） ════════
        let mut wal_entries: Vec<WalEntry<T>> = Vec::with_capacity(ops.len());
        let mut generated_ids: Vec<NodeId> = Vec::new();

        for op in &ops {
            match op {
                TxOp::Insert { .. } => {
                    return Err(TriviumError::InvalidInput(
                        "事务插入缺少预分配 ID (Transaction insert is missing a pre-assigned ID)"
                            .into(),
                    ));
                }
                TxOp::InsertWithId {
                    id,
                    vector,
                    payload,
                } => {
                    let payload_str = payload.to_string();
                    if payload_str.len() > 8 * 1024 * 1024 {
                        return Err(crate::error::TriviumError::PayloadTooLarge {
                            size_bytes: payload_str.len(),
                            max_bytes: 8 * 1024 * 1024,
                        });
                    }
                    validate_transaction_json(&payload_str)?;
                    generated_ids.push(*id);
                    wal_entries.push(WalEntry::Insert {
                        id: *id,
                        vector: vector.clone(),
                        payload: payload_str,
                    });
                }
                TxOp::Link {
                    src,
                    dst,
                    label,
                    weight,
                    metadata,
                } => {
                    let metadata = metadata.to_string();
                    validate_transaction_json(&metadata)?;
                    wal_entries.push(WalEntry::Link {
                        src: *src,
                        dst: *dst,
                        label: label.clone(),
                        weight: *weight,
                        metadata,
                    });
                }
                TxOp::Delete { id } => {
                    wal_entries.push(WalEntry::Delete { id: *id });
                }
                TxOp::Unlink { src, dst } => {
                    wal_entries.push(WalEntry::Unlink {
                        src: *src,
                        dst: *dst,
                    });
                }
                TxOp::UnlinkLabel { src, dst, label } => {
                    wal_entries.push(WalEntry::UnlinkLabel {
                        src: *src,
                        dst: *dst,
                        label: label.clone(),
                    });
                }
                TxOp::UpdatePayload { id, payload } => {
                    let payload_str = payload.to_string();
                    if payload_str.len() > 8 * 1024 * 1024 {
                        return Err(crate::error::TriviumError::PayloadTooLarge {
                            size_bytes: payload_str.len(),
                            max_bytes: 8 * 1024 * 1024,
                        });
                    }
                    validate_transaction_json(&payload_str)?;
                    wal_entries.push(WalEntry::UpdatePayload {
                        id: *id,
                        payload: payload_str,
                    });
                }
                TxOp::UpdateVector { id, vector } => {
                    wal_entries.push(WalEntry::UpdateVector {
                        id: *id,
                        vector: vector.clone(),
                    });
                }
            }
        }

        // ════════ 第三阶段：先写 WAL（若失败则 memtable 完全未变） ════════
        {
            let mut w = lock_or_recover(&self.wal);
            let tx_id = next_transaction_id(&NEXT_TX_ID)?;
            w.append_batch(tx_id, &wal_entries)?;
        }
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::AfterWalAppend);

        // ════════ 第四阶段：按预检顺序应用原始值，不再解码 WAL JSON ════════
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeMemtableApply);
        // 暂停 QuIVer 增量同步，避免事务中途的 QuIVer 状态需要回滚
        let mut application = ApplicationGuard::new(&mut mt, &self.wal, &self.lifecycle.0);

        let apply_result = (|| -> Result<()> {
            for op in ops {
                match op {
                    TxOp::InsertWithId {
                        id,
                        vector,
                        payload,
                    } => {
                        application.mt.insert_with_id(id, &vector, payload)?;
                    }
                    TxOp::Link {
                        src,
                        dst,
                        label,
                        weight,
                        metadata,
                    } => {
                        application
                            .mt
                            .upsert_edge(src, dst, label, weight, metadata)?;
                    }
                    TxOp::Delete { id } => {
                        application.mt.delete(id)?;
                    }
                    TxOp::Unlink { src, dst } => {
                        application.mt.unlink(src, dst)?;
                    }
                    TxOp::UnlinkLabel { src, dst, label } => {
                        application.mt.unlink_label(src, dst, &label)?;
                    }
                    TxOp::UpdatePayload { id, payload } => {
                        application.mt.update_payload(id, payload)?;
                    }
                    TxOp::UpdateVector { id, vector } => {
                        application.mt.update_vector(id, &vector)?;
                    }
                    TxOp::Insert { .. } => {
                        return Err(TriviumError::InvalidInput(
                        "事务插入缺少预分配 ID (Transaction insert is missing a pre-assigned ID)"
                            .into(),
                    ));
                    }
                }
                #[cfg(feature = "test-hooks")]
                crate::test_hooks::io_result(crate::test_hooks::IoPoint::TransactionApply)?;
            }
            Ok(())
        })();

        // 恢复 QuIVer 同步
        application
            .mt
            .set_quiver_sync_paused(application.previous_pause);
        apply_result?;
        application.mt.apply_unique_delta(unique_delta)?;
        application
            .mt
            .set_unique_updates_paused(application.previous_unique_pause);

        // ════════ 第五阶段：QuIVer 增量同步（分离时间线） ════════
        // 数据应用已完成后再同步索引，避免应用期间触发中间代际的增量维护。
        // 委托给 MemTable 方法处理，避免直接访问私有字段。
        application.mt.quiver_sync_tx_entries(&wal_entries);
        application.complete = true;
        drop(application);

        drop(mt);

        Ok(generated_ids)
    }

    /// 通过 TxBuilder 原子提交事务（FFI 友好入口）
    ///
    /// ```rust,ignore
    /// let mut builder = TxBuilder::new();
    /// builder.insert(&[1.0, 0.0], json!({"name": "Alice"}));
    /// let ids = db.commit_tx(builder)?;
    /// ```
    pub fn commit_tx(&mut self, builder: TxBuilder<T>) -> Result<Vec<NodeId>> {
        self.commit_ops(builder.into_ops())
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;
    use std::sync::{Mutex, atomic::AtomicU64};

    #[cfg(feature = "test-hooks")]
    #[test]
    fn 已准入的共享搜索在事务应用失败后同样拒绝部分状态() {
        use crate::test_hooks::{ConcurrencyPoint, IoPoint, fail_io_at, pause_at};
        for count in [1, 3] {
            let root = std::env::temp_dir()
                .join(format!("tdb-tx-admitted-{count}-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("sample.tdb");
            let mut db = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
            db.insert_with_id(1, &[1., 0.], serde_json::json!({"state":"old"}))
                .unwrap();
            let handle = db.search_handle();
            let barrier = pause_at(ConcurrencyPoint::BeforeSearchReadLock);
            let query = std::thread::spawn(move || {
                handle.search_batch(
                    &vec![vec![1., 0.]; count],
                    &super::super::SearchConfig {
                        top_k: 1,
                        expand_depth: 0,
                        ..Default::default()
                    },
                    &Default::default(),
                )
            });
            barrier.wait_until_arrived();
            let failure = fail_io_at(IoPoint::TransactionApply);
            let mut tx = TxBuilder::new();
            tx.update_payload(1, serde_json::json!({"state":"new"}));
            tx.insert_with_id(2, &[0., 1.], serde_json::json!({}));
            let result = db.commit_tx(tx);
            drop(failure);
            barrier.release();
            let result_query = query.join().unwrap();
            assert!(matches!(result, Err(TriviumError::Io(_))));
            assert!(matches!(result_query, Err(TriviumError::WalClosed)));
            drop(db);
            let mut reopened = Database::<f32>::open(path.to_str().unwrap(), 2).unwrap();
            assert_eq!(reopened.node_count().unwrap(), 2);
            assert_eq!(
                reopened.get_payload(1).unwrap(),
                Some(serde_json::json!({"state":"new"}))
            );
            reopened.close().unwrap();
            drop(reopened);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn 事务编号并发唯一且耗尽后保持拒绝() {
        let counter = AtomicU64::new(1);
        let batches = std::thread::scope(|scope| {
            (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        (0..256)
                            .map(|_| next_transaction_id(&counter).unwrap())
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        let mut ids = batches.into_iter().flatten().collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, (1..=1024).collect::<Vec<_>>());
        let counter = AtomicU64::new(u64::MAX - 1);
        assert_eq!(next_transaction_id(&counter).unwrap(), u64::MAX - 1);
        for _ in 0..2 {
            assert!(matches!(
                next_transaction_id(&counter),
                Err(TriviumError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn 应用guard正常结束和异常展开均恢复原暂停状态() {
        for previous in [false, true] {
            for panicking in [false, true] {
                let root = std::env::temp_dir().join(format!(
                    "tdb-tx-guard-{previous}-{panicking}-{}",
                    std::process::id()
                ));
                std::fs::create_dir_all(&root).unwrap();
                let mut db =
                    Database::<f32>::open(root.join("sample.tdb").to_str().unwrap(), 2).unwrap();
                let wal: &Mutex<_> = &db.wal;
                let mut mt = write_or_recover(&db.memtable);
                mt.set_quiver_sync_paused(previous);
                mt.set_unique_updates_paused(previous);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut guard = ApplicationGuard::new(&mut mt, wal, &db.lifecycle.0);
                    assert!(guard.mt.quiver_sync_paused());
                    assert!(guard.mt.unique_updates_paused());
                    if panicking {
                        panic!("受控事务应用展开");
                    }
                    guard.complete = true;
                }));
                assert_eq!(result.is_err(), panicking);
                assert_eq!(mt.quiver_sync_paused(), previous);
                assert_eq!(mt.unique_updates_paused(), previous);
                assert_eq!(
                    lock_or_recover(&db.lifecycle.0)
                        .recovery_required
                        .load(std::sync::atomic::Ordering::Acquire),
                    panicking
                );
                drop(mt);
                if panicking {
                    assert!(matches!(db.node_count(), Err(TriviumError::WalClosed)));
                } else {
                    db.close().unwrap();
                }
                drop(db);
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }
}
