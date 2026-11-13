//! 稀疏文本召回索引：Aho-Corasick 精确关键词 + BM25 2-Gram。
//!
//! 关键词目录负责高精度锚点，BM25 维护词频、文档长度和全局统计，两路结果由上层
//! 混合检索管线融合。快照包含独立魔数/版本；增删节点必须同步
//! 清理所有 posting，防止 tombstone 文档在重启后重新出现。

use crate::node::NodeId;
use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufReader, BufWriter};
use std::path::Path;
use std::sync::OnceLock;

pub(crate) mod analyzer;
pub(crate) mod positions;
pub use analyzer::TEXT_ANALYZER_ID;
use analyzer::tokenize;

const TEXT_INDEX_MAGIC: &[u8; 4] = b"TIDX";
pub const TEXT_INDEX_VERSION: u32 = 2;

/// 完整重建时显式提供的节点正文与关键词。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextIndexDocument {
    pub id: NodeId,
    pub text: String,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Serialize)]
struct TextIndexSnapshotRef<'a> {
    keyword_to_nodes: &'a HashMap<String, Vec<NodeId>>,
    bm25_tf: &'a HashMap<String, HashMap<NodeId, usize>>,
    doc_lengths: &'a HashMap<NodeId, usize>,
}

#[derive(Deserialize)]
struct TextIndexSnapshot {
    keyword_to_nodes: HashMap<String, Vec<NodeId>>,
    bm25_tf: HashMap<String, HashMap<NodeId, usize>>,
    doc_lengths: HashMap<NodeId, usize>,
}

struct CompiledKeywords {
    matcher: AhoCorasick,
    keywords: Vec<String>,
    bytes: usize,
}

/// 综合文本搜索引擎：AC自动机 (精准关键词触发) + BM25 (大段落兜底打分)
///
/// 作为一个可选项，它与稠密向量搜索（Dense）形成完全互补，
/// 构成了 TriviumDB 的 混合检索（Hybrid Search）闭环机制。
#[derive(Default)]
pub struct TextIndex {
    // === 1. AC 自动机（特征锚点） ===
    // 用于精确捕获非常短、但置信度极高的指代特征词
    ac_matcher: OnceLock<Option<CompiledKeywords>>,
    keyword_to_nodes: HashMap<String, Vec<NodeId>>,
    // 只记录注册过关键词的节点，删除无关键词节点时不扫描整个词典。
    keyword_nodes: HashSet<NodeId>,

    // === 2. BM25 稀疏倒排索引 ===
    // 基于 2-Gram 滑动窗口的轻量级实现，兼容中英无分词器环境
    // Term -> NodeId -> TF (Term Frequency)
    bm25_tf: HashMap<String, HashMap<NodeId, usize>>,
    // Document Lengths (节点对应的文档长度)
    doc_lengths: HashMap<NodeId, usize>,
    avg_dl: f32,
    total_docs: usize,
    total_tokens: u128,
    keyword_bytes: usize,
    bm25_bytes: usize,
}

