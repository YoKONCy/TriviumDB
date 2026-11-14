//! 结构化错误、异常类别及写入结果的语言适配。

use napi::Env;

pub(crate) fn to_napi_error(env: Env, error: crate::error::TriviumError) -> napi::Error {
    let mut info = error.info();
    if let Some(
        crate::error::ErrorTarget::Field { field }
        | crate::error::ErrorTarget::FieldReference { field, .. },
    ) = &mut info.target
    {
        *field = match field.as_str() {
            "min_score" => "minScore",
            "request_id" => "requestId",
            "timeout_ms" => "timeoutMs",
            "teleport_alpha" => "teleportAlpha",
            "fista_lambda" => "fistaLambda",
            "fista_threshold" => "fistaThreshold",
            "dpp_quality_weight" => "dppQualityWeight",
            "text_boost" => "textBoost",
            "min_edge_weight" => "minEdgeWeight",
            "diffusion_bias" => "diffusionBias",
            "hybrid_alpha" => "hybridAlpha",
            "max_results" => "maxResults",
            _ => field.as_str(),
        }
        .to_owned();
    }
    let details = crate::sdk::json_error_details(info.details);
    let result = env
        .create_error(napi::Error::from_reason(info.message))
        .and_then(|mut object| {
            object.set_named_property("schema_version", info.schema_version)?;
            object.set_named_property("code", info.code)?;
            object.set_named_property("details", details.clone())?;
            if let Some(stage) = info.stage {
                object.set_named_property("stage", stage.as_str())?;
            }
            if let Some(target) = info.target {
                let target = serde_json::to_value(target)
                    .map_err(|error| napi::Error::from_reason(error.to_string()))?;
                object.set_named_property("target", target)?;
            }
            if let Some(write) = info.write {
                let write = serde_json::to_value(write)
                    .map_err(|error| napi::Error::from_reason(error.to_string()))?;
                object.set_named_property("write", write)?;
            }
            if let Some(fields) = details.as_object() {
                for (name, value) in fields {
                    if matches!(
                        name.as_str(),
                        "schema_version"
                            | "code"
                            | "message"
                            | "details"
                            | "stage"
                            | "target"
                            | "write"
                    ) {
                        continue;
                    }
                    object.set_named_property(name, value.clone())?;
                }
            }
            Ok(napi::Error::from(object.into_unknown()))
        });
    match result {
        Ok(error) | Err(error) => error,
    }
}

pub(super) fn with_napi_write_info(
    env: Env,
    error: napi::Error,
    write: crate::error::WriteErrorInfo,
) -> napi::Error {
    let result = napi::JsError::from(error)
        .into_unknown(env)
        .coerce_to_object()
        .and_then(|mut object| {
            let write = serde_json::to_value(write)
                .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            object.set_named_property("write", write)?;
            Ok(napi::Error::from(object.into_unknown()))
        });
    match result {
        Ok(error) | Err(error) => error,
    }
}

pub(super) fn invalid_constructor_field(env: Env, field: &str, reason: &str) -> napi::Error {
    to_napi_error(
        env,
        crate::TriviumError::InvalidInput(reason.into()).at_field(field),
    )
}

pub(super) fn write_input_error(env: Env, field: &str, reason: &str) -> napi::Error {
    argument_input_error(
        env,
        field,
        reason,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )
}

pub(super) fn argument_input_error(
    env: Env,
    field: &str,
    reason: &str,
    write: Option<crate::error::WriteErrorInfo>,
) -> napi::Error {
    let input = crate::TriviumError::InvalidInput(reason.into()).at_field(field);
    to_napi_error(
        env,
        match write {
            Some(write) => input.with_write_info(write),
            None => input,
        },
    )
}
