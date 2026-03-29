use crate::VectorType;
use bytemuck::{Pod, Zeroable};

/// 512-bit (64-byte) 二进制量化指纹 (Binary Quantization Fingerprint)
///
/// 这是一个标准的数据库 LSH（Locality-Sensitive Hashing）实现途径。
/// 将高精度的 f32 向量降维到 512 位，能使用位操作（XOR + Popcount）
/// 实现超高吞吐量的 Hamming 距离初筛（粗排），替代千万级数据下昂贵的全局 f32 余弦。
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, Pod, Zeroable)]
pub struct BqSignature {
    pub data: [u64; 8],
}

impl BqSignature {
    /// 预分配一个全零签名
    pub fn empty() -> Self {
        Self { data: [0; 8] }
    }

    /// 执行 1-bit 二值量化：遍历实数向量，将 > 0.0 的维度置为 1。
    /// 最多支持将其映射为 512 位的离散指纹（超过 512 维度的部分将被忽略，适合头部分布映射）
    pub fn from_vector<T: VectorType>(vec: &[T]) -> Self {
        let mut data = [0u64; 8];
        for i in 0..8 {
            let mut chunk_bits = 0u64;
            for j in 0..64 {
                let idx = i * 64 + j;
                if idx < vec.len() && vec[idx].to_f32() > 0.0 {
                    chunk_bits |= 1u64 << j;
                }
            }
            data[i] = chunk_bits;
        }
        Self { data }
    }

    /// 执行位掩码比较 (Hamming 距离)，结果越小说明两个签名在这 512 个空间切分下越接近
    #[inline]
    pub fn hamming_distance(&self, other: &Self) -> u32 {
        self.data
            .iter()
            .zip(other.data.iter())
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    }
}
