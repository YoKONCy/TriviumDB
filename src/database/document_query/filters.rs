//! 文档和片段元数据的候选交集及原始事实复核。
use super::*;
use crate::filter::Filter;
use crate::index::property::PropertyKey;
use serde_json::Value;

pub(super) struct Prepared {
    document: Option<Filter>,
    block: Option<Filter>,
    members: Vec<PreparedMember>,
}

struct PreparedMember {
    scope: DocumentMetadataScope,
    field: String,
    mode: DocumentMemberMode,
    values: BTreeSet<PropertyKey>,
}

impl Prepared {
    pub fn new(query: &DocumentSearchQuery) -> Result<Self> {
        let mut kinds = BTreeSet::new();
        for (position, field) in query.fields.iter().enumerate() {
            crate::request::check_current()?;
            let path = format!("query.fields.{position}");
            if field.kind.is_empty() || !kinds.insert(&field.kind) {
                return Err(invalid(
                    &format!("{path}.kind"),
                    "字段类型须非空且唯一 (Field kinds must be nonempty and unique)",
                ));
            }
            if !field.weight.is_finite() || field.weight <= 0.0 {
                return Err(invalid(
                    &format!("{path}.weight"),
                    "字段权重须为有限正数 (Field weights must be finite and positive)",
                ));
            }
        }
        if query.level == DocumentSearchLevel::Document {
            if !query.fields.is_empty() {
                return Err(invalid(
                    "query.fields",
                    "字段检索使用block层级 (Field retrieval uses the block level)",
                ));
            }
            if query.block_filter.is_some() {
                return Err(invalid(
                    "query.block_filter",
                    "片段过滤使用block层级 (Block filtering uses the block level)",
                ));
            }
        }
        let mut members = Vec::new();
        for (position, filter) in query.members.iter().enumerate() {
            crate::request::check_current()?;
            let path = format!("query.members.{position}");
            if filter.field.is_empty() {
                return Err(invalid(
                    &format!("{path}.field"),
                    "成员字段名不能为空 (Member field names must be nonempty)",
                ));
            }
            if query.level == DocumentSearchLevel::Document
                && filter.scope == DocumentMetadataScope::Block
            {
                return Err(invalid(
                    &format!("{path}.scope"),
                    "片段成员过滤使用block层级 (Block member filtering uses the block level)",
                ));
            }
            let mut values = BTreeSet::new();
            for (index, value) in filter.values.iter().enumerate() {
                crate::request::check_current()?;
                let key = PropertyKey::from_json(value).ok_or_else(|| {
                    invalid(
                        &format!("{path}.values.{index}"),
                        "成员条件要求JSON标量 (Member conditions require JSON scalars)",
                    )
                })?;
                values.insert(key);
            }
            members.push(PreparedMember {
                scope: filter.scope,
                field: filter.field.clone(),
                mode: filter.mode,
                values,
            });
        }
        let parse = |value: &Option<Value>, field| -> Result<Option<Filter>> {
            value
                .as_ref()
                .map(|value| {
                    Filter::from_json(value).map_err(|reason| {
                        invalid(
                            field,
                            &format!(
                                "文档元数据过滤无效 (Invalid document metadata filter): {reason}"
                            ),
                        )
                    })
                })
                .transpose()
        };
        Ok(Self {
            document: parse(&query.document_filter, "query.document_filter")?,
            block: parse(&query.block_filter, "query.block_filter")?,
            members,
        })
    }
}

fn member_matches(
    data: &Value,
    filters: &[PreparedMember],
    scope: DocumentMetadataScope,
    cost: &mut DocumentReadCost,
    limits: &DocumentQueryLimits,
) -> Result<bool> {
    for filter in filters.iter().filter(|filter| filter.scope == scope) {
        let Some(array) = data.get(&filter.field).and_then(Value::as_array) else {
            return Ok(false);
        };
        let mut actual = BTreeSet::new();
        for value in array {
            crate::request::check_current()?;
            cost.member_values = cost.member_values.saturating_add(1);
            budget(
                "document_member_values",
                cost.member_values,
                limits.max_member_values,
            )?;
            if let Some(key) = PropertyKey::from_json(value) {
                actual.insert(key);
            }
        }
        let mut count = 0;
        for key in &filter.values {
            crate::request::check_current()?;
            cost.member_values = cost.member_values.saturating_add(1);
            budget(
                "document_member_values",
                cost.member_values,
                limits.max_member_values,
            )?;
            count += usize::from(actual.contains(key));
        }
        let matches = match filter.mode {
            DocumentMemberMode::Any => count > 0,
            DocumentMemberMode::All => count == filter.values.len(),
        };
        if !matches {
            return Ok(false);
        }
    }
    Ok(true)
}

fn intersect(target: &mut Option<BTreeSet<NodeId>>, rows: BTreeSet<NodeId>) {
    if let Some(target) = target {
        target.retain(|id| rows.contains(id));
    } else {
        *target = Some(rows);
    }
}

