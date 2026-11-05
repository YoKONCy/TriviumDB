//! 面向 JSON/JavaScript 传输的错误字段编码；Rust/Python 保持原生整数。

use serde_json::Value;

/// 将超出 JavaScript 安全整数域的错误字段编码为十进制字符串。
pub fn json_error_details(value: Value) -> Value {
    match value {
        Value::Number(number)
            if number
                .as_u64()
                .is_some_and(|value| value > 9_007_199_254_740_991)
                || number
                    .as_i64()
                    .is_some_and(|value| value.unsigned_abs() > 9_007_199_254_740_991) =>
        {
            Value::String(number.to_string())
        }
        Value::Array(values) => Value::Array(values.into_iter().map(json_error_details).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, json_error_details(value)))
                .collect(),
        ),
        other => other,
    }
}
