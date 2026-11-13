//! 命名图通过普通事务提交，读取在同一事实视图内选择显式图作用域。

use super::{Database, read_or_recover, reject_hook_reentrant_access};
use crate::database::transaction::TxOp;
use crate::named_graph::*;
use crate::{NodeId, Result, TriviumError, VectorType};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

fn check_result(value: &impl Serialize, limits: &NamedGraphLimits) -> Result<()> {
    bound(
        "named_graph_return_bytes",
        crate::storage::payload_store::serialized_json_len(value)?,
        limits.max_return_bytes,
    )
}

/// 借用输出字段计量JSON，复制边内容前核对完整响应额度。
#[derive(Serialize)]
struct EdgeEncoding<'a> {
    graph_id: Option<&'a str>,
    #[serde(with = "crate::document::decimal_id")]
    source_id: NodeId,
    #[serde(with = "crate::document::decimal_id")]
    target_id: NodeId,
    label: &'a str,
    weight: f32,
    metadata: &'a serde_json::Value,
}

fn edge_bytes(
    graph_id: Option<&str>,
    source_id: NodeId,
    target_id: NodeId,
    label: &str,
    weight: f32,
    metadata: &serde_json::Value,
) -> Result<usize> {
    crate::storage::payload_store::serialized_json_len(&EdgeEncoding {
        graph_id,
        source_id,
        target_id,
        label,
        weight,
        metadata,
    })
}

impl<T: VectorType + Serialize + DeserializeOwned> Database<T> {
    /// None创建新图，已有图通过预期修订原子替换。
    pub fn put_named_graph(
        &mut self,
        graph: NamedGraphWrite,
        expected_revision: Option<&str>,
    ) -> Result<NamedGraphSnapshot> {
        Self::write_operation(|_| {
            let _operation = self.enter_operation()?;
            self.check_transaction_admission()?;
            graph.validate_input()?;
            let bytes = graph.memory_bytes().saturating_mul(8).saturating_add(
                crate::storage::payload_store::serialized_json_len(&graph)?.saturating_mul(4),
            );
            self.ensure_incremental_memory_budget(bytes)?;
            let _workspace = self.resource_runtime.reserve_workspace(bytes)?;
            let snapshot = NamedGraphSnapshot::from_write(graph, expected_revision)?;
            self.commit_ops(vec![TxOp::NamedGraph {
                graph_id: snapshot.graph.graph_id.clone(),
                expected_revision: expected_revision.map(str::to_owned),
                snapshot: Some(snapshot.clone()),
            }])?;
            Ok(snapshot)
        })
    }

