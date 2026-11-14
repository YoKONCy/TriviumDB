//! 边、遍历、子图、图统计与聚类入口。

use super::arguments::{
    argument_object, node_id_to_js, optional_bool, parse_search_usize, parse_usize_argument,
    raw_string, read_array, read_string, write_id, write_json, write_number, write_string,
};
use super::database::TriviumDB;
use super::errors::to_napi_error;
use super::json::{NodeJson, node_json_value};
use super::options::parse_reachability_options;
use super::types::{
    JsClusterResult, JsEdge, JsIncomingEdge, JsReachabilityOutput, JsReachabilityResult,
    JsSubgraphEdge, JsSubgraphNode, JsSubgraphResult, round_api_f32, to_js_reachability,
};
use napi::bindgen_prelude::Either;
use napi::{Env, JsUnknown};
use napi_derive::napi;

#[napi]
impl TriviumDB {
    #[napi]
    pub fn graph_stats(&self, env: Env) -> napi::Result<NodeJson> {
        node_json_value(
            dispatch!(self, db => db.graph_stats().map_err(|error| to_napi_error(env, error))?),
        )
        .map_err(|error| napi::Error::from_reason(error.to_string()))
    }

    #[napi]
    pub fn validate_graph(&self, env: Env) -> napi::Result<NodeJson> {
        node_json_value(
            dispatch!(self, db => db.validate_graph().map_err(|error| to_napi_error(env, error))?),
        )
        .map_err(|error| napi::Error::from_reason(error.to_string()))
    }

    #[napi]
    pub fn repair_graph_indexes(&mut self, env: Env) -> napi::Result<NodeJson> {
        let report = dispatch!(self, mut db => db.repair_graph_indexes())
            .map_err(|error| to_napi_error(env, error))?;
        node_json_value(report).map_err(|error| napi::Error::from_reason(error.to_string()))
    }

    // ── 图谱操作 ──

