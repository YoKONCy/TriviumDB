//! 轻量级事务支持 + WAL 回放
//!
//! 从 database.rs 独立拆分，包含：
//! - `TxOp`: 事务操作类型（内部缓冲用）
//! - `Transaction`: 轻量级事务（Dry-Run 预检 + WAL-first 语义）
//! - `replay_entry`: WAL 崩溃恢复回放

use crate::VectorType;
use crate::database::Database;
use crate::error::Result;
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use crate::storage::wal::WalEntry;

use super::{lock_or_recover, write_or_recover};

/// WAL 崩溃恢复：回放单条 WAL 记录到 MemTable
///
/// 设计要点：
/// - 幂等性：已存在的 ID 会被跳过
/// - 无论是否跳过插入，都推进 `next_id` 防止 ID 复用
pub(crate) fn replay_entry<T: VectorType>(mt: &mut MemTable<T>, entry: WalEntry<T>) {
    match entry {
        WalEntry::Insert {
            id,
            vector,
            payload,
        } => {
            if mt.contains(id) {
                // 幂等：该 ID 已存在（可能来自 .tdb 加载或重复回放），跳过
                tracing::debug!(
                    "WAL 回放跳过已存在的节点 (WAL replay skipped existing node) {}",
                    id
                );
            } else {
                let payload_val: serde_json::Value =
                    serde_json::from_str(&payload).unwrap_or_default();
                let _ = mt.raw_insert(id, &vector, payload_val);
            }
            // 无论是否跳过，都必须推进 next_id 防止后续 insert 复用已物化的 ID
            mt.advance_next_id(id + 1);
        }
        WalEntry::Link {
            src,
            dst,
            label,
            weight,
            metadata,
        } => {
            if mt.contains(src) && mt.contains(dst) {
                let metadata = serde_json::from_str(&metadata).unwrap_or(serde_json::Value::Null);
                let _ = mt.upsert_edge(src, dst, label, weight, metadata);
            }
        }
        WalEntry::Delete { id } => {
            if mt.contains(id) {
                let _ = mt.delete(id);
            }
        }
        WalEntry::Unlink { src, dst } => {
            if mt.contains(src) {
                let _ = mt.unlink(src, dst);
            }
        }
        WalEntry::UnlinkLabel { src, dst, label } => {
            if mt.contains(src) {
                let _ = mt.unlink_label(src, dst, &label);
            }
        }
        WalEntry::UpdatePayload { id, payload } => {
            if mt.contains(id) {
                let payload_val: serde_json::Value =
                    serde_json::from_str(&payload).unwrap_or_default();
                let _ = mt.update_payload(id, payload_val);
            }
        }
        WalEntry::UpdateVector { id, vector } => {
            if mt.contains(id) {
                let _ = mt.update_vector(id, &vector);
            }
        }
        WalEntry::TxBegin { .. } | WalEntry::TxCommit { .. } => {
            // 已在 wal.rs 内的回放过滤环节处理，这里不应再收到，直接忽略
        }
    }
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
/// - `commit()` → 一次性持有锁，按顺序应用到 memtable + WAL，任何一步失败则回滚
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
    ///   1. Dry-Run 预检：虚拟状态验证 + 预分配 ID
    ///   2. 构建 WAL 条目（不触碰 memtable）
    ///   3. 先写 WAL（若失败则 memtable 完全未变，安全回滚）
    ///   4. 再应用到 memtable（Infallible，干跑已排除所有异常）
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
    pub(crate) fn commit_ops(&mut self, ops: Vec<TxOp<T>>) -> Result<Vec<NodeId>> {
        let _operation = self.enter_operation()?;
        self.require_write_access("transaction commit")?;
        if ops.is_empty() {
            return Ok(Vec::new());
        }

        let mut mt = write_or_recover(&self.memtable);

        // ════════ 第一阶段：预检前置 (Dry-Run) + 预分配 ID ════════
        let mut sim_next_id = mt.next_id_value();
        let dim = mt.dim();
        let mut pending_ids = std::collections::HashSet::new();
        let mut pending_deletes = std::collections::HashSet::new();
        let mut pre_assigned_ids: Vec<Option<NodeId>> = Vec::with_capacity(ops.len());

        macro_rules! check_exists {
            ($id:expr) => {
                !pending_deletes.contains($id) && (pending_ids.contains($id) || mt.contains(*$id))
            };
        }

        for op in &ops {
            match op {
                TxOp::Insert { vector, .. } => {
                    if vector.len() != dim {
                        return Err(crate::error::TriviumError::DimensionMismatch {
                            expected: dim,
                            got: vector.len(),
                        });
                    }
                    for item in vector {
                        let f = item.to_f32();
                        if f.is_nan() || f.is_infinite() {
                            return Err(crate::error::TriviumError::InvalidVector {
                                reason:
                                    "向量包含 NaN 或 Infinity (Vector contains NaN or Infinity)"
                                        .into(),
                            });
                        }
                    }
                    pre_assigned_ids.push(Some(sim_next_id));
                    pending_ids.insert(sim_next_id);
                    sim_next_id = sim_next_id.checked_add(1).ok_or_else(|| {
                        crate::error::TriviumError::InvalidInput("节点 ID 空间已耗尽".into())
                    })?;
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
                    for item in vector {
                        let f = item.to_f32();
                        if f.is_nan() || f.is_infinite() {
                            return Err(crate::error::TriviumError::InvalidVector {
                                reason:
                                    "向量包含 NaN 或 Infinity (Vector contains NaN or Infinity)"
                                        .into(),
                            });
                        }
                    }
                    pre_assigned_ids.push(Some(*id));
                    pending_ids.insert(*id);
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
                    pre_assigned_ids.push(None);
                }
                TxOp::Delete { id } => {
                    if !check_exists!(id) {
                        return Err(crate::error::TriviumError::NodeNotFound(*id));
                    }
                    pending_deletes.insert(*id);
                    pre_assigned_ids.push(None);
                }
                TxOp::Unlink { src, .. } | TxOp::UnlinkLabel { src, .. } => {
                    if !check_exists!(src) {
                        return Err(crate::error::TriviumError::NodeNotFound(*src));
                    }
                    pre_assigned_ids.push(None);
                }
                TxOp::UpdatePayload { id, .. } => {
                    if !check_exists!(id) {
                        return Err(crate::error::TriviumError::NodeNotFound(*id));
                    }
                    pre_assigned_ids.push(None);
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
                    for item in vector {
                        let f = item.to_f32();
                        if f.is_nan() || f.is_infinite() {
                            return Err(crate::error::TriviumError::InvalidVector {
                                reason:
                                    "向量包含 NaN 或 Infinity (Vector contains NaN or Infinity)"
                                        .into(),
                            });
                        }
                    }
                    pre_assigned_ids.push(None);
                }
            }
        }

        let mut unique_changes =
            std::collections::BTreeMap::<NodeId, Option<serde_json::Value>>::new();
        for (index, op) in ops.iter().enumerate() {
            match op {
                TxOp::Insert { payload, .. } | TxOp::InsertWithId { payload, .. } => {
                    let id = pre_assigned_ids.get(index).copied().flatten().ok_or_else(|| {
                        crate::error::TriviumError::InvalidInput(
                            "事务插入缺少预分配 ID (Transaction insert is missing a pre-assigned ID)"
                                .into(),
                        )
                    })?;
                    unique_changes.insert(id, Some(payload.clone()));
                }
                TxOp::UpdatePayload { id, payload } => {
                    unique_changes.insert(*id, Some(payload.clone()));
                }
                TxOp::Delete { id } => {
                    unique_changes.insert(*id, None);
                }
                _ => {}
            }
        }
        mt.validate_unique_changes(
            unique_changes
                .iter()
                .map(|(id, payload)| (*id, payload.as_ref())),
        )?;

        let insert_count = pre_assigned_ids.iter().filter(|id| id.is_some()).count();
        let estimated_bytes = mt.estimate_reserve_bytes(insert_count)?;
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
        mt.try_reserve_for_insert(insert_count)?;

        // ════════ 第二阶段：构建 WAL 条目（不触碰 memtable） ════════
        let mut wal_entries: Vec<WalEntry<T>> = Vec::with_capacity(ops.len());
        let mut generated_ids: Vec<NodeId> = Vec::new();

        for (i, op) in ops.iter().enumerate() {
            match op {
                TxOp::Insert { vector, payload } => {
                    let id = pre_assigned_ids.get(i).copied().flatten().ok_or_else(|| {
                        crate::error::TriviumError::InvalidInput(
                            "事务插入缺少预分配 ID (Transaction insert is missing a pre-assigned ID)"
                                .into(),
                        )
                    })?;
                    let payload_str = payload.to_string();
                    if payload_str.len() > 8 * 1024 * 1024 {
                        return Err(crate::error::TriviumError::PayloadTooLarge {
                            size_bytes: payload_str.len(),
                            max_bytes: 8 * 1024 * 1024,
                        });
                    }
                    generated_ids.push(id);
                    wal_entries.push(WalEntry::Insert {
                        id,
                        vector: vector.clone(),
                        payload: payload_str,
                    });
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
                    wal_entries.push(WalEntry::Link {
                        src: *src,
                        dst: *dst,
                        label: label.clone(),
                        weight: *weight,
                        metadata: metadata.to_string(),
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
            let tx_id = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            w.append_batch(tx_id, &wal_entries)?;
        }
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::AfterWalAppend);

        // ════════ 第四阶段：应用到 memtable（Infallible Apply） ════════
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeMemtableApply);
        // 暂停 QuIVer 增量同步，避免事务中途的 QuIVer 状态需要回滚
        mt.set_quiver_sync_paused(true);

        for entry in &wal_entries {
            match entry {
                WalEntry::Insert {
                    id,
                    vector,
                    payload,
                } => {
                    let payload_val: serde_json::Value =
                        serde_json::from_str(payload).unwrap_or_default();
                    let _ = mt.insert_with_id(*id, vector, payload_val);
                }
                WalEntry::Link {
                    src,
                    dst,
                    label,
                    weight,
                    metadata,
                } => {
                    let metadata =
                        serde_json::from_str(metadata).unwrap_or(serde_json::Value::Null);
                    let _ = mt.upsert_edge(*src, *dst, label.clone(), *weight, metadata);
                }
                WalEntry::Delete { id } => {
                    let _ = mt.delete(*id);
                }
                WalEntry::Unlink { src, dst } => {
                    let _ = mt.unlink(*src, *dst);
                }
                WalEntry::UnlinkLabel { src, dst, label } => {
                    let _ = mt.unlink_label(*src, *dst, label);
                }
                WalEntry::UpdatePayload { id, payload } => {
                    let payload_val: serde_json::Value =
                        serde_json::from_str(payload).unwrap_or_default();
                    let _ = mt.update_payload(*id, payload_val);
                }
                WalEntry::UpdateVector { id, vector } => {
                    let _ = mt.update_vector(*id, vector);
                }
                _ => {}
            }
        }

        // 恢复 QuIVer 同步
        mt.set_quiver_sync_paused(false);

        // ════════ 第五阶段：QuIVer 增量同步（分离时间线） ════════
        // Phase 4 已成功完成（Infallible），此处不需要回滚能力。
        // 委托给 MemTable 方法处理，避免直接访问私有字段。
        mt.quiver_sync_tx_entries(&wal_entries);

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
