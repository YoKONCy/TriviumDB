//! 图遍历的统一预算与耗尽策略。
//!
//! 所有 BFS/路径/扩散算子共享访问节点、扫描边、前沿峰值和深度四个维度，
//! 使调用方能在分配或继续扩展前停止失控查询。默认策略为 fail-closed；只有
//! 显式选择 Partial 时才允许返回截断结果，调用者必须同时检查 truncated 语义。

use crate::error::{Result, TriviumError};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetExhaustionPolicy {
    Error,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TraversalBudget {
    pub max_visited_nodes: usize,
    pub max_examined_edges: usize,
    pub max_frontier_size: usize,
    pub max_depth: usize,
    pub exhaustion_policy: BudgetExhaustionPolicy,
}

impl Default for TraversalBudget {
    fn default() -> Self {
        Self {
            max_visited_nodes: 10_000,
            max_examined_edges: 50_000,
            max_frontier_size: 10_000,
            max_depth: 10,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        }
    }
}

impl TraversalBudget {
    pub fn validate(&self) -> Result<()> {
        if self.max_visited_nodes == 0
            || self.max_examined_edges == 0
            || self.max_frontier_size == 0
        {
            return Err(TriviumError::InvalidInput(
                "遍历预算必须大于 0 (Traversal budgets must be greater than zero)".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TraversalMetrics {
    pub visited_nodes: usize,
    pub examined_edges: usize,
    pub peak_frontier_size: usize,
    pub depth_reached: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetDimension {
    VisitedNodes,
    ExaminedEdges,
    FrontierSize,
    Depth,
}

pub(crate) fn exhausted(
    budget: &TraversalBudget,
    dimension: BudgetDimension,
    metrics: TraversalMetrics,
) -> Result<bool> {
    match budget.exhaustion_policy {
        BudgetExhaustionPolicy::Partial => Ok(true),
        BudgetExhaustionPolicy::Error => Err(TriviumError::TraversalBudgetExceeded {
            dimension,
            visited_nodes: metrics.visited_nodes,
            examined_edges: metrics.examined_edges,
            peak_frontier_size: metrics.peak_frontier_size,
            depth_reached: metrics.depth_reached,
        }),
    }
}
