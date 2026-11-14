//! 绑定错误与数值表示的既有契约验收。

use super::queries::node_tql_scalar_json;
use super::types::round_api_f32;

#[test]
fn 错误字段中的大整数保持十进制精度() {
    assert_eq!(
        node_tql_scalar_json(
            serde_json::json!({"id": 18446744073709551614u64, "used": 9007199254740993u64, "limit": 9007199254740991u64})
        ),
        serde_json::json!({"id": "18446744073709551614", "used": "9007199254740993", "limit": 9007199254740991u64})
    );
}

#[test]
fn 边权重_api舍入到六位小数() {
    assert_eq!(round_api_f32(0.9f32), 0.9);
    assert_eq!(round_api_f32(0.12345678f32), 0.123457);
}
