use super::{AccessMode, BatchSearchConfig, Config, Database, SearchConfig};
use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::graph::reachability::{
    ReachabilityConfig, ReachabilityOutput, ReachabilityResult, SubgraphResult,
};
use crate::hook::HookContext;
use crate::node::{Edge, GroupedSearchResult, IncomingEdge, NodeId, NodeView, SearchHit};
use crate::query::tql_executor::TqlResult;
use crate::storage::snapshot::GenerationManifest;
use std::ops::{Deref, DerefMut};

pub struct DatabaseReader<T: VectorType> {
    inner: Database<T>,
}

impl<T: VectorType + serde::Serialize + serde::de::DeserializeOwned> DatabaseReader<T> {
    pub fn open_read_only(path: &str, dim: usize) -> Result<Self> {
        Database::open_read_only(path, dim).map(|inner| Self { inner })
    }

    pub fn open_immutable(path: &str, dim: usize) -> Result<Self> {
        Database::open_immutable(path, dim).map(|inner| Self { inner })
    }

    pub fn open_with_config(path: &str, config: Config) -> Result<Self> {
        if config.access_mode == AccessMode::ReadWrite {
            return Err(TriviumError::InvalidInput(
                "DatabaseReader 只接受 ReadOnly 或 Immutable 配置".into(),
            ));
        }
        Database::open_with_config(path, config).map(|inner| Self { inner })
    }

    pub fn search(
        &self,
        query_vector: &[T],
        top_k: usize,
        expand_depth: usize,
        min_score: f32,
    ) -> Result<Vec<SearchHit>> {
        self.inner
            .search(query_vector, top_k, expand_depth, min_score)
    }

    pub fn search_batch(
        &self,
        query_vectors: &[Vec<T>],
        search_config: &SearchConfig,
        batch_config: &BatchSearchConfig,
    ) -> Result<Vec<Vec<SearchHit>>> {
        self.inner
            .search_batch(query_vectors, search_config, batch_config)
    }

    pub fn search_exact(&self, query_vector: &[T], top_k: usize) -> Result<Vec<SearchHit>> {
        self.inner.search_exact(query_vector, top_k)
    }

    pub fn search_advanced(
        &self,
        query_vector: &[T],
        config: &SearchConfig,
    ) -> Result<Vec<SearchHit>> {
        self.inner.search_advanced(query_vector, config)
    }

    pub fn search_hybrid(
        &self,
        query_text: Option<&str>,
        query_vector: Option<&[T]>,
        config: &SearchConfig,
    ) -> Result<Vec<SearchHit>> {
        self.inner.search_hybrid(query_text, query_vector, config)
    }

    pub fn search_hybrid_with_context(
        &self,
        query_text: Option<&str>,
        query_vector: Option<&[T]>,
        config: &SearchConfig,
    ) -> Result<(Vec<SearchHit>, HookContext)> {
        self.inner
            .search_hybrid_with_context(query_text, query_vector, config)
    }

    pub fn search_hybrid_grouped(
        &self,
        query_text: Option<&str>,
        query_vector: Option<&[T]>,
        config: &SearchConfig,
    ) -> Result<GroupedSearchResult> {
        self.inner
            .search_hybrid_grouped(query_text, query_vector, config)
    }

    pub fn search_graph_first(
        &self,
        query: &[T],
        anchor_ids: &[NodeId],
        top_k: usize,
        max_anchor_nodes: usize,
    ) -> Result<Vec<SearchHit>> {
        self.inner
            .search_graph_first(query, anchor_ids, top_k, max_anchor_nodes)
    }

    pub fn get(&self, id: NodeId) -> Option<NodeView<T>> {
        self.inner.get(id)
    }

    pub fn get_payload(&self, id: NodeId) -> Option<serde_json::Value> {
        self.inner.get_payload(id)
    }

    pub fn get_edges(&self, id: NodeId) -> Vec<Edge> {
        self.inner.get_edges(id)
    }

    pub fn get_edge(&self, src: NodeId, dst: NodeId, label: &str) -> Option<Edge> {
        self.inner.get_edge(src, dst, label)
    }

    pub fn get_incoming_edges(&self, id: NodeId, label: Option<&str>) -> Vec<IncomingEdge> {
        self.inner.get_incoming_edges(id, label)
    }

    pub fn neighbors(&self, id: NodeId, depth: usize) -> Vec<NodeId> {
        self.inner.neighbors(id, depth)
    }

    pub fn neighbors_with_labels(
        &self,
        id: NodeId,
        depth: usize,
        labels: Option<&[String]>,
    ) -> Vec<NodeId> {
        self.inner.neighbors_with_labels(id, depth, labels)
    }

    pub fn reachable(
        &self,
        id: NodeId,
        config: &ReachabilityConfig,
    ) -> Result<Vec<ReachabilityResult>> {
        self.inner.reachable(id, config)
    }

    pub fn reachable_detailed(
        &self,
        id: NodeId,
        config: &ReachabilityConfig,
    ) -> Result<ReachabilityOutput> {
        self.inner.reachable_detailed(id, config)
    }

    pub fn query_subgraph(
        &self,
        id: NodeId,
        config: &ReachabilityConfig,
    ) -> Result<SubgraphResult> {
        self.inner.query_subgraph(id, config)
    }

    pub fn tql(&self, input: &str) -> Result<TqlResult<T>> {
        self.inner.tql(input)
    }

    pub fn node_count(&self) -> usize {
        self.inner.node_count()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.inner.contains(id)
    }

    pub fn dim(&self) -> usize {
        self.inner.dim()
    }

    pub fn all_node_ids(&self) -> Vec<NodeId> {
        self.inner.all_node_ids()
    }

    pub fn estimated_memory(&self) -> usize {
        self.inner.estimated_memory()
    }

    pub fn close(&mut self) -> Result<()> {
        self.inner.close()
    }
}

pub struct DatabaseWriter<T: VectorType> {
    inner: Database<T>,
}

impl<T: VectorType + serde::Serialize + serde::de::DeserializeOwned> DatabaseWriter<T> {
    pub fn open(path: &str, dim: usize) -> Result<Self> {
        Database::open(path, dim).map(|inner| Self { inner })
    }

    pub fn open_with_config(path: &str, config: Config) -> Result<Self> {
        if config.access_mode != AccessMode::ReadWrite {
            return Err(TriviumError::InvalidInput(
                "DatabaseWriter 只接受 ReadWrite 配置".into(),
            ));
        }
        Database::open_with_config(path, config).map(|inner| Self { inner })
    }

    pub fn publish_generation_manifest(
        &mut self,
        generation_id: &str,
    ) -> Result<GenerationManifest> {
        self.inner.publish_generation_manifest(generation_id)
    }
}

impl<T: VectorType> Deref for DatabaseWriter<T> {
    type Target = Database<T>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: VectorType> DerefMut for DatabaseWriter<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
