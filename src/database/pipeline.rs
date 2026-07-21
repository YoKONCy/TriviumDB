//! 混合检索管线 (Hybrid Search Pipeline)
//!
//! 从 database.rs 独立拆分的核心检索逻辑，包含：
//! - L0 安全防御（NaN/Inf/维度检查 + 参数钳位）
//! - L1 文本稀疏召回（AC 自动机 + BM25）
//! - L2 向量稠密召回（BruteForce / BQ 三级火箭）
//! - L3 Payload 预过滤（Parallel Bit-Tag Array 布隆拦截）
//! - L4 FISTA 残差搜索
//! - L5 影子查询
//! - L6 PPR 图谱扩散
//! - L7 不应期/侧向抑制
//! - L9 DPP 多样性采样
//!
//! 以及 6 个 Hook 调用点的集成。

use crate::VectorType;
use crate::database::config::SearchConfig;
use crate::error::Result;
use crate::hook::{HookContext, SearchHook};
use crate::index::brute_force;
use crate::index::quiver::QuIVerSearchConfig;
use crate::node::{NodeId, SearchHit};
use crate::storage::memtable::MemTable;
use std::sync::{Arc, Mutex};

use super::lock_or_recover;

/// 执行完整的混合检索管线
///
/// 这是从 `Database::search_hybrid_internal` 中提取出的核心管线逻辑。
/// 将 ~500 行的检索实现独立为专门文件，便于维护和测试。
pub(crate) fn execute_pipeline<T: VectorType>(
    memtable: &Arc<Mutex<MemTable<T>>>,
    hook: &Arc<dyn SearchHook>,
    query_text: Option<&str>,
    query_vector: Option<&[T]>,
    config: &SearchConfig,
    ctx: &mut HookContext,
) -> Result<Vec<SearchHit>> {
    #[allow(unused_mut)]
    let mut mt = lock_or_recover(memtable);

    // ═══════════════════════════════════════════════════════
    //  L0: 容错与防御式编程 (Sanity Checks)
    // ═══════════════════════════════════════════════════════
    let dim = mt.dim();
    if let Some(qv) = query_vector {
        if qv.len() != dim {
            return Err(crate::error::TriviumError::DimensionMismatch {
                expected: dim,
                got: qv.len(),
            });
        }
        for item in qv {
            let f = item.to_f32();
            if f.is_nan() || f.is_infinite() {
                return Err(crate::error::TriviumError::InvalidVector {
                    reason: "查询向量包含 NaN 或 Infinity (Query vector contains NaN or Infinity)"
                        .to_string(),
                });
            }
        }
    }

    // 隔离作用域：强行钳平越界的玄学配置参数，防止底层矩阵求解 Panic 或死循环
    let mut safe_cfg = config.clone();
    safe_cfg.top_k = safe_cfg.top_k.max(1);
    safe_cfg.fista_lambda = safe_cfg.fista_lambda.clamp(1e-5, 100.0);
    safe_cfg.teleport_alpha = safe_cfg.teleport_alpha.clamp(0.0, 1.0);
    safe_cfg.dpp_quality_weight = safe_cfg.dpp_quality_weight.clamp(0.0, 10.0);
    safe_cfg.fista_threshold = safe_cfg.fista_threshold.clamp(0.0, f32::MAX);

    // ═══════════════════════════════════════════════════════
    // 🔌 Hook #1: on_pre_search — 查询预处理
    // ═══════════════════════════════════════════════════════
    let mut query_vec_f32: Vec<f32> = query_vector
        .map(|qv| qv.iter().map(|x| x.to_f32()).collect())
        .unwrap_or_default();
    {
        let t0 = std::time::Instant::now();
        hook.on_pre_search(&mut query_vec_f32, &mut safe_cfg, ctx);
        ctx.record_timing("hook_pre_search", t0.elapsed());
    }

    // 如果 Hook 请求提前终止管线，直接返回空结果
    if ctx.abort {
        return Ok(vec![]);
    }

    // 如果 Hook 修改了查询向量，需要转回泛型 T
    let hooked_query: Vec<T> = query_vec_f32.iter().map(|&x| T::from_f32(x)).collect();
    let query_vector: Option<&[T]> = if query_vector.is_some() {
        Some(&hooked_query)
    } else {
        None
    };

    let config = &safe_cfg;

    // ═══════════════════════════════════════════════════════
    // 🔌 Hook #2: on_custom_recall — 自定义召回
    // ═══════════════════════════════════════════════════════
    let custom_recall_result = {
        let t0 = std::time::Instant::now();
        let result = hook.on_custom_recall(&query_vec_f32, config, ctx);
        ctx.record_timing("hook_custom_recall", t0.elapsed());
        result
    };

    // ═══════════════════════════════════════════════════════
    //  L1 + L2 + L3: 混合召回（文本 + 向量 + 布隆拦截）
    // ═══════════════════════════════════════════════════════
    let mut anchor_hits: Vec<SearchHit> = Vec::new();
    let mut seed_map: std::collections::HashMap<NodeId, f32> = std::collections::HashMap::new();

    if let Some(custom_hits) = custom_recall_result {
        // 使用自定义召回结果，跳过内置管线
        for hit in custom_hits {
            *seed_map.entry(hit.id).or_insert(0.0) += hit.score;
        }
    } else {
        // === 内置召回管线 ===
        // 提前确保向量缓存已就绪（需要 &mut，只在此处调用一次）。
        // 仅当将走暴力路径或启用残差影子查询时才需要物化全量 merged 缓存；
        // 纯 QuIVer 路径按需从 mmap 读取冷向量，无需 merged（避免全量入堆）。
        let need_flat =
            config.force_brute_force || (config.enable_advanced_pipeline && config.enable_sparse_residual);
        mt.ensure_vectors_cache(need_flat);
        recall_text(&mt, config, query_text, &mut seed_map);
        recall_vector(&mut mt, config, query_vector, &mut seed_map);
        recall_residual(&mt, config, query_vector, &mut seed_map);
    }

    // 将 seed_map 聚合为 anchor_hits
    aggregate_seeds(&mt, config, &seed_map, &mut anchor_hits);

    // ═══════════════════════════════════════════════════════
    // 🔌 Hook #3: on_post_recall — 召回后处理
    // ═══════════════════════════════════════════════════════
    {
        let t0 = std::time::Instant::now();
        hook.on_post_recall(&mut anchor_hits, ctx);
        ctx.record_timing("hook_post_recall", t0.elapsed());
    }

    if anchor_hits.is_empty() {
        return Ok(vec![]);
    }

    // 补充 Payload 并构建种子集
    let mut seeds = Vec::with_capacity(anchor_hits.len());
    for mut hit in anchor_hits {
        if let Some(payload) = mt.get_payload(hit.id) {
            hit.payload = payload.clone();
            seeds.push(hit);
        }
    }

    // ═══════════════════════════════════════════════════════
    // 🔌 Hook #4: on_pre_graph_expand — 图扩散前拦截
    // ═══════════════════════════════════════════════════════
    {
        let t0 = std::time::Instant::now();
        hook.on_pre_graph_expand(&mut seeds, ctx);
        ctx.record_timing("hook_pre_graph_expand", t0.elapsed());
    }

    // ═══════════════════════════════════════════════════════
    //  L6 + L7: PPR 图谱扩散 + 不应期/侧向抑制
    // ═══════════════════════════════════════════════════════
    let t_graph = std::time::Instant::now();
    let mut expanded = crate::graph::traversal::expand_graph(
        &mt,
        seeds,
        config.expand_depth,
        config.teleport_alpha,
        config.enable_inverse_inhibition,
        config.lateral_inhibition_threshold,
        config.enable_refractory_fatigue,
        config.diffusion_bias.as_deref(), // CCSA: 传递扩散偏置向量
    );
    ctx.record_timing("graph_expand", t_graph.elapsed());

    // L8 (时间衰减与多维重排) 已被设计哲学剥离：交由上层 Hook 或 Agent 侧处理。

    // ═══════════════════════════════════════════════════════
    // 🔌 Hook #5: on_rerank — 自定义重排序
    // ═══════════════════════════════════════════════════════
    {
        let t0 = std::time::Instant::now();
        if let Some(reranked) = hook.on_rerank(&mut expanded, ctx) {
            expanded = reranked;
        }
        ctx.record_timing("hook_rerank", t0.elapsed());
    }

    // ═══════════════════════════════════════════════════════
    //  L9: DPP 多样性采样
    // ═══════════════════════════════════════════════════════
    if config.enable_advanced_pipeline
        && config.enable_dpp
        && expanded.len() > config.top_k
        && let Some(mut final_results) = apply_dpp(&mt, config, &expanded)
    {
        // 🔌 Hook #6: on_post_search（DPP 分支）
        {
            let t0 = std::time::Instant::now();
            hook.on_post_search(&mut final_results, ctx);
            ctx.record_timing("hook_post_search", t0.elapsed());
        }
        return Ok(final_results);
    }

    expanded.truncate(config.top_k);

    // ═══════════════════════════════════════════════════════
    // 🔌 Hook #6: on_post_search — 最终后处理
    // ═══════════════════════════════════════════════════════
    {
        let t0 = std::time::Instant::now();
        hook.on_post_search(&mut expanded, ctx);
        ctx.record_timing("hook_post_search", t0.elapsed());
    }

    Ok(expanded)
}

