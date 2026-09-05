//! 图遍历、路径、社区与中心性算法集合。
//!
//! 算法直接读取 MemTable/NodeSet 子图，统一使用稳定 NodeId 顺序、TraversalBudget 和
//! 明确的截断策略；并行实现只优化执行，不得改变语义或确定性。

pub mod analytics;
pub mod budget;
pub mod centrality;
pub mod constrained;
pub mod leiden;
pub mod pagerank;
pub mod pathfinding;
pub mod reachability;
pub mod subset;
pub mod traversal;
pub mod wcc;
