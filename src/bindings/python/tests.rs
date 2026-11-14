//! 绑定错误与数值表示的既有契约验收。

use super::errors::to_py_error;
use super::types::round_api_f32;

#[test]
fn 核心错误完整映射到_python异常() {
    pyo3::prepare_freethreaded_python();
    pyo3::Python::with_gil(|py| {
        use pyo3::prelude::*;
        let json = py.import("json").unwrap();
        for (error, code, expected) in crate::error::tests::cases() {
            let message = error.to_string();
            let exception = to_py_error(error);
            let value = exception.value(py);
            assert_eq!(
                value
                    .getattr("schema_version")
                    .unwrap()
                    .extract::<u32>()
                    .unwrap(),
                1
            );
            assert_eq!(
                value
                    .getattr("message")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                message
            );
            assert_eq!(
                value.getattr("code").unwrap().extract::<String>().unwrap(),
                code
            );
            let expected = json.call_method1("loads", (expected.to_string(),)).unwrap();
            assert!(
                value.getattr("details").unwrap().eq(expected).unwrap(),
                "{code}"
            );
        }
    });
}

#[test]
fn 边权重_api舍入到六位小数() {
    assert_eq!(round_api_f32(0.9f32), 0.9);
    assert_eq!(round_api_f32(0.12345678f32), 0.123457);
}
