//! TQL 从词法分析到物理执行的完整查询子系统。
//!
//! 文本依次经过 Lexer、Parser/AST、Planner/Cascades 和 NodeSet Pipeline，最后投影为
//! 一等值结果。Prepared 只替换已验证参数，所有阶段共享预算、确定性和 EXPLAIN 语义。

pub mod cascades;
pub mod parallel;
pub mod pipeline;
pub mod planner;
pub mod tql_ast;
pub mod tql_executor;
pub mod tql_lexer;
pub mod tql_parser;
pub mod tql_prepared;
