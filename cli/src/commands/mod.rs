//! 非交互子命令的共享实现层。
//!
//! 这些函数同时被 CLI 子命令、REPL 元命令复用。

pub mod compact;
pub mod exec;
pub mod export;
pub mod import;
pub mod info;
pub mod repair;
