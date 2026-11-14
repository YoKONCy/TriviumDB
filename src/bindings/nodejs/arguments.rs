//! 语言实参与数值、字符串、序列的边界转换。

use super::errors::{argument_input_error, invalid_constructor_field, to_napi_error};
use super::json::NodeJson;
use napi::bindgen_prelude::Either;
use napi::{Env, JsObject, JsUnknown, ValueType};

pub(super) fn parse_node_id(env: Env, value: f64, name: &str) -> napi::Result<u64> {
    Ok(parse_safe_usize(env, value, name)? as u64)
}

pub(super) fn validate_u64_component(value: f64) -> crate::Result<u64> {
    if !value.is_finite()
        || value < 0.0
        || value.fract() != 0.0
        || value >= 18_446_744_073_709_551_616.0
    {
        return Err(crate::TriviumError::InvalidInput("u64 向量分量必须是 [0, 2^64) 内的有限整数 (u64 vector components must be finite integers in [0, 2^64))".into()));
    }
    Ok(value as u64)
}

fn parse_u64_component(env: Env, value: f64) -> napi::Result<u64> {
    validate_u64_component(value).map_err(|error| to_napi_error(env, error))
}

pub(super) fn parse_u64_vector(env: Env, values: &[f64]) -> napi::Result<Vec<u64>> {
    values
        .iter()
        .copied()
        .map(|value| parse_u64_component(env, value))
        .collect()
}

pub(super) fn parse_safe_usize(env: Env, value: f64, name: &str) -> napi::Result<usize> {
    validate_safe_usize(value, name).map_err(|error| to_napi_error(env, error))
}

fn validate_safe_usize(value: f64, name: &str) -> crate::Result<usize> {
    if !value.is_finite()
        || value < 0.0
        || value.fract() != 0.0
        || value > 9_007_199_254_740_991.0
        || value > usize::MAX as f64
    {
        return Err(crate::TriviumError::InvalidInput(format!(
                    "{name} 必须是 JavaScript 安全范围内的非负整数 ({name} must be a non-negative JavaScript safe integer)"
                )).at_field(name));
    }
    Ok(value as usize)
}

pub(super) fn parse_management_usize(
    env: Env,
    value: Either<f64, JsUnknown>,
    field: &str,
) -> napi::Result<usize> {
    parse_usize_argument(
        env,
        value,
        field,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )
}

pub(super) fn parse_usize_argument(
    env: Env,
    value: Either<f64, JsUnknown>,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
) -> napi::Result<usize> {
    let parsed = match value {
        Either::A(value) => validate_safe_usize(value, field),
        Either::B(_) => Err(crate::TriviumError::InvalidInput(format!(
            "参数必须是数值 (Argument must be a number): {field}"
        ))
        .at_field(field)),
    };
    parsed.map_err(|error| {
        to_napi_error(
            env,
            match write {
                Some(write) => error.with_write_info(write),
                None => error,
            },
        )
    })
}

pub(super) fn parse_management_string(
    env: Env,
    value: Either<String, JsUnknown>,
    field: &str,
) -> napi::Result<String> {
    parse_string_argument(
        env,
        value,
        field,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )
}

pub(super) fn parse_string_argument(
    env: Env,
    value: Either<String, JsUnknown>,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
) -> napi::Result<String> {
    match value {
        Either::A(value) => Ok(value),
        Either::B(_) => {
            let error = crate::TriviumError::InvalidInput(format!(
                "参数必须是字符串 (Argument must be a string): {field}"
            ))
            .at_field(field);
            Err(to_napi_error(
                env,
                match write {
                    Some(write) => error.with_write_info(write),
                    None => error,
                },
            ))
        }
    }
}

pub(super) fn write_number(env: Env, value: JsUnknown, field: &str) -> napi::Result<f64> {
    number_value(
        env,
        value,
        field,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )
}

pub(super) fn read_number(env: Env, value: JsUnknown, field: &str) -> napi::Result<f64> {
    number_value(env, value, field, None)
}

fn number_value(
    env: Env,
    value: JsUnknown,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
) -> napi::Result<f64> {
    if value.get_type()? != ValueType::Number {
        return Err(argument_input_error(
            env,
            field,
            "参数必须是数值 (Argument must be a number)",
            write,
        ));
    }
    napi::JsNumber::try_from(value)?.get_double()
}

pub(super) fn write_id(env: Env, value: JsUnknown, field: &str) -> napi::Result<u64> {
    let value = write_number(env, value, field)?;
    validate_safe_usize(value, field)
        .map(|value| value as u64)
        .map_err(|error| {
            to_napi_error(
                env,
                error.with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
            )
        })
}

