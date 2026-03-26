/// 手写词法分析器（Tokenizer / Lexer）
/// 零外部依赖，纯 Rust 迭代器实现。

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // 关键字
    Match,
    Where,
    Return,
    And,
    Or,

    // 标识符 & 字面量
    Ident(String),
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),
    BoolLit(bool),

    // 符号
    LParen,    // (
    RParen,    // )
    LBracket,  // [
    RBracket,  // ]
    LBrace,    // {
    RBrace,    // }
    Colon,     // :
    Dot,       // .
    Comma,     // ,
    Arrow,     // ->
    Dash,      // -

    // 运算符
    Eq,        // ==
    Ne,        // !=
    Gte,       // >=
    Lte,       // <=
    Gt,        // >
    Lt,        // <

    Eof,
}

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
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

    pub fn tokenize(&mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();

        loop {
            self.skip_whitespace();

            match self.peek() {
                None => {
                    tokens.push(Token::Eof);
                    break;
                }
                Some(ch) => {
                    let tok = match ch {
                        '(' => { self.advance(); Token::LParen }
                        ')' => { self.advance(); Token::RParen }
                        '[' => { self.advance(); Token::LBracket }
                        ']' => { self.advance(); Token::RBracket }
                        '{' => { self.advance(); Token::LBrace }
                        '}' => { self.advance(); Token::RBrace }
                        ':' => { self.advance(); Token::Colon }
                        '.' => { self.advance(); Token::Dot }
                        ',' => { self.advance(); Token::Comma }

                        '-' => {
                            self.advance();
                            if self.peek() == Some('>') {
                                self.advance();
                                Token::Arrow
                            } else {
                                Token::Dash
                            }
                        }

                        '=' => {
                            self.advance();
                            if self.peek() == Some('=') {
                                self.advance();
                                Token::Eq
                            } else {
                                return Err("Expected '==' but got '='".into());
                            }
                        }

                        '!' => {
                            self.advance();
                            if self.peek() == Some('=') {
                                self.advance();
                                Token::Ne
                            } else {
                                return Err("Expected '!=' but got '!'".into());
                            }
                        }

                        '>' => {
                            self.advance();
                            if self.peek() == Some('=') {
                                self.advance();
                                Token::Gte
                            } else {
                                Token::Gt
                            }
                        }

                        '<' => {
                            self.advance();
                            if self.peek() == Some('=') {
                                self.advance();
                                Token::Lte
                            } else {
                                Token::Lt
                            }
                        }

                        '"' | '\'' => {
                            let quote = ch;
                            self.advance(); // skip opening quote
                            let mut s = String::new();
                            loop {
                                match self.advance() {
                                    Some(c) if c == quote => break,
                                    Some(c) => s.push(c),
                                    None => return Err("Unterminated string literal".into()),
                                }
                            }
                            Token::StringLit(s)
                        }

                        c if c.is_ascii_digit() => {
                            let mut num_str = String::new();
                            let mut is_float = false;
                            while let Some(c) = self.peek() {
                                if c.is_ascii_digit() {
                                    num_str.push(c);
                                    self.advance();
                                } else if c == '.' && !is_float {
                                    is_float = true;
                                    num_str.push(c);
                                    self.advance();
                                } else {
                                    break;
                                }
                            }
                            if is_float {
                                Token::FloatLit(num_str.parse().map_err(|e| format!("Bad float: {}", e))?)
                            } else {
                                Token::IntLit(num_str.parse().map_err(|e| format!("Bad int: {}", e))?)
                            }
                        }

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
                            // 关键字匹配（大小写不敏感）
                            match ident.to_uppercase().as_str() {
                                "MATCH" => Token::Match,
                                "WHERE" => Token::Where,
                                "RETURN" => Token::Return,
                                "AND" => Token::And,
                                "OR" => Token::Or,
                                "TRUE" => Token::BoolLit(true),
                                "FALSE" => Token::BoolLit(false),
                                _ => Token::Ident(ident),
                            }
                        }

                        _ => return Err(format!("Unexpected character: '{}'", ch)),
                    };
                    tokens.push(tok);
                }
            }
        }

        Ok(tokens)
    }
}
