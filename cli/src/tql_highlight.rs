//! 轻量 TQL 分词器与着色，供 REPL（ANSI）与 TUI（ratatui Span）共享。
//!
//! 这是一个用于**语法高亮**的近似分词器，不追求与正式 TQL lexer 完全一致，
//! 只需把关键词 / 字符串 / 数字 / 操作符 / 标点区分开即可。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TokenKind {
    Keyword,
    Str,
    Number,
    Operator, // $eq / $gt ... 以及比较符 > < = !
    Punct,    // { } [ ] ( ) , : . 等
    Ident,
    Whitespace,
}

pub struct Token<'a> {
    pub text: &'a str,
    pub kind: TokenKind,
}

const KEYWORDS: &[&str] = &[
    "FIND", "MATCH", "SEARCH", "VECTOR", "RETURN", "WHERE", "ORDER", "BY", "LIMIT", "OFFSET",
    "CREATE", "SET", "DELETE", "DETACH", "MERGE", "REMOVE", "AS", "AND", "OR", "NOT", "ASC",
    "DESC", "DISTINCT", "COUNT", "SUM", "AVG", "MIN", "MAX", "EXPLAIN",
];

/// 将 TQL 文本切分为带类型的 token（保留空白，便于原样重组）。
pub fn tokenize(input: &str) -> Vec<Token<'_>> {
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let n = chars.len();
    let mut tokens = Vec::new();
    let mut i = 0;

    // 当前 token 的起始字符下标 -> 结束字符下标（独占），切出字节切片
    let slice = |input: &str, chars: &[(usize, char)], a: usize, b: usize| -> (usize, usize) {
        let start = chars[a].0;
        let end = chars.get(b).map(|(byte, _)| *byte).unwrap_or(input.len());
        (start, end)
    };

    while i < n {
        let c = chars[i].1;

        if c.is_whitespace() {
            let a = i;
            while i < n && chars[i].1.is_whitespace() {
                i += 1;
            }
            let (s, e) = slice(input, &chars, a, i);
            tokens.push(Token { text: &input[s..e], kind: TokenKind::Whitespace });
        } else if c == '"' || c == '\'' {
            let quote = c;
            let a = i;
            i += 1;
            while i < n && chars[i].1 != quote {
                // 跳过转义字符
                if chars[i].1 == '\\' && i + 1 < n {
                    i += 1;
                }
                i += 1;
            }
            if i < n {
                i += 1; // 收尾引号
            }
            let (s, e) = slice(input, &chars, a, i);
            tokens.push(Token { text: &input[s..e], kind: TokenKind::Str });
        } else if c.is_ascii_digit() {
            let a = i;
            while i < n && (chars[i].1.is_ascii_digit() || chars[i].1 == '.') {
                i += 1;
            }
            let (s, e) = slice(input, &chars, a, i);
            tokens.push(Token { text: &input[s..e], kind: TokenKind::Number });
        } else if c == '$' {
            let a = i;
            i += 1;
            while i < n && (chars[i].1.is_alphanumeric() || chars[i].1 == '_') {
                i += 1;
            }
            let (s, e) = slice(input, &chars, a, i);
            tokens.push(Token { text: &input[s..e], kind: TokenKind::Operator });
        } else if c.is_alphabetic() || c == '_' {
            let a = i;
            while i < n && (chars[i].1.is_alphanumeric() || chars[i].1 == '_') {
                i += 1;
            }
            let (s, e) = slice(input, &chars, a, i);
            let text = &input[s..e];
            let kind = if KEYWORDS.contains(&text.to_ascii_uppercase().as_str()) {
                TokenKind::Keyword
            } else {
                TokenKind::Ident
            };
            tokens.push(Token { text, kind });
        } else if matches!(c, '>' | '<' | '=' | '!') {
            let a = i;
            while i < n && matches!(chars[i].1, '>' | '<' | '=' | '!') {
                i += 1;
            }
            let (s, e) = slice(input, &chars, a, i);
            tokens.push(Token { text: &input[s..e], kind: TokenKind::Operator });
        } else {
            let a = i;
            i += 1;
            let (s, e) = slice(input, &chars, a, i);
            tokens.push(Token { text: &input[s..e], kind: TokenKind::Punct });
        }
    }

    tokens
}

/// TUI 用：转换为彩色 ratatui Span 列表。
pub fn highlight_spans(input: &str) -> Vec<Span<'static>> {
    tokenize(input)
        .into_iter()
        .map(|t| Span::styled(t.text.to_string(), style_for(t.kind)))
        .collect()
}

fn style_for(kind: TokenKind) -> Style {
    match kind {
        TokenKind::Keyword => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        TokenKind::Str => Style::default().fg(Color::Green),
        TokenKind::Number => Style::default().fg(Color::Yellow),
        TokenKind::Operator => Style::default().fg(Color::Magenta),
        TokenKind::Punct => Style::default().fg(Color::DarkGray),
        TokenKind::Ident | TokenKind::Whitespace => Style::default(),
    }
}

/// REPL 用：转换为带 ANSI 转义序列的着色字符串。
pub fn highlight_ansi(input: &str) -> String {
    const RESET: &str = "\x1b[0m";
    let mut out = String::with_capacity(input.len() + 16);
    for t in tokenize(input) {
        match ansi_for(t.kind) {
            Some(code) => {
                out.push_str(code);
                out.push_str(t.text);
                out.push_str(RESET);
            }
            None => out.push_str(t.text),
        }
    }
    out
}

fn ansi_for(kind: TokenKind) -> Option<&'static str> {
    match kind {
        TokenKind::Keyword => Some("\x1b[1;36m"), // 粗体青
        TokenKind::Str => Some("\x1b[32m"),       // 绿
        TokenKind::Number => Some("\x1b[33m"),     // 黄
        TokenKind::Operator => Some("\x1b[35m"),   // 品红
        TokenKind::Punct => Some("\x1b[90m"),      // 暗灰
        TokenKind::Ident | TokenKind::Whitespace => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_basic_query() {
        let toks = tokenize("FIND {type: \"person\", age: 28} RETURN n");
        // 第一个非空白 token 是关键词 FIND
        assert_eq!(toks[0].text, "FIND");
        assert_eq!(toks[0].kind, TokenKind::Keyword);
        // 含字符串与数字
        assert!(toks.iter().any(|t| t.kind == TokenKind::Str && t.text == "\"person\""));
        assert!(toks.iter().any(|t| t.kind == TokenKind::Number && t.text == "28"));
        // RETURN 也是关键词
        assert!(toks.iter().any(|t| t.kind == TokenKind::Keyword && t.text == "RETURN"));
    }

    #[test]
    fn roundtrip_preserves_text() {
        let q = "MATCH (a)-[:knows]->(b) WHERE a.age > 20 RETURN a, b";
        let joined: String = tokenize(q).iter().map(|t| t.text).collect();
        assert_eq!(joined, q);
    }

    #[test]
    fn handles_operators_and_utf8_strings() {
        let toks = tokenize("FIND {name: \"张三\", n: {$gte: 3}}");
        assert!(toks.iter().any(|t| t.kind == TokenKind::Str && t.text == "\"张三\""));
        assert!(toks.iter().any(|t| t.kind == TokenKind::Operator && t.text == "$gte"));
    }
}
