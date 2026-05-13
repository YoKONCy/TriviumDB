//! 检索管线 Hook 系统
//!
//! 提供 6 个管线关键阶段的自定义扩展点，允许开发者在构建 RAG 系统时：
//! - 注入自定义字段、改写查询参数
//! - 替代/增强内置召回（对接外部 C++ FAISS / ScaNN 等高性能模块）
//! - 自定义评分调权、业务逻辑过滤
//! - 外置 Cross-Encoder 精排 / ONNX 推理
//! - 结果增强、统计埋点、回传自定义数据
//!
//! # 设计原则
//!
//! 1. **零开销可选**: 默认 `NoopHook`，未注册 Hook 时编译器内联消除全部调用
//! 2. **按需覆写**: 所有方法都有默认空实现，开发者只需覆写感兴趣的阶段
//! 3. **FFI 友好**: `#[repr(C)]` 数据结构 + `extern "C"` 函数签名，支持 C/C++ 动态库

use crate::database::SearchConfig;
use crate::node::SearchHit;

// ═══════════════════════════════════════════════════════
//  Hook 上下文：管线各阶段之间的共享状态容器
// ═══════════════════════════════════════════════════════

/// Hook 上下文，在管线各阶段之间传递共享状态。
///
/// 开发者可以在 `on_pre_search` 中写入自定义数据，
/// 后续阶段（如 `on_rerank`）可以读取这些数据。
#[derive(Debug, Clone)]
pub struct HookContext {
    /// 开发者自定义的附加数据（任意 JSON）
    ///
    /// 典型用途：
    /// - 传入用户身份 `{"user_id": "u_12345"}`
    /// - 传入会话上下文 `{"session": "abc", "history_ids": [1,2,3]}`
    /// - 回传统计信息 `{"latency_ms": 12.5, "recall_count": 100}`
    pub custom_data: serde_json::Value,

    /// 管线阶段计时统计（自动填充）
    ///
    /// 格式: `[(阶段名称, 耗时)]`，如 `[("vector_recall", 3.2ms), ("graph_expand", 1.1ms)]`
    pub stage_timings: Vec<(String, std::time::Duration)>,

    /// 当前查询是否被 Hook 提前终止
    ///
    /// 若在 `on_pre_search` 中设为 `true`，管线将跳过后续所有阶段，
    /// 直接返回 `on_pre_search` 阶段已有的结果。
    pub abort: bool,
}

impl Default for HookContext {
    fn default() -> Self {
        Self::new()
    }
}

impl HookContext {
    pub fn new() -> Self {
        Self {
            custom_data: serde_json::Value::Null,
            stage_timings: Vec::new(),
            abort: false,
        }
    }

    /// 记录一个阶段的耗时
    pub fn record_timing(&mut self, stage: impl Into<String>, elapsed: std::time::Duration) {
        self.stage_timings.push((stage.into(), elapsed));
    }
}

// ═══════════════════════════════════════════════════════
//  核心 Hook Trait
// ═══════════════════════════════════════════════════════

