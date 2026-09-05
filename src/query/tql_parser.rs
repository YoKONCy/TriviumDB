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
        let analyze = if explain && self.at(&TqlToken::Analyze) {
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
            TqlToken::Ident(name) if name.eq_ignore_ascii_case("text") => {
                self.parse_text_entry()?
            }
            other => {
                return Err(format!(
                    "Expected MATCH, OPTIONAL MATCH, FIND, SEARCH, or TEXT, got {:?}",
                    other
                ));
            }
        };

        // 2. WITH 管线阶段
        let pipeline = self.parse_pipeline_stages()?;

        // 3. WHERE (可选)
        let predicate = if self.at(&TqlToken::Where) {
            self.advance();
            Some(self.parse_predicate()?)
        } else {
            None
        };

        let rank = if self.at(&TqlToken::Rank) {
            Some(self.parse_rank_clause()?)
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

        let query = TqlQuery {
            explain,
            analyze,
            entry,
            pipeline,
            predicate,
            rank,
            returns,
            order_by,
            limit,
            offset,
        };
        validate_pipeline_scope(&query)?;
        Ok(query)
    }

    fn parse_text_entry(&mut self) -> Result<QueryEntry, String> {
        self.expect_ident_keyword("text")?;
        let kind = match self.parse_ident()?.to_ascii_lowercase().as_str() {
            "bm25" => TextSearchKind::Bm25,
            "ac" => TextSearchKind::Ac,
            "hybrid" => TextSearchKind::Hybrid,
            other => {
                return Err(format!(
                    "未知文本召回类型 {other} (Unknown text search kind)"
                ));
            }
        };
        let query = match self.advance() {
            TqlToken::StringLit(value) => value,
            other => return Err(format!("TEXT 查询需要字符串，收到 {other:?}")),
        };
        self.expect(&TqlToken::Top)?;
        let top_k = self.parse_positive_int()?;
        let mut k1 = 1.2f32;
        let mut b = 0.75f32;
        let mut ac_weight = 1.0f32;
        while let TqlToken::Ident(name) = self.peek().clone() {
            let target = match name.to_ascii_lowercase().as_str() {
                "k1" => &mut k1,
                "b" => &mut b,
                "ac_weight" => &mut ac_weight,
                _ => break,
            };
            self.advance();
            *target = self.parse_finite_f32()?;
        }
        Ok(QueryEntry::Text {
            clause: TextSearchClause {
                kind,
                query,
                top_k,
                k1,
                b,
                ac_weight,
            },
        })
    }

    fn parse_finite_f32(&mut self) -> Result<f32, String> {
        let value = match self.advance() {
            TqlToken::FloatLit(value) => value,
            TqlToken::IntLit(value) => value as f64,
            other => return Err(format!("Expected finite number, got {other:?}")),
        };
        if !value.is_finite() || value < f32::MIN as f64 || value > f32::MAX as f64 {
            return Err("参数必须是有限 f32 数值 (Parameter must be finite f32)".into());
        }
        Ok(value as f32)
    }

    fn parse_rank_clause(&mut self) -> Result<RankClause, String> {
        self.expect(&TqlToken::Rank)?;
        let var = self.parse_ident()?;
        self.expect(&TqlToken::By)?;
        self.expect(&TqlToken::Vector)?;
        self.expect(&TqlToken::LBracket)?;
        let mut vector = Vec::new();
        while !self.at(&TqlToken::RBracket) {
            let value = match self.advance() {
                TqlToken::FloatLit(value) => value,
                TqlToken::IntLit(value) => value as f64,
                other => return Err(format!("Expected number in RANK vector, got {other:?}")),
            };
            vector.push(value);
            if self.at(&TqlToken::Comma) {
                self.advance();
            }
        }
        self.expect(&TqlToken::RBracket)?;
        self.expect(&TqlToken::Top)?;
        let top_k = self.parse_positive_int()?;
        Ok(RankClause { var, vector, top_k })
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

        // 向量元素支持数字字面量或 Prepared 标量参数。
        self.expect(&TqlToken::LBracket)?;
        let mut vector = Vec::new();
        let mut vector_parameters = Vec::new();
        loop {
            if self.at(&TqlToken::RBracket) {
                break;
            }
            let index = vector.len();
            let value = match self.advance() {
                TqlToken::FloatLit(value) => value,
                TqlToken::IntLit(value) => value as f64,
                TqlToken::DollarOp(name) => {
                    vector_parameters.push((index, name.trim_start_matches('$').to_owned()));
                    0.0
                }
                other => {
                    return Err(format!(
                        "Expected number or parameter in vector, got {other:?}"
                    ));
                }
            };
            vector.push(value);
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
            vector_parameters,
            top_k,
            expand,
        })
    }

    fn parse_pipeline_stages(&mut self) -> Result<Vec<PipelineStage>, String> {
        let mut stages = Vec::new();
        while self.at(&TqlToken::As) || self.at(&TqlToken::With) {
            // Source AS name 是首个作用域绑定，不单独形成执行阶段。
            if self.at(&TqlToken::As) {
                self.advance();
                let alias = self.parse_ident()?;
                stages.push(PipelineStage::With(WithStage {
                    items: vec![WithItem {
                        expr: TqlExpr::Variable("_".into()),
                        alias,
                    }],
                }));
                continue;
            }
            self.expect(&TqlToken::With)?;
            let mut items = Vec::new();
            loop {
                let expr = self.parse_expr()?;
                let alias = if self.at(&TqlToken::As) {
                    self.advance();
                    self.parse_ident()?
                } else if let TqlExpr::Variable(var) = &expr {
                    var.clone()
                } else {
                    return Err("WITH 标量表达式必须使用 AS 别名 (WITH scalar expression requires AS alias)".into());
                };
                items.push(WithItem { expr, alias });
                if self.at(&TqlToken::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            stages.push(PipelineStage::With(WithStage { items }));

            if self.at(&TqlToken::Expand) {
                self.advance();
                let input = self.parse_ident()?;
                let expand = self.parse_expand_clause()?;
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                stages.push(PipelineStage::Expand(PipelineExpandStage {
                    input,
                    output,
                    expand,
                }));
            }
            if let TqlToken::Ident(name) = self.peek().clone()
                && matches!(
                    name.to_ascii_lowercase().as_str(),
                    "pagerank"
                        | "wcc"
                        | "degree"
                        | "label_propagation"
                        | "leiden"
                        | "sa_ppr"
                        | "scc"
                        | "k_core"
                        | "articulation_points"
                        | "triangle_count"
                        | "hits"
                        | "harmonic_centrality"
                )
            {
                self.advance();
                let input = self.parse_ident()?;
                let subset = if matches!(self.peek(), TqlToken::Ident(value) if value.eq_ignore_ascii_case("mode"))
                {
                    self.advance();
                    let mode = match self.advance() {
                        TqlToken::Expand => "expand".to_owned(),
                        TqlToken::Ident(mode) => mode.to_ascii_lowercase(),
                        other => return Err(format!("Expected graph subset mode, got {other:?}")),
                    };
                    match mode.as_str() {
                        "induced" => GraphSubsetSpec::Induced,
                        mode @ ("expand" | "boundary") => {
                            self.expect_ident_keyword("hops")?;
                            let hops = self.parse_positive_int()?;
                            let direction = if matches!(self.peek(), TqlToken::Outgoing) {
                                self.advance();
                                EdgeDirection::Forward
                            } else if matches!(self.peek(), TqlToken::Incoming) {
                                self.advance();
                                EdgeDirection::Backward
                            } else if matches!(self.peek(), TqlToken::Both) {
                                self.advance();
                                EdgeDirection::Both
                            } else {
                                EdgeDirection::Forward
                            };
                            let labels = if matches!(self.peek(), TqlToken::Ident(value) if value.eq_ignore_ascii_case("labels"))
                            {
                                self.advance();
                                Some(self.parse_ident_list()?)
                            } else {
                                None
                            };
                            if mode == "expand" {
                                GraphSubsetSpec::Expand {
                                    hops,
                                    labels,
                                    direction,
                                }
                            } else {
                                GraphSubsetSpec::Boundary {
                                    hops,
                                    labels,
                                    direction,
                                }
                            }
                        }
                        mode => {
                            return Err(format!(
                                "未知图子集模式 {mode} (Unknown graph subset mode)"
                            ));
                        }
                    }
                } else {
                    GraphSubsetSpec::Induced
                };
                let label_filter = if matches!(self.peek(), TqlToken::Ident(value) if value.eq_ignore_ascii_case("label"))
                {
                    self.advance();
                    Some(self.parse_ident()?)
                } else {
                    None
                };
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                let algorithm = match name.to_ascii_lowercase().as_str() {
                    "pagerank" => GraphAlgorithmKind::PageRank,
                    "wcc" => GraphAlgorithmKind::Wcc,
                    "degree" => GraphAlgorithmKind::Degree,
                    "label_propagation" => GraphAlgorithmKind::LabelPropagation,
                    "leiden" => GraphAlgorithmKind::Leiden,
                    "sa_ppr" => GraphAlgorithmKind::SaPpr,
                    "scc" => GraphAlgorithmKind::Scc,
                    "k_core" => GraphAlgorithmKind::KCore,
                    "articulation_points" => GraphAlgorithmKind::ArticulationPoints,
                    "triangle_count" => GraphAlgorithmKind::TriangleCount,
                    "hits" => GraphAlgorithmKind::Hits,
                    "harmonic_centrality" => GraphAlgorithmKind::HarmonicCentrality,
                    _ => return Err(format!("未知图算法: {name}")),
                };
                stages.push(PipelineStage::GraphAlgorithm(GraphAlgorithmStage {
                    input,
                    output,
                    algorithm,
                    subset,
                    label_filter,
                }));
            }
            if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("all_paths"))
            {
                self.advance();
                let input = self.parse_ident()?;
                self.expect_ident_keyword("to")?;
                let targets = self.parse_u64_list()?;
                self.expect_ident_keyword("depth")?;
                let max_depth = self.parse_positive_int()?;
                self.expect_ident_keyword("paths")?;
                let max_paths = self.parse_positive_int()?;
                self.expect_ident_keyword("aggregate")?;
                let aggregation = match self.parse_ident()?.to_ascii_lowercase().as_str() {
                    "max_product" => PathAggregation::MaxProduct,
                    "sum_product" => PathAggregation::SumProduct,
                    "average_weight" => PathAggregation::AverageWeight,
                    other => {
                        return Err(format!(
                            "未知路径聚合方式 {other} (Unknown path aggregation {other})"
                        ));
                    }
                };
                let label_sequence = if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("labels"))
                {
                    self.advance();
                    Some(self.parse_ident_list()?)
                } else {
                    None
                };
                let forbidden_nodes = if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("avoid"))
                {
                    self.advance();
                    self.parse_u64_list()?
                } else {
                    Vec::new()
                };
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                stages.push(PipelineStage::AllPaths(AllPathsStage {
                    input,
                    output,
                    targets,
                    max_depth,
                    max_paths,
                    aggregation,
                    label_sequence,
                    forbidden_nodes,
                }));
            }
            if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("shortest_paths"))
            {
                self.advance();
                let input = self.parse_ident()?;
                self.expect_ident_keyword("to")?;
                let targets = self.parse_u64_list()?;
                let label = if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("label"))
                {
                    self.advance();
                    Some(self.parse_ident()?)
                } else {
                    None
                };
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                stages.push(PipelineStage::ShortestPaths(ShortestPathsStage {
                    input,
                    output,
                    targets,
                    label,
                }));
            }
            if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("weighted_paths"))
            {
                self.advance();
                let input = self.parse_ident()?;
                self.expect_ident_keyword("to")?;
                let targets = self.parse_u64_list()?;
                let label = if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("label"))
                {
                    self.advance();
                    Some(self.parse_ident()?)
                } else {
                    None
                };
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                stages.push(PipelineStage::WeightedPaths(WeightedPathsStage {
                    input,
                    output,
                    targets,
                    label,
                }));
            }
            if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("yen_paths"))
            {
                self.advance();
                let input = self.parse_ident()?;
                self.expect_ident_keyword("to")?;
                let targets = self.parse_u64_list()?;
                self.expect_ident_keyword("k")?;
                let k = self.parse_positive_int()?;
                let label = if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("label"))
                {
                    self.advance();
                    Some(self.parse_ident()?)
                } else {
                    None
                };
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                stages.push(PipelineStage::YenPaths(YenPathsStage {
                    input,
                    output,
                    targets,
                    k,
                    label,
                }));
            }
            if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("node_similarity"))
            {
                self.advance();
                let input = self.parse_ident()?;
                self.expect(&TqlToken::Top)?;
                let top_k = self.parse_positive_int()?;
                self.expect_ident_keyword("cutoff")?;
                let cutoff = self.parse_json_number()?;
                let label = if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("label"))
                {
                    self.advance();
                    Some(self.parse_ident()?)
                } else {
                    None
                };
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                stages.push(PipelineStage::NodeSimilarity(NodeSimilarityStage {
                    input,
                    output,
                    top_k,
                    cutoff,
                    label,
                }));
            }
            if let TqlToken::Ident(name) = self.peek().clone()
                && matches!(
                    name.to_ascii_lowercase().as_str(),
                    "union" | "intersect" | "except"
                )
            {
                self.advance();
                let input = self.parse_ident()?;
                self.expect_ident_keyword("ids")?;
                let other_ids = self.parse_u64_list()?;
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                let operation = match name.to_ascii_lowercase().as_str() {
                    "union" => TqlSetOperation::Union,
                    "intersect" => TqlSetOperation::Intersect,
                    "except" => TqlSetOperation::Except,
                    _ => return Err(format!("未知集合运算: {name}")),
                };
                stages.push(PipelineStage::SetCombine(SetCombineStage {
                    input,
                    other_ids,
                    output,
                    operation,
                }));
            }
            if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("iterate"))
            {
                self.advance();
                let input = self.parse_ident()?;
                self.expect(&TqlToken::Expand)?;
                let expand = self.parse_expand_clause()?;
                self.expect_ident_keyword("times")?;
                let max_iterations = self.parse_positive_int()?;
                let stop_on_fixed_point = if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("fixed"))
                {
                    self.advance();
                    true
                } else {
                    false
                };
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                stages.push(PipelineStage::Iterate(IterateStage {
                    input,
                    output,
                    expand,
                    max_iterations,
                    stop_on_fixed_point,
                }));
            }
            if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("diversify"))
            {
                self.advance();
                let input = self.parse_ident()?;
                self.expect(&TqlToken::Top)?;
                let top_k = self.parse_positive_int()?;
                let quality_weight = if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("quality_weight"))
                {
                    self.advance();
                    self.parse_finite_f32()?
                } else {
                    1.0
                };
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                stages.push(PipelineStage::Diversify(DiversifyStage {
                    input,
                    output,
                    top_k,
                    quality_weight,
                }));
            }
            if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("residual"))
            {
                self.advance();
                let input = self.parse_ident()?;
                self.expect(&TqlToken::By)?;
                self.expect(&TqlToken::Vector)?;
                self.expect(&TqlToken::LBracket)?;
                let mut vector = Vec::new();
                while !self.at(&TqlToken::RBracket) {
                    vector.push(self.parse_finite_f32()? as f64);
                    if self.at(&TqlToken::Comma) {
                        self.advance();
                    }
                }
                self.expect(&TqlToken::RBracket)?;
                self.expect(&TqlToken::Top)?;
                let top_k = self.parse_positive_int()?;
                self.expect_ident_keyword("lambda")?;
                let lambda = self.parse_finite_f32()?;
                self.expect_ident_keyword("threshold")?;
                let threshold = self.parse_finite_f32()?;
                self.expect_ident_keyword("iterations")?;
                let iterations = self.parse_positive_int()?;
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                stages.push(PipelineStage::Residual(ResidualStage {
                    input,
                    output,
                    vector,
                    top_k,
                    lambda,
                    threshold,
                    iterations,
                }));
            }
            if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("topics")) {
                self.advance();
                let input = self.parse_ident()?;
                self.expect_ident_keyword("k")?;
                let topics = self.parse_positive_int()?;
                self.expect_ident_keyword("iterations")?;
                let iterations = self.parse_positive_int()?;
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                stages.push(PipelineStage::Topics(TopicsStage {
                    input,
                    output,
                    topics,
                    iterations,
                }));
            }
            if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("sa_ppr_config"))
            {
                self.advance();
                let input = self.parse_ident()?;
                self.expect_ident_keyword("depth")?;
                let max_depth = self.parse_positive_int()?;
                self.expect_ident_keyword("alpha")?;
                let restart_alpha = self.parse_finite_f32()?;
                self.expect_ident_keyword("max_edges")?;
                let max_edges_per_node = self.parse_positive_int()?;
                self.expect_ident_keyword("min_weight")?;
                let min_edge_weight = self.parse_finite_f32()?;
                let labels = if matches!(self.peek(), TqlToken::Ident(name) if name.eq_ignore_ascii_case("labels"))
                {
                    self.advance();
                    Some(self.parse_ident_list()?)
                } else {
                    None
                };
                self.expect(&TqlToken::As)?;
                let output = self.parse_ident()?;
                stages.push(PipelineStage::SaPpr(SaPprStage {
                    input,
                    output,
                    max_depth,
                    restart_alpha,
                    max_edges_per_node,
                    min_edge_weight,
                    labels,
                }));
            }
            if self.at(&TqlToken::Where) {
                self.advance();
                stages.push(PipelineStage::Filter(self.parse_predicate()?));
            }
            if self.at(&TqlToken::Rank) {
                stages.push(PipelineStage::Rank(self.parse_rank_clause()?));
            }
        }
        Ok(stages)
    }

    fn expect_ident_keyword(&mut self, expected: &str) -> Result<(), String> {
        match self.advance() {
            TqlToken::Ident(value) if value.eq_ignore_ascii_case(expected) => Ok(()),
            other => Err(format!("Expected {expected}, got {other:?}")),
        }
    }

    fn parse_u64_list(&mut self) -> Result<Vec<u64>, String> {
        self.expect(&TqlToken::LBracket)?;
        let mut values = Vec::new();
        while !self.at(&TqlToken::RBracket) {
            match self.advance() {
                TqlToken::IntLit(value) if value > 0 => values.push(value as u64),
                other => {
                    return Err(format!(
                        "节点 ID 必须为正整数 (Node ID must be positive), got {other:?}"
                    ));
                }
            }
            if self.at(&TqlToken::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(&TqlToken::RBracket)?;
        if values.is_empty() {
            return Err("节点 ID 列表不能为空 (Node ID list cannot be empty)".into());
        }
        Ok(values)
    }

    fn parse_ident_list(&mut self) -> Result<Vec<String>, String> {
        self.expect(&TqlToken::LBracket)?;
        let mut values = Vec::new();
        while !self.at(&TqlToken::RBracket) {
            values.push(self.parse_ident()?);
            if self.at(&TqlToken::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(&TqlToken::RBracket)?;
        if values.is_empty() {
            return Err("标签列表不能为空 (Label list cannot be empty)".into());
        }
        Ok(values)
    }

    /// EXPAND [OUTGOING|INCOMING|BOTH] [:label*min..max]
    fn parse_expand_clause(&mut self) -> Result<ExpandClause, String> {
        let direction = match self.peek() {
            TqlToken::Outgoing => {
                self.advance();
                EdgeDirection::Forward
            }
            TqlToken::Incoming => {
                self.advance();
                EdgeDirection::Backward
            }
            TqlToken::Both => {
                self.advance();
                EdgeDirection::Both
            }
            _ => EdgeDirection::Forward,
        };
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
        if min_depth > max_depth {
            return Err(format!(
                "EXPAND min depth ({min_depth}) > max depth ({max_depth})"
            ));
        }

        self.expect(&TqlToken::RBracket)?;

        Ok(ExpandClause {
            labels,
            min_depth,
            max_depth,
            direction,
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

                TqlToken::Ident(_) | TqlToken::StringLit(_) | TqlToken::Rank => {
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
            "$gt" | "$gte" | "$lt" | "$lte" | "$before" | "$beforeEq" | "$after" | "$afterEq" => {
                let value = self.parse_json_value()?;
                let json = serde_json::json!({field: {op: value}});
                Filter::from_json(&json)
            }
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

        // 表达式比较或 var MATCHES 文档过滤
        if matches!(
            self.peek(),
            TqlToken::Ident(_) | TqlToken::Rank | TqlToken::DollarOp(_)
        ) {
            let checkpoint = self.pos;
            let left = self.parse_expr()?;
            if let TqlExpr::Variable(ident) = &left
                && self.at(&TqlToken::Matches)
            {
                self.advance();
                let filter = self.parse_doc_filter()?;
                return Ok(Predicate::DocFilter {
                    var: Some(ident.clone()),
                    filter,
                });
            }
            if matches!(
                self.peek(),
                TqlToken::Eq
                    | TqlToken::Ne
                    | TqlToken::Gt
                    | TqlToken::Gte
                    | TqlToken::Lt
                    | TqlToken::Lte
            ) {
                let op = self.parse_comp_op()?;
                let right = self.parse_expr()?;
                return Ok(Predicate::Compare { left, op, right });
            }
            self.pos = checkpoint;
            let ident = self.parse_ident()?;
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
            let inner = if self.at(&TqlToken::Star) {
                self.advance();
                ReturnExprKind::Var("*".to_owned())
            } else {
                self.parse_return_expr_kind()?
            };
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
        let expr = self.parse_expr()?;
        match expr {
            TqlExpr::Variable(var) => Ok(ReturnExprKind::Var(var)),
            TqlExpr::Property { var, field } => Ok(ReturnExprKind::Property(var, field)),
            scalar => Ok(ReturnExprKind::Scalar(scalar)),
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
        let expr = self.parse_additive_expr()?;
        if !self.at(&TqlToken::Is) {
            return Ok(expr);
        }
        self.advance();
        let negated = if self.at(&TqlToken::Not) {
            self.advance();
            true
        } else {
            false
        };
        self.expect(&TqlToken::Null)?;
        Ok(TqlExpr::IsNull {
            expr: Box::new(expr),
            negated,
        })
    }

    fn parse_additive_expr(&mut self) -> Result<TqlExpr, String> {
        let mut left = self.parse_multiplicative_expr()?;
        while self.at(&TqlToken::Plus) || self.at(&TqlToken::Dash) {
            let op = if self.at(&TqlToken::Plus) {
                TqlBinaryOp::Add
            } else {
                TqlBinaryOp::Subtract
            };
            self.advance();
            let right = self.parse_multiplicative_expr()?;
            left = TqlExpr::Binary {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_multiplicative_expr(&mut self) -> Result<TqlExpr, String> {
        let mut left = self.parse_primary_expr()?;
        while self.at(&TqlToken::Star) || self.at(&TqlToken::Slash) {
            let op = if self.at(&TqlToken::Star) {
                TqlBinaryOp::Multiply
            } else {
                TqlBinaryOp::Divide
            };
            self.advance();
            let right = self.parse_primary_expr()?;
            left = TqlExpr::Binary {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_primary_expr(&mut self) -> Result<TqlExpr, String> {
        match self.peek().clone() {
            TqlToken::Ident(name)
                if matches!(
                    name.to_ascii_lowercase().as_str(),
                    "similarity"
                        | "graph_score"
                        | "core_number"
                        | "triangle_count"
                        | "clustering_coefficient"
                        | "authority_score"
                        | "hub_score"
                        | "harmonic_centrality"
                        | "weighted_distance"
                        | "node_similarity"
                        | "path_rank"
                        | "pair_left"
                        | "pair_right"
                        | "text_score"
                        | "diversity_score"
                        | "residual_score"
                        | "topic"
                        | "topic_score"
                        | "depth"
                        | "path_strength"
                        | "path_count"
                        | "community"
                        | "path"
                        | "path_length"
                        | "id"
                ) =>
            {
                self.advance();
                self.expect(&TqlToken::LParen)?;
                let var = self.parse_ident()?;
                self.expect(&TqlToken::RParen)?;
                match name.to_ascii_lowercase().as_str() {
                    "similarity" => Ok(TqlExpr::Similarity { var }),
                    "graph_score" => Ok(TqlExpr::GraphScore { var }),
                    "core_number"
                    | "triangle_count"
                    | "clustering_coefficient"
                    | "authority_score"
                    | "hub_score"
                    | "harmonic_centrality"
                    | "weighted_distance"
                    | "node_similarity" => Ok(TqlExpr::GraphMetric {
                        var,
                        metric: name.to_ascii_lowercase(),
                    }),
                    "path_rank" => Ok(TqlExpr::PathRank { var }),
                    "pair_left" => Ok(TqlExpr::PairLeft { var }),
                    "pair_right" => Ok(TqlExpr::PairRight { var }),
                    "text_score" => Ok(TqlExpr::TextScore { var }),
                    "diversity_score" => Ok(TqlExpr::DiversityScore { var }),
                    "residual_score" => Ok(TqlExpr::ResidualScore { var }),
                    "topic" => Ok(TqlExpr::Topic { var }),
                    "topic_score" => Ok(TqlExpr::TopicScore { var }),
                    "depth" => Ok(TqlExpr::Depth { var }),
                    "path_strength" => Ok(TqlExpr::PathStrength { var }),
                    "path_count" => Ok(TqlExpr::PathCount { var }),
                    "community" => Ok(TqlExpr::Community { var }),
                    "path" => Ok(TqlExpr::Path { var }),
                    "path_length" => Ok(TqlExpr::PathLength { var }),
                    "id" => Ok(TqlExpr::Property {
                        var,
                        field: "id".to_owned(),
                    }),
                    _ => Err(format!("未知标量函数: {name}")),
                }
            }
            TqlToken::DollarOp(name) => {
                self.advance();
                Ok(TqlExpr::Parameter(name.trim_start_matches('$').to_owned()))
            }
            TqlToken::Ident(name) if name.eq_ignore_ascii_case("coalesce") => {
                self.advance();
                self.expect(&TqlToken::LParen)?;
                let mut values = vec![self.parse_expr()?];
                while self.at(&TqlToken::Comma) {
                    self.advance();
                    values.push(self.parse_expr()?);
                }
                self.expect(&TqlToken::RParen)?;
                Ok(TqlExpr::Coalesce(values))
            }
            TqlToken::Ident(_) | TqlToken::Rank => {
                let ident = self.parse_ident()?;
                if self.at(&TqlToken::Dot) {
                    self.advance();
                    let field = self.parse_ident()?;
                    Ok(TqlExpr::Property { var: ident, field })
                } else {
                    Ok(TqlExpr::Variable(ident))
                }
            }
            TqlToken::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&TqlToken::RParen)?;
                Ok(expr)
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
            TqlToken::Rank => Ok("rank".into()),
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
            TqlToken::Rank => Ok("rank".into()),
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
                self.depth += 1;
                if self.depth > 128 {
                    self.depth -= 1;
                    return Err("Parser recursion depth exceeded (JSON nesting too deep)".into());
                }
                let result = self.parse_json_array().map(serde_json::Value::Array);
                self.depth -= 1;
                result
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
            self.expect(&TqlToken::LParen)?;
            let mut current_var = if let TqlToken::Ident(name) = self.peek().clone() {
                self.advance();
                name
            } else {
                format!("_auto_{}", nodes.len())
            };
            let payload = if self.at(&TqlToken::LBrace) {
                self.parse_create_payload()?
            } else {
                serde_json::json!({})
            };
            self.expect(&TqlToken::RParen)?;
            merge_create_node(&mut nodes, current_var.clone(), payload)?;

            while self.at(&TqlToken::Dash) {
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
                merge_create_node(&mut nodes, dst_var.clone(), dst_payload)?;
                edges.push(CreateEdge {
                    src_var: current_var,
                    dst_var: dst_var.clone(),
                    label,
                    weight,
                });
                current_var = dst_var;
            }

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
                if self.at(&TqlToken::Vector) {
                    self.advance();
                    self.expect(&TqlToken::LParen)?;
                    let var = self.parse_ident()?;
                    self.expect(&TqlToken::RParen)?;
                    self.expect(&TqlToken::Eq)?;
                    MutationAction::SetVector {
                        var,
                        vector: self.parse_numeric_vector()?,
                    }
                } else {
                    let assignments = self.parse_set_assignments()?;
                    MutationAction::Set(assignments)
                }
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

    fn parse_numeric_vector(&mut self) -> Result<Vec<f64>, String> {
        self.expect(&TqlToken::LBracket)?;
        let mut vector = Vec::new();
        while !self.at(&TqlToken::RBracket) {
            let value = match self.advance() {
                TqlToken::FloatLit(value) => value,
                TqlToken::IntLit(value) => value as f64,
                other => return Err(format!("Expected number in vector, got {other:?}")),
            };
            if !value.is_finite() {
                return Err("Vector values must be finite".into());
            }
            vector.push(value);
            if self.at(&TqlToken::Comma) {
                self.advance();
            } else if !self.at(&TqlToken::RBracket) {
                return Err(format!("Expected comma in vector, got {:?}", self.peek()));
            }
        }
        self.expect(&TqlToken::RBracket)?;
        if vector.is_empty() {
            return Err("Vector must not be empty".into());
        }
        Ok(vector)
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

fn merge_create_node(
    nodes: &mut Vec<CreateNode>,
    var: String,
    payload: serde_json::Value,
) -> Result<(), String> {
    if let Some(existing) = nodes
        .iter_mut()
        .find(|node| node.var.as_deref() == Some(var.as_str()))
    {
        let has_existing_payload = existing
            .payload
            .as_object()
            .is_some_and(|object| !object.is_empty());
        let has_new_payload = payload.as_object().is_some_and(|object| !object.is_empty());
        if has_existing_payload && has_new_payload && existing.payload != payload {
            return Err(format!("CREATE 变量 {var} 定义了冲突的 payload"));
        }
        if !has_existing_payload && has_new_payload {
            existing.payload = payload;
        }
        return Ok(());
    }

    nodes.push(CreateNode {
        var: Some(var),
        payload,
    });
    Ok(())
}

fn collect_return_kind_vars(kind: &ReturnExprKind, output: &mut Vec<String>) {
    match kind {
        ReturnExprKind::Var(var) if var != "*" => output.push(var.clone()),
        ReturnExprKind::Property(var, _) => output.push(var.clone()),
        ReturnExprKind::Scalar(expr) => collect_expr_vars(expr, output),
        ReturnExprKind::Aggregate(_, inner) => collect_return_kind_vars(inner, output),
        ReturnExprKind::Var(_) => {}
    }
}

fn collect_expr_vars(expr: &TqlExpr, output: &mut Vec<String>) {
    match expr {
        TqlExpr::Variable(var)
        | TqlExpr::Property { var, .. }
        | TqlExpr::Similarity { var }
        | TqlExpr::GraphScore { var }
        | TqlExpr::GraphMetric { var, .. }
        | TqlExpr::TextScore { var }
        | TqlExpr::DiversityScore { var }
        | TqlExpr::ResidualScore { var }
        | TqlExpr::Topic { var }
        | TqlExpr::TopicScore { var }
        | TqlExpr::Depth { var }
        | TqlExpr::PathStrength { var }
        | TqlExpr::PathCount { var }
        | TqlExpr::Community { var }
        | TqlExpr::Path { var }
        | TqlExpr::PathLength { var }
        | TqlExpr::PathRank { var }
        | TqlExpr::PairLeft { var }
        | TqlExpr::PairRight { var } => output.push(var.clone()),
        TqlExpr::Binary { left, right, .. } => {
            collect_expr_vars(left, output);
            collect_expr_vars(right, output);
        }
        TqlExpr::Coalesce(values) => {
            for value in values {
                collect_expr_vars(value, output);
            }
        }
        TqlExpr::IsNull { expr, .. } => collect_expr_vars(expr, output),
        TqlExpr::Parameter(_) | TqlExpr::Literal(_) => {}
    }
}

fn expr_var(expr: &TqlExpr) -> Option<&str> {
    match expr {
        TqlExpr::Variable(var)
        | TqlExpr::Property { var, .. }
        | TqlExpr::Similarity { var }
        | TqlExpr::GraphScore { var }
        | TqlExpr::GraphMetric { var, .. }
        | TqlExpr::TextScore { var }
        | TqlExpr::DiversityScore { var }
        | TqlExpr::ResidualScore { var }
        | TqlExpr::Topic { var }
        | TqlExpr::TopicScore { var }
        | TqlExpr::Depth { var }
        | TqlExpr::PathStrength { var }
        | TqlExpr::PathCount { var }
        | TqlExpr::Community { var }
        | TqlExpr::Path { var }
        | TqlExpr::PathLength { var }
        | TqlExpr::PathRank { var }
        | TqlExpr::PairLeft { var }
        | TqlExpr::PairRight { var } => Some(var),
        TqlExpr::Binary { left, right, .. } => expr_var(left).or_else(|| expr_var(right)),
        TqlExpr::Coalesce(values) => values.iter().find_map(expr_var),
        TqlExpr::IsNull { expr, .. } => expr_var(expr),
        TqlExpr::Parameter(_) | TqlExpr::Literal(_) => None,
    }
}

fn predicate_vars(predicate: &Predicate, output: &mut Vec<String>) {
    match predicate {
        Predicate::Compare { left, right, .. } => {
            collect_expr_vars(left, output);
            collect_expr_vars(right, output);
        }
        Predicate::DocFilter { var: Some(var), .. } => output.push(var.clone()),
        Predicate::DocFilter { var: None, .. } => {}
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            predicate_vars(left, output);
            predicate_vars(right, output);
        }
        Predicate::Not(inner) => predicate_vars(inner, output),
    }
}

fn validate_pipeline_scope(query: &TqlQuery) -> Result<(), String> {
    if query.pipeline.is_empty() {
        return Ok(());
    }
    let mut scope = std::collections::BTreeSet::from(["_".to_owned()]);
    let mut scalar_aliases = std::collections::BTreeSet::new();
    for stage in &query.pipeline {
        match stage {
            PipelineStage::With(with) => {
                let mut next = std::collections::BTreeSet::new();
                let mut next_scalars = std::collections::BTreeSet::new();
                for item in &with.items {
                    if let Some(var) = expr_var(&item.expr)
                        && !scope.contains(var)
                        && !scalar_aliases.contains(var)
                    {
                        return Err(format!(
                            "WITH 引用了未定义变量 {var} (WITH references undefined variable {var})"
                        ));
                    }
                    let is_node = matches!(item.expr, TqlExpr::Variable(_));
                    let inserted = if is_node {
                        next.insert(item.alias.clone())
                    } else {
                        next_scalars.insert(item.alias.clone())
                    };
                    if !inserted || next.contains(&item.alias) && next_scalars.contains(&item.alias)
                    {
                        return Err(format!(
                            "WITH 重复定义别名 {} (Duplicate WITH alias {})",
                            item.alias, item.alias
                        ));
                    }
                }
                scalar_aliases = with
                    .items
                    .iter()
                    .filter(|item| !matches!(item.expr, TqlExpr::Variable(_)))
                    .map(|item| item.alias.clone())
                    .collect();
                scope = next;
            }
            PipelineStage::Expand(expand) => {
                if !scope.contains(&expand.input) {
                    return Err(format!(
                        "EXPAND 引用了未定义变量 {} (EXPAND references undefined variable {})",
                        expand.input, expand.input
                    ));
                }
                scope.insert(expand.output.clone());
            }
            PipelineStage::GraphAlgorithm(graph) => {
                if !scope.contains(&graph.input) {
                    return Err(format!(
                        "图算法引用了未定义变量 {} (Graph algorithm references undefined variable {})",
                        graph.input, graph.input
                    ));
                }
                scope.insert(graph.output.clone());
            }
            PipelineStage::AllPaths(paths) => {
                if !scope.contains(&paths.input) {
                    return Err(format!(
                        "ALL_PATHS 引用了未定义变量 {} (ALL_PATHS references undefined variable {})",
                        paths.input, paths.input
                    ));
                }
                scope.insert(paths.output.clone());
            }
            PipelineStage::ShortestPaths(paths) => {
                if !scope.contains(&paths.input) {
                    return Err(format!(
                        "SHORTEST_PATHS 引用了未定义变量 {} (SHORTEST_PATHS references undefined variable {})",
                        paths.input, paths.input
                    ));
                }
                scope.insert(paths.output.clone());
            }
            PipelineStage::WeightedPaths(paths) => {
                if !scope.contains(&paths.input) {
                    return Err(format!("WEIGHTED_PATHS 引用了未定义变量 {}", paths.input));
                }
                scope.insert(paths.output.clone());
            }
            PipelineStage::YenPaths(paths) => {
                if !scope.contains(&paths.input) {
                    return Err(format!("YEN_PATHS 引用了未定义变量 {}", paths.input));
                }
                scope.insert(paths.output.clone());
            }
            PipelineStage::NodeSimilarity(stage) => {
                if !scope.contains(&stage.input) {
                    return Err(format!("NODE_SIMILARITY 引用了未定义变量 {}", stage.input));
                }
                scope.insert(stage.output.clone());
            }
            PipelineStage::SetCombine(combine) => {
                if !scope.contains(&combine.input) {
                    return Err(format!(
                        "集合运算引用了未定义变量 {} (Set operation references undefined variable {})",
                        combine.input, combine.input
                    ));
                }
                scope.insert(combine.output.clone());
            }
            PipelineStage::Iterate(iterate) => {
                if !scope.contains(&iterate.input) {
                    return Err(format!(
                        "ITERATE 引用了未定义变量 {} (ITERATE references undefined variable {})",
                        iterate.input, iterate.input
                    ));
                }
                scope.insert(iterate.output.clone());
            }
            PipelineStage::Diversify(stage) => {
                if !scope.contains(&stage.input) {
                    return Err(format!("DIVERSIFY 引用了未定义变量 {}", stage.input));
                }
                scope.insert(stage.output.clone());
            }
            PipelineStage::Residual(stage) => {
                if !scope.contains(&stage.input) {
                    return Err(format!("RESIDUAL 引用了未定义变量 {}", stage.input));
                }
                scope.insert(stage.output.clone());
            }
            PipelineStage::Topics(stage) => {
                if !scope.contains(&stage.input) {
                    return Err(format!("TOPICS 引用了未定义变量 {}", stage.input));
                }
                scope.insert(stage.output.clone());
            }
            PipelineStage::SaPpr(stage) => {
                if !scope.contains(&stage.input) {
                    return Err(format!("SA_PPR_CONFIG 引用了未定义变量 {}", stage.input));
                }
                scope.insert(stage.output.clone());
            }
            PipelineStage::Filter(predicate) => {
                let mut vars = Vec::new();
                predicate_vars(predicate, &mut vars);
                if let Some(var) = vars
                    .into_iter()
                    .find(|var| !scope.contains(var) && !scalar_aliases.contains(var))
                {
                    return Err(format!(
                        "WHERE 引用了作用域外变量 {var} (WHERE references out-of-scope variable {var})"
                    ));
                }
            }
            PipelineStage::Rank(rank) => {
                if !scope.contains(&rank.var) {
                    return Err(format!(
                        "RANK 引用了未定义变量 {} (RANK references undefined variable {})",
                        rank.var, rank.var
                    ));
                }
            }
        }
    }
    let final_scope = scope;
    let mut referenced = Vec::new();
    if let Some(predicate) = &query.predicate {
        predicate_vars(predicate, &mut referenced);
    }
    for order in &query.order_by {
        collect_expr_vars(&order.expr, &mut referenced);
    }
    match &query.returns {
        ReturnClause::Variables(vars) => referenced.extend(vars.iter().cloned()),
        ReturnClause::Expressions(expressions) => {
            for expression in expressions {
                match &expression.kind {
                    ReturnExprKind::Var(var) | ReturnExprKind::Property(var, _) => {
                        referenced.push(var.clone());
                    }
                    ReturnExprKind::Scalar(expr) => collect_expr_vars(expr, &mut referenced),
                    ReturnExprKind::Aggregate(_, inner) => {
                        collect_return_kind_vars(inner, &mut referenced);
                    }
                }
            }
        }
        ReturnClause::All => {}
    }
    if let Some(var) = referenced
        .into_iter()
        .find(|var| !final_scope.contains(var) && !scalar_aliases.contains(var))
    {
        return Err(format!(
            "RETURN/WHERE/ORDER BY 引用了作用域外变量 {var} (Final clause references out-of-scope variable {var})"
        ));
    }
    Ok(())
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
