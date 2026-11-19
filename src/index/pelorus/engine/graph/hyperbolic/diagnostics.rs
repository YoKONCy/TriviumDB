//! 计时外查询损失与阶段剖析。

use super::*;
impl QuIVer {
    /// 零 HashSet 插桩污染采集单次双曲查询四阶段纯墙钟耗时分解（纳秒）。
    #[cfg(feature = "research")]
    pub(crate) fn search_native_stage_profile(
        &self,
        space: &NativeSpace<'_>,
        query: &[f32],
        factor: f64,
        config: &SearchConfig,
        quantized: bool,
    ) -> crate::index::pelorus::engine::Result<SearchStageProfile> {
        struct ProfGuard;
        impl Drop for ProfGuard {
            fn drop(&mut self) {
                STAGE_PROFILE_ACTIVE.store(false, AtomicOrdering::Relaxed);
            }
        }
        STAGE_PROFILE.with(|p| *p.borrow_mut() = SearchStageProfile::default());
        STAGE_PROFILE_ACTIVE.store(true, AtomicOrdering::Relaxed);
        let _guard = ProfGuard;
        let _ = self.search_native(space, query, factor, config, quantized)?;
        STAGE_PROFILE_ACTIVE.store(false, AtomicOrdering::Relaxed);
        Ok(STAGE_PROFILE.with(|p| *p.borrow()))
    }

    /// 在实际搜索路径上采集 V、C、F、R 集合，插桩仅在诊断调用期间启用。
    #[cfg(feature = "research")]
    pub(crate) fn search_native_loss_stats(
        &self,
        space: &NativeSpace<'_>,
        query: &[f32],
        factor: f64,
        config: &SearchConfig,
        quantized: bool,
        truth_ids: &[u64],
    ) -> crate::index::pelorus::engine::Result<SearchLossStats> {
        struct TraceGuard;
        impl Drop for TraceGuard {
            fn drop(&mut self) {
                TRACE_ACTIVE.store(false, AtomicOrdering::Relaxed);
                TRACE.with(|t| {
                    let _ = t.borrow_mut().take();
                });
            }
        }
        TRACE.with(|t| *t.borrow_mut() = Some(QueryTrace::default()));
        TRACE_ACTIVE.store(true, AtomicOrdering::Relaxed);
        let _guard = TraceGuard;
        let result = self.search_native(space, query, factor, config, quantized)?;
        let trace = TRACE.with(|t| t.borrow_mut().take().unwrap());
        TRACE_ACTIVE.store(false, AtomicOrdering::Relaxed);
        let truth_internal: std::collections::HashSet<_> = truth_ids
            .iter()
            .filter_map(|id| self.id_to_internal.get(id).copied())
            .collect();
        let count = |set: &std::collections::HashSet<u32>| {
            truth_internal.iter().filter(|id| set.contains(id)).count()
        };
        let mut scored = trace.scored;
        for slot in &trace.raw_slots {
            if self.slot_indices.get(*slot) == Some(slot) {
                scored.insert(*slot as u32);
            } else if let Some(node) = self.slot_indices.iter().position(|s| s == slot) {
                scored.insert(node as u32);
            }
        }
        // 最终候选包含已评分节点，精排产生的读取不扩大全搜索集合定义。
        scored.extend(trace.beam.iter().copied());
        Ok(SearchLossStats {
            hits_returned: result
                .iter()
                .filter(|(id, _)| truth_ids.contains(id))
                .count(),
            hits_visited: count(&scored),
            hits_in_beam: count(&trace.beam),
            hits_selected_for_rerank: count(&trace.selected),
            visited_nodes: scored.len(),
            beam_candidates: trace.beam.len(),
            selected_for_rerank: trace.selected.len(),
            expanded_nodes: trace.expanded.len(),
            expansion_calls: trace.expansion_calls,
            exact_vector_evals: trace.exact_evals,
            approximate_evals: trace.approximate_evals,
            bound_rejections: trace.bound_rejections,
            header_checks: trace.header_checks,
            header_rejections: trace.header_rejections,
        })
    }
}
