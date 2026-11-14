//! 图算法的只读拓扑视图，事实节点和资源始终借用同一存储内核。

use crate::named_graph::{NamedGraphLimits, NamedGraphScope};
use crate::node::{Edge, IncomingEdge, NodeId};
use crate::storage::memtable::MemTable;
use crate::{Result, TriviumError, VectorType};
use std::collections::BTreeMap;
use std::sync::Arc;

pub(crate) trait GraphRead<T: VectorType>: Sync {
    fn facts(&self) -> &MemTable<T>;
    fn get_edges(&self, id: NodeId) -> Option<&[Edge]>;
    fn get_incoming_sources(&self, id: NodeId) -> &[NodeId];
    fn all_node_ids(&self) -> Vec<NodeId>;
    fn contains(&self, id: NodeId) -> bool;
    fn internal_slot_count(&self) -> usize;
    fn internal_slot_of(&self, id: NodeId) -> Option<usize>;
    fn active_id_at_slot(&self, slot: usize) -> Option<NodeId>;

    fn node_count(&self) -> usize {
        self.internal_slot_count()
    }
    fn ensure_readable(&self) -> Result<()> {
        self.facts().ensure_readable()
    }
    fn resource_runtime(&self) -> &crate::ResourceRuntime {
        self.facts().resource_runtime()
    }
    fn get_in_degree(&self, id: NodeId) -> usize {
        self.get_incoming_sources(id)
            .iter()
            .map(|source| {
                self.get_edges(*source)
                    .unwrap_or_default()
                    .iter()
                    .filter(|edge| edge.target_id == id)
                    .count()
            })
            .sum()
    }
    fn get_incoming_edges(&self, id: NodeId, label: Option<&str>) -> Vec<IncomingEdge> {
        collect_incoming(self, id, label)
    }
    fn edge_graph_id(&self, _source: NodeId, _edge: &Edge) -> Option<&str> {
        None
    }
}

fn collect_incoming<T: VectorType, G: GraphRead<T> + ?Sized>(
    graph: &G,
    id: NodeId,
    label: Option<&str>,
) -> Vec<IncomingEdge> {
    let mut edges: Vec<_> = graph
        .get_incoming_sources(id)
        .iter()
        .flat_map(|source| {
            graph
                .get_edges(*source)
                .unwrap_or_default()
                .iter()
                .filter(move |edge| {
                    edge.target_id == id && label.is_none_or(|label| edge.label == label)
                })
                .map(move |edge| IncomingEdge {
                    source_id: *source,
                    target_id: id,
                    label: edge.label.clone(),
                    weight: edge.weight,
                    metadata: edge.metadata.clone(),
                })
        })
        .collect();
    edges.sort_by(|a, b| a.source_id.cmp(&b.source_id).then(a.label.cmp(&b.label)));
    edges
}

impl<T: VectorType> GraphRead<T> for MemTable<T> {
    fn facts(&self) -> &MemTable<T> {
        self
    }
    fn get_edges(&self, id: NodeId) -> Option<&[Edge]> {
        self.get_edges(id)
    }
    fn get_incoming_sources(&self, id: NodeId) -> &[NodeId] {
        self.get_incoming_sources(id)
    }
    fn all_node_ids(&self) -> Vec<NodeId> {
        self.all_node_ids()
    }
    fn contains(&self, id: NodeId) -> bool {
        self.contains(id)
    }
    fn internal_slot_count(&self) -> usize {
        self.internal_slot_count()
    }
    fn internal_slot_of(&self, id: NodeId) -> Option<usize> {
        self.internal_slot_of(id)
    }
    fn active_id_at_slot(&self, slot: usize) -> Option<NodeId> {
        self.active_id_at_slot(slot)
    }
    fn node_count(&self) -> usize {
        self.node_count()
    }
    fn get_in_degree(&self, id: NodeId) -> usize {
        self.get_in_degree(id)
    }
    fn get_incoming_edges(&self, id: NodeId, label: Option<&str>) -> Vec<IncomingEdge> {
        self.get_incoming_edges(id, label)
    }
}

/// 请求内拥有的所选拓扑，持有预算凭证直至最后一个执行者退出。
pub(crate) struct ScopedGraph {
    pub stats: crate::storage::memtable::GraphStats,
    pub revisions: BTreeMap<String, String>,
    ids: Vec<NodeId>,
    outgoing: BTreeMap<NodeId, Vec<Edge>>,
    identities: BTreeMap<NodeId, Vec<Option<Arc<str>>>>,
    incoming: BTreeMap<NodeId, Vec<NodeId>>,
    in_degrees: BTreeMap<NodeId, usize>,
    _workspace: crate::resources::WorkspaceReservation,
}

