//! GraphFirst 候选集合内的精确向量排名门面。
//!
//! 图模式先产生合法 anchor，本模块负责维度、候选预算与节点存在性检查，再委托 exact
//! Top-K 执行器评分。它绝不从全库补充近邻，因此图约束是硬边界；候选超过预算时在
//! 分配和打分前 fail-closed。

use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::node::{NodeId, SearchHit};
use crate::storage::memtable::MemTable;

pub fn rank_within<T: VectorType>(
    db: &MemTable<T>,
    query: &[T],
    anchor_ids: &[NodeId],
    top_k: usize,
    max_anchor_nodes: usize,
) -> Result<Vec<SearchHit>> {
    db.ensure_readable()?;
    crate::vector::validate_query_vector(query, db.dim())?;
    if top_k == 0 || max_anchor_nodes == 0 {
        return Err(TriviumError::InvalidInput(
            "GraphFirst top_k 和 max_anchor_nodes 必须大于 0".into(),
        ));
    }
    let bytes = anchor_ids
        .len()
        .checked_mul(std::mem::size_of::<NodeId>() * 2)
        .ok_or_else(|| TriviumError::CapacityAllocationFailed {
            reason: "GraphFirst anchor 工作区溢出 (GraphFirst anchor workspace overflowed)".into(),
        })?;
    let _workspace = db.resource_runtime().reserve_workspace(bytes)?;
    let mut anchors = anchor_ids.to_vec();
    anchors.sort_unstable();
    anchors.dedup();
    if anchors.len() > max_anchor_nodes {
        return Err(TriviumError::QueryExecution(format!(
            "GraphFirst anchor 数量 {} 超过预算 {}",
            anchors.len(),
            max_anchor_nodes
        )));
    }
    crate::index::exact::rank_unique_ids(db, query, &anchors, top_k)
}
