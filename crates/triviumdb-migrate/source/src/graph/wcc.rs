//! 弱连通分量 (Weakly Connected Components)
//!
//! 将有向图视为无向图，使用 Union-Find（路径压缩 + 按秩合并）进行分量识别。
//!
//! - 时间复杂度：O(V + E · α(V))，近似线性
//! - 用途：发现孤立子图、事件簇、数据孤岛

use crate::VectorType;
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use std::collections::HashMap;

/// Union-Find 数据结构（路径压缩 + 按秩合并）
struct UnionFind {
    parent: HashMap<NodeId, NodeId>,
    rank: HashMap<NodeId, usize>,
}

impl UnionFind {
    fn new(nodes: &[NodeId]) -> Self {
        let mut parent = HashMap::with_capacity(nodes.len());
        let mut rank = HashMap::with_capacity(nodes.len());
        for &id in nodes {
            parent.insert(id, id);
            rank.insert(id, 0);
        }
        Self { parent, rank }
    }

    fn find(&mut self, x: NodeId) -> NodeId {
        let p = self.parent[&x];
        if p != x {
            let root = self.find(p);
            self.parent.insert(x, root);
            root
        } else {
            x
        }
    }

    fn union(&mut self, a: NodeId, b: NodeId) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        let rank_a = self.rank[&ra];
        let rank_b = self.rank[&rb];
        if rank_a < rank_b {
            self.parent.insert(ra, rb);
        } else if rank_a > rank_b {
            self.parent.insert(rb, ra);
        } else {
            self.parent.insert(rb, ra);
            if let Some(r) = self.rank.get_mut(&ra) {
                *r += 1;
            }
        }
    }
}

/// 弱连通分量：将图视为无向图，返回每个连通分量的节点集合
///
/// 返回值按分量大小降序排列（最大分量在前）。
pub fn weakly_connected_components<T: VectorType>(mt: &MemTable<T>) -> Vec<Vec<NodeId>> {
    let all_ids = mt.all_node_ids();
    if all_ids.is_empty() {
        return Vec::new();
    }

    let mut uf = UnionFind::new(&all_ids);

    for &src in &all_ids {
        if let Some(edges) = mt.get_edges(src) {
            for edge in edges {
                uf.union(src, edge.target_id);
            }
        }
    }

    let mut components: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for &id in &all_ids {
        let root = uf.find(id);
        components.entry(root).or_default().push(id);
    }

    let mut result: Vec<Vec<NodeId>> = components.into_values().collect();
    result.sort_by(|a, b| b.len().cmp(&a.len()));
    result
}
