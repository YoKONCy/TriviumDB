//! 三模数据模型的公共节点、边与查询结果类型。
//!
//! NodeId 是跨向量、Payload 和图关系的唯一身份；Edge 以 `(src,dst,label)` 形成业务
//! 唯一键。公共视图使用拥有型数据隔离内部 slot/mmap 生命周期，避免绑定层持有悬垂引用。

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
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// 指向目标节点的完整入边视图。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IncomingEdge {
    pub source_id: NodeId,
    pub target_id: NodeId,
    pub label: Label,
    pub weight: Weight,
    pub metadata: serde_json::Value,
}

/// 查询投影使用的拥有型边视图。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EdgeView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_id: Option<String>,
    pub source_id: NodeId,
    pub target_id: NodeId,
    pub label: Label,
    pub weight: Weight,
    pub metadata: serde_json::Value,
}

impl EdgeView {
    /// 传输层提供自身的无损ID表示，图身份随完整边内容传递。
    pub(crate) fn query_json(
        &self,
        source: serde_json::Value,
        target: serde_json::Value,
    ) -> serde_json::Value {
        let mut value = serde_json::json!({"type":"edge", "source":source, "target":target, "label":self.label, "weight":self.weight, "metadata":self.metadata});
        if let Some(graph_id) = &self.graph_id {
            value["graph_id"] = graph_id.clone().into();
        }
        value
    }
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

#[derive(Debug, Clone, Default)]
pub struct GroupedSearchResult {
    pub semantic_hits: Vec<SearchHit>,
    pub graph_hits: Vec<SearchHit>,
}
