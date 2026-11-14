//! 同一事实读视图中的文档语料、来源分组和上下文展开。
use super::documents::{check_revision, read_snapshot_checked, stored_error};
use super::{Database, lock_or_recover, read_or_recover};
use crate::document::*;
use crate::document::{budget, check_range, invalid, node_id};
use crate::index::document::{DocumentHeader, DocumentReadCost, DocumentTextIndex, DocumentUnit};
use crate::index::text::TEXT_ANALYZER_ID;
use crate::index::text::analyzer::{AnalyzedToken, visit_tokens};
use crate::index::text::positions::PositionQuery;
use crate::resources::WorkspaceReservation;
use crate::storage::memtable::MemTable;
use crate::{NodeId, Result, TriviumError, VectorType};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::Ordering;

mod context;
mod filters;
pub(crate) mod vector;

pub(super) struct ReadBudget<'a> {
    pub cost: &'a mut DocumentReadCost,
    pub limits: &'a DocumentQueryLimits,
    pub workspace: &'a mut WorkspaceReservation,
    pub base: usize,
    pub memory_limit: usize,
}

impl ReadBudget<'_> {
    pub fn payload<T: VectorType>(
        &mut self,
        mt: &MemTable<T>,
        bytes: usize,
        workspace: usize,
    ) -> Result<()> {
        self.cost.payload_bytes = self.cost.payload_bytes.saturating_add(bytes);
        self.cost.check(self.limits)?;
        let peak = self.base.saturating_add(workspace);
        mt.ensure_additional_memory(self.memory_limit, peak)?;
        self.workspace.resize(peak)
    }
    pub fn node<T: VectorType>(&mut self, mt: &MemTable<T>, id: NodeId) -> Result<()> {
        let raw = mt
            .get_payload_raw(id)
            .ok_or(TriviumError::NodeNotFound(id))?;
        self.cost.nodes = self.cost.nodes.saturating_add(1);
        self.cost.payload_bytes = self.cost.payload_bytes.saturating_add(raw.len());
        self.cost.check(self.limits)?;
        let peak = self
            .base
            .saturating_add(raw.len().saturating_mul(32))
            .saturating_add(4096);
        mt.ensure_additional_memory(self.memory_limit, peak)?;
        self.workspace.resize(peak)
    }
    pub fn edges(&mut self, count: usize) -> Result<()> {
        self.cost.edges = self.cost.edges.saturating_add(count);
        self.cost.check(self.limits)
    }
}

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(
        rename = "_tdb_document",
        default,
        borrow,
        deserialize_with = "present_raw"
    )]
    document: Option<&'a RawValue>,
}

fn present_raw<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<&'de RawValue>, D::Error> {
    <&RawValue>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct TagHead<'a> {
    #[serde(default, borrow, deserialize_with = "present_raw")]
    key: Option<&'a RawValue>,
    #[serde(default, borrow, deserialize_with = "present_raw")]
    document_id: Option<&'a RawValue>,
}

fn tag(raw: &[u8]) -> Result<Option<TagHead<'_>>> {
    if raw.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return Ok(None);
    }
    let envelope: Envelope<'_> = serde_json::from_slice(raw).map_err(stored_error)?;
    envelope
        .document
        .map(|raw| serde_json::from_str(raw.get()).map_err(stored_error))
        .transpose()
}

fn scan<T: VectorType>(
    mt: &MemTable<T>,
    id: NodeId,
    cost: &mut DocumentReadCost,
    limits: &DocumentQueryLimits,
) -> Result<()> {
    let raw = mt
        .get_payload_raw(id)
        .ok_or(TriviumError::NodeNotFound(id))?;
    cost.nodes = cost.nodes.saturating_add(1);
    cost.payload_bytes = cost.payload_bytes.saturating_add(raw.len());
    cost.check(limits)
}

fn reserve<T: VectorType>(
    mt: &MemTable<T>,
    memory_limit: usize,
    workspace: &mut WorkspaceReservation,
    bytes: usize,
) -> Result<()> {
    mt.ensure_additional_memory(memory_limit, bytes)?;
    workspace.resize(bytes)
}

