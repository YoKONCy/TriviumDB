//! 索引构建与查询的公开配置。

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuildConfig {
    pub m: usize,
    pub ef_construction: usize,
    /// Vamana α 参数：1.0=严格剪枝，1.2=推荐值，越大图越密
    pub alpha: f32,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            m: 16,
            ef_construction: 128,
            alpha: 1.2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchConfig {
    pub top_k: usize,
    pub ef_search: usize,
    pub rerank_limit: Option<usize>,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            top_k: 10,
            ef_search: 64,
            rerank_limit: None,
        }
    }
}

impl SearchConfig {
    #[inline]
    pub(crate) fn rerank_limit(&self) -> usize {
        self.rerank_limit.unwrap_or(self.ef_search).max(self.top_k)
    }
}

/// 优先读取当前配置名，并兼容早期环境变量。
pub(crate) fn environment_flag(name: &str, previous_name: &str) -> bool {
    std::env::var(name)
        .or_else(|_| std::env::var(previous_name))
        .is_ok_and(|value| value == "1")
}