    /// 同一批成员与边编辑共享一次修订和一次事务提交。
    pub fn edit_named_graph(
        &mut self,
        graph_id: &str,
        expected_revision: &str,
        edits: Vec<NamedGraphEdit>,
    ) -> Result<NamedGraphSnapshot> {
        Self::write_operation(|_| {
            let _operation = self.enter_operation()?;
            self.check_transaction_admission()?;
            let limits = NamedGraphLimits::default();
            let mut snapshot = self.get_named_graph(graph_id, &limits)?.ok_or_else(|| {
                TriviumError::GraphNotFound {
                    graph_id: graph_id.into(),
                }
            })?;
            if snapshot.revision != expected_revision {
                return Err(TriviumError::GraphRevisionMismatch {
                    graph_id: graph_id.into(),
                    expected: expected_revision.into(),
                    actual: snapshot.revision,
                }
                .at_field("expected_revision"));
            }
            bound("named_graph_edits", edits.len(), 1_000_000)?;
            for edit in &edits {
                match edit {
                    NamedGraphEdit::SetMetadata { metadata } => {
                        crate::storage::payload_store::json_encoded_len(metadata)
                            .map_err(|error| error.at_field("edits.metadata"))?;
                    }
                    NamedGraphEdit::UpsertEdge { edge } => {
                        crate::storage::payload_store::json_encoded_len(&edge.metadata)
                            .map_err(|error| error.at_field("edits.edge.metadata"))?;
                    }
                    _ => {}
                }
            }
            let bytes = crate::storage::payload_store::serialized_json_len(&edits)?
                .saturating_mul(64)
                .saturating_add(snapshot.graph.memory_bytes().saturating_mul(4));
            self.ensure_incremental_memory_budget(bytes)?;
            let _workspace = self.resource_runtime.reserve_workspace(bytes)?;
            // 有序映射让批量编辑的边键合并保持确定性，删除成员同时清理该图关联边。
            let mut members = snapshot.graph.members.into_iter().collect::<BTreeSet<_>>();
            let mut edges = snapshot
                .graph
                .edges
                .into_iter()
                .map(|edge| ((edge.source_id, edge.target_id, edge.label.clone()), edge))
                .collect::<BTreeMap<_, _>>();
            for edit in edits {
                crate::request::check_current()?;
                match edit {
                    NamedGraphEdit::AddMember { node_id } => {
                        crate::document::valid_id(node_id, "edits.node_id")?;
                        members.insert(node_id);
                    }
                    NamedGraphEdit::RemoveMember { node_id } => {
                        crate::document::valid_id(node_id, "edits.node_id")?;
                        members.remove(&node_id);
                        edges.retain(|(src, dst, _), _| *src != node_id && *dst != node_id);
                    }
                    NamedGraphEdit::UpsertEdge { edge } => {
                        crate::document::valid_id(edge.source_id, "edits.edge.source_id")?;
                        crate::document::valid_id(edge.target_id, "edits.edge.target_id")?;
                        if !edge.weight.is_finite() {
                            return Err(invalid(
                                "edits.edge.weight",
                                "边权重必须有限 (Edge weight must be finite)",
                            ));
                        }
                        edges.insert((edge.source_id, edge.target_id, edge.label.clone()), edge);
                    }
                    NamedGraphEdit::RemoveEdge {
                        source_id,
                        target_id,
                        label,
                    } => {
                        crate::document::valid_id(source_id, "edits.source_id")?;
                        crate::document::valid_id(target_id, "edits.target_id")?;
                        edges.remove(&(source_id, target_id, label));
                    }
                    NamedGraphEdit::SetMetadata { metadata } => {
                        snapshot.graph.metadata = metadata;
                    }
                }
            }
            snapshot.graph.members = members.into_iter().collect();
            snapshot.graph.edges = edges.into_values().collect();
            self.put_named_graph(snapshot.graph, Some(expected_revision))
        })
    }

    /// 删除图的成员关系和独立边，保留全库节点事实。
    pub fn delete_named_graph(&mut self, graph_id: &str, expected_revision: &str) -> Result<()> {
        self.commit_ops(vec![TxOp::NamedGraph {
            graph_id: graph_id.into(),
            expected_revision: Some(expected_revision.into()),
            snapshot: None,
        }])
        .map(|_| ())
    }

    pub fn get_named_graph(
        &self,
        graph_id: &str,
        limits: &NamedGraphLimits,
    ) -> Result<Option<NamedGraphSnapshot>> {
        self.read_operation(|| {
            reject_hook_reentrant_access(&self.memtable)?;
            crate::request::check_current()?;
            validate_id(graph_id)?;
            let mt = read_or_recover(&self.memtable);
            let Some(record) = mt.named_graphs.graphs.get(graph_id) else {
                check_result(&Option::<NamedGraphSnapshot>::None, limits)?;
                return Ok(None);
            };
            bound("named_graph_count", 1, limits.max_graphs)?;
            bound(
                "named_graph_nodes",
                record.graph.members.len(),
                limits.max_nodes,
            )?;
            bound(
                "named_graph_edges",
                record.graph.edges.len(),
                limits.max_edges,
            )?;
            check_result(record.as_ref(), limits)?;
            let bytes = record.graph.memory_bytes().saturating_add(256);
            mt.ensure_additional_memory(self.memory_limit, bytes)?;
            let _workspace = self.resource_runtime.reserve_workspace(bytes)?;
            Ok(Some(record.as_ref().clone()))
        })?
    }

    pub fn list_named_graphs(&self, limits: &NamedGraphLimits) -> Result<Vec<NamedGraphInfo>> {
        self.read_operation(|| {
            reject_hook_reentrant_access(&self.memtable)?;
            crate::request::check_current()?;
            let mt = read_or_recover(&self.memtable);
            bound(
                "named_graph_count",
                mt.named_graphs.graphs.len(),
                limits.max_graphs,
            )?;
            let bytes = mt
                .named_graphs
                .graphs
                .keys()
                .map(|id| id.len().saturating_add(256))
                .sum::<usize>();
            mt.ensure_additional_memory(self.memory_limit, bytes)?;
            let _workspace = self.resource_runtime.reserve_workspace(bytes)?;
            let result = mt
                .named_graphs
                .graphs
                .values()
                .map(|record| NamedGraphInfo {
                    graph_id: record.graph.graph_id.clone(),
                    revision: record.revision.clone(),
                    member_count: record.graph.members.len(),
                    edge_count: record.graph.edges.len(),
                })
                .collect::<Vec<_>>();
            check_result(&result, limits)?;
            Ok(result)
        })?
    }