/// 检索管线 Hook Trait
///
/// 所有方法都有默认实现（no-op），开发者只需覆写感兴趣的阶段。
///
/// # 管线阶段与 Hook 点
///
/// ```text
///   查询输入
///       │
///   🔌 #1 on_pre_search        — 查询预处理
///       │
///   🔌 #2 on_custom_recall     — 自定义召回（可替代内置）
///       │
///   ┌── 内置召回管线 ──┐
///   │  L1 文本稀疏召回  │
///   │  L2 向量稠密召回  │
///   │  L3 布隆预过滤    │
///   └──────────────────┘
///       │
///   🔌 #3 on_post_recall       — 召回后处理
///       │
///   ┌── 认知管线 ──────┐
///   │  L4 FISTA 残差    │
///   │  L5 影子查询      │
///   └──────────────────┘
///       │
///   🔌 #4 on_pre_graph_expand  — 图扩散前
///       │
///   ┌── 图谱扩散 ──────┐
///   │  L6 PPR 扩散      │
///   │  L7 不应期/抑制    │
///   └──────────────────┘
///       │
///   🔌 #5 on_rerank            — 重排序
///       │
///   ┌── 多样性 ────────┐
///   │  L9 DPP 采样      │
///   └──────────────────┘
///       │
///   🔌 #6 on_post_search       — 最终后处理
///       │
///   返回结果
/// ```
pub trait SearchHook: Send + Sync {
    /// 🔌 Hook #1：查询预处理
    ///
    /// 在检索管线启动前调用。可以修改查询向量、调整配置参数、注入上下文数据。
    ///
    /// # 典型用途
    /// - 动态调整 `top_k`（先多召回再精排）
    /// - 向 `ctx` 注入用户画像 / 会话信息
    /// - 对查询向量做归一化或增强
    /// - 设置 `ctx.abort = true` 提前终止管线
    fn on_pre_search(
        &self,
        _query_vector: &mut Vec<f32>,
        _config: &mut SearchConfig,
        _ctx: &mut HookContext,
    ) {
        // 默认空实现
    }

    /// 🔌 Hook #2：自定义召回
    ///
    /// 返回 `Some(hits)` 表示**替代**内置召回结果，`None` 走默认管线。
    ///
    /// # 模式
    /// - **替代 (Replace)**: 返回 `Some(my_results)` 完全跳过内置召回
    /// - **增强 (Augment)**: 返回 `None`，在 `on_post_recall` 中合并额外结果
    ///
    /// # 典型用途
    /// - 对接外部 C++ FAISS / ScaNN 索引做粗排
    /// - 使用自定义 ANN 算法
    /// - 从外部缓存/服务拉取预计算结果
    fn on_custom_recall(
        &self,
        _query_vector: &[f32],
        _config: &SearchConfig,
        _ctx: &mut HookContext,
    ) -> Option<Vec<SearchHit>> {
        None // 默认不替代
    }

    /// 🔌 Hook #3：召回后处理
    ///
    /// 在内置召回（或自定义召回）完成后、图扩散之前调用。
    ///
    /// # 典型用途
    /// - 自定义评分调权（时间衰减、推荐权重等）
    /// - 业务逻辑过滤（去除已读内容、黑名单等）
    /// - 向 `SearchHit.payload` 注入额外字段
    /// - 合并来自外部数据源的额外候选
    fn on_post_recall(&self, _hits: &mut Vec<SearchHit>, _ctx: &mut HookContext) {
        // 默认空实现
    }

    /// 🔌 Hook #4：图扩散前拦截
    ///
    /// 在向量/文本召回的锚点结果送入 PPR 图扩散之前调用。
    ///
    /// # 典型用途
    /// - 修改种子集（添加/移除特定节点）
    /// - 注入来自外部知识图谱的额外种子
    /// - 根据业务逻辑动态调整扩散参数
    fn on_pre_graph_expand(&self, _seeds: &mut Vec<SearchHit>, _ctx: &mut HookContext) {
        // 默认空实现
    }

    /// 🔌 Hook #5：自定义重排序
    ///
    /// 在图扩散完成后、DPP 多样性采样之前调用。
    /// 返回 `Some(reranked)` 表示覆盖排序结果，`None` 走默认排序。
    ///
    /// # 典型用途
    /// - Cross-Encoder 精排
    /// - 外置 C++ ONNX Runtime 推理
    /// - 基于业务规则的自定义排序逻辑
    /// - 多路结果的 Reciprocal Rank Fusion (RRF)
    fn on_rerank(
        &self,
        _hits: &mut Vec<SearchHit>,
        _ctx: &mut HookContext,
    ) -> Option<Vec<SearchHit>> {
        None // 默认不覆盖
    }

