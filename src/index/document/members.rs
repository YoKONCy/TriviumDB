//! 文档元数据数组的派生成员倒排，标量键复用属性索引编码。
use crate::document::{DocumentMemberFilter, DocumentMemberMode, budget};
use crate::index::property::PropertyKey;
use crate::{NodeId, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct MemberField {
    arrays: BTreeSet<NodeId>,
    values: BTreeMap<PropertyKey, BTreeSet<NodeId>>,
}

#[derive(Default)]
pub(crate) struct MemberIndex {
    fields: BTreeMap<String, MemberField>,
    bytes: usize,
}

impl MemberIndex {
    pub fn heap_bytes(&self) -> usize {
        self.bytes
    }

    pub fn array(&mut self, field: &str, id: NodeId) {
        if !self.fields.contains_key(field) {
            self.bytes = self.bytes.saturating_add(field.len()).saturating_add(256);
        }
        if self
            .fields
            .entry(field.into())
            .or_default()
            .arrays
            .insert(id)
        {
            self.bytes = self.bytes.saturating_add(64);
        }
    }

    pub fn value(&mut self, field: &str, id: NodeId, value: &Value) {
        let Some(key) = PropertyKey::from_json(value) else {
            return;
        };
        let values = &mut self.fields.get_mut(field).expect("数组字段已登记").values;
        if !values.contains_key(&key) {
            self.bytes = self
                .bytes
                .saturating_add(key.encoded_len())
                .saturating_add(128);
        }
        if values.entry(key).or_default().insert(id) {
            self.bytes = self.bytes.saturating_add(64);
        }
    }

    pub fn candidates(
        &self,
        filter: &DocumentMemberFilter,
        keys: &BTreeSet<PropertyKey>,
        limit: usize,
        mut in_scope: impl FnMut(NodeId) -> bool,
        mut visit: impl FnMut() -> Result<()>,
    ) -> Result<BTreeSet<NodeId>> {
        let Some(field) = self.fields.get(&filter.field) else {
            return Ok(BTreeSet::new());
        };
        let mut output = BTreeSet::new();
        let mut insert = |id| -> Result<()> {
            if in_scope(id) && !output.contains(&id) {
                budget(
                    "document_filter_candidates",
                    output.len().saturating_add(1),
                    limit,
                )?;
                output.insert(id);
            }
            Ok(())
        };
        match filter.mode {
            DocumentMemberMode::Any => {
                for key in keys {
                    crate::request::check_current()?;
                    if let Some(posting) = field.values.get(key) {
                        for &id in posting {
                            visit()?;
                            insert(id)?;
                        }
                    }
                }
            }
            DocumentMemberMode::All => {
                let mut postings = Vec::new();
                for key in keys {
                    crate::request::check_current()?;
                    let Some(posting) = field.values.get(key) else {
                        return Ok(BTreeSet::new());
                    };
                    postings.push(posting);
                }
                postings.sort_by_key(|posting| posting.len());
                let first = postings.first().copied().unwrap_or(&field.arrays);
                for &id in first {
                    visit()?;
                    let mut matches = true;
                    for posting in postings.iter().skip(1) {
                        visit()?;
                        if !posting.contains(&id) {
                            matches = false;
                            break;
                        }
                    }
                    if matches {
                        insert(id)?;
                    }
                }
            }
        }
        Ok(output)
    }
}
