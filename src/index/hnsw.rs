use crate::VectorType;
use crate::node::{NodeId, SearchHit};
use instant_distance::{Builder, Point, Search};

/// 泛型向量点：持有任意 VectorType 的向量数据
///
/// 通过 VectorType::similarity() 计算距离，实现 instant_distance::Point trait。
/// 这样 HNSW 索引可以透明支持 f32 / f16 / u64 等所有向量类型。
#[derive(Clone, Debug)]
pub struct VectorPoint<T: VectorType> {
    pub id: NodeId,
    pub vec: Vec<T>,
}

/// SAFETY: VectorType 已经要求 Sync，Vec<T> 在 T: Send+Sync 时也是 Sync
unsafe impl<T: VectorType> Sync for VectorPoint<T> {}

impl<T: VectorType> Point for VectorPoint<T> {
    fn distance(&self, other: &Self) -> f32 {
        // VectorType::similarity() 返回越大越相似
        // Point::distance() 要求越小越相近
        // 因此 distance = 1.0 - similarity
        let sim = T::similarity(&self.vec, &other.vec);
        1.0 - sim
    }
}

/// 泛型 HNSW 索引，支持任意 VectorType
pub struct HnswIndex<T: VectorType> {
    dim: usize,
    index: Option<instant_distance::HnswMap<VectorPoint<T>, NodeId>>,
}

impl<T: VectorType> HnswIndex<T> {
    /// 创建空索引实例
    pub fn new(dim: usize) -> Self {
        Self { dim, index: None }
    }

    /// 从 SoA 扁平向量池全量重建 HNSW 索引
    ///
    /// - `flat_vectors`: MemTable 中的 SoA 向量池引用
    /// - `id_mapper`: 将内部索引号转换为 NodeId 的回调
    ///
    /// 注意：此方法会跳过已逻辑删除（全零向量）的节点，
    /// 避免将死数据纳入索引产生噪音命中。
    pub fn rebuild(
        &mut self,
        flat_vectors: &[T],
        dim: usize,
        id_mapper: impl Fn(usize) -> NodeId,
        is_active: impl Fn(usize) -> bool,
    ) {
        self.dim = dim;
        let num_vectors = flat_vectors.len() / self.dim;
        if num_vectors == 0 {
            self.index = None;
            return;
        }

        let mut points = Vec::with_capacity(num_vectors);
        let mut values = Vec::with_capacity(num_vectors);

        for i in 0..num_vectors {
            // 跳过已删除的节点（逻辑删除后向量为全零）
            if !is_active(i) {
                continue;
            }

            let offset = i * self.dim;
            let vec_slice = &flat_vectors[offset..offset + self.dim];
            let id = id_mapper(i);

            points.push(VectorPoint {
                id,
                vec: vec_slice.to_vec(),
            });
            values.push(id);
        }

        if points.is_empty() {
            self.index = None;
            return;
        }

        let hnsw = Builder::default().build(points, values);
        self.index = Some(hnsw);
    }

    /// 在 HNSW 索引中进行 Top-K 近似搜索
    ///
    /// 返回的 SearchHit 中 payload 为 Null，需要调用方从 MemTable 补充。
    pub fn search(&self, query: &[T], top_k: usize, min_score: f32) -> Vec<SearchHit> {
        if let Some(ref hnsw) = self.index {
            let q_point = VectorPoint {
                id: 0,
                vec: query.to_vec(),
            };
            let mut search = Search::default();
            let results = hnsw.search(&q_point, &mut search);

            let mut hits = Vec::new();
            for item in results.take(top_k) {
                let score = 1.0 - item.distance; // 转换回 similarity
                if score >= min_score {
                    hits.push(SearchHit {
                        id: *item.value,
                        score,
                        payload: serde_json::Value::Null,
                    });
                }
            }
            hits
        } else {
            Vec::new()
        }
    }

    /// 索引是否已构建
    pub fn is_built(&self) -> bool {
        self.index.is_some()
    }
}
