//! REPL Tab 补全：TQL 关键词 + 点号元命令。

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};

const TQL_KEYWORDS: &[&str] = &[
    "FIND", "MATCH", "SEARCH", "VECTOR", "RETURN", "WHERE", "ORDER BY", "LIMIT", "OFFSET",
    "CREATE", "SET", "DELETE", "DETACH DELETE", "AS", "AND", "OR", "ASC", "DESC", "DISTINCT",
    "COUNT", "SUM", "AVG", "MIN", "MAX",
];

const META_COMMANDS: &[&str] = &[
    ".help", ".info", ".stats", ".schema", ".flush", ".compact", ".export", ".format", ".quit",
    ".exit",
];

/// REPL 的 rustyline Helper：仅实现补全，其余 trait 用默认实现。
pub struct ReplHelper;

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // 当前单词的起始位置（上一个空白之后）
        let start = line[..pos]
            .rfind(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(0);
        let word = &line[start..pos];
        if word.is_empty() {
            return Ok((start, Vec::new()));
        }

        let candidates: Vec<Pair> = if word.starts_with('.') {
            META_COMMANDS
                .iter()
                .filter(|c| c.starts_with(word))
                .map(|c| pair(c))
                .collect()
        } else {
            let upper = word.to_ascii_uppercase();
            TQL_KEYWORDS
                .iter()
                .filter(|k| k.starts_with(&upper))
                .map(|k| pair(k))
                .collect()
        };

        Ok((start, candidates))
    }
}

fn pair(s: &str) -> Pair {
    Pair {
        display: s.to_string(),
        replacement: s.to_string(),
    }
}

impl Hinter for ReplHelper {
    type Hint = String;
}

impl Highlighter for ReplHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> std::borrow::Cow<'l, str> {
        // 元命令（.xxx）与空行不着色，避免干扰
        if line.is_empty() || line.trim_start().starts_with('.') {
            std::borrow::Cow::Borrowed(line)
        } else {
            std::borrow::Cow::Owned(crate::tql_highlight::highlight_ansi(line))
        }
    }

    fn highlight_char(&self, line: &str, _pos: usize, _forced: bool) -> bool {
        // 仅当输入是 TQL（非空、非元命令）时才触发逐键重绘
        !line.is_empty() && !line.trim_start().starts_with('.')
    }
}

impl Validator for ReplHelper {}
impl Helper for ReplHelper {}
