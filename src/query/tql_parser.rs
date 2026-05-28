//! TQL 递归下降语法分析器
//!
//! 将 TqlToken 流解析为 TqlQuery AST。
//! 支持查询入口：MATCH / OPTIONAL MATCH / FIND / SEARCH
//! 支持聚合函数、DISTINCT、AS 别名

use super::tql_ast::*;
use super::tql_lexer::{ParseErrorAt, TqlToken};
use crate::filter::Filter;

pub struct TqlParser {
    tokens: Vec<TqlToken>,
    pos: usize,
    depth: usize,
    /// 与 tokens 平行的字节起始位置（用于错误诊断）
    positions: Option<Vec<usize>>,
}

impl TqlParser {
    pub fn new(tokens: Vec<TqlToken>) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
            positions: None,
        }
    }

    /// 构造带位置信息的 parser（错误时可定位到字节偏移）
    pub fn new_with_positions(tokens: Vec<TqlToken>, positions: Vec<usize>) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
            positions: Some(positions),
        }
    }

    /// 当前 token 在原始输入中的字节起始位置；若未提供位置信息则返回 None
    pub fn current_byte_pos(&self) -> Option<usize> {
        let positions = self.positions.as_ref()?;
        positions
            .get(self.pos)
            .or_else(|| positions.last())
            .copied()
    }

    fn peek(&self) -> &TqlToken {
        self.tokens.get(self.pos).unwrap_or(&TqlToken::Eof)
    }

    fn advance(&mut self) -> TqlToken {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(TqlToken::Eof);
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &TqlToken) -> Result<(), String> {
        let tok = self.advance();
        if &tok == expected {
            Ok(())
        } else {
            Err(format!("Expected {:?}, got {:?}", expected, tok))
        }
    }

    fn at(&self, expected: &TqlToken) -> bool {
        self.peek() == expected
    }

    // ═══════════════════════════════════════════════════════════════
    //  顶层入口
    // ═══════════════════════════════════════════════════════════════

    pub fn parse_query(&mut self) -> Result<TqlQuery, String> {
        // 0. EXPLAIN 前缀（可选）
        let explain = if self.at(&TqlToken::Explain) {
            self.advance();
            true
        } else {
            false
        };

        // 1. 查询入口
        let entry = match self.peek() {
            TqlToken::Match => self.parse_match_entry()?,
            TqlToken::Optional => self.parse_optional_match_entry()?,
            TqlToken::Find => self.parse_find_entry()?,
            TqlToken::Search => self.parse_search_entry()?,
            other => {
                return Err(format!(
                    "Expected MATCH, OPTIONAL MATCH, FIND, or SEARCH, got {:?}",
                    other
                ));
            }
        };

        // 2. WHERE (可选)
        let predicate = if self.at(&TqlToken::Where) {
            self.advance();
            Some(self.parse_predicate()?)
        } else {
            None
        };

        // 3. RETURN
        self.expect(&TqlToken::Return)?;
        let returns = self.parse_return_clause()?;

        // 4. ORDER BY (可选)
        let order_by = if self.at(&TqlToken::Order) {
            self.advance();
            self.expect(&TqlToken::By)?;
            self.parse_order_by_list()?
        } else {
            Vec::new()
        };

        // 5. LIMIT (可选)
        let limit = if self.at(&TqlToken::Limit) {
            self.advance();
            Some(self.parse_positive_int()?)
        } else {
            None
        };

        // 6. OFFSET (可选)
        let offset = if self.at(&TqlToken::Offset) {
            self.advance();
            Some(self.parse_positive_int()?)
        } else {
            None
        };

        Ok(TqlQuery {
            explain,
            entry,
            predicate,
            returns,
            order_by,
            limit,
            offset,
        })
    }

    // ═══════════════════════════════════════════════════════════════
    //  MATCH 入口
    // ═══════════════════════════════════════════════════════════════

    fn parse_match_entry(&mut self) -> Result<QueryEntry, String> {
        self.expect(&TqlToken::Match)?;
        let pattern = self.parse_pattern()?;
        Ok(QueryEntry::Match { pattern })
    }

    fn parse_optional_match_entry(&mut self) -> Result<QueryEntry, String> {
        self.expect(&TqlToken::Optional)?;
        self.expect(&TqlToken::Match)?;
        let pattern = self.parse_pattern()?;
        Ok(QueryEntry::OptionalMatch { pattern })
    }

    fn parse_pattern(&mut self) -> Result<TqlPattern, String> {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        nodes.push(self.parse_node_pattern()?);

        while self.at(&TqlToken::Dash) || self.at(&TqlToken::LeftArrow) {
            edges.push(self.parse_edge_pattern()?);
            let next_node = self.parse_node_pattern()?;
            if next_node.var.is_none() {
                return Err("路径中间或末尾的节点必须指定变量名".into());
            }
            nodes.push(next_node);
        }

        if !edges.is_empty() && nodes[0].var.is_none() {
            return Err("包含边的路径中，起始节点也必须指定变量名".into());
        }

        Ok(TqlPattern { nodes, edges })
    }

    /// 节点模式: (var {doc_filter})
    fn parse_node_pattern(&mut self) -> Result<TqlNodePattern, String> {
        self.expect(&TqlToken::LParen)?;

        // 变量名（可选）
        let var = if let TqlToken::Ident(_) = self.peek() {
            if let TqlToken::Ident(name) = self.advance() {
                Some(name)
            } else {
                None
            }
        } else {
            None
        };

        // 内联文档过滤 {key: val, ...} (Q1: 支持 Mongo 操作符)
        // 空 {} 视为无条件（与旧 Cypher 兼容）
        let filter = if self.at(&TqlToken::LBrace) {
            // 探测是否为空 {}
            let saved_pos = self.pos;
            self.advance(); // consume {
            if self.at(&TqlToken::RBrace) {
                self.advance(); // consume }
                None // 空 {} = 无条件
            } else {
                self.pos = saved_pos; // 回溯
                Some(self.parse_doc_filter()?)
            }
        } else {
            None
        };

        self.expect(&TqlToken::RParen)?;

        Ok(TqlNodePattern { var, filter })
    }

    /// 边模式，支持三种方向：
    /// - 正向: -[:label]->
    /// - 反向: <-[:label]-
    /// - 双向: -[:label]-
    fn parse_edge_pattern(&mut self) -> Result<TqlEdgePattern, String> {
        // 判断起始方向
        let starts_with_left_arrow = self.at(&TqlToken::LeftArrow);
        if starts_with_left_arrow {
            self.advance(); // consume <-
        } else {
            self.expect(&TqlToken::Dash)?; // consume -
        }

        self.expect(&TqlToken::LBracket)?;

        let mut labels = Vec::new();
        let mut hop_range = None;

        // 标签部分（可选）
        if self.at(&TqlToken::Colon) {
            self.advance();
            // 第一个标签
            labels.push(self.parse_ident()?);
            // 管道符分隔的后续标签 (Q2: 多标签 OR)
            while self.at(&TqlToken::Pipe) {
                self.advance();
                labels.push(self.parse_ident()?);
            }
        }

        // 可变长跳数范围（可选）
        if self.at(&TqlToken::Star) {
            self.advance();
            let min = self.parse_positive_int()?;
            self.expect(&TqlToken::DotDot)?;
            let max = self.parse_positive_int()?;
            if min > max {
                return Err(format!("Hop range min ({}) > max ({})", min, max));
            }
            hop_range = Some(HopRange { min, max });
        }

        self.expect(&TqlToken::RBracket)?;

        // 判断结束方向
        let direction = if starts_with_left_arrow {
            // <-[...]-  反向（期望尾部是 -）
            self.expect(&TqlToken::Dash)?;
            EdgeDirection::Backward
        } else if self.at(&TqlToken::Arrow) {
            // -[...]->  正向
            self.advance();
            EdgeDirection::Forward
        } else if self.at(&TqlToken::Dash) {
            // -[]- 双向
            self.advance();
            EdgeDirection::Both
        } else {
            return Err(format!(
                "Expected '->' or '-' after edge pattern, got {:?}",
                self.peek()
            ));
        };

        Ok(TqlEdgePattern {
            labels,
            hop_range,
            direction,
        })
    }

    // ═══════════════════════════════════════════════════════════════
    //  FIND 入口
    // ═══════════════════════════════════════════════════════════════

    fn parse_find_entry(&mut self) -> Result<QueryEntry, String> {
        self.expect(&TqlToken::Find)?;
        let filter = self.parse_doc_filter()?;
        Ok(QueryEntry::Find { filter })
    }

    // ═══════════════════════════════════════════════════════════════
    //  SEARCH 入口
    // ═══════════════════════════════════════════════════════════════

    fn parse_search_entry(&mut self) -> Result<QueryEntry, String> {
        self.expect(&TqlToken::Search)?;
        self.expect(&TqlToken::Vector)?;

        // 向量字面量 [0.1, -0.2, ...]
        self.expect(&TqlToken::LBracket)?;
        let mut vector = Vec::new();
        loop {
            if self.at(&TqlToken::RBracket) {
                break;
            }
            let val = match self.advance() {
                TqlToken::FloatLit(f) => f,
                TqlToken::IntLit(n) => n as f64,
                other => return Err(format!("Expected number in vector, got {:?}", other)),
            };
            vector.push(val);
            if self.at(&TqlToken::Comma) {
                self.advance();
            }
        }
        self.expect(&TqlToken::RBracket)?;

        // TOP k
        self.expect(&TqlToken::Top)?;
        let top_k = self.parse_positive_int()?;

        // EXPAND (可选, Q3: Phase 2 只做 EXPAND)
        let expand = if self.at(&TqlToken::Expand) {
            self.advance();
            Some(self.parse_expand_clause()?)
        } else {
            None
        };

        Ok(QueryEntry::Search {
            vector,
            top_k,
            expand,
        })
    }

    /// EXPAND [:label*min..max]
    fn parse_expand_clause(&mut self) -> Result<ExpandClause, String> {
        self.expect(&TqlToken::LBracket)?;

        let mut labels = Vec::new();
        if self.at(&TqlToken::Colon) {
            self.advance();
            labels.push(self.parse_ident()?);
            while self.at(&TqlToken::Pipe) {
                self.advance();
                labels.push(self.parse_ident()?);
            }
        }

        // *min..max（必须）
        self.expect(&TqlToken::Star)?;
        let min_depth = self.parse_positive_int()?;
        self.expect(&TqlToken::DotDot)?;
        let max_depth = self.parse_positive_int()?;

        self.expect(&TqlToken::RBracket)?;

        Ok(ExpandClause {
            labels,
            min_depth,
            max_depth,
        })
    }

    // ═══════════════════════════════════════════════════════════════
    //  文档过滤 (MongoDB 风格)
    // ═══════════════════════════════════════════════════════════════

    /// 解析 {key: value, ...} 形式的文档过滤
    fn parse_doc_filter(&mut self) -> Result<Filter, String> {
        self.depth += 1;
        if self.depth > 128 {
            self.depth -= 1;
            return Err("Parser recursion depth exceeded (doc filter nesting too deep)".into());
        }
        let result = self.parse_doc_filter_inner();
        self.depth -= 1;
        result
    }

    /// parse_doc_filter 的内部实现，深度限制由外层 parse_doc_filter 统一守护
    fn parse_doc_filter_inner(&mut self) -> Result<Filter, String> {
        self.expect(&TqlToken::LBrace)?;

        let mut filters = Vec::new();

        while !self.at(&TqlToken::RBrace) {
            match self.peek().clone() {
                TqlToken::DollarOp(op) if op == "$and" || op == "$or" => {
                    let op = op.clone();
                    self.advance(); // $and / $or
                    self.expect(&TqlToken::Colon)?;
                    self.expect(&TqlToken::LBracket)?;

                    let mut sub_filters = Vec::new();
                    while !self.at(&TqlToken::RBracket) {
                        sub_filters.push(self.parse_doc_filter()?);
                        if self.at(&TqlToken::Comma) {
                            self.advance();
                        }
                    }
                    self.expect(&TqlToken::RBracket)?;

                    let combined = if op == "$and" {
                        Filter::And(sub_filters)
                    } else {
                        Filter::Or(sub_filters)
                    };
                    filters.push(combined);
                }

                TqlToken::Ident(_) | TqlToken::StringLit(_) => {
                    let field = self.parse_field_name()?;
                    self.expect(&TqlToken::Colon)?;

                    if self.at(&TqlToken::LBrace) {
                        // 操作符对象: {$gt: 18}
                        self.advance();
                        while !self.at(&TqlToken::RBrace) {
                            let op = match self.advance() {
                                TqlToken::DollarOp(s) => s,
                                other => {
                                    return Err(format!("Expected $operator, got {:?}", other));
                                }
                            };
                            self.expect(&TqlToken::Colon)?;

                            let f = self.parse_filter_op_value(&field, &op)?;
                            filters.push(f);

                            if self.at(&TqlToken::Comma) {
                                self.advance();
                            }
                        }
                        self.expect(&TqlToken::RBrace)?;
                    } else {
                        // 隐式 $eq: {name: "Alice"}
                        let val = self.parse_json_value()?;
                        filters.push(Filter::Eq(field, val));
                    }
                }

                _ => return Err(format!("Unexpected token in doc filter: {:?}", self.peek())),
            }

            if self.at(&TqlToken::Comma) {
                self.advance();
            }
        }

        self.expect(&TqlToken::RBrace)?;

        match filters.len() {
            0 => Err("文档过滤不能为空".into()),
            1 => Ok(filters
                .into_iter()
                .next()
                .expect("BUG: len==1 but next() returned None")),
            _ => Ok(Filter::And(filters)),
        }
    }

    /// 解析操作符值: $gt → Filter::Gt, $in → Filter::In, etc.
    fn parse_filter_op_value(&mut self, field: &str, op: &str) -> Result<Filter, String> {
        match op {
            "$eq" => Ok(Filter::Eq(field.into(), self.parse_json_value()?)),
            "$ne" => Ok(Filter::Ne(field.into(), self.parse_json_value()?)),
            "$gt" => Ok(Filter::Gt(field.into(), self.parse_json_number()?)),
            "$gte" => Ok(Filter::Gte(field.into(), self.parse_json_number()?)),
            "$lt" => Ok(Filter::Lt(field.into(), self.parse_json_number()?)),
            "$lte" => Ok(Filter::Lte(field.into(), self.parse_json_number()?)),
            "$in" => Ok(Filter::In(field.into(), self.parse_json_array()?)),
            "$nin" => Ok(Filter::Nin(field.into(), self.parse_json_array()?)),
            "$exists" => {
                let b = match self.advance() {
                    TqlToken::BoolLit(b) => b,
                    other => return Err(format!("$exists expects boolean, got {:?}", other)),
                };
                Ok(Filter::Exists(field.into(), b))
            }
            "$size" => {
                let n = self.parse_positive_int()?;
                Ok(Filter::Size(field.into(), n))
            }
            "$all" => Ok(Filter::All(field.into(), self.parse_json_array()?)),
            "$type" => {
                let t = match self.advance() {
                    TqlToken::StringLit(s) => s,
                    other => return Err(format!("$type expects string, got {:?}", other)),
                };
                Ok(Filter::TypeMatch(field.into(), t))
            }
            "$startsWith" => {
                let prefix = match self.advance() {
                    TqlToken::StringLit(s) => s,
                    other => return Err(format!("$startsWith expects string, got {:?}", other)),
                };
                Ok(Filter::StartsWith(field.into(), prefix))
            }
            "$contains" => {
                let substr = match self.advance() {
                    TqlToken::StringLit(s) => s,
                    other => return Err(format!("$contains expects string, got {:?}", other)),
                };
                Ok(Filter::Contains(field.into(), substr))
            }
            unknown => Err(format!("Unknown operator: {}", unknown)),
        }
    }

    // ═══════════════════════════════════════════════════════════════
    //  统一谓词 (WHERE 子句)
    // ═══════════════════════════════════════════════════════════════

    fn parse_predicate(&mut self) -> Result<Predicate, String> {
        self.depth += 1;
        if self.depth > 128 {
            self.depth -= 1;
            return Err("Parser recursion depth exceeded".into());
        }
        let result = self.parse_predicate_or();
        self.depth -= 1;
        result
    }

    /// OR 层
    fn parse_predicate_or(&mut self) -> Result<Predicate, String> {
        let mut left = self.parse_predicate_and()?;
        while self.at(&TqlToken::Or) {
            self.advance();
            let right = self.parse_predicate_and()?;
            left = Predicate::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// AND 层
    fn parse_predicate_and(&mut self) -> Result<Predicate, String> {
        let mut left = self.parse_predicate_atom()?;
        while self.at(&TqlToken::And) {
            self.advance();
            let right = self.parse_predicate_atom()?;
            left = Predicate::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// 原子谓词
    fn parse_predicate_atom(&mut self) -> Result<Predicate, String> {
        // NOT
        if self.at(&TqlToken::Not) {
            self.advance();
            let inner = self.parse_predicate_atom()?;
            return Ok(Predicate::Not(Box::new(inner)));
        }

        // 括号
        if self.at(&TqlToken::LParen) {
            self.advance();
            let inner = self.parse_predicate()?;
            self.expect(&TqlToken::RParen)?;
            return Ok(inner);
        }

        // 文档过滤: {field: {$op: val}}
        if self.at(&TqlToken::LBrace) {
            let filter = self.parse_doc_filter()?;
            return Ok(Predicate::DocFilter { var: None, filter });
        }

        // var MATCHES {doc_filter} 或 Cypher 比较: a.field op literal
        if let TqlToken::Ident(_) = self.peek() {
            // 先记录位置，探测是 MATCHES 还是比较
            let ident = match self.advance() {
                TqlToken::Ident(s) => s,
                _ => {
                    return Err(
                        "BUG: peek() was Ident but advance() returned different token".into(),
                    );
                }
            };

            // var MATCHES {doc_filter}
            if self.at(&TqlToken::Matches) {
                self.advance();
                let filter = self.parse_doc_filter()?;
                return Ok(Predicate::DocFilter {
                    var: Some(ident),
                    filter,
                });
            }

            // a.field op value (Cypher 比较)
            if self.at(&TqlToken::Dot) {
                self.advance();
                let field = self.parse_ident()?;
                let left = TqlExpr::Property { var: ident, field };
                let op = self.parse_comp_op()?;
                let right = self.parse_expr()?;
                return Ok(Predicate::Compare { left, op, right });
            }

            return Err(format!(
                "Unexpected token after identifier '{}': {:?}",
                ident,
                self.peek()
            ));
        }

        Err(format!("Unexpected token in predicate: {:?}", self.peek()))
    }

    // ═══════════════════════════════════════════════════════════════
    //  RETURN / ORDER BY
    // ═══════════════════════════════════════════════════════════════

    fn parse_return_clause(&mut self) -> Result<ReturnClause, String> {
        if self.at(&TqlToken::Star) {
            self.advance();
            return Ok(ReturnClause::All);
        }

        // 尝试解析为表达式列表（支持聚合、DISTINCT、属性访问、AS）
        let mut exprs = Vec::new();
        exprs.push(self.parse_return_expr()?);
        while self.at(&TqlToken::Comma) {
            self.advance();
            exprs.push(self.parse_return_expr()?);
        }

        // 如果所有项都是纯变量引用（无聚合、无 DISTINCT、无属性访问、无 alias），降级为 Variables
        let all_simple = exprs
            .iter()
            .all(|e| matches!(&e.kind, ReturnExprKind::Var(_)) && e.alias.is_none() && !e.distinct);

        if all_simple {
            let mut vars = Vec::with_capacity(exprs.len());
            for e in exprs {
                match e.kind {
                    ReturnExprKind::Var(v) => vars.push(v),
                    _ => return Err("BUG: expected Var in all_simple path".into()),
                }
            }
            Ok(ReturnClause::Variables(vars))
        } else {
            Ok(ReturnClause::Expressions(exprs))
        }
    }

    /// 解析单个 RETURN 表达式项
    ///
    /// 支持格式：
    /// - `a` — 变量引用
    /// - `a.name` — 属性访问
    /// - `DISTINCT a` / `DISTINCT a.name` — 去重
    /// - `count(b)` / `avg(b.age)` — 聚合函数
    /// - `... AS alias` — 别名
    fn parse_return_expr(&mut self) -> Result<ReturnExpr, String> {
        // DISTINCT 前缀
        let distinct = if self.at(&TqlToken::Distinct) {
            self.advance();
            true
        } else {
            false
        };

        // 聚合函数: count(...), sum(...), etc.
        let kind = if let Some(func) = self.try_parse_agg_func() {
            self.expect(&TqlToken::LParen)?;
            let inner = self.parse_return_expr_kind()?;
            self.expect(&TqlToken::RParen)?;
            ReturnExprKind::Aggregate(func, Box::new(inner))
        } else {
            self.parse_return_expr_kind()?
        };

        // AS 别名
        let alias = if self.at(&TqlToken::As) {
            self.advance();
            Some(self.parse_ident()?)
        } else {
            None
        };

        Ok(ReturnExpr {
            kind,
            alias,
            distinct,
        })
    }

    /// 解析 RETURN 表达式内部类型（变量或属性访问）
    fn parse_return_expr_kind(&mut self) -> Result<ReturnExprKind, String> {
        // * 在聚合内部不合法，这里只处理 ident 和 ident.field
        let ident = self.parse_ident()?;
        if self.at(&TqlToken::Dot) {
            self.advance();
            let field = self.parse_ident()?;
            Ok(ReturnExprKind::Property(ident, field))
        } else {
            Ok(ReturnExprKind::Var(ident))
        }
    }

    /// 尝试解析聚合函数关键字，不消耗 token（使用探测）
    fn try_parse_agg_func(&mut self) -> Option<AggFunc> {
        match self.peek() {
            TqlToken::Count => {
                self.advance();
                Some(AggFunc::Count)
            }
            TqlToken::Sum => {
                self.advance();
                Some(AggFunc::Sum)
            }
            TqlToken::Avg => {
                self.advance();
                Some(AggFunc::Avg)
            }
            TqlToken::Min => {
                self.advance();
                Some(AggFunc::Min)
            }
            TqlToken::Max => {
                self.advance();
                Some(AggFunc::Max)
            }
            TqlToken::Collect => {
                self.advance();
                Some(AggFunc::Collect)
            }
            _ => None,
        }
    }

    fn parse_order_by_list(&mut self) -> Result<Vec<OrderExpr>, String> {
        let mut items = Vec::new();
        items.push(self.parse_order_expr()?);
        while self.at(&TqlToken::Comma) {
            self.advance();
            items.push(self.parse_order_expr()?);
        }
        Ok(items)
    }

    fn parse_order_expr(&mut self) -> Result<OrderExpr, String> {
        let expr = self.parse_expr()?;
        let descending = if self.at(&TqlToken::Desc) {
            self.advance();
            true
        } else if self.at(&TqlToken::Asc) {
            self.advance();
            false
        } else {
            false // 默认升序
        };
        Ok(OrderExpr { expr, descending })
    }

    // ═══════════════════════════════════════════════════════════════
    //  表达式 & 辅助方法
    // ═══════════════════════════════════════════════════════════════

    fn parse_expr(&mut self) -> Result<TqlExpr, String> {
        match self.peek().clone() {
            TqlToken::Ident(_) => {
                let ident = match self.advance() {
                    TqlToken::Ident(s) => s,
                    _ => {
                        return Err(
                            "BUG: peek() was Ident but advance() returned different token".into(),
                        );
                    }
                };
                if self.at(&TqlToken::Dot) {
                    self.advance();
                    let field = self.parse_ident()?;
                    Ok(TqlExpr::Property { var: ident, field })
                } else {
                    // 裸标识符当做字符串字面量
                    Ok(TqlExpr::Literal(TqlLiteral::Str(ident)))
                }
            }
            TqlToken::IntLit(_)
            | TqlToken::FloatLit(_)
            | TqlToken::StringLit(_)
            | TqlToken::BoolLit(_)
            | TqlToken::Null => {
                let lit = self.parse_tql_literal()?;
                Ok(TqlExpr::Literal(lit))
            }
            other => Err(format!("Expected expression, got {:?}", other)),
        }
    }

    fn parse_comp_op(&mut self) -> Result<TqlCompOp, String> {
        match self.advance() {
            TqlToken::Eq => Ok(TqlCompOp::Eq),
            TqlToken::Ne => Ok(TqlCompOp::Ne),
            TqlToken::Gt => Ok(TqlCompOp::Gt),
            TqlToken::Gte => Ok(TqlCompOp::Gte),
            TqlToken::Lt => Ok(TqlCompOp::Lt),
            TqlToken::Lte => Ok(TqlCompOp::Lte),
            other => Err(format!("Expected comparison operator, got {:?}", other)),
        }
    }

    fn parse_tql_literal(&mut self) -> Result<TqlLiteral, String> {
        match self.advance() {
            TqlToken::IntLit(n) => Ok(TqlLiteral::Int(n)),
            TqlToken::FloatLit(f) => Ok(TqlLiteral::Float(f)),
            TqlToken::StringLit(s) => Ok(TqlLiteral::Str(s)),
            TqlToken::BoolLit(b) => Ok(TqlLiteral::Bool(b)),
            TqlToken::Null => Ok(TqlLiteral::Null),
            other => Err(format!("Expected literal, got {:?}", other)),
        }
    }

    fn parse_ident(&mut self) -> Result<String, String> {
        match self.advance() {
            TqlToken::Ident(s) => Ok(s),
            other => Err(format!("Expected identifier, got {:?}", other)),
        }
    }

    fn parse_positive_int(&mut self) -> Result<usize, String> {
        match self.advance() {
            TqlToken::IntLit(n) if n >= 0 => Ok(n as usize),
            other => Err(format!("Expected positive integer, got {:?}", other)),
        }
    }

    fn parse_field_name(&mut self) -> Result<String, String> {
        match self.advance() {
            TqlToken::Ident(s) => Ok(s),
            TqlToken::StringLit(s) => Ok(s),
            other => Err(format!("Expected field name, got {:?}", other)),
        }
    }

    /// 解析 JSON 值（用于文档过滤的值部分）
    fn parse_json_value(&mut self) -> Result<serde_json::Value, String> {
        match self.peek().clone() {
            TqlToken::IntLit(_) => {
                if let TqlToken::IntLit(n) = self.advance() {
                    Ok(serde_json::json!(n))
                } else {
                    Err("BUG: peek() was IntLit but advance() returned different token".into())
                }
            }
            TqlToken::FloatLit(_) => {
                if let TqlToken::FloatLit(f) = self.advance() {
                    Ok(serde_json::json!(f))
                } else {
                    Err("BUG: peek() was FloatLit but advance() returned different token".into())
                }
            }
            TqlToken::StringLit(_) => {
                if let TqlToken::StringLit(s) = self.advance() {
                    Ok(serde_json::json!(s))
                } else {
                    Err("BUG: peek() was StringLit but advance() returned different token".into())
                }
            }
            TqlToken::BoolLit(_) => {
                if let TqlToken::BoolLit(b) = self.advance() {
                    Ok(serde_json::json!(b))
                } else {
                    Err("BUG: peek() was BoolLit but advance() returned different token".into())
                }
            }
            TqlToken::Null => {
                self.advance();
                Ok(serde_json::Value::Null)
            }
            TqlToken::LBracket => {
                let arr = self.parse_json_array()?;
                Ok(serde_json::Value::Array(arr))
            }
            other => Err(format!("Expected JSON value, got {:?}", other)),
        }
    }

    fn parse_json_number(&mut self) -> Result<f64, String> {
        match self.advance() {
            TqlToken::IntLit(n) => Ok(n as f64),
            TqlToken::FloatLit(f) => Ok(f),
            other => Err(format!("Expected number, got {:?}", other)),
        }
    }

    fn parse_json_array(&mut self) -> Result<Vec<serde_json::Value>, String> {
        self.expect(&TqlToken::LBracket)?;
        let mut items = Vec::new();
        while !self.at(&TqlToken::RBracket) {
            items.push(self.parse_json_value()?);
            if self.at(&TqlToken::Comma) {
                self.advance();
            }
        }
        self.expect(&TqlToken::RBracket)?;
        Ok(items)
    }

    // ═══════════════════════════════════════════════════════════════
    //  DML 解析（写操作）
    // ═══════════════════════════════════════════════════════════════

    /// 顶层语句解析：判断是读查询还是写操作
    pub fn parse_statement(&mut self) -> Result<TqlStatement, String> {
        match self.peek() {
            // 写操作入口
            TqlToken::Create => {
                let mutation = self.parse_create()?;
                Ok(TqlStatement::Mutation(mutation))
            }
            TqlToken::Set | TqlToken::Delete | TqlToken::Detach => {
                // 无 MATCH 前缀的 SET/DELETE → 语法错误
                Err("SET/DELETE requires a preceding MATCH clause".to_string())
            }
            TqlToken::Match if self.is_dml_after_match() => {
                let mutation = self.parse_match_then_dml()?;
                Ok(TqlStatement::Mutation(mutation))
            }
            // 读查询
            _ => {
                let query = self.parse_query()?;
                Ok(TqlStatement::Query(query))
            }
        }
    }

    /// 探测 MATCH 之后是否跟着 DML（SET/DELETE/CREATE）
    fn is_dml_after_match(&self) -> bool {
        // 扫描后续 token，跳过 MATCH 模式和 WHERE，看是否碰到 SET/DELETE/CREATE（而非 RETURN）
        let mut depth = 0;
        for i in self.pos..self.tokens.len() {
            match &self.tokens[i] {
                TqlToken::LParen | TqlToken::LBracket | TqlToken::LBrace => depth += 1,
                TqlToken::RParen | TqlToken::RBracket | TqlToken::RBrace => depth -= 1,
                TqlToken::Return if depth == 0 => return false,
                TqlToken::Set | TqlToken::Delete | TqlToken::Detach if depth == 0 => return true,
                TqlToken::Create if depth == 0 => return true,
                TqlToken::Eof => return false,
                _ => {}
            }
        }
        false
    }

    /// 解析 CREATE 语句（无前置 MATCH）
    ///
    /// 语法：
    /// ```text
    /// CREATE (var {payload})
    /// CREATE (var {payload}), (var2 {payload2})
    /// ```
    fn parse_create(&mut self) -> Result<TqlMutation, String> {
        self.expect(&TqlToken::Create)?;
        let action = self.parse_create_action()?;
        Ok(TqlMutation {
            source: None,
            action: MutationAction::Create(action),
        })
    }

    /// 解析 CREATE 动作内容（节点和边列表）
    fn parse_create_action(&mut self) -> Result<CreateAction, String> {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        loop {
            // 解析 (var {payload}) 或 (var)-[:label]->(var2)
            self.expect(&TqlToken::LParen)?;

            let var = if let TqlToken::Ident(_) = self.peek() {
                if let TqlToken::Ident(name) = self.advance() {
                    Some(name)
                } else {
                    None
                }
            } else {
                None
            };

            let payload = if self.at(&TqlToken::LBrace) {
                self.parse_create_payload()?
            } else {
                serde_json::json!({})
            };

            self.expect(&TqlToken::RParen)?;

            nodes.push(CreateNode {
                var: var.clone(),
                payload,
            });

            // 检查是否有边: )-[:label]->(
            if self.at(&TqlToken::Dash) {
                let src_var = var.unwrap_or_else(|| format!("_auto_{}", nodes.len() - 1));
                // 回填变量名
                if nodes.last().unwrap().var.is_none() {
                    nodes.last_mut().unwrap().var = Some(src_var.clone());
                }

                // 解析边模式
                self.expect(&TqlToken::Dash)?;
                self.expect(&TqlToken::LBracket)?;
                let mut label = String::new();
                let mut weight = 1.0f32;
                if self.at(&TqlToken::Colon) {
                    self.advance();
                    label = self.parse_ident()?;
                }
                // 可选权重: {weight: 0.5}
                if self.at(&TqlToken::LBrace) {
                    self.advance();
                    // 简单解析 weight 字段
                    loop {
                        if self.at(&TqlToken::RBrace) {
                            break;
                        }
                        let key = self.parse_field_name()?;
                        self.expect(&TqlToken::Colon)?;
                        if key == "weight" {
                            weight = self.parse_json_number()? as f32;
                        } else {
                            let _ = self.parse_json_value()?;
                        }
                        if self.at(&TqlToken::Comma) {
                            self.advance();
                        }
                    }
                    self.expect(&TqlToken::RBrace)?;
                }
                self.expect(&TqlToken::RBracket)?;
                self.expect(&TqlToken::Arrow)?;

                // 目标节点
                self.expect(&TqlToken::LParen)?;
                let dst_var = if let TqlToken::Ident(_) = self.peek() {
                    if let TqlToken::Ident(name) = self.advance() {
                        name
                    } else {
                        format!("_auto_{}", nodes.len())
                    }
                } else {
                    format!("_auto_{}", nodes.len())
                };
                let dst_payload = if self.at(&TqlToken::LBrace) {
                    self.parse_create_payload()?
                } else {
                    serde_json::json!({})
                };
                self.expect(&TqlToken::RParen)?;

                // 如果目标节点有 payload，添加为新节点
                let needs_create_dst = !dst_payload.as_object().is_none_or(|m| m.is_empty());
                if needs_create_dst {
                    nodes.push(CreateNode {
                        var: Some(dst_var.clone()),
                        payload: dst_payload,
                    });
                }

                edges.push(CreateEdge {
                    src_var,
                    dst_var,
                    label,
                    weight,
                });
            }

            // 逗号分隔的多节点创建
            if self.at(&TqlToken::Comma) {
                self.advance();
                continue;
            }
            break;
        }

        Ok(CreateAction { nodes, edges })
    }

    /// 解析 CREATE 节点的 payload: {key: val, ...} → serde_json::Value
    fn parse_create_payload(&mut self) -> Result<serde_json::Value, String> {
        self.expect(&TqlToken::LBrace)?;
        let mut map = serde_json::Map::new();
        while !self.at(&TqlToken::RBrace) {
            let key = self.parse_field_name()?;
            self.expect(&TqlToken::Colon)?;
            let val = self.parse_json_value()?;
            map.insert(key, val);
            if self.at(&TqlToken::Comma) {
                self.advance();
            }
        }
        self.expect(&TqlToken::RBrace)?;
        Ok(serde_json::Value::Object(map))
    }

    /// 解析 MATCH ... WHERE ... SET/DELETE/CREATE
    fn parse_match_then_dml(&mut self) -> Result<TqlMutation, String> {
        self.expect(&TqlToken::Match)?;
        let pattern = self.parse_pattern()?;

        let predicate = if self.at(&TqlToken::Where) {
            self.advance();
            Some(self.parse_predicate()?)
        } else {
            None
        };

        let source = Some(MutationSource { pattern, predicate });

        let action = match self.peek() {
            TqlToken::Set => {
                self.advance();
                let assignments = self.parse_set_assignments()?;
                MutationAction::Set(assignments)
            }
            TqlToken::Delete => {
                self.advance();
                let vars = self.parse_delete_vars()?;
                MutationAction::Delete {
                    vars,
                    detach: false,
                }
            }
            TqlToken::Detach => {
                self.advance();
                self.expect(&TqlToken::Delete)?;
                let vars = self.parse_delete_vars()?;
                MutationAction::Delete { vars, detach: true }
            }
            TqlToken::Create => {
                self.advance();
                // MATCH ... CREATE (a)-[:r]->(b) — 创建边
                let create_action = self.parse_create_action()?;
                MutationAction::Create(create_action)
            }
            other => {
                return Err(format!(
                    "Expected SET, DELETE, DETACH DELETE, or CREATE after MATCH, got {:?}",
                    other
                ));
            }
        };

        Ok(TqlMutation { source, action })
    }

    /// 解析 SET 赋值列表: a.name = "Alice", a.age = 30
    fn parse_set_assignments(&mut self) -> Result<Vec<SetAssignment>, String> {
        let mut assignments = Vec::new();
        loop {
            let var = self.parse_ident()?;
            self.expect(&TqlToken::Dot)?;
            let field = self.parse_ident()?;
            // = 号
            if self.peek() == &TqlToken::Eq {
                self.advance(); // ==
            } else {
                return Err(format!(
                    "Expected '==' in SET assignment, got {:?}",
                    self.peek()
                ));
            }
            let value = self.parse_json_value()?;
            assignments.push(SetAssignment { var, field, value });
            if self.at(&TqlToken::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(assignments)
    }

    /// 解析 DELETE 变量列表: a, b
    fn parse_delete_vars(&mut self) -> Result<Vec<String>, String> {
        let mut vars = Vec::new();
        vars.push(self.parse_ident()?);
        while self.at(&TqlToken::Comma) {
            self.advance();
            vars.push(self.parse_ident()?);
        }
        Ok(vars)
    }
}

/// 便捷入口：TQL 字符串 → TqlQuery AST（仅读查询）
pub fn parse_tql(input: &str) -> Result<TqlQuery, String> {
    let mut lexer = super::tql_lexer::TqlLexer::new(input);
    let tokens = lexer.tokenize()?;
    let mut parser = TqlParser::new(tokens);
    parser.parse_query()
}

/// 便捷入口：TQL 字符串 → TqlStatement（读查询 或 写操作）
pub fn parse_tql_statement(input: &str) -> Result<TqlStatement, String> {
    let mut lexer = super::tql_lexer::TqlLexer::new(input);
    let tokens = lexer.tokenize()?;
    let mut parser = TqlParser::new(tokens);
    parser.parse_statement()
}

/// 带位置诊断的 TQL 解析（仅读查询）。错误中包含字节偏移，便于上层 CLI/IDE 高亮。
pub fn parse_tql_with_pos(input: &str) -> Result<TqlQuery, ParseErrorAt> {
    let mut lexer = super::tql_lexer::TqlLexer::new(input);
    let tokens_with_pos = lexer.tokenize_with_positions()?;
    let mut tokens = Vec::with_capacity(tokens_with_pos.len());
    let mut positions = Vec::with_capacity(tokens_with_pos.len());
    for pt in tokens_with_pos {
        tokens.push(pt.token);
        positions.push(pt.byte_start);
    }
    let mut parser = TqlParser::new_with_positions(tokens, positions);
    parser.parse_query().map_err(|msg| {
        let pos = parser.current_byte_pos().unwrap_or(input.len());
        ParseErrorAt::new(msg, pos)
    })
}

/// 带位置诊断的 TQL 解析（读 / 写均可）。错误中包含字节偏移。
pub fn parse_tql_statement_with_pos(input: &str) -> Result<TqlStatement, ParseErrorAt> {
    let mut lexer = super::tql_lexer::TqlLexer::new(input);
    let tokens_with_pos = lexer.tokenize_with_positions()?;
    let mut tokens = Vec::with_capacity(tokens_with_pos.len());
    let mut positions = Vec::with_capacity(tokens_with_pos.len());
    for pt in tokens_with_pos {
        tokens.push(pt.token);
        positions.push(pt.byte_start);
    }
    let mut parser = TqlParser::new_with_positions(tokens, positions);
    parser.parse_statement().map_err(|msg| {
        let pos = parser.current_byte_pos().unwrap_or(input.len());
        ParseErrorAt::new(msg, pos)
    })
}
