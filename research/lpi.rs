//! Layered Projection Index (LPI)
//!
//! Cache-friendly ANN 索引：基于两层随机投影排序的近似最近邻检索。
//!
//! # 数学基础
//!
//! - 投影向量：标准高斯随机向量（Johnson-Lindenstrauss 保证，数据无关）
//! - Layer 1：按投影 p₁ 粗排 → √N 个段（段间有序）
//! - Layer 2：段内按投影 p₂ 精排（段内有序）
//! - 查询：定位段 → 定位位置 → 线性扫描窗口
//!
//! # 无硬编码常数
//!
//! 所有参数（段数、窗口大小、探针数）均由调用方控制。
//! 算法内部不含任何"工程直觉"的魔法数字。
//! 投影向量由调用方提供（保持模块无外部依赖）。

/// LPI 索引结构
///
/// 物理上是一维连续数组，逻辑上是二维分层结构。
pub struct LpiIndex {
    dim: usize,
    n: usize,

    // ═══ 连续存储（cache 友好） ═══
    /// 重排后的向量数组，物理连续：n × dim
    vectors: Vec<f32>,
    /// 原始 ID 映射（与 vectors 同序）
    ids: Vec<u64>,

    // ═══ 随机投影向量（JL 保证，数据无关） ═══
    proj1: Vec<f32>,
    proj2: Vec<f32>,

    // ═══ Layer 1：段结构 ═══
    num_segments: usize,
    /// 每段起始索引（长度 = num_segments + 1）
    segment_starts: Vec<usize>,
    /// 每段的 p1 最小值（用于二分定位）
    segment_p1_bounds: Vec<f32>,

    // ═══ Layer 2：段内 p2 投影值（已排序） ═══
    p2_values: Vec<f32>,
}

/// 查询配置（全部可调，无魔法常数）
pub struct LpiQuery {
    pub top_k: usize,
    /// 段内扫描窗口大小
    pub window_size: usize,
    /// 探测段数（类似 IVF 的 nprobe）
    pub num_probes: usize,
}

impl LpiIndex {
    /// 构建索引
    ///
    /// - `vectors`: N × dim 的连续 f32 数组
    /// - `ids`: N 个原始 ID
    /// - `dim`: 向量维度
    /// - `num_segments`: 段数（推荐 √N）
    /// - `proj1`, `proj2`: 两个归一化的 d 维随机投影向量（调用方生成）
    pub fn build(
        vectors: &[f32],
        ids: &[u64],
        dim: usize,
        num_segments: usize,
        proj1: Vec<f32>,
        proj2: Vec<f32>,
    ) -> Self {
        let n = ids.len();
        assert_eq!(vectors.len(), n * dim);
        assert_eq!(proj1.len(), dim);
        assert_eq!(proj2.len(), dim);
        assert!(num_segments > 0);

        // 计算每个向量的两层投影值
        let mut entries: Vec<(f32, f32, usize)> = (0..n)
            .map(|i| {
                let v = &vectors[i * dim..(i + 1) * dim];
                let p1 = dot(v, &proj1);
                let p2 = dot(v, &proj2);
                (p1, p2, i)
            })
            .collect();

        // Layer 1：按 p1 全局排序
        entries.sort_unstable_by(|a, b| {
            a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
        });

        // 均匀分段
        let seg_size = (n + num_segments - 1) / num_segments;
        let mut segment_starts = Vec::with_capacity(num_segments + 1);
        let mut segment_p1_bounds = Vec::with_capacity(num_segments);

        let actual_segments = {
            let mut count = 0;
            for s in 0..num_segments {
                let start = s * seg_size;
                if start >= n {
                    break;
                }
                segment_starts.push(start);
                segment_p1_bounds.push(entries[start].0);
                count += 1;
            }
            segment_starts.push(n);
            count
        };

        // Layer 2：段内按 p2 排序
        for s in 0..actual_segments {
            let start = segment_starts[s];
            let end = segment_starts[s + 1];
            entries[start..end].sort_unstable_by(|a, b| {
                a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // 构建连续物理存储
        let mut reordered_vecs = Vec::with_capacity(n * dim);
        let mut reordered_ids = Vec::with_capacity(n);
        let mut p2_values = Vec::with_capacity(n);

        for &(_, p2_val, orig_idx) in &entries {
            reordered_vecs.extend_from_slice(&vectors[orig_idx * dim..(orig_idx + 1) * dim]);
            reordered_ids.push(ids[orig_idx]);
            p2_values.push(p2_val);
        }

        LpiIndex {
            dim,
            n,
            vectors: reordered_vecs,
            ids: reordered_ids,
            proj1,
            proj2,
            num_segments: actual_segments,
            segment_starts,
            segment_p1_bounds,
            p2_values,
        }
    }

    /// 查询
    pub fn search(&self, query: &[f32], config: &LpiQuery) -> Vec<(u64, f32)> {
        assert_eq!(query.len(), self.dim);

        // 投影查询向量
        let q1 = dot(query, &self.proj1);
        let q2 = dot(query, &self.proj2);

        // Layer 1：二分定位目标段
        let target_seg = match self.segment_p1_bounds.binary_search_by(|v| {
            v.partial_cmp(&q1).unwrap_or(std::cmp::Ordering::Equal)
        }) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };

        // 确定探测范围
        let half_probes = config.num_probes / 2;
        let seg_lo = target_seg.saturating_sub(half_probes);
        let seg_hi = (target_seg + half_probes + 1).min(self.num_segments);

        // 收集候选
        let mut candidates: Vec<(u64, f32)> = Vec::new();
        let half_window = config.window_size / 2;

        for seg in seg_lo..seg_hi {
            let start = self.segment_starts[seg];
            let end = self.segment_starts[seg + 1];
            let seg_p2 = &self.p2_values[start..end];
            let seg_len = end - start;

            // Layer 2：段内二分定位着陆点
            let landing = match seg_p2.binary_search_by(|v| {
                v.partial_cmp(&q2).unwrap_or(std::cmp::Ordering::Equal)
            }) {
                Ok(i) | Err(i) => i,
            };

            // 线性扫描窗口（纯连续内存访问）
            let scan_lo = landing.saturating_sub(half_window);
            let scan_hi = (landing + half_window).min(seg_len);

            for i in scan_lo..scan_hi {
                let global_idx = start + i;
                let v = &self.vectors[global_idx * self.dim..(global_idx + 1) * self.dim];
                let score = cosine_sim(query, v);
                candidates.push((self.ids[global_idx], score));
            }
        }

        // 取 top-k
        candidates.sort_unstable_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(config.top_k);
        candidates
    }

    /// 索引统计
    pub fn node_count(&self) -> usize {
        self.n
    }

    /// 索引占用的内存（字节）
    pub fn memory_bytes(&self) -> usize {
        self.vectors.len() * 4
            + self.ids.len() * 8
            + self.proj1.len() * 4
            + self.proj2.len() * 4
            + self.segment_starts.len() * 8
            + self.segment_p1_bounds.len() * 4
            + self.p2_values.len() * 4
    }
}

// ═══════ 纯数学辅助函数（无硬编码常数） ═══════

/// 内积
#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// 余弦相似度
#[inline]
fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let ab: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    let denom = na * nb;
    if denom < 1e-30 {
        0.0
    } else {
        ab / denom
    }
}
