//! 查询、过滤与数据库配置的语言表示。

use super::arguments::{
    argument_object, optional_bool, optional_number, parse_search_usize, read_array, read_json,
    read_number, read_string,
};
use super::errors::{argument_input_error, invalid_constructor_field, to_napi_error};
use napi::{Env, JsUnknown};

pub(super) fn parse_reachability_options(
    env: Env,
    options: Option<JsUnknown>,
) -> napi::Result<crate::graph::reachability::ReachabilityConfig> {
    let object = match options {
        Some(value) => argument_object(env, value, "options", None)?,
        None => env.create_object()?,
    };
    let direction = object
        .get("direction")?
        .map(|value| read_string(env, value, "direction"))
        .transpose()?;
    let direction = match direction.as_deref().unwrap_or("outgoing") {
        "outgoing" => crate::graph::reachability::ReachabilityDirection::Outgoing,
        "incoming" => crate::graph::reachability::ReachabilityDirection::Incoming,
        "both" => crate::graph::reachability::ReachabilityDirection::Both,
        _ => {
            return Err(argument_input_error(
                env,
                "direction",
                "direction 必须是 outgoing / incoming / both (direction must be outgoing / incoming / both)",
                None,
            ));
        }
    };
    Ok(crate::graph::reachability::ReachabilityConfig {
        min_depth: parse_search_usize(env, object.get("minDepth")?, 1.0, "minDepth", false)?,
        max_depth: parse_search_usize(env, object.get("maxDepth")?, 1.0, "maxDepth", false)?,
        labels: object
            .get("labels")?
            .map(|value| read_array(env, value, "labels", read_string))
            .transpose()?,
        direction,
        max_visited_nodes: parse_search_usize(
            env,
            object.get("maxVisitedNodes")?,
            10_000.0,
            "maxVisitedNodes",
            false,
        )?,
        max_results: parse_search_usize(
            env,
            object.get("maxResults")?,
            10_000.0,
            "maxResults",
            false,
        )?,
        max_edges: parse_search_usize(env, object.get("maxEdges")?, 50_000.0, "maxEdges", false)?,
        max_frontier_size: 10_000,
        exhaustion_policy: crate::graph::budget::BudgetExhaustionPolicy::Partial,
    })
}

pub(super) fn parse_search_config(
    env: Env,
    value: Option<JsUnknown>,
    advanced: bool,
) -> napi::Result<(crate::database::SearchConfig, Option<String>)> {
    let object = match value {
        Some(value) => argument_object(env, value, "config", None)?,
        None => env.create_object()?,
    };
    let config = crate::database::SearchConfig {
        top_k: parse_search_usize(env, object.get("topK")?, 5.0, "topK", true)?,
        recall_k: parse_search_usize(env, object.get("recallK")?, 0.0, "recallK", false)?,
        rerank_k: parse_search_usize(env, object.get("rerankK")?, 0.0, "rerankK", false)?,
        expand_depth: parse_search_usize(
            env,
            object.get("expandDepth")?,
            2.0,
            "expandDepth",
            false,
        )?,
        min_score: optional_number(env, object.get("minScore")?, 0.1, "minScore")? as f32,
        teleport_alpha: optional_number(env, object.get("teleportAlpha")?, 0.0, "teleportAlpha")?
            as f32,
        enable_advanced_pipeline: optional_bool(
            env,
            object.get("enableAdvancedPipeline")?,
            advanced,
            "enableAdvancedPipeline",
        )?,
        enable_sparse_residual: optional_bool(
            env,
            object.get("enableSparseResidual")?,
            false,
            "enableSparseResidual",
        )?,
        fista_lambda: optional_number(env, object.get("fistaLambda")?, 0.1, "fistaLambda")? as f32,
        fista_threshold: optional_number(env, object.get("fistaThreshold")?, 0.3, "fistaThreshold")?
            as f32,
        enable_dpp: optional_bool(env, object.get("enableDpp")?, false, "enableDpp")?,
        dpp_quality_weight: optional_number(
            env,
            object.get("dppQualityWeight")?,
            1.0,
            "dppQualityWeight",
        )? as f32,
        enable_refractory_fatigue: optional_bool(
            env,
            object.get("enableRefractoryFatigue")?,
            false,
            "enableRefractoryFatigue",
        )?,
        enable_text_hybrid_search: optional_bool(
            env,
            object.get("enableTextHybridSearch")?,
            false,
            "enableTextHybridSearch",
        )?,
        text_boost: optional_number(env, object.get("textBoost")?, 1.5, "textBoost")? as f32,
        force_brute_force: optional_bool(
            env,
            object.get("forceBruteForce")?,
            false,
            "forceBruteForce",
        )?,
        diffusion_bias: object
            .get("diffusionBias")?
            .map(|value| {
                read_array(env, value, "diffusionBias", read_number)
                    .map(|values| values.into_iter().map(|value| value as f32).collect())
            })
            .transpose()?,
        expand_labels: object
            .get("expandLabels")?
            .map(|value| read_array(env, value, "expandLabels", read_string))
            .transpose()?,
        max_edges_per_node: parse_search_usize(
            env,
            object.get("maxEdgesPerNode")?,
            0.0,
            "maxEdgesPerNode",
            false,
        )?,
        min_edge_weight: optional_number(env, object.get("minEdgeWeight")?, 0.0, "minEdgeWeight")?
            as f32,
        edge_direction: parse_edge_direction(
            env,
            object
                .get("edgeDirection")?
                .map(|value| read_string(env, value, "edgeDirection"))
                .transpose()?
                .as_deref(),
        )?,
        payload_filter: parse_payload_filter(
            env,
            object
                .get("payloadFilter")?
                .map(|value| read_json(env, value, "payloadFilter"))
                .transpose()?,
        )?,
        ..Default::default()
    };
    let query_text = object
        .get("customQueryText")?
        .map(|value| read_string(env, value, "customQueryText"))
        .transpose()?;
    Ok((config, query_text))
}

