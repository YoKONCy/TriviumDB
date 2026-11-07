//! 实例内编辑版本与有界持久回执，共用事实锁及原子提交边界。

use crate::resources::{ResourceRuntime, WorkspaceReservation};
use crate::{Result, TriviumError};
use serde::{Serialize, Serializer};
use std::collections::VecDeque;

pub(crate) const RECEIPT_CAPACITY: usize = crate::storage::edit_receipts::MAX_RECEIPTS;
pub(crate) const RECEIPT_BYTES: usize = crate::storage::edit_receipts::MAX_HEAP_BYTES;

#[derive(Clone, Copy)]
pub(crate) struct Revision {
    epoch: uuid::Uuid,
    counter: u64,
}

impl std::fmt::Display for Revision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{:016x}", self.epoch.simple(), self.counter)
    }
}

impl Serialize for Revision {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

pub(crate) struct EditReceipt {
    pub key: String,
    pub expected: String,
    pub fingerprint: Vec<u8>,
    pub ids: Vec<u64>,
    /// None 表示已进入 WAL 提交阶段，但还没有可确认的成功回执。
    pub committed: Option<Revision>,
    pub bytes: usize,
    pub planned: Option<Revision>,
    pub _workspace: Option<WorkspaceReservation>,
}

pub(crate) struct EditState {
    epoch: uuid::Uuid,
    counter: Option<u64>,
    receipts: VecDeque<EditReceipt>,
    receipt_bytes: usize,
    queue_workspace: Option<WorkspaceReservation>,
}

impl Default for EditState {
    fn default() -> Self {
        Self {
            epoch: uuid::Uuid::new_v4(),
            counter: Some(0),
            receipts: VecDeque::new(),
            receipt_bytes: 0,
            queue_workspace: None,
        }
    }
}

impl EditState {
    pub fn next_revision(&self) -> Result<Revision> {
        let mut revision = self.revision()?;
        revision.counter = revision.counter.checked_add(1).ok_or_else(|| {
            TriviumError::InvalidInput("编辑版本空间已耗尽 (Edit revision exhausted)".into())
        })?;
        Ok(revision)
    }

    pub fn restore(&mut self, raw: &[u8]) -> Result<()> {
        let stored = crate::storage::edit_receipts::decode(raw)?;
        let revision = parse_revision(&stored.revision)?;
        parse_revision(&stored.expected)?;
        if let Some(index) = self
            .receipts
            .iter()
            .position(|receipt| receipt.key == stored.key)
            && let Some(old) = self.receipts.remove(index)
        {
            self.receipt_bytes -= old.bytes;
        }
        let bytes = stored.fingerprint.capacity()
            + stored.key.capacity()
            + stored.expected.capacity()
            + stored.ids.capacity() * std::mem::size_of::<u64>()
            + 256;
        if bytes > RECEIPT_BYTES {
            return Err(TriviumError::CorruptedFile(
                "回执超过容量限制 (Receipt exceeds capacity)".into(),
            ));
        }
        while self.receipts.len() >= RECEIPT_CAPACITY || self.receipt_bytes + bytes > RECEIPT_BYTES
        {
            if let Some(old) = self.receipts.pop_front() {
                self.receipt_bytes -= old.bytes;
            }
        }
        self.receipts
            .try_reserve(1)
            .map_err(|error| TriviumError::CapacityAllocationFailed {
                reason: format!("恢复回执分配失败 (Receipt recovery allocation failed): {error}"),
            })?;
        self.receipt_bytes += bytes;
        self.receipts.push_back(EditReceipt {
            key: stored.key,
            expected: stored.expected,
            fingerprint: stored.fingerprint,
            ids: stored.ids,
            committed: Some(revision),
            planned: Some(revision),
            bytes,
            _workspace: None,
        });
        Ok(())
    }

