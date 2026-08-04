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
    Create,
    Set,
    Delete,
    Detach,

    // ── 标识符 & 字面量 ──
    Ident(String),
    /// $eq, $gt 等 Mongo 操作符（含 $ 前缀）
    DollarOp(String),
    IntLit(i64),
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

pub struct TqlLexer {
    chars: Vec<char>,
    pos: usize,
    /// 与 chars 平行：chars[i] 在原始输入字符串中的字节起始偏移
    char_byte_offsets: Vec<usize>,
}

impl TqlLexer {
    pub fn new(input: &str) -> Self {
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
    /// 错误也带位置（指向当前字符的字节偏移）。
    pub fn tokenize_with_positions(&mut self) -> Result<Vec<PosToken>, ParseErrorAt> {
        let mut positions = Vec::new();
        let tokens = self
            .tokenize_inner(Some(&mut positions))
            .map_err(|msg| ParseErrorAt::new(msg, self.current_byte_pos()))?;
        debug_assert_eq!(tokens.len(), positions.len());
        Ok(tokens
            .into_iter()
            .zip(positions)
            .map(|(token, byte_start)| PosToken { token, byte_start })
            .collect())
    }

    fn tokenize_inner(
        &mut self,
        mut positions: Option<&mut Vec<usize>>,
    ) -> Result<Vec<TqlToken>, String> {
        let mut tokens = Vec::new();

        loop {
            self.skip_whitespace();

            match self.peek() {
                None => {
                    if let Some(rec) = positions.as_deref_mut() {
                        rec.push(self.current_byte_pos());
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
                                "WHERE" => TqlToken::Where,
                                "RETURN" => TqlToken::Return,
                                "LIMIT" => TqlToken::Limit,
                                "AND" => TqlToken::And,
                                "OR" => TqlToken::Or,
                                "NOT" => TqlToken::Not,
                                "FIND" => TqlToken::Find,
                                "SEARCH" => TqlToken::Search,
                                "VECTOR" => TqlToken::Vector,
                                "TOP" => TqlToken::Top,
                                "EXPAND" => TqlToken::Expand,
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
                        rec.push(token_byte_start);
                    }
                    tokens.push(tok);
                }
            }
        }

        Ok(tokens)
    }

    /// 读取数字（整数或浮点数）
    fn read_number(&mut self) -> Result<TqlToken, String> {
        let mut num_str = String::new();
        let mut is_float = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                num_str.push(c);
                self.advance();
            } else if c == '.'
                && !is_float
                && self.peek_ahead(1).is_some_and(|c| c.is_ascii_digit())
            {
                // 只有 "数字.数字" 才是浮点数，"数字.." 是整数 + DotDot
                is_float = true;
                num_str.push(c);
                self.advance();
            } else {
                break;
            }
        }
        if is_float {
            Ok(TqlToken::FloatLit(
                num_str.parse().map_err(|e| format!("Bad float: {}", e))?,
            ))
        } else {
            Ok(TqlToken::IntLit(
                num_str.parse().map_err(|e| format!("Bad int: {}", e))?,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_query() {
        let mut lexer = TqlLexer::new(r#"FIND {type: "event", heat: {$gte: 0.7}} RETURN *"#);
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], TqlToken::Find);
        assert_eq!(tokens[1], TqlToken::LBrace);
        // type: "event"
        assert_eq!(tokens[2], TqlToken::Ident("type".into()));
        assert_eq!(tokens[3], TqlToken::Colon);
        assert_eq!(tokens[4], TqlToken::StringLit("event".into()));
        // $gte: 0.7
        assert!(tokens.contains(&TqlToken::DollarOp("$gte".into())));
        assert!(tokens.contains(&TqlToken::Star));
    }

    #[test]
    fn test_match_variable_length() {
        let mut lexer = TqlLexer::new("MATCH (a)-[:knows*1..3]->(b) RETURN b");
        let tokens = lexer.tokenize().unwrap();
        assert!(tokens.contains(&TqlToken::Star));
        assert!(tokens.contains(&TqlToken::DotDot));
    }

    #[test]
    fn test_pipe_multi_label() {
        let mut lexer = TqlLexer::new("MATCH (a)-[:knows|works_at]->(b) RETURN b");
        let tokens = lexer.tokenize().unwrap();
        assert!(tokens.contains(&TqlToken::Pipe));
    }

    #[test]
    fn test_search_entry() {
        let mut lexer = TqlLexer::new("SEARCH VECTOR [0.1, -0.2, 0.3] TOP 10 RETURN *");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], TqlToken::Search);
        assert_eq!(tokens[1], TqlToken::Vector);
        assert_eq!(tokens[2], TqlToken::LBracket);
    }