pub(super) fn parse_row_overflow(
    env: Env,
    value: Option<&str>,
) -> napi::Result<crate::database::RowOverflowPolicy> {
    match value.unwrap_or("throw") {
        "throw" => Ok(crate::database::RowOverflowPolicy::Throw),
        "break" => Ok(crate::database::RowOverflowPolicy::Break),
        _ => Err(invalid_constructor_field(
            env,
            "rowOverflow",
            "rowOverflow 必须是 throw / break (rowOverflow must be throw / break)",
        )),
    }
}

pub(super) fn parse_storage_mode(
    env: Env,
    value: Option<&str>,
) -> napi::Result<crate::database::StorageMode> {
    match value.unwrap_or("mmap") {
        "mmap" => Ok(crate::database::StorageMode::Mmap),
        "rom" => Ok(crate::database::StorageMode::Rom),
        _ => Err(invalid_constructor_field(
            env,
            "storageMode",
            "storageMode 必须是 mmap / rom (storageMode must be mmap / rom)",
        )),
    }
}

pub(super) fn parse_access_mode(
    env: Env,
    value: Option<&str>,
) -> napi::Result<crate::database::AccessMode> {
    match value.unwrap_or("readWrite") {
        "readWrite" => Ok(crate::database::AccessMode::ReadWrite),
        "readOnly" => Ok(crate::database::AccessMode::ReadOnly),
        "immutable" => Ok(crate::database::AccessMode::Immutable),
        _ => Err(invalid_constructor_field(
            env,
            "accessMode",
            "accessMode 必须是 readWrite / readOnly / immutable (accessMode must be readWrite / readOnly / immutable)",
        )),
    }
}

pub(super) fn parse_missing_index_policy(
    env: Env,
    value: Option<&str>,
) -> napi::Result<crate::database::MissingIndexPolicy> {
    match value.unwrap_or("fallback") {
        "fallback" => Ok(crate::database::MissingIndexPolicy::Fallback),
        "buildInMemory" => Ok(crate::database::MissingIndexPolicy::BuildInMemory),
        "error" => Ok(crate::database::MissingIndexPolicy::Error),
        _ => Err(invalid_constructor_field(
            env,
            "missingIndexPolicy",
            "missingIndexPolicy 必须是 fallback / buildInMemory / error (missingIndexPolicy must be fallback / buildInMemory / error)",
        )),
    }
}

pub(super) fn parse_sync_mode(env: Env, s: &str) -> napi::Result<crate::storage::wal::SyncMode> {
    crate::storage::wal::SyncMode::parse(s)
        .map_err(|reason| invalid_constructor_field(env, "syncMode", &reason))
}

pub(super) fn parse_payload_filter(
    env: Env,
    value: Option<serde_json::Value>,
) -> napi::Result<Option<crate::filter::Filter>> {
    value
        .map(|filter| {
            crate::filter::Filter::from_json(&filter).map_err(|error| {
                to_napi_error(
                    env,
                    crate::TriviumError::InvalidInput(format!(
                        "payloadFilter 无效 (Invalid payloadFilter): {error}"
                    ))
                    .at_field("payloadFilter"),
                )
            })
        })
        .transpose()
}

fn parse_edge_direction(
    env: Env,
    value: Option<&str>,
) -> napi::Result<crate::database::EdgeDirection> {
    match value.unwrap_or("out") {
        "out" | "outgoing" => Ok(crate::database::EdgeDirection::Outgoing),
        "in" | "incoming" => Ok(crate::database::EdgeDirection::Incoming),
        "both" => Ok(crate::database::EdgeDirection::Both),
        _ => Err(argument_input_error(
            env,
            "edgeDirection",
            "edgeDirection 必须是 out / in / both (edgeDirection must be out / in / both)",
            None,
        )),
    }
}
