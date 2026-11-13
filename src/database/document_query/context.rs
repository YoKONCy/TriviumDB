//! 在同一文档修订内展开同级块与祖先，并合并证据窗口。
use super::*;

impl<T: VectorType + Serialize + DeserializeOwned> Database<T> {
    pub fn document_context(&self, query: &DocumentContextQuery) -> Result<Vec<DocumentContext>> {
        self.read_operation(|| {
            if query.seeds.is_empty() {
                return Err(invalid(
                    "query.seeds",
                    "上下文引用不能为空 (Context references must be nonempty)",
                ));
            }
            budget(
                "document_candidates",
                query.seeds.len(),
                query.limits.max_candidates,
            )?;
            let mt = read_or_recover(&self.memtable);
            let request_bytes =
                crate::storage::payload_store::serialized_json_len(query)?.saturating_mul(32);
            let mut workspace = self.resource_runtime.reserve_workspace(0)?;
            reserve(&mt, self.memory_limit, &mut workspace, request_bytes)?;
            let mut sources = BTreeMap::<NodeId, Vec<&DocumentContextSeed>>::new();
            for seed in &query.seeds {
                crate::document::valid_id(
                    seed.reference.document_id,
                    "query.seeds.reference.document_id",
                )?;
                sources
                    .entry(seed.reference.document_id)
                    .or_default()
                    .push(seed);
            }
            let mut cost = DocumentReadCost::default();
            let mut examined = query.seeds.len();
            let mut visits = 0usize;
            let mut returned = 2usize;
            let mut result = Vec::new();
            for (id, seeds) in sources {
                crate::request::check_current()?;
                let raw = mt
                    .get_payload_raw(id)
                    .ok_or(TriviumError::NodeNotFound(id))?;
                scan(&mt, id, &mut cost, &query.limits)?;
                #[derive(Deserialize)]
                struct Count {
                    blocks: Vec<serde::de::IgnoredAny>,
                }
                reserve(
                    &mt,
                    self.memory_limit,
                    &mut workspace,
                    request_bytes
                        .saturating_add(returned.saturating_mul(4))
                        .saturating_add(raw.len().saturating_mul(4)),
                )?;
                let envelope: Envelope<'_> = serde_json::from_slice(raw).map_err(stored_error)?;
                let document = envelope.document.ok_or_else(|| {
                    invalid(
                        "document_id",
                        "目标节点未登记文档来源 (Target node has no document source)",
                    )
                })?;
                let count: Count = serde_json::from_str(document.get()).map_err(stored_error)?;
                examined = examined.saturating_add(count.blocks.len());
                budget("document_candidates", examined, query.limits.max_candidates)?;
                let base = request_bytes
                    .saturating_add(returned.saturating_mul(4))
                    .saturating_add(raw.len().saturating_mul(32));
                let mut read = ReadBudget {
                    cost: &mut cost,
                    limits: &query.limits,
                    workspace: &mut workspace,
                    base,
                    memory_limit: self.memory_limit,
                };
                let snapshot = read_snapshot_checked(&mt, id, Some(&mut read))?
                    .ok_or(TriviumError::NodeNotFound(id))?;
                let expanded = context_ranges(&snapshot, &seeds, query, &mut visits)?;
                let mut keys = Vec::new();
                returned = returned
                    .saturating_add(512)
                    .saturating_add(snapshot.source_id.len().saturating_mul(6))
                    .saturating_add(snapshot.source_revision.len().saturating_mul(6));
                budget(
                    "document_return_bytes",
                    returned,
                    query.limits.max_return_bytes,
                )?;
                for index in expanded.block_indices {
                    let key = &snapshot.blocks[index].key;
                    returned = returned
                        .saturating_add(crate::storage::payload_store::serialized_json_len(key)?)
                        .saturating_add(1);
                    budget(
                        "document_return_bytes",
                        returned,
                        query.limits.max_return_bytes,
                    )?;
                    reserve(
                        &mt,
                        self.memory_limit,
                        &mut workspace,
                        base.saturating_add(returned.saturating_mul(4)),
                    )?;
                    keys.push(key.clone());
                }
                let mut windows = Vec::new();
                for (start, end) in expanded.ranges {
                    returned = returned.saturating_add(evidence_charge(&snapshot, start, end)?);
                    budget(
                        "document_return_bytes",
                        returned,
                        query.limits.max_return_bytes,
                    )?;
                    reserve(
                        &mt,
                        self.memory_limit,
                        &mut workspace,
                        base.saturating_add(returned.saturating_mul(4)),
                    )?;
                    windows.push(evidence(&snapshot, start, end)?);
                }
                result.push(DocumentContext {
                    document_id: id,
                    revision: snapshot.revision,
                    source_id: snapshot.source_id,
                    source_revision: snapshot.source_revision,
                    block_keys: keys,
                    windows,
                });
            }
            crate::request::check_current()?;
            Ok(result)
        })?
    }
}

