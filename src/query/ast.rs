/// TriviumDB 图谱查询语言 AST（抽象语法树）
///
/// 支持的语法子集（类 Cypher）：
///   MATCH (a)-[:knows]->(b)-[:likes]->(c)
///   WHERE c.age > 20 AND b.role == "admin"
///   RETURN c
///
///   MATCH (a {id: 42})-[]->(b)
///   RETURN b

/// 顶层查询
#[derive(Debug, Clone)]
pub struct Query {
    pub pattern: Pattern,
    pub where_clause: Option<Condition>,
    pub return_vars: Vec<String>,
}

/// 路径模式：交替的 节点模式 和 边模式
#[derive(Debug, Clone)]
pub struct Pattern {
    pub nodes: Vec<NodePattern>,
    pub edges: Vec<EdgePattern>,
    // 布局: nodes[0] -edges[0]-> nodes[1] -edges[1]-> nodes[2] ...
    // 保证 nodes.len() == edges.len() + 1
}

/// (varName {id: 42, name: "Alice"})
#[derive(Debug, Clone)]
pub struct NodePattern {
    pub var: Option<String>,
    pub props: Vec<PropFilter>,
}

/// 节点内联属性过滤: {key: value}
#[derive(Debug, Clone)]
pub struct PropFilter {
    pub key: String,
    pub value: LitValue,
}

/// -[:label]-> 或 -[]->
#[derive(Debug, Clone)]
pub struct EdgePattern {
    pub label: Option<String>,
}

/// WHERE 子句条件
#[derive(Debug, Clone)]
pub enum Condition {
    Compare { left: Expr, op: CompOp, right: Expr },
    And(Box<Condition>, Box<Condition>),
    Or(Box<Condition>, Box<Condition>),
}

/// 表达式
#[derive(Debug, Clone)]
pub enum Expr {
    /// a.name 形式
    Property { var: String, field: String },
    /// 字面量
    Literal(LitValue),
}

/// 字面量值
#[derive(Debug, Clone)]
pub enum LitValue {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
}

/// 比较运算符
#[derive(Debug, Clone, Copy)]
pub enum CompOp {
    Eq,  // ==
    Ne,  // !=
    Gt,  // >
    Gte, // >=
    Lt,  // <
    Lte, // <=
}
