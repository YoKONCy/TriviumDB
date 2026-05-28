//! TUI 图视图的字符渲染模式：根据终端能力选择 Braille / Dot / Block / HalfBlock。
//!
//! 旧 `cmd.exe` 与传统 Windows 控制台对 Braille 字符（U+2800..U+28FF）
//! 字体支持有限，渲染会出现 □ 或缺口；现代终端（Windows Terminal、
//! VS Code、kitty、iTerm2 等）则可正确渲染。本模块提供：
//! - **Auto**：基于平台 + 环境变量启发式选择
//! - 显式指定（来自配置或 TUI 内 `m` 键切换）
//!
//! 启发式规则（Windows）：
//! - `WT_SESSION` 环境变量存在 → Windows Terminal，使用 Braille
//! - `TERM_PROGRAM == "vscode"` → VS Code 集成终端，使用 Braille
//! - `TERMINAL_EMULATOR` 包含 "JetBrains" → JetBrains 终端，使用 Braille
//! - 否则降级为 Dot
//!
//! 非 Windows 平台默认 Braille。

use ratatui::symbols::Marker;
use serde::Deserialize;

/// 配置值；与 `Marker` 一一对应外加 `Auto`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphMarker {
    #[default]
    Auto,
    Braille,
    Dot,
    Block,
    HalfBlock,
}

impl GraphMarker {
    /// 解析为实际 ratatui marker（Auto 触发启发式检测）。
    pub fn resolve(self) -> Marker {
        match self {
            GraphMarker::Auto => detect_marker(),
            GraphMarker::Braille => Marker::Braille,
            GraphMarker::Dot => Marker::Dot,
            GraphMarker::Block => Marker::Block,
            GraphMarker::HalfBlock => Marker::HalfBlock,
        }
    }

    /// 显示名（用于状态栏 / 标题）。
    pub fn label(marker: Marker) -> &'static str {
        match marker {
            Marker::Braille => "braille",
            Marker::Dot => "dot",
            Marker::Block => "block",
            Marker::HalfBlock => "halfblock",
            Marker::Bar => "bar",
        }
    }

    /// 在 Braille / Dot / Block / HalfBlock 之间循环（按 `m` 键切换时使用）。
    pub fn cycle(current: Marker) -> Marker {
        match current {
            Marker::Braille => Marker::Dot,
            Marker::Dot => Marker::Block,
            Marker::Block => Marker::HalfBlock,
            Marker::HalfBlock => Marker::Braille,
            Marker::Bar => Marker::Braille,
        }
    }
}

/// 启发式检测当前终端是否支持 Braille 字符渲染。
pub fn detect_marker() -> Marker {
    detect_marker_from_env(|key| std::env::var(key).ok())
}

/// 注入式接口（便于测试）。
pub fn detect_marker_from_env<F: Fn(&str) -> Option<String>>(get: F) -> Marker {
    if cfg!(target_os = "windows") {
        if get("WT_SESSION").is_some() {
            return Marker::Braille;
        }
        if let Some(term_program) = get("TERM_PROGRAM")
            && term_program.eq_ignore_ascii_case("vscode")
        {
            return Marker::Braille;
        }
        if let Some(emu) = get("TERMINAL_EMULATOR")
            && emu.to_ascii_lowercase().contains("jetbrains")
        {
            return Marker::Braille;
        }
        // 默认 cmd.exe / 老 PowerShell：降级为 Dot
        Marker::Dot
    } else {
        Marker::Braille
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn cycle_visits_all_markers() {
        let mut m = Marker::Braille;
        let mut seen = vec![GraphMarker::label(m)];
        for _ in 0..3 {
            m = GraphMarker::cycle(m);
            seen.push(GraphMarker::label(m));
        }
        assert_eq!(seen, vec!["braille", "dot", "block", "halfblock"]);
        // 第 5 次应回到 braille
        m = GraphMarker::cycle(m);
        assert_eq!(GraphMarker::label(m), "braille");
    }

    #[test]
    fn explicit_resolve_does_not_invoke_detection() {
        assert_eq!(
            std::mem::discriminant(&GraphMarker::Dot.resolve()),
            std::mem::discriminant(&Marker::Dot)
        );
        assert_eq!(
            std::mem::discriminant(&GraphMarker::Braille.resolve()),
            std::mem::discriminant(&Marker::Braille)
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_with_wt_session_uses_braille() {
        let m = detect_marker_from_env(fake_env(&[("WT_SESSION", "1")]));
        assert_eq!(
            std::mem::discriminant(&m),
            std::mem::discriminant(&Marker::Braille)
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_without_modern_terminal_falls_back_to_dot() {
        let m = detect_marker_from_env(fake_env(&[]));
        assert_eq!(
            std::mem::discriminant(&m),
            std::mem::discriminant(&Marker::Dot)
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_with_vscode_uses_braille() {
        let m = detect_marker_from_env(fake_env(&[("TERM_PROGRAM", "vscode")]));
        assert_eq!(
            std::mem::discriminant(&m),
            std::mem::discriminant(&Marker::Braille)
        );
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn non_windows_defaults_to_braille() {
        let m = detect_marker_from_env(fake_env(&[]));
        assert_eq!(
            std::mem::discriminant(&m),
            std::mem::discriminant(&Marker::Braille)
        );
    }
}