// ═══════════════════════════════════════════════════════════
//  子管线函数：将各阶段拆为独立函数，提高可读性与可测试性
// ═══════════════════════════════════════════════════════════

/// L1: 文本稀疏召回（AC 自动机精准锚点 + BM25 兜底打分）
fn recall_text<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    query_text: Option<&str>,
    seed_map: &mut std::collections::HashMap<NodeId, f32>,
) {
    if !config.enable_text_hybrid_search {
        return;
    }
    if let Some(txt) = query_text {
        let text_engine = mt.text_engine();
        // AC 精准命中
        let ac_hits = text_engine.search_ac(txt);
        for (id, score) in ac_hits {
            *seed_map.entry(id).or_insert(0.0) += score * config.text_boost;
        }
        // BM25 兜底
        let bm25_hits = text_engine.search_bm25(txt, config.bm25_k1, config.bm25_b);
        for (id, score) in bm25_hits {
            let normalized_score = (score / 10.0).clamp(0.0, 1.0) * config.text_boost;
            *seed_map.entry(id).or_insert(0.0) += normalized_score;
        }
    }
}

/// L2 + L3: 向量稠密召回（自适应路由 + 布隆预过滤）
fn recall_vector<T: VectorType>(
    mt: &mut MemTable<T>,
    config: &SearchConfig,
    query_vector: Option<&[T]>,
    seed_map: &mut std::collections::HashMap<NodeId, f32>,
) {
    let query_vector = match query_vector {
        Some(qv) => qv,
        None => return,
    };

    let dim = mt.dim();

    // ═══════════════════════════════════════════════════════
    // 动态引擎路由：
    // 1. QuIVer Vamana 图搜索（N >= 10,000 时由 ensure_vectors_cache 自动构建）
    //    —— 冷热分离：f32 向量按需从 mmap 读取，无需物化全量 flat 数组
    // 2. 暴力全扫（N < 10,000 或 force_brute_force）
    //    —— 需要连续 flat 数组，由 ensure_vectors_cache 在该路径下构建 merged 缓存
    // ═══════════════════════════════════════════════════════
    let vector_hits: Vec<SearchHit> = if !config.force_brute_force && mt.quiver().is_some() {
        let approximate_hits = quiver_pipeline(mt, config, query_vector);
        if config.payload_filter.is_some() && approximate_hits.len() < config.top_k {
            // QuIVer 的 BQ beam 候选池本身不感知 Payload。高选择性过滤可能令
            // `ef_search` 池中不足 top_k，即使池外仍有匹配节点。仅在确实欠填时
            // 物化连续缓存并回退精确扫描，兼顾常规冷路径与过滤结果完整性。
            tracing::debug!(
                returned = approximate_hits.len(),
                requested = config.top_k,
                "QuIVer 过滤后结果不足，回退精确暴力扫描 (filtered QuIVer underfilled; falling back to exact scan)"
            );
            mt.ensure_vectors_cache(true);
            let vectors = mt.flat_vectors();
            brute_force_pipeline(mt, config, query_vector, vectors, dim)
        } else {
            approximate_hits
        }
    } else {
        // ensure_vectors_cache() 已在 execute_pipeline 中按需构建好 merged 缓存
        let vectors = mt.flat_vectors();
        brute_force_pipeline(mt, config, query_vector, vectors, dim)
    };

    for hit in vector_hits {
        *seed_map.entry(hit.id).or_insert(0.0) += hit.score;
    }
}

