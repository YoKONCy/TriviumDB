//! TQL 词法分析器
//!
//! 扩展自现有 Cypher Lexer，新增支持：
//! - 关键字: FIND, SEARCH, VECTOR, TOP, EXPAND, MATCHES, ORDER, BY, ASC, DESC, OFFSET, NOT
//! - Phase 2 新增: DISTINCT, AS, OPTIONAL, COUNT, SUM, AVG, MIN, MAX, COLLECT
//! - 操作符: `$eq`, `$gt` 等 Mongo 操作符（作为特殊标识符）
//! - 符号: `|`（多标签 OR）, `*`（可变长）, `..`（范围）

#[derive(Debug, Clone, PartialEq)]
pub enum TqlToken {
    // ── 关键字 (继承) ──
    Match,
    With,
    Where,
    Return,
    Limit,
    And,
    Or,

    // ── 关键字 (TQL 新增) ──
    Find,
    Search,
    Vector,
    Top,
    Expand,
    Rank,
    Outgoing,
    Incoming,
    Both,
    Matches,
    Order,
    By,
    Asc,
    Desc,
    Offset,
    Not,

    // ── 关键字 (Phase 2 新增) ──
    Distinct,
    As,
    Optional,
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Collect,
    Explain,
    Analyze,
    Create,
    Set,
    Delete,
    Detach,

    // ── 标识符 & 字面量 ──
    Ident(String),
    /// $eq, $gt 等 Mongo 操作符（含 $ 前缀）
    DollarOp(String),
    IntLit(i64),
    UIntLit(u64),
    FloatLit(f64),
    StringLit(String),
    BoolLit(bool),
    Null,

    // ── 符号 ──
    LParen,    // (
    RParen,    // )
    LBracket,  // [
    RBracket,  // ]
    LBrace,    // {
    RBrace,    // }
    Colon,     // :
    Dot,       // .
    DotDot,    // ..
    Comma,     // ,
    Arrow,     // ->
    LeftArrow, // <-
    Dash,      // -
    Pipe,      // |
    Star,      // *
    Plus,      // +
    Slash,     // /
    Is,

    // ── 比较运算符 ──
    Eq,  // ==
    Ne,  // !=
    Gte, // >=
    Lte, // <=
    Gt,  // >
    Lt,  // <

    Eof,
}

/// 带源位置信息的 token（用于错误诊断高亮）
#[derive(Debug, Clone)]
pub struct PosToken {
    pub token: TqlToken,
    /// 在原始输入字符串中的字节起始位置
    pub byte_start: usize,
}

/// 词法单元的原始半开字节范围，保留转义前的字符串及参数前缀。
#[derive(Debug, Clone)]
pub struct SpanToken {
    pub token: TqlToken,
    pub byte_start: usize,
    pub byte_end: usize,
}

/// 带位置的词法/语法错误
#[derive(Debug, Clone)]
pub struct ParseErrorAt {
    pub msg: String,
    /// 在原始输入字符串中的字节位置（错误锚点）
    pub byte_pos: usize,
}

impl ParseErrorAt {
    pub fn new(msg: impl Into<String>, byte_pos: usize) -> Self {
        Self {
            msg: msg.into(),
            byte_pos,
        }
    }
}

impl std::fmt::Display for ParseErrorAt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at byte {})", self.msg, self.byte_pos)
    }
}

impl std::error::Error for ParseErrorAt {}

/// 输入字节与 token 上限在解析树分配前生效。
pub const MAX_TQL_INPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TQL_TOKENS: usize = 262_144;

pub struct TqlLexer {
    chars: Vec<char>,
    pos: usize,
    /// 与 chars 平行：chars[i] 在原始输入字符串中的字节起始偏移
    char_byte_offsets: Vec<usize>,
    input_too_large: bool,
    /// 当前 token 的起点；消费失败后仍指向被拒绝的词法单元。
    token_byte_start: usize,
    /// 预算拒绝发生在 token 之间，保留该边界的零长度范围。
    error_end: Option<usize>,
}

