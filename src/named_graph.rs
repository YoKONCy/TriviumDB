//! 命名图的成员、独立边、修订与显式联合查询契约。

use crate::{NodeId, Result, TriviumError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) const MAX_GRAPH_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NamedGraphEdge {
    #[serde(with = "crate::document::decimal_id")]
    pub source_id: NodeId,
    #[serde(with = "crate::document::decimal_id")]
    pub target_id: NodeId,
    pub label: String,
    pub weight: f32,
    #[serde(default)]
    pub metadata: Value,
}

impl NamedGraphEdge {
    pub(crate) fn key(&self) -> (NodeId, NodeId, &str) {
        (self.source_id, self.target_id, &self.label)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NamedGraphWrite {
    pub graph_id: String,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default, with = "crate::document::decimal_ids")]
    pub members: Vec<NodeId>,
    #[serde(default)]
    pub edges: Vec<NamedGraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NamedGraphSnapshot {
    pub schema_version: u16,
    pub revision: String,
    #[serde(flatten)]
    pub graph: NamedGraphWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum NamedGraphEdit {
    AddMember {
        #[serde(with = "crate::document::decimal_id")]
        node_id: NodeId,
    },
    RemoveMember {
        #[serde(with = "crate::document::decimal_id")]
        node_id: NodeId,
    },
    UpsertEdge {
        edge: NamedGraphEdge,
    },
    RemoveEdge {
        #[serde(with = "crate::document::decimal_id")]
        source_id: NodeId,
        #[serde(with = "crate::document::decimal_id")]
        target_id: NodeId,
        label: String,
    },
    SetMetadata {
        metadata: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NamedGraphLimits {
    pub max_graphs: usize,
    pub max_nodes: usize,
    pub max_edges: usize,
    pub max_return_bytes: usize,
}
impl Default for NamedGraphLimits {
    fn default() -> Self {
        Self {
            max_graphs: 10_000,
            max_nodes: 1_000_000,
            max_edges: 1_000_000,
            max_return_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NamedGraphInfo {
    pub graph_id: String,
    pub revision: String,
    pub member_count: usize,
    pub edge_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct NamedGraphScope {
    pub graph_ids: Vec<String>,
    pub include_default: bool,
    pub expected_revisions: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScopedGraphEdge {
    pub graph_id: Option<String>,
    #[serde(flatten)]
    pub edge: NamedGraphEdge,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScopedGraphSnapshot {
    pub revisions: BTreeMap<String, String>,
    #[serde(with = "crate::document::decimal_ids")]
    pub members: Vec<NodeId>,
    pub edges: Vec<ScopedGraphEdge>,
}

pub(crate) fn invalid(field: &str, reason: &str) -> TriviumError {
    crate::document::invalid(field, reason)
}
pub(crate) fn bound(resource: &'static str, value: usize, limit: usize) -> Result<()> {
    crate::document::budget(resource, value, limit)
}
pub(crate) fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 1024 || id.chars().any(char::is_control) {
        return Err(invalid(
            "graph_id",
            "图身份要求1至1024字节的可打印UTF-8文本 (Graph identity requires 1 to 1024 bytes of printable UTF-8 text)",
        ));
    }
    Ok(())
}
pub(crate) fn revision_parts(revision: &str) -> Result<(&str, u64)> {
    let (incarnation, counter) = revision
        .rsplit_once(':')
        .ok_or_else(|| invalid("revision", "图修订格式无效 (Invalid graph revision format)"))?;
    let uuid = uuid::Uuid::parse_str(incarnation).map_err(|_| {
        invalid(
            "revision",
            "图修订身份无效 (Invalid graph revision identity)",
        )
    })?;
    let counter = counter
        .parse::<u64>()
        .ok()
        .filter(|counter| {
            uuid.get_version_num() == 4 && *counter > 0 && revision == format!("{uuid}:{counter}")
        })
        .ok_or_else(|| {
            invalid(
                "revision",
                "图修订计数无效 (Invalid graph revision counter)",
            )
        })?;
    Ok((incarnation, counter))
}

impl NamedGraphSnapshot {
    pub(crate) fn from_write(
        mut graph: NamedGraphWrite,
        expected_revision: Option<&str>,
    ) -> Result<Self> {
        graph.normalize()?;
        let revision = match expected_revision {
            Some(previous) => next_revision(previous)?,
            None => format!("{}:1", uuid::Uuid::new_v4()),
        };
        Ok(Self {
            schema_version: 1,
            revision,
            graph,
        })
    }
}
pub(crate) fn next_revision(previous: &str) -> Result<String> {
    let (incarnation, counter) = revision_parts(previous)?;
    let counter = counter.checked_add(1).ok_or_else(|| {
        invalid(
            "revision",
            "图修订空间已耗尽 (Graph revision space exhausted)",
        )
    })?;
    Ok(format!("{incarnation}:{counter}"))
}

impl NamedGraphWrite {
    pub(crate) fn validate_input(&self) -> Result<()> {
        validate_id(&self.graph_id)?;
        bound("named_graph_nodes", self.members.len(), 1_000_000)?;
        bound("named_graph_edges", self.edges.len(), 1_000_000)?;
        crate::storage::payload_store::json_encoded_len(&self.metadata)
            .map_err(|error| error.at_field("metadata"))?;
        for edge in &self.edges {
            crate::request::check_current()?;
            crate::storage::payload_store::json_encoded_len(&edge.metadata)
                .map_err(|error| error.at_field("edges.metadata"))?;
        }
        Ok(())
    }

    pub(crate) fn normalize(&mut self) -> Result<()> {
        self.validate_input()?;
        self.members.sort_unstable();
        if self.members.windows(2).any(|pair| pair[0] == pair[1])
            || self.members.iter().any(|id| *id == 0 || *id == u64::MAX)
        {
            return Err(invalid(
                "members",
                "图成员要求唯一的有效节点ID (Graph members require unique valid node IDs)",
            ));
        }
        self.edges.sort_by(|a, b| a.key().cmp(&b.key()));
        if self
            .edges
            .windows(2)
            .any(|pair| pair[0].key() == pair[1].key())
        {
            return Err(invalid(
                "edges",
                "同一图中边键重复 (Duplicate edge key within one graph)",
            ));
        }
        for edge in &self.edges {
            crate::request::check_current()?;
            if !edge.weight.is_finite() {
                return Err(invalid(
                    "edges.weight",
                    "边权重必须有限 (Edge weight must be finite)",
                ));
            }
            if self.members.binary_search(&edge.source_id).is_err()
                || self.members.binary_search(&edge.target_id).is_err()
            {
                return Err(invalid(
                    "edges",
                    "边的两个端点必须属于该图 (Both edge endpoints must belong to the graph)",
                ));
            }
        }
        bound(
            "named_graph_bytes",
            crate::storage::payload_store::serialized_json_len(self)?,
            MAX_GRAPH_BYTES,
        )
    }
    pub(crate) fn memory_bytes(&self) -> usize {
        self.graph_id
            .capacity()
            .saturating_add(self.members.capacity().saturating_mul(8))
            .saturating_add(crate::storage::payload_store::estimate_json_memory(
                &self.metadata,
            ))
            .saturating_add(
                self.edges
                    .capacity()
                    .saturating_mul(std::mem::size_of::<NamedGraphEdge>()),
            )
            .saturating_add(
                self.edges
                    .iter()
                    .map(|edge| {
                        edge.label.capacity().saturating_add(
                            crate::storage::payload_store::estimate_json_memory(&edge.metadata),
                        )
                    })
                    .sum::<usize>(),
            )
            .saturating_add(256)
    }
}
