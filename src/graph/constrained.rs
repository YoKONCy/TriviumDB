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
    if query.len() != db.dim() {
        return Err(TriviumError::DimensionMismatch {
            expected: db.dim(),
            got: query.len(),
        });
    }
    if top_k == 0 || max_anchor_nodes == 0 {
        return Err(TriviumError::InvalidInput(
            "GraphFirst top_k 和 max_anchor_nodes 必须大于 0".into(),
        ));
    }
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
    let mut hits = Vec::with_capacity(anchors.len());
    for id in anchors {
        let Some(vector) = db.get_vector(id) else {
            continue;
        };
        let Some(payload) = db.get_payload(id) else {
            continue;
        };
        let score = T::similarity(query, vector);
        if score.is_finite() {
            hits.push(SearchHit {
                id,
                score,
                payload: payload.clone(),
            });
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.id.cmp(&right.id))
    });
    hits.truncate(top_k);
    Ok(hits)
}
