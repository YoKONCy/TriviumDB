//! TQL 解析错误诊断渲染：把 `(input, byte_pos, msg)` 渲染为带 caret 的多行错误信息。
//!
//! 同时支持：
//! - **ANSI 渲染**（REPL / 非交互命令）：彩色 caret + 行号
//! - **结构化渲染**（TUI）：返回 `(line_index, col_index, line_text, msg)`，由调用方决定如何画
//!
//! 通用：`byte_pos` 是 `input` 字符串中的字节偏移；超过末尾时被截到末尾。

use colored::Colorize;
use triviumdb::query::tql_lexer::ParseErrorAt;

/// 解析后的诊断信息（行号/列号均为 0 起）。
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// 0-based 行号
    pub line: usize,
    /// 0-based 列号（按 unicode scalar 计数）
    pub col: usize,
    /// 该行原始文本
    pub line_text: String,
    /// 错误消息
    pub msg: String,
}

impl Diagnostic {
    /// 从 `(input, ParseErrorAt)` 计算诊断；位置自动 clamp 到 input 范围内。
    pub fn from_parse_error(input: &str, err: &ParseErrorAt) -> Self {
        let pos = err.byte_pos.min(input.len());
        let mut line_start = 0usize;
        let mut line_no = 0usize;
        for (i, ch) in input.char_indices() {
            if i >= pos {
                break;
            }
            if ch == '\n' {
                line_no += 1;
                line_start = i + 1;
            }
        }
        let line_end = input[line_start..]
            .find('\n')
            .map(|n| line_start + n)
            .unwrap_or(input.len());
        let line_text = input[line_start..line_end].to_string();
        // 列号按 unicode scalar 数（与编辑器列号一致），跳过行起始字节
        let col = input[line_start..pos].chars().count();
        Diagnostic {
            line: line_no,
            col,
            line_text,
            msg: err.msg.clone(),
        }
    }

    /// 渲染为 ANSI 多行错误信息（REPL / exec 用）。
    ///
    /// 输出形如：
    /// ```text
    /// error: Expected MATCH, got Eof
    ///   --> line 2, col 14
    ///    |
    ///  2 |   WHERE x == 1
    ///    |              ^
    /// ```
    pub fn render_ansi(&self) -> String {
        let line_no = self.line + 1;
        let col_no = self.col + 1;
        let gutter = format!("{line_no:>3}");
        let pad = " ".repeat(gutter.len());
        let caret_indent = " ".repeat(self.col);
        let mut out = String::new();
        out.push_str(&format!("{} {}\n", "error:".red().bold(), self.msg));
        out.push_str(&format!(
            "{}{} line {line_no}, col {col_no}\n",
            pad,
            "-->".cyan()
        ));
        out.push_str(&format!("{} {}\n", pad, "|".cyan()));
        out.push_str(&format!(
            "{} {} {}\n",
            gutter.cyan(),
            "|".cyan(),
            self.line_text
        ));
        out.push_str(&format!(
            "{} {} {}{}\n",
            pad,
            "|".cyan(),
            caret_indent,
            "^".red().bold()
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(byte_pos: usize, msg: &str) -> ParseErrorAt {
        ParseErrorAt::new(msg, byte_pos)
    }

    #[test]
    fn diagnostic_first_line_first_col() {
        let input = "MATCH";
        let d = Diagnostic::from_parse_error(input, &err(0, "boom"));
        assert_eq!(d.line, 0);
        assert_eq!(d.col, 0);
        assert_eq!(d.line_text, "MATCH");
        assert_eq!(d.msg, "boom");
    }

    #[test]
    fn diagnostic_multi_line() {
        let input = "MATCH (n)\nWHERE n.x == 1\nRETURN n";
        // pos 指向第二行 "==" 第一个字符
        let pos = input.find("==").unwrap();
        let d = Diagnostic::from_parse_error(input, &err(pos, "bad op"));
        assert_eq!(d.line, 1);
        assert_eq!(d.line_text, "WHERE n.x == 1");
        // "WHERE n.x " 是 10 个字符
        assert_eq!(d.col, 10);
    }

    #[test]
    fn diagnostic_pos_at_eof_clamps() {
        let input = "MATCH (n)";
        let d = Diagnostic::from_parse_error(input, &err(999, "eof"));
        assert_eq!(d.line, 0);
        assert_eq!(d.col, input.chars().count());
        assert_eq!(d.line_text, input);
    }

    #[test]
    fn diagnostic_unicode_column() {
        let input = "MATCH (节点) RETURN n";
        // pos 指向 ')' 后的空格
        let pos = input.find(") ").unwrap() + 1;
        let d = Diagnostic::from_parse_error(input, &err(pos, "x"));
        // "MATCH (节点)" → 10 个 unicode scalar：M A T C H ' ' ( 节 点 )
        assert_eq!(d.col, 10);
    }

    #[test]
    fn render_ansi_contains_caret_and_line_text() {
        let input = "MATCH (n) WHERE";
        let d = Diagnostic::from_parse_error(input, &err(input.len(), "Expected RETURN, got Eof"));
        let out = d.render_ansi();
        assert!(out.contains("Expected RETURN"));
        assert!(out.contains("MATCH (n) WHERE"));
        assert!(out.contains("^"));
    }
}