fn build_index<T: VectorType>(
    mt: &MemTable<T>,
    query: &DocumentSearchQuery,
    memory_limit: usize,
    workspace: &mut WorkspaceReservation,
) -> Result<DocumentTextIndex> {
    let limits = &query.limits;
    let request_work =
        crate::storage::payload_store::serialized_json_len(query)?.saturating_mul(64);
    let mut index = DocumentTextIndex::new(
        query.uses_positions(),
        !query.fields.is_empty(),
        !query.members.is_empty(),
    );
    for id in mt.active_node_ids() {
        crate::request::check_current()?;
        scan(mt, id, &mut index.cost, limits)?;
        let raw = mt
            .get_payload_raw(id)
            .ok_or(TriviumError::NodeNotFound(id))?;
        let base = index
            .heap_bytes()
            .saturating_mul(3)
            .saturating_add(request_work)
            .saturating_add(raw.len().saturating_mul(32))
            .saturating_add(4096);
        reserve(mt, memory_limit, workspace, base)?;
        if tag(raw)?.is_none_or(|tag| tag.key.is_some()) {
            continue;
        }
        let mut read = ReadBudget {
            cost: &mut index.cost,
            limits,
            workspace,
            base,
            memory_limit,
        };
        let snapshot = read_snapshot_checked(mt, id, Some(&mut read))?
            .ok_or(TriviumError::NodeNotFound(id))?;
        index.insert_header(
            id,
            DocumentHeader {
                revision: snapshot.revision.clone(),
                source_id: snapshot.source_id.clone(),
                source_revision: snapshot.source_revision.clone(),
            },
        );
        add_unit(
            mt,
            memory_limit,
            workspace,
            &mut index,
            limits,
            DocumentUnit {
                node_id: id,
                document_id: id,
                block_key: None,
                kind: "document".into(),
            },
            &snapshot.content,
            base,
        )?;
        add_members(mt, id, &mut index, limits, memory_limit, workspace, base)?;
        for block in &snapshot.blocks {
            if let Some(raw_id) = &block.node_id {
                let node_id = node_id(raw_id, "document.blocks.node_id")?;
                let text = snapshot
                    .content
                    .get(block.start_byte..block.end_byte)
                    .ok_or_else(|| stored_error("片段范围无效 (Invalid fragment range)"))?;
                add_unit(
                    mt,
                    memory_limit,
                    workspace,
                    &mut index,
                    limits,
                    DocumentUnit {
                        node_id,
                        document_id: id,
                        block_key: Some(block.key.clone()),
                        kind: block.kind.clone(),
                    },
                    text,
                    base,
                )?;
                add_members(
                    mt,
                    node_id,
                    &mut index,
                    limits,
                    memory_limit,
                    workspace,
                    base,
                )?;
            }
        }
    }
    // 再次检查片段归属，根节点或结构清单被普通CRUD破坏时拒绝残留语料。
    for id in mt.active_node_ids() {
        crate::request::check_current()?;
        scan(mt, id, &mut index.cost, limits)?;
        let raw = mt
            .get_payload_raw(id)
            .ok_or(TriviumError::NodeNotFound(id))?;
        reserve(
            mt,
            memory_limit,
            workspace,
            index
                .heap_bytes()
                .saturating_mul(3)
                .saturating_add(request_work)
                .saturating_add(raw.len().saturating_mul(2))
                .saturating_add(4096),
        )?;
        if let Some(head) = tag(raw)?
            && head.key.is_some()
        {
            let raw_id = head
                .document_id
                .ok_or_else(|| {
                    stored_error("片段来源身份缺失 (Fragment source identity is missing)")
                })?
                .get();
            if raw_id.len() > 122 {
                return Err(stored_error(
                    "片段来源身份过长 (Fragment source identity is too long)",
                ));
            }
            let source: String = serde_json::from_str(raw_id).map_err(stored_error)?;
            let source = node_id(&source, "document_id").map_err(stored_error)?;
            if index
                .units
                .get(&id)
                .is_none_or(|unit| unit.document_id != source || unit.block_key.is_none())
            {
                return Err(stored_error(
                    "片段未登记到文档清单 (Fragment is absent from the document manifest)",
                ));
            }
        }
    }
    Ok(index)
}

fn add_members<T: VectorType>(
    mt: &MemTable<T>,
    id: NodeId,
    index: &mut DocumentTextIndex,
    limits: &DocumentQueryLimits,
    memory_limit: usize,
    workspace: &mut WorkspaceReservation,
    retained: usize,
) -> Result<()> {
    if index.members.is_none() {
        return Ok(());
    }
    let raw = mt
        .get_payload_raw(id)
        .ok_or(TriviumError::NodeNotFound(id))?;
    let base = retained
        .saturating_add(raw.len().saturating_mul(32))
        .saturating_add(4096);
    reserve(
        mt,
        memory_limit,
        workspace,
        base.saturating_add(index.heap_bytes().saturating_mul(3)),
    )?;
    let payload = mt
        .try_get_payload(id)?
        .ok_or(TriviumError::NodeNotFound(id))?;
    if let Some(data) = payload.get("data").and_then(serde_json::Value::as_object) {
        for (field, value) in data {
            if let Some(values) = value.as_array() {
                index
                    .members
                    .as_mut()
                    .expect("成员索引已启用")
                    .array(field, id);
                for value in values {
                    crate::request::check_current()?;
                    index.cost.member_values = index.cost.member_values.saturating_add(1);
                    budget(
                        "document_member_values",
                        index.cost.member_values,
                        limits.max_member_values,
                    )?;
                    reserve(
                        mt,
                        memory_limit,
                        workspace,
                        base.saturating_add(index.heap_bytes().saturating_mul(3)),
                    )?;
                    index
                        .members
                        .as_mut()
                        .expect("成员索引已启用")
                        .value(field, id, value);
                }
            }
        }
    }
    Ok(())
}

