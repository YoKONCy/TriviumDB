//! FFI 绑定层
//!
//! 将 TriviumDB 核心引擎暴露给 Python (PyO3) 和 Node.js (NAPI) 运行时。
//! 两个子模块共享 `Filter::from_json` 和 `SyncMode::parse` 公共逻辑，
//! 仅包含各自运行时的类型转换和 FFI 胶水代码。

#[cfg(feature = "nodejs")]
pub mod nodejs;

#[cfg(feature = "python")]
pub mod python;
