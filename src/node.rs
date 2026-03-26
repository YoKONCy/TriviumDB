use serde::{Deserialize, Serialize};

pub type NodeId = u64;
pub type Label = String;
pub type Weight = f32;

/// 图谱层：表示两个节点之间有向带权边
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub target_id: NodeId,
    pub label: Label,
    pub weight: Weight,
}

/// 用户在查询时返回的统一节点数据视图
#[derive(Debug, Clone)]
pub struct NodeView<T> {
    pub id: NodeId,
    pub vector: Vec<T>,
    pub payload: serde_json::Value,
    pub edges: Vec<Edge>,
}

/// Node 是 NodeView 的别名，用于图谱查询引擎
pub type Node<T> = NodeView<T>;

/// 查询命中时的返回结构
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: NodeId,
    pub score: f32, // 查询得分（例如 Cosine Similarity 或图扩散热度）
    pub payload: serde_json::Value,
}