fn add_unit<T: VectorType>(
    mt: &MemTable<T>,
    memory_limit: usize,
    workspace: &mut WorkspaceReservation,
    index: &mut DocumentTextIndex,
    limits: &DocumentQueryLimits,
    unit: DocumentUnit,
    text: &str,
    base: usize,
) -> Result<()> {
    crate::request::check_current()?;
    index.cost.text_bytes = index.cost.text_bytes.saturating_add(text.len());
    index.cost.check(limits)?;
    let mut reserved = index
        .heap_bytes()
        .saturating_mul(3)
        .saturating_add(base)
        .saturating_add(text.len().saturating_mul(64))
        .saturating_add(4096);
    reserve(mt, memory_limit, workspace, reserved)?;
    let mut frequencies = HashMap::new();
    let mut length = 0usize;
    visit_tokens(
        text,
        |token| {
            index.cost.positions = index.cost.positions.saturating_add(1);
            index.cost.check(limits)?;
            if index.positions.is_some() {
                budget(
                    "document_positions",
                    index.cost.positions,
                    limits.max_positions,
                )?;
            }
            let peak = index
                .heap_bytes()
                .saturating_mul(3)
                .saturating_add(base)
                .saturating_add(text.len().saturating_mul(64))
                .saturating_add(4096)
                .saturating_add(token.term.len().saturating_mul(4));
            if peak > reserved {
                reserve(mt, memory_limit, workspace, peak)?;
                reserved = peak;
            }
            length = length.saturating_add(1);
            let frequency = frequencies.entry(token.term.clone()).or_insert(0usize);
            *frequency = frequency.saturating_add(1);
            if let Some(positions) = &mut index.positions {
                positions.add(unit.node_id, token);
            }
            Ok(())
        },
        crate::request::check_current,
    )?;
    if unit.block_key.is_some() {
        if index.field_text.is_some() {
            index.add_field(&unit, frequencies.clone(), length);
        }
        index
            .block_text
            .add_term_frequencies(unit.node_id, frequencies, length);
    } else {
        index
            .document_text
            .add_term_frequencies(unit.node_id, frequencies, length);
    }
    index.insert_unit(unit)
}

struct PendingMatch {
    start: usize,
    end: usize,
    terms: Vec<DocumentTokenMatch>,
}

struct SelectedHit {
    id: NodeId,
    score: f32,
    matches: Vec<PendingMatch>,
    scores: Option<DocumentMatchScores>,
}

struct Selection {
    document_id: NodeId,
    score: f32,
    hits: Vec<SelectedHit>,
}

fn check_revisions(index: &DocumentTextIndex, query: &DocumentSearchQuery) -> Result<()> {
    for (&id, revision) in &query.expected_revisions {
        crate::document::valid_id(id, "query.expected_revisions")?;
        let actual = &index
            .documents
            .get(&id)
            .ok_or(TriviumError::NodeNotFound(id))?
            .revision;
        if revision != actual {
            return Err(TriviumError::DocumentRevisionMismatch {
                document_id: id,
                expected: revision.clone(),
                actual: actual.clone(),
            }
            .at_field("query.expected_revisions"));
        }
    }
    Ok(())
}

fn text_scores(
    index: &DocumentTextIndex,
    query: &DocumentSearchQuery,
    tokens: &[AnalyzedToken],
    candidates: Option<&BTreeSet<NodeId>>,
    postings: &mut usize,
) -> Result<BTreeMap<NodeId, f32>> {
    index.cost.check(&query.limits)?;
    let ids = query.document_ids.iter().copied().collect::<BTreeSet<_>>();
    let sources = query
        .source_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let kinds = query
        .kinds
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut scores = BTreeMap::<NodeId, f32>::new();
    let fields = query
        .fields
        .iter()
        .map(|field| (field.kind.as_str(), field.weight))
        .collect::<BTreeMap<_, _>>();
    let corpora = if fields.is_empty() {
        vec![(index.text(query.level), 1.0)]
    } else {
        let text = index
            .field_text
            .as_ref()
            .ok_or_else(|| stored_error("字段语料未就绪 (Field corpora are not ready)"))?;
        fields
            .iter()
            .filter_map(|(kind, weight)| text.get(*kind).map(|text| (text, *weight)))
            .collect()
    };
    for (text, weight) in corpora {
        text.visit_bm25_terms(
            tokens.iter().map(|token| token.term.as_str()),
            1.2,
            0.75,
            |id, score| -> Result<()> {
                crate::request::check_current()?;
                *postings = postings.saturating_add(1);
                budget("document_postings", *postings, query.limits.max_postings)?;
                let unit = index.units.get(&id).ok_or_else(|| {
                    stored_error(
                        "文本posting缺少表示身份 (Text posting has no representation identity)",
                    )
                })?;
                let header = index.documents.get(&unit.document_id).ok_or_else(|| {
                    stored_error("表示缺少来源身份 (Representation has no source identity)")
                })?;
                if (!ids.is_empty() && !ids.contains(&unit.document_id))
                    || (!sources.is_empty() && !sources.contains(header.source_id.as_str()))
                    || (!kinds.is_empty() && !kinds.contains(unit.kind.as_str()))
                    || candidates.is_some_and(|candidates| !candidates.contains(&id))
                {
                    return Ok(());
                }
                if !scores.contains_key(&id) {
                    budget(
                        "document_candidates",
                        scores.len().saturating_add(1),
                        query.limits.max_candidates,
                    )?;
                }
                let total = scores.entry(id).or_default();
                *total += score * weight;
                if !total.is_finite() {
                    return Err(invalid(
                        "query.fields",
                        "字段加权分数溢出 (Weighted field score overflow)",
                    ));
                }
                Ok(())
            },
        )?;
    }
    Ok(scores)
}