impl TextIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// 清空并准备重建
    pub fn clear(&mut self) {
        self.ac_matcher.take();
        self.keyword_to_nodes.clear();
        self.keyword_nodes.clear();
        self.bm25_tf.clear();
        self.doc_lengths.clear();
        self.avg_dl = 0.0;
        self.total_docs = 0;
        self.total_tokens = 0;
        self.keyword_bytes = 0;
        self.bm25_bytes = 0;
    }

    /// 注册一个高权重短元特征汇聚点（精准提取并置信度极高）
    pub fn add_keyword(&mut self, id: NodeId, keyword: &str) {
        let kw = keyword.to_lowercase();
        if kw.is_empty() {
            return;
        }
        let new_keyword = !self.keyword_to_nodes.contains_key(&kw);
        self.keyword_nodes.insert(id);
        let before = self
            .keyword_to_nodes
            .get_key_value(&kw)
            .map_or(0, |(term, ids)| term.capacity() + ids.capacity() * 8);
        let nodes = self.keyword_to_nodes.entry(kw.clone()).or_default();
        if let Err(position) = nodes.binary_search(&id) {
            nodes.insert(position, id);
        }
        let after = self
            .keyword_to_nodes
            .get_key_value(&kw)
            .map_or(0, |(term, ids)| term.capacity() + ids.capacity() * 8);
        self.keyword_bytes = self
            .keyword_bytes
            .saturating_sub(before)
            .saturating_add(after);
        if new_keyword {
            self.ac_matcher.take();
        }
    }

    /// 注册一段长文本：ASCII字母和数字按词切分，CJK文本使用字符2-Gram。
    pub fn add_text(&mut self, id: NodeId, text: &str) {
        let tokens = tokenize(text);
        let mut local_tf = HashMap::new();
        for token in &tokens {
            *local_tf.entry(token.clone()).or_insert(0) += 1;
        }
        self.add_term_frequencies(id, local_tf, tokens.len());
    }

    pub(crate) fn add_term_frequencies(
        &mut self,
        id: NodeId,
        local_tf: HashMap<String, usize>,
        dl: usize,
    ) {
        self.remove_document(id);
        if dl == 0 {
            return;
        }
        self.doc_lengths.insert(id, dl);
        self.total_tokens += dl as u128;

        for (token, tf) in local_tf {
            let before = self
                .bm25_tf
                .get_key_value(&token)
                .map_or(0, |(term, docs)| term.capacity() + docs.capacity() * 16);
            self.bm25_tf
                .entry(token.clone())
                .or_default()
                .insert(id, tf);
            let after = self
                .bm25_tf
                .get_key_value(&token)
                .map_or(0, |(term, docs)| term.capacity() + docs.capacity() * 16);
            self.bm25_bytes = self.bm25_bytes.saturating_sub(before).saturating_add(after);
        }
        self.refresh_statistics();
    }

    fn refresh_statistics(&mut self) {
        self.total_docs = self.doc_lengths.len();
        self.avg_dl = if self.total_docs == 0 {
            0.0
        } else {
            self.total_tokens as f32 / self.total_docs as f32
        };
    }

    fn remove_document(&mut self, id: NodeId) {
        if let Some(length) = self.doc_lengths.remove(&id) {
            self.total_tokens -= length as u128;
            self.bm25_tf.retain(|term, documents| {
                let before = term.capacity() + documents.capacity() * 16;
                documents.remove(&id);
                let after = if documents.is_empty() {
                    0
                } else {
                    term.capacity() + documents.capacity() * 16
                };
                self.bm25_bytes = self.bm25_bytes.saturating_sub(before).saturating_add(after);
                !documents.is_empty()
            });
            self.refresh_statistics();
        }
    }

    /// 清理节点的正文、关键词与统计；重复删除保持幂等。
    pub fn remove_node(&mut self, id: NodeId) {
        self.remove_document(id);
        if !self.keyword_nodes.remove(&id) {
            return;
        }
        let before = self.keyword_to_nodes.len();
        self.keyword_to_nodes.retain(|term, nodes| {
            let before = term.capacity() + nodes.capacity() * 8;
            nodes.retain(|node| *node != id);
            let after = if nodes.is_empty() {
                0
            } else {
                term.capacity() + nodes.capacity() * 8
            };
            self.keyword_bytes = self
                .keyword_bytes
                .saturating_sub(before)
                .saturating_add(after);
            !nodes.is_empty()
        });
        if before != self.keyword_to_nodes.len() {
            self.ac_matcher.take();
        }
    }

    /// 加载历史快照时清理已删除节点，并规范化关键词 posting。
    pub(crate) fn retain_nodes(&mut self, keep: impl Fn(NodeId) -> bool) {
        self.doc_lengths.retain(|id, _| keep(*id));
        self.bm25_tf.retain(|_, documents| {
            documents.retain(|id, _| self.doc_lengths.contains_key(id));
            !documents.is_empty()
        });
        self.keyword_to_nodes.retain(|keyword, nodes| {
            nodes.retain(|id| keep(*id));
            nodes.sort_unstable();
            nodes.dedup();
            !keyword.is_empty() && !nodes.is_empty()
        });
        self.keyword_nodes = self.keyword_to_nodes.values().flatten().copied().collect();
        self.keyword_bytes = self
            .keyword_to_nodes
            .iter()
            .map(|(term, ids)| term.capacity() + ids.capacity() * 8)
            .sum();
        self.bm25_bytes = self
            .bm25_tf
            .iter()
            .map(|(term, docs)| term.capacity() + docs.capacity() * 16)
            .sum();
        self.ac_matcher.take();
        self.total_tokens = self
            .doc_lengths
            .values()
            .map(|length| *length as u128)
            .sum();
        self.refresh_statistics();
    }

    /// 全量构建索引 (编译 AC，计算平均文档长度与频次基数)
    pub fn build(&mut self) {
        self.rebuild_runtime();
    }

    pub(crate) fn try_build(&mut self) -> crate::error::Result<()> {
        self.rebuild_runtime();
        if !self.keyword_to_nodes.is_empty() && self.compiled_keywords().is_none() {
            return Err(crate::error::TriviumError::InvalidInput(
                "关键词自动机构建失败".into(),
            ));
        }
        Ok(())
    }

    fn rebuild_runtime(&mut self) {
        self.compiled_keywords();
        self.total_tokens = self
            .doc_lengths
            .values()
            .map(|length| *length as u128)
            .sum();
        self.refresh_statistics();
    }

    fn compiled_keywords(&self) -> Option<&CompiledKeywords> {
        self.ac_matcher
            .get_or_init(|| {
                let mut keywords: Vec<_> = self.keyword_to_nodes.keys().cloned().collect();
                keywords.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
                if keywords.is_empty() {
                    return None;
                }
                AhoCorasickBuilder::new()
                    .match_kind(MatchKind::LeftmostLongest)
                    .build(&keywords)
                    .ok()
                    .map(|matcher| {
                        let bytes = matcher.memory_usage()
                            + keywords.capacity() * std::mem::size_of::<String>()
                            + keywords.iter().map(String::capacity).sum::<usize>();
                        CompiledKeywords {
                            matcher,
                            keywords,
                            bytes,
                        }
                    })
            })
            .as_ref()
    }

    pub fn is_empty(&self) -> bool {
        self.keyword_to_nodes.is_empty() && self.bm25_tf.is_empty()
    }

    pub fn estimated_memory_bytes(&self) -> usize {
        self.keyword_bytes
            + self.bm25_bytes
            + self
                .ac_matcher
                .get()
                .and_then(Option::as_ref)
                .map_or(0, |compiled| compiled.bytes)
            + self.keyword_nodes.capacity() * std::mem::size_of::<NodeId>()
            + self.doc_lengths.capacity()
                * (std::mem::size_of::<NodeId>() + std::mem::size_of::<usize>())
    }

    pub fn save_to_file(&self, path: &Path) -> std::io::Result<()> {
        if self.is_empty() {
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            return Ok(());
        }
        let tmp = path.with_extension("text.tmp");
        let mut writer = BufWriter::new(std::fs::File::create(&tmp)?);
        use std::io::Write;
        writer.write_all(TEXT_INDEX_MAGIC)?;
        writer.write_all(&TEXT_INDEX_VERSION.to_le_bytes())?;
        bincode::serialize_into(
            &mut writer,
            &TextIndexSnapshotRef {
                keyword_to_nodes: &self.keyword_to_nodes,
                bm25_tf: &self.bm25_tf,
                doc_lengths: &self.doc_lengths,
            },
        )
        .map_err(std::io::Error::other)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        crate::storage::fs::robust_rename_and_sync(&tmp, path)
    }

    pub fn load_from_file(path: &Path) -> std::io::Result<Self> {
        use bincode::Options;
        use std::io::Read;
        let file = std::fs::File::open(path)?;
        let limit = file.metadata()?.len();
        let mut reader = BufReader::new(file);
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if &magic != TEXT_INDEX_MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "TextIndex 魔数无效",
            ));
        }
        let mut version = [0u8; 4];
        reader.read_exact(&mut version)?;
        if u32::from_le_bytes(version) != TEXT_INDEX_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "TextIndex 版本不受支持",
            ));
        }
        let snapshot: TextIndexSnapshot = bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(limit.saturating_sub(8))
            .deserialize_from(&mut reader)
            .map_err(std::io::Error::other)?;
        let mut trailing = [0u8; 1];
        if reader.read(&mut trailing)? != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "文本索引存在尾部数据",
            ));
        }
        if snapshot
            .doc_lengths
            .iter()
            .any(|(id, length)| *id == 0 || *length == 0)
            || snapshot.bm25_tf.iter().any(|(term, nodes)| {
                term.is_empty() || nodes.iter().any(|(id, tf)| *id == 0 || *tf == 0)
            })
            || snapshot
                .keyword_to_nodes
                .iter()
                .any(|(keyword, nodes)| keyword.is_empty() || nodes.contains(&0))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "文本索引的节点、词频或文档长度无效",
            ));
        }
        let mut index = Self {
            ac_matcher: OnceLock::new(),
            keyword_to_nodes: snapshot.keyword_to_nodes,
            keyword_nodes: HashSet::new(),
            bm25_tf: snapshot.bm25_tf,
            doc_lengths: snapshot.doc_lengths,
            avg_dl: 0.0,
            total_docs: 0,
            total_tokens: 0,
            keyword_bytes: 0,
            bm25_bytes: 0,
        };
        index.retain_nodes(|_| true);
        Ok(index)
    }

    pub(crate) fn document_count(&self) -> usize {
        self.total_docs
    }

    pub(crate) fn average_length(&self) -> f32 {
        self.avg_dl
    }

    /// 执行 BM25 检索，返回命中节点的原始相似度得分
    pub fn search_bm25(&self, query: &str, k1: f32, b: f32) -> HashMap<NodeId, f32> {
        let mut results = HashMap::new();
        let outcome = self.visit_bm25(query, k1, b, |id, score| {
            *results.entry(id).or_insert(0.) += score;
            Ok::<(), std::convert::Infallible>(())
        });
        match outcome {
            Ok(()) => results,
            Err(never) => match never {},
        }
    }

    /// 调用方可在每个posting累加前检查预算、取消与范围，评分顺序复用原实现。
    pub(crate) fn visit_bm25<E>(
        &self,
        query: &str,
        k1: f32,
        b: f32,
        emit: impl FnMut(NodeId, f32) -> std::result::Result<(), E>,
    ) -> std::result::Result<(), E> {
        if self.total_docs == 0 {
            return Ok(());
        }

        let tokens = tokenize(query);
        self.visit_bm25_terms(tokens.iter().map(String::as_str), k1, b, emit)
    }

    /// 已分析词项复用相同评分，调用方可以在自身预算内完成一次分词。
    pub(crate) fn visit_bm25_terms<'a, E>(
        &self,
        terms: impl Iterator<Item = &'a str>,
        k1: f32,
        b: f32,
        mut emit: impl FnMut(NodeId, f32) -> std::result::Result<(), E>,
    ) -> std::result::Result<(), E> {
        if self.total_docs == 0 {
            return Ok(());
        }
        let mut tokens = terms.collect::<Vec<_>>();
        if tokens.is_empty() {
            return Ok(());
        }

        tokens.sort_unstable();
        tokens.dedup();

        let n = self.total_docs as f32;
        let avg_dl = self.avg_dl;

        for token in tokens {
            if let Some(docs) = self.bm25_tf.get(token) {
                let df = docs.len() as f32;
                // IDF 平滑 (BM25 标准)
                let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();

                for (&id, &tf) in docs {
                    let dl = *self.doc_lengths.get(&id).unwrap_or(&0) as f32;
                    let tf_f32 = tf as f32;
                    // Okapi BM25 打分公式
                    let tf_norm =
                        (tf_f32 * (k1 + 1.0)) / (tf_f32 + k1 * (1.0 - b + b * dl / avg_dl));
                    emit(id, idf * tf_norm)?;
                }
            }
        }
        Ok(())
    }

    /// 执行 AC 自动机精准锚点激发
    pub fn search_ac(&self, query: &str) -> HashMap<NodeId, f32> {
        let mut results = HashMap::new();
        if let Some(compiled) = self.compiled_keywords() {
            let query_lower = query.to_lowercase();
            for mat in compiled.matcher.find_iter(&query_lower) {
                let kw = &compiled.keywords[mat.pattern()];
                if let Some(nodes) = self.keyword_to_nodes.get(kw) {
                    for &id in nodes {
                        *results.entry(id).or_insert(0.0) += 1.0;
                    }
                }
            }
        }
        results
    }
}

