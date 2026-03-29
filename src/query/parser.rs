/// 递归下降语法分析器（Parser）
/// 将 Token 流解析为 AST。零外部依赖。
///
/// 语法：
///   Query       := MATCH Pattern (WHERE Condition)? RETURN ReturnList
///   Pattern     := NodePat (EdgePat NodePat)*
///   NodePat     := '(' Ident? ('{' PropList '}')? ')'
///   EdgePat     := '-[' (':' Ident)? ']->'
///   Condition   := CompareExpr ((AND | OR) CompareExpr)*
///   CompareExpr := Expr CompOp Expr
///   Expr        := Ident '.' Ident | Literal
///   ReturnList  := Ident (',' Ident)*
use super::ast::*;
use super::lexer::Token;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        let tok = self.advance();
        if &tok == expected {
            Ok(())
        } else {
            Err(format!("Expected {:?}, got {:?}", expected, tok))
        }
    }

    /// 入口：解析完整查询
    pub fn parse_query(&mut self) -> Result<Query, String> {
        // MATCH
        self.expect(&Token::Match)?;
        let pattern = self.parse_pattern()?;

        // WHERE (可选)
        let where_clause = if self.peek() == &Token::Where {
            self.advance();
            Some(self.parse_condition()?)
        } else {
            None
        };

        // RETURN
        self.expect(&Token::Return)?;
        let return_vars = self.parse_return_list()?;

        Ok(Query {
            pattern,
            where_clause,
            return_vars,
        })
    }

    /// 解析路径模式: (a)-[:rel]->(b)-[:rel2]->(c)
    fn parse_pattern(&mut self) -> Result<Pattern, String> {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        // 第一个节点（允许匿名，用于纯起点匹配场景）
        nodes.push(self.parse_node_pattern()?);

        // 后续的 边->节点 对
        while self.peek() == &Token::Dash {
            edges.push(self.parse_edge_pattern()?);
            let next_node = self.parse_node_pattern()?;
            // 路径中间/末尾的节点必须有变量名，否则执行器无法追踪遍历状态
            if next_node.var.is_none() {
                return Err(
                    "路径中间或末尾的节点必须指定变量名（如 (b)），不支持匿名中间节点".to_string(),
                );
            }
            nodes.push(next_node);
        }

        // 如果路径有边，起始节点也必须有变量名（执行器需要它来获取当前节点 ID）
        if !edges.is_empty() && nodes[0].var.is_none() {
            return Err("包含边的路径中，起始节点也必须指定变量名（如 (a)）".to_string());
        }

        Ok(Pattern { nodes, edges })
    }

    /// (varName {key: value, ...})
    fn parse_node_pattern(&mut self) -> Result<NodePattern, String> {
        self.expect(&Token::LParen)?;

        // 变量名（可选）
        let var = if let Token::Ident(_) = self.peek() {
            if let Token::Ident(name) = self.advance() {
                Some(name)
            } else {
                None
            }
        } else {
            None
        };

        // 内联属性过滤 {key: val, ...}（可选）
        let mut props = Vec::new();
        if self.peek() == &Token::LBrace {
            self.advance(); // {
            while self.peek() != &Token::RBrace {
                let key = match self.advance() {
                    Token::Ident(k) => k,
                    other => return Err(format!("Expected property key, got {:?}", other)),
                };
                self.expect(&Token::Colon)?;
                let value = self.parse_lit_value()?;
                props.push(PropFilter { key, value });
                if self.peek() == &Token::Comma {
                    self.advance();
                }
            }
            self.expect(&Token::RBrace)?; // }
        }

        self.expect(&Token::RParen)?;

        Ok(NodePattern { var, props })
    }

    /// -[:label]-> 或 -[]->
    fn parse_edge_pattern(&mut self) -> Result<EdgePattern, String> {
        self.expect(&Token::Dash)?;
        self.expect(&Token::LBracket)?;

        let label = if self.peek() == &Token::Colon {
            self.advance(); // :
            match self.advance() {
                Token::Ident(l) => Some(l),
                other => return Err(format!("Expected edge label, got {:?}", other)),
            }
        } else {
            None
        };

        self.expect(&Token::RBracket)?;
        self.expect(&Token::Arrow)?;

        Ok(EdgePattern { label })
    }

    /// 条件表达式：a.x == 1 AND b.y > 2 OR ...
    fn parse_condition(&mut self) -> Result<Condition, String> {
        let mut left = self.parse_comparison()?;

        loop {
            match self.peek() {
                Token::And => {
                    self.advance();
                    let right = self.parse_comparison()?;
                    left = Condition::And(Box::new(left), Box::new(right));
                }
                Token::Or => {
                    self.advance();
                    let right = self.parse_comparison()?;
                    left = Condition::Or(Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }

        Ok(left)
    }

    /// 单个比较: expr op expr
    fn parse_comparison(&mut self) -> Result<Condition, String> {
        let left = self.parse_expr()?;
        let op = self.parse_comp_op()?;
        let right = self.parse_expr()?;
        Ok(Condition::Compare { left, op, right })
    }

    /// 表达式: var.field 或 字面量
    fn parse_expr(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Token::Ident(_) => {
                let ident = match self.advance() {
                    Token::Ident(s) => s,
                    _ => unreachable!(),
                };
                if self.peek() == &Token::Dot {
                    self.advance(); // .
                    let field = match self.advance() {
                        Token::Ident(f) => f,
                        other => {
                            return Err(format!("Expected field name after '.', got {:?}", other));
                        }
                    };
                    Ok(Expr::Property { var: ident, field })
                } else {
                    // 裸标识符当做字符串字面量
                    Ok(Expr::Literal(LitValue::Str(ident)))
                }
            }
            Token::IntLit(_) | Token::FloatLit(_) | Token::StringLit(_) | Token::BoolLit(_) => {
                let lit = self.parse_lit_value()?;
                Ok(Expr::Literal(lit))
            }
            other => Err(format!("Expected expression, got {:?}", other)),
        }
    }

    fn parse_comp_op(&mut self) -> Result<CompOp, String> {
        match self.advance() {
            Token::Eq => Ok(CompOp::Eq),
            Token::Ne => Ok(CompOp::Ne),
            Token::Gt => Ok(CompOp::Gt),
            Token::Gte => Ok(CompOp::Gte),
            Token::Lt => Ok(CompOp::Lt),
            Token::Lte => Ok(CompOp::Lte),
            other => Err(format!("Expected comparison operator, got {:?}", other)),
        }
    }

    fn parse_lit_value(&mut self) -> Result<LitValue, String> {
        match self.advance() {
            Token::IntLit(n) => Ok(LitValue::Int(n)),
            Token::FloatLit(f) => Ok(LitValue::Float(f)),
            Token::StringLit(s) => Ok(LitValue::Str(s)),
            Token::BoolLit(b) => Ok(LitValue::Bool(b)),
            other => Err(format!("Expected literal value, got {:?}", other)),
        }
    }

    /// RETURN a, b, c
    fn parse_return_list(&mut self) -> Result<Vec<String>, String> {
        let mut vars = Vec::new();
        match self.advance() {
            Token::Ident(s) => vars.push(s),
            other => return Err(format!("Expected variable name in RETURN, got {:?}", other)),
        }
        while self.peek() == &Token::Comma {
            self.advance();
            match self.advance() {
                Token::Ident(s) => vars.push(s),
                other => return Err(format!("Expected variable name, got {:?}", other)),
            }
        }
        Ok(vars)
    }
}

/// 便捷入口：输入字符串 → 输出 AST
pub fn parse(input: &str) -> Result<Query, String> {
    let mut lexer = super::lexer::Lexer::new(input);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    parser.parse_query()
}
