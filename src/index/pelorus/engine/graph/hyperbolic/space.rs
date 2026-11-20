//! 双曲距离表示与向量访问。

use super::*;
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Distance(pub(crate) f64);
impl Eq for Distance {}
impl PartialOrd for Distance {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Distance {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}
pub(crate) type Candidate = (Distance, u32);
/// 边界因子由拥有向量的 Index 维护，不在每次距离计算时重复求范数。
pub(crate) struct NativeSpace<'a> {
    pub vectors: &'a [f32],
    pub delta: &'a [f32],
    pub rerank: Option<&'a crate::index::pelorus::engine::storage::ColdStore>,
    pub boundary_factors: &'a [f64],
    pub dimensions: usize,
    pub curvature: f64,
    pub meter: Option<&'a crate::index::pelorus::SearchWork>,
}
impl NativeSpace<'_> {
    #[inline]
    pub(crate) fn visit(&self) -> bool {
        self.meter.is_none_or(|meter| meter.visit())
    }

    pub(crate) fn checkpoint(&self) -> crate::index::pelorus::engine::Result<()> {
        self.meter
            .map_or(Ok(()), |meter| meter.checkpoint())
            .map_err(|error| crate::index::pelorus::engine::Error::Host(std::sync::Arc::new(error)))
    }

    pub(crate) fn charge_score(&self) -> crate::index::pelorus::engine::Result<()> {
        self.meter
            .map_or(Ok(()), |meter| meter.score())
            .map_err(|error| crate::index::pelorus::engine::Error::Host(std::sync::Arc::new(error)))
    }
    #[inline]
    pub(crate) fn vector(&self, slot: usize) -> &[f32] {
        let offset = slot * self.dimensions;
        if offset < self.vectors.len() {
            &self.vectors[offset..offset + self.dimensions]
        } else {
            &self.delta[offset - self.vectors.len()..offset - self.vectors.len() + self.dimensions]
        }
    }

    #[inline]
    pub(crate) fn prefetch_vector(&self, slot: usize) {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
            let ptr = self.vector(slot).as_ptr() as *const i8;
            let bytes = self.dimensions * 4;
            _mm_prefetch(ptr, _MM_HINT_T0);
            if bytes > 64 {
                _mm_prefetch(ptr.add(64), _MM_HINT_T0);
            }
            // 对跨页宽向量（如 768D = 3072B）跨 4KB 页边界唤醒硬件 L2 流式预取器，同时不溢出 LFB
            if bytes > 512 {
                _mm_prefetch(ptr.add(512), _MM_HINT_T0);
            }
            if bytes > 1536 {
                _mm_prefetch(ptr.add(1536), _MM_HINT_T0);
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = slot;
        }
    }

    #[inline]
    pub(crate) fn proxy(&self, query: &[f32], query_factor: f64, slot: usize) -> f64 {
        #[cfg(feature = "research")]
        trace_query(|t| {
            t.raw_slots.insert(slot);
            t.exact_evals += 1;
        });
        poincare_proxy(
            query,
            self.vector(slot),
            query_factor * self.boundary_factors[slot],
        )
    }

    #[inline]
    pub(crate) fn proxy_fast(
        &self,
        query: &[f32],
        query_factor: f64,
        slot: usize,
        trace_slow: bool,
    ) -> f64 {
        #[cfg(feature = "research")]
        if trace_slow {
            trace_query(|t| {
                t.raw_slots.insert(slot);
                t.exact_evals += 1;
            });
        }
        #[cfg(not(feature = "research"))]
        let _ = trace_slow;
        poincare_proxy(
            query,
            self.vector(slot),
            query_factor * self.boundary_factors[slot],
        )
    }

    pub(crate) fn prefetch_rerank(&self, slot: usize) {
        if let Some(store) = self.rerank {
            store.prefetch(slot);
        } else {
            self.prefetch_vector(slot);
        }
    }

    pub(crate) fn rerank_proxy(
        &self,
        query: &[f32],
        factor: f64,
        slot: usize,
        trace_slow: bool,
        buffer: &mut crate::index::pelorus::engine::storage::RerankScratch,
    ) -> crate::index::pelorus::engine::Result<f64> {
        if let Some(store) = self.rerank {
            let metric = crate::index::pelorus::engine::Metric::Poincare {
                curvature: self.curvature,
            };
            store.score(query, factor, slot, metric, buffer)
        } else {
            Ok(self.proxy_fast(query, factor, slot, trace_slow))
        }
    }
}