#[cfg(test)]
mod ledger_tests {
    use super::*;

    fn assert_allocations(index: &TextIndex) {
        // 独立遍历实际容器，核对增量账本；序列化重载允许容量不同。
        let keywords = index
            .keyword_to_nodes
            .iter()
            .map(|(key, ids)| key.capacity() + ids.capacity() * std::mem::size_of::<NodeId>())
            .sum::<usize>();
        let terms = index
            .bm25_tf
            .iter()
            .map(|(key, ids)| {
                key.capacity() + ids.capacity() * std::mem::size_of::<(NodeId, usize)>()
            })
            .sum::<usize>();
        assert_eq!(index.keyword_bytes, keywords);
        assert_eq!(index.bm25_bytes, terms);
        let compiled = index
            .ac_matcher
            .get()
            .and_then(Option::as_ref)
            .map_or(0, |ac| {
                ac.matcher.memory_usage()
                    + ac.keywords.capacity() * std::mem::size_of::<String>()
                    + ac.keywords.iter().map(String::capacity).sum::<usize>()
            });
        assert_eq!(
            index.estimated_memory_bytes(),
            keywords
                + terms
                + compiled
                + index.keyword_nodes.capacity() * std::mem::size_of::<NodeId>()
                + index.doc_lengths.capacity() * std::mem::size_of::<(NodeId, usize)>()
        );
    }