pub(super) fn candidates<T: VectorType>(
    work: &mut SearchWorkspace<'_, T>,
    index: &DocumentTextIndex,
    query: &DocumentSearchQuery,
    cost: &mut DocumentReadCost,
    postings: &mut usize,
    base: usize,
    required: bool,
) -> Result<Option<BTreeSet<NodeId>>> {
    if !required
        && work.filters.document.is_none()
        && work.filters.block.is_none()
        && query.members.is_empty()
    {
        return Ok(None);
    }
    let mut documents = None;
    let mut blocks = None;
    if !query.members.is_empty() {
        budget(
            "document_member_values",
            index.cost.member_values,
            query.limits.max_member_values,
        )?;
        let members = index
            .members
            .as_ref()
            .ok_or_else(|| stored_error("成员倒排未就绪 (Member postings are not ready)"))?;
        for (filter, prepared) in query.members.iter().zip(&work.filters.members) {
            crate::request::check_current()?;
            let rows = members.candidates(
                filter,
                &prepared.values,
                query.limits.max_filter_candidates,
                |id| {
                    index.units.get(&id).is_some_and(|unit| match filter.scope {
                        DocumentMetadataScope::Document => unit.block_key.is_none(),
                        DocumentMetadataScope::Block => unit.block_key.is_some(),
                    })
                },
                || {
                    crate::request::check_current()?;
                    #[cfg(feature = "test-hooks")]
                    crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::DocumentMemberStep);
                    crate::request::check_current()?;
                    *postings = postings.saturating_add(1);
                    budget("document_postings", *postings, query.limits.max_postings)
                },
            )?;
            match filter.scope {
                DocumentMetadataScope::Document => intersect(&mut documents, rows),
                DocumentMetadataScope::Block => intersect(&mut blocks, rows),
            }
        }
    }
    let has_document = work.filters.document.is_some() || documents.is_some();
    let has_block = work.filters.block.is_some() || blocks.is_some();
    let mut accepted_documents = BTreeSet::new();
    let mut read = ReadBudget {
        cost,
        limits: &query.limits,
        workspace: work.workspace,
        base,
        memory_limit: work.memory_limit,
    };
    if has_document {
        for &id in index.documents.keys() {
            crate::request::check_current()?;
            if documents.as_ref().is_some_and(|ids| !ids.contains(&id)) {
                continue;
            }
            read.node(work.mt, id)?;
            let payload = work
                .mt
                .try_get_payload(id)?
                .ok_or(TriviumError::NodeNotFound(id))?;
            let data = payload.get("data").unwrap_or(&Value::Null);
            if work
                .filters
                .document
                .as_ref()
                .is_none_or(|filter| filter.matches(data))
                && member_matches(
                    data,
                    &work.filters.members,
                    DocumentMetadataScope::Document,
                    read.cost,
                    read.limits,
                )?
            {
                budget(
                    "document_filter_candidates",
                    accepted_documents.len().saturating_add(1),
                    query.limits.max_filter_candidates,
                )?;
                accepted_documents.insert(id);
            }
        }
    }
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
    let fields = query
        .fields
        .iter()
        .map(|field| field.kind.as_str())
        .collect::<BTreeSet<_>>();
    let mut candidates = BTreeSet::new();
    for (&id, unit) in &index.units {
        crate::request::check_current()?;
        let header = &index.documents[&unit.document_id];
        if (unit.block_key.is_some() != (query.level == DocumentSearchLevel::Block))
            || (has_document && !accepted_documents.contains(&unit.document_id))
            || blocks.as_ref().is_some_and(|ids| !ids.contains(&id))
            || (!ids.is_empty() && !ids.contains(&unit.document_id))
            || (!sources.is_empty() && !sources.contains(header.source_id.as_str()))
            || (!kinds.is_empty() && !kinds.contains(unit.kind.as_str()))
            || (!fields.is_empty() && !fields.contains(unit.kind.as_str()))
        {
            continue;
        }
        if has_block {
            read.node(work.mt, id)?;
            let payload = work
                .mt
                .try_get_payload(id)?
                .ok_or(TriviumError::NodeNotFound(id))?;
            let data = payload.get("data").unwrap_or(&Value::Null);
            if work
                .filters
                .block
                .as_ref()
                .is_some_and(|filter| !filter.matches(data))
                || !member_matches(
                    data,
                    &work.filters.members,
                    DocumentMetadataScope::Block,
                    read.cost,
                    read.limits,
                )?
            {
                continue;
            }
        }
        budget(
            "document_filter_candidates",
            candidates.len().saturating_add(1),
            query.limits.max_filter_candidates,
        )?;
        candidates.insert(id);
    }
    Ok(Some(candidates))
}

pub(super) fn field_corpora(
    index: &DocumentTextIndex,
    query: &DocumentSearchQuery,
) -> Vec<DocumentFieldCorpus> {
    let mut fields = query
        .fields
        .iter()
        .map(|field| {
            let text = index
                .field_text
                .as_ref()
                .and_then(|fields| fields.get(&field.kind));
            DocumentFieldCorpus {
                kind: field.kind.clone(),
                weight: field.weight,
                corpus_units: text.map_or(0, |text| text.document_count()),
                average_length: text.map_or(0.0, |text| text.average_length()),
            }
        })
        .collect::<Vec<_>>();
    fields.sort_by(|a, b| a.kind.cmp(&b.kind));
    fields
}
