//! 索引诊断与表示测量接口。

use super::*;
impl Index {
    /// 图、方向缓存与径向编码的派生有效载荷，原始向量另计。
    pub fn navigation_storage_bytes(&self) -> usize {
        self.graph.stats().hot_bytes
    }

    /// 切换 2-bit SM 签名的 64B 对齐单数组交织布局（true）与历史双数组 SoA 布局（false），供 O2a 消融测量。
    #[cfg(feature = "research")]
    pub fn set_bq_contiguous_layout(&mut self, contiguous: bool) {
        self.graph.set_bq_contiguous_layout(contiguous);
    }

    /// 恢复分离签名与元数据，供固定图上的布局配对测量使用。
    #[cfg(feature = "research")]
    pub fn restore_split_hot_store(&mut self) {
        self.graph.restore_split_hot_store();
    }

    /// 预构建单页对齐热节点合表，回收分离数组后继续支持读取及增量写入。
    pub fn prepare_colocated_hot_store(&mut self, reclaim_split: bool) -> Result<f64> {
        let Metric::Poincare { curvature } = self.metric else {
            return Err(Error::InvalidSearchConfig("单页合表仅支持双曲索引"));
        };
        if !crate::index::pelorus::engine::graph::HotSmStore::supports_dim(self.dimensions) {
            return Err(Error::InvalidSearchConfig(
                "当前维度的合表记录无法容纳于单页",
            ));
        }
        let (vectors, delta) = self.vectors.slices();
        let secs = self.graph.prepare_colocated_hot_store(
            &NativeSpace {
                vectors,
                delta,
                rerank: None,
                boundary_factors: &self.boundary_factors,
                dimensions: self.dimensions,
                curvature,
                meter: None,
            },
            reclaim_split,
        );
        Ok(secs)
    }

    /// 为固定图准备方向表示对照，编码准备发生在查询计时之外。
    #[cfg(feature = "research")]
    pub fn prepare_direction_representation(
        &mut self,
        mode: crate::index::pelorus::engine::research::HyperbolicStrategy,
    ) -> Result<()> {
        if self.metric == Metric::Cosine {
            return Err(Error::InvalidSearchConfig("方向对照仅支持双曲索引"));
        }
        let data = self.vectors.to_owned();
        let (base, _) = data.slices();
        self.graph.prepare_direction_representation(base, mode);
        Ok(())
    }

    /// 返回当前索引提取的球面切锥正交残差主轴数量（未启用或各向同性分布返回 0）。
    #[cfg(feature = "research")]
    pub fn residual_cone_count(&self) -> usize {
        self.graph.residual_cone_count()
    }

    /// 只读检查底层图的有向可达性、弱连通分量及节点度数，包含墓碑中转节点。
    #[cfg(feature = "research")]
    pub fn graph_diagnostics(&self) -> crate::index::pelorus::engine::research::GraphDiagnostics {
        self.graph.graph_diagnostics()
    }
}