    #[test]
    fn 文本账本覆盖编译替换删除清空与重载() {
        let mut index = TextIndex::new();
        for id in 1..=32 {
            index.add_keyword(id, "共享锚点");
            index.add_keyword(id, &format!("keyword{id}"));
            index.add_text(id, &format!("alpha beta word{id} 中文索引"));
            assert_allocations(&index);
        }
        index.add_keyword(1, "共享锚点");
        index.build();
        assert_allocations(&index);
        index.add_text(1, "alpha alpha gamma");
        assert_allocations(&index);
        for id in [1, 1, 17, 32] {
            index.remove_node(id);
            assert_allocations(&index);
        }
        index.retain_nodes(|id| id < 20);
        assert_allocations(&index);
        let root = std::env::temp_dir().join(format!("tdb-text-ledger-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("sample.text");
        index.save_to_file(&path).unwrap();
        let mut loaded = TextIndex::load_from_file(&path).unwrap();
        assert_allocations(&loaded);
        assert_eq!(loaded.search_ac("共享锚点"), index.search_ac("共享锚点"));
        assert_eq!(
            loaded.search_bm25("alpha", 1.2, 0.75),
            index.search_bm25("alpha", 1.2, 0.75)
        );
        loaded.build();
        assert_allocations(&loaded);
        loaded.clear();
        assert_allocations(&loaded);
        std::fs::remove_dir_all(root).unwrap();
    }
}
