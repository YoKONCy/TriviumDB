//! 索引表示与热存储生命周期。

use super::*;
impl QuIVer {
    /// 返回当前索引切锥正交残差字典提取的主轴数量（未启用或各向同性分布返回 0）。
    pub(crate) fn residual_cone_count(&self) -> usize {
        self.polar_router
            .as_ref()
            .and_then(|r| r.residual_dict.as_ref())
            .map_or(0, |d| d.num_axes)
    }

    /// 构图冻结完成后，将切锥残差签名与重构元数据回写至主图存储，并释放构图临时缓冲。
    pub(crate) fn finalize_residual_store(&mut self, helper: BuildResidualHelper) {
        let Some(router) = &mut self.polar_router else {
            return;
        };
        let n = self.n.min(helper.meta.len()).min(router.node_meta.len());
        for i in 0..n {
            let rm = helper.meta[i];
            if rm.cone_id != NO_CONE_ID {
                self.bq_store.set_node_slices(
                    i,
                    helper.res_store.pos_slice(i),
                    helper.res_store.strong_slice(i),
                );
                router.node_meta[i].set_residual_cone(rm.cone_id, rm.alpha, rm.sm);
            }
        }
    }

    #[inline]
    pub(crate) fn prefetch_native_direction(&self, space: &NativeSpace<'_>, node: usize) {
        let _ = space;
        #[cfg(feature = "research")]
        if self.dirty_count == 0
            && let Some(store) = &self.direction_store
        {
            store.prefetch(node, space.vector(self.slot_indices[node]));
            return;
        }
        self.bq_store.prefetch_sig(node);
    }

    /// 同步精确预取节点完整 64B 对齐签名块与 32B `PolarNodeMeta` 热元数据（受 LFB 滚动窗口节流保护）。
    #[inline]
    pub(crate) fn prefetch_hot_sm_node(&self, space: &NativeSpace<'_>, node: usize) {
        if let Some(router) = &self.polar_router
            && node < router.node_meta.len()
        {
            #[cfg(target_arch = "x86_64")]
            unsafe {
                use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
                let meta_ptr = router.node_meta.as_ptr().add(node) as *const i8;
                _mm_prefetch(meta_ptr, _MM_HINT_T0);
            }
        }
        #[cfg(feature = "research")]
        if self.dirty_count == 0
            && let Some(store) = &self.direction_store
        {
            store.prefetch(node, space.vector(self.slot_indices[node]));
            return;
        }
        let _ = space;
        self.bq_store.prefetch_node_exact(node);
    }

    /// 在消融实验中原地切换主图与极坐标锥目录的 `Bq2Store` 连续缓存行对齐布局（`true`）与传统分离双数组布局（`false`）。
    #[cfg(feature = "research")]
    pub(crate) fn set_bq_contiguous_layout(&mut self, contiguous: bool) {
        self.bq_store.set_contiguous_layout(contiguous);
        if let Some(router) = &mut self.polar_router {
            router.cone_store.set_contiguous_layout(contiguous);
        }
    }

    /// 实验切回分离布局时恢复完整签名和元数据，完成后释放合表。
    #[cfg(feature = "research")]
    pub(crate) fn restore_split_hot_store(&mut self) {
        if let Some(router) = &mut self.polar_router
            && router.colocated_store.is_some()
        {
            self.bq_store.set_contiguous_layout(true);
            router.restore_split_meta();
        }
    }

    /// 预构建 768D 单页对齐 256B 合表热节点记录（`HotSmStore`），可选释放旧分离缓冲以计量真实常驻内存，返回物化耗时（秒）。
    pub(crate) fn prepare_colocated_hot_store(
        &mut self,
        space: &NativeSpace<'_>,
        reclaim_split: bool,
    ) -> f64 {
        let t0 = std::time::Instant::now();
        if HotSmStore::supports_dim(self.dim)
            && let Some(router) = &mut self.polar_router
        {
            if router.colocated_store.is_none() {
                let store =
                    HotSmStore::build(space, &self.bq_store, &self.slot_indices, &router.node_meta);
                router.colocated_store = Some(Arc::new(store));
            }
            if reclaim_split && let Some(store) = &router.colocated_store {
                self.bq_store.reclaim_for_colocated(Arc::clone(store));
                router.node_meta = Vec::new();
            }
        }
        t0.elapsed().as_secs_f64()
    }

    #[cfg(feature = "research")]
    pub(crate) fn prepare_direction_representation(
        &mut self,
        vectors: &[f32],
        mode: HyperbolicStrategy,
    ) {
        let direction_mode = matches!(
            mode,
            HyperbolicStrategy::ScalarQuantizedDirection
                | HyperbolicStrategy::FullPrecisionDirection
        );
        if direction_mode
            && self.direction_store.as_ref().is_some_and(|s| {
                s.is_sq8() == (mode == HyperbolicStrategy::ScalarQuantizedDirection)
            })
        {
            return;
        }
        self.direction_store = match mode {
            HyperbolicStrategy::ScalarQuantizedDirection
            | HyperbolicStrategy::FullPrecisionDirection => Some(
                crate::index::pelorus::engine::direction::DirectionStore::build(
                    vectors,
                    self.dim,
                    mode == HyperbolicStrategy::ScalarQuantizedDirection,
                ),
            ),
            _ => None,
        };
    }

    #[inline]
    pub(crate) fn native_neighbors(&self, node: u32, layer: usize) -> &[u32] {
        #[cfg(feature = "research")]
        trace_query(|t| {
            t.expanded.insert(node);
            t.expansion_calls += 1;
        });
        if layer == 0 {
            self.layer0.neighbors(node)
        } else {
            &self.upper_layers[layer - 1][node as usize]
        }
    }

    #[inline]
    pub(crate) fn native_layer0_neighbors_fast(&self, node: u32, trace_slow: bool) -> &[u32] {
        #[cfg(feature = "research")]
        if trace_slow {
            trace_query(|t| {
                t.expanded.insert(node);
                t.expansion_calls += 1;
            });
        }
        #[cfg(not(feature = "research"))]
        let _ = trace_slow;
        self.layer0.neighbors(node)
    }

    pub(crate) fn set_native_neighbors(&mut self, node: u32, layer: usize, neighbors: &[u32]) {
        if layer == 0 {
            self.layer0.set_neighbors(node, neighbors);
        } else {
            self.upper_layers[layer - 1][node as usize] = neighbors.to_vec();
        }
    }
}
