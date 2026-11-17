// SM 独立快照复用正式并行构图。
use super::*;

impl SmIndex {
    /// 使用TDB批量并行BQ构图，再转换为静态SM查询快照；构图不使用校正系数。
    /// 此入口返回独立快照。TDB运行期对象可直接快速构图后调用enable_corrected_sm。
    pub fn build_fast(
        base: &[f32],
        coefficients: &[f32],
        config: &super::super::QuIVerConfig,
        threads: usize,
    ) -> io::Result<Self> {
        Self::build_fast_with_dimension(DIM, base, coefficients, config, threads)
    }

    /// 明确指定维度的TDB快速并行构图；覆盖全部合法维度和尾部长度。
    pub fn build_fast_with_dimension(
        dim: usize,
        base: &[f32],
        coefficients: &[f32],
        config: &super::super::QuIVerConfig,
        threads: usize,
    ) -> io::Result<Self> {
        dynamic::check_dimension(dim)?;
        let n = base.len() / dim;
        if n < 2
            || !base.len().is_multiple_of(dim)
            || n > u32::MAX as usize
            || threads == 0
            || config.m == 0
            || config.m > MAX_DEGREE / 2
            || config.ef_construction < config.m * 2
            || !config.alpha.is_finite()
            || config.alpha <= 0.
            || (!coefficients.is_empty() && coefficients.len() != n)
            || coefficients.iter().any(|x| !x.is_finite())
            || base.chunks_exact(dim).any(|row| {
                let norm = row.iter().map(|&x| (x as f64).powi(2)).sum::<f64>();
                !norm.is_finite() || (norm - 1.).abs() > 1e-5
            })
        {
            return Err(invalid(
                "快速SM构建要求合法维度单位向量、有效系数、线程数和出度参数",
            ));
        }
        let mut index = QuIVer::new(dim, config);
        let ids = (0..n as u64).collect::<Vec<_>>();
        let slots = (0..n).collect::<Vec<_>>();
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .map_err(|error| invalid(&format!("无法创建快速构图线程池：{error}")))?
            .install(|| index.batch_build(base, &ids, &slots));
        index.corrected_sm_snapshot(base, coefficients)
    }
}