    #[test]
    fn test_comment_skip() {
        let mut lexer = TqlLexer::new("FIND {type: \"event\"} -- this is a comment\nRETURN *");
        let tokens = lexer.tokenize().unwrap();
        assert!(tokens.contains(&TqlToken::Return));
    }

    #[test]
    fn test_order_by() {
        let mut lexer = TqlLexer::new("ORDER BY a.score DESC LIMIT 10 OFFSET 20");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], TqlToken::Order);
        assert_eq!(tokens[1], TqlToken::By);
        assert!(tokens.contains(&TqlToken::Desc));
        assert!(tokens.contains(&TqlToken::Offset));
    }

    #[test]
    fn test_negative_number() {
        let mut lexer = TqlLexer::new("[-0.5, -3]");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[1], TqlToken::FloatLit(-0.5));
        assert_eq!(tokens[3], TqlToken::IntLit(-3));
    }

    #[test]
    fn test_dot_dot_not_float() {
        // "1..3" should be IntLit(1), DotDot, IntLit(3) — NOT FloatLit(1.0) + error
        let mut lexer = TqlLexer::new("1..3");
        let tokens = lexer.tokenize().unwrap();
        assert_eq!(tokens[0], TqlToken::IntLit(1));
        assert_eq!(tokens[1], TqlToken::DotDot);
        assert_eq!(tokens[2], TqlToken::IntLit(3));
    }

    #[test]
    fn tokenize_with_positions_records_byte_starts() {
        let input = "MATCH (n) RETURN n";
        let mut lexer = TqlLexer::new(input);
        let toks = lexer.tokenize_with_positions().unwrap();
        // 第一个 token 是 MATCH，起始位置 0
        assert!(matches!(toks[0].token, TqlToken::Match));
        assert_eq!(toks[0].byte_start, 0);
        // (n) 中的 ( 在 "MATCH " 之后
        let lparen = toks
            .iter()
            .find(|t| matches!(t.token, TqlToken::LParen))
            .unwrap();
        assert_eq!(lparen.byte_start, 6);
        // EOF 位置 = 字符串长度
        let last = toks.last().unwrap();
        assert!(matches!(last.token, TqlToken::Eof));
        assert_eq!(last.byte_start, input.len());
    }

    #[test]
    fn tokenize_with_positions_lex_error_carries_byte_pos() {
        // '@' 是非法字符
        let input = "MATCH (n) @ RETURN n";
        let mut lexer = TqlLexer::new(input);
        let err = lexer.tokenize_with_positions().unwrap_err();
        assert!(err.msg.contains("Unexpected character"));
        assert_eq!(err.byte_pos, 10);
    }

    #[test]
    fn parse_tql_with_pos_reports_position() {
        // 缺少 RETURN
        let input = "MATCH (n) WHERE n.x == 1";
        let err = super::super::tql_parser::parse_tql_with_pos(input).unwrap_err();
        // 错误位置应在 EOF（即 input 末尾）
        assert!(err.byte_pos <= input.len());
        assert!(err.byte_pos >= input.find("==").unwrap());
    }

    #[test]
    fn parse_tql_with_pos_ok_returns_query() {
        let input = "MATCH (n) RETURN n LIMIT 5";
        let q = super::super::tql_parser::parse_tql_with_pos(input).unwrap();
        assert_eq!(q.limit, Some(5));
    }
}
