//! 输入、参数与结果范围校验。

use super::*;
pub(crate) fn score_distance(id: u64, distance: f64) -> Result<(u64, f32)> {
    let score = -distance as f32;
    if score.is_finite() {
        Ok((id, score))
    } else {
        Err(Error::DistanceOverflow)
    }
}
pub(crate) fn validate_build_config(dimensions: usize, config: &BuildConfig) -> Result<()> {
    if dimensions == 0 {
        return Err(Error::InvalidDimensions);
    }
    if dimensions > u32::MAX as usize {
        return Err(Error::CapacityExceeded);
    }
    if config.m < 2 {
        return Err(Error::InvalidBuildConfig("m 必须至少为 2"));
    }
    if config.m > u16::MAX as usize / 2 {
        return Err(Error::InvalidBuildConfig("m 超出邻接表格式限制"));
    }
    if config.ef_construction < config.m {
        return Err(Error::InvalidBuildConfig("ef_construction 必须不小于 m"));
    }
    if !config.alpha.is_finite() || config.alpha < 1.0 {
        return Err(Error::InvalidBuildConfig("alpha 必须是大于等于 1 的有限数"));
    }
    Ok(())
}
pub(crate) fn validate_search_config(config: &SearchConfig) -> Result<()> {
    if config.top_k == 0 {
        return Err(Error::InvalidSearchConfig("top_k 必须大于零"));
    }
    if config.ef_search < config.top_k {
        return Err(Error::InvalidSearchConfig("ef_search 必须不小于 top_k"));
    }
    if config
        .rerank_limit
        .is_some_and(|limit| limit < config.top_k)
    {
        return Err(Error::InvalidSearchConfig("rerank_limit 必须不小于 top_k"));
    }
    Ok(())
}
pub(crate) fn validate_dataset(dimensions: usize, ids: &[u64], vectors: &[f32]) -> Result<usize> {
    if !vectors.len().is_multiple_of(dimensions) {
        return Err(Error::VectorLength {
            expected: vectors.len().div_ceil(dimensions) * dimensions,
            actual: vectors.len(),
        });
    }
    let count = vectors.len() / dimensions;
    if count > u32::MAX as usize {
        return Err(Error::CapacityExceeded);
    }
    if ids.len() != count {
        return Err(Error::IdCount {
            expected: count,
            actual: ids.len(),
        });
    }
    let mut unique = HashSet::with_capacity(ids.len());
    for &id in ids {
        if !unique.insert(id) {
            return Err(Error::DuplicateId(id));
        }
    }
    validate_finite(vectors)?;
    Ok(count)
}
pub(crate) fn validate_vector(vector: &[f32], dimensions: usize) -> Result<()> {
    if vector.len() != dimensions {
        return Err(Error::VectorLength {
            expected: dimensions,
            actual: vector.len(),
        });
    }
    validate_finite(vector)
}
pub(crate) fn validate_finite(values: &[f32]) -> Result<()> {
    if let Some(index) = values.iter().position(|value| !value.is_finite()) {
        return Err(Error::NonFiniteVector { index });
    }
    Ok(())
}