pub(super) fn raw_string(
    env: Env,
    value: JsUnknown,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
) -> napi::Result<String> {
    let parsed = if value.get_type()? == ValueType::String {
        crate::bindings::nodejs::json::string_from_js(value)
    } else {
        Err(napi::Error::new(
            napi::Status::InvalidArg,
            "参数必须是字符串 (Argument must be a string)",
        ))
    };
    parsed.map_err(|error| {
        if error.status != napi::Status::InvalidArg {
            return error;
        }
        let input = crate::TriviumError::InvalidInput(error.reason).at_field(field);
        to_napi_error(
            env,
            match write {
                Some(write) => input.with_write_info(write),
                None => input,
            },
        )
    })
}

pub(super) fn write_string(env: Env, value: JsUnknown, field: &str) -> napi::Result<String> {
    raw_string(
        env,
        value,
        field,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
    )
}

pub(super) fn write_array<T>(
    env: Env,
    value: JsUnknown,
    field: &str,
    parse: impl FnMut(Env, JsUnknown, &str) -> napi::Result<T>,
) -> napi::Result<Vec<T>> {
    array_value(
        env,
        value,
        field,
        Some(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
        parse,
    )
}

pub(super) fn read_array<T>(
    env: Env,
    value: JsUnknown,
    field: &str,
    parse: impl FnMut(Env, JsUnknown, &str) -> napi::Result<T>,
) -> napi::Result<Vec<T>> {
    array_value(env, value, field, None, parse)
}

fn array_value<T>(
    env: Env,
    value: JsUnknown,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
    mut parse: impl FnMut(Env, JsUnknown, &str) -> napi::Result<T>,
) -> napi::Result<Vec<T>> {
    let invalid = || {
        argument_input_error(
            env,
            field,
            "参数必须是数组 (Argument must be an array)",
            write,
        )
    };
    if value.get_type()? != ValueType::Object {
        return Err(invalid());
    }
    let array = JsObject::try_from(value)?;
    if !array.is_array()? {
        return Err(invalid());
    }
    let mut result = Vec::new();
    for index in 0..array.get_array_length()? {
        let path = crate::sdk::field_member(field, &index.to_string());
        let item = parse(env, array.get_element(index)?, &path)?;
        result.try_reserve(1).map_err(|error| {
            let error = crate::TriviumError::CapacityAllocationFailed {
                reason: format!("参数缓冲分配失败 (Argument buffer allocation failed): {error}"),
            }
            .at_field(&path);
            to_napi_error(
                env,
                match write {
                    Some(write) => error.with_write_info(write),
                    None => error,
                },
            )
        })?;
        result.push(item);
    }
    Ok(result)
}

pub(super) fn write_json(
    env: Env,
    value: JsUnknown,
    field: &str,
) -> napi::Result<serde_json::Value> {
    NodeJson::from_js_at(env, value, field).map(|value| value.0)
}

pub(super) fn read_json(
    env: Env,
    value: JsUnknown,
    field: &str,
) -> napi::Result<serde_json::Value> {
    NodeJson::from_js_field(env, value, field, None).map(|value| value.0)
}

pub(super) fn read_id(env: Env, value: JsUnknown, field: &str) -> napi::Result<u64> {
    validate_safe_usize(read_number(env, value, field)?, field)
        .map(|value| value as u64)
        .map_err(|error| to_napi_error(env, error))
}

pub(super) fn read_string(env: Env, value: JsUnknown, field: &str) -> napi::Result<String> {
    raw_string(env, value, field, None)
}

pub(super) fn read_vector(
    env: Env,
    value: JsUnknown,
    field: &str,
    dtype: &str,
) -> napi::Result<Vec<f64>> {
    read_array(env, value, field, |env, value, path| {
        let number = read_number(env, value, path)?;
        if dtype == "u64" {
            validate_u64_component(number)
                .map_err(|error| to_napi_error(env, error.at_field(path)))?;
        }
        Ok(number)
    })
}

pub(super) fn argument_object(
    env: Env,
    value: JsUnknown,
    field: &str,
    write: Option<crate::error::WriteErrorInfo>,
) -> napi::Result<JsObject> {
    let invalid = || {
        argument_input_error(
            env,
            field,
            "参数必须是非数组对象 (Argument must be a non-array object)",
            write,
        )
    };
    if value.get_type()? != ValueType::Object {
        return Err(invalid());
    }
    let object = JsObject::try_from(value)?;
    if object.is_array()? {
        return Err(invalid());
    }
    Ok(object)
}

pub(super) fn optional_number(
    env: Env,
    value: Option<JsUnknown>,
    default: f64,
    field: &str,
) -> napi::Result<f64> {
    value
        .map(|value| read_number(env, value, field))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

pub(super) fn optional_bool(
    env: Env,
    value: Option<JsUnknown>,
    default: bool,
    field: &str,
) -> napi::Result<bool> {
    value
        .map(|value| {
            if value.get_type()? != ValueType::Boolean {
                return Err(argument_input_error(
                    env,
                    field,
                    "参数必须是布尔值 (Argument must be a boolean)",
                    None,
                ));
            }
            napi::JsBoolean::try_from(value)?.get_value()
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

pub(super) fn write_vector(
    env: Env,
    value: JsUnknown,
    field: &str,
    dtype: &str,
) -> napi::Result<Vec<f64>> {
    write_array(env, value, field, |env, value, path| {
        let number = write_number(env, value, path)?;
        if dtype == "u64" {
            validate_u64_component(number).map_err(|error| {
                to_napi_error(
                    env,
                    error
                        .at_field(path)
                        .with_write_info(crate::error::WriteErrorInfo::MAINTENANCE_VALIDATION),
                )
            })?;
        }
        Ok(number)
    })
}

pub(super) fn constructor_options(env: Env, options: Option<JsUnknown>) -> napi::Result<JsObject> {
    let Some(value) = options else {
        return env.create_object();
    };
    let invalid = || {
        invalid_constructor_field(
            env,
            "options",
            "配置必须是非数组对象 (Options must be a non-array object)",
        )
    };
    if value.get_type()? != ValueType::Object {
        return Err(invalid());
    }
    let object = JsObject::try_from(value)?;
    if object.is_array()? {
        return Err(invalid());
    }
    Ok(object)
}

pub(super) fn constructor_string(
    env: Env,
    value: Option<JsUnknown>,
    default: &str,
    field: &str,
) -> napi::Result<String> {
    match value {
        Some(value) => raw_string(env, value, field, None),
        None => Ok(default.into()),
    }
}

pub(super) fn constructor_bool(
    env: Env,
    value: Option<Either<bool, JsUnknown>>,
    default: bool,
    field: &str,
) -> napi::Result<bool> {
    match value {
        None => Ok(default),
        Some(Either::A(value)) => Ok(value),
        Some(Either::B(_)) => Err(invalid_constructor_field(
            env,
            field,
            "参数必须是布尔值 (Argument must be a boolean)",
        )),
    }
}

pub(super) fn constructor_mib(
    env: Env,
    value: usize,
    field: &str,
    max_bytes: u128,
) -> napi::Result<usize> {
    value
        .checked_mul(1024 * 1024)
        .filter(|bytes| (*bytes as u128) <= max_bytes)
        .ok_or_else(|| {
            invalid_constructor_field(
                env,
                field,
                "MiB 换算超出字节表示范围 (MiB conversion exceeds the byte representation range)",
            )
        })
}

pub(super) fn parse_search_usize(
    env: Env,
    value: Option<Either<f64, JsUnknown>>,
    default: f64,
    field: &str,
    positive: bool,
) -> napi::Result<usize> {
    let value = parse_usize_argument(env, value.unwrap_or(Either::A(default)), field, None)?;
    if positive && value == 0 {
        return Err(to_napi_error(
            env,
            crate::TriviumError::InvalidInput(format!(
                "{field} 必须大于零 ({field} must be positive)"
            ))
            .at_field(field),
        ));
    }
    Ok(value)
}

pub(super) fn parse_positive_usize(env: Env, value: f64, name: &str) -> napi::Result<usize> {
    let value = parse_safe_usize(env, value, name)?;
    if value == 0 {
        return Err(to_napi_error(
            env,
            crate::TriviumError::InvalidInput(format!(
                "{name} 必须大于零 ({name} must be positive)"
            )),
        ));
    }
    Ok(value)
}

pub(super) fn node_id_to_js(env: Env, id: u64) -> napi::Result<f64> {
    if id > 9_007_199_254_740_991 {
        return Err(to_napi_error(env, crate::TriviumError::InvalidInput("节点 ID 超出 JavaScript 安全整数范围，请使用字符串 ID 的 TQL 接口 (Node ID exceeds the JavaScript safe-integer range; use TQL string IDs)".into())));
    }
    Ok(id as f64)
}

pub(super) fn node_ids_to_js(
    env: Env,
    ids: impl IntoIterator<Item = u64>,
) -> napi::Result<Vec<f64>> {
    ids.into_iter()
        .map(|value| node_id_to_js(env, value))
        .collect()
}