/// 暴力全扫管线（N < 10,000 或 force_brute_force 时使用）
fn brute_force_pipeline<T: VectorType + Sync>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    query_vector: &[T],
    vectors: &[T],
    dim: usize,
) -> Vec<SearchHit> {
    let bloom_mask = config
        .payload_filter
        .as_ref()
        .map(|f| f.extract_must_have_mask())
        .unwrap_or(0);
    brute_force::search_filter_map(
        query_vector,
        vectors,
        dim,
        config.top_k,
        config.min_score,
        |idx| eligible_node_id(mt, config.payload_filter.as_ref(), bloom_mask, idx),
    )
}

/// QuIVer 管线：BQ-native Vamana 图搜索（替代三级火箭）
///
/// 冷热分离架构：
/// - Hot 路径（O(log N)）：2-bit BQ 签名 beam search 遍历 Vamana 图
/// - Cold 路径（O(ef)）：仅对候选集做 f32 cosine 精排
/// - 相比三级火箭的 O(N) 全扫，大规模数据下速度提升显著
fn quiver_pipeline<T: VectorType + Sync>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    query_vector: &[T],
) -> Vec<SearchHit> {
    let quiver = mt.quiver().unwrap();
    let q_f32: Vec<f32> = query_vector.iter().map(|x| x.to_f32()).collect();

    // ef_search 默认为 top_k * 8，BQ 2-bit 量化精度较低，需要更宽的 beam 覆盖
    let ef_search = config.top_k.max(1) * 8;
    let search_cfg = QuIVerSearchConfig {
        top_k: config.top_k.max(1) * 2, // 多召回一些，给 filter 留余量
        ef_search,
        rerank_limit: None,
    };

    let filter_ref = config.payload_filter.as_ref();
    let bloom_mask = filter_ref
        .map(|filter| filter.extract_must_have_mask())
        .unwrap_or(0);

    // 冷热分离精排回调：按 MemTable slot 索引**按需**从 mmap 零拷贝取回单条向量，
    // 转为 f32 写入复用缓冲区。整个查询只触达 ~ef 个候选，
    // 不再物化全量 f32 数组（不触发 merged 缓存），冷数据始终留在 OS PageCache。
    let vec_pool = mt.vec_pool();
    let raw_results = quiver.search(
        &q_f32,
        |slot, buf| {
            // QuIVer 仍可穿过不匹配节点进行图导航，但只有满足过滤条件的候选
            // 才进入 f32 精排与内部 top_k，避免固定 2× 过召回被无效候选占满。
            if eligible_node_id(mt, filter_ref, bloom_mask, slot).is_none() {
                return false;
            }
            match vec_pool.get(slot) {
                Some(v) => {
                    buf.clear();
                    buf.extend(v.iter().map(|x| x.to_f32()));
                    true
                }
                None => false,
            }
        },
        &search_cfg,
    );

    // 应用 payload 过滤 + min_score 阈值
    let mut hits: Vec<SearchHit> = raw_results
        .into_iter()
        .filter(|&(id, score)| {
            score >= config.min_score && matches_payload_filter(mt, filter_ref, id)
        })
        .map(|(id, score)| SearchHit {
            id,
            score,
            payload: mt
                .get_payload(id)
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        })
        .collect();

    hits.sort_unstable_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(config.top_k);
    hits
}

/// L4 + L5: FISTA 残差搜索 + 影子查询
fn recall_residual<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    query_vector: Option<&[T]>,
    seed_map: &mut std::collections::HashMap<NodeId, f32>,
) {
    if !config.enable_advanced_pipeline || !config.enable_sparse_residual || seed_map.is_empty() {
        return;
    }
    let query_vector = match query_vector {
        Some(qv) => qv,
        None => return,
    };

    let filter_ref = config.payload_filter.as_ref();
    let bloom_mask = filter_ref
        .map(|filter| filter.extract_must_have_mask())
        .unwrap_or(0);

    let entity_vecs: Vec<Vec<f32>> = seed_map
        .keys()
        .filter_map(|&id| {
            if !matches_payload_filter(mt, filter_ref, id) {
                return None;
            }
            mt.get_vector(id)
                .map(|v| v.iter().map(|&x| x.to_f32()).collect())
        })
        .collect();
    if entity_vecs.is_empty() {
        return;
    }
    let q_f32: Vec<f32> = query_vector.iter().map(|&x| x.to_f32()).collect();

    let (_, residual, residual_norm) =
        crate::cognitive::fista_solve(&q_f32, &entity_vecs, config.fista_lambda, 80);

    // L5: 残差足够大时触发影子查询
    if residual_norm > config.fista_threshold {
        tracing::debug!(
            "FISTA 残差较高 (FISTA residual high) ({} > {})，触发影子查询 (shadow query triggered)",
            residual_norm,
            config.fista_threshold
        );
        let r_orig: Vec<T> = residual.iter().map(|&x| T::from_f32(x)).collect();
        let dim = mt.dim();
        let shadow_hits = brute_force::search_filter_map(
            &r_orig,
            mt.flat_vectors(),
            dim,
            config.top_k,
            config.min_score,
            |idx| eligible_node_id(mt, filter_ref, bloom_mask, idx),
        );
        for sh in shadow_hits {
            *seed_map.entry(sh.id).or_insert(0.0) += sh.score * 0.8; // 影子抑制衰减
        }
    }
}

