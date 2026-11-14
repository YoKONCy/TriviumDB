//! 按访问能力分离的类型化数据库门面。
//!
//! DatabaseReader 只暴露查询、统计和关闭能力，且不通过 Deref 泄露底层写 API；
//! DatabaseWriter 保留完整可写接口。该分离在 Rust 编译期表达 ReadOnly/Immutable
//! 零写边界，动态语言则由运行时 AccessMode 和稳定错误码提供同等约束。

use crate::VectorType;
use crate::database::{
    AccessMode, BatchSearchConfig, Config, Database, InstanceStats, SearchConfig, ShutdownStatus,
};
use crate::error::{Result, TriviumError};
use crate::graph::reachability::{
    ReachabilityConfig, ReachabilityOutput, ReachabilityResult, SubgraphResult,
};
use crate::hook::HookContext;
use crate::node::{Edge, GroupedSearchResult, IncomingEdge, NodeId, NodeView, SearchHit};
use crate::query::tql_executor::{TqlResult, TqlValueResult};
use crate::storage::snapshot::GenerationManifest;
use std::ops::{Deref, DerefMut};

pub struct DatabaseReader<T: VectorType> {
    inner: Database<T>,
}

impl<T: VectorType + serde::Serialize + serde::de::DeserializeOwned> DatabaseReader<T> {
    pub fn configure_quiver(
        &self,
        options: crate::index::quiver::ssd::SsdRuntimeOptions,
    ) -> Result<crate::index::quiver::ssd::SsdStats> {
        self.inner.configure_quiver(options)
    }
    pub fn search_quiver(
        &self,
        query: &[f32],
        config: &crate::index::quiver::QuIVerSearchConfig,
    ) -> Result<Vec<(u64, f32)>> {
        self.inner.search_quiver(query, config)
    }
    pub fn quiver_stats(&self) -> Result<Option<crate::index::quiver::ssd::SsdStats>> {
        self.inner.quiver_stats()
    }
    pub fn close_quiver(&self) -> Result<()> {
        self.inner.close_quiver()
    }
    pub fn configure_quiver_int8(
        &self,
        options: crate::index::quiver::ssd::SsdRuntimeOptions,
    ) -> Result<crate::index::quiver::ssd::SsdStats> {
        self.inner.configure_quiver_int8(options)
    }

    pub fn search_quiver_int8(
        &self,
        query: &[f32],
        config: &crate::index::quiver::QuIVerSearchConfig,
    ) -> Result<Vec<(u64, f32)>> {
        self.inner.search_quiver_int8(query, config)
    }

    pub fn quiver_int8_stats(&self) -> Result<Option<crate::index::quiver::ssd::SsdStats>> {
        self.inner.quiver_int8_stats()
    }

    pub fn close_quiver_int8(&self) -> Result<()> {
        self.inner.close_quiver_int8()
    }

    pub fn with_request_context<R>(
        &self,
        context: &super::RequestContext,
        operation: impl FnOnce(&Self) -> Result<R>,
    ) -> Result<R> {
        context.run(|| operation(self))
    }

    pub fn open_read_only(path: &str, dim: usize) -> Result<Self> {
        Database::open_read_only(path, dim).map(|inner| Self { inner })
    }

    pub fn open_immutable(path: &str, dim: usize) -> Result<Self> {
        Database::open_immutable(path, dim).map(|inner| Self { inner })
    }