    /// 🔌 Hook #6：最终后处理
    ///
    /// 在所有管线阶段完成后、结果返回给调用方之前调用。
    ///
    /// # 典型用途
    /// - 结果增强（添加摘要、翻译等）
    /// - 回传统计数据到 `ctx.custom_data`
    /// - 日志/埋点记录
    /// - 最终截断或格式化
    fn on_post_search(&self, _results: &mut Vec<SearchHit>, _ctx: &mut HookContext) {
        // 默认空实现
    }
}

// ═══════════════════════════════════════════════════════
//  内置 Hook 实现
// ═══════════════════════════════════════════════════════

/// 空操作 Hook：所有方法都是 no-op
///
/// 作为默认 Hook 使用，编译器会内联消除所有调用，实现真正的零开销。
pub struct NoopHook;

impl SearchHook for NoopHook {}

// ═══════════════════════════════════════════════════════
//  组合 Hook：链式调用多个 Hook
// ═══════════════════════════════════════════════════════

/// 组合 Hook：按注册顺序依次调用多个 Hook
///
/// 当开发者需要同时使用多个独立的 Hook 模块时（如一个做过滤、一个做重排），
/// 可以用 `CompositeHook` 将它们组合起来。
///
/// # 语义
/// - `on_pre_search`: 所有 Hook 依次调用
/// - `on_custom_recall`: 第一个返回 `Some` 的 Hook 生效
/// - `on_post_recall`: 所有 Hook 依次调用
/// - `on_pre_graph_expand`: 所有 Hook 依次调用
/// - `on_rerank`: 第一个返回 `Some` 的 Hook 生效
/// - `on_post_search`: 所有 Hook 依次调用
pub struct CompositeHook {
    hooks: Vec<Box<dyn SearchHook>>,
}

impl CompositeHook {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// 添加一个 Hook 到链末尾
    pub fn add(&mut self, hook: impl SearchHook + 'static) {
        self.hooks.push(Box::new(hook));
    }
}

impl Default for CompositeHook {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchHook for CompositeHook {
    fn on_pre_search(
        &self,
        query_vector: &mut Vec<f32>,
        config: &mut SearchConfig,
        ctx: &mut HookContext,
    ) {
        for hook in &self.hooks {
            hook.on_pre_search(query_vector, config, ctx);
            if ctx.abort {
                return;
            }
        }
    }

    fn on_custom_recall(
        &self,
        query_vector: &[f32],
        config: &SearchConfig,
        ctx: &mut HookContext,
    ) -> Option<Vec<SearchHit>> {
        // 第一个返回 Some 的 Hook 生效
        for hook in &self.hooks {
            if let Some(result) = hook.on_custom_recall(query_vector, config, ctx) {
                return Some(result);
            }
        }
        None
    }

    fn on_post_recall(&self, hits: &mut Vec<SearchHit>, ctx: &mut HookContext) {
        for hook in &self.hooks {
            hook.on_post_recall(hits, ctx);
        }
    }

    fn on_pre_graph_expand(&self, seeds: &mut Vec<SearchHit>, ctx: &mut HookContext) {
        for hook in &self.hooks {
            hook.on_pre_graph_expand(seeds, ctx);
        }
    }

    fn on_rerank(
        &self,
        hits: &mut Vec<SearchHit>,
        ctx: &mut HookContext,
    ) -> Option<Vec<SearchHit>> {
        for hook in &self.hooks {
            if let Some(result) = hook.on_rerank(hits, ctx) {
                return Some(result);
            }
        }
        None
    }

    fn on_post_search(&self, results: &mut Vec<SearchHit>, ctx: &mut HookContext) {
        for hook in &self.hooks {
            hook.on_post_search(results, ctx);
        }
    }
}

// ═══════════════════════════════════════════════════════
//  FFI 桥接层：支持 C/C++ 动态库扩展
// ═══════════════════════════════════════════════════════

/// C ABI 兼容的搜索命中结构体
///
/// 用于与 C/C++ 动态库交换数据。
/// 开发者可以用 C++ 编写高性能的召回/重排模块，
/// 编译为 `.dll` / `.so` 后通过 `FfiHook` 加载。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FfiSearchHit {
    pub id: u64,
    pub score: f32,
}