impl TqlLexer {
    pub fn new(input: &str) -> Self {
        if input.len() > MAX_TQL_INPUT_BYTES {
            return Self {
                chars: Vec::new(),
                pos: 0,
                char_byte_offsets: vec![0],
                input_too_large: true,
                token_byte_start: 0,
                error_end: None,
            };
        }
        let chars: Vec<char> = input.chars().collect();
        let mut char_byte_offsets = Vec::with_capacity(chars.len() + 1);
        let mut byte = 0usize;
        for c in &chars {
            char_byte_offsets.push(byte);
            byte += c.len_utf8();
        }
        char_byte_offsets.push(byte); // 末尾哨兵：input.len()
        Self {
            chars,
            pos: 0,
            char_byte_offsets,
            input_too_large: false,
            token_byte_start: 0,
            error_end: None,
        }
    }

    /// 当前字符位置 -> 原始输入字节偏移
    fn current_byte_pos(&self) -> usize {
        self.char_byte_offsets
            .get(self.pos)
            .copied()
            .unwrap_or_else(|| *self.char_byte_offsets.last().unwrap_or(&0))
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_ahead(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.chars.get(self.pos).copied();
        self.pos += 1;
        ch
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    /// 跳过单行注释 (-- 开头)
    fn skip_comment(&mut self) {
        while let Some(ch) = self.peek() {
            if ch == '\n' {
                break;
            }
            self.advance();
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<TqlToken>, String> {
        self.tokenize_inner(None)
    }

    /// 同时返回 token 流与每个 token 在输入中的字节起始位置。
    /// 错误指向失败词法单元的起点。
    pub fn tokenize_with_positions(&mut self) -> Result<Vec<PosToken>, ParseErrorAt> {
        self.tokenize_with_spans().map(|tokens| {
            tokens
                .into_iter()
                .map(|token| PosToken {
                    token: token.token,
                    byte_start: token.byte_start,
                })
                .collect()
        })
    }

    /// 同步记录扫描器实际消费的终点，空白与注释不属于相邻 token。
    pub fn tokenize_with_spans(&mut self) -> Result<Vec<SpanToken>, ParseErrorAt> {
        let mut ranges = Vec::new();
        let tokens = self
            .tokenize_inner(Some(&mut ranges))
            .map_err(|msg| ParseErrorAt::new(msg, self.token_byte_start))?;
        debug_assert_eq!(tokens.len(), ranges.len());
        Ok(tokens
            .into_iter()
            .zip(ranges)
            .map(|(token, range)| SpanToken {
                token,
                byte_start: range.start,
                byte_end: range.end,
            })
            .collect())
    }

    /// 消费失败使用已消费范围，未消费的非法字符使用该字符的完整范围。
    pub(crate) fn error_byte_end(&self) -> usize {
        if let Some(end) = self.error_end {
            return end;
        }
        let end = self.current_byte_pos();
        if end > self.token_byte_start {
            end
        } else {
            end + self.peek().map_or(0, char::len_utf8)
        }
    }

    fn tokenize_inner(
        &mut self,
        mut positions: Option<&mut Vec<std::ops::Range<usize>>>,
    ) -> Result<Vec<TqlToken>, String> {
        self.error_end = None;
        if self.input_too_large {
            return Err(format!(
                "TQL 输入超过 {MAX_TQL_INPUT_BYTES} 字节 (TQL input byte limit exceeded)"
            ));
        }
        let mut tokens = Vec::new();

        loop {
            self.token_byte_start = self.current_byte_pos();
            if tokens.len() >= MAX_TQL_TOKENS {
                self.error_end = Some(self.token_byte_start);
                return Err(format!(
                    "TQL token 数量超过 {MAX_TQL_TOKENS} (TQL token limit exceeded)"
                ));
            }
            self.skip_whitespace();
            self.token_byte_start = self.current_byte_pos();

            match self.peek() {
                None => {
                    if let Some(rec) = positions.as_deref_mut() {
                        rec.push(self.current_byte_pos()..self.current_byte_pos());
                    }
                    tokens.push(TqlToken::Eof);
                    break;
                }
                Some(ch) => {
                    let token_byte_start = self.current_byte_pos();
                    let tok = match ch {
                        '(' => {
                            self.advance();
                            TqlToken::LParen
                        }
                        ')' => {
                            self.advance();
                            TqlToken::RParen
                        }
                        '[' => {
                            self.advance();
                            TqlToken::LBracket
                        }
                        ']' => {
                            self.advance();
                            TqlToken::RBracket
                        }
                        '{' => {
                            self.advance();
                            TqlToken::LBrace
                        }
                        '}' => {
                            self.advance();
                            TqlToken::RBrace
                        }
                        ':' => {
                            self.advance();
                            TqlToken::Colon
                        }
                        ',' => {
                            self.advance();
                            TqlToken::Comma
                        }
                        '|' => {
                            self.advance();
                            TqlToken::Pipe
                        }
                        '*' => {
                            self.advance();
                            TqlToken::Star
                        }
                        '+' => {
                            self.advance();
                            TqlToken::Plus
                        }
                        '/' => {
                            self.advance();
                            TqlToken::Slash
                        }

                        '.' => {
                            self.advance();
                            if self.peek() == Some('.') {
                                self.advance();
                                TqlToken::DotDot
                            } else {
                                TqlToken::Dot
                            }
                        }

                        '-' => {
                            self.advance();
                            if self.peek() == Some('>') {
                                self.advance();
                                TqlToken::Arrow
                            } else if self.peek() == Some('-') {
                                // 单行注释: --
                                self.advance();
                                self.skip_comment();
                                continue;
                            } else {
                                // 检查是否是负数: - 后面紧跟数字
                                if let Some(c) = self.peek() {
                                    if c.is_ascii_digit() {
                                        let num_tok = self.read_number()?;
                                        match num_tok {
                                            TqlToken::IntLit(n) => TqlToken::IntLit(-n),
                                            TqlToken::UIntLit(n)=>TqlToken::IntLit(i64::try_from(-i128::from(n)).map_err(|_|"负整数字面量超出 i64 范围 (Negative integer literal exceeds i64 range)".to_owned())?),
                                            TqlToken::FloatLit(f) => TqlToken::FloatLit(-f),
                                            _ => TqlToken::Dash,
                                        }
                                    } else {
                                        TqlToken::Dash
                                    }
                                } else {
                                    TqlToken::Dash
                                }
                            }
                        }

                        '=' => {
                            self.advance();
                            if self.peek() == Some('=') {
                                self.advance();
                                TqlToken::Eq
                            } else {
                                return Err("Expected '==' but got '='".into());
                            }
                        }

                        '!' => {
                            self.advance();
                            if self.peek() == Some('=') {
                                self.advance();
                                TqlToken::Ne
                            } else {
                                return Err("Expected '!=' but got '!'".into());
                            }
                        }

                        '>' => {
                            self.advance();
                            if self.peek() == Some('=') {
                                self.advance();
                                TqlToken::Gte
                            } else {
                                TqlToken::Gt
                            }
                        }

                        '<' => {
                            self.advance();
                            if self.peek() == Some('=') {
                                self.advance();
                                TqlToken::Lte
                            } else if self.peek() == Some('-') {
                                self.advance();
                                TqlToken::LeftArrow
                            } else {
                                TqlToken::Lt
                            }
                        }

                        '"' | '\'' => {
                            let quote = ch;
                            self.advance();
                            let mut s = String::new();
                            loop {
                                match self.advance() {
                                    Some('\\') => {
                                        // 转义字符支持
                                        match self.advance() {
                                            Some('n') => s.push('\n'),
                                            Some('t') => s.push('\t'),
                                            Some('\\') => s.push('\\'),
                                            Some(c) if c == quote => s.push(c),
                                            Some(c) => {
                                                s.push('\\');
                                                s.push(c);
                                            }
                                            None => return Err("Unterminated string escape".into()),
                                        }
                                    }
                                    Some(c) if c == quote => break,
                                    Some(c) => s.push(c),
                                    None => return Err("Unterminated string literal".into()),
                                }
                            }
                            TqlToken::StringLit(s)
                        }

                        '$' => {
                            // Mongo 操作符: $eq, $gt, $in, etc.
                            self.advance();
                            let mut name = String::from("$");
                            while let Some(c) = self.peek() {
                                if c.is_ascii_alphanumeric() || c == '_' {
                                    name.push(c);
                                    self.advance();
                                } else {
                                    break;
                                }
                            }
                            TqlToken::DollarOp(name)
                        }

                        c if c.is_ascii_digit() => self.read_number()?,

                        c if c.is_ascii_alphabetic() || c == '_' => {
                            let mut ident = String::new();
                            while let Some(c) = self.peek() {
                                if c.is_ascii_alphanumeric() || c == '_' {
                                    ident.push(c);
                                    self.advance();
                                } else {
                                    break;
                                }
                            }
                            match ident.to_uppercase().as_str() {
                                "MATCH" => TqlToken::Match,
                                "WITH" => TqlToken::With,
                                "WHERE" => TqlToken::Where,
                                "RETURN" => TqlToken::Return,
                                "LIMIT" => TqlToken::Limit,
                                "AND" => TqlToken::And,
                                "OR" => TqlToken::Or,
                                "NOT" => TqlToken::Not,
                                "IS" => TqlToken::Is,
                                "FIND" => TqlToken::Find,
                                "SEARCH" => TqlToken::Search,
                                "VECTOR" => TqlToken::Vector,
                                "TOP" => TqlToken::Top,
                                "EXPAND" => TqlToken::Expand,
                                "RANK" => TqlToken::Rank,
                                "OUTGOING" => TqlToken::Outgoing,
                                "INCOMING" => TqlToken::Incoming,
                                "BOTH" => TqlToken::Both,
                                "MATCHES" => TqlToken::Matches,
                                "ORDER" => TqlToken::Order,
                                "BY" => TqlToken::By,
                                "ASC" => TqlToken::Asc,
                                "DESC" => TqlToken::Desc,
                                "OFFSET" => TqlToken::Offset,
                                "DISTINCT" => TqlToken::Distinct,
                                "AS" => TqlToken::As,
                                "OPTIONAL" => TqlToken::Optional,
                                "COUNT" => TqlToken::Count,
                                "SUM" => TqlToken::Sum,
                                "AVG" => TqlToken::Avg,
                                "MIN" => TqlToken::Min,
                                "MAX" => TqlToken::Max,
                                "COLLECT" => TqlToken::Collect,
                                "EXPLAIN" => TqlToken::Explain,
                                "ANALYZE" => TqlToken::Analyze,
                                "CREATE" => TqlToken::Create,
                                "SET" => TqlToken::Set,
                                "DELETE" => TqlToken::Delete,
                                "DETACH" => TqlToken::Detach,
                                "TRUE" => TqlToken::BoolLit(true),
                                "FALSE" => TqlToken::BoolLit(false),
                                "NULL" => TqlToken::Null,
                                _ => TqlToken::Ident(ident),
                            }
                        }

                        _ => return Err(format!("Unexpected character: '{}'", ch)),
                    };
                    if let Some(rec) = positions.as_deref_mut() {
                        rec.push(token_byte_start..self.current_byte_pos());
                    }
                    tokens.push(tok);
                }
            }
        }

        Ok(tokens)
    }

    /// 读取十进制数字，支持整数、小数和科学计数法。
    fn read_number(&mut self) -> Result<TqlToken, String> {
        let mut num_str = String::new();
        let mut is_float = false;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            num_str.push(self.advance().unwrap_or_default());
        }
        if self.peek() == Some('.') && self.peek_ahead(1).is_some_and(|c| c.is_ascii_digit()) {
            // 只有“数字.数字”才是小数，“数字..”保留为整数和 DotDot。
            is_float = true;
            num_str.push(self.advance().unwrap_or_default());
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                num_str.push(self.advance().unwrap_or_default());
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            is_float = true;
            num_str.push(self.advance().unwrap_or_default());
            if matches!(self.peek(), Some('+' | '-')) {
                num_str.push(self.advance().unwrap_or_default());
            }
            if !self.peek().is_some_and(|c| c.is_ascii_digit()) {
                return Err(format!("Bad float exponent: {num_str}"));
            }
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                num_str.push(self.advance().unwrap_or_default());
            }
        }
        if is_float {
            let value: f64 = num_str.parse().map_err(|e| format!("Bad float: {e}"))?;
            if !value.is_finite() {
                return Err(format!("Non-finite float literal: {num_str}"));
            }
            Ok(TqlToken::FloatLit(value))
        } else {
            if let Ok(value) = num_str.parse::<i64>() {
                Ok(TqlToken::IntLit(value))
            } else {
                num_str
                    .parse::<u64>()
                    .map(TqlToken::UIntLit)
                    .map_err(|e| format!("整数字面量越界 (Integer literal out of range): {e}"))
            }
        }
    }
}