    pub fn open_with_runtime(
        path: &str,
        config: Config,
        runtime: crate::resources::ResourceRuntime,
    ) -> Result<Self> {
        if config.access_mode == AccessMode::ReadWrite {
            return Err(TriviumError::InvalidInput(
                "DatabaseReader 只接受 ReadOnly 或 Immutable 配置".into(),
            ));
        }
        Database::open_with_runtime(path, config, runtime).map(|inner| Self { inner })
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

    pub fn tsng_ground_truth(
        &self,
        query: &crate::tsng::TsngQuery<'_, T>,
    ) -> Result<crate::tsng::TsngGroundTruth> {
        self.inner.tsng_ground_truth(query)
    }

    pub fn search_tsng(
        &self,
        query: &crate::tsng::TsngQuery<'_, T>,
        config: crate::tsng::TsngSearchConfig,
    ) -> Result<crate::tsng::TsngSearchResult> {
        self.inner.search_tsng(query, config)
    }

    pub fn search_tsng_post_filter(
        &self,
        query: &crate::tsng::TsngQuery<'_, T>,
        config: crate::tsng::TsngSearchConfig,
    ) -> Result<crate::tsng::TsngSearchResult> {
        self.inner.search_tsng_post_filter(query, config)
    }

    pub fn search_tsng_graph_union(
        &self,
        query: &crate::tsng::TsngQuery<'_, T>,
        config: crate::tsng::TsngSearchConfig,
    ) -> Result<crate::tsng::TsngSearchResult> {
        self.inner.search_tsng_graph_union(query, config)
    }

    pub fn search_tsng_industrial(
        &self,
        query: &crate::tsng::TsngQuery<'_, T>,
        config: crate::tsng::IndustrialSearchConfig,
    ) -> Result<crate::tsng::TsngSearchResult> {
        self.inner.search_tsng_industrial(query, config)
    }

    pub fn resource_stats(&self) -> crate::resources::ResourceStats {
        self.inner.resource_stats()
    }

    pub fn instance_stats(&self) -> InstanceStats {
        self.inner.instance_stats()
    }

    pub fn index_memory_stats(&self) -> crate::observability::IndexMemoryStats {
        self.inner.index_memory_stats()
    }

    pub fn payload_memory_stats(&self) -> crate::observability::PayloadMemoryStats {
        self.inner.payload_memory_stats()
    }

    pub fn storage_write_stats(&self) -> crate::observability::StorageWriteStats {
        self.inner.storage_write_stats()
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

    pub fn get(&self, id: NodeId) -> Result<Option<NodeView<T>>> {
        self.inner.get(id)
    }

    pub fn get_named_graph(
        &self,
        graph_id: &str,
        limits: &crate::named_graph::NamedGraphLimits,
    ) -> Result<Option<crate::named_graph::NamedGraphSnapshot>> {
        self.inner.get_named_graph(graph_id, limits)
    }
    pub fn list_named_graphs(
        &self,
        limits: &crate::named_graph::NamedGraphLimits,
    ) -> Result<Vec<crate::named_graph::NamedGraphInfo>> {
        self.inner.list_named_graphs(limits)
    }
    pub fn named_graph_scope(
        &self,
        scope: &crate::named_graph::NamedGraphScope,
        limits: &crate::named_graph::NamedGraphLimits,
    ) -> Result<crate::named_graph::ScopedGraphSnapshot> {
        self.inner.named_graph_scope(scope, limits)
    }
    pub fn traverse_named_graph(
        &self,
        scope: &crate::named_graph::NamedGraphScope,
        start: NodeId,
        max_depth: usize,
        limits: &crate::named_graph::NamedGraphLimits,
    ) -> Result<crate::named_graph::ScopedGraphSnapshot> {
        self.inner
            .traverse_named_graph(scope, start, max_depth, limits)
    }

    pub fn parse_document(
        &self,
        input: &crate::document::parsing::DocumentParseInput,
    ) -> Result<crate::document::parsing::ParsedDocument> {
        self.inner.parse_document(input)
    }
    pub fn document_upload_status(
        &self,
        upload_id: &str,
    ) -> Result<crate::document::DocumentUploadStatus> {
        self.inner.document_upload_status(upload_id)
    }
    pub fn list_document_uploads(
        &self,
        document_id: Option<NodeId>,
    ) -> Result<Vec<crate::document::DocumentUploadStatus>> {
        self.inner.list_document_uploads(document_id)
    }

    pub fn get_document(&self, id: NodeId) -> Result<Option<crate::document::DocumentSnapshot>> {
        self.inner.get_document(id)
    }

    pub fn search_documents(
        &self,
        query: &crate::document::DocumentSearchQuery,
    ) -> Result<crate::document::DocumentSearchResult> {
        self.inner.search_documents(query)
    }

    pub fn document_context(
        &self,
        query: &crate::document::DocumentContextQuery,
    ) -> Result<Vec<crate::document::DocumentContext>> {
        self.inner.document_context(query)
    }

    pub fn search_documents_vector(
        &self,
        query: &crate::document::DocumentSearchQuery,
        vector: &[T],
        options: &crate::document::DocumentVectorOptions,
    ) -> Result<crate::document::DocumentSearchResult> {
        self.inner.search_documents_vector(query, vector, options)
    }

    pub fn search_documents_hybrid(
        &self,
        query: &crate::document::DocumentSearchQuery,
        vector: &[T],
        options: &crate::document::DocumentHybridOptions,
    ) -> Result<crate::document::DocumentSearchResult> {
        self.inner.search_documents_hybrid(query, vector, options)
    }

    pub fn clear_document_index(&self) -> Result<()> {
        self.inner.clear_document_index()
    }

    pub fn resolve_document_reference(
        &self,
        reference: crate::document::DocumentReference,
    ) -> Result<crate::document::DocumentEvidence> {
        self.inner.resolve_document_reference(reference)
    }

    pub fn get_payload(&self, id: NodeId) -> Result<Option<serde_json::Value>> {
        self.inner.get_payload(id)
    }

    pub fn get_edges(&self, id: NodeId) -> Result<Vec<Edge>> {
        self.inner.get_edges(id)
    }

    pub fn get_edge(&self, src: NodeId, dst: NodeId, label: &str) -> Result<Option<Edge>> {
        self.inner.get_edge(src, dst, label)
    }

    pub fn get_incoming_edges(&self, id: NodeId, label: Option<&str>) -> Result<Vec<IncomingEdge>> {
        self.inner.get_incoming_edges(id, label)
    }

    pub fn neighbors(&self, id: NodeId, depth: usize) -> Result<Vec<NodeId>> {
        self.inner.neighbors(id, depth)
    }

    pub fn neighbors_with_labels(
        &self,
        id: NodeId,
        depth: usize,
        labels: Option<&[String]>,
    ) -> Result<Vec<NodeId>> {
        self.inner.neighbors_with_labels(id, depth, labels)
    }

    pub fn shortest_path_bidirectional(
        &self,
        source: NodeId,
        target: NodeId,
        label: Option<&str>,
        budget: &crate::graph::budget::TraversalBudget,
    ) -> Result<crate::graph::pathfinding::ShortestPathOutput> {
        self.inner
            .shortest_path_bidirectional(source, target, label, budget)
    }

    pub fn graph_stats(&self) -> Result<crate::storage::memtable::GraphStats> {
        self.inner.graph_stats()
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

    pub fn tql(&self, input: &str) -> Result<TqlValueResult<T>> {
        self.inner.tql(input)
    }

    pub fn tql_nodes(&self, input: &str) -> Result<TqlResult<T>> {
        self.inner.tql_nodes(input)
    }

    pub fn query(&self, input: &str) -> Result<TqlValueResult<T>> {
        self.inner.query(input)
    }

    pub fn tql_values(&self, input: &str) -> Result<TqlValueResult<T>> {
        self.inner.query(input)
    }

    pub fn node_count(&self) -> Result<usize> {
        self.inner.node_count()
    }

    pub fn contains(&self, id: NodeId) -> Result<bool> {
        self.inner.contains(id)
    }

    pub fn dim(&self) -> usize {
        self.inner.dim()
    }

    pub fn all_node_ids(&self) -> Result<Vec<NodeId>> {
        self.inner.all_node_ids()
    }

    pub fn list_indexes(&self) -> Result<Vec<String>> {
        self.inner.list_indexes()
    }

    pub fn estimated_memory(&self) -> usize {
        self.inner.estimated_memory()
    }

    pub fn request_shutdown(&mut self) -> Result<ShutdownStatus> {
        self.inner.request_shutdown()
    }
    pub fn shutdown_status(&self) -> ShutdownStatus {
        self.inner.shutdown_status()
    }
    pub fn close_timeout(&mut self, timeout: std::time::Duration) -> Result<bool> {
        self.inner.close_timeout(timeout)
    }

    pub fn close(&mut self) -> Result<()> {
        self.inner.close()
    }
}

pub struct DatabaseWriter<T: VectorType> {
    inner: Database<T>,
}

impl<T: VectorType + serde::Serialize + serde::de::DeserializeOwned> DatabaseWriter<T> {
    pub fn with_request_context<R>(
        &mut self,
        context: &super::RequestContext,
        operation: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        context.run(|| operation(self))
    }

    pub fn open(path: &str, dim: usize) -> Result<Self> {
        Database::open(path, dim).map(|inner| Self { inner })
    }

    pub fn open_with_runtime(
        path: &str,
        config: Config,
        runtime: crate::resources::ResourceRuntime,
    ) -> Result<Self> {
        if config.access_mode != AccessMode::ReadWrite {
            return Err(TriviumError::InvalidInput(
                "DatabaseWriter 只接受 ReadWrite 配置".into(),
            ));
        }
        Database::open_with_runtime(path, config, runtime).map(|inner| Self { inner })
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
