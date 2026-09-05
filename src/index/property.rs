//! 持久化属性索引的统一注册表与稳定键编码。
//!
//! 本模块实现 Hash、Ordered ART、Composite ART 和 Roaring Bitmap 四类索引。
//! PropertyKey 对 JSON 类型做显式标记并保持数值/字符串排序语义，防止跨类型碰撞；
//! Registry 负责 CRUD、slot 复用、统计和 Planner 查询。`.pidx` 使用版本头、边界
//! 检查、CRC 与原子发布，索引始终是可重建加速层而非数据真相源。

use crate::error::{Result, TriviumError};
use crate::index::art::ArtMap;
use crate::node::NodeId;
use crate::storage::fs::robust_rename_and_sync;
use serde::Serialize;
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

const MAGIC: &[u8; 4] = b"TPDX";
pub const FORMAT_VERSION: u16 = 6;
const KEY_ENCODING_VERSION: u16 = 2;
const HEADER_SIZE: usize = 36;
const MAX_FIELD_BYTES: usize = 1024 * 1024;
const MAX_KEY_BYTES: usize = 16 * 1024 * 1024;
const NGRAM_WIDTH: usize = 3;
const MAX_NGRAMS_PER_VALUE: usize = 4096;

fn text_ngrams(text: &str) -> Result<Vec<String>> {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() < NGRAM_WIDTH {
        return Ok(Vec::new());
    }
    let count = chars.len() - NGRAM_WIDTH + 1;
    if count > MAX_NGRAMS_PER_VALUE {
        return Err(TriviumError::InvalidInput(format!(
            "N-gram 索引字符串超过每值 {MAX_NGRAMS_PER_VALUE} 个 gram 的预算 (N-gram value exceeds gram budget)"
        )));
    }
    let mut grams = (0..count)
        .map(|start| chars[start..start + NGRAM_WIDTH].iter().collect::<String>())
        .collect::<Vec<_>>();
    grams.sort();
    grams.dedup();
    Ok(grams)
}

