//! TQL NodeSet 物理算子与统一执行上下文。
//!
//! NodeSet 是阶段间唯一交换格式，携带稳定 NodeId、score、provenance、Path 和截断信息。
//! 本模块实现扩展、过滤、精排、图算法、路径、集合代数与定点迭代，并统一执行内存、
//! 遍历及并行预算。并行算子必须稳定归并，任何线程数都不能改变输出顺序或分数位模式。

use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::filter::Filter;
use crate::graph::budget::{BudgetExhaustionPolicy, TraversalBudget};
use crate::graph::reachability::{
    ReachabilityConfig, ReachabilityDirection, traverse_compact, traverse_compact_parallel,
};
use crate::node::NodeId;
use crate::query::parallel::{QueryParallelismBudget, query_pool};
use crate::storage::memtable::MemTable;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoreKind {
    Approximate,
    Exact,
    DepthBounded,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ScoreValue {
    pub value: f32,
    pub kind: ScoreKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PropertyOrigin {
    pub field: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Provenance {
    pub source_ids: Vec<NodeId>,
    pub min_depth: Option<usize>,
    /// 仅在集合由单个等值属性谓词产生时保留，供后续成本估算与审计使用。
    pub property_origin: Option<PropertyOrigin>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeRow {
    pub id: NodeId,
    pub similarity: Option<ScoreValue>,
    pub property_score: Option<ScoreValue>,
    pub graph_score: Option<ScoreValue>,
    pub community_id: Option<u64>,
    pub path_strength: Option<ScoreValue>,
    pub path_count: Option<usize>,
    pub path: Option<Vec<NodeId>>,
    pub provenance: Provenance,
}

impl NodeRow {
    pub fn new(id: NodeId) -> Self {
        Self {
            id,
            similarity: None,
            property_score: None,
            graph_score: None,
            community_id: None,
            path_strength: None,
            path_count: None,
            path: None,
            provenance: Provenance::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct NodeSet {
    rows: Vec<NodeRow>,
}

impl NodeSet {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_ids(ids: impl IntoIterator<Item = NodeId>) -> Self {
        let mut rows = ids.into_iter().map(NodeRow::new).collect::<Vec<_>>();
        rows.sort_unstable_by_key(|row| row.id);
        rows.dedup_by_key(|row| row.id);
        Self { rows }
    }

    pub fn from_rows(rows: Vec<NodeRow>) -> Self {
        let mut set = Self { rows };
        set.normalize();
        set
    }

    pub fn rows(&self) -> &[NodeRow] {
        &self.rows
    }

    pub fn into_rows(self) -> Vec<NodeRow> {
        self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn estimated_bytes(&self) -> usize {
        self.rows
            .len()
            .saturating_mul(std::mem::size_of::<NodeRow>())
    }

    fn normalize(&mut self) {
        self.rows.sort_by_key(|row| row.id);
        self.rows.dedup_by(|left, right| {
            if left.id != right.id {
                return false;
            }
            merge_row(left, right);
            true
        });
    }
}

fn merge_row(target: &mut NodeRow, source: &NodeRow) {
    target.similarity = better_score(target.similarity, source.similarity);
    target.property_score = better_score(target.property_score, source.property_score);
    target.graph_score = better_score(target.graph_score, source.graph_score);
    target.community_id = target.community_id.or(source.community_id);
    target.path_strength = better_score(target.path_strength, source.path_strength);
    target.path_count = match (target.path_count, source.path_count) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        (left, right) => left.or(right),
    };
    if source.path.as_ref().is_some_and(|candidate| {
        target.path.as_ref().is_none_or(|current| {
            candidate.len() < current.len()
                || candidate.len() == current.len() && candidate < current
        })
    }) {
        target.path = source.path.clone();
    }
    target
        .provenance
        .source_ids
        .extend(source.provenance.source_ids.iter().copied());
    target.provenance.source_ids.sort_unstable();
    target.provenance.source_ids.dedup();
    target.provenance.min_depth = match (target.provenance.min_depth, source.provenance.min_depth) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    };
    if target.provenance.property_origin != source.provenance.property_origin {
        target.provenance.property_origin = None;
    }
}

fn better_score(left: Option<ScoreValue>, right: Option<ScoreValue>) -> Option<ScoreValue> {
    fn quality(kind: ScoreKind) -> u8 {
        match kind {
            ScoreKind::Approximate => 0,
            ScoreKind::DepthBounded => 1,
            ScoreKind::Exact => 2,
        }
    }
    match (left, right) {
        (Some(left), Some(right)) => {
            if quality(right.kind) > quality(left.kind)
                || right.kind == left.kind && right.value > left.value
            {
                Some(right)
            } else {
                Some(left)
            }
        }
        (left, right) => left.or(right),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineBudget {
    pub max_stages: usize,
    pub max_nodes: usize,
    pub max_node_set_bytes: usize,
    pub max_vector_read_bytes: usize,
    pub traversal: TraversalBudget,
    pub parallelism: QueryParallelismBudget,
}

impl Default for PipelineBudget {
    fn default() -> Self {
        Self {
            max_stages: 16,
            max_nodes: 100_000,
            max_node_set_bytes: 64 * 1024 * 1024,
            max_vector_read_bytes: 256 * 1024 * 1024,
            traversal: TraversalBudget::default(),
            parallelism: QueryParallelismBudget::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PipelineStageMetrics {
    pub operator: &'static str,
    pub input_rows: usize,
    pub output_rows: usize,
    pub elapsed_ns: u64,
    pub node_set_bytes: usize,
    pub vector_read_bytes: usize,
    pub visited_nodes: usize,
    pub examined_edges: usize,
}

pub struct PipelineContext<'a, T: VectorType> {
    pub memtable: &'a MemTable<T>,
    pub budget: PipelineBudget,
    pub metrics: Vec<PipelineStageMetrics>,
    stages: usize,
    vector_read_bytes: usize,
}

impl<'a, T: VectorType> PipelineContext<'a, T> {
    pub fn new(memtable: &'a MemTable<T>, budget: PipelineBudget) -> Self {
        Self {
            memtable,
            budget,
            metrics: Vec::new(),
            stages: 0,
            vector_read_bytes: 0,
        }
    }

    fn begin_stage(&mut self) -> Result<()> {
        self.stages = self.stages.saturating_add(1);
        if self.stages > self.budget.max_stages {
            return Err(TriviumError::QueryExecution(format!(
                "管线阶段数超过预算 {} (Pipeline stage count exceeds budget {})",
                self.budget.max_stages, self.budget.max_stages
            )));
        }
        Ok(())
    }

    fn charge_vectors(&mut self, vectors: usize) -> Result<usize> {
        let bytes = vectors
            .checked_mul(self.memtable.dim())
            .and_then(|value| value.checked_mul(std::mem::size_of::<T>()))
            .ok_or_else(|| TriviumError::QueryExecution("管线向量读取字节溢出".into()))?;
        self.vector_read_bytes = self.vector_read_bytes.saturating_add(bytes);
        if self.vector_read_bytes > self.budget.max_vector_read_bytes {
            return Err(TriviumError::QueryExecution(format!(
                "管线向量读取 {} 字节超过预算 {} (Pipeline vector reads {} bytes exceed budget {})",
                self.vector_read_bytes,
                self.budget.max_vector_read_bytes,
                self.vector_read_bytes,
                self.budget.max_vector_read_bytes
            )));
        }
        Ok(bytes)
    }

    fn validate_output(&self, output: &NodeSet) -> Result<()> {
        let bytes = output.estimated_bytes();
        if output.len() > self.budget.max_nodes || bytes > self.budget.max_node_set_bytes {
            return Err(TriviumError::QueryExecution(format!(
                "管线 NodeSet 超过预算：{} 个节点，{} 字节 (Pipeline NodeSet exceeds budget: {} nodes, {} bytes)",
                output.len(),
                bytes,
                output.len(),
                bytes
            )));
        }
        Ok(())
    }
}

pub trait PipelineOperator<T: VectorType> {
    fn name(&self) -> &'static str;
    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet>;
}

pub fn execute_pipeline<T: VectorType>(
    context: &mut PipelineContext<'_, T>,
    operators: &[Box<dyn PipelineOperator<T> + '_>],
) -> Result<NodeSet> {
    let mut current = NodeSet::empty();
    for operator in operators {
        context.begin_stage()?;
        let input_rows = current.len();
        let started = Instant::now();
        current = operator.apply(current, context)?;
        current.normalize();
        context.validate_output(&current)?;
        context.metrics.push(PipelineStageMetrics {
            operator: operator.name(),
            input_rows,
            output_rows: current.len(),
            elapsed_ns: started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            node_set_bytes: current.estimated_bytes(),
            vector_read_bytes: context.vector_read_bytes,
            visited_nodes: 0,
            examined_edges: 0,
        });
    }
    Ok(current)
}

pub struct NodeIdsSource {
    pub ids: Vec<NodeId>,
}

impl<T: VectorType> PipelineOperator<T> for NodeIdsSource {
    fn name(&self) -> &'static str {
        "node_ids"
    }

    fn apply(&self, _input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        Ok(NodeSet::from_ids(
            self.ids
                .iter()
                .copied()
                .filter(|id| context.memtable.contains(*id)),
        ))
    }
}

pub struct ExactVectorSearch<T: VectorType> {
    pub query: Vec<T>,
    pub top_k: usize,
}

impl<T: VectorType> PipelineOperator<T> for ExactVectorSearch<T> {
    fn name(&self) -> &'static str {
        "exact_vector_search"
    }

    fn apply(&self, _input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        if self.query.len() != context.memtable.dim() {
            return Err(TriviumError::DimensionMismatch {
                expected: context.memtable.dim(),
                got: self.query.len(),
            });
        }
        let ids = context.memtable.all_node_ids();
        context.charge_vectors(ids.len())?;
        let mut rows = ids
            .into_iter()
            .filter_map(|id| {
                let vector = context.memtable.get_vector(id)?;
                let mut row = NodeRow::new(id);
                row.similarity = Some(ScoreValue {
                    value: T::similarity(&self.query, vector),
                    kind: ScoreKind::Exact,
                });
                row.provenance.source_ids.push(id);
                Some(row)
            })
            .collect::<Vec<_>>();
        rows.sort_by(compare_similarity);
        rows.truncate(self.top_k);
        Ok(NodeSet { rows })
    }
}

pub struct PropertyLookup {
    pub field: String,
    pub value: serde_json::Value,
}

impl<T: VectorType> PipelineOperator<T> for PropertyLookup {
    fn name(&self) -> &'static str {
        "property_lookup"
    }

    fn apply(&self, _input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let ids = if let Some(ids) = context
            .memtable
            .find_by_property_index(&self.field, &self.value)
        {
            ids
        } else {
            let threads = context
                .budget
                .parallelism
                .threads(context.memtable.node_count());
            if threads == 1 {
                context
                    .memtable
                    .find_nodes_by_field(&self.field, &self.value)
            } else {
                query_pool(threads)?.install(|| {
                    context
                        .memtable
                        .find_nodes_by_field_parallel(&self.field, &self.value, threads)
                })
            }
        };
        let mut output = NodeSet::from_ids(ids);
        for row in &mut output.rows {
            row.property_score = Some(ScoreValue {
                value: 1.0,
                kind: ScoreKind::Exact,
            });
            row.provenance.property_origin = Some(PropertyOrigin {
                field: self.field.clone(),
                value: self.value.clone(),
            });
        }
        Ok(output)
    }
}

pub struct ExactRerank<T: VectorType> {
    pub query: Vec<T>,
    pub top_k: Option<usize>,
}

impl<T: VectorType> PipelineOperator<T> for ExactRerank<T> {
    fn name(&self) -> &'static str {
        "exact_rerank"
    }

    fn apply(&self, mut input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        if self.query.len() != context.memtable.dim() {
            return Err(TriviumError::DimensionMismatch {
                expected: context.memtable.dim(),
                got: self.query.len(),
            });
        }
        context.charge_vectors(input.len())?;
        let threads = context.budget.parallelism.threads(input.len());
        let score_row = |mut row: NodeRow| {
            let vector = context.memtable.get_vector(row.id)?;
            row.similarity = Some(ScoreValue {
                value: T::similarity(&self.query, vector),
                kind: ScoreKind::Exact,
            });
            Some(row)
        };
        input.rows = if threads == 1 {
            input.rows.into_iter().filter_map(score_row).collect()
        } else {
            query_pool(threads)?
                .install(|| input.rows.into_par_iter().filter_map(score_row).collect())
        };
        if let Some(top_k) = self.top_k {
            if top_k == 0 {
                input.rows.clear();
            } else if top_k < input.rows.len() {
                input.rows.select_nth_unstable_by(top_k, compare_similarity);
                input.rows.truncate(top_k);
                input.rows.sort_by(compare_similarity);
            } else {
                input.rows.sort_by(compare_similarity);
            }
        } else {
            input.rows.sort_by(compare_similarity);
        }
        Ok(input)
    }
}

fn compare_similarity(left: &NodeRow, right: &NodeRow) -> std::cmp::Ordering {
    right
        .similarity
        .map(|score| score.value)
        .unwrap_or(f32::NEG_INFINITY)
        .total_cmp(
            &left
                .similarity
                .map(|score| score.value)
                .unwrap_or(f32::NEG_INFINITY),
        )
        .then_with(|| left.id.cmp(&right.id))
}

pub struct PayloadFilter {
    pub filter: Filter,
}

impl<T: VectorType> PipelineOperator<T> for PayloadFilter {
    fn name(&self) -> &'static str {
        "filter"
    }

    fn apply(&self, mut input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        input.rows.retain(|row| {
            context
                .memtable
                .get_payload(row.id)
                .is_some_and(|payload| self.filter.matches(payload))
        });
        Ok(input)
    }
}

pub struct Limit {
    pub limit: usize,
}

impl<T: VectorType> PipelineOperator<T> for Limit {
    fn name(&self) -> &'static str {
        "limit"
    }

    fn apply(&self, mut input: NodeSet, _context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        input.rows.truncate(self.limit);
        Ok(input)
    }
}

pub struct Expand {
    pub min_depth: usize,
    pub max_depth: usize,
    pub labels: Option<Vec<String>>,
    pub direction: ReachabilityDirection,
    pub include_input: bool,
}

#[derive(Debug, Clone, Copy)]
struct ExpandHit {
    target_id: NodeId,
    source_id: NodeId,
    depth: usize,
}

#[derive(Debug)]
struct ExpandSourceOutput {
    hits: Vec<ExpandHit>,
    visited_nodes: usize,
    examined_edges: usize,
}

impl<T: VectorType> PipelineOperator<T> for Expand {
    fn name(&self) -> &'static str {
        "expand"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        // 遍历期间只追加紧凑命中记录；结果确定后一次排序、线性分组并物化 NodeRow。
        // 这避免在热路径维护包含大 NodeRow 的树，也不承担通用 HashMap 的哈希成本。
        let sources = input.rows.iter().map(|row| row.id).collect::<Vec<_>>();
        let traversal = context.budget.traversal;
        let config = ReachabilityConfig {
            min_depth: self.min_depth,
            max_depth: self.max_depth.min(traversal.max_depth),
            labels: self.labels.clone(),
            direction: self.direction,
            max_visited_nodes: traversal.max_visited_nodes,
            max_results: context.budget.max_nodes,
            max_edges: traversal.max_examined_edges,
            max_frontier_size: traversal.max_frontier_size,
            exhaustion_policy: BudgetExhaustionPolicy::Error,
        };
        let traverse_source = |&source_id: &NodeId| -> Result<ExpandSourceOutput> {
            let reached = traverse_compact(context.memtable, source_id, &config)?;
            Ok(ExpandSourceOutput {
                hits: reached
                    .results
                    .into_iter()
                    .map(|target| ExpandHit {
                        target_id: target.target_id,
                        source_id,
                        depth: target.depth,
                    })
                    .collect(),
                visited_nodes: reached.visited_nodes,
                examined_edges: reached.traversed_edges,
            })
        };
        let graph_stats = context.memtable.graph_stats();
        let estimated_reachable = if sources.len() == 1 {
            let degree = graph_stats.avg_out_degree.max(1.0);
            (0..self.max_depth).fold(1.0f64, |total, _| {
                (total * degree).min(graph_stats.node_count as f64)
            }) as usize
        } else {
            sources.len()
        };
        let threads = if sources.len() == 1 && estimated_reachable < 100_000 {
            1
        } else {
            context.budget.parallelism.threads(estimated_reachable)
        };
        let outputs = if sources.len() == 1 && threads > 1 {
            vec![query_pool(threads)?.install(|| {
                let source_id = sources[0];
                let reached = traverse_compact_parallel(context.memtable, source_id, &config)?;
                Ok::<_, TriviumError>(ExpandSourceOutput {
                    hits: reached
                        .results
                        .into_iter()
                        .map(|target| ExpandHit {
                            target_id: target.target_id,
                            source_id,
                            depth: target.depth,
                        })
                        .collect(),
                    visited_nodes: reached.visited_nodes,
                    examined_edges: reached.traversed_edges,
                })
            })?]
        } else if threads == 1 {
            sources
                .iter()
                .map(traverse_source)
                .collect::<Result<Vec<_>>>()?
        } else {
            query_pool(threads)?.install(|| {
                sources
                    .par_iter()
                    .map(traverse_source)
                    .collect::<Result<Vec<_>>>()
            })?
        };
        let mut hits = Vec::<ExpandHit>::new();
        let mut visited_nodes = 0usize;
        let mut examined_edges = 0usize;
        for output in outputs {
            visited_nodes = visited_nodes.saturating_add(output.visited_nodes);
            examined_edges = examined_edges.saturating_add(output.examined_edges);
            if visited_nodes > traversal.max_visited_nodes
                || examined_edges > traversal.max_examined_edges
            {
                return Err(TriviumError::QueryExecution(
                    "多源图扩展超过全局遍历预算 (Multi-source expansion exceeds global traversal budget)".into(),
                ));
            }
            hits.extend(output.hits);
        }
        hits.sort_unstable_by_key(|hit| (hit.target_id, hit.source_id, hit.depth));
        let mut expanded_rows = Vec::new();
        let mut cursor = 0usize;
        while cursor < hits.len() {
            let target_id = hits[cursor].target_id;
            let mut row = NodeRow::new(target_id);
            let mut min_depth = usize::MAX;
            while cursor < hits.len() && hits[cursor].target_id == target_id {
                let hit = hits[cursor];
                if row.provenance.source_ids.last().copied() != Some(hit.source_id) {
                    row.provenance.source_ids.push(hit.source_id);
                }
                min_depth = min_depth.min(hit.depth);
                cursor += 1;
            }
            row.provenance.min_depth = Some(min_depth);
            row.graph_score = Some(ScoreValue {
                value: 1.0 / min_depth.max(1) as f32,
                kind: ScoreKind::Exact,
            });
            expanded_rows.push(row);
        }
        if !self.include_input {
            return Ok(NodeSet {
                rows: expanded_rows,
            });
        }
        Ok(NodeSet::from_rows(
            input.rows.into_iter().chain(expanded_rows).collect(),
        ))
    }
}

pub struct ExpandExactRerank<T: VectorType> {
    pub expand: Expand,
    pub query: Vec<T>,
    pub top_k: usize,
}

impl<T: VectorType> PipelineOperator<T> for ExpandExactRerank<T> {
    fn name(&self) -> &'static str {
        "expand_exact_rerank"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let expanded = self.expand.apply(input, context)?;
        ExactRerank {
            query: self.query.clone(),
            top_k: Some(self.top_k),
        }
        .apply(expanded, context)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorAggregation {
    Sum,
    Max,
    WeightedSum,
}

/// 多锚点扩展：保留所有命中锚点来源，并显式聚合锚点贡献。
pub struct MultiAnchorExpand {
    pub max_depth: usize,
    pub labels: Option<Vec<String>>,
    pub direction: ReachabilityDirection,
    pub anchor_weights: BTreeMap<NodeId, f32>,
    pub aggregation: AnchorAggregation,
}

impl<T: VectorType> PipelineOperator<T> for MultiAnchorExpand {
    fn name(&self) -> &'static str {
        "multi_anchor_expand"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let mut output = BTreeMap::<NodeId, NodeRow>::new();
        let mut visited_nodes = 0usize;
        let mut examined_edges = 0usize;
        for source in input.rows {
            let reached = traverse_compact(
                context.memtable,
                source.id,
                &ReachabilityConfig {
                    min_depth: 1,
                    max_depth: self.max_depth.min(context.budget.traversal.max_depth),
                    labels: self.labels.clone(),
                    direction: self.direction,
                    max_visited_nodes: context.budget.traversal.max_visited_nodes,
                    max_results: context.budget.max_nodes,
                    max_edges: context.budget.traversal.max_examined_edges,
                    max_frontier_size: context.budget.traversal.max_frontier_size,
                    exhaustion_policy: BudgetExhaustionPolicy::Error,
                },
            )?;
            visited_nodes = visited_nodes.saturating_add(reached.visited_nodes);
            examined_edges = examined_edges.saturating_add(reached.traversed_edges);
            if visited_nodes > context.budget.traversal.max_visited_nodes
                || examined_edges > context.budget.traversal.max_examined_edges
            {
                return Err(TriviumError::QueryExecution(
                    "多锚点扩展超过全局遍历预算 (Multi-anchor expansion exceeds global traversal budget)".into(),
                ));
            }
            let anchor_weight = self.anchor_weights.get(&source.id).copied().unwrap_or(1.0);
            for target in reached.results {
                let base = 1.0 / target.depth.max(1) as f32;
                let contribution = match self.aggregation {
                    AnchorAggregation::WeightedSum => base * anchor_weight,
                    AnchorAggregation::Sum | AnchorAggregation::Max => base,
                };
                let row = output.entry(target.target_id).or_insert_with(|| {
                    let mut row = NodeRow::new(target.target_id);
                    row.graph_score = Some(ScoreValue {
                        value: 0.0,
                        kind: ScoreKind::Exact,
                    });
                    row
                });
                let current = row.graph_score.map_or(0.0, |score| score.value);
                row.graph_score = Some(ScoreValue {
                    value: match self.aggregation {
                        AnchorAggregation::Max => current.max(contribution),
                        AnchorAggregation::Sum | AnchorAggregation::WeightedSum => {
                            current + contribution
                        }
                    },
                    kind: ScoreKind::Exact,
                });
                if row.provenance.source_ids.last().copied() != Some(source.id) {
                    row.provenance.source_ids.push(source.id);
                }
                row.provenance.min_depth = Some(
                    row.provenance
                        .min_depth
                        .map_or(target.depth, |depth| depth.min(target.depth)),
                );
            }
        }
        Ok(NodeSet::from_rows(output.into_values().collect()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStrengthAggregation {
    MaxProduct,
    SumProduct,
    AverageWeight,
}

/// 从输入锚点到目标集合执行批量最短路径。
pub struct BatchShortestPaths {
    pub targets: Vec<NodeId>,
    pub label_filter: Option<String>,
}

impl<T: VectorType> PipelineOperator<T> for BatchShortestPaths {
    fn name(&self) -> &'static str {
        "batch_shortest_paths"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let mut output = BTreeMap::<NodeId, NodeRow>::new();
        let mut consumed_nodes = 0usize;
        let mut consumed_edges = 0usize;
        for source in input.rows {
            for &target in &self.targets {
                let result = crate::graph::pathfinding::shortest_path_bidirectional(
                    context.memtable,
                    source.id,
                    target,
                    self.label_filter.as_deref(),
                    &context.budget.traversal,
                )?;
                consumed_nodes = consumed_nodes.saturating_add(result.metrics.visited_nodes);
                consumed_edges = consumed_edges.saturating_add(result.metrics.examined_edges);
                if consumed_nodes > context.budget.traversal.max_visited_nodes
                    || consumed_edges > context.budget.traversal.max_examined_edges
                {
                    return Err(TriviumError::QueryExecution(
                        "批量最短路径超过全局遍历预算 (Batch shortest paths exceed global traversal budget)".into(),
                    ));
                }
                let Some(path) = result.path else {
                    continue;
                };
                let depth = path.len().saturating_sub(1);
                let candidate = NodeRow {
                    id: target,
                    graph_score: Some(ScoreValue {
                        value: 1.0 / depth.max(1) as f32,
                        kind: ScoreKind::Exact,
                    }),
                    path_count: Some(1),
                    path: Some(path),
                    provenance: Provenance {
                        source_ids: vec![source.id],
                        min_depth: Some(depth),
                        property_origin: source.provenance.property_origin.clone(),
                    },
                    ..NodeRow::new(target)
                };
                if let Some(existing) = output.get_mut(&target) {
                    merge_row(existing, &candidate);
                } else {
                    output.insert(target, candidate);
                }
            }
        }
        Ok(NodeSet::from_rows(output.into_values().collect()))
    }
}

/// 从输入锚点到目标集合枚举有界简单路径并聚合路径强度。
pub struct BoundedAllPaths {
    pub targets: Vec<NodeId>,
    pub config: crate::graph::pathfinding::BoundedPathConfig,
    pub aggregation: PathStrengthAggregation,
}

impl<T: VectorType> PipelineOperator<T> for BoundedAllPaths {
    fn name(&self) -> &'static str {
        "bounded_all_paths"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let mut output = BTreeMap::<NodeId, NodeRow>::new();
        let mut consumed_nodes = 0usize;
        let mut consumed_edges = 0usize;
        for source in input.rows {
            for &target in &self.targets {
                let result = crate::graph::pathfinding::bounded_all_paths(
                    context.memtable,
                    source.id,
                    target,
                    &self.config,
                    &context.budget.traversal,
                )?;
                consumed_nodes = consumed_nodes.saturating_add(result.metrics.visited_nodes);
                consumed_edges = consumed_edges.saturating_add(result.metrics.examined_edges);
                if consumed_nodes > context.budget.traversal.max_visited_nodes
                    || consumed_edges > context.budget.traversal.max_examined_edges
                {
                    return Err(TriviumError::QueryExecution(
                        "批量全路径超过全局遍历预算 (Batch all-paths exceed global traversal budget)".into(),
                    ));
                }
                if result.paths.is_empty() {
                    continue;
                }
                let score = match self.aggregation {
                    PathStrengthAggregation::MaxProduct => result
                        .paths
                        .iter()
                        .map(|path| path.strength_product)
                        .max_by(f32::total_cmp)
                        .unwrap_or(0.0),
                    PathStrengthAggregation::SumProduct => {
                        result.paths.iter().map(|path| path.strength_product).sum()
                    }
                    PathStrengthAggregation::AverageWeight => {
                        result
                            .paths
                            .iter()
                            .map(|path| path.strength_average)
                            .sum::<f32>()
                            / result.paths.len() as f32
                    }
                };
                let best_path = result
                    .paths
                    .iter()
                    .max_by(|left, right| {
                        left.strength_product
                            .total_cmp(&right.strength_product)
                            .then_with(|| right.nodes.cmp(&left.nodes))
                    })
                    .map(|path| path.nodes.clone());
                let depth = best_path
                    .as_ref()
                    .map_or(0, |path| path.len().saturating_sub(1));
                let candidate = NodeRow {
                    id: target,
                    path_strength: Some(ScoreValue {
                        value: score,
                        kind: ScoreKind::Exact,
                    }),
                    path_count: Some(result.paths.len()),
                    path: best_path,
                    provenance: Provenance {
                        source_ids: vec![source.id],
                        min_depth: Some(depth),
                        property_origin: source.provenance.property_origin.clone(),
                    },
                    ..NodeRow::new(target)
                };
                if let Some(existing) = output.get_mut(&target) {
                    merge_row(existing, &candidate);
                } else {
                    output.insert(target, candidate);
                }
            }
        }
        Ok(NodeSet::from_rows(output.into_values().collect()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphSubsetMode {
    Induced,
    Expand {
        hops: usize,
        labels: Option<Vec<String>>,
        direction: ReachabilityDirection,
    },
    Boundary {
        hops: usize,
        labels: Option<Vec<String>>,
        direction: ReachabilityDirection,
    },
}

fn graph_subset<T: VectorType>(
    input: &NodeSet,
    mode: &GraphSubsetMode,
    context: &mut PipelineContext<'_, T>,
) -> Result<(BTreeSet<NodeId>, BTreeSet<NodeId>)> {
    let original = ids(input);
    match mode {
        GraphSubsetMode::Induced => Ok((original.clone(), original)),
        GraphSubsetMode::Expand {
            hops,
            labels,
            direction,
        }
        | GraphSubsetMode::Boundary {
            hops,
            labels,
            direction,
        } => {
            let expanded = Expand {
                min_depth: 1,
                max_depth: *hops,
                labels: labels.clone(),
                direction: *direction,
                include_input: true,
            }
            .apply(input.clone(), context)?;
            let universe = ids(&expanded);
            let output = if matches!(mode, GraphSubsetMode::Boundary { .. }) {
                original
            } else {
                universe.clone()
            };
            Ok((universe, output))
        }
    }
}

pub struct PageRankOperator {
    pub mode: GraphSubsetMode,
    pub config: crate::graph::subset::SubsetPageRankConfig,
    pub label_filter: Option<String>,
}

impl<T: VectorType> PipelineOperator<T> for PageRankOperator {
    fn name(&self) -> &'static str {
        "subset_pagerank"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let (universe, output_ids) = graph_subset(&input, &self.mode, context)?;
        let threads = context.budget.parallelism.threads(universe.len());
        let result = if threads == 1 {
            crate::graph::subset::subset_pagerank(
                context.memtable,
                &universe,
                self.config,
                self.label_filter.as_deref(),
                context.budget.traversal.max_examined_edges,
            )?
        } else {
            query_pool(threads)?.install(|| {
                crate::graph::subset::subset_pagerank_parallel(
                    context.memtable,
                    &universe,
                    self.config,
                    self.label_filter.as_deref(),
                    context.budget.traversal.max_examined_edges,
                )
            })?
        };
        let mut rows = rows_by_id(&input)
            .into_iter()
            .map(|(id, row)| (id, row.clone()))
            .collect::<HashMap<_, _>>();
        for (id, score) in result.scores {
            if output_ids.contains(&id) {
                let row = rows.entry(id).or_insert_with(|| NodeRow::new(id));
                row.graph_score = Some(ScoreValue {
                    value: score as f32,
                    kind: ScoreKind::Exact,
                });
            }
        }
        Ok(NodeSet {
            rows: output_ids
                .into_iter()
                .filter_map(|id| rows.remove(&id))
                .collect(),
        })
    }
}

pub struct DegreeCentralityOperator {
    pub mode: GraphSubsetMode,
    pub label_filter: Option<String>,
}

impl<T: VectorType> PipelineOperator<T> for DegreeCentralityOperator {
    fn name(&self) -> &'static str {
        "subset_degree_centrality"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let (universe, output_ids) = graph_subset(&input, &self.mode, context)?;
        let degree_threads = context.budget.parallelism.threads(universe.len());
        let (scores, _) = if degree_threads == 1 {
            crate::graph::subset::subset_degree_centrality(
                context.memtable,
                &universe,
                self.label_filter.as_deref(),
                context.budget.traversal.max_examined_edges,
            )?
        } else {
            query_pool(degree_threads)?.install(|| {
                crate::graph::subset::subset_degree_centrality_parallel(
                    context.memtable,
                    &universe,
                    self.label_filter.as_deref(),
                    context.budget.traversal.max_examined_edges,
                )
            })?
        };
        let old = rows_by_id(&input);
        let build_row = |score: &crate::graph::subset::SubsetDegreeCentrality| {
            if !output_ids.contains(&score.id) {
                return None;
            }
            let mut row = old
                .get(&score.id)
                .map_or_else(|| NodeRow::new(score.id), |row| (*row).clone());
            row.graph_score = Some(ScoreValue {
                value: score.normalized as f32,
                kind: ScoreKind::Exact,
            });
            Some(row)
        };
        let rows = if degree_threads == 1 {
            scores.iter().filter_map(build_row).collect()
        } else {
            query_pool(degree_threads)?
                .install(|| scores.par_iter().filter_map(build_row).collect())
        };
        Ok(NodeSet { rows })
    }
}

pub struct BetweennessOperator {
    pub mode: GraphSubsetMode,
    pub label_filter: Option<String>,
    pub sample_size: Option<usize>,
}

impl<T: VectorType> PipelineOperator<T> for BetweennessOperator {
    fn name(&self) -> &'static str {
        "subset_betweenness"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let (universe, output_ids) = graph_subset(&input, &self.mode, context)?;
        let threads = context.budget.parallelism.threads(universe.len());
        let result = if threads == 1 {
            crate::graph::subset::subset_betweenness(
                context.memtable,
                &universe,
                self.label_filter.as_deref(),
                self.sample_size,
                context.budget.traversal.max_examined_edges,
            )?
        } else {
            query_pool(threads)?.install(|| {
                crate::graph::subset::subset_betweenness_parallel(
                    context.memtable,
                    &universe,
                    self.label_filter.as_deref(),
                    self.sample_size,
                    context.budget.traversal.max_examined_edges,
                )
            })?
        };
        let old = rows_by_id(&input);
        let kind = if result.exact {
            ScoreKind::Exact
        } else {
            ScoreKind::Approximate
        };
        let rows = result
            .scores
            .into_iter()
            .filter(|(id, _)| output_ids.contains(id))
            .map(|(id, score)| {
                let mut row = old
                    .get(&id)
                    .map_or_else(|| NodeRow::new(id), |row| (*row).clone());
                row.graph_score = Some(ScoreValue {
                    value: score as f32,
                    kind,
                });
                row
            })
            .collect();
        Ok(NodeSet { rows })
    }
}

pub struct WccOperator {
    pub mode: GraphSubsetMode,
    pub label_filter: Option<String>,
}

impl<T: VectorType> PipelineOperator<T> for WccOperator {
    fn name(&self) -> &'static str {
        "subset_wcc"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let (universe, output_ids) = graph_subset(&input, &self.mode, context)?;
        let threads = context.budget.parallelism.threads(universe.len());
        let (components, _) = if threads == 1 {
            crate::graph::subset::subset_wcc(
                context.memtable,
                &universe,
                self.label_filter.as_deref(),
                context.budget.traversal.max_examined_edges,
            )?
        } else {
            query_pool(threads)?.install(|| {
                crate::graph::subset::subset_wcc_parallel(
                    context.memtable,
                    &universe,
                    self.label_filter.as_deref(),
                    context.budget.traversal.max_examined_edges,
                )
            })?
        };
        let old = rows_by_id(&input);
        let mut memberships = BTreeMap::new();
        for (index, component) in components.into_iter().enumerate() {
            for id in component {
                memberships.insert(id, index as u64 + 1);
            }
        }
        let rows = output_ids
            .into_iter()
            .map(|id| {
                let mut row = old
                    .get(&id)
                    .map_or_else(|| NodeRow::new(id), |row| (*row).clone());
                row.community_id = memberships.get(&id).copied();
                row
            })
            .collect();
        Ok(NodeSet { rows })
    }
}

pub struct LeidenOperator {
    pub mode: GraphSubsetMode,
    pub config: crate::graph::leiden::LeidenConfig,
}

impl<T: VectorType> PipelineOperator<T> for LeidenOperator {
    fn name(&self) -> &'static str {
        "leiden_modularity"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let (universe, output_ids) = graph_subset(&input, &self.mode, context)?;
        let allowed = universe.iter().copied().collect::<BTreeSet<_>>();
        let mut edges = HashMap::new();
        let mut examined = 0usize;
        for &source in &universe {
            let mut neighbors = Vec::new();
            for edge in context.memtable.get_edges(source).unwrap_or(&[]) {
                examined = examined.saturating_add(1);
                if examined > context.budget.traversal.max_examined_edges {
                    return Err(TriviumError::QueryExecution(
                        "Leiden 检查边数量超过预算 (Leiden examined-edge budget exceeded)".into(),
                    ));
                }
                if allowed.contains(&edge.target_id) {
                    neighbors.push((edge.target_id, edge.weight));
                }
            }
            edges.insert(source, neighbors);
        }
        let result = crate::graph::leiden::run_leiden(
            &crate::graph::leiden::AdjacencySnapshot {
                edges,
                node_ids: universe.into_iter().collect(),
            },
            &self.config,
        );
        let old = rows_by_id(&input);
        let rows = output_ids
            .into_iter()
            .filter_map(|id| {
                let community = result.node_to_cluster.get(&id).copied()?;
                let mut row = old
                    .get(&id)
                    .map_or_else(|| NodeRow::new(id), |row| (*row).clone());
                row.community_id = Some(community as u64);
                Some(row)
            })
            .collect();
        Ok(NodeSet { rows })
    }
}

/// 确定性加权标签传播算子。它不是 Leiden，质量标记固定为近似。
pub struct LabelPropagationOperator {
    pub mode: GraphSubsetMode,
    pub config: crate::graph::subset::LabelPropagationConfig,
    pub label_filter: Option<String>,
}

impl<T: VectorType> PipelineOperator<T> for LabelPropagationOperator {
    fn name(&self) -> &'static str {
        "deterministic_label_propagation"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let (universe, output_ids) = graph_subset(&input, &self.mode, context)?;
        let threads = context.budget.parallelism.threads(universe.len());
        let result = if threads == 1 {
            crate::graph::subset::deterministic_label_propagation(
                context.memtable,
                &universe,
                self.config,
                self.label_filter.as_deref(),
                context.budget.traversal.max_examined_edges,
            )?
        } else {
            query_pool(threads)?.install(|| {
                crate::graph::subset::deterministic_label_propagation_parallel(
                    context.memtable,
                    &universe,
                    self.config,
                    self.label_filter.as_deref(),
                    context.budget.traversal.max_examined_edges,
                )
            })?
        };
        let old = rows_by_id(&input);
        let rows = output_ids
            .into_iter()
            .filter_map(|id| {
                let community = result.node_to_community.get(&id).copied()?;
                let mut row = old
                    .get(&id)
                    .map_or_else(|| NodeRow::new(id), |row| (*row).clone());
                row.community_id = Some(community);
                Some(row)
            })
            .collect();
        Ok(NodeSet { rows })
    }
}

/// 有限深度 SA-PPR 算子。分数质量明确标记为 DepthBounded，而非收敛型 PPR Exact。
pub struct SaPprOperator {
    pub max_depth: usize,
    pub restart_alpha: f32,
    pub labels: Option<Vec<String>>,
    pub max_edges_per_node: usize,
    pub min_edge_weight: f32,
}

impl<T: VectorType> PipelineOperator<T> for SaPprOperator {
    fn name(&self) -> &'static str {
        "sa_ppr_depth_bounded"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        let seeds = input
            .rows
            .iter()
            .filter_map(|row| {
                let payload = context.memtable.get_payload(row.id)?.clone();
                Some(crate::node::SearchHit {
                    id: row.id,
                    score: row
                        .similarity
                        .or(row.graph_score)
                        .map_or(1.0, |score| score.value.max(0.0)),
                    payload,
                })
            })
            .collect();
        let hits = crate::graph::traversal::expand_graph_with_labels(
            context.memtable,
            seeds,
            self.max_depth.min(context.budget.traversal.max_depth),
            self.restart_alpha,
            false,
            context.budget.max_nodes,
            false,
            None,
            self.labels.as_deref(),
            self.max_edges_per_node,
            self.min_edge_weight,
            crate::database::EdgeDirection::Outgoing,
        );
        Ok(NodeSet::from_rows(
            hits.into_iter()
                .map(|hit| {
                    let mut row = NodeRow::new(hit.id);
                    row.graph_score = Some(ScoreValue {
                        value: hit.score,
                        kind: ScoreKind::DepthBounded,
                    });
                    row.provenance.source_ids = input.rows.iter().map(|seed| seed.id).collect();
                    row.provenance.min_depth = Some(self.max_depth);
                    row
                })
                .collect(),
        ))
    }
}

pub struct BoundedIterate<T: VectorType> {
    pub operators: Vec<Box<dyn PipelineOperator<T>>>,
    pub max_iterations: usize,
    pub stop_on_fixed_point: bool,
}

impl<T: VectorType> PipelineOperator<T> for BoundedIterate<T> {
    fn name(&self) -> &'static str {
        "bounded_iterate"
    }

    fn apply(&self, input: NodeSet, context: &mut PipelineContext<'_, T>) -> Result<NodeSet> {
        if self.max_iterations == 0 {
            return Err(TriviumError::InvalidInput(
                "ITERATE 最大轮数必须大于 0 (ITERATE max_iterations must be greater than zero)"
                    .into(),
            ));
        }
        let mut current = input;
        let mut seen = ids(&current);
        for _ in 0..self.max_iterations {
            let mut next = current.clone();
            for operator in &self.operators {
                next = operator.apply(next, context)?;
                next.normalize();
                context.validate_output(&next)?;
            }
            let next_ids = ids(&next);
            let new_count = next_ids.difference(&seen).count();
            seen.extend(next_ids);
            current = combine_sets(current, next, SetOperation::Union);
            context.validate_output(&current)?;
            if self.stop_on_fixed_point && new_count == 0 {
                break;
            }
        }
        Ok(current)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOperation {
    Union,
    Intersect,
    Difference,
}

pub fn combine_sets(left: NodeSet, right: NodeSet, operation: SetOperation) -> NodeSet {
    let mut left = left
        .into_rows()
        .into_iter()
        .map(|row| (row.id, row))
        .collect::<BTreeMap<_, _>>();
    let right = right
        .into_rows()
        .into_iter()
        .map(|row| (row.id, row))
        .collect::<BTreeMap<_, _>>();
    match operation {
        SetOperation::Union => {
            for (id, row) in right {
                if let Some(existing) = left.get_mut(&id) {
                    merge_row(existing, &row);
                } else {
                    left.insert(id, row);
                }
            }
        }
        SetOperation::Intersect => {
            left.retain(|id, row| {
                if let Some(other) = right.get(id) {
                    merge_row(row, other);
                    true
                } else {
                    false
                }
            });
        }
        SetOperation::Difference => left.retain(|id, _| !right.contains_key(id)),
    }
    NodeSet {
        rows: left.into_values().collect(),
    }
}

pub fn lower_search_entry<T: VectorType>(
    vector: &[f64],
    top_k: usize,
    expand: Option<&crate::query::tql_ast::ExpandClause>,
) -> Vec<Box<dyn PipelineOperator<T>>> {
    let mut operators: Vec<Box<dyn PipelineOperator<T>>> = vec![Box::new(ExactVectorSearch {
        query: vector
            .iter()
            .map(|value| T::from_f32(*value as f32))
            .collect(),
        top_k,
    })];
    if let Some(expand) = expand {
        let direction = match expand.direction {
            crate::query::tql_ast::EdgeDirection::Forward => ReachabilityDirection::Outgoing,
            crate::query::tql_ast::EdgeDirection::Backward => ReachabilityDirection::Incoming,
            crate::query::tql_ast::EdgeDirection::Both => ReachabilityDirection::Both,
        };
        operators.push(Box::new(Expand {
            min_depth: expand.min_depth,
            max_depth: expand.max_depth,
            labels: (!expand.labels.is_empty()).then(|| expand.labels.clone()),
            direction,
            include_input: true,
        }));
    }
    operators
}

pub fn ids(set: &NodeSet) -> BTreeSet<NodeId> {
    set.rows.iter().map(|row| row.id).collect()
}

pub fn rows_by_id(set: &NodeSet) -> HashMap<NodeId, &NodeRow> {
    set.rows.iter().map(|row| (row.id, row)).collect()
}
