//! 公开查询入口与精确距离计算。

use super::*;
impl Index {
    pub fn poincare_navigation(&self) -> PoincareNavigation {
        self.navigation
    }
    pub(in crate::index::pelorus) fn search_poincare_distances(
        &self,
        query: &[f32],
        config: &SearchConfig,
        eligible: &mut dyn FnMut(u64) -> bool,
        ssd: Option<&crate::index::pelorus::io::PelorusSsd>,
        use_profile_rerank: bool,
        work: &crate::index::pelorus::SearchWork,
    ) -> Result<Vec<(u64, f64)>> {
        validate_search_config(config)?;
        validate_vector(query, self.dimensions)?;
        let Metric::Poincare { curvature } = self.metric else {
            return Err(Error::InvalidMetric("当前索引需要使用 Poincaré 度量"));
        };
        let factor = self.metric.boundary_factor(query, 0)?;
        if self.is_empty() {
            return Ok(Vec::new());
        }
        let mut space = self.native_space(curvature, use_profile_rerank);
        space.meter = Some(work);
        self.graph.search_native(
            &space,
            query,
            factor,
            config,
            self.navigation == PoincareNavigation::GeometryBq2,
            eligible,
            ssd,
        )
    }
}
