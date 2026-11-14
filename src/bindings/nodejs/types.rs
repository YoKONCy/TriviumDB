//! 查询结果与 Hook 上下文的语言数据类型。

use super::arguments::node_id_to_js;
use super::json::NodeJson;
use napi::bindgen_prelude::Either;
use napi::{Env, JsUnknown};
use napi_derive::napi;

// ════════ JS 侧返回结构体 ════════

/// 向量检索命中结果
#[napi(object)]
pub struct JsSearchHit {
    /// 节点 ID（JS Number，安全范围内的 u64）
    pub id: f64,
    /// 相似度得分
    pub score: f64,
    /// 节点元数据（JSON 对象）
    pub payload: NodeJson,
}

#[napi(object)]
pub struct JsGroupedSearchResult {
    pub semantic_hits: Vec<JsSearchHit>,
    pub graph_hits: Vec<JsSearchHit>,
}

pub(super) fn round_api_f32(value: f32) -> f64 {
    ((value as f64) * 1_000_000.0).round() / 1_000_000.0
}

pub(super) fn search_hit_to_js(env: Env, hit: crate::node::SearchHit) -> napi::Result<JsSearchHit> {
    Ok(JsSearchHit {
        id: node_id_to_js(env, hit.id)?,
        score: hit.score as f64,
        payload: hit.payload.into(),
    })
}

#[napi(object)]
pub struct JsReachabilityStep {
    pub from: f64,
    pub to: f64,
    pub label: String,
    pub weight: f64,
    pub metadata: NodeJson,
}

#[napi(object)]
pub struct JsReachabilityResult {
    pub source_id: f64,
    pub target_id: f64,
    pub depth: u32,
    pub path: Vec<f64>,
    pub steps: Vec<JsReachabilityStep>,
}

#[napi(object)]
pub struct JsReachabilityOutput {
    pub results: Vec<JsReachabilityResult>,
    pub visited_nodes: f64,
    pub traversed_edges: f64,
    pub truncated: bool,
}

#[napi(object)]
pub struct JsSubgraphNode {
    pub id: f64,
    pub payload: NodeJson,
}

#[napi(object)]
pub struct JsSubgraphEdge {
    pub source_id: f64,
    pub target_id: f64,
    pub label: String,
    pub weight: f64,
    pub metadata: NodeJson,
}

#[napi(object)]
pub struct JsSubgraphResult {
    pub nodes: Vec<JsSubgraphNode>,
    pub edges: Vec<JsSubgraphEdge>,
    pub visited_nodes: f64,
    pub traversed_edges: f64,
    pub truncated: bool,
}

/// 数据库打开与容量规划配置。
#[napi(object)]
pub struct JsDatabaseOptions {
    pub runtime: Option<JsUnknown>,
    pub dim: Option<Either<f64, JsUnknown>>,
    pub dtype: Option<JsUnknown>,
    pub sync_mode: Option<JsUnknown>,
    pub storage_mode: Option<JsUnknown>,
    pub auto_build_quiver: Option<Either<bool, JsUnknown>>,
    pub load_text_index: Option<Either<bool, JsUnknown>>,
    pub expected_nodes: Option<Either<f64, JsUnknown>>,
    pub memory_limit_mb: Option<Either<f64, JsUnknown>>,
    pub payload_cache_mb: Option<Either<f64, JsUnknown>>,
    pub payload_cache_entry_mb: Option<Either<f64, JsUnknown>>,
    pub access_mode: Option<JsUnknown>,
    pub missing_index_policy: Option<JsUnknown>,
    pub max_query_rows: Option<Either<f64, JsUnknown>>,
    pub row_overflow: Option<JsUnknown>,
}

/// 节点关系边
#[napi(object)]
pub struct JsEdge {
    pub target_id: f64,
    pub label: String,
    pub weight: f64,
    pub metadata: NodeJson,
}

/// 完整入边视图
#[napi(object)]
pub struct JsIncomingEdge {
    pub source_id: f64,
    pub target_id: f64,
    pub label: String,
    pub weight: f64,
    pub metadata: NodeJson,
}

/// 节点完整视图
#[napi(object)]
pub struct JsNodeView {
    pub id: f64,
    pub vector: Vec<f64>,
    pub payload: NodeJson,
    pub edges: Vec<JsEdge>,
    pub num_edges: u32,
}

/// Leiden 聚类结果结构
#[napi(object)]
pub struct JsClusterResult {
    /// 平铺数组: [nodeId1, clusterId1, nodeId2, clusterId2, ...]
    pub node_to_cluster: Vec<f64>,
    /// 平铺数组: [clusterId1, "label1", ...]
    pub cluster_labels: Vec<String>,
    /// 平铺首尾连接数组: [clusterId1, vector[0]...vector[dim], clusterId2, ...]
    pub centroids: Vec<f64>,
}

/// Leiden 聚类配置 (全部可选)
#[napi(object)]
pub struct JsLeidenConfig {
    /// 最小社区大小 (节点数 < 此值的碎片簇被丢弃, 默认 3)
    pub min_community_size: Option<f64>,
    /// 最大迭代轮次 (默认 15)
    pub max_iterations: Option<f64>,
    /// 是否计算质心 (默认 true)
    pub with_centroids: Option<bool>,
}

/// Hook 管线执行上下文（包含各阶段计时统计和自定义数据）
#[napi(object)]
pub struct JsHookContext {
    /// 各管线阶段的耗时统计（JSON 对象, 单位: 毫秒）
    pub timings: NodeJson,
    /// 每阶段候选数量
    pub counts: NodeJson,
    /// Hook 注入的自定义数据
    pub custom_data: NodeJson,
    pub observations: NodeJson,
    /// 管线是否被 Hook 提前终止
    pub aborted: bool,
}

/// 带上下文的检索结果
#[napi(object)]
pub struct JsSearchWithContextResult {
    /// 检索结果列表
    pub hits: Vec<JsSearchHit>,
    /// Hook 管线上下文
    pub context: JsHookContext,
}

pub(super) fn to_js_reachability(
    env: Env,
    result: crate::graph::reachability::ReachabilityResult,
) -> napi::Result<JsReachabilityResult> {
    Ok(JsReachabilityResult {
        source_id: node_id_to_js(env, result.source_id)?,
        target_id: node_id_to_js(env, result.target_id)?,
        depth: result.depth as u32,
        path: result
            .path
            .into_iter()
            .map(|value| node_id_to_js(env, value))
            .collect::<napi::Result<_>>()?,
        steps: result
            .steps
            .into_iter()
            .map(|step| {
                Ok(JsReachabilityStep {
                    from: node_id_to_js(env, step.from)?,
                    to: node_id_to_js(env, step.to)?,
                    label: step.label,
                    weight: round_api_f32(step.weight),
                    metadata: step.metadata.into(),
                })
            })
            .collect::<napi::Result<_>>()?,
    })
}