struct ExpandedContext {
    ranges: Vec<(usize, usize)>,
    block_indices: BTreeSet<usize>,
}

fn context_ranges(
    snapshot: &DocumentSnapshot,
    seeds: &[&DocumentContextSeed],
    query: &DocumentContextQuery,
    visits: &mut usize,
) -> Result<ExpandedContext> {
    let by_key = snapshot
        .blocks
        .iter()
        .enumerate()
        .map(|(i, block)| (block.key.as_str(), i))
        .collect::<HashMap<_, _>>();
    let mut siblings = BTreeMap::<Option<&str>, Vec<usize>>::new();
    for (i, block) in snapshot.blocks.iter().enumerate() {
        siblings.entry(block.parent.as_deref()).or_default().push(i);
    }
    let mut positions = vec![0usize; snapshot.blocks.len()];
    for list in siblings.values_mut() {
        list.sort_by_key(|&i| {
            (
                snapshot.blocks[i].start_byte,
                snapshot.blocks[i].end_byte,
                i,
            )
        });
        for (position, &i) in list.iter().enumerate() {
            positions[i] = position;
        }
    }
    let mut ranges = Vec::new();
    let mut selected = BTreeSet::new();
    for seed in seeds {
        crate::request::check_current()?;
        check_revision(snapshot, Some(&seed.reference.revision))
            .map_err(|error| error.at_field("query.seeds.reference.revision"))?;
        check_range(
            &snapshot.content,
            seed.reference.start_byte,
            seed.reference.end_byte,
            "query.seeds.reference",
        )?;
        let Some(key) = &seed.block_key else {
            ranges.push((seed.reference.start_byte, seed.reference.end_byte));
            continue;
        };
        let index = *by_key.get(key.as_str()).ok_or_else(|| {
            invalid(
                "query.seeds.block_key",
                "上下文块身份不存在 (Context block key does not exist)",
            )
        })?;
        let block = &snapshot.blocks[index];
        if seed.reference.start_byte < block.start_byte || seed.reference.end_byte > block.end_byte
        {
            return Err(invalid(
                "query.seeds.reference",
                "引用超出指定块范围 (Reference exceeds the selected block)",
            ));
        }
        let list = siblings
            .get(&block.parent.as_deref())
            .ok_or_else(|| stored_error("块同级目录缺失 (Block sibling directory is missing)"))?;
        let position = positions[index];
        let first = position.saturating_sub(query.before);
        let end = position
            .saturating_add(query.after)
            .saturating_add(1)
            .min(list.len());
        let (mut start_byte, mut end_byte) = (block.start_byte, block.end_byte);
        for &i in &list[first..end] {
            *visits = visits.saturating_add(1);
            budget("document_postings", *visits, query.limits.max_postings)?;
            crate::request::check_current()?;
            selected.insert(i);
            start_byte = start_byte.min(snapshot.blocks[i].start_byte);
            end_byte = end_byte.max(snapshot.blocks[i].end_byte);
        }
        ranges.push((start_byte, end_byte));
        let mut parent = block.parent.as_deref();
        for _ in 0..query.ancestor_depth {
            *visits = visits.saturating_add(1);
            budget("document_postings", *visits, query.limits.max_postings)?;
            crate::request::check_current()?;
            let Some(key) = parent else {
                ranges.push((0, snapshot.content.len()));
                break;
            };
            let index = *by_key
                .get(key)
                .ok_or_else(|| stored_error("上下文父级缺失 (Context parent is missing)"))?;
            let ancestor = &snapshot.blocks[index];
            selected.insert(index);
            ranges.push((ancestor.start_byte, ancestor.end_byte));
            parent = ancestor.parent.as_deref();
        }
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    Ok(ExpandedContext {
        ranges: merged,
        block_indices: selected,
    })
}