fn intersect_postings<'a, I>(mut postings: I) -> Vec<NodeId>
where
    I: Iterator<Item = &'a [NodeId]>,
{
    let Some(first) = postings.next() else {
        return Vec::new();
    };
    let mut output = first.to_vec();
    for posting in postings {
        output.retain(|id| posting.binary_search(id).is_ok());
        if output.is_empty() {
            break;
        }
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PropertyKey(Vec<u8>);

impl PropertyKey {
    pub fn from_json(value: &Value) -> Option<Self> {
        let mut bytes = Vec::new();
        match value {
            Value::Null => bytes.push(0),
            Value::Bool(flag) => {
                bytes.push(1);
                bytes.push(u8::from(*flag));
            }
            Value::Number(number) => {
                if let Some(value) = number.as_i64() {
                    bytes.push(2);
                    bytes.extend_from_slice(&((value as u64) ^ (1u64 << 63)).to_be_bytes());
                } else if let Some(value) = number.as_u64() {
                    bytes.push(3);
                    bytes.extend_from_slice(&value.to_be_bytes());
                } else {
                    let mut value = number.as_f64()?;
                    if !value.is_finite() {
                        return None;
                    }
                    if value == 0.0 {
                        value = 0.0;
                    }
                    let bits = value.to_bits();
                    let ordered = if bits & (1u64 << 63) != 0 {
                        !bits
                    } else {
                        bits ^ (1u64 << 63)
                    };
                    bytes.push(4);
                    bytes.extend_from_slice(&ordered.to_be_bytes());
                }
            }
            Value::String(text) => {
                bytes.push(5);
                bytes.extend_from_slice(text.as_bytes());
            }
            Value::Array(_) | Value::Object(_) => return None,
        }
        Some(Self(bytes))
    }

    pub fn from_comparable(value: &crate::filter::ComparableValue) -> Option<Self> {
        match value {
            crate::filter::ComparableValue::Number(number) => {
                Self::from_json(&Value::Number(number.clone()))
            }
            crate::filter::ComparableValue::String(text) => {
                Self::from_json(&Value::String(text.clone()))
            }
        }
    }

    fn from_encoded_kind(bytes: Vec<u8>, kind: u8) -> Result<Self> {
        if bytes.is_empty() || (kind < 2 && bytes[0] > 5) {
            return Err(TriviumError::CorruptedFile(
                "属性索引包含无效键编码 (Property index contains an invalid key encoding)".into(),
            ));
        }
        Ok(Self(bytes))
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, Default)]
pub struct HashPropertyIndex {
    entries: HashMap<PropertyKey, Vec<NodeId>>,
}

impl HashPropertyIndex {
    fn insert(&mut self, id: NodeId, value: &Value) {
        if let Some(key) = PropertyKey::from_json(value) {
            let ids = self.entries.entry(key).or_default();
            match ids.binary_search(&id) {
                Ok(_) => {}
                Err(position) => ids.insert(position, id),
            }
        }
    }

    fn remove(&mut self, id: NodeId, value: &Value) {
        let Some(key) = PropertyKey::from_json(value) else {
            return;
        };
        let mut remove_key = false;
        if let Some(ids) = self.entries.get_mut(&key) {
            if let Ok(position) = ids.binary_search(&id) {
                ids.remove(position);
            }
            remove_key = ids.is_empty();
        }
        if remove_key {
            self.entries.remove(&key);
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PropertyIndexKind {
    Hash,
    Ordered,
    Composite,
    Bitmap,
    Ngram,
}

#[derive(Debug, Clone, Default)]
struct NgramPropertyIndex {
    entries: HashMap<String, Vec<NodeId>>,
}

impl NgramPropertyIndex {
    fn insert(&mut self, id: NodeId, value: &Value) -> Result<()> {
        let text = value.as_str().ok_or_else(|| {
            TriviumError::InvalidInput(
                "N-gram 索引字段只支持字符串 (N-gram index only supports strings)".into(),
            )
        })?;
        for gram in text_ngrams(text)? {
            let ids = self.entries.entry(gram).or_default();
            if let Err(position) = ids.binary_search(&id) {
                ids.insert(position, id);
            }
        }
        Ok(())
    }

    fn remove(&mut self, id: NodeId, value: &Value) {
        let Some(text) = value.as_str() else {
            return;
        };
        let Ok(grams) = text_ngrams(text) else {
            return;
        };
        for gram in grams {
            let mut empty = false;
            if let Some(ids) = self.entries.get_mut(&gram) {
                if let Ok(position) = ids.binary_search(&id) {
                    ids.remove(position);
                }
                empty = ids.is_empty();
            }
            if empty {
                self.entries.remove(&gram);
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OrderedPropertyIndex {
    entries: ArtMap<Vec<NodeId>>,
}

fn ordered_key(value: &Value) -> Option<PropertyKey> {
    match value {
        Value::Number(number) => {
            let mut number = number.as_f64()?;
            if !number.is_finite() {
                return None;
            }
            if number == 0.0 {
                number = 0.0;
            }
            let bits = number.to_bits();
            let ordered = if bits & (1u64 << 63) != 0 {
                !bits
            } else {
                bits ^ (1u64 << 63)
            };
            let mut bytes = vec![4];
            bytes.extend_from_slice(&ordered.to_be_bytes());
            Some(PropertyKey(bytes))
        }
        Value::String(text) => {
            let mut bytes = vec![5];
            bytes.extend_from_slice(text.as_bytes());
            Some(PropertyKey(bytes))
        }
        _ => None,
    }
}

impl OrderedPropertyIndex {
    fn insert(&mut self, id: NodeId, value: &Value) {
        if let Some(key) = ordered_key(value) {
            if let Some(ids) = self.entries.get_mut(key.as_bytes()) {
                if let Err(position) = ids.binary_search(&id) {
                    ids.insert(position, id);
                }
            } else {
                self.entries.insert(key.0, vec![id]);
            }
        }
    }

    fn remove(&mut self, id: NodeId, value: &Value) {
        let Some(key) = ordered_key(value) else {
            return;
        };
        let mut remove_key = false;
        if let Some(ids) = self.entries.get_mut(key.as_bytes()) {
            if let Ok(position) = ids.binary_search(&id) {
                ids.remove(position);
            }
            remove_key = ids.is_empty();
        }
        if remove_key {
            self.entries.remove(key.as_bytes());
        }
    }

    fn range(
        &self,
        op: Ordering,
        inclusive: bool,
        value: &Value,
        descending: bool,
        limit: Option<usize>,
    ) -> Vec<NodeId> {
        let Some(key) = ordered_key(value) else {
            return Vec::new();
        };
        let take = limit.unwrap_or(usize::MAX);
        self.entries
            .range_values(key.as_bytes(), op, inclusive, descending, take)
            .into_iter()
            .flat_map(|ids| ids.iter().copied())
            .take(take)
            .collect()
    }

    fn ordered_ids(&self, descending: bool, limit: Option<usize>) -> Vec<NodeId> {
        let take = limit.unwrap_or(usize::MAX);
        self.entries
            .ordered_values(descending, take)
            .into_iter()
            .flat_map(|ids| ids.iter().copied())
            .take(take)
            .collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PropertyIndexStats {
    pub field: String,
    pub fields: Vec<String>,
    pub kind: PropertyIndexKind,
    pub unique: bool,
    pub entry_count: usize,
    pub distinct_count: usize,
    pub null_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnPairStats {
    pub left_field: String,
    pub right_field: String,
    pub left_distinct: usize,
    pub right_distinct: usize,
    pub joint_distinct: usize,
    pub sampled_rows: usize,
}

impl ColumnPairStats {
    pub fn dependency(&self) -> f64 {
        let independent = self.left_distinct.saturating_mul(self.right_distinct) as f64;
        if independent <= 1.0 {
            return 0.0;
        }
        ((independent - self.joint_distinct as f64) / (independent - 1.0)).clamp(0.0, 1.0)
    }
}

#[derive(Debug)]
struct MappedPostingStore {
    mmap: memmap2::Mmap,
    blocks: BTreeMap<(String, PropertyKey), Range<usize>>,
    mapped_bytes: usize,
    posting_entries: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct CompositeDefinition(Vec<String>);

#[derive(Debug, Clone, Default)]
struct CompositePropertyIndex {
    entries: ArtMap<Vec<NodeId>>,
}

fn append_composite_part(bytes: &mut Vec<u8>, value: &Value) -> Option<()> {
    let key = match value {
        Value::Number(_) | Value::String(_) => ordered_key(value)?,
        _ => PropertyKey::from_json(value)?,
    };
    let len = u32::try_from(key.0.len()).ok()?;
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(&key.0);
    Some(())
}

fn composite_key(payload: &Value, fields: &[String]) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    for field in fields {
        append_composite_part(&mut bytes, payload.get(field)?)?;
    }
    Some(bytes)
}

fn unique_key(payload: &Value, fields: &[String]) -> Result<Option<Vec<u8>>> {
    let Some(first) = fields.first() else {
        return Err(TriviumError::InvalidInput(
            "唯一约束至少需要一个字段 (Unique constraint requires at least one field)".into(),
        ));
    };
    if fields.len() == 1 {
        let Some(value) = payload.get(first) else {
            return Ok(None);
        };
        return PropertyKey::from_json(value).map(|key| Some(key.0)).ok_or_else(|| {
            TriviumError::InvalidInput(format!(
                "唯一索引字段 {first} 只支持 JSON 标量 (Unique index field {first} only supports JSON scalars)"
            ))
        });
    }

    if fields.iter().any(|field| payload.get(field).is_none()) {
        return Ok(None);
    }
    composite_key(payload, fields).map(Some).ok_or_else(|| {
        TriviumError::InvalidInput(format!(
            "复合唯一索引字段 {fields:?} 只支持 JSON 标量 (Composite unique index only supports JSON scalars)"
        ))
    })
}

fn composite_prefix(
    values: &HashMap<String, Value>,
    fields: &[String],
) -> Option<(Vec<u8>, usize)> {
    let mut bytes = Vec::new();
    let mut matched = 0usize;
    for field in fields {
        let Some(value) = values.get(field) else {
            break;
        };
        append_composite_part(&mut bytes, value)?;
        matched += 1;
    }
    (matched > 0).then_some((bytes, matched))
}

fn encoded_key_matches(key: &[u8], bound: &[u8], ordering: Ordering, inclusive: bool) -> bool {
    let comparison = key.cmp(bound);
    match ordering {
        Ordering::Greater => comparison == Ordering::Greater || inclusive && comparison.is_eq(),
        Ordering::Less => comparison == Ordering::Less || inclusive && comparison.is_eq(),
        Ordering::Equal => comparison.is_eq(),
    }
}

#[derive(Debug, Clone, Default)]
struct BitmapPropertyIndex {
    entries: HashMap<PropertyKey, roaring::RoaringTreemap>,
}

#[derive(Debug, Clone)]
pub struct PropertyIndexRegistry {
    indexes: HashMap<String, HashPropertyIndex>,
    ordered_indexes: HashMap<String, OrderedPropertyIndex>,
    composite_indexes: HashMap<CompositeDefinition, CompositePropertyIndex>,
    bitmap_indexes: HashMap<String, BitmapPropertyIndex>,
    ngram_indexes: HashMap<String, NgramPropertyIndex>,
    unique_indexes: HashSet<String>,
    unique_composite_indexes: HashSet<CompositeDefinition>,
    mapped: Option<Arc<MappedPostingStore>>,
    key_encoding_version: u16,
}

impl Default for PropertyIndexRegistry {
    fn default() -> Self {
        Self {
            indexes: HashMap::new(),
            ordered_indexes: HashMap::new(),
            composite_indexes: HashMap::new(),
            bitmap_indexes: HashMap::new(),
            ngram_indexes: HashMap::new(),
            unique_indexes: HashSet::new(),
            unique_composite_indexes: HashSet::new(),
            mapped: None,
            key_encoding_version: KEY_ENCODING_VERSION,
        }
    }
}

impl PropertyIndexRegistry {
    pub fn register_unique<I, P>(&mut self, field: &str, payloads: I) -> Result<()>
    where
        I: IntoIterator<Item = (NodeId, P)>,
        P: std::borrow::Borrow<Value>,
    {
        let fields = vec![field.to_owned()];
        let payloads = payloads.into_iter().collect::<Vec<_>>();
        let mut seen = HashMap::<Vec<u8>, NodeId>::new();
        for (id, payload) in &payloads {
            let Some(key) = unique_key(payload.borrow(), &fields)? else {
                continue;
            };
            if let Some(existing_id) = seen.insert(key, *id) {
                return Err(TriviumError::UniqueConstraintViolation {
                    fields: fields.clone(),
                    existing_id,
                    conflicting_id: *id,
                });
            }
        }
        self.register(field, payloads);
        self.unique_indexes.insert(field.to_owned());
        Ok(())
    }

    pub fn register_unique_composite<I, P>(&mut self, fields: &[String], payloads: I) -> Result<()>
    where
        I: IntoIterator<Item = (NodeId, P)>,
        P: std::borrow::Borrow<Value>,
    {
        if fields.len() < 2 {
            return Err(TriviumError::InvalidInput(
                "复合唯一索引至少需要两个字段 (Composite unique index requires at least two fields)".into(),
            ));
        }
        let payloads = payloads.into_iter().collect::<Vec<_>>();
        let mut seen = HashMap::<Vec<u8>, NodeId>::new();
        for (id, payload) in &payloads {
            let Some(key) = unique_key(payload.borrow(), fields)? else {
                continue;
            };
            if let Some(existing_id) = seen.insert(key, *id) {
                return Err(TriviumError::UniqueConstraintViolation {
                    fields: fields.to_vec(),
                    existing_id,
                    conflicting_id: *id,
                });
            }
        }
        self.register_composite(fields, payloads);
        self.unique_composite_indexes
            .insert(CompositeDefinition(fields.to_vec()));
        Ok(())
    }

    pub fn unique_definitions(&self) -> Vec<Vec<String>> {
        let mut definitions = self
            .unique_indexes
            .iter()
            .map(|field| vec![field.clone()])
            .chain(
                self.unique_composite_indexes
                    .iter()
                    .map(|definition| definition.0.clone()),
            )
            .collect::<Vec<_>>();
        definitions.sort();
        definitions
    }

    pub fn clear_unique_constraints(&mut self) {
        self.unique_indexes.clear();
        self.unique_composite_indexes.clear();
    }

    pub fn validate_unique_changes<'a, I>(&self, changes: I) -> Result<()>
    where
        I: IntoIterator<Item = (NodeId, Option<&'a Value>)>,
    {
        let changes = changes.into_iter().collect::<Vec<_>>();
        for (_, payload) in &changes {
            let Some(payload) = payload else {
                continue;
            };
            for field in self.ngram_indexes.keys() {
                if let Some(value) = payload.get(field) {
                    let text = value.as_str().ok_or_else(|| {
                        TriviumError::InvalidInput(format!(
                            "N-gram 索引字段 {field} 只支持字符串 (N-gram indexed field {field} only supports strings)"
                        ))
                    })?;
                    text_ngrams(text)?;
                }
            }
        }
        let touched = changes.iter().map(|(id, _)| *id).collect::<HashSet<_>>();
        for fields in self.unique_definitions() {
            let mut pending = HashMap::<Vec<u8>, NodeId>::new();
            for (id, payload) in &changes {
                let Some(payload) = payload else {
                    continue;
                };
                let Some(key) = unique_key(payload, &fields)? else {
                    continue;
                };
                if let Some(existing_id) = pending.insert(key.clone(), *id) {
                    return Err(TriviumError::UniqueConstraintViolation {
                        fields,
                        existing_id,
                        conflicting_id: *id,
                    });
                }
                let existing = if fields.len() == 1 {
                    payload
                        .get(&fields[0])
                        .and_then(|value| self.lookup(&fields[0], value))
                        .unwrap_or_default()
                } else {
                    self.composite_indexes
                        .get(&CompositeDefinition(fields.clone()))
                        .and_then(|index| index.entries.get(&key).cloned())
                        .unwrap_or_default()
                };
                if let Some(existing_id) =
                    existing.into_iter().find(|other| !touched.contains(other))
                {
                    return Err(TriviumError::UniqueConstraintViolation {
                        fields,
                        existing_id,
                        conflicting_id: *id,
                    });
                }
            }
        }
        Ok(())
    }

    pub fn validate_unique_payloads<'a, I>(&self, payloads: I) -> Result<()>
    where
        I: IntoIterator<Item = (NodeId, &'a Value)>,
    {
        let payloads = payloads.into_iter().collect::<Vec<_>>();
        for fields in self.unique_definitions() {
            let mut seen = HashMap::<Vec<u8>, NodeId>::new();
            for (id, payload) in &payloads {
                let Some(key) = unique_key(payload, &fields)? else {
                    continue;
                };
                if let Some(existing_id) = seen.insert(key, *id) {
                    return Err(TriviumError::UniqueConstraintViolation {
                        fields,
                        existing_id,
                        conflicting_id: *id,
                    });
                }
            }
        }
        Ok(())
    }

    pub fn register<I, P>(&mut self, field: &str, payloads: I)
    where
        I: IntoIterator<Item = (NodeId, P)>,
        P: std::borrow::Borrow<Value>,
    {
        if self.indexes.contains_key(field) {
            return;
        }
        let mut index = HashPropertyIndex::default();
        for (id, payload) in payloads {
            if let Some(value) = payload.borrow().get(field)
                && let Some(key) = PropertyKey::from_json(value)
            {
                index.entries.entry(key).or_default().push(id);
            }
        }
        for ids in index.entries.values_mut() {
            ids.sort_unstable();
            ids.dedup();
        }
        self.indexes.insert(field.to_owned(), index);
    }

    pub fn register_composite<I, P>(&mut self, fields: &[String], payloads: I)
    where
        I: IntoIterator<Item = (NodeId, P)>,
        P: std::borrow::Borrow<Value>,
    {
        let definition = CompositeDefinition(fields.to_vec());
        if fields.len() < 2 || self.composite_indexes.contains_key(&definition) {
            return;
        }
        let mut grouped = BTreeMap::<Vec<u8>, Vec<NodeId>>::new();
        for (id, payload) in payloads {
            if let Some(key) = composite_key(payload.borrow(), fields) {
                grouped.entry(key).or_default().push(id);
            }
        }
        let mut index = CompositePropertyIndex::default();
        for (key, mut ids) in grouped {
            ids.sort_unstable();
            ids.dedup();
            index.entries.insert(key, ids);
        }
        self.composite_indexes.insert(definition, index);
    }

    pub fn composite_lookup(
        &self,
        equalities: &[(String, Value)],
    ) -> Option<(Vec<String>, Vec<NodeId>)> {
        let values = equalities.iter().cloned().collect::<HashMap<_, _>>();
        let (definition, index, prefix, matched) = self
            .composite_indexes
            .iter()
            .filter_map(|(definition, index)| {
                let (prefix, matched) = composite_prefix(&values, &definition.0)?;
                Some((definition, index, prefix, matched))
            })
            .max_by(|left, right| {
                left.3
                    .cmp(&right.3)
                    .then_with(|| left.0.0.len().cmp(&right.0.0.len()))
                    .then_with(|| right.0.cmp(left.0))
            })?;
        let mut ids = if matched == definition.0.len() {
            index.entries.get(&prefix).cloned().unwrap_or_default()
        } else {
            index
                .entries
                .prefix_values(&prefix)
                .into_iter()
                .flat_map(|posting| posting.iter().copied())
                .collect()
        };
        ids.sort_unstable();
        ids.dedup();
        Some((definition.0[..matched].to_vec(), ids))
    }

    pub fn composite_range_lookup(
        &self,
        equalities: &[(String, Value)],
        range_field: &str,
        op: Ordering,
        inclusive: bool,
        value: &Value,
        descending: bool,
        limit: Option<usize>,
    ) -> Option<(Vec<String>, Vec<NodeId>)> {
        let values = equalities.iter().cloned().collect::<HashMap<_, _>>();
        let (definition, index, prefix) = self
            .composite_indexes
            .iter()
            .filter_map(|(definition, index)| {
                let range_position = definition.0.iter().position(|field| field == range_field)?;
                if range_position + 1 != definition.0.len() {
                    return None;
                }
                let (prefix, matched) = composite_prefix(&values, &definition.0)?;
                (matched == range_position).then_some((definition, index, prefix))
            })
            .max_by_key(|(definition, _, _)| definition.0.len())?;
        let range_key = ordered_key(value)?;
        let take = limit.unwrap_or(usize::MAX);
        let mut matching = index
            .entries
            .prefix_entries(&prefix)
            .into_iter()
            .filter_map(|(key, posting)| {
                let suffix = &key[prefix.len()..];
                if suffix.len() < 4 {
                    return None;
                }
                let encoded_len = u32::from_be_bytes(suffix[..4].try_into().ok()?) as usize;
                let encoded = suffix.get(4..4usize.saturating_add(encoded_len))?;
                (encoded.first() == range_key.as_bytes().first()
                    && encoded_key_matches(encoded, range_key.as_bytes(), op, inclusive))
                .then_some((encoded, posting))
            })
            .collect::<Vec<_>>();
        matching.sort_by(|left, right| left.0.cmp(right.0));
        if descending {
            matching.reverse();
        }
        let ids = matching
            .into_iter()
            .flat_map(|(_, posting)| posting.iter().copied())
            .take(take)
            .collect();
        Some((definition.0.clone(), ids))
    }

    pub fn register_ngram<I, P>(&mut self, field: &str, payloads: I) -> Result<()>
    where
        I: IntoIterator<Item = (NodeId, P)>,
        P: std::borrow::Borrow<Value>,
    {
        if self.ngram_indexes.contains_key(field) {
            return Ok(());
        }
        let mut index = NgramPropertyIndex::default();
        for (id, payload) in payloads {
            if let Some(value) = payload.borrow().get(field) {
                index.insert(id, value)?;
            }
        }
        self.ngram_indexes.insert(field.to_owned(), index);
        Ok(())
    }

    pub fn ngram_lookup(&self, field: &str, needle: &str) -> Result<Option<Vec<NodeId>>> {
        let index = match self.ngram_indexes.get(field) {
            Some(index) => index,
            None => return Ok(None),
        };
        let grams = text_ngrams(needle)?;
        if grams.is_empty() {
            return Err(TriviumError::InvalidInput(format!(
                "N-gram 查询至少需要 {NGRAM_WIDTH} 个 Unicode 字符 (N-gram lookup requires at least {NGRAM_WIDTH} Unicode characters)"
            )));
        }
        let mut postings = grams
            .iter()
            .map(|gram| {
                index
                    .entries
                    .get(gram)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        postings.sort_by_key(|posting| posting.len());
        if postings.first().is_some_and(|posting| posting.is_empty()) {
            return Ok(Some(Vec::new()));
        }
        Ok(Some(intersect_postings(postings.into_iter())))
    }

    pub fn drop_ngram_index(&mut self, field: &str) {
        self.ngram_indexes.remove(field);
    }

    pub fn register_bitmap<I, P>(&mut self, field: &str, payloads: I)
    where
        I: IntoIterator<Item = (NodeId, P)>,
        P: std::borrow::Borrow<Value>,
    {
        if self.bitmap_indexes.contains_key(field) {
            return;
        }
        let mut index = BitmapPropertyIndex::default();
        for (id, payload) in payloads {
            if let Some(key) = payload.borrow().get(field).and_then(PropertyKey::from_json) {
                index.entries.entry(key).or_default().insert(id);
            }
        }
        self.bitmap_indexes.insert(field.to_owned(), index);
    }

    pub fn bitmap_lookup(&self, field: &str, value: &Value) -> Option<Vec<NodeId>> {
        let key = PropertyKey::from_json(value)?;
        Some(
            self.bitmap_indexes
                .get(field)?
                .entries
                .get(&key)
                .map(|bitmap| bitmap.iter().collect())
                .unwrap_or_default(),
        )
    }

    pub fn bitmap_intersection(&self, equalities: &[(String, Value)]) -> Option<Vec<NodeId>> {
        let mut bitmaps = equalities.iter().map(|(field, value)| {
            let key = PropertyKey::from_json(value)?;
            self.bitmap_indexes.get(field)?.entries.get(&key).cloned()
        });
        let mut output = bitmaps.next()??;
        for bitmap in bitmaps {
            output &= bitmap?;
        }
        Some(output.iter().collect())
    }

    pub fn register_ordered<I, P>(&mut self, field: &str, payloads: I)
    where
        I: IntoIterator<Item = (NodeId, P)>,
        P: std::borrow::Borrow<Value>,
    {
        if self.ordered_indexes.contains_key(field) {
            return;
        }
        let mut index = OrderedPropertyIndex::default();
        let mut entries = HashMap::<Vec<u8>, Vec<NodeId>>::new();
        for (id, payload) in payloads {
            if let Some(value) = payload.borrow().get(field)
                && let Some(key) = ordered_key(value)
            {
                entries.entry(key.0).or_default().push(id);
            }
        }
        let mut entries = entries.into_iter().collect::<Vec<_>>();
        entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        for (key, mut ids) in entries {
            ids.sort_unstable();
            ids.dedup();
            index.entries.insert(key, ids);
        }
        self.ordered_indexes.insert(field.to_owned(), index);
    }

    pub fn drop_composite_index(&mut self, fields: &[String]) {
        let definition = CompositeDefinition(fields.to_vec());
        self.composite_indexes.remove(&definition);
        self.unique_composite_indexes.remove(&definition);
    }

    pub fn drop_bitmap_index(&mut self, field: &str) {
        self.bitmap_indexes.remove(field);
    }

    pub fn drop_ordered_index(&mut self, field: &str) {
        self.ordered_indexes.remove(field);
    }

    pub fn range_lookup(
        &self,
        field: &str,
        op: Ordering,
        inclusive: bool,
        value: &Value,
        descending: bool,
        limit: Option<usize>,
    ) -> Option<Vec<NodeId>> {
        self.ordered_indexes
            .get(field)
            .map(|index| index.range(op, inclusive, value, descending, limit))
    }

    pub fn ordered_ids(
        &self,
        field: &str,
        descending: bool,
        limit: Option<usize>,
    ) -> Option<Vec<NodeId>> {
        self.ordered_indexes
            .get(field)
            .map(|index| index.ordered_ids(descending, limit))
    }

    pub fn contains_ordered(&self, field: &str) -> bool {
        self.ordered_indexes.contains_key(field)
    }

    pub fn drop_index(&mut self, field: &str) {
        self.indexes.remove(field);
    }

    pub fn lookup_limit(
        &self,
        field: &str,
        value: &Value,
        limit: Option<usize>,
    ) -> Option<Vec<NodeId>> {
        let key = PropertyKey::from_json(value)?;
        if let Some(index) = self.indexes.get(field) {
            let ids = index
                .entries
                .get(&key)
                .map(Vec::as_slice)
                .unwrap_or_default();
            return Some(
                ids.iter()
                    .copied()
                    .take(limit.unwrap_or(usize::MAX))
                    .collect(),
            );
        }
        let mapped = self.mapped.as_ref()?;
        let Some(range) = mapped.blocks.get(&(field.to_owned(), key)) else {
            return mapped
                .blocks
                .keys()
                .any(|(name, _)| name == field)
                .then(Vec::new);
        };
        let bytes = mapped.mmap.get(range.clone())?;
        if !bytes.len().is_multiple_of(std::mem::size_of::<NodeId>()) {
            return None;
        }
        Some(
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .take(limit.unwrap_or(usize::MAX))
                .map(|raw| u64::from_le_bytes(*raw))
                .collect(),
        )
    }

    pub fn lookup(&self, field: &str, value: &Value) -> Option<Vec<NodeId>> {
        self.lookup_limit(field, value, None)
    }

    pub fn contains(&self, field: &str) -> bool {
        self.indexes.contains_key(field)
            || self
                .mapped
                .as_ref()
                .is_some_and(|mapped| mapped.blocks.keys().any(|(name, _)| name == field))
    }

    pub fn key_encoding_version(&self) -> u16 {
        self.key_encoding_version
    }

    pub fn index_definitions(&self) -> Vec<(PropertyIndexKind, Vec<String>)> {
        let mut definitions = self
            .indexes
            .keys()
            .map(|field| (PropertyIndexKind::Hash, vec![field.clone()]))
            .chain(
                self.ordered_indexes
                    .keys()
                    .map(|field| (PropertyIndexKind::Ordered, vec![field.clone()])),
            )
            .chain(
                self.composite_indexes
                    .keys()
                    .map(|definition| (PropertyIndexKind::Composite, definition.0.clone())),
            )
            .chain(
                self.bitmap_indexes
                    .keys()
                    .map(|field| (PropertyIndexKind::Bitmap, vec![field.clone()])),
            )
            .chain(
                self.ngram_indexes
                    .keys()
                    .map(|field| (PropertyIndexKind::Ngram, vec![field.clone()])),
            )
            .chain(self.mapped.iter().flat_map(|mapped| {
                mapped
                    .blocks
                    .keys()
                    .map(|(field, _)| (PropertyIndexKind::Hash, vec![field.clone()]))
            }))
            .collect::<Vec<_>>();
        definitions.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
        definitions.dedup();
        definitions
    }

    pub fn field_names(&self) -> HashSet<&str> {
        self.indexes
            .keys()
            .chain(self.ordered_indexes.keys())
            .chain(self.bitmap_indexes.keys())
            .chain(self.ngram_indexes.keys())
            .map(String::as_str)
            .chain(
                self.composite_indexes
                    .keys()
                    .flat_map(|definition| definition.0.iter().map(String::as_str)),
            )
            .chain(
                self.mapped
                    .iter()
                    .flat_map(|mapped| mapped.blocks.keys().map(|(field, _)| field.as_str())),
            )
            .collect()
    }

    pub fn composite_definitions(&self) -> Vec<Vec<String>> {
        self.composite_indexes
            .keys()
            .map(|definition| definition.0.clone())
            .collect()
    }

    pub fn stats(&self) -> Vec<PropertyIndexStats> {
        let mut stats: Vec<_> = self
            .indexes
            .iter()
            .map(|(field, index)| PropertyIndexStats {
                field: field.clone(),
                fields: vec![field.clone()],
                kind: PropertyIndexKind::Hash,
                unique: self.unique_indexes.contains(field),
                entry_count: index.entries.values().map(Vec::len).sum(),
                distinct_count: index.entries.len(),
                null_count: index
                    .entries
                    .iter()
                    .find(|(key, _)| key.as_bytes() == [0])
                    .map(|(_, ids)| ids.len())
                    .unwrap_or_default(),
            })
            .chain(self.ordered_indexes.iter().map(|(field, index)| {
                PropertyIndexStats {
                    field: field.clone(),
                    fields: vec![field.clone()],
                    kind: PropertyIndexKind::Ordered,
                    unique: false,
                    entry_count: index
                        .entries
                        .entries(false)
                        .into_iter()
                        .map(|(_, ids)| ids.len())
                        .sum(),
                    distinct_count: index.entries.len(),
                    null_count: 0,
                }
            }))
            .chain(self.composite_indexes.iter().map(|(definition, index)| {
                PropertyIndexStats {
                    field: definition.0.join(","),
                    fields: definition.0.clone(),
                    kind: PropertyIndexKind::Composite,
                    unique: self.unique_composite_indexes.contains(definition),
                    entry_count: index
                        .entries
                        .entries(false)
                        .into_iter()
                        .map(|(_, ids)| ids.len())
                        .sum(),
                    distinct_count: index.entries.len(),
                    null_count: 0,
                }
            }))
            .chain(self.bitmap_indexes.iter().map(|(field, index)| {
                PropertyIndexStats {
                    field: field.clone(),
                    fields: vec![field.clone()],
                    kind: PropertyIndexKind::Bitmap,
                    unique: false,
                    entry_count: index
                        .entries
                        .values()
                        .map(|bitmap| bitmap.len() as usize)
                        .sum(),
                    distinct_count: index.entries.len(),
                    null_count: index
                        .entries
                        .iter()
                        .find(|(key, _)| key.as_bytes() == [0])
                        .map(|(_, bitmap)| bitmap.len() as usize)
                        .unwrap_or_default(),
                }
            }))
            .chain(
                self.ngram_indexes
                    .iter()
                    .map(|(field, index)| PropertyIndexStats {
                        field: field.clone(),
                        fields: vec![field.clone()],
                        kind: PropertyIndexKind::Ngram,
                        unique: false,
                        entry_count: index.entries.values().map(Vec::len).sum(),
                        distinct_count: index.entries.len(),
                        null_count: 0,
                    }),
            )
            .collect();
        if let Some(mapped) = &self.mapped {
            let mut by_field = BTreeMap::<String, (usize, usize, usize)>::new();
            for ((field, key), range) in &mapped.blocks {
                let entry = by_field.entry(field.clone()).or_default();
                let posting_len = range.len() / std::mem::size_of::<NodeId>();
                entry.0 = entry.0.saturating_add(posting_len);
                entry.1 = entry.1.saturating_add(1);
                if key.as_bytes() == [0] {
                    entry.2 = entry.2.saturating_add(posting_len);
                }
            }
            for (field, (entry_count, distinct_count, null_count)) in by_field {
                if self.indexes.contains_key(&field) {
                    continue;
                }
                stats.push(PropertyIndexStats {
                    fields: vec![field.clone()],
                    field: field.clone(),
                    kind: PropertyIndexKind::Hash,
                    unique: self.unique_indexes.contains(&field),
                    entry_count,
                    distinct_count,
                    null_count,
                });
            }
        }
        stats.sort_by(|left, right| left.field.cmp(&right.field));
        stats
    }

    pub fn insert(&mut self, id: NodeId, payload: &Value) {
        for (field, index) in &mut self.indexes {
            if let Some(value) = payload.get(field) {
                index.insert(id, value);
            }
        }
        for (field, index) in &mut self.ordered_indexes {
            if let Some(value) = payload.get(field) {
                index.insert(id, value);
            }
        }
        for (definition, index) in &mut self.composite_indexes {
            if let Some(key) = composite_key(payload, &definition.0) {
                if let Some(ids) = index.entries.get_mut(&key) {
                    if let Err(position) = ids.binary_search(&id) {
                        ids.insert(position, id);
                    }
                } else {
                    index.entries.insert(key, vec![id]);
                }
            }
        }
        for (field, index) in &mut self.bitmap_indexes {
            if let Some(key) = payload.get(field).and_then(PropertyKey::from_json) {
                index.entries.entry(key).or_default().insert(id);
            }
        }
        for (field, index) in &mut self.ngram_indexes {
            if let Some(value) = payload.get(field) {
                let _ = index.insert(id, value);
            }
        }
    }

    pub fn remove(&mut self, id: NodeId, payload: &Value) {
        for (field, index) in &mut self.indexes {
            if let Some(value) = payload.get(field) {
                index.remove(id, value);
            }
        }
        for (field, index) in &mut self.ordered_indexes {
            if let Some(value) = payload.get(field) {
                index.remove(id, value);
            }
        }
        for (definition, index) in &mut self.composite_indexes {
            if let Some(key) = composite_key(payload, &definition.0) {
                let mut remove_key = false;
                if let Some(ids) = index.entries.get_mut(&key) {
                    if let Ok(position) = ids.binary_search(&id) {
                        ids.remove(position);
                    }
                    remove_key = ids.is_empty();
                }
                if remove_key {
                    index.entries.remove(&key);
                }
            }
        }
        for (field, index) in &mut self.bitmap_indexes {
            if let Some(key) = payload.get(field).and_then(PropertyKey::from_json)
                && let Some(bitmap) = index.entries.get_mut(&key)
            {
                bitmap.remove(id);
                if bitmap.is_empty() {
                    index.entries.remove(&key);
                }
            }
        }
        for (field, index) in &mut self.ngram_indexes {
            if let Some(value) = payload.get(field) {
                index.remove(id, value);
            }
        }
    }

    pub fn update(&mut self, id: NodeId, old_payload: &Value, new_payload: &Value) {
        self.remove(id, old_payload);
        self.insert(id, new_payload);
    }

    pub fn is_empty(&self) -> bool {
        self.indexes.is_empty()
            && self.ordered_indexes.is_empty()
            && self.composite_indexes.is_empty()
            && self.bitmap_indexes.is_empty()
            && self.ngram_indexes.is_empty()
    }

    pub fn mapped_bytes(&self) -> usize {
        self.mapped.as_ref().map_or(0, |mapped| mapped.mapped_bytes)
    }

    pub fn mapped_posting_entries(&self) -> usize {
        self.mapped
            .as_ref()
            .map_or(0, |mapped| mapped.posting_entries)
    }

    pub fn estimated_memory_bytes(&self) -> usize {
        let hash_bytes = self
            .indexes
            .iter()
            .map(|(field, index)| {
                field.capacity()
                    + index
                        .entries
                        .iter()
                        .map(|(key, ids)| {
                            key.0.capacity() + ids.capacity() * std::mem::size_of::<NodeId>()
                        })
                        .sum::<usize>()
            })
            .sum::<usize>();
        let ordered_bytes = self
            .ordered_indexes
            .iter()
            .map(|(field, index)| {
                field.capacity()
                    + index
                        .entries
                        .entries(false)
                        .into_iter()
                        .map(|(key, ids)| {
                            key.len() + ids.capacity() * std::mem::size_of::<NodeId>()
                        })
                        .sum::<usize>()
            })
            .sum::<usize>();
        let composite_bytes = self
            .composite_indexes
            .iter()
            .map(|(definition, index)| {
                definition.0.iter().map(String::capacity).sum::<usize>()
                    + index
                        .entries
                        .entries(false)
                        .into_iter()
                        .map(|(key, ids)| {
                            key.len() + ids.capacity() * std::mem::size_of::<NodeId>()
                        })
                        .sum::<usize>()
            })
            .sum::<usize>();
        let bitmap_bytes = self
            .bitmap_indexes
            .iter()
            .map(|(field, index)| {
                field.capacity()
                    + index
                        .entries
                        .iter()
                        .map(|(key, bitmap)| key.0.capacity() + bitmap.serialized_size())
                        .sum::<usize>()
            })
            .sum::<usize>();
        let ngram_bytes = self
            .ngram_indexes
            .iter()
            .map(|(field, index)| {
                field.capacity()
                    + index
                        .entries
                        .iter()
                        .map(|(gram, ids)| {
                            gram.capacity() + ids.capacity() * std::mem::size_of::<NodeId>()
                        })
                        .sum::<usize>()
            })
            .sum::<usize>();
        hash_bytes
            .saturating_add(ordered_bytes)
            .saturating_add(composite_bytes)
            .saturating_add(bitmap_bytes)
            .saturating_add(ngram_bytes)
            .saturating_add(self.mapped.as_ref().map_or(0, |mapped| {
                mapped
                    .blocks
                    .keys()
                    .map(|(field, key)| field.capacity() + key.0.capacity() + 48)
                    .sum()
            }))
    }
}

fn file_crc32(path: &Path) -> std::io::Result<u32> {
    let mut reader = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize())
}

fn read_exact<'a>(
    bytes: &'a [u8],
    offset: &mut usize,
    len: usize,
    field: &str,
) -> Result<&'a [u8]> {
    let end = offset.checked_add(len).ok_or_else(|| {
        TriviumError::CorruptedFile(format!("属性索引 {field} 偏移溢出 (offset overflow)"))
    })?;
    let value = bytes.get(*offset..end).ok_or_else(|| {
        TriviumError::CorruptedFile(format!("属性索引 {field} 被截断 (is truncated)"))
    })?;
    *offset = end;
    Ok(value)
}

fn read_u16(bytes: &[u8], offset: &mut usize, field: &str) -> Result<u16> {
    let raw: [u8; 2] = read_exact(bytes, offset, 2, field)?
        .try_into()
        .map_err(|_| TriviumError::CorruptedFile(format!("属性索引 {field} 无效")))?;
    Ok(u16::from_le_bytes(raw))
}

fn read_u32(bytes: &[u8], offset: &mut usize, field: &str) -> Result<u32> {
    let raw: [u8; 4] = read_exact(bytes, offset, 4, field)?
        .try_into()
        .map_err(|_| TriviumError::CorruptedFile(format!("属性索引 {field} 无效")))?;
    Ok(u32::from_le_bytes(raw))
}

fn read_u64(bytes: &[u8], offset: &mut usize, field: &str) -> Result<u64> {
    let raw: [u8; 8] = read_exact(bytes, offset, 8, field)?
        .try_into()
        .map_err(|_| TriviumError::CorruptedFile(format!("属性索引 {field} 无效")))?;
    Ok(u64::from_le_bytes(raw))
}

enum PersistedIndexRef<'a> {
    Hash(&'a HashPropertyIndex),
    Ordered(&'a OrderedPropertyIndex),
    Composite(&'a CompositePropertyIndex),
    Bitmap(&'a BitmapPropertyIndex),
    Ngram(&'a NgramPropertyIndex),
}

impl PersistedIndexRef<'_> {
    fn kind(&self) -> u8 {
        match self {
            Self::Hash(_) => 0,
            Self::Ordered(_) => 1,
            Self::Composite(_) => 2,
            Self::Bitmap(_) => 3,
            Self::Ngram(_) => 4,
        }
    }

    fn entries(&self) -> Vec<(PropertyKey, Vec<NodeId>)> {
        let mut entries: Vec<_> = match self {
            Self::Hash(index) => index
                .entries
                .iter()
                .map(|(key, ids)| (key.clone(), ids.clone()))
                .collect(),
            Self::Ordered(index) => index
                .entries
                .entries(false)
                .into_iter()
                .map(|(key, ids)| (PropertyKey(key.to_vec()), ids.clone()))
                .collect(),
            Self::Composite(index) => index
                .entries
                .entries(false)
                .into_iter()
                .map(|(key, ids)| (PropertyKey(key.to_vec()), ids.clone()))
                .collect(),
            Self::Bitmap(index) => index
                .entries
                .iter()
                .map(|(key, bitmap)| (key.clone(), bitmap.iter().collect()))
                .collect(),
            Self::Ngram(index) => index
                .entries
                .iter()
                .map(|(gram, ids)| (PropertyKey(gram.as_bytes().to_vec()), ids.clone()))
                .collect(),
        };
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }
}

pub fn save_sidecar(
    registry: &PropertyIndexRegistry,
    db_path: &str,
    node_count: usize,
) -> Result<()> {
    let sidecar_path = format!("{db_path}.pidx");
    if registry.is_empty() {
        std::fs::remove_file(&sidecar_path).ok();
        return Ok(());
    }

    let main_path = Path::new(db_path);
    let main_size = std::fs::metadata(main_path)?.len();
    let main_crc = file_crc32(main_path)?;
    let mut fields: Vec<(String, PersistedIndexRef<'_>, bool)> = registry
        .indexes
        .iter()
        .map(|(field, index)| {
            (
                field.clone(),
                PersistedIndexRef::Hash(index),
                registry.unique_indexes.contains(field),
            )
        })
        .chain(
            registry
                .ordered_indexes
                .iter()
                .map(|(field, index)| (field.clone(), PersistedIndexRef::Ordered(index), false)),
        )
        .chain(
            registry
                .composite_indexes
                .iter()
                .map(|(definition, index)| {
                    (
                        definition.0.join("\0"),
                        PersistedIndexRef::Composite(index),
                        registry.unique_composite_indexes.contains(definition),
                    )
                }),
        )
        .chain(
            registry
                .bitmap_indexes
                .iter()
                .map(|(field, index)| (field.clone(), PersistedIndexRef::Bitmap(index), false)),
        )
        .chain(
            registry
                .ngram_indexes
                .iter()
                .map(|(field, index)| (field.clone(), PersistedIndexRef::Ngram(index), false)),
        )
        .collect();
    fields.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.kind().cmp(&right.1.kind()))
    });

    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&KEY_ENCODING_VERSION.to_le_bytes());
    bytes.extend_from_slice(&main_size.to_le_bytes());
    bytes.extend_from_slice(&main_crc.to_le_bytes());
    bytes.extend_from_slice(&(node_count as u64).to_le_bytes());
    bytes.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());

    for (field, index, unique) in fields {
        bytes.push(index.kind());
        bytes.push(u8::from(unique));
        let field_bytes = field.as_bytes();
        let field_len = u32::try_from(field_bytes.len()).map_err(|_| {
            TriviumError::InvalidInput(
                "属性索引字段名过长 (Property index field is too long)".into(),
            )
        })?;
        bytes.extend_from_slice(&field_len.to_le_bytes());
        bytes.extend_from_slice(field_bytes);
        let entries = index.entries();
        bytes.extend_from_slice(&(entries.len() as u64).to_le_bytes());
        for (key, ids) in entries {
            let key_len = u32::try_from(key.as_bytes().len()).map_err(|_| {
                TriviumError::InvalidInput("属性索引键过长 (Property index key is too long)".into())
            })?;
            bytes.extend_from_slice(&key_len.to_le_bytes());
            bytes.extend_from_slice(key.as_bytes());
            bytes.extend_from_slice(&(ids.len() as u64).to_le_bytes());
            let mut posting = Vec::with_capacity(ids.len().saturating_mul(8));
            for id in ids {
                posting.extend_from_slice(&id.to_le_bytes());
            }
            bytes.extend_from_slice(&(posting.len() as u64).to_le_bytes());
            bytes.extend_from_slice(&crc32fast::hash(&posting).to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
            bytes.extend_from_slice(&posting);
        }
    }
    let checksum = crc32fast::hash(&bytes);
    bytes.extend_from_slice(&checksum.to_le_bytes());

    let tmp_path = format!("{sidecar_path}.tmp");
    {
        let mut file = std::fs::File::create(&tmp_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    robust_rename_and_sync(Path::new(&tmp_path), Path::new(&sidecar_path))?;
    Ok(())
}

#[allow(clippy::needless_borrow)]
pub fn load_sidecar(
    db_path: &str,
    expected_node_count: usize,
    prefer_mmap_postings: bool,
) -> Result<Option<PropertyIndexRegistry>> {
    let sidecar_path = format!("{db_path}.pidx");
    let path = Path::new(&sidecar_path);
    if !path.exists() {
        return Ok(None);
    }
    let file = std::fs::File::open(path)?;
    // SAFETY: 文件句柄在映射创建期间有效，映射仅以只读方式访问且生命周期由 registry 持有。
    let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(TriviumError::Io)?;
    let bytes = &mmap[..];
    if bytes.len() < HEADER_SIZE + 4 {
        return Err(TriviumError::CorruptedFile(
            "属性索引文件头被截断 (Property index header is truncated)".into(),
        ));
    }
    let payload_end = bytes.len() - 4;
    let expected_crc = u32::from_le_bytes(
        bytes[payload_end..]
            .try_into()
            .map_err(|_| TriviumError::CorruptedFile("属性索引校验和无效".into()))?,
    );
    if crc32fast::hash(&bytes[..payload_end]) != expected_crc {
        return Err(TriviumError::CorruptedFile(
            "属性索引 CRC32 不匹配 (Property index CRC32 mismatch)".into(),
        ));
    }

    let mut offset = 0usize;
    if read_exact(&bytes[..payload_end], &mut offset, 4, "magic")? != MAGIC {
        return Err(TriviumError::CorruptedFile(
            "属性索引魔数无效 (Invalid property index magic)".into(),
        ));
    }
    let version = read_u16(&bytes, &mut offset, "format_version")?;
    let key_version = read_u16(&bytes, &mut offset, "key_encoding_version")?;
    if !(1..=FORMAT_VERSION).contains(&version)
        || !(1..=KEY_ENCODING_VERSION).contains(&key_version)
    {
        return Err(TriviumError::CorruptedFile(format!(
            "不支持的属性索引版本 (Unsupported property index version): {version}/{key_version}"
        )));
    }
    let main_size = read_u64(&bytes, &mut offset, "main_size")?;
    let main_crc = read_u32(&bytes, &mut offset, "main_crc")?;
    let node_count = read_u64(&bytes, &mut offset, "node_count")?;
    let field_count = read_u32(&bytes, &mut offset, "field_count")? as usize;
    let _reserved = read_u32(&bytes, &mut offset, "reserved")?;
    let actual_main_size = std::fs::metadata(db_path)?.len();
    let actual_main_crc = file_crc32(Path::new(db_path))?;
    if main_size != actual_main_size
        || main_crc != actual_main_crc
        || node_count != expected_node_count as u64
    {
        return Err(TriviumError::CorruptedFile(
            "属性索引与主数据库 generation 不匹配 (Property index does not match the main database generation)".into(),
        ));
    }
    if field_count > expected_node_count.saturating_add(1024) {
        return Err(TriviumError::CorruptedFile(
            "属性索引字段数量不合理 (Unreasonable property index field count)".into(),
        ));
    }

    let mut registry = PropertyIndexRegistry {
        key_encoding_version: key_version,
        ..Default::default()
    };
    let mut all_mapped_blocks = BTreeMap::new();
    let mut all_mapped_posting_entries = 0usize;
    for _ in 0..field_count {
        let kind = if version >= 2 {
            *read_exact(&bytes, &mut offset, 1, "index_kind")?
                .first()
                .ok_or_else(|| TriviumError::CorruptedFile("属性索引 kind 缺失".into()))?
        } else {
            0
        };
        if kind > 4 || version < 4 && kind > 1 || version < 6 && kind > 3 {
            return Err(TriviumError::CorruptedFile(
                "属性索引 kind 无效 (Invalid property index kind)".into(),
            ));
        }
        let unique = if version >= 5 {
            match *read_exact(&bytes, &mut offset, 1, "index_flags")?
                .first()
                .ok_or_else(|| TriviumError::CorruptedFile("属性索引 flags 缺失".into()))?
            {
                0 => false,
                1 => true,
                _ => {
                    return Err(TriviumError::CorruptedFile(
                        "属性索引 flags 无效 (Invalid property index flags)".into(),
                    ));
                }
            }
        } else {
            false
        };
        if unique && !matches!(kind, 0 | 2) {
            return Err(TriviumError::CorruptedFile(
                "仅 Hash/Composite 索引可声明唯一约束 (Only Hash/Composite indexes may be unique)"
                    .into(),
            ));
        }
        let field_len = read_u32(&bytes, &mut offset, "field_len")? as usize;
        if field_len > MAX_FIELD_BYTES {
            return Err(TriviumError::CorruptedFile(
                "属性索引字段名过长 (Property index field is too long)".into(),
            ));
        }
        let field = std::str::from_utf8(read_exact(&bytes, &mut offset, field_len, "field")?)
            .map_err(|_| TriviumError::CorruptedFile("属性索引字段名不是 UTF-8".into()))?
            .to_owned();
        let duplicate = match kind {
            0 => registry.indexes.contains_key(&field),
            1 => registry.ordered_indexes.contains_key(&field),
            2 => registry
                .composite_indexes
                .contains_key(&CompositeDefinition(
                    field.split('\0').map(str::to_owned).collect(),
                )),
            3 => registry.bitmap_indexes.contains_key(&field),
            4 => registry.ngram_indexes.contains_key(&field),
            _ => false,
        };
        if duplicate {
            return Err(TriviumError::CorruptedFile(
                "属性索引包含重复字段 (Property index contains duplicate fields)".into(),
            ));
        }
        let entry_count = read_u64(&bytes, &mut offset, "entry_count")? as usize;
        let max_entry_count = if kind == 4 {
            expected_node_count
                .saturating_mul(MAX_NGRAMS_PER_VALUE)
                .saturating_add(1)
        } else {
            expected_node_count.saturating_add(1)
        };
        if entry_count > max_entry_count {
            return Err(TriviumError::CorruptedFile(
                "属性索引键数量不合理 (Unreasonable property index key count)".into(),
            ));
        }
        let mut entries = HashMap::new();
        let mut mapped_blocks = BTreeMap::new();
        let mut mapped_posting_entries = 0usize;
        for _ in 0..entry_count {
            let key_len = read_u32(&bytes, &mut offset, "key_len")? as usize;
            if key_len > MAX_KEY_BYTES {
                return Err(TriviumError::CorruptedFile(
                    "属性索引键过长 (Property index key is too long)".into(),
                ));
            }
            let key = if kind == 4 {
                PropertyKey(read_exact(&bytes, &mut offset, key_len, "key")?.to_vec())
            } else {
                PropertyKey::from_encoded_kind(
                    read_exact(&bytes, &mut offset, key_len, "key")?.to_vec(),
                    kind,
                )?
            };
            let id_count = read_u64(&bytes, &mut offset, "id_count")? as usize;
            if id_count > expected_node_count {
                return Err(TriviumError::CorruptedFile(
                    "属性索引 ID 数量不合理 (Unreasonable property index ID count)".into(),
                ));
            }
            let posting_range = if version >= 3 {
                let block_len = read_u64(&bytes, &mut offset, "posting_block_len")? as usize;
                let expected_block_len = id_count.checked_mul(8).ok_or_else(|| {
                    TriviumError::CorruptedFile("属性索引 posting 长度溢出".into())
                })?;
                if block_len != expected_block_len {
                    return Err(TriviumError::CorruptedFile(
                        "属性索引 posting block 长度无效 (Invalid property posting block length)"
                            .into(),
                    ));
                }
                let expected_block_crc = read_u32(&bytes, &mut offset, "posting_block_crc")?;
                let _reserved = read_u32(&bytes, &mut offset, "posting_block_reserved")?;
                let start = offset;
                let block = read_exact(&bytes, &mut offset, block_len, "posting_block")?;
                if crc32fast::hash(block) != expected_block_crc {
                    return Err(TriviumError::CorruptedFile(
                        "属性索引 posting block CRC32 不匹配 (Property posting block CRC32 mismatch)"
                            .into(),
                    ));
                }
                Some(start..offset)
            } else {
                None
            };
            let raw_ids = posting_range.as_ref().map(|range| &bytes[range.clone()]);
            let mut ids = if prefer_mmap_postings && version >= 3 && kind == 0 {
                Vec::new()
            } else {
                Vec::with_capacity(id_count)
            };
            let mut previous = 0;
            for index in 0..id_count {
                let id = if let Some(raw) = raw_ids {
                    let start = index * 8;
                    u64::from_le_bytes(raw[start..start + 8].try_into().map_err(|_| {
                        TriviumError::CorruptedFile("属性索引 NodeId 块无效".into())
                    })?)
                } else {
                    read_u64(&bytes, &mut offset, "node_id")?
                };
                if id == 0 || previous >= id && index > 0 {
                    return Err(TriviumError::CorruptedFile(
                        "属性索引 NodeId 无效或未严格排序 (Invalid or unsorted property index NodeId)".into(),
                    ));
                }
                previous = id;
                if !(prefer_mmap_postings && version >= 3 && kind == 0) {
                    ids.push(id);
                }
            }
            if prefer_mmap_postings && version >= 3 && kind == 0 {
                mapped_blocks.insert(
                    (field.clone(), key.clone()),
                    posting_range.expect("v3 posting 必须有范围"),
                );
                mapped_posting_entries = mapped_posting_entries.saturating_add(id_count);
            } else if entries.insert(key, ids).is_some() {
                return Err(TriviumError::CorruptedFile(
                    "属性索引包含重复键 (Property index contains duplicate keys)".into(),
                ));
            }
        }
        all_mapped_blocks.extend(mapped_blocks);
        all_mapped_posting_entries =
            all_mapped_posting_entries.saturating_add(mapped_posting_entries);
        if unique {
            if kind == 0 {
                registry.unique_indexes.insert(field.clone());
            } else {
                registry
                    .unique_composite_indexes
                    .insert(CompositeDefinition(
                        field.split('\0').map(str::to_owned).collect(),
                    ));
            }
        }
        if kind == 0 && !(prefer_mmap_postings && version >= 3) {
            registry.indexes.insert(
                field,
                HashPropertyIndex {
                    entries: entries.into_iter().collect(),
                },
            );
        } else if kind == 1 {
            let mut art_entries = ArtMap::default();
            for (key, ids) in entries {
                art_entries.insert(key.0, ids);
            }
            registry.ordered_indexes.insert(
                field,
                OrderedPropertyIndex {
                    entries: art_entries,
                },
            );
        } else if kind == 2 {
            let mut art_entries = ArtMap::default();
            for (key, ids) in entries {
                art_entries.insert(key.0, ids);
            }
            registry.composite_indexes.insert(
                CompositeDefinition(field.split('\0').map(str::to_owned).collect()),
                CompositePropertyIndex {
                    entries: art_entries,
                },
            );
        } else if kind == 3 {
            registry.bitmap_indexes.insert(
                field,
                BitmapPropertyIndex {
                    entries: entries
                        .into_iter()
                        .map(|(key, ids)| (key, ids.into_iter().collect()))
                        .collect(),
                },
            );
        } else if kind == 4 {
            let mut ngram_entries = HashMap::new();
            for (key, ids) in entries {
                let gram = String::from_utf8(key.0)
                    .map_err(|_| TriviumError::CorruptedFile("N-gram 索引键不是 UTF-8".into()))?;
                ngram_entries.insert(gram, ids);
            }
            registry.ngram_indexes.insert(
                field,
                NgramPropertyIndex {
                    entries: ngram_entries,
                },
            );
        }
    }
    if offset != payload_end {
        return Err(TriviumError::CorruptedFile(
            "属性索引包含尾部垃圾数据 (Property index contains trailing bytes)".into(),
        ));
    }
    if !all_mapped_blocks.is_empty() {
        registry.mapped = Some(Arc::new(MappedPostingStore {
            mapped_bytes: mmap.len(),
            mmap,
            blocks: all_mapped_blocks,
            posting_entries: all_mapped_posting_entries,
        }));
    }
    Ok(Some(registry))
}

#[cfg(test)]
mod tests {
    use super::{PropertyIndexRegistry, PropertyKey};
    use serde_json::json;
    use std::cmp::Ordering;

    #[test]
    fn 稳定键区分_json_标量类型() {
        assert_ne!(
            PropertyKey::from_json(&json!(1)),
            PropertyKey::from_json(&json!("1"))
        );
        assert_ne!(
            PropertyKey::from_json(&json!(true)),
            PropertyKey::from_json(&json!(1))
        );
        assert!(PropertyKey::from_json(&json!([])).is_none());
        assert!(PropertyKey::from_json(&json!({})).is_none());
    }

    #[test]
    fn 复合索引支持最长左前缀且拒绝跳列() {
        let payloads = [
            (1, json!({"tenant": "a", "kind": "x", "year": 2025})),
            (2, json!({"tenant": "a", "kind": "y", "year": 2026})),
            (3, json!({"tenant": "b", "kind": "x", "year": 2025})),
        ];
        let mut registry = PropertyIndexRegistry::default();
        registry.register_composite(
            &["tenant".into(), "kind".into(), "year".into()],
            payloads.iter().map(|(id, payload)| (*id, payload)),
        );

        assert_eq!(
            registry.composite_lookup(&[("tenant".into(), json!("a"))]),
            Some((vec!["tenant".into()], vec![1, 2]))
        );
        assert_eq!(
            registry
                .composite_lookup(&[("kind".into(), json!("x")), ("tenant".into(), json!("a")),]),
            Some((vec!["tenant".into(), "kind".into()], vec![1]))
        );
        assert_eq!(
            registry.composite_range_lookup(
                &[("tenant".into(), json!("a")), ("kind".into(), json!("x"))],
                "year",
                Ordering::Greater,
                true,
                &json!(2025),
                false,
                None,
            ),
            Some((vec!["tenant".into(), "kind".into(), "year".into()], vec![1]))
        );
        assert_eq!(
            registry.composite_lookup(&[("kind".into(), json!("x"))]),
            None
        );
    }

    #[test]
    fn 浮点零使用统一键() {
        assert_eq!(
            PropertyKey::from_json(&json!(0.0)),
            PropertyKey::from_json(&json!(-0.0))
        );
    }
}