fn group_scores(
    index: &DocumentTextIndex,
    query: &DocumentSearchQuery,
    scores: BTreeMap<NodeId, f32>,
    components: &BTreeMap<NodeId, DocumentMatchScores>,
) -> Result<(Vec<Selection>, usize)> {
    let matched = scores.len();
    let mut groups = BTreeMap::<NodeId, Vec<(NodeId, f32)>>::new();
    for (id, score) in scores {
        let unit = index
            .units
            .get(&id)
            .ok_or_else(|| stored_error("候选表示缺失 (Candidate representation is missing)"))?;
        groups
            .entry(unit.document_id)
            .or_default()
            .push((id, score));
    }
    let mut selected = Vec::new();
    for (id, mut hits) in groups {
        hits.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        hits.truncate(query.hits_per_document);
        let score = hits
            .first()
            .ok_or_else(|| stored_error("来源候选为空 (Source candidates are empty)"))?
            .1;
        selected.push(Selection {
            document_id: id,
            score,
            hits: hits
                .into_iter()
                .map(|(id, score)| SelectedHit {
                    id,
                    score,
                    matches: Vec::new(),
                    scores: components.get(&id).cloned(),
                })
                .collect(),
        });
    }
    selected.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.document_id.cmp(&b.document_id))
    });
    selected.truncate(query.max_documents);
    Ok((selected, matched))
}

struct SearchWorkspace<'a, T: VectorType> {
    mt: &'a MemTable<T>,
    workspace: &'a mut WorkspaceReservation,
    memory_limit: usize,
    filters: &'a filters::Prepared,
}

struct SearchSelection {
    rows: Vec<Selection>,
    matched: usize,
    work: usize,
    cost: DocumentReadCost,
    retrieval: Option<DocumentRetrievalInfo>,
}