impl ScopedGraph {
    pub(crate) fn reserved_bytes(&self) -> usize {
        self._workspace.bytes()
    }
    pub(crate) fn build<T: VectorType>(
        mt: &MemTable<T>,
        scope: &NamedGraphScope,
        memory_limit: usize,
    ) -> Result<Arc<Self>> {
        let limits = NamedGraphLimits::default();
        let selected = scope.selected(&limits)?;
        let mut bytes = 4096usize;
        let mut members = std::collections::BTreeSet::new();
        let mut workspace = mt.resource_runtime().reserve_workspace(bytes)?;
        let mut add_member = |id| -> Result<()> {
            crate::request::check_current()?;
            if members.contains(&id) {
                return Ok(());
            }
            crate::named_graph::bound(
                "named_graph_nodes",
                members.len().saturating_add(1),
                limits.max_nodes,
            )?;
            let peak =
                4096usize.saturating_add(members.len().saturating_add(1).saturating_mul(256));
            mt.ensure_additional_memory(memory_limit, peak)?;
            workspace.resize(peak)?;
            members.insert(id);
            Ok(())
        };
        let mut edges = 0usize;
        for id in &selected {
            crate::request::check_current()?;
            let record =
                mt.named_graphs
                    .graphs
                    .get(*id)
                    .ok_or_else(|| TriviumError::GraphNotFound {
                        graph_id: (*id).to_owned(),
                    })?;
            if let Some(expected) = scope.expected_revisions.get(*id) {
                mt.named_graphs.check(id, Some(expected))?;
            }
            for member in &record.graph.members {
                add_member(*member)?;
            }
            edges = edges.saturating_add(record.graph.edges.len());
            bytes = bytes.saturating_add(record.graph.memory_bytes().saturating_mul(4));
        }
        if scope.include_default {
            for id in mt.active_node_ids() {
                crate::request::check_current()?;
                add_member(id)?;
                for edge in mt.get_edges(id).unwrap_or_default() {
                    edges = edges.saturating_add(1);
                    bytes = bytes.saturating_add(
                        edge.label
                            .len()
                            .saturating_add(crate::storage::payload_store::estimate_json_memory(
                                &edge.metadata,
                            ))
                            .saturating_mul(4),
                    );
                }
            }
        }
        let nodes = members.len();
        crate::named_graph::bound("named_graph_edges", edges, limits.max_edges)?;
        bytes = bytes
            .saturating_add(nodes.saturating_mul(256))
            .saturating_add(edges.saturating_mul(512));
        mt.ensure_additional_memory(memory_limit, bytes)?;
        workspace.resize(bytes)?;
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::NamedGraphRead);
        crate::request::check_current()?;
        let mut view = Self {
            stats: crate::storage::memtable::GraphStats {
                node_count: 0,
                edge_count: 0,
                isolated_node_count: 0,
                label_count: 0,
                avg_out_degree: 0.,
                avg_in_degree: 0.,
                max_out_degree: 0,
                max_in_degree: 0,
                label_stats: BTreeMap::new(),
                out_degree_histogram: Vec::new(),
                in_degree_histogram: Vec::new(),
            },
            revisions: BTreeMap::new(),
            ids: members.into_iter().collect(),
            outgoing: BTreeMap::new(),
            identities: BTreeMap::new(),
            incoming: BTreeMap::new(),
            in_degrees: BTreeMap::new(),
            _workspace: workspace,
        };
        for id in selected {
            let record = &mt.named_graphs.graphs[id];
            view.revisions
                .insert(id.to_owned(), record.revision.clone());
            let identity: Arc<str> = Arc::from(id);
            for edge in &record.graph.edges {
                crate::request::check_current()?;
                view.insert(
                    edge.source_id,
                    Edge {
                        target_id: edge.target_id,
                        label: edge.label.clone(),
                        weight: edge.weight,
                        metadata: edge.metadata.clone(),
                    },
                    Some(identity.clone()),
                );
            }
        }
        if scope.include_default {
            for id in mt.active_node_ids() {
                crate::request::check_current()?;
                for edge in mt.get_edges(id).unwrap_or_default() {
                    view.insert(id, edge.clone(), None);
                }
            }
        }
        for sources in view.incoming.values_mut() {
            sources.sort_unstable();
            sources.dedup();
        }
        view.stats = view.statistics()?;
        Ok(Arc::new(view))
    }
    fn insert(&mut self, source: NodeId, edge: Edge, graph: Option<Arc<str>>) {
        *self.in_degrees.entry(edge.target_id).or_default() += 1;
        self.incoming
            .entry(edge.target_id)
            .or_default()
            .push(source);
        self.outgoing.entry(source).or_default().push(edge);
        self.identities.entry(source).or_default().push(graph);
    }

    fn statistics(&self) -> Result<crate::storage::memtable::GraphStats> {
        use crate::storage::memtable::{DegreeBucket, GraphStats, LabelStats};
        let mut labels = BTreeMap::<
            &str,
            (
                usize,
                std::collections::BTreeSet<NodeId>,
                std::collections::BTreeSet<NodeId>,
            ),
        >::new();
        let mut out_hist = BTreeMap::<usize, usize>::new();
        let mut in_hist = BTreeMap::<usize, usize>::new();
        let mut isolated = 0;
        let mut edges = 0;
        for id in &self.ids {
            crate::request::check_current()?;
            let outgoing = self.outgoing.get(id).map_or(&[][..], Vec::as_slice);
            let incoming = self.in_degrees.get(id).copied().unwrap_or(0);
            *out_hist.entry(outgoing.len()).or_default() += 1;
            *in_hist.entry(incoming).or_default() += 1;
            isolated += usize::from(outgoing.is_empty() && incoming == 0);
            edges += outgoing.len();
            for edge in outgoing {
                crate::request::check_current()?;
                let counts = labels.entry(&edge.label).or_default();
                counts.0 += 1;
                counts.1.insert(*id);
                counts.2.insert(edge.target_id);
            }
        }
        let degree = edges as f64 / self.ids.len().max(1) as f64;
        Ok(GraphStats {
            node_count: self.ids.len(),
            edge_count: edges,
            isolated_node_count: isolated,
            label_count: labels.len(),
            avg_out_degree: degree,
            avg_in_degree: degree,
            max_out_degree: out_hist.keys().next_back().copied().unwrap_or(0),
            max_in_degree: in_hist.keys().next_back().copied().unwrap_or(0),
            label_stats: labels
                .into_iter()
                .map(|(label, (edge_count, sources, targets))| {
                    (
                        label.to_owned(),
                        LabelStats {
                            edge_count,
                            distinct_source_count: sources.len(),
                            distinct_target_count: targets.len(),
                        },
                    )
                })
                .collect(),
            out_degree_histogram: out_hist
                .into_iter()
                .map(|(upper_bound, node_count)| DegreeBucket {
                    upper_bound,
                    node_count,
                })
                .collect(),
            in_degree_histogram: in_hist
                .into_iter()
                .map(|(upper_bound, node_count)| DegreeBucket {
                    upper_bound,
                    node_count,
                })
                .collect(),
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct GraphView<'a, T: VectorType> {
    pub facts: &'a MemTable<T>,
    pub scoped: Option<&'a ScopedGraph>,
}

impl<T: VectorType> GraphRead<T> for GraphView<'_, T> {
    fn facts(&self) -> &MemTable<T> {
        self.facts
    }
    fn get_edges(&self, id: NodeId) -> Option<&[Edge]> {
        match self.scoped {
            Some(view) => view.outgoing.get(&id).map(Vec::as_slice),
            None => self.facts.get_edges(id),
        }
    }
    fn get_incoming_sources(&self, id: NodeId) -> &[NodeId] {
        match self.scoped {
            Some(view) => view.incoming.get(&id).map_or(&[], Vec::as_slice),
            None => self.facts.get_incoming_sources(id),
        }
    }
    fn all_node_ids(&self) -> Vec<NodeId> {
        match self.scoped {
            Some(view) => view.ids.clone(),
            None => self.facts.all_node_ids(),
        }
    }
    fn contains(&self, id: NodeId) -> bool {
        match self.scoped {
            Some(view) => view.ids.binary_search(&id).is_ok(),
            None => self.facts.contains(id),
        }
    }
    fn internal_slot_count(&self) -> usize {
        self.scoped
            .map_or_else(|| self.facts.internal_slot_count(), |view| view.ids.len())
    }
    fn internal_slot_of(&self, id: NodeId) -> Option<usize> {
        match self.scoped {
            Some(view) => view.ids.binary_search(&id).ok(),
            None => self.facts.internal_slot_of(id),
        }
    }
    fn active_id_at_slot(&self, slot: usize) -> Option<NodeId> {
        match self.scoped {
            Some(view) => view.ids.get(slot).copied(),
            None => self.facts.active_id_at_slot(slot),
        }
    }
    fn node_count(&self) -> usize {
        self.scoped
            .map_or_else(|| self.facts.node_count(), |view| view.ids.len())
    }
    fn get_in_degree(&self, id: NodeId) -> usize {
        self.scoped.map_or_else(
            || self.facts.get_in_degree(id),
            |view| view.in_degrees.get(&id).copied().unwrap_or(0),
        )
    }
    fn get_incoming_edges(&self, id: NodeId, label: Option<&str>) -> Vec<IncomingEdge> {
        if self.scoped.is_none() {
            self.facts.get_incoming_edges(id, label)
        } else {
            collect_incoming(self, id, label)
        }
    }
    fn edge_graph_id(&self, source: NodeId, edge: &Edge) -> Option<&str> {
        let view = self.scoped?;
        let edges = view.outgoing.get(&source)?;
        // 只读视图中的边数组保持原位，通过地址差取得槽位，再校验引用身份。
        // 该过程只计算整数并安全索引，避免每条边重复扫描整个出边数组。
        let offset = (edge as *const Edge)
            .addr()
            .checked_sub(edges.as_ptr().addr())?;
        let position = offset / std::mem::size_of::<Edge>();
        if !std::ptr::eq(edges.get(position)?, edge) {
            return None;
        }
        view.identities.get(&source)?.get(position)?.as_deref()
    }
}
