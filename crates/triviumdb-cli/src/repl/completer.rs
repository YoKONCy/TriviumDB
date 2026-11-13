//! REPL Tab 补全：TQL 关键词 + 点号元命令。

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Context, Helper};

const TQL_KEYWORDS: &[&str] = &[
    "FIND",
    "MATCH",
    "SEARCH",
    "VECTOR",
    "RETURN",
    "WHERE",
    "ORDER BY",
    "LIMIT",
    "OFFSET",
    "CREATE",
    "SET",
    "DELETE",
    "DETACH DELETE",
    "AS",
    "AND",
    "OR",
    "ASC",
    "DESC",
    "DISTINCT",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
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

impl Validator for ReplHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
        Ok(check_complete(ctx.input()))
    }
}

/// 判断输入是否为完整的 TQL 语句（供 Validator 和单元测试共用）。
fn check_complete(input: &str) -> ValidationResult {
    let trimmed = input.trim();

    // 空行或元命令直接提交
    if trimmed.is_empty() || trimmed.starts_with('.') {
        return ValidationResult::Valid(None);
    }

    // 统计括号深度（跳过字符串字面量内部）
    let mut paren = 0i32; // ()
    let mut brace = 0i32; // {}
    let mut bracket = 0i32; // []
    let mut in_sq = false; // 单引号字符串
    let mut in_dq = false; // 双引号字符串
    let mut escape = false;

    for ch in trimmed.chars() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && (in_sq || in_dq) {
            escape = true;
            continue;
        }
        if in_sq {
            if ch == '\'' {
                in_sq = false;
            }
            continue;
        }
        if in_dq {
            if ch == '"' {
                in_dq = false;
            }
            continue;
        }
        match ch {
            '\'' => in_sq = true,
            '"' => in_dq = true,
            '(' => paren += 1,
            ')' => paren -= 1,
            '{' => brace += 1,
            '}' => brace -= 1,
            '[' => bracket += 1,
            ']' => bracket -= 1,
            _ => {}
        }
    }

    // 未关闭的字符串或括号 → 继续输入
    if in_sq || in_dq || paren > 0 || brace > 0 || bracket > 0 {
        return ValidationResult::Incomplete;
    }

    // 缺少末尾分号 → 继续输入
    if !trimmed.ends_with(';') {
        return ValidationResult::Incomplete;
    }

    ValidationResult::Valid(None)
}
impl Helper for ReplHelper {}