impl<T: VectorType> SearchWorkspace<'_, T> {
    fn select(
        &mut self,
        index: &DocumentTextIndex,
        query: &DocumentSearchQuery,
        tokens: &[AnalyzedToken],
        retained: usize,
        recall: Option<&vector::Recall<'_, T>>,
    ) -> Result<SearchSelection> {
        let work = selection_workspace(index, query)?.saturating_add(if recall.is_some() {
            index
                .units
                .len()
                .min(query.limits.max_filter_candidates)
                .saturating_mul(512)
        } else {
            0
        });
        let base = work.saturating_add(retained);
        reserve(self.mt, self.memory_limit, self.workspace, base)?;
        index.cost.check(&query.limits)?;
        check_revisions(index, query)?;
        let mut cost = index.cost.clone();
        let mut postings = 0usize;
        let mut candidates = filters::candidates(
            self,
            index,
            query,
            &mut cost,
            &mut postings,
            base,
            recall.is_some(),
        )?;
        let options = PositionQuery {
            tokens,
            mode: query.match_mode,
            max_gap: query.max_gap,
            limits: &query.limits,
        };
        let positions = if query.uses_positions() {
            budget(
                "document_positions",
                index.cost.positions,
                query.limits.max_positions,
            )?;
            Some(index.positions.as_ref().ok_or_else(|| {
                stored_error("文档位置索引未就绪 (Document position index is not ready)")
            })?)
        } else {
            None
        };
        let mut visits = 0usize;
        let mut sparse = if recall.is_none_or(|recall| recall.text_weight.is_some()) {
            text_scores(index, query, tokens, candidates.as_ref(), &mut postings)?
        } else {
            BTreeMap::new()
        };
        if query.match_mode != DocumentMatchMode::Any {
            let positions = positions.ok_or_else(|| {
                stored_error("文档位置索引未就绪 (Document position index is not ready)")
            })?;
            let mut qualified = BTreeMap::new();
            for (id, score) in sparse {
                let bytes = positions.workspace_bytes(id, &options, visits)?;
                reserve(
                    self.mt,
                    self.memory_limit,
                    self.workspace,
                    base.saturating_add(bytes),
                )?;
                if positions.visit_matches(id, &options, &mut visits, |_| Ok(false))? {
                    qualified.insert(id, score);
                }
            }
            sparse = qualified;
            if recall.is_some() {
                candidates = Some(sparse.keys().copied().collect());
            }
        }
        let (scores, components, retrieval) = if let Some(recall) = recall {
            let eligible = candidates.as_ref().ok_or_else(|| {
                stored_error("文档候选集缺失 (Document candidate set is missing)")
            })?;
            let output = vector::recall(self, index, query, recall, eligible, sparse, base)?;
            (output.scores, output.components, Some(output.info))
        } else {
            (sparse, BTreeMap::new(), None)
        };
        let (mut selected, matched) = group_scores(index, query, scores, &components)?;
        let mut count = 0usize;
        let mut pending_bytes = 0usize;
        if query.include_matches {
            let positions = positions.ok_or_else(|| {
                stored_error("文档位置索引未就绪 (Document position index is not ready)")
            })?;
            for row in &mut selected {
                for hit in &mut row.hits {
                    let scratch = positions.workspace_bytes(hit.id, &options, visits)?;
                    reserve(
                        self.mt,
                        self.memory_limit,
                        self.workspace,
                        base.saturating_add(scratch)
                            .saturating_add(pending_bytes.saturating_mul(4)),
                    )?;
                    positions.visit_matches(hit.id, &options, &mut visits, |found| {
                        count = count.saturating_add(1);
                        budget("document_matches", count, query.limits.max_matches)?;
                        let bytes = found.terms.iter().fold(
                            512usize
                                .saturating_add(found.end_byte.saturating_sub(found.start_byte)),
                            |size, (term, _)| size.saturating_add(term.len()).saturating_add(128),
                        );
                        pending_bytes = pending_bytes.saturating_add(bytes);
                        budget(
                            "document_return_bytes",
                            pending_bytes,
                            query.limits.max_return_bytes,
                        )?;
                        reserve(
                            self.mt,
                            self.memory_limit,
                            self.workspace,
                            base.saturating_add(scratch)
                                .saturating_add(pending_bytes.saturating_mul(4)),
                        )?;
                        hit.matches.push(PendingMatch {
                            start: found.start_byte,
                            end: found.end_byte,
                            terms: found
                                .terms
                                .into_iter()
                                .map(|(term, position)| DocumentTokenMatch {
                                    term: term.to_owned(),
                                    position: position.position,
                                    start_byte: position.start_byte,
                                    end_byte: position.end_byte,
                                })
                                .collect(),
                        });
                        Ok(true)
                    })?;
                }
            }
        }
        Ok(SearchSelection {
            rows: selected,
            matched,
            work: work.saturating_add(pending_bytes.saturating_mul(4)),
            cost,
            retrieval,
        })
    }
}

fn query_tokens(query: &DocumentSearchQuery) -> Result<Vec<AnalyzedToken>> {
    if let Some(id) = &query.analyzer_id
        && id != TEXT_ANALYZER_ID
    {
        return Err(invalid(
            "query.analyzer_id",
            "分析器身份不匹配 (Analyzer identity mismatch)",
        ));
    }
    if query.match_mode != DocumentMatchMode::OrderedNear && query.max_gap != 0 {
        return Err(invalid(
            "query.max_gap",
            "max_gap适用于有序邻近查询 (max_gap applies to ordered-near queries)",
        ));
    }
    let mut tokens = Vec::new();
    let positional = query.uses_positions();
    visit_tokens(
        &query.text,
        |token| {
            if positional {
                budget(
                    "document_query_terms",
                    tokens.len().saturating_add(1),
                    query.limits.max_query_terms,
                )?;
            }
            tokens.push(token);
            Ok(())
        },
        crate::request::check_current,
    )?;
    if positional && tokens.is_empty() {
        return Err(invalid(
            "query.text",
            "位置查询需要至少一个词项 (Positional queries require at least one token)",
        ));
    }
    Ok(tokens)
}

pub(super) fn evidence(
    snapshot: &DocumentSnapshot,
    start: usize,
    end: usize,
) -> Result<DocumentEvidence> {
    evidence_at(snapshot, start, end, None)
}

