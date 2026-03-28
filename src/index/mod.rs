pub mod brute_force;
pub mod bq; // LSH初筛 (Binary Quantization)
pub mod text; // 纯文本倒排与特征矩阵 (AC/BM25)
#[cfg(feature = "hnsw")]
pub mod hnsw;