/// C 侧召回函数签名
///
/// ```c
/// int trivium_recall(
///     const float* query, size_t query_len, size_t top_k,
///     FfiSearchHit* out_hits, size_t* out_count
/// );
/// ```
pub type FfiRecallFn = unsafe extern "C" fn(
    query_ptr: *const f32,
    query_len: usize,
    top_k: usize,
    out_hits: *mut FfiSearchHit,
    out_count: *mut usize,
) -> i32;

/// C 侧重排函数签名
///
/// ```c
/// int trivium_rerank(FfiSearchHit* hits, size_t count);
/// ```
pub type FfiRerankFn = unsafe extern "C" fn(hits_ptr: *mut FfiSearchHit, hits_count: usize) -> i32;

/// FFI Hook：通过动态库加载 C/C++ 扩展模块
///
/// # 使用方法
///
/// ```rust,ignore
/// // C++ 侧编译为 libmy_plugin.so / my_plugin.dll
/// let hook = FfiHook::load("./libmy_plugin.so")?;
/// db.set_hook(hook);
/// ```
///
/// # C++ 侧实现示例
///
/// ```cpp
/// extern "C" int trivium_recall(
///     const float* query, size_t query_len, size_t top_k,
///     FfiSearchHit* out_hits, size_t* out_count
/// ) {
///     // 使用 FAISS / 自定义算法做检索
///     *out_count = /* 实际命中数 */;
///     return 0; // 0 = 成功
/// }
/// ```
pub struct FfiHook {
    /// 持有动态库句柄，防止提前卸载
    _lib: libloading::Library,
    recall_fn: Option<FfiRecallFn>,
    rerank_fn: Option<FfiRerankFn>,
}

// SAFETY: FFI 函数指针本身是 Send+Sync 安全的（它们是全局函数地址）
// 动态库在 FfiHook 生命周期内保持加载
unsafe impl Send for FfiHook {}
unsafe impl Sync for FfiHook {}

impl FfiHook {
    /// 从动态库文件加载 Hook
    ///
    /// 会尝试查找以下符号（均为可选）：
    /// - `trivium_recall`: 自定义召回函数
    /// - `trivium_rerank`: 自定义重排函数
    ///
    /// # 错误
    /// 当动态库文件不存在或无法加载时返回错误。
    pub fn load(path: &str) -> crate::error::Result<Self> {
        unsafe {
            let lib = libloading::Library::new(path).map_err(|e| {
                crate::error::TriviumError::HookLoadError(format!(
                    "无法加载外置 Hook 动态库 (Failed to load FFI Hook library) '{}': {}",
                    path, e
                ))
            })?;

            let recall_fn = lib.get::<FfiRecallFn>(b"trivium_recall").ok().map(|f| *f);
            let rerank_fn = lib.get::<FfiRerankFn>(b"trivium_rerank").ok().map(|f| *f);

            tracing::info!(
                "已加载外置 Hook 模块 (FFI Hook loaded): {} (recall={}, rerank={})",
                path,
                recall_fn.is_some(),
                rerank_fn.is_some()
            );

            Ok(Self {
                _lib: lib,
                recall_fn,
                rerank_fn,
            })
        }
    }
}