    /// 显式联合保留每条边的所属图，共享成员在节点集合中只出现一次。
    pub fn named_graph_scope(
        &self,
        scope: &NamedGraphScope,
        limits: &NamedGraphLimits,
    ) -> Result<ScopedGraphSnapshot> {
        self.read_operation(|| {
            reject_hook_reentrant_access(&self.memtable)?;
            crate::request::check_current()?;
            bound(
                "named_graph_count",
                scope
                    .graph_ids
                    .len()
                    .saturating_add(usize::from(scope.include_default)),
                limits.max_graphs,
            )?;
            let scope_bytes =
                crate::storage::payload_store::serialized_json_len(scope)?.saturating_mul(64);
            self.ensure_incremental_memory_budget(scope_bytes)?;
            let _scope_workspace = self.resource_runtime.reserve_workspace(scope_bytes)?;
            let mut selected = BTreeSet::new();
            for id in &scope.graph_ids {
                validate_id(id)?;
                if !selected.insert(id.as_str()) {
                    return Err(invalid(
                        "scope.graph_ids",
                        "图作用域包含重复身份 (Graph scope contains duplicate identities)",
                    ));
                }
            }
            if selected.is_empty() && !scope.include_default {
                return Err(invalid(
                    "scope",
                    "查询要求显式选择图 (Query requires an explicit graph selection)",
                ));
            }
            if scope
                .expected_revisions
                .keys()
                .any(|id| !selected.contains(id.as_str()))
            {
                return Err(invalid(
                    "scope.expected_revisions",
                    "预期修订必须对应所选图 (Expected revisions must refer to selected graphs)",
                ));
            }
            let mt = read_or_recover(&self.memtable);
            let mut nodes = if scope.include_default {
                mt.node_count()
            } else {
                0
            };
            let mut edges = if scope.include_default {
                mt.active_node_ids()
                    .map(|id| mt.get_edges(id).map_or(0, <[_]>::len))
                    .sum::<usize>()
            } else {
                0
            };
            let mut bytes = nodes
                .saturating_mul(128)
                .saturating_add(edges.saturating_mul(256));
            for id in &selected {
                let record =
                    mt.named_graphs
                        .graphs
                        .get(*id)
                        .ok_or_else(|| TriviumError::GraphNotFound {
                            graph_id: (*id).into(),
                        })?;
                if let Some(expected) = scope.expected_revisions.get(*id) {
                    mt.named_graphs.check(id, Some(expected))?;
                }
                nodes = nodes.saturating_add(record.graph.members.len());
                edges = edges.saturating_add(record.graph.edges.len());
                bytes = bytes
                    .saturating_add(record.graph.memory_bytes().saturating_mul(4))
                    .saturating_add(
                        record
                            .graph
                            .edges
                            .len()
                            .saturating_mul(id.len())
                            .saturating_mul(2),
                    );
            }
            // 准入按读取的成员关系与边计数，联合后的去重不会扩大工作预算。
            bound("named_graph_nodes", nodes, limits.max_nodes)?;
            bound("named_graph_edges", edges, limits.max_edges)?;
            if scope.include_default {
                for id in mt.active_node_ids() {
                    for edge in mt.get_edges(id).unwrap_or(&[]) {
                        bytes = bytes.saturating_add(
                            edge.label
                                .len()
                                .saturating_add(
                                    crate::storage::payload_store::estimate_json_memory(
                                        &edge.metadata,
                                    ),
                                )
                                .saturating_mul(4),
                        );
                    }
                }
            }
            mt.ensure_additional_memory(self.memory_limit, bytes)?;
            let _workspace = self.resource_runtime.reserve_workspace(bytes)?;
            let mut members = BTreeSet::new();
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::NamedGraphRead);
            crate::request::check_current()?;
            let mut result = ScopedGraphSnapshot {
                revisions: BTreeMap::new(),
                members: Vec::new(),
                edges: Vec::new(),
            };
            let mut encoded_edges = 0usize;
            for id in &selected {
                let record = &mt.named_graphs.graphs[*id];
                result
                    .revisions
                    .insert((*id).into(), record.revision.clone());
                members.extend(record.graph.members.iter().copied());
                for edge in &record.graph.edges {
                    crate::request::check_current()?;
                    encoded_edges = encoded_edges.saturating_add(edge_bytes(
                        Some(id),
                        edge.source_id,
                        edge.target_id,
                        &edge.label,
                        edge.weight,
                        &edge.metadata,
                    )?);
                }
            }
            if scope.include_default {
                for id in mt.active_node_ids() {
                    crate::request::check_current()?;
                    members.insert(id);
                    for edge in mt.get_edges(id).unwrap_or(&[]) {
                        encoded_edges = encoded_edges.saturating_add(edge_bytes(
                            None,
                            id,
                            edge.target_id,
                            &edge.label,
                            edge.weight,
                            &edge.metadata,
                        )?);
                    }
                }
            }
            let encoded_members = members
                .iter()
                .fold(0usize, |bytes, id| {
                    bytes.saturating_add(id.to_string().len() + 2)
                })
                .saturating_add(members.len().saturating_sub(1));
            // 两个空数组已计入外层编码，只补元素内容与分隔符。
            let response_bytes = crate::storage::payload_store::serialized_json_len(&result)?
                .saturating_add(encoded_members)
                .saturating_add(encoded_edges)
                .saturating_add(edges.saturating_sub(1));
            bound(
                "named_graph_return_bytes",
                response_bytes,
                limits.max_return_bytes,
            )?;
            result.members = members.into_iter().collect();
            for id in selected {
                for edge in &mt.named_graphs.graphs[id].graph.edges {
                    crate::request::check_current()?;
                    result.edges.push(ScopedGraphEdge {
                        graph_id: Some(id.into()),
                        edge: edge.clone(),
                    });
                }
            }
            if scope.include_default {
                for id in mt.active_node_ids() {
                    for edge in mt.get_edges(id).unwrap_or(&[]) {
                        crate::request::check_current()?;
                        result.edges.push(ScopedGraphEdge {
                            graph_id: None,
                            edge: NamedGraphEdge {
                                source_id: id,
                                target_id: edge.target_id,
                                label: edge.label.clone(),
                                weight: edge.weight,
                                metadata: edge.metadata.clone(),
                            },
                        });
                    }
                }
            }
            result.edges.sort_by(|a, b| {
                a.edge
                    .key()
                    .cmp(&b.edge.key())
                    .then_with(|| a.graph_id.cmp(&b.graph_id))
            });
            check_result(&result, limits)?;
            Ok(result)
        })?
    }

    /// 出边广度遍历，跨图路径必须通过显式联合的共享NodeId连接。
    pub fn traverse_named_graph(
        &self,
        scope: &NamedGraphScope,
        start: NodeId,
        max_depth: usize,
        limits: &NamedGraphLimits,
    ) -> Result<ScopedGraphSnapshot> {
        self.read_operation(|| {
            bound("named_graph_depth", max_depth, 1000)?;
            let mut snapshot = self.named_graph_scope(scope, limits)?;
            if snapshot.members.binary_search(&start).is_err() {
                return Err(invalid(
                    "start",
                    "起点必须属于所选图 (Start node must belong to the selected graph)",
                ));
            }
            let bytes = crate::storage::payload_store::serialized_json_len(&snapshot)?
                .saturating_mul(64)
                .saturating_add(snapshot.members.len().saturating_mul(128))
                .saturating_add(snapshot.edges.len().saturating_mul(32));
            self.ensure_incremental_memory_budget(bytes)?;
            let _workspace = self.resource_runtime.reserve_workspace(bytes)?;
            let mut adjacency = BTreeMap::<NodeId, Vec<usize>>::new();
            for (index, edge) in snapshot.edges.iter().enumerate() {
                adjacency
                    .entry(edge.edge.source_id)
                    .or_default()
                    .push(index);
            }
            let mut depths = BTreeMap::from([(start, 0usize)]);
            let mut queue = VecDeque::from([start]);
            while let Some(id) = queue.pop_front() {
                crate::request::check_current()?;
                let depth = depths[&id];
                if depth == max_depth {
                    continue;
                }
                for index in adjacency.get(&id).into_iter().flatten() {
                    let target = snapshot.edges[*index].edge.target_id;
                    if let std::collections::btree_map::Entry::Vacant(entry) = depths.entry(target)
                    {
                        entry.insert(depth + 1);
                        queue.push_back(target);
                    }
                }
            }
            snapshot.members.retain(|id| depths.contains_key(id));
            snapshot.edges.retain(|edge| {
                depths
                    .get(&edge.edge.source_id)
                    .is_some_and(|depth| *depth < max_depth)
            });
            check_result(&snapshot, limits)?;
            Ok(snapshot)
        })?
    }
}