    /// 建立有向带权边
    #[napi]
    pub fn link(
        &mut self,
        env: Env,
        src: JsUnknown,
        dst: JsUnknown,
        label: Option<JsUnknown>,
        weight: Option<JsUnknown>,
    ) -> napi::Result<()> {
        let src = write_id(env, src, "src")?;
        let dst = write_id(env, dst, "dst")?;
        let label = label
            .map(|value| write_string(env, value, "label"))
            .transpose()?
            .unwrap_or_else(|| "related".into());
        let weight = weight
            .map(|value| write_number(env, value, "weight"))
            .transpose()?
            .unwrap_or(1.0) as f32;
        dispatch!(self, mut db => db.link(src, dst, &label, weight))
            .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn get_edge(
        &self,
        env: Env,
        src: Either<f64, JsUnknown>,
        dst: Either<f64, JsUnknown>,
        label: JsUnknown,
    ) -> napi::Result<Option<JsEdge>> {
        let src = parse_usize_argument(env, src, "src", None)? as u64;
        let dst = parse_usize_argument(env, dst, "dst", None)? as u64;
        let label = raw_string(env, label, "label", None)?;
        Ok({
            dispatch!(self, db => db.get_edge(src, dst, &label).map_err(|error| to_napi_error(env, error))?).map(|edge| Ok::<_,napi::Error>(JsEdge {
                target_id: node_id_to_js(env, edge.target_id)?,
                label: edge.label,
                weight: round_api_f32(edge.weight),
                metadata: edge.metadata.into(),
            })).transpose()?
        })
    }

    #[napi]
    pub fn upsert_edge(
        &mut self,
        env: Env,
        src: JsUnknown,
        dst: JsUnknown,
        label: JsUnknown,
        weight: JsUnknown,
        metadata: Option<JsUnknown>,
    ) -> napi::Result<()> {
        let src = write_id(env, src, "src")?;
        let dst = write_id(env, dst, "dst")?;
        let label = write_string(env, label, "label")?;
        let weight = write_number(env, weight, "weight")?;
        let metadata = metadata
            .map(|value| write_json(env, value, "metadata"))
            .transpose()?;
        dispatch!(self, mut db => db.upsert_edge(
            src,
            dst,
            &label,
            weight as f32,
            metadata.unwrap_or(serde_json::Value::Null),
        ))
        .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn update_edge(
        &mut self,
        env: Env,
        src: JsUnknown,
        dst: JsUnknown,
        label: JsUnknown,
        weight: Option<JsUnknown>,
        metadata: Option<JsUnknown>,
    ) -> napi::Result<()> {
        let src = write_id(env, src, "src")?;
        let dst = write_id(env, dst, "dst")?;
        let label = write_string(env, label, "label")?;
        let weight = weight
            .map(|value| write_number(env, value, "weight"))
            .transpose()?;
        let metadata = metadata
            .map(|value| write_json(env, value, "metadata"))
            .transpose()?;
        dispatch!(self, mut db => db.update_edge(
            src,
            dst,
            &label,
            weight.map(|value| value as f32),
            metadata,
        ))
        .map_err(|error| to_napi_error(env, error))
    }

    /// 断开两节点间的边；提供 label 时仅删除该标签。
    #[napi]
    pub fn unlink(
        &mut self,
        env: Env,
        src: JsUnknown,
        dst: JsUnknown,
        label: Option<JsUnknown>,
    ) -> napi::Result<()> {
        let src = write_id(env, src, "src")?;
        let dst = write_id(env, dst, "dst")?;
        let label = label
            .map(|value| write_string(env, value, "label"))
            .transpose()?;
        match label {
            Some(label) => {
                dispatch!(self, mut db => db.unlink_label(src, dst, &label))
            }
            None => dispatch!(self, mut db => db.unlink(src, dst)),
        }
        .map_err(|error| to_napi_error(env, error))
    }

    /// 获取 N 跳邻居节点 ID 列表，可按标签白名单遍历。
    #[napi]
    pub fn neighbors(
        &self,
        env: Env,
        id: Either<f64, JsUnknown>,
        depth: Option<Either<f64, JsUnknown>>,
        labels: Option<JsUnknown>,
    ) -> napi::Result<Vec<f64>> {
        let labels = labels
            .map(|value| read_array(env, value, "labels", read_string))
            .transpose()?;
        Ok({
            let depth = parse_search_usize(env, depth, 1.0, "depth", false)?;
            let id = parse_usize_argument(env, id, "id", None)? as u64;
            dispatch!(self, db => db.neighbors_with_labels(id, depth, labels.as_deref()).map_err(|error| to_napi_error(env, error))?)
                .into_iter()
                .map(|value| node_id_to_js(env, value)).collect::<napi::Result<_>>()?
        })
    }

    #[napi]
    pub fn reachable(
        &self,
        env: Env,
        id: Either<f64, JsUnknown>,
        options: Option<JsUnknown>,
    ) -> napi::Result<Vec<JsReachabilityResult>> {
        let config = parse_reachability_options(env, options)?;
        let id = parse_usize_argument(env, id, "id", None)? as u64;
        dispatch!(self, db => db.reachable(id, &config))
            .map_err(|error| to_napi_error(env, error))?
            .into_iter()
            .map(|value| to_js_reachability(env, value))
            .collect()
    }

    #[napi]
    pub fn reachable_detailed(
        &self,
        env: Env,
        id: Either<f64, JsUnknown>,
        options: Option<JsUnknown>,
    ) -> napi::Result<JsReachabilityOutput> {
        let config = parse_reachability_options(env, options)?;
        let id = parse_usize_argument(env, id, "id", None)? as u64;
        let output = dispatch!(self, db => db.reachable_detailed(id, &config))
            .map_err(|error| to_napi_error(env, error))?;
        Ok(JsReachabilityOutput {
            results: output
                .results
                .into_iter()
                .map(|value| to_js_reachability(env, value))
                .collect::<napi::Result<_>>()?,
            visited_nodes: output.visited_nodes as f64,
            traversed_edges: output.traversed_edges as f64,
            truncated: output.truncated,
        })
    }

    #[napi]
    pub fn query_subgraph(
        &self,
        env: Env,
        id: Either<f64, JsUnknown>,
        options: Option<JsUnknown>,
    ) -> napi::Result<JsSubgraphResult> {
        let config = parse_reachability_options(env, options)?;
        let id = parse_usize_argument(env, id, "id", None)? as u64;
        let output = dispatch!(self, db => db.query_subgraph(id, &config))
            .map_err(|error| to_napi_error(env, error))?;
        Ok(JsSubgraphResult {
            nodes: output
                .nodes
                .into_iter()
                .map(|node| {
                    Ok(JsSubgraphNode {
                        id: node_id_to_js(env, node.id)?,
                        payload: node.payload.into(),
                    })
                })
                .collect::<napi::Result<_>>()?,
            edges: output
                .edges
                .into_iter()
                .map(|edge| {
                    Ok(JsSubgraphEdge {
                        source_id: node_id_to_js(env, edge.source_id)?,
                        target_id: node_id_to_js(env, edge.target_id)?,
                        label: edge.label,
                        weight: round_api_f32(edge.weight),
                        metadata: edge.metadata.into(),
                    })
                })
                .collect::<napi::Result<_>>()?,
            visited_nodes: output.visited_nodes as f64,
            traversed_edges: output.traversed_edges as f64,
            truncated: output.truncated,
        })
    }

    // ── 社区聚类 ──

    /// 基于物理记忆图谱进行 Leiden 社区发现
    ///
    /// **无锁设计**: 短暂持锁快照邻接表后立即释放，聚类在锁外计算。
    /// 调用期间数据库仍可正常读写。
    #[napi]
    pub fn leiden_cluster(
        &self,
        env: Env,
        config: Option<JsUnknown>,
    ) -> napi::Result<JsClusterResult> {
        let object = match config {
            Some(value) => argument_object(env, value, "config", None)?,
            None => env.create_object()?,
        };
        let min_c = parse_search_usize(
            env,
            object.get("minCommunitySize")?,
            3.0,
            "minCommunitySize",
            false,
        )?;
        let max_iter = parse_search_usize(
            env,
            object.get("maxIterations")?,
            15.0,
            "maxIterations",
            false,
        )?;
        let with_cent = optional_bool(env, object.get("withCentroids")?, true, "withCentroids")?;

        let result =
            dispatch!(self, db => db.leiden_cluster(min_c, Some(max_iter), Some(with_cent)))
                .map_err(|error| to_napi_error(env, error))?;

        // 排序确保确定性输出
        let mut sorted_nodes: Vec<_> = result.node_to_cluster.into_iter().collect();
        sorted_nodes.sort_by_key(|&(id, _)| id);

        let mut node_to_cluster = Vec::with_capacity(sorted_nodes.len() * 2);
        for (n, c) in sorted_nodes {
            node_to_cluster.push(node_id_to_js(env, n)?);
            node_to_cluster.push(f64::from(c));
        }

        // 簇标签: 排序后输出
        let mut sorted_sizes: Vec<_> = result.cluster_sizes.iter().collect();
        sorted_sizes.sort_by_key(|(c, _)| *c);

        let mut cluster_labels = Vec::with_capacity(sorted_sizes.len() * 2);
        for (c, size) in &sorted_sizes {
            cluster_labels.push(c.to_string());
            cluster_labels.push(format!("Cluster {} ({})", c, size));
        }

        // 质心: 排序后平铺
        let mut sorted_centroids: Vec<_> = result.centroids.into_iter().collect();
        sorted_centroids.sort_by_key(|(c, _)| *c);

        let mut centroids = Vec::new();
        for (c, v) in sorted_centroids {
            centroids.push(f64::from(c));
            for val in v {
                centroids.push(val as f64);
            }
        }

        Ok(JsClusterResult {
            node_to_cluster,
            cluster_labels,
            centroids,
        })
    }

    /// 获取节点的出边列表
    #[napi]
    pub fn get_edges(&self, env: Env, id: Either<f64, JsUnknown>) -> napi::Result<Vec<JsEdge>> {
        let id = parse_usize_argument(env, id, "id", None)? as u64;
        Ok({
            dispatch!(self, db => db.get_edges(id).map_err(|error| to_napi_error(env, error))?)
                .into_iter()
                .map(|e| {
                    Ok(JsEdge {
                        target_id: node_id_to_js(env, e.target_id)?,
                        label: e.label,
                        weight: round_api_f32(e.weight),
                        metadata: e.metadata.into(),
                    })
                })
                .collect::<napi::Result<_>>()?
        })
    }

    /// 获取节点完整入边，可按标签过滤。
    #[napi]
    pub fn get_incoming_edges(
        &self,
        env: Env,
        id: Either<f64, JsUnknown>,
        label: Option<JsUnknown>,
    ) -> napi::Result<Vec<JsIncomingEdge>> {
        let id = parse_usize_argument(env, id, "id", None)? as u64;
        let label = label
            .map(|value| raw_string(env, value, "label", None))
            .transpose()?;
        Ok({
            dispatch!(self, db => db.get_incoming_edges(id, label.as_deref()).map_err(|error| to_napi_error(env, error))?)
                .into_iter()
                .map(|edge| Ok(JsIncomingEdge {
                    source_id: node_id_to_js(env, edge.source_id)?,
                    target_id: node_id_to_js(env, edge.target_id)?,
                    label: edge.label,
                    weight: round_api_f32(edge.weight),
                    metadata: edge.metadata.into(),
                })).collect::<napi::Result<_>>()?
        })
    }
}
