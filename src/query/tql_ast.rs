//! TQL (Trivium Query Language) 统一抽象语法树
//!
//! 合并 Cypher 图模式匹配和 MongoDB 文档过滤为统一 AST。
//! 支持查询入口：MATCH / OPTIONAL MATCH / FIND / SEARCH
//! 支持聚合函数：COUNT / SUM / AVG / MIN / MAX / COLLECT
//! 支持 DISTINCT 去重、AS 别名、EXPLAIN 查询计划

use crate::filter::Filter;

/// TQL 顶层查询 AST
#[derive(Debug, Clone)]
pub struct TqlQuery {
    /// EXPLAIN 模式：输出查询计划而不执行
    pub explain: bool,
    /// EXPLAIN ANALYZE：执行查询并返回计划与实际观测。
    pub analyze: bool,
    /// 查询入口
    pub entry: QueryEntry,
    /// `WITH` 链形成的管线阶段。空数组表示旧单阶段查询。
    pub pipeline: Vec<PipelineStage>,
    /// WHERE 过滤（统一谓词）
    pub predicate: Option<Predicate>,
    /// MATCH 产生 anchor 后，在指定变量集合内执行精确向量排序。
    pub rank: Option<RankClause>,
    /// RETURN 投影
    pub returns: ReturnClause,
    /// ORDER BY
    pub order_by: Vec<OrderExpr>,
    /// LIMIT
    pub limit: Option<usize>,
    /// OFFSET
    pub offset: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct RankClause {
    pub var: String,
    pub vector: Vec<f64>,
    pub top_k: usize,
}

#[derive(Debug, Clone)]
pub enum PipelineStage {
    With(WithStage),
    Expand(PipelineExpandStage),
    Filter(Predicate),
    Rank(RankClause),
    GraphAlgorithm(GraphAlgorithmStage),
    AllPaths(AllPathsStage),
    ShortestPaths(ShortestPathsStage),
    SetCombine(SetCombineStage),
    Iterate(IterateStage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphAlgorithmKind {
    PageRank,
    Wcc,
    Degree,
    LabelPropagation,
    Leiden,
    SaPpr,
}

#[derive(Debug, Clone)]
pub struct GraphAlgorithmStage {
    pub input: String,
    pub output: String,
    pub algorithm: GraphAlgorithmKind,
}

#[derive(Debug, Clone)]
pub struct WithStage {
    pub items: Vec<WithItem>,
}

#[derive(Debug, Clone)]
pub struct WithItem {
    pub expr: TqlExpr,
    pub alias: String,
}

#[derive(Debug, Clone)]
pub struct PipelineExpandStage {
    pub input: String,
    pub output: String,
    pub expand: ExpandClause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathAggregation {
    MaxProduct,
    SumProduct,
    AverageWeight,
}

#[derive(Debug, Clone)]
pub struct AllPathsStage {
    pub input: String,
    pub output: String,
    pub targets: Vec<u64>,
    pub max_depth: usize,
    pub max_paths: usize,
    pub aggregation: PathAggregation,
    pub label_sequence: Option<Vec<String>>,
    pub forbidden_nodes: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct ShortestPathsStage {
    pub input: String,
    pub output: String,
    pub targets: Vec<u64>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TqlSetOperation {
    Union,
    Intersect,
    Except,
}

#[derive(Debug, Clone)]
pub struct SetCombineStage {
    pub input: String,
    pub other_ids: Vec<u64>,
    pub output: String,
    pub operation: TqlSetOperation,
}

#[derive(Debug, Clone)]
pub struct IterateStage {
    pub input: String,
    pub output: String,
    pub expand: ExpandClause,
    pub max_iterations: usize,
    pub stop_on_fixed_point: bool,
}

/// 查询入口类型
#[derive(Debug, Clone)]
pub enum QueryEntry {
    /// MATCH (a)-[:r]->(b)
    Match { pattern: TqlPattern },

    /// OPTIONAL MATCH (a)-[:r]->(b) — 左外连接语义
    OptionalMatch { pattern: TqlPattern },

    /// FIND {type: "event", heat: {$gt: 0.7}}
    Find { filter: Filter },

    /// SEARCH VECTOR [...] TOP k [EXPAND ...]
    Search {
        vector: Vec<f64>,
        top_k: usize,
        expand: Option<ExpandClause>,
    },
}

/// 图路径模式：交替的节点模式和边模式
#[derive(Debug, Clone)]
pub struct TqlPattern {
    pub nodes: Vec<TqlNodePattern>,
    pub edges: Vec<TqlEdgePattern>,
    // 布局: nodes[0] -edges[0]-> nodes[1] -edges[1]-> nodes[2] ...
    // 保证 nodes.len() == edges.len() + 1
}

/// 节点模式：(varName {doc_filter})
#[derive(Debug, Clone)]
pub struct TqlNodePattern {
    /// 绑定变量名（可选）
    pub var: Option<String>,
    /// 文档过滤条件（支持 Mongo 操作符，Q1 决策 B）
    pub filter: Option<Filter>,
}

/// 边遍历方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDirection {
    /// 正向：(a)-[]->(b)
    Forward,
    /// 反向：(a)<-[]-(b)  — "谁指向了 a"
    Backward,
    /// 双向：(a)-[]-(b)
    Both,
}

/// 边模式：-[:label1|label2*min..max]->
#[derive(Debug, Clone)]
pub struct TqlEdgePattern {
    /// 边标签过滤（多标签 OR，Q2 决策 A）
    pub labels: Vec<String>,
    /// 可变长跳数范围（None 表示恰好 1 跳）
    pub hop_range: Option<HopRange>,
    /// 遍历方向
    pub direction: EdgeDirection,
}

/// 可变长跳数范围
#[derive(Debug, Clone, Copy)]
pub struct HopRange {
    pub min: usize,
    pub max: usize,
}

/// EXPAND 子句（SEARCH 入口专用）
#[derive(Debug, Clone)]
pub struct ExpandClause {
    /// 边标签过滤
    pub labels: Vec<String>,
    /// 跳数范围
    pub min_depth: usize,
    pub max_depth: usize,
    /// 默认正向；可显式指定反向或双向。
    pub direction: EdgeDirection,
}

// ═══════════════════════════════════════════════════════════════════════
//  统一谓词系统
// ═══════════════════════════════════════════════════════════════════════

/// 统一谓词表达式（合并 Cypher Condition + MongoDB Filter）
#[derive(Debug, Clone)]
pub enum Predicate {
    /// Cypher 风格比较: a.name == "Alice"
    Compare {
        left: TqlExpr,
        op: TqlCompOp,
        right: TqlExpr,
    },

    /// MongoDB 风格文档过滤，绑定到指定变量
    /// `boss MATCHES {level: {$in: ["director"]}}`
    DocFilter {
        /// 绑定变量（None = 作用于隐式上下文，如 FIND 场景）
        var: Option<String>,
        filter: Filter,
    },

    /// 逻辑与
    And(Box<Predicate>, Box<Predicate>),
    /// 逻辑或
    Or(Box<Predicate>, Box<Predicate>),
    /// 逻辑非
    Not(Box<Predicate>),
}

/// TQL 表达式
#[derive(Debug, Clone)]
pub enum TqlExpr {
    /// 节点集合、标量别名或节点变量引用。
    Variable(String),
    /// Prepared TQL 参数。
    Parameter(String),
    /// 确定性二元算术表达式。
    Binary {
        left: Box<TqlExpr>,
        op: TqlBinaryOp,
        right: Box<TqlExpr>,
    },
    /// 返回首个非空表达式。
    Coalesce(Vec<TqlExpr>),
    /// 空值判定。
    IsNull { expr: Box<TqlExpr>, negated: bool },
    /// 属性访问: a.name
    Property { var: String, field: String },
    /// 当前节点相对源查询向量的精确相似度。
    Similarity { var: String },
    /// 图算法分数列。
    GraphScore { var: String },
    /// 图扩展最小深度。
    Depth { var: String },
    /// 有界路径强度。
    PathStrength { var: String },
    /// 路径条数。
    PathCount { var: String },
    /// 社区 ID。
    Community { var: String },
    /// 当前行携带的路径。
    Path { var: String },
    /// 当前行携带路径的边数。
    PathLength { var: String },
    /// 字面量值
    Literal(TqlLiteral),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TqlBinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
}

/// TQL 字面量
#[derive(Debug, Clone)]
pub enum TqlLiteral {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
}

/// 比较运算符
#[derive(Debug, Clone, Copy)]
pub enum TqlCompOp {
    Eq,  // ==
    Ne,  // !=
    Gt,  // >
    Gte, // >=
    Lt,  // <
    Lte, // <=
}

// ═══════════════════════════════════════════════════════════════════════
//  投影与排序
// ═══════════════════════════════════════════════════════════════════════

/// RETURN 子句
#[derive(Debug, Clone)]
pub enum ReturnClause {
    /// RETURN *
    All,
    /// RETURN a, b
    Variables(Vec<String>),
    /// RETURN count(b), avg(b.age) AS avg_age, DISTINCT a.name
    Expressions(Vec<ReturnExpr>),
}

/// RETURN 表达式项
#[derive(Debug, Clone)]
pub struct ReturnExpr {
    pub kind: ReturnExprKind,
    pub alias: Option<String>,
    pub distinct: bool,
}

/// RETURN 表达式类型
#[derive(Debug, Clone)]
pub enum ReturnExprKind {
    /// 直接变量引用: a
    Var(String),
    /// 一等查询表达式，如 similarity(a)、depth(a)。
    Scalar(TqlExpr),
    /// 属性访问: a.name
    Property(String, String),
    /// 聚合函数: count(b), avg(b.age)
    Aggregate(AggFunc, Box<ReturnExprKind>),
}

/// 聚合函数类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AggFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Collect,
}

/// ORDER BY 表达式
#[derive(Debug, Clone)]
pub struct OrderExpr {
    pub expr: TqlExpr,
    pub descending: bool,
}

// ═══════════════════════════════════════════════════════════════════════
//  DML（写操作）AST
// ═══════════════════════════════════════════════════════════════════════

/// TQL 顶层语句：读查询 或 写操作
#[derive(Debug, Clone)]
pub enum TqlStatement {
    Query(TqlQuery),
    Mutation(TqlMutation),
}

/// TQL 写操作 AST
#[derive(Debug, Clone)]
pub struct TqlMutation {
    /// 可选前置 MATCH + WHERE（用于 SET / DELETE 定位目标节点）
    pub source: Option<MutationSource>,
    /// 写操作类型
    pub action: MutationAction,
}

/// 写操作的数据源（MATCH 定位）
#[derive(Debug, Clone)]
pub struct MutationSource {
    pub pattern: TqlPattern,
    pub predicate: Option<Predicate>,
}

/// 写操作类型
#[derive(Debug, Clone)]
pub enum MutationAction {
    /// CREATE ({name: "Alice"})
    /// CREATE (a)-[:knows]->(b)
    Create(CreateAction),
    /// SET a.name = "Bob", a.age = 30
    Set(Vec<SetAssignment>),
    /// DELETE a
    Delete { vars: Vec<String>, detach: bool },
}

/// CREATE 操作
#[derive(Debug, Clone)]
pub struct CreateAction {
    /// 待创建的节点
    pub nodes: Vec<CreateNode>,
    /// 待创建的边（引用已 MATCH 的变量 或 本次 CREATE 的变量）
    pub edges: Vec<CreateEdge>,
}

/// CREATE 节点
#[derive(Debug, Clone)]
pub struct CreateNode {
    /// 绑定变量（可选，用于后续引用）
    pub var: Option<String>,
    /// 节点 payload
    pub payload: serde_json::Value,
}

/// CREATE 边
#[derive(Debug, Clone)]
pub struct CreateEdge {
    pub src_var: String,
    pub dst_var: String,
    pub label: String,
    pub weight: f32,
}

/// SET 赋值
#[derive(Debug, Clone)]
pub struct SetAssignment {
    pub var: String,
    pub field: String,
    pub value: serde_json::Value,
}