/// 将 seed_map 聚合为排序后的 anchor_hits
fn aggregate_seeds<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    seed_map: &std::collections::HashMap<NodeId, f32>,
    anchor_hits: &mut Vec<SearchHit>,
) {
    let filter_ref = config.payload_filter.as_ref();
    for (&id, &score) in seed_map {
        if score >= config.min_score && matches_payload_filter(mt, filter_ref, id) {
            let payload = mt
                .get_payload(id)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            anchor_hits.push(SearchHit { id, score, payload });
        }
    }
    anchor_hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    anchor_hits.truncate(config.top_k.max(15));
}

/// 检查一个逻辑节点是否仍然活跃并满足精确 Payload 条件。
#[inline]
fn matches_payload_filter<T: VectorType>(
    mt: &MemTable<T>,
    filter: Option<&crate::filter::Filter>,
    id: NodeId,
) -> bool {
    if id == 0 {
        return false;
    }
    match filter {
        None => mt.contains(id),
        Some(filter) => mt
            .get_payload(id)
            .is_some_and(|payload| filter.matches(payload)),
    }
}

/// 将物理向量槽位映射为可参与召回的逻辑节点。
///
/// 顺序固定为：墓碑检查 → Bloom true-negative → 精确 Payload 判断。
/// 返回 `None` 的槽位不会计算相似度，也不会占用任何 `top_k` 名额。
#[inline]
fn eligible_node_id<T: VectorType>(
    mt: &MemTable<T>,
    filter: Option<&crate::filter::Filter>,
    bloom_mask: u64,
    idx: usize,
) -> Option<NodeId> {
    let id = *mt.internal_indices().get(idx)?;
    if id == 0 {
        return None;
    }
    if bloom_mask != 0
        && mt
            .fast_tags_slice()
            .get(idx)
            .is_some_and(|tag| (*tag & bloom_mask) != bloom_mask)
    {
        return None;
    }
    match filter {
        None => Some(id),
        Some(filter) => mt
            .get_payload(id)
            .filter(|payload| filter.matches(payload))
            .map(|_| id),
    }
}

/// L9: DPP 多样性采样
fn apply_dpp<T: VectorType>(
    mt: &MemTable<T>,
    config: &SearchConfig,
    expanded: &[SearchHit],
) -> Option<Vec<SearchHit>> {
    let limit = config.top_k;
    let dpp_pool_size = std::cmp::min(expanded.len(), limit * 3);
    let mut pool_vecs = Vec::with_capacity(dpp_pool_size);
    let mut pool_scores = Vec::with_capacity(dpp_pool_size);
    let mut pool_valid = Vec::with_capacity(dpp_pool_size);

    for i in 0..dpp_pool_size {
        let hit = &expanded[i];
        if let Some(v) = mt.get_vector(hit.id) {
            pool_vecs.push(v.iter().map(|&x| x.to_f32()).collect());
            pool_scores.push(hit.score);
            pool_valid.push(hit.clone());
        }
    }

    if pool_valid.len() <= limit {
        return None;
    }

    let selected_idx =
        crate::cognitive::dpp_greedy(&pool_vecs, &pool_scores, limit, config.dpp_quality_weight);

    let mut final_results = Vec::with_capacity(limit);
    for &idx in &selected_idx {
        final_results.push(pool_valid[idx].clone());
    }
    final_results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Some(final_results)
}