fn evidence_at(
    snapshot: &DocumentSnapshot,
    start: usize,
    end: usize,
    locations: Option<&BTreeMap<usize, crate::error::QuerySpan>>,
) -> Result<DocumentEvidence> {
    check_range(&snapshot.content, start, end, "reference")?;
    let position = |byte| match locations {
        Some(locations) => locations
            .get(&byte)
            .copied()
            .ok_or_else(|| stored_error("证据位置未登记 (Evidence position is absent)")),
        None => crate::error::QuerySpan::at(&snapshot.content, byte)
            .ok_or_else(|| invalid("reference", "引用位置无效 (Invalid reference position)")),
    };
    let first = position(start)?;
    let last = position(end)?;
    Ok(DocumentEvidence {
        reference: DocumentReference {
            document_id: snapshot.document_id,
            revision: snapshot.revision.clone(),
            start_byte: start,
            end_byte: end,
        },
        source_id: snapshot.source_id.clone(),
        source_revision: snapshot.source_revision.clone(),
        parser_id: snapshot.parser_id.clone(),
        content_hash: snapshot.content_hash.clone(),
        text: snapshot
            .content
            .get(start..end)
            .ok_or_else(|| invalid("reference", "引用范围无效 (Invalid reference range)"))?
            .to_owned(),
        line: first.line,
        column: first.column,
        end_line: last.line,
        end_column: last.column,
        source_spans: snapshot.source_spans(start, end)?,
    })
}

fn evidence_charge(snapshot: &DocumentSnapshot, start: usize, end: usize) -> Result<usize> {
    let text = snapshot
        .content
        .get(start..end)
        .ok_or_else(|| invalid("reference", "引用范围无效 (Invalid reference range)"))?;
    let mut bytes = 512usize.saturating_add(
        snapshot
            .source
            .as_ref()
            .map(|source| source.span_charge(&snapshot.content, start, end))
            .transpose()?
            .unwrap_or(0),
    );
    for value in [
        text,
        snapshot.source_id.as_str(),
        snapshot.source_revision.as_str(),
        snapshot.parser_id.as_str(),
        snapshot.content_hash.as_str(),
        snapshot.revision.as_str(),
    ] {
        bytes = bytes.saturating_add(crate::storage::payload_store::serialized_json_len(&value)?);
    }
    Ok(bytes)
}

fn selection_workspace(index: &DocumentTextIndex, query: &DocumentSearchQuery) -> Result<usize> {
    Ok(index
        .units
        .len()
        .min(query.limits.max_candidates)
        .saturating_mul(512)
        .saturating_add(
            if query.document_filter.is_some()
                || query.block_filter.is_some()
                || !query.members.is_empty()
            {
                index
                    .units
                    .len()
                    .min(query.limits.max_filter_candidates)
                    .saturating_mul(512)
            } else {
                0
            },
        )
        .saturating_add(
            crate::storage::payload_store::serialized_json_len(query)?.saturating_mul(64),
        ))
}

impl<T: VectorType + Serialize + DeserializeOwned> Database<T> {
    /// 回收可重建的文档文本缓存，物理文件保持只读契约。
    pub fn clear_document_index(&self) -> Result<()> {
        self.read_operation(|| {
            let mt = read_or_recover(&self.memtable);
            let mut cache = lock_or_recover(&mt.document_index);
            *cache = None;
            #[cfg(feature = "test-hooks")]
            crate::test_hooks::hit_for_path(
                crate::test_hooks::ConcurrencyPoint::BeforeDocumentIndexAccountingReset,
                &self.db_path,
            );
            mt.document_index_bytes.store(0, Ordering::Release);
        })
    }

    /// 在完整语料统计下生成候选，再按来源和组内分数截断。
    pub fn search_documents(&self, query: &DocumentSearchQuery) -> Result<DocumentSearchResult> {
        self.search_document_query(query, None)
    }

    pub fn search_documents_vector(
        &self,
        query: &DocumentSearchQuery,
        query_vector: &[T],
        options: &DocumentVectorOptions,
    ) -> Result<DocumentSearchResult> {
        self.search_document_query(
            query,
            Some(vector::Recall {
                vector: query_vector,
                options,
                text_weight: None,
            }),
        )
    }

    pub fn search_documents_hybrid(
        &self,
        query: &DocumentSearchQuery,
        query_vector: &[T],
        options: &DocumentHybridOptions,
    ) -> Result<DocumentSearchResult> {
        self.search_document_query(
            query,
            Some(vector::Recall {
                vector: query_vector,
                options: &options.vector,
                text_weight: Some(options.text_weight),
            }),
        )
    }

    fn search_document_query(
        &self,
        query: &DocumentSearchQuery,
        recall: Option<vector::Recall<'_, T>>,
    ) -> Result<DocumentSearchResult> {
        self.read_operation(|| {
            let mut mt = read_or_recover(&self.memtable);
            validate_search(&mt, query, recall.as_ref())?;
            if recall
                .as_ref()
                .is_some_and(|recall| recall.options.rerank == DocumentVectorRerank::Compressed)
            {
                drop(mt);
                self.prepare_quiver_rerank(true)?;
                mt = read_or_recover(&self.memtable);
            }
            search_locked(&mt, query, recall, self.memory_limit)
        })?
    }
}