    pub fn checkpoint(&self) -> Result<Vec<u8>> {
        let completed = self
            .receipts
            .iter()
            .filter(|receipt| receipt.committed.is_some());
        let mut bytes = Vec::new();
        let count = completed.clone().count();
        bytes.extend_from_slice(&(count as u16).to_le_bytes());
        for receipt in completed {
            if let Some(revision) = receipt.committed {
                let raw = receipt.encode(&receipt.ids, revision)?;
                bytes.try_reserve(raw.len() + 4).map_err(|error| {
                    TriviumError::CapacityAllocationFailed {
                        reason: format!(
                            "检查点回执分配失败 (Checkpoint receipt allocation failed): {error}"
                        ),
                    }
                })?;
                bytes.extend_from_slice(&(raw.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&raw);
            }
        }
        Ok(bytes)
    }
    pub fn revision(&self) -> Result<Revision> {
        self.counter
            .map(|counter| Revision { epoch: self.epoch, counter })
            .ok_or_else(|| TriviumError::InvalidInput(
                "编辑版本空间已耗尽，请重新打开实例 (Edit revision exhausted; reopen the instance)".into(),
            ))
    }

    pub fn changed(&mut self) {
        // 不能回绕为旧令牌；耗尽后编辑协议保持拒绝，普通事实操作仍遵循原有契约。
        self.counter = self.counter.and_then(|counter| counter.checked_add(1));
    }

    pub fn check(&self, expected: &str) -> Result<()> {
        let actual = self.revision()?.to_string();
        if expected != actual {
            return Err(TriviumError::WriteConflict {
                expected: expected.into(),
                actual,
            });
        }
        Ok(())
    }

    pub fn receipt(&self, key: &str) -> Option<&EditReceipt> {
        self.receipts.iter().find(|receipt| receipt.key == key)
    }

    pub fn queue_bytes(&self) -> usize {
        RECEIPT_CAPACITY * std::mem::size_of::<EditReceipt>()
    }

    pub fn additional_bytes(&self, receipt: &EditReceipt) -> usize {
        receipt.bytes
            + if self.queue_workspace.is_none() {
                self.queue_bytes()
            } else {
                0
            }
    }

    pub fn memory_bytes(&self) -> usize {
        self.receipt_bytes + self.receipts.capacity() * std::mem::size_of::<EditReceipt>()
    }

    /// 在 WAL 之前完成回执登记和全部容器分配，并消耗原版本作为重试隔离点。
    pub fn begin(
        &mut self,
        mut receipt: EditReceipt,
        ids: &[u64],
        runtime: &ResourceRuntime,
    ) -> Result<()> {
        self.check(&receipt.expected)?;
        receipt.planned = Some(self.next_revision()?);
        if self.counter == Some(u64::MAX) {
            return Err(TriviumError::InvalidInput(
                "编辑版本空间已耗尽 (Edit revision exhausted)".into(),
            ));
        }
        if self.receipt(&receipt.key).is_some() {
            return Err(TriviumError::IdempotencyConflict);
        }
        if receipt.bytes > RECEIPT_BYTES {
            return Err(TriviumError::ResourceBudgetExceeded {
                resource: "console_receipt_bytes",
                requested: receipt.bytes,
                used: 0,
                limit: RECEIPT_BYTES,
            });
        }
        if self.queue_workspace.is_none() {
            let mut workspace = runtime.reserve_workspace(self.queue_bytes())?;
            self.receipts.try_reserve_exact(RECEIPT_CAPACITY.saturating_sub(self.receipts.len())).map_err(|error| {
                TriviumError::CapacityAllocationFailed { reason: format!(
                    "事务回执队列分配失败 (Transaction receipt queue allocation failed): {error}"
                ) }
            })?;
            workspace.resize(self.receipts.capacity() * std::mem::size_of::<EditReceipt>())?;
            self.queue_workspace = Some(workspace);
        }
        if ids.len() > receipt.ids.capacity() {
            return Err(TriviumError::InvalidInput(
                "事务回执 ID 预留不足 (Transaction receipt ID reservation is insufficient)".into(),
            ));
        }
        receipt.ids.extend_from_slice(ids);
        while self.receipts.len() >= RECEIPT_CAPACITY
            || self.receipt_bytes + receipt.bytes > RECEIPT_BYTES
        {
            if let Some(old) = self.receipts.pop_front() {
                self.receipt_bytes -= old.bytes;
            }
        }
        self.receipt_bytes += receipt.bytes;
        self.receipts.push_back(receipt);
        self.changed();
        Ok(())
    }

    pub fn complete(&mut self, key: &str) -> Result<()> {
        let receipt = self
            .receipts
            .iter_mut()
            .find(|receipt| receipt.key == key)
            .ok_or(TriviumError::WriteOutcomeUnknown)?;
        let revision = receipt.planned.ok_or(TriviumError::WriteOutcomeUnknown)?;
        receipt.committed = Some(revision);
        self.counter = Some(revision.counter);
        Ok(())
    }
}

impl EditReceipt {
    pub fn encode(&self, ids: &[u64], revision: Revision) -> Result<Vec<u8>> {
        crate::storage::edit_receipts::encode(
            &self.key,
            &self.expected,
            &self.fingerprint,
            ids,
            &revision.to_string(),
        )
    }
}

fn parse_revision(raw: &str) -> Result<Revision> {
    let invalid =
        || TriviumError::CorruptedFile("回执版本令牌无效 (Invalid receipt revision token)".into());
    if raw.len() != 49 || !raw.is_ascii() || raw.as_bytes()[32] != b':' {
        return Err(invalid());
    }
    let epoch = uuid::Uuid::parse_str(&raw[..32]).map_err(|_| invalid())?;
    let counter = u64::from_str_radix(&raw[33..], 16).map_err(|_| invalid())?;
    Ok(Revision { epoch, counter })
}
