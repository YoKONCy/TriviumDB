//! TQL 快照与返回阶段共用的查询峰值准入，凭证随最外层执行作用域释放。
use super::{Node, NodeId, TriviumError, VectorType};
use crate::resources::{ResourceRuntime, WorkspaceReservation};
use crate::storage::memtable::MemTable;
use std::cell::{Cell, RefCell};

pub(super) struct QueryMemory {
    runtime: ResourceRuntime,
    limit: usize,
    bytes: Cell<usize>,
    leases: RefCell<Vec<WorkspaceReservation>>,
}

pub(super) struct Temporary<'a> {
    owner: &'a QueryMemory,
    bytes: usize,
    lease: Option<WorkspaceReservation>,
}

impl Drop for Temporary<'_> {
    fn drop(&mut self) {
        self.owner.bytes.set(self.owner.bytes.get() - self.bytes);
    }
}

pub(super) fn overflow() -> TriviumError {
    TriviumError::CapacityAllocationFailed {
        reason: "TQL 物化工作区估算溢出 (TQL materialization workspace estimate overflowed)".into(),
    }
}

pub(super) fn node_bytes<T: VectorType>(id: NodeId, mt: &MemTable<T>) -> crate::Result<usize> {
    let mut bytes = mt
        .payload_query_peak(id)?
        .checked_add(
            mt.dim()
                .checked_mul(std::mem::size_of::<T>())
                .ok_or_else(overflow)?,
        )
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Node<T>>() + 256))
        .ok_or_else(overflow)?;
    for edge in mt.get_edges(id).unwrap_or(&[]) {
        bytes = bytes
            .checked_add(std::mem::size_of_val(edge) * 2)
            .and_then(|bytes| bytes.checked_add(edge.label.len()))
            .and_then(|bytes| {
                bytes.checked_add(crate::storage::payload_store::query_clone_peak(
                    &edge.metadata,
                ))
            })
            .ok_or_else(overflow)?;
    }
    Ok(bytes)
}

pub(super) fn snapshot_bytes<T: VectorType>(node: &Node<T>) -> crate::Result<usize> {
    let mut bytes = crate::storage::payload_store::query_clone_peak(&node.payload)
        .checked_add(
            node.vector
                .len()
                .checked_mul(std::mem::size_of::<T>())
                .ok_or_else(overflow)?,
        )
        .and_then(|v| v.checked_add(std::mem::size_of::<Node<T>>() + 256))
        .ok_or_else(overflow)?;
    for edge in &node.edges {
        bytes = bytes
            .checked_add(std::mem::size_of_val(edge) * 2)
            .and_then(|v| v.checked_add(edge.label.len()))
            .and_then(|v| {
                v.checked_add(crate::storage::payload_store::query_clone_peak(
                    &edge.metadata,
                ))
            })
            .ok_or_else(overflow)?;
    }
    Ok(bytes)
}

impl QueryMemory {
    pub(super) fn new<T: VectorType>(limit: usize, mt: &MemTable<T>) -> Self {
        Self {
            runtime: mt.resource_runtime().clone(),
            limit,
            bytes: Cell::new(0),
            leases: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn retain(&self, bytes: usize) -> crate::Result<()> {
        let mut allocation = self.temporary(bytes)?;
        if let Some(lease) = allocation.lease.take() {
            self.leases.borrow_mut().push(lease);
        }
        // 凭证转移给查询作用域，临时句柄不再归还已转移的计数。
        allocation.bytes = 0;
        Ok(())
    }

    pub(super) fn temporary(&self, bytes: usize) -> crate::Result<Temporary<'_>> {
        let used = self.bytes.get();
        let projected = used.checked_add(bytes).ok_or_else(overflow)?;
        if self.limit > 0 && projected > self.limit {
            return Err(TriviumError::ResourceBudgetExceeded {
                resource: "query_memory_bytes",
                requested: bytes,
                used,
                limit: self.limit,
            });
        }
        let lease = self.runtime.reserve_workspace(bytes)?;
        self.bytes.set(projected);
        Ok(Temporary {
            owner: self,
            bytes,
            lease: Some(lease),
        })
    }

    pub(super) fn snapshot<T: VectorType>(
        &self,
        id: NodeId,
        mt: &MemTable<T>,
    ) -> crate::Result<Node<T>> {
        self.retain(node_bytes(id, mt)?)?;
        let payload = mt
            .try_get_payload(id)?
            .ok_or(TriviumError::NodeNotFound(id))?;
        super::build_node_with_payload(id, payload, mt).ok_or_else(|| {
            TriviumError::CorruptedFile(
                "TQL 活跃节点缺少向量 (TQL active node has no vector)".into(),
            )
        })
    }
}