/// 公共入口和规划器共享参数检查，检查过程读取当前事实与运行时配置。
pub(crate) fn validate_search<T: VectorType>(
    mt: &MemTable<T>,
    query: &DocumentSearchQuery,
    recall: Option<&vector::Recall<'_, T>>,
) -> Result<()> {
    if recall
        .as_ref()
        .is_none_or(|recall| recall.text_weight.is_some())
        && query.text.trim().is_empty()
    {
        return Err(invalid(
            "query.text",
            "文档查询文本不能为空 (Document query text must be nonempty)",
        ));
    }
    if query.max_documents == 0 {
        return Err(invalid(
            "query.max_documents",
            "文档数须为正 (Document limit must be positive)",
        ));
    }
    if query.hits_per_document == 0 {
        return Err(invalid(
            "query.hits_per_document",
            "组内命中数须为正 (Per-document hit limit must be positive)",
        ));
    }
    for &id in &query.document_ids {
        crate::document::valid_id(id, "query.document_ids")?;
    }
    if let Some(recall) = &recall {
        recall.validate(query, mt)?;
    }
    Ok(())
}

/// 文档 API 和 TQL 在同一事实读视图中复用查询及证据装配。
pub(crate) fn search_locked<T: VectorType>(
    mt: &MemTable<T>,
    query: &DocumentSearchQuery,
    recall: Option<vector::Recall<'_, T>>,
    memory_limit: usize,
) -> Result<DocumentSearchResult> {
    validate_search(mt, query, recall.as_ref())?;
    let mut workspace = mt.resource_runtime().reserve_workspace(0)?;
    let request_work =
        crate::storage::payload_store::serialized_json_len(query)?.saturating_mul(64);
    reserve(mt, memory_limit, &mut workspace, request_work)?;
    let tokens = query_tokens(query)?;
    let filters = filters::Prepared::new(query)?;
    let (selection, documents, units, field_corpora) = if query.use_cache {
        let mut cache = lock_or_recover(&mt.document_index);
        if cache.as_ref().is_none_or(|index| {
            (query.uses_positions() && index.positions.is_none())
                || (!query.fields.is_empty() && index.field_text.is_none())
                || (!query.members.is_empty() && index.members.is_none())
        }) {
            // 升级派生缓存时持有同一互斥锁，旧缓存先归还额度。
            *cache = None;
            mt.document_index_bytes.store(0, Ordering::Release);
            let built = build_index(mt, query, memory_limit, &mut workspace)?;
            let bytes = built.heap_bytes();
            mt.ensure_additional_memory(memory_limit, bytes)?;
            mt.document_index_bytes.store(bytes, Ordering::Release);
            *cache = Some(built);
        }
        let index = cache
            .as_ref()
            .ok_or_else(|| stored_error("文档索引未就绪 (Document index is not ready)"))?;
        let selection = SearchWorkspace {
            mt,
            workspace: &mut workspace,
            memory_limit,
            filters: &filters,
        }
        .select(index, query, &tokens, 0, recall.as_ref())?;
        (
            selection,
            index.documents.len(),
            index.text(query.level).document_count(),
            filters::field_corpora(index, query),
        )
    } else {
        let index = build_index(mt, query, memory_limit, &mut workspace)?;
        let selection = SearchWorkspace {
            mt,
            workspace: &mut workspace,
            memory_limit,
            filters: &filters,
        }
        .select(&index, query, &tokens, index.heap_bytes(), recall.as_ref())?;
        (
            selection,
            index.documents.len(),
            index.text(query.level).document_count(),
            filters::field_corpora(&index, query),
        )
    };
    let SearchSelection {
        rows: selected,
        matched,
        work,
        mut cost,
        retrieval,
    } = selection;
    let mut groups = Vec::new();
    let mut returned = 256usize
        .saturating_add(crate::storage::payload_store::serialized_json_len(
            &field_corpora,
        )?)
        .saturating_add(crate::storage::payload_store::serialized_json_len(
            &retrieval,
        )?);
    budget(
        "document_return_bytes",
        returned,
        query.limits.max_return_bytes,
    )?;
    for row in selected {
        crate::request::check_current()?;
        let raw = mt
            .get_payload_raw(row.document_id)
            .ok_or(TriviumError::NodeNotFound(row.document_id))?;
        let base = work
            .saturating_add(returned.saturating_mul(8))
            .saturating_add(raw.len().saturating_mul(32));
        let mut read = ReadBudget {
            cost: &mut cost,
            limits: &query.limits,
            workspace: &mut workspace,
            base,
            memory_limit,
        };
        let snapshot = read_snapshot_checked(mt, row.document_id, Some(&mut read))?
            .ok_or(TriviumError::NodeNotFound(row.document_id))?;
        let mut by_node = BTreeMap::new();
        for block in &snapshot.blocks {
            if let Some(id) = &block.node_id {
                by_node.insert(node_id(id, "document.blocks.node_id")?, block);
            }
        }
        let mut boundaries = BTreeSet::new();
        for hit in &row.hits {
            let (first, last) = if hit.id == snapshot.document_id {
                (0, snapshot.content.len())
            } else {
                let block = by_node
                    .get(&hit.id)
                    .ok_or_else(|| stored_error("候选片段缺失 (Candidate fragment is missing)"))?;
                (block.start_byte, block.end_byte)
            };
            boundaries.insert(first);
            boundaries.insert(last);
            for found in &hit.matches {
                let start = first
                    .checked_add(found.start)
                    .ok_or_else(|| stored_error("命中范围溢出 (Match range overflow)"))?;
                let end = first
                    .checked_add(found.end)
                    .ok_or_else(|| stored_error("命中范围溢出 (Match range overflow)"))?;
                if end > last {
                    return Err(stored_error(
                        "命中超出检索单位 (Match exceeds its retrieval unit)",
                    ));
                }
                boundaries.insert(start);
                boundaries.insert(end);
            }
        }
        let boundaries = boundaries.into_iter().collect::<Vec<_>>();
        let locations = crate::error::QuerySpan::at_many(&snapshot.content, &boundaries)?
            .into_iter()
            .map(|span| (span.start_byte, span))
            .collect::<BTreeMap<_, _>>();
        returned = returned
            .saturating_add(512)
            .saturating_add(snapshot.source_id.len().saturating_mul(6))
            .saturating_add(snapshot.source_revision.len().saturating_mul(6));
        budget(
            "document_return_bytes",
            returned,
            query.limits.max_return_bytes,
        )?;
        let mut hits = Vec::new();
        for SelectedHit {
            id,
            score,
            matches: pending,
            scores,
        } in row.hits
        {
            crate::request::check_current()?;
            let (key, kind, start, end) = if id == snapshot.document_id {
                (None, "document", 0, snapshot.content.len())
            } else {
                let block = by_node
                    .get(&id)
                    .ok_or_else(|| stored_error("候选片段缺失 (Candidate fragment is missing)"))?;
                (
                    Some(block.key.as_str()),
                    block.kind.as_str(),
                    block.start_byte,
                    block.end_byte,
                )
            };
            returned = returned
                .saturating_add(crate::storage::payload_store::serialized_json_len(&scores)?)
                .saturating_add(evidence_charge(&snapshot, start, end)?)
                .saturating_add(key.map_or(0, str::len).saturating_mul(6))
                .saturating_add(kind.len().saturating_mul(6));
            budget(
                "document_return_bytes",
                returned,
                query.limits.max_return_bytes,
            )?;
            reserve(
                mt,
                memory_limit,
                &mut workspace,
                base.saturating_add(returned.saturating_mul(4)),
            )?;
            let mut matches = Vec::new();
            for mut found in pending {
                crate::request::check_current()?;
                let first = start
                    .checked_add(found.start)
                    .ok_or_else(|| stored_error("命中范围溢出 (Match range overflow)"))?;
                let last = start
                    .checked_add(found.end)
                    .ok_or_else(|| stored_error("命中范围溢出 (Match range overflow)"))?;
                if last > end {
                    return Err(stored_error(
                        "命中超出检索单位 (Match exceeds its retrieval unit)",
                    ));
                }
                for term in &mut found.terms {
                    term.start_byte = start
                        .checked_add(term.start_byte)
                        .ok_or_else(|| stored_error("词项范围溢出 (Token range overflow)"))?;
                    term.end_byte = start
                        .checked_add(term.end_byte)
                        .ok_or_else(|| stored_error("词项范围溢出 (Token range overflow)"))?;
                    returned = returned
                        .saturating_add(crate::storage::payload_store::serialized_json_len(term)?)
                        .saturating_add(1);
                }
                returned = returned.saturating_add(evidence_charge(&snapshot, first, last)?);
                budget(
                    "document_return_bytes",
                    returned,
                    query.limits.max_return_bytes,
                )?;
                reserve(
                    mt,
                    memory_limit,
                    &mut workspace,
                    base.saturating_add(returned.saturating_mul(4)),
                )?;
                matches.push(DocumentTextMatch {
                    evidence: evidence_at(&snapshot, first, last, Some(&locations))?,
                    terms: found.terms,
                });
            }
            hits.push(DocumentMatch {
                node_id: id,
                block_key: key.map(str::to_owned),
                kind: kind.to_owned(),
                score,
                evidence: evidence_at(&snapshot, start, end, Some(&locations))?,
                matches,
                scores,
            });
        }
        groups.push(DocumentSearchGroup {
            document_id: row.document_id,
            revision: snapshot.revision,
            source_id: snapshot.source_id,
            source_revision: snapshot.source_revision,
            score: row.score,
            hits,
        });
    }
    crate::request::check_current()?;
    Ok(DocumentSearchResult {
        analyzer_id: TEXT_ANALYZER_ID.into(),
        level: query.level,
        corpus_documents: documents,
        corpus_units: units,
        matched_units: matched,
        groups,
        field_corpora,
        retrieval,
    })
}
