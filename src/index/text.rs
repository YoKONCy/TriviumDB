use crate::node::NodeId;
use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use std::collections::HashMap;

/// 综合文本搜索引擎：AC自动机 (精准关键词触发) + BM25 (大段落兜底打分)
///
/// 作为一个可选项，它与稠密向量搜索（Dense）形成完全互补，
/// 构成了 TriviumDB 的 混合检索（Hybrid Search）闭环机制。
#[derive(Default)]
pub struct TextIndex {
    // === 1. AC 自动机（特征锚点） ===
    // 用于精确捕获非常短、但置信度极高的指代特征词
    ac_matcher: Option<AhoCorasick>,
    keywords: Vec<String>,
    keyword_to_nodes: HashMap<String, Vec<NodeId>>,

    // === 2. BM25 稀疏倒排索引 ===
    // 基于 2-Gram 滑动窗口的轻量级实现，兼容中英无分词器环境
    // Term -> NodeId -> TF (Term Frequency)
    bm25_tf: HashMap<String, HashMap<NodeId, usize>>,
    // Document Lengths (节点对应的文档长度)
    doc_lengths: HashMap<NodeId, usize>,
    avg_dl: f32,
    total_docs: usize,
}

impl TextIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// 清空并准备重建
    pub fn clear(&mut self) {
        self.ac_matcher = None;
        self.keywords.clear();
        self.keyword_to_nodes.clear();
        self.bm25_tf.clear();
        self.doc_lengths.clear();
        self.avg_dl = 0.0;
        self.total_docs = 0;
    }

    /// 注册一个高权重短元特征汇聚点（精准提取并置信度极高）
    pub fn add_keyword(&mut self, id: NodeId, keyword: &str) {
        let kw = keyword.to_lowercase();
        self.keyword_to_nodes
            .entry(kw.clone())
            .or_default()
            .push(id);
    }

    /// 注册一段长文本，建立 BM25 2-Gram 倒排
    pub fn add_text(&mut self, id: NodeId, text: &str) {
        let text_lower = text.to_lowercase();
        let chars: Vec<char> = text_lower.chars().collect();
        if chars.is_empty() {
            return;
        }

        // 我们通过 2-Gram 滑动窗口进行纯净的稀疏表征
        let tokens: Vec<String> = if chars.len() > 1 {
            chars.windows(2).map(|w| w.iter().collect()).collect()
        } else {
            vec![text_lower]
        };

        let mut local_tf = HashMap::new();
        for token in &tokens {
            *local_tf.entry(token.clone()).or_insert(0) += 1;
        }

        let dl = tokens.len();
        self.doc_lengths.insert(id, dl);

        for (token, tf) in local_tf {
            self.bm25_tf.entry(token).or_default().insert(id, tf);
        }
    }

    /// 全量构建索引 (编译 AC，计算平均文档长度与频次基数)
    pub fn build(&mut self) {
        // 1. 构建 AC
        let mut keys: Vec<String> = self.keyword_to_nodes.keys().cloned().collect();
        keys.sort_by(|a, b| b.len().cmp(&a.len())); // 优先匹配长词，防止截断
        if !keys.is_empty() {
            if let Ok(ac) = AhoCorasickBuilder::new()
                .match_kind(MatchKind::LeftmostLongest)
                .build(&keys)
            {
                self.ac_matcher = Some(ac);
                self.keywords = keys;
            }
        }

        // 2. 计算 BM25 AvgDL
        self.total_docs = self.doc_lengths.len();
        if self.total_docs > 0 {
            let sum_dl: usize = self.doc_lengths.values().sum();
            self.avg_dl = sum_dl as f32 / self.total_docs as f32;
        }
    }

    /// 执行 BM25 检索，返回命中节点的原始相似度得分
    pub fn search_bm25(&self, query: &str, k1: f32, b: f32) -> HashMap<NodeId, f32> {
        let mut results = HashMap::new();
        if self.total_docs == 0 {
            return results;
        }

        let query_lower = query.to_lowercase();
        let chars: Vec<char> = query_lower.chars().collect();
        if chars.is_empty() {
            return results;
        }

        // 查询向量化
        let tokens: Vec<String> = if chars.len() > 1 {
            chars.windows(2).map(|w| w.iter().collect()).collect()
        } else {
            vec![query_lower]
        };

        let mut query_tf = HashMap::new();
        for token in &tokens {
            *query_tf.entry(token).or_insert(0) += 1;
        }

        let n = self.total_docs as f32;
        let avg_dl = self.avg_dl;

        for (token, _q_tf) in query_tf {
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
                    *results.entry(id).or_insert(0.0) += idf * tf_norm;
                }
            }
        }
        results
    }

    /// 执行 AC 自动机精准锚点激发
    pub fn search_ac(&self, query: &str) -> HashMap<NodeId, f32> {
        let mut results = HashMap::new();
        if let Some(ac) = &self.ac_matcher {
            let query_lower = query.to_lowercase();
            for mat in ac.find_iter(&query_lower) {
                let kw = &self.keywords[mat.pattern()];
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
