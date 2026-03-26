use crate::node::{NodeId, SearchHit};
use instant_distance::{Builder, Point, Search};

#[derive(Clone, Debug)]
pub struct VectorPoint {
    pub id: NodeId,
    pub vec: Vec<f32>,
}

impl Point for VectorPoint {
    fn distance(&self, other: &Self) -> f32 {
        // Cosine distance = 1.0 - Cosine Similarity
        let mut dot = 0.0;
        let mut norm_a = 0.0;
        let mut norm_b = 0.0;
        for (va, vb) in self.vec.iter().zip(other.vec.iter()) {
            dot += va * vb;
            norm_a += va * va;
            norm_b += vb * vb;
        }
        if norm_a == 0.0 || norm_b == 0.0 {
            return 1.0; // Max distance
        }
        let sim = dot / (norm_a.sqrt() * norm_b.sqrt());
        1.0 - sim
    }
}

pub struct HnswIndex {
    dim: usize,
    index: Option<instant_distance::HnswMap<VectorPoint, NodeId>>,
}

impl HnswIndex {
    pub fn new(dim: usize) -> Self {
        Self { dim, index: None }
    }

    pub fn rebuild(&mut self, flat_vectors: &[f32], id_mapper: impl Fn(usize) -> NodeId) {
        let num_vectors = flat_vectors.len() / self.dim;
        if num_vectors == 0 {
            self.index = None;
            return;
        }

        let mut points = Vec::with_capacity(num_vectors);
        let mut values = Vec::with_capacity(num_vectors);

        for i in 0..num_vectors {
            let offset = i * self.dim;
            let vec_slice = &flat_vectors[offset..offset + self.dim];
            let id = id_mapper(i);
            
            points.push(VectorPoint {
                id,
                vec: vec_slice.to_vec(),
            });
            values.push(id);
        }

        let hnsw = Builder::default().build(points, values);
        self.index = Some(hnsw);
    }

    pub fn search(&self, query: &[f32], top_k: usize, min_score: f32) -> Vec<SearchHit> {
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
}