impl SearchHook for FfiHook {
    fn on_custom_recall(
        &self,
        query_vector: &[f32],
        config: &SearchConfig,
        _ctx: &mut HookContext,
    ) -> Option<Vec<SearchHit>> {
        let recall_fn = self.recall_fn?;

        // 预分配输出缓冲区（最多 top_k * 2 个候选）
        let buf_size = config.top_k * 2;
        let mut buf = vec![FfiSearchHit { id: 0, score: 0.0 }; buf_size];
        let mut count: usize = 0;

        // SAFETY: 调用 C 侧函数，指针有效性由 buf 的生命周期保证
        let ret = unsafe {
            (recall_fn)(
                query_vector.as_ptr(),
                query_vector.len(),
                config.top_k,
                buf.as_mut_ptr(),
                &mut count,
            )
        };

        if ret != 0 {
            tracing::warn!("FFI recall 函数返回错误码 (FFI recall returned error code): {}", ret);
            return None;
        }

        // 转换 FFI 结构体为 Rust SearchHit
        let hits: Vec<SearchHit> = buf[..count.min(buf_size)]
            .iter()
            .filter(|h| h.id != 0)
            .map(|h| SearchHit {
                id: h.id,
                score: h.score,
                payload: serde_json::Value::Null,
            })
            .collect();

        Some(hits)
    }

    fn on_rerank(
        &self,
        hits: &mut Vec<SearchHit>,
        _ctx: &mut HookContext,
    ) -> Option<Vec<SearchHit>> {
        let rerank_fn = self.rerank_fn?;

        // 转换为 FFI 格式
        let mut ffi_hits: Vec<FfiSearchHit> = hits
            .iter()
            .map(|h| FfiSearchHit {
                id: h.id,
                score: h.score,
            })
            .collect();

        // SAFETY: 调用 C 侧函数，原地修改分数
        let ret = unsafe { (rerank_fn)(ffi_hits.as_mut_ptr(), ffi_hits.len()) };

        if ret != 0 {
            tracing::warn!("FFI rerank 函数返回错误码 (FFI rerank returned error code): {}", ret);
            return None;
        }

        // 将 FFI 侧修改过的分数写回 SearchHit
        let mut reranked: Vec<SearchHit> = hits
            .iter()
            .zip(ffi_hits.iter())
            .map(|(original, ffi)| SearchHit {
                id: original.id,
                score: ffi.score,
                payload: original.payload.clone(),
            })
            .collect();

        // 按新分数降序排序
        reranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Some(reranked)
    }
}