// ═══════════════════════════════════════════════════════════
//  单元测试
// ═══════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::config::SearchConfig;
    use crate::filter::Filter;
    use crate::hook::{HookContext, NoopHook, SearchHook};
    use crate::node::SearchHit;
    use crate::storage::memtable::MemTable;
    use std::sync::{Arc, Mutex};

    /// 构建一个包含若干 f32 节点的内存 MemTable（无磁盘 IO）
    fn make_memtable(dim: usize, nodes: &[(u64, Vec<f32>, serde_json::Value)]) -> MemTable<f32> {
        let mut mt = MemTable::new(dim);
        for (id, vec, payload) in nodes {
            mt.insert_with_id(*id, vec, payload.clone()).unwrap();
        }
        mt
    }

    fn filter_regression_nodes() -> Vec<(u64, Vec<f32>, serde_json::Value)> {
        vec![
            (
                10,
                vec![1.0, 0.0],
                serde_json::json!({"tenant": "drop", "rank": 0}),
            ),
            (
                11,
                vec![0.8, 0.6],
                serde_json::json!({"tenant": "drop", "rank": 0}),
            ),
            (
                12,
                vec![0.6, 0.8],
                serde_json::json!({"tenant": "keep", "rank": 1}),
            ),
            (
                13,
                vec![0.0, 1.0],
                serde_json::json!({"tenant": "keep", "rank": 2}),
            ),
            (
                14,
                vec![-0.6, 0.8],
                serde_json::json!({"tenant": "keep", "rank": 3}),
            ),
        ]
    }

    fn wrap(mt: MemTable<f32>) -> Arc<Mutex<MemTable<f32>>> {
        Arc::new(Mutex::new(mt))
    }

    fn default_config() -> SearchConfig {
        SearchConfig {
            top_k: 5,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        }
    }

    // ════════ aggregate_seeds ════════

    #[test]
    fn test_aggregate_seeds_sorts_descending_and_truncates() {
        let mt = make_memtable(
            2,
            &[
                (1, vec![1.0, 0.0], serde_json::json!({"a": 1})),
                (2, vec![0.0, 1.0], serde_json::json!({"a": 2})),
                (3, vec![0.5, 0.5], serde_json::json!({"a": 3})),
            ],
        );
        let cfg = SearchConfig {
            top_k: 2,
            min_score: 0.0,
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        seed_map.insert(1u64, 0.9f32);
        seed_map.insert(2, 0.5);
        seed_map.insert(3, 0.7);

        let mut hits = Vec::new();
        aggregate_seeds(&mt, &cfg, &seed_map, &mut hits);

        // top_k=2 但 aggregate_seeds 内部 truncate 到 max(top_k, 15)
        assert!(hits.len() <= 15);
        // 排序检查：降序
        for w in hits.windows(2) {
            assert!(w[0].score >= w[1].score, "应按分数降序");
        }
    }

    #[test]
    fn test_aggregate_seeds_filters_by_min_score() {
        let mt = make_memtable(
            2,
            &[
                (1, vec![1.0, 0.0], serde_json::json!({})),
                (2, vec![0.0, 1.0], serde_json::json!({})),
            ],
        );
        let cfg = SearchConfig {
            top_k: 10,
            min_score: 0.8,
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        seed_map.insert(1u64, 0.9f32);
        seed_map.insert(2, 0.3); // 低于 min_score

        let mut hits = Vec::new();
        aggregate_seeds(&mt, &cfg, &seed_map, &mut hits);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, 1);
    }

    #[test]
    fn test_aggregate_seeds_with_payload_filter() {
        let mt = make_memtable(
            2,
            &[
                (1, vec![1.0, 0.0], serde_json::json!({"role": "admin"})),
                (2, vec![0.0, 1.0], serde_json::json!({"role": "user"})),
            ],
        );
        let cfg = SearchConfig {
            top_k: 10,
            min_score: 0.0,
            payload_filter: Some(Filter::eq("role", serde_json::json!("admin"))),
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        seed_map.insert(1u64, 0.9f32);
        seed_map.insert(2, 0.8);

        let mut hits = Vec::new();
        aggregate_seeds(&mt, &cfg, &seed_map, &mut hits);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, 1);
    }

    #[test]
    fn test_aggregate_seeds_empty_map() {
        let mt = make_memtable(2, &[(1, vec![1.0, 0.0], serde_json::json!({}))]);
        let cfg = default_config();
        let seed_map = std::collections::HashMap::new();
        let mut hits = Vec::new();
        aggregate_seeds(&mt, &cfg, &seed_map, &mut hits);
        assert!(hits.is_empty());
    }

    // ════════ recall_vector (brute-force 路径) ════════

    #[test]
    fn test_recall_vector_basic() {
        let mut mt = make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({})),
                (2, vec![0.0, 1.0, 0.0], serde_json::json!({})),
                (3, vec![0.0, 0.0, 1.0], serde_json::json!({})),
            ],
        );
        mt.ensure_vectors_cache(true);

        let cfg = SearchConfig {
            top_k: 2,
            min_score: 0.0,
            ..Default::default()
        };
        let query: Vec<f32> = vec![1.0, 0.0, 0.0];
        let mut seed_map = std::collections::HashMap::new();

        recall_vector(&mut mt, &cfg, Some(&query), &mut seed_map);

        assert!(!seed_map.is_empty(), "应召回至少一个节点");
        // 节点 1 与 query 完全对齐，得分最高
        let best_id = seed_map
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert_eq!(*best_id, 1);
    }

    #[test]
    fn test_recall_vector_none_query_is_noop() {
        let mut mt = make_memtable(3, &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))]);
        mt.ensure_vectors_cache(true);
        let cfg = default_config();
        let mut seed_map = std::collections::HashMap::new();
        recall_vector(&mut mt, &cfg, None, &mut seed_map);
        assert!(seed_map.is_empty());
    }

    #[test]
    fn test_recall_vector_with_payload_filter() {
        let mut mt = make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({"tag": "yes"})),
                (2, vec![0.9, 0.1, 0.0], serde_json::json!({"tag": "no"})),
            ],
        );
        mt.ensure_vectors_cache(true);

        let cfg = SearchConfig {
            top_k: 5,
            min_score: 0.0,
            payload_filter: Some(Filter::eq("tag", serde_json::json!("yes"))),
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut seed_map = std::collections::HashMap::new();
        recall_vector(&mut mt, &cfg, Some(&query), &mut seed_map);

        assert!(seed_map.contains_key(&1));
        assert!(
            !seed_map.contains_key(&2),
            "node 2 应被 payload_filter 过滤"
        );
    }

    #[test]
    fn test_recall_vector_filter_before_top_k() {
        let mut mt = make_memtable(2, &filter_regression_nodes());
        mt.ensure_vectors_cache(true);

        let cfg = SearchConfig {
            top_k: 2,
            min_score: -1.0,
            force_brute_force: true,
            payload_filter: Some(Filter::eq("tenant", serde_json::json!("keep"))),
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        recall_vector(&mut mt, &cfg, Some(&[1.0, 0.0]), &mut seed_map);

        assert_eq!(seed_map.len(), 2);
        assert!(seed_map.contains_key(&12));
        assert!(seed_map.contains_key(&13));
        assert!(!seed_map.contains_key(&0));
        assert!(!seed_map.contains_key(&10));
        assert!(!seed_map.contains_key(&11));
    }

    #[test]
    fn test_recall_vector_non_bloom_filter_before_top_k() {
        let mut mt = make_memtable(2, &filter_regression_nodes());
        mt.ensure_vectors_cache(true);

        let cfg = SearchConfig {
            top_k: 2,
            min_score: -1.0,
            force_brute_force: true,
            payload_filter: Some(Filter::gt("rank", 0.0)),
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        recall_vector(&mut mt, &cfg, Some(&[1.0, 0.0]), &mut seed_map);

        assert_eq!(seed_map.len(), 2);
        assert!(seed_map.contains_key(&12));
        assert!(seed_map.contains_key(&13));
    }

    #[test]
    fn test_recall_vector_tombstone_does_not_consume_top_k() {
        let mut mt = make_memtable(
            2,
            &[
                (1, vec![1.0, 0.0], serde_json::json!({})),
                (2, vec![-1.0, 0.0], serde_json::json!({})),
                (3, vec![-0.8, 0.6], serde_json::json!({})),
            ],
        );
        mt.delete(1).unwrap();
        mt.ensure_vectors_cache(true);

        let cfg = SearchConfig {
            top_k: 1,
            min_score: -1.0,
            force_brute_force: true,
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        recall_vector(&mut mt, &cfg, Some(&[1.0, 0.0]), &mut seed_map);

        assert_eq!(seed_map.len(), 1);
        assert!(seed_map.contains_key(&3));
        assert!(!seed_map.contains_key(&0));
    }

    #[test]
    fn test_recall_vector_quiver_filters_before_internal_top_k() {
        let mut mt = make_memtable(2, &filter_regression_nodes());
        mt.build_quiver(&crate::index::quiver::QuIVerConfig::default());
        assert!(mt.quiver().is_some());

        let cfg = SearchConfig {
            top_k: 2,
            min_score: -1.0,
            payload_filter: Some(Filter::eq("tenant", serde_json::json!("keep"))),
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        recall_vector(&mut mt, &cfg, Some(&[1.0, 0.0]), &mut seed_map);

        assert_eq!(seed_map.len(), 2);
        assert!(seed_map.contains_key(&12));
        assert!(seed_map.contains_key(&13));
    }

    #[test]
    fn test_recall_vector_quiver_high_selectivity_falls_back_to_exact_scan() {
        let mut nodes: Vec<(u64, Vec<f32>, serde_json::Value)> = (1..=40)
            .map(|id| (id, vec![1.0, 0.0], serde_json::json!({"tenant": "drop"})))
            .collect();
        nodes.extend([
            (100, vec![0.0, 1.0], serde_json::json!({"tenant": "keep"})),
            (101, vec![-0.8, 0.6], serde_json::json!({"tenant": "keep"})),
            (102, vec![-1.0, 0.0], serde_json::json!({"tenant": "keep"})),
        ]);
        let mut mt = make_memtable(2, &nodes);
        mt.build_quiver(&crate::index::quiver::QuIVerConfig::default());

        let cfg = SearchConfig {
            top_k: 2,
            min_score: -1.0,
            payload_filter: Some(Filter::eq("tenant", serde_json::json!("keep"))),
            ..Default::default()
        };
        let approximate_hits = quiver_pipeline(&mt, &cfg, &[1.0, 0.0]);
        assert!(
            approximate_hits.len() < cfg.top_k,
            "该数据集必须先复现 QuIVer 未过滤候选池欠填"
        );
        let mut seed_map = std::collections::HashMap::new();
        recall_vector(&mut mt, &cfg, Some(&[1.0, 0.0]), &mut seed_map);

        assert_eq!(seed_map.len(), 2);
        assert!(seed_map.contains_key(&100));
        assert!(seed_map.contains_key(&101));
    }

    #[test]
    fn test_recall_vector_bloom_preserves_signed_zero_equality() {
        let mut mt = make_memtable(2, &[(1, vec![1.0, 0.0], serde_json::json!({"zero": -0.0}))]);
        mt.ensure_vectors_cache(true);

        let filter = Filter::eq("zero", serde_json::json!(0.0));
        let bloom_mask = filter.extract_must_have_mask();
        assert_ne!(bloom_mask, 0);
        assert_eq!(mt.fast_tags_slice()[0] & bloom_mask, bloom_mask);

        let cfg = SearchConfig {
            top_k: 1,
            min_score: -1.0,
            force_brute_force: true,
            payload_filter: Some(filter),
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        recall_vector(&mut mt, &cfg, Some(&[1.0, 0.0]), &mut seed_map);

        assert_eq!(seed_map.len(), 1);
        assert!(seed_map.contains_key(&1));
    }

    // ════════ recall_text ════════

    #[test]
    fn test_recall_text_disabled_is_noop() {
        let mt = make_memtable(
            2,
            &[(1, vec![1.0, 0.0], serde_json::json!({"text": "hello"}))],
        );
        let cfg = SearchConfig {
            enable_text_hybrid_search: false,
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        recall_text(&mt, &cfg, Some("hello"), &mut seed_map);
        assert!(seed_map.is_empty());
    }

    #[test]
    fn test_recall_text_none_query_is_noop() {
        let mt = make_memtable(
            2,
            &[(1, vec![1.0, 0.0], serde_json::json!({"text": "hello"}))],
        );
        let cfg = SearchConfig {
            enable_text_hybrid_search: true,
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::new();
        recall_text(&mt, &cfg, None, &mut seed_map);
        assert!(seed_map.is_empty());
    }

    // ════════ recall_residual ════════

    #[test]
    fn test_recall_residual_disabled_is_noop() {
        let mut mt = make_memtable(3, &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))]);
        mt.ensure_vectors_cache(true);
        let cfg = SearchConfig {
            enable_advanced_pipeline: false,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut seed_map = std::collections::HashMap::new();
        seed_map.insert(1u64, 0.9f32);
        let before = seed_map.clone();
        recall_residual(&mt, &cfg, Some(&query), &mut seed_map);
        assert_eq!(seed_map, before, "disabled 时 seed_map 不应变化");
    }

    #[test]
    fn test_recall_residual_empty_seeds_is_noop() {
        let mut mt = make_memtable(3, &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))]);
        mt.ensure_vectors_cache(true);
        let cfg = SearchConfig {
            enable_advanced_pipeline: true,
            enable_sparse_residual: true,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut seed_map = std::collections::HashMap::new();
        recall_residual(&mt, &cfg, Some(&query), &mut seed_map);
        assert!(seed_map.is_empty());
    }

    #[test]
    fn test_recall_residual_shadow_filters_before_top_k() {
        let mut mt = make_memtable(
            2,
            &[
                (1, vec![0.0, 1.0], serde_json::json!({"tenant": "keep"})),
                (2, vec![1.0, 0.0], serde_json::json!({"tenant": "drop"})),
                (3, vec![0.8, 0.6], serde_json::json!({"tenant": "drop"})),
                (4, vec![0.6, 0.8], serde_json::json!({"tenant": "keep"})),
                (5, vec![-0.6, 0.8], serde_json::json!({"tenant": "keep"})),
            ],
        );
        mt.ensure_vectors_cache(true);

        let cfg = SearchConfig {
            top_k: 2,
            min_score: -1.0,
            enable_advanced_pipeline: true,
            enable_sparse_residual: true,
            fista_threshold: 0.0,
            payload_filter: Some(Filter::eq("tenant", serde_json::json!("keep"))),
            ..Default::default()
        };
        let mut seed_map = std::collections::HashMap::from([(1, 1.0)]);

        recall_residual(&mt, &cfg, Some(&[1.0, 0.0]), &mut seed_map);

        assert_eq!(seed_map.len(), 2);
        assert!(seed_map.contains_key(&1));
        assert!(seed_map.contains_key(&4));
        assert!(!seed_map.contains_key(&2));
        assert!(!seed_map.contains_key(&3));
    }

    // ════════ apply_dpp ════════

    #[test]
    fn test_apply_dpp_returns_none_when_pool_too_small() {
        let mt = make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({})),
                (2, vec![0.0, 1.0, 0.0], serde_json::json!({})),
            ],
        );
        let cfg = SearchConfig {
            top_k: 5,
            enable_dpp: true,
            dpp_quality_weight: 1.0,
            ..Default::default()
        };
        let expanded = vec![
            SearchHit {
                id: 1,
                score: 0.9,
                payload: serde_json::json!({}),
            },
            SearchHit {
                id: 2,
                score: 0.5,
                payload: serde_json::json!({}),
            },
        ];
        // pool_valid.len() <= top_k → 返回 None
        assert!(apply_dpp(&mt, &cfg, &expanded).is_none());
    }

    #[test]
    fn test_apply_dpp_selects_diverse_subset() {
        let mt = make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({})),
                (2, vec![0.99, 0.01, 0.0], serde_json::json!({})),
                (3, vec![0.0, 1.0, 0.0], serde_json::json!({})),
                (4, vec![0.0, 0.0, 1.0], serde_json::json!({})),
            ],
        );
        let cfg = SearchConfig {
            top_k: 2,
            enable_dpp: true,
            dpp_quality_weight: 1.0,
            ..Default::default()
        };
        let expanded = vec![
            SearchHit {
                id: 1,
                score: 1.0,
                payload: serde_json::json!({}),
            },
            SearchHit {
                id: 2,
                score: 0.95,
                payload: serde_json::json!({}),
            },
            SearchHit {
                id: 3,
                score: 0.8,
                payload: serde_json::json!({}),
            },
            SearchHit {
                id: 4,
                score: 0.7,
                payload: serde_json::json!({}),
            },
        ];

        let result = apply_dpp(&mt, &cfg, &expanded);
        assert!(result.is_some());
        let selected = result.unwrap();
        assert_eq!(selected.len(), 2);
        // DPP 应该选择多样化的组合，而不是得分最高但相似的 1 和 2
        let ids: Vec<u64> = selected.iter().map(|h| h.id).collect();
        assert!(ids.contains(&1), "最高分节点应被选中");
        // 节点 2 与 1 高度相似，DPP 倾向选择 3 或 4 而非 2
        assert!(!ids.contains(&2), "DPP 应优先选择多样化的节点而非相似节点");
    }

    // ════════ execute_pipeline 集成 ════════

    #[test]
    fn test_execute_pipeline_dimension_mismatch() {
        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = default_config();
        let bad_query = vec![1.0, 0.0]; // dim=2, 期望 dim=3
        let mut ctx = HookContext::new();

        let result = execute_pipeline(&mt, &hook, None, Some(&bad_query), &cfg, &mut ctx);
        assert!(result.is_err(), "维度不匹配应返回错误");
    }

    #[test]
    fn test_execute_pipeline_nan_query_rejected() {
        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = default_config();
        let nan_query = vec![f32::NAN, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let result = execute_pipeline(&mt, &hook, None, Some(&nan_query), &cfg, &mut ctx);
        assert!(result.is_err(), "NaN 查询向量应被拒绝");
    }

    #[test]
    fn test_execute_pipeline_inf_query_rejected() {
        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = default_config();
        let inf_query = vec![f32::INFINITY, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let result = execute_pipeline(&mt, &hook, None, Some(&inf_query), &cfg, &mut ctx);
        assert!(result.is_err(), "Infinity 查询向量应被拒绝");
    }

    #[test]
    fn test_execute_pipeline_empty_db() {
        let mt = wrap(MemTable::<f32>::new(3));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = default_config();
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results = execute_pipeline(&mt, &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert!(results.is_empty(), "空库应返回空结果");
    }

    #[test]
    fn test_execute_pipeline_basic_vector_search() {
        let mt = wrap(make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({"name": "a"})),
                (2, vec![0.0, 1.0, 0.0], serde_json::json!({"name": "b"})),
                (3, vec![0.0, 0.0, 1.0], serde_json::json!({"name": "c"})),
            ],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = SearchConfig {
            top_k: 2,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results = execute_pipeline(&mt, &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].id, 1, "最相似节点应排第一");
    }

    #[test]
    fn test_execute_pipeline_payload_filter_fills_top_k() {
        let mt = wrap(make_memtable(2, &filter_regression_nodes()));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = SearchConfig {
            top_k: 2,
            min_score: -1.0,
            expand_depth: 0,
            force_brute_force: true,
            payload_filter: Some(Filter::eq("tenant", serde_json::json!("keep"))),
            ..Default::default()
        };
        let mut ctx = HookContext::new();

        let results =
            execute_pipeline(&mt, &hook, None, Some(&[1.0, 0.0]), &cfg, &mut ctx).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(
            results.iter().map(|hit| hit.id).collect::<Vec<_>>(),
            vec![12, 13]
        );
        assert!(results.iter().all(|hit| hit.payload["tenant"] == "keep"));
    }

    #[test]
    fn test_execute_pipeline_respects_top_k() {
        let nodes: Vec<(u64, Vec<f32>, serde_json::Value)> = (1..=10)
            .map(|i| {
                (
                    i as u64,
                    vec![1.0, i as f32 * 0.01, 0.0],
                    serde_json::json!({}),
                )
            })
            .collect();
        let mt = wrap(make_memtable(3, &nodes));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = SearchConfig {
            top_k: 3,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results = execute_pipeline(&mt, &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert!(results.len() <= 3, "结果数不应超过 top_k");
    }

    #[test]
    fn test_execute_pipeline_records_timings() {
        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = default_config();
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let _ = execute_pipeline(&mt, &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert!(!ctx.stage_timings.is_empty(), "管线应记录阶段计时");
        let stage_names: Vec<&str> = ctx.stage_timings.iter().map(|(n, _)| n.as_str()).collect();
        assert!(stage_names.contains(&"hook_pre_search"));
        assert!(stage_names.contains(&"hook_post_search"));
    }

    // ════════ Hook 集成 ════════

    #[test]
    fn test_hook_abort_returns_empty() {
        struct AbortHook;
        impl SearchHook for AbortHook {
            fn on_pre_search(&self, _: &mut Vec<f32>, _: &mut SearchConfig, ctx: &mut HookContext) {
                ctx.abort = true;
            }
        }

        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(AbortHook);
        let cfg = default_config();
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results = execute_pipeline(&mt, &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert!(results.is_empty(), "abort=true 时应返回空结果");
    }

    #[test]
    fn test_hook_custom_recall_overrides_builtin() {
        struct FixedRecallHook;
        impl SearchHook for FixedRecallHook {
            fn on_custom_recall(
                &self,
                _: &[f32],
                _: &SearchConfig,
                _: &mut HookContext,
            ) -> Option<Vec<SearchHit>> {
                Some(vec![SearchHit {
                    id: 999,
                    score: 1.0,
                    payload: serde_json::Value::Null,
                }])
            }
        }

        let mt = wrap(make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({})),
                (
                    999,
                    vec![0.0, 0.0, 1.0],
                    serde_json::json!({"custom": true}),
                ),
            ],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(FixedRecallHook);
        let cfg = SearchConfig {
            top_k: 5,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results = execute_pipeline(&mt, &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, 999, "自定义召回应覆盖内置召回");
    }

    #[test]
    fn test_hook_post_recall_filters() {
        struct FilterLowScoreHook;
        impl SearchHook for FilterLowScoreHook {
            fn on_post_recall(&self, hits: &mut Vec<SearchHit>, _: &mut HookContext) {
                hits.retain(|h| h.score > 0.5);
            }
        }

        let mt = wrap(make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({})),
                (2, vec![0.0, 1.0, 0.0], serde_json::json!({})),
                (3, vec![0.0, 0.0, 1.0], serde_json::json!({})),
            ],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(FilterLowScoreHook);
        let cfg = SearchConfig {
            top_k: 10,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results = execute_pipeline(&mt, &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        for r in &results {
            assert!(
                r.score > 0.5,
                "Hook 过滤后不应有低分结果: score={}",
                r.score
            );
        }
    }

    #[test]
    fn test_hook_rerank_reverses_order() {
        struct ReverseRerankHook;
        impl SearchHook for ReverseRerankHook {
            fn on_rerank(
                &self,
                hits: &mut Vec<SearchHit>,
                _: &mut HookContext,
            ) -> Option<Vec<SearchHit>> {
                let mut reversed = hits.clone();
                reversed.reverse();
                Some(reversed)
            }
        }

        let mt = wrap(make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({})),
                (2, vec![0.7, 0.7, 0.0], serde_json::json!({})),
            ],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(ReverseRerankHook);
        let cfg = SearchConfig {
            top_k: 5,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results = execute_pipeline(&mt, &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        assert!(results.len() >= 2);
        // rerank hook 反转了顺序，原本分低的现在排前面
        assert_eq!(results[0].id, 2, "rerank 反转后 node 2 应排第一");
    }

    // ════════ 参数钳位 (L0 安全防御) ════════

    #[test]
    fn test_pipeline_clamps_extreme_config() {
        // top_k=0 应被钳到 1，不应 panic
        let mt = wrap(make_memtable(
            3,
            &[(1, vec![1.0, 0.0, 0.0], serde_json::json!({}))],
        ));
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = SearchConfig {
            top_k: 0,
            min_score: 0.0,
            expand_depth: 0,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results = execute_pipeline(&mt, &hook, None, Some(&query), &cfg, &mut ctx);
        assert!(results.is_ok(), "极端参数不应 panic");
    }

    // ════════ 图扩散集成 ════════

    #[test]
    fn test_pipeline_with_graph_expansion() {
        let mut mt = make_memtable(
            3,
            &[
                (1, vec![1.0, 0.0, 0.0], serde_json::json!({"name": "seed"})),
                (
                    2,
                    vec![0.0, 1.0, 0.0],
                    serde_json::json!({"name": "neighbor"}),
                ),
            ],
        );
        mt.link(1, 2, "related".to_string(), 0.8).unwrap();

        let mt = wrap(mt);
        let hook: Arc<dyn SearchHook> = Arc::new(NoopHook);
        let cfg = SearchConfig {
            top_k: 5,
            min_score: 0.0,
            expand_depth: 1,
            ..Default::default()
        };
        let query = vec![1.0, 0.0, 0.0];
        let mut ctx = HookContext::new();

        let results = execute_pipeline(&mt, &hook, None, Some(&query), &cfg, &mut ctx).unwrap();
        let ids: Vec<u64> = results.iter().map(|h| h.id).collect();
        assert!(ids.contains(&2), "图扩散应将邻居节点 2 纳入结果");
    }
}
