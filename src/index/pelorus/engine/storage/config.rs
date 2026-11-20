//! 冷向量存储与重排表示的公开配置。

use crate::index::pelorus::engine::{Error, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StorageMode {
    #[default]
    /// 重排记录常驻堆；启用编码时，原始向量使用文件后备供维护按需访问。
    Memory,
    /// 图与导航状态常驻内存，原始及重排向量通过只读映射按需分页。
    Mmap,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RerankPrecision {
    #[default]
    Fp32,
    Fp16,
    Bf16,
    Sq8,
    Sq4,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RerankRepresentation {
    #[default]
    Cartesian,
    /// 单独保存原始 FP64 半径平方，方向在解码后以 FP64 重新归一化。
    Polar,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarQuantization {
    #[default]
    Symmetric,
    /// 从活跃库向量训练共享逐维均值，插入时沿用已冻结的均值。
    Centered,
    /// 每条向量的每组独立保存 FP32 中点偏置与尺度，支持非整组尾部。
    Affine { group_size: usize },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RerankConfig {
    pub precision: RerankPrecision,
    pub representation: RerankRepresentation,
    pub scalar_quantization: ScalarQuantization,
    /// 为笛卡尔编码增加 FP32 范数恢复系数。
    pub norm_correction: bool,
}

impl RerankConfig {
    pub(crate) fn validate(self) -> Result<()> {
        let scalar = matches!(self.precision, RerankPrecision::Sq8 | RerankPrecision::Sq4);
        if !scalar && self.scalar_quantization != ScalarQuantization::Symmetric {
            return Err(Error::InvalidRerankConfig(
                "中心化与分组配置只适用于 SQ 编码",
            ));
        }
        if matches!(
            self.scalar_quantization,
            ScalarQuantization::Affine { group_size: 0 }
        ) {
            return Err(Error::InvalidRerankConfig("量化组大小必须大于零"));
        }
        if self.norm_correction && self.representation == RerankRepresentation::Polar {
            return Err(Error::InvalidRerankConfig("极坐标表示已独立保存原始半径"));
        }
        Ok(())
    }
}

/// 堆载荷与逻辑映射长度分开统计；映射驻留页由操作系统管理。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VectorStorageStats {
    pub canonical_heap_bytes: usize,
    pub canonical_mapped_bytes: usize,
    pub rerank_heap_bytes: usize,
    pub rerank_mapped_bytes: usize,
    pub shared_metadata_bytes: usize,
    pub rerank_metadata_bytes: usize,
    pub rerank_record_bytes: usize,
}
