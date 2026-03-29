pub mod bq; // LSH初筛 (Binary Quantization)
pub mod brute_force;
#[cfg(feature = "hnsw")]
pub mod hnsw;
pub mod text; // 纯文本倒排与特征矩阵 (AC/BM25)
