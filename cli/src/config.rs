//! 配置文件支持：`~/.triviumdb.toml`。
//!
//! 优先级：CLI 参数 > 配置文件 > 内置默认。本模块只负责读取/解析配置；
//! 与 CLI 参数的合并在 `main.rs` 完成。
//!
//! 示例：
//! ```toml
//! [defaults]
//! dtype  = "f32"      # f32 | f16 | u64
//! format = "table"    # table | json | csv
//!
//! [tui]
//! default_limit = 50  # TUI 启动默认 MATCH (n) ... LIMIT N
//! ```

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub defaults: Defaults,
    pub tui: TuiConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub dtype: Option<String>,
    pub format: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TuiConfig {
    pub default_limit: Option<usize>,
    /// 图视图字符渲染：auto / braille / dot / block / half_block
    pub graph_marker: Option<String>,
}

impl Config {
    /// 读取 `~/.triviumdb.toml`。文件缺失返回默认；解析失败打印警告后返回默认。
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return Self::default(), // 文件不存在 = 全默认
        };
        match toml::from_str::<Self>(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("warning: 配置解析失败 {}: {e}（已忽略，使用默认）", path.display());
                Self::default()
            }
        }
    }
}

fn config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".triviumdb.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let cfg: Config = toml::from_str(
            r#"
            [defaults]
            dtype = "f16"
            format = "json"
            [tui]
            default_limit = 100
        "#,
        )
        .unwrap();
        assert_eq!(cfg.defaults.dtype.as_deref(), Some("f16"));
        assert_eq!(cfg.defaults.format.as_deref(), Some("json"));
        assert_eq!(cfg.tui.default_limit, Some(100));
    }

    #[test]
    fn empty_config_is_all_none() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.defaults.dtype.is_none());
        assert!(cfg.defaults.format.is_none());
        assert!(cfg.tui.default_limit.is_none());
    }

    #[test]
    fn partial_config_ok() {
        let cfg: Config = toml::from_str("[defaults]\nformat = \"csv\"\n").unwrap();
        assert_eq!(cfg.defaults.format.as_deref(), Some("csv"));
        assert!(cfg.defaults.dtype.is_none());
    }
}