// ═══════════════════════════════════════════════════════
//  单元测试
// ═══════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_hook_is_default() {
        let hook = NoopHook;
        let mut ctx = HookContext::new();
        let mut vec = vec![1.0, 2.0, 3.0];
        let mut config = SearchConfig::default();

        // 所有方法应该什么都不做
        hook.on_pre_search(&mut vec, &mut config, &mut ctx);
        assert_eq!(vec, vec![1.0, 2.0, 3.0]); // 未被修改
        assert!(!ctx.abort);

        assert!(hook.on_custom_recall(&vec, &config, &mut ctx).is_none());

        let mut hits = vec![];
        hook.on_post_recall(&mut hits, &mut ctx);
        hook.on_pre_graph_expand(&mut hits, &mut ctx);
        assert!(hook.on_rerank(&mut hits, &mut ctx).is_none());
        hook.on_post_search(&mut hits, &mut ctx);
    }

    #[test]
    fn test_hook_context() {
        let mut ctx = HookContext::new();
        assert!(ctx.custom_data.is_null());
        assert!(ctx.stage_timings.is_empty());
        assert!(!ctx.abort);

        ctx.custom_data = serde_json::json!({"user_id": "u_123"});
        ctx.record_timing("recall", std::time::Duration::from_millis(5));

        assert_eq!(ctx.custom_data["user_id"], "u_123");
        assert_eq!(ctx.stage_timings.len(), 1);
        assert_eq!(ctx.stage_timings[0].0, "recall");
    }

    /// 自定义 Hook 示例：对召回结果施加时间衰减
    struct TimeDecayHook {
        decay_rate: f32,
    }

    impl SearchHook for TimeDecayHook {
        fn on_post_recall(&self, hits: &mut Vec<SearchHit>, _ctx: &mut HookContext) {
            for hit in hits.iter_mut() {
                hit.score *= self.decay_rate;
            }
        }
    }

    #[test]
    fn test_custom_hook() {
        let hook = TimeDecayHook { decay_rate: 0.8 };
        let mut ctx = HookContext::new();
        let mut hits = vec![
            SearchHit {
                id: 1,
                score: 1.0,
                payload: serde_json::Value::Null,
            },
            SearchHit {
                id: 2,
                score: 0.5,
                payload: serde_json::Value::Null,
            },
        ];

        hook.on_post_recall(&mut hits, &mut ctx);
        assert!((hits[0].score - 0.8).abs() < 1e-6);
        assert!((hits[1].score - 0.4).abs() < 1e-6);
    }

    #[test]
    fn test_composite_hook() {
        struct BoostHook;
        impl SearchHook for BoostHook {
            fn on_post_recall(&self, hits: &mut Vec<SearchHit>, _ctx: &mut HookContext) {
                for hit in hits.iter_mut() {
                    hit.score *= 2.0;
                }
            }
        }

        struct FilterHook;
        impl SearchHook for FilterHook {
            fn on_post_recall(&self, hits: &mut Vec<SearchHit>, _ctx: &mut HookContext) {
                hits.retain(|h| h.score > 0.5);
            }
        }

        let mut composite = CompositeHook::new();
        composite.add(BoostHook);
        composite.add(FilterHook);

        let mut ctx = HookContext::new();
        let mut hits = vec![
            SearchHit {
                id: 1,
                score: 0.3, // 0.3 * 2 = 0.6 > 0.5 → 保留
                payload: serde_json::Value::Null,
            },
            SearchHit {
                id: 2,
                score: 0.2, // 0.2 * 2 = 0.4 < 0.5 → 过滤
                payload: serde_json::Value::Null,
            },
        ];

        composite.on_post_recall(&mut hits, &mut ctx);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, 1);
    }

    #[test]
    fn test_hook_context_default() {
        let ctx = HookContext::default();
        assert!(ctx.custom_data.is_null());
        assert!(!ctx.abort);
    }

    #[test]
    fn test_composite_hook_default() {
        let c = CompositeHook::default();
        assert!(c.hooks.is_empty());
    }

    #[test]
    fn test_composite_on_pre_search_abort() {
        struct AbortHook;
        impl SearchHook for AbortHook {
            fn on_pre_search(
                &self,
                _qv: &mut Vec<f32>,
                _cfg: &mut SearchConfig,
                ctx: &mut HookContext,
            ) {
                ctx.abort = true;
            }
        }

        struct NeverReachedHook;
        impl SearchHook for NeverReachedHook {
            fn on_pre_search(
                &self,
                qv: &mut Vec<f32>,
                _cfg: &mut SearchConfig,
                _ctx: &mut HookContext,
            ) {
                qv[0] = 999.0; // 不应被执行
            }
        }

        let mut composite = CompositeHook::new();
        composite.add(AbortHook);
        composite.add(NeverReachedHook);

        let mut ctx = HookContext::new();
        let mut vec = vec![1.0, 2.0];
        let mut config = SearchConfig::default();
        composite.on_pre_search(&mut vec, &mut config, &mut ctx);

        assert!(ctx.abort);
        assert_eq!(vec[0], 1.0, "abort 后第二个 hook 不应执行");
    }

    #[test]
    fn test_composite_on_custom_recall_first_wins() {
        struct RecallA;
        impl SearchHook for RecallA {
            fn on_custom_recall(
                &self,
                _qv: &[f32],
                _cfg: &SearchConfig,
                _ctx: &mut HookContext,
            ) -> Option<Vec<SearchHit>> {
                Some(vec![SearchHit {
                    id: 42,
                    score: 1.0,
                    payload: serde_json::Value::Null,
                }])
            }
        }

        struct RecallB;
        impl SearchHook for RecallB {
            fn on_custom_recall(
                &self,
                _qv: &[f32],
                _cfg: &SearchConfig,
                _ctx: &mut HookContext,
            ) -> Option<Vec<SearchHit>> {
                Some(vec![SearchHit {
                    id: 99,
                    score: 0.5,
                    payload: serde_json::Value::Null,
                }])
            }
        }

        let mut composite = CompositeHook::new();
        composite.add(RecallA);
        composite.add(RecallB);

        let mut ctx = HookContext::new();
        let config = SearchConfig::default();
        let result = composite.on_custom_recall(&[1.0], &config, &mut ctx);

        assert!(result.is_some());
        assert_eq!(result.unwrap()[0].id, 42, "第一个返回 Some 的 hook 应生效");
    }

    #[test]
    fn test_composite_on_custom_recall_all_none() {
        let composite = CompositeHook::new();
        let mut ctx = HookContext::new();
        let config = SearchConfig::default();
        assert!(
            composite
                .on_custom_recall(&[1.0], &config, &mut ctx)
                .is_none()
        );
    }

    #[test]
    fn test_composite_on_rerank_first_wins() {
        struct RerankHook;
        impl SearchHook for RerankHook {
            fn on_rerank(
                &self,
                hits: &mut Vec<SearchHit>,
                _ctx: &mut HookContext,
            ) -> Option<Vec<SearchHit>> {
                let mut reranked = hits.clone();
                reranked.reverse();
                Some(reranked)
            }
        }

        let mut composite = CompositeHook::new();
        composite.add(RerankHook);

        let mut ctx = HookContext::new();
        let mut hits = vec![
            SearchHit {
                id: 1,
                score: 1.0,
                payload: serde_json::Value::Null,
            },
            SearchHit {
                id: 2,
                score: 0.5,
                payload: serde_json::Value::Null,
            },
        ];
        let result = composite.on_rerank(&mut hits, &mut ctx);
        assert!(result.is_some());
        assert_eq!(result.unwrap()[0].id, 2);
    }

    #[test]
    fn test_composite_on_pre_graph_expand() {
        struct SeedBoostHook;
        impl SearchHook for SeedBoostHook {
            fn on_pre_graph_expand(&self, seeds: &mut Vec<SearchHit>, _ctx: &mut HookContext) {
                seeds.push(SearchHit {
                    id: 100,
                    score: 0.9,
                    payload: serde_json::Value::Null,
                });
            }
        }

        let mut composite = CompositeHook::new();
        composite.add(SeedBoostHook);

        let mut ctx = HookContext::new();
        let mut seeds = vec![SearchHit {
            id: 1,
            score: 1.0,
            payload: serde_json::Value::Null,
        }];
        composite.on_pre_graph_expand(&mut seeds, &mut ctx);
        assert_eq!(seeds.len(), 2);
        assert_eq!(seeds[1].id, 100);
    }

    #[test]
    fn test_composite_on_post_search() {
        struct TruncateHook;
        impl SearchHook for TruncateHook {
            fn on_post_search(&self, results: &mut Vec<SearchHit>, _ctx: &mut HookContext) {
                results.truncate(1);
            }
        }

        let mut composite = CompositeHook::new();
        composite.add(TruncateHook);

        let mut ctx = HookContext::new();
        let mut results = vec![
            SearchHit {
                id: 1,
                score: 1.0,
                payload: serde_json::Value::Null,
            },
            SearchHit {
                id: 2,
                score: 0.5,
                payload: serde_json::Value::Null,
            },
        ];
        composite.on_post_search(&mut results, &mut ctx);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_ffi_search_hit_repr() {
        let h = FfiSearchHit {
            id: 42,
            score: 0.95,
        };
        assert_eq!(h.id, 42);
        assert_eq!(h.score, 0.95);
        // 验证 Copy/Clone/Debug
        let h2 = h;
        assert_eq!(format!("{:?}", h2), format!("{:?}", h));
    }

    #[test]
    fn test_ffi_hook_load_nonexistent() {
        let result = FfiHook::load("nonexistent_library.dll");
        assert!(result.is_err());
    }
}
