//! TQL 入口与图模式的 AccessPath 规划器。
//!
//! Planner 根据主键、四类属性索引、Fast Tags、Label/方向和统计选择候选生成路径，
//! 再由执行器做完整谓词验证。成本比较使用稳定 tie-break，索引缺失或选择性不足时
//! 退化为扫描而不改变查询语义；EXPLAIN 复用同一决策结果。

use super::tql_ast::{EdgeDirection, TqlEdgePattern, TqlNodePattern, TqlPattern};
use crate::VectorType;
use crate::filter::Filter;
use crate::node::NodeId;
use crate::storage::memtable::MemTable;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccessPath {
    PrimaryKey { id: NodeId },
    PropertyIndex { field: String },
    OrderedPropertyIndex { field: String, descending: bool },
    CompositePropertyIndex { fields: Vec<String> },
    BitmapPropertyIndex { fields: Vec<String> },
    PropertyIndexIntersection { fields: Vec<String> },
    EdgeLabelIndex { labels: Vec<String> },
    FullNodeScan,
}

impl AccessPath {
    pub fn name(&self) -> &'static str {
        match self {
            Self::PrimaryKey { .. } => "primary_key",
            Self::PropertyIndex { .. } => "property_index",
            Self::OrderedPropertyIndex { .. } => "ordered_property_index",
            Self::CompositePropertyIndex { .. } => "composite_property_index",
            Self::BitmapPropertyIndex { .. } => "bitmap_property_index",
            Self::PropertyIndexIntersection { .. } => "property_index_intersection",
            Self::EdgeLabelIndex { .. } => "edge_label_index",
            Self::FullNodeScan => "full_node_scan",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeAccessPlan {
    pub access_path: AccessPath,
    pub estimated_rows: usize,
    #[serde(skip)]
    pub candidates: Vec<NodeId>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatchPlan {
    pub access_path: AccessPath,
    pub estimated_rows: usize,
    pub reversed: bool,
    #[serde(skip)]
    pub pattern: TqlPattern,
    #[serde(skip)]
    pub candidates: Vec<NodeId>,
}

pub fn plan_filter_ordered<T: VectorType>(
    filter: &Filter,
    order_field: &str,
    descending: bool,
    limit: Option<usize>,
    mt: &MemTable<T>,
) -> Option<NodeAccessPlan> {
    if let Some((field, op, inclusive, value)) = ordered_range(filter)
        && field == order_field
    {
        let candidates =
            mt.find_by_property_range(field, op, inclusive, &value, descending, limit)?;
        return Some(NodeAccessPlan {
            access_path: AccessPath::OrderedPropertyIndex {
                field: field.to_owned(),
                descending,
            },
            estimated_rows: candidates.len(),
            candidates,
        });
    }
    let candidates = mt.ordered_property_ids(order_field, descending, limit)?;
    Some(NodeAccessPlan {
        access_path: AccessPath::OrderedPropertyIndex {
            field: order_field.to_owned(),
            descending,
        },
        estimated_rows: candidates.len(),
        candidates,
    })
}

pub fn plan_filter_with_limit<T: VectorType>(
    filter: &Filter,
    limit: Option<usize>,
    mt: &MemTable<T>,
) -> NodeAccessPlan {
    if let Some((field, op, inclusive, value)) = ordered_range(filter)
        && let Some((fields, candidates)) = mt.find_by_composite_property_range(
            &filter_equalities(filter),
            field,
            op,
            inclusive,
            &value,
            false,
            limit,
        )
    {
        return NodeAccessPlan {
            access_path: AccessPath::CompositePropertyIndex { fields },
            estimated_rows: candidates.len(),
            candidates,
        };
    }
    if let Some((field, op, inclusive, value)) = ordered_range(filter)
        && let Some(candidates) =
            mt.find_by_property_range(field, op, inclusive, &value, false, limit)
    {
        return NodeAccessPlan {
            access_path: AccessPath::OrderedPropertyIndex {
                field: field.to_owned(),
                descending: false,
            },
            estimated_rows: candidates.len(),
            candidates,
        };
    }
    plan_filter(filter, mt)
}

pub fn plan_filter<T: VectorType>(filter: &Filter, mt: &MemTable<T>) -> NodeAccessPlan {
    if let Some((field, op, inclusive, value)) = ordered_range(filter)
        && let Some((fields, candidates)) = mt.find_by_composite_property_range(
            &filter_equalities(filter),
            field,
            op,
            inclusive,
            &value,
            false,
            None,
        )
    {
        return NodeAccessPlan {
            access_path: AccessPath::CompositePropertyIndex { fields },
            estimated_rows: candidates.len(),
            candidates,
        };
    }
    if let Some((field, op, inclusive, value)) = ordered_range(filter)
        && let Some(candidates) =
            mt.find_by_property_range(field, op, inclusive, &value, false, None)
    {
        return NodeAccessPlan {
            access_path: AccessPath::OrderedPropertyIndex {
                field: field.to_owned(),
                descending: false,
            },
            estimated_rows: candidates.len(),
            candidates,
        };
    }

    let equalities = filter_equalities(filter);
    if let Some((fields, candidates)) = mt.find_by_composite_property_index(&equalities) {
        return NodeAccessPlan {
            access_path: AccessPath::CompositePropertyIndex { fields },
            estimated_rows: candidates.len(),
            candidates,
        };
    }

    if let Some((fields, candidates)) = bitmap_filter_candidates(filter, mt) {
        return NodeAccessPlan {
            access_path: AccessPath::BitmapPropertyIndex { fields },
            estimated_rows: candidates.len(),
            candidates,
        };
    }

    let indexed = indexed_equalities(filter, mt);
    if !indexed.is_empty() {
        let indexed_equalities = indexed
            .iter()
            .map(|(field, value, _)| (field.clone(), value.clone()))
            .collect::<Vec<_>>();
        if let Some(candidates) = mt.find_by_bitmap_intersection(&indexed_equalities) {
            return NodeAccessPlan {
                access_path: AccessPath::BitmapPropertyIndex {
                    fields: indexed_equalities
                        .iter()
                        .map(|(field, _)| field.clone())
                        .collect(),
                },
                estimated_rows: candidates.len(),
                candidates,
            };
        }
        let mut sorted = indexed;
        sorted.sort_by(|left, right| {
            left.2
                .len()
                .cmp(&right.2.len())
                .then_with(|| left.0.cmp(&right.0))
        });
        let fields: Vec<String> = sorted.iter().map(|(field, _, _)| field.clone()).collect();
        let candidates = if sorted.len() == 1 {
            sorted[0].2.clone()
        } else {
            intersect_sorted(sorted.iter().map(|(_, _, ids)| ids.as_slice()))
        };
        let access_path = if fields.len() == 1 {
            AccessPath::PropertyIndex {
                field: fields[0].clone(),
            }
        } else {
            AccessPath::PropertyIndexIntersection { fields }
        };
        return NodeAccessPlan {
            estimated_rows: candidates.len(),
            access_path,
            candidates,
        };
    }

    NodeAccessPlan {
        access_path: AccessPath::FullNodeScan,
        estimated_rows: mt.node_count(),
        candidates: mt.all_node_ids(),
    }
}

pub fn plan_match<T: VectorType>(
    pattern: &TqlPattern,
    mt: &MemTable<T>,
    optional: bool,
) -> MatchPlan {
    if optional {
        let mut forward = if let Some(filter) = &pattern.nodes[0].filter {
            plan_filter(filter, mt)
        } else {
            NodeAccessPlan {
                access_path: AccessPath::FullNodeScan,
                estimated_rows: mt.node_count(),
                candidates: Vec::new(),
            }
        };
        materialize_full_scan(&mut forward, mt);
        return MatchPlan {
            access_path: forward.access_path,
            estimated_rows: forward.estimated_rows,
            reversed: false,
            pattern: pattern.clone(),
            candidates: forward.candidates,
        };
    }

    let mut forward = plan_pattern_start(pattern, mt);
    let can_reverse_improve = pattern
        .nodes
        .last()
        .is_some_and(|node| node.filter.is_some());
    if pattern.edges.is_empty()
        || pattern.edges.iter().any(|edge| edge.hop_range.is_some())
        || !can_reverse_improve
    {
        materialize_full_scan(&mut forward, mt);
        return MatchPlan {
            access_path: forward.access_path,
            estimated_rows: forward.estimated_rows,
            reversed: false,
            pattern: pattern.clone(),
            candidates: forward.candidates,
        };
    }

    let reversed_pattern = reverse_pattern(pattern);
    let mut backward = plan_pattern_start(&reversed_pattern, mt);
    let reverse_is_better = backward.estimated_rows < forward.estimated_rows
        || (backward.estimated_rows == forward.estimated_rows
            && access_path_key(&backward.access_path) < access_path_key(&forward.access_path));
    if reverse_is_better {
        materialize_full_scan(&mut backward, mt);
        MatchPlan {
            access_path: backward.access_path,
            estimated_rows: backward.estimated_rows,
            reversed: true,
            pattern: reversed_pattern,
            candidates: backward.candidates,
        }
    } else {
        materialize_full_scan(&mut forward, mt);
        MatchPlan {
            access_path: forward.access_path,
            estimated_rows: forward.estimated_rows,
            reversed: false,
            pattern: pattern.clone(),
            candidates: forward.candidates,
        }
    }
}

fn plan_pattern_start<T: VectorType>(pattern: &TqlPattern, mt: &MemTable<T>) -> NodeAccessPlan {
    let node = &pattern.nodes[0];
    if let Some(filter) = &node.filter {
        if let Some(id) = extract_id(filter) {
            return NodeAccessPlan {
                access_path: AccessPath::PrimaryKey { id },
                estimated_rows: usize::from(mt.contains(id)),
                candidates: if mt.contains(id) {
                    vec![id]
                } else {
                    Vec::new()
                },
            };
        }
        return plan_filter(filter, mt);
    }
    if let Some(edge) = pattern.edges.first()
        && !edge.labels.is_empty()
    {
        let candidates = label_candidates(edge, mt);
        return NodeAccessPlan {
            access_path: AccessPath::EdgeLabelIndex {
                labels: edge.labels.clone(),
            },
            estimated_rows: candidates.len(),
            candidates,
        };
    }
    NodeAccessPlan {
        access_path: AccessPath::FullNodeScan,
        estimated_rows: mt.node_count(),
        candidates: Vec::new(),
    }
}

fn materialize_full_scan<T: VectorType>(plan: &mut NodeAccessPlan, mt: &MemTable<T>) {
    if matches!(plan.access_path, AccessPath::FullNodeScan) && plan.candidates.is_empty() {
        plan.candidates = mt.all_node_ids();
    }
}

fn ordered_range(filter: &Filter) -> Option<(&str, Ordering, bool, serde_json::Value)> {
    fn exact_ordered_bound(value: serde_json::Value) -> Option<serde_json::Value> {
        const MAX_EXACT_INTEGER: u64 = 1u64 << 53;
        let precise = value
            .as_i64()
            .map(|number| number.unsigned_abs() < MAX_EXACT_INTEGER)
            .or_else(|| value.as_u64().map(|number| number < MAX_EXACT_INTEGER))
            .unwrap_or(true);
        precise.then_some(value)
    }

    match filter {
        Filter::Gt(field, value) => {
            Some((field, Ordering::Greater, false, serde_json::json!(value)))
        }
        Filter::Gte(field, value) => {
            Some((field, Ordering::Greater, true, serde_json::json!(value)))
        }
        Filter::Lt(field, value) => Some((field, Ordering::Less, false, serde_json::json!(value))),
        Filter::Lte(field, value) => Some((field, Ordering::Less, true, serde_json::json!(value))),
        Filter::Range(field, op, value) => {
            let value = match value {
                crate::filter::ComparableValue::Number(value) => {
                    exact_ordered_bound(serde_json::Value::Number(value.clone()))?
                }
                crate::filter::ComparableValue::String(value) => serde_json::json!(value),
            };
            let (ordering, inclusive) = match op {
                crate::filter::RangeOp::Gt => (Ordering::Greater, false),
                crate::filter::RangeOp::Gte => (Ordering::Greater, true),
                crate::filter::RangeOp::Lt => (Ordering::Less, false),
                crate::filter::RangeOp::Lte => (Ordering::Less, true),
            };
            Some((field, ordering, inclusive, value))
        }
        Filter::And(filters) => filters.iter().find_map(ordered_range),
        _ => None,
    }
}

fn bitmap_filter_candidates<T: VectorType>(
    filter: &Filter,
    mt: &MemTable<T>,
) -> Option<(Vec<String>, Vec<NodeId>)> {
    fn evaluate<T: VectorType>(
        filter: &Filter,
        mt: &MemTable<T>,
        universe: &[NodeId],
    ) -> Option<(HashSet<String>, Vec<NodeId>)> {
        match filter {
            Filter::Eq(field, value) if field != "id" => Some((
                HashSet::from([field.clone()]),
                mt.find_by_bitmap_property_index(field, value)?,
            )),
            Filter::In(field, values) => {
                let mut union = Vec::new();
                for value in values {
                    union.extend(mt.find_by_bitmap_property_index(field, value)?);
                }
                union.sort_unstable();
                union.dedup();
                Some((HashSet::from([field.clone()]), union))
            }
            Filter::Ne(field, value) => {
                let excluded = mt.find_by_bitmap_property_index(field, value)?;
                Some((
                    HashSet::from([field.clone()]),
                    difference_sorted(universe, &excluded),
                ))
            }
            Filter::Nin(field, values) => {
                let mut excluded = Vec::new();
                for value in values {
                    excluded.extend(mt.find_by_bitmap_property_index(field, value)?);
                }
                excluded.sort_unstable();
                excluded.dedup();
                Some((
                    HashSet::from([field.clone()]),
                    difference_sorted(universe, &excluded),
                ))
            }
            Filter::And(filters) => {
                let mut evaluated = filters.iter().map(|filter| evaluate(filter, mt, universe));
                let (mut fields, mut result) = evaluated.next()??;
                for candidate in evaluated {
                    let (candidate_fields, candidate) = candidate?;
                    fields.extend(candidate_fields);
                    result =
                        intersect_sorted([result.as_slice(), candidate.as_slice()].into_iter());
                }
                Some((fields, result))
            }
            Filter::Or(filters) => {
                let mut fields = HashSet::new();
                let mut union = Vec::new();
                for filter in filters {
                    let (candidate_fields, candidate) = evaluate(filter, mt, universe)?;
                    fields.extend(candidate_fields);
                    union.extend(candidate);
                }
                union.sort_unstable();
                union.dedup();
                Some((fields, union))
            }
            _ => None,
        }
    }

    let universe = mt.all_node_ids();
    let (fields, candidates) = evaluate(filter, mt, &universe)?;
    let mut fields = fields.into_iter().collect::<Vec<_>>();
    fields.sort();
    Some((fields, candidates))
}

fn difference_sorted(left: &[NodeId], right: &[NodeId]) -> Vec<NodeId> {
    let mut output = Vec::with_capacity(left.len());
    let (mut left_index, mut right_index) = (0usize, 0usize);
    while left_index < left.len() {
        while right_index < right.len() && right[right_index] < left[left_index] {
            right_index += 1;
        }
        if right_index == right.len() || right[right_index] != left[left_index] {
            output.push(left[left_index]);
        }
        left_index += 1;
    }
    output
}

fn filter_equalities(filter: &Filter) -> Vec<(String, serde_json::Value)> {
    match filter {
        Filter::Eq(field, value) if field != "id" => vec![(field.clone(), value.clone())],
        Filter::And(filters) => filters.iter().flat_map(filter_equalities).collect(),
        _ => Vec::new(),
    }
}

fn indexed_equalities<T: VectorType>(
    filter: &Filter,
    mt: &MemTable<T>,
) -> Vec<(String, serde_json::Value, Vec<NodeId>)> {
    match filter {
        Filter::Eq(field, value) if field != "id" => mt
            .find_by_property_index(field, value)
            .or_else(|| mt.find_by_bitmap_property_index(field, value))
            .map(|ids| vec![(field.clone(), value.clone(), ids.to_vec())])
            .unwrap_or_default(),
        Filter::And(filters) => filters
            .iter()
            .filter_map(|filter| {
                if let Filter::Eq(field, value) = filter
                    && field != "id"
                {
                    return mt
                        .find_by_property_index(field, value)
                        .or_else(|| mt.find_by_bitmap_property_index(field, value))
                        .map(|ids| (field.clone(), value.clone(), ids));
                }
                None
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn intersect_sorted<'a>(sets: impl Iterator<Item = &'a [NodeId]>) -> Vec<NodeId> {
    let mut sets = sets;
    let Some(first) = sets.next() else {
        return Vec::new();
    };
    let mut result = first.to_vec();
    for set in sets {
        let mut intersection = Vec::with_capacity(result.len().min(set.len()));
        let (mut left, mut right) = (0usize, 0usize);
        while left < result.len() && right < set.len() {
            match result[left].cmp(&set[right]) {
                std::cmp::Ordering::Less => left += 1,
                std::cmp::Ordering::Greater => right += 1,
                std::cmp::Ordering::Equal => {
                    intersection.push(result[left]);
                    left += 1;
                    right += 1;
                }
            }
        }
        result = intersection;
        if result.is_empty() {
            break;
        }
    }
    result
}

fn label_candidates<T: VectorType>(edge: &TqlEdgePattern, mt: &MemTable<T>) -> Vec<NodeId> {
    let mut candidates = HashSet::new();
    for label in &edge.labels {
        for &(source, target) in mt.get_edges_by_label(label) {
            match edge.direction {
                EdgeDirection::Forward => {
                    candidates.insert(source);
                }
                EdgeDirection::Backward => {
                    candidates.insert(target);
                }
                EdgeDirection::Both => {
                    candidates.insert(source);
                    candidates.insert(target);
                }
            }
        }
    }
    let mut candidates: Vec<_> = candidates.into_iter().collect();
    candidates.sort_unstable();
    candidates
}

fn reverse_pattern(pattern: &TqlPattern) -> TqlPattern {
    TqlPattern {
        nodes: pattern.nodes.iter().cloned().rev().collect(),
        edges: pattern
            .edges
            .iter()
            .cloned()
            .rev()
            .map(|mut edge| {
                edge.direction = match edge.direction {
                    EdgeDirection::Forward => EdgeDirection::Backward,
                    EdgeDirection::Backward => EdgeDirection::Forward,
                    EdgeDirection::Both => EdgeDirection::Both,
                };
                edge
            })
            .collect(),
    }
}

fn access_path_key(path: &AccessPath) -> (u8, String) {
    match path {
        AccessPath::PrimaryKey { id } => (0, id.to_string()),
        AccessPath::PropertyIndex { field } => (1, field.clone()),
        AccessPath::OrderedPropertyIndex { field, descending } => {
            (2, format!("{field}:{descending}"))
        }
        AccessPath::CompositePropertyIndex { fields } => (3, fields.join("\0")),
        AccessPath::BitmapPropertyIndex { fields } => (4, fields.join("\0")),
        AccessPath::PropertyIndexIntersection { fields } => (5, fields.join("\0")),
        AccessPath::EdgeLabelIndex { labels } => (6, labels.join("\0")),
        AccessPath::FullNodeScan => (7, String::new()),
    }
}

fn extract_id(filter: &Filter) -> Option<NodeId> {
    match filter {
        Filter::Eq(field, value) if field == "id" => value.as_u64(),
        Filter::And(filters) => filters.iter().find_map(extract_id),
        _ => None,
    }
}

pub fn describe_node_pattern(node: &TqlNodePattern) -> String {
    node.var.clone().unwrap_or_else(|| "_".to_owned())
}
