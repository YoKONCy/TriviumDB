use crate::error::{Result, TriviumError};
use crate::node::{Edge, NodeId};
use crate::VectorType;
use std::collections::HashMap;

/// 内存工作区，扮演类似 LSM Tree 中 MemTable 的角色。
/// 在当前架构下，它负责存储运行时的向量（SoA 布局）、JSON 负载和图关系。
pub struct MemTable<T: VectorType> {
    dim: usize,
    next_id: NodeId,

    // --- 三位一体的核心存储 ---

    // 1. 向量池（纯 SoA 布局）：
    // 所有向量在此扁平展开。长度永远等于 `node_count * dim`
    // 提供最极端的 CPU 缓存命中以计算余弦相似度。
    vectors: Vec<T>,

    // 2. 元数据映射（关系型负载）
    payloads: HashMap<NodeId, serde_json::Value>,

    // 3. 图谱邻接表
    edges: HashMap<NodeId, Vec<Edge>>,

    // 映射表：内部索引 (0, 1, 2...) 到 NodeId
    // 用于在 vectors 数组里定位数据位置
    indices_to_ids: Vec<NodeId>,
    ids_to_indices: HashMap<NodeId, usize>,
}

impl<T: VectorType> MemTable<T> {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            next_id: 1, // 从 1 开始，保留 0 作为特殊标记
            vectors: Vec::new(),
            payloads: HashMap::new(),
            edges: HashMap::new(),
            indices_to_ids: Vec::new(),
            ids_to_indices: HashMap::new(),
        }
    }

    /// 从持久化文件恢复时使用：指定起始 ID
    pub fn new_with_next_id(dim: usize, next_id: NodeId) -> Self {
        let mut mt = Self::new(dim);
        mt.next_id = next_id;
        mt
    }

    /// 暴露当前 ID 计数器值（供 save 时写入文件头）
    pub fn next_id_value(&self) -> NodeId {
        self.next_id
    }

    /// 带指定 ID 的插入（从文件重建时使用，不自增 ID）
    pub fn raw_insert(&mut self, id: NodeId, vector: &[T], payload: serde_json::Value) -> Result<()> {
        if vector.len() != self.dim {
            return Err(TriviumError::DimensionMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }

        let idx = self.indices_to_ids.len();
        self.vectors.extend_from_slice(vector);
        self.payloads.insert(id, payload);
        self.indices_to_ids.push(id);
        self.ids_to_indices.insert(id, idx);
        Ok(())
    }

    /// 插入具有原生三维度属性的节点，保证原子性。
    pub fn insert(&mut self, vector: &[T], payload: serde_json::Value) -> Result<NodeId> {
        if vector.len() != self.dim {
            return Err(TriviumError::DimensionMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }

        let id = self.next_id;
        self.next_id += 1;

        // 1. 记录向量（压入尾部）
        let idx = self.indices_to_ids.len();
        self.vectors.extend_from_slice(vector);

        // 2. 更新关系型负载
        self.payloads.insert(id, payload);

        // 3. 构建映射
        self.indices_to_ids.push(id);
        self.ids_to_indices.insert(id, idx);

        Ok(id)
    }

    /// 使用外部指定的 ID 插入节点（例如从 PEDSA 导入数据）。
    /// 如果 ID 已存在会返回错误，并且会自动更新内部的 next_id 以免未来冲突。
    pub fn insert_with_id(&mut self, id: NodeId, vector: &[T], payload: serde_json::Value) -> Result<()> {
        if self.payloads.contains_key(&id) {
            return Err(TriviumError::Generic(format!("Node {} already exists", id)));
        }
        if vector.len() != self.dim {
            return Err(TriviumError::DimensionMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }

        // 推入底层数组并映射
        let idx = self.indices_to_ids.len();
        self.vectors.extend_from_slice(vector);
        self.payloads.insert(id, payload);
        self.indices_to_ids.push(id);
        self.ids_to_indices.insert(id, idx);

        // 防御性推进分配器指针，避免后续普通 insert 撞车
        if id >= self.next_id {
            self.next_id = id + 1;
        }

        Ok(())
    }

    /// 在两节点间建立图谱边
    pub fn link(&mut self, src: NodeId, dst: NodeId, label: String, weight: f32) -> Result<()> {
        if !self.payloads.contains_key(&src) {
            return Err(TriviumError::NodeNotFound(src));
        }
        if !self.payloads.contains_key(&dst) {
            return Err(TriviumError::NodeNotFound(dst));
        }

        let edge = Edge { target_id: dst, label, weight };
        self.edges.entry(src).or_default().push(edge);
        Ok(())
    }

    /// 暴露底层向量数组供检索层消费
    #[inline]
    pub fn flat_vectors(&self) -> &[T] {
        &self.vectors
    }

    #[inline]
    pub fn dim(&self) -> usize {
        self.dim
    }

    #[inline]
    pub fn get_id_by_index(&self, idx: usize) -> NodeId {
        self.indices_to_ids[idx]
    }

    pub fn get_payload(&self, id: NodeId) -> Option<&serde_json::Value> {
        self.payloads.get(&id)
    }

    pub fn get_edges(&self, id: NodeId) -> Option<&[Edge]> {
        self.edges.get(&id).map(|e| e.as_slice())
    }

    /// 删除节点：三层原子联删（向量标记为死区 + Payload移除 + 所有关联边清理）
    pub fn delete(&mut self, id: NodeId) -> Result<()> {
        if !self.payloads.contains_key(&id) {
            return Err(TriviumError::NodeNotFound(id));
        }

        // 1. 向量层：将对应区间置零（逻辑删除，不移动数组避免索引全部重建）
        if let Some(&idx) = self.ids_to_indices.get(&id) {
            let offset = idx * self.dim;
            for i in offset..offset + self.dim {
                self.vectors[i] = T::zero();
            }
        }

        // 2. 元数据层
        self.payloads.remove(&id);

        // 3. 图谱层：删除出边 + 清理其他节点指向该节点的入边
        self.edges.remove(&id);
        for edge_list in self.edges.values_mut() {
            edge_list.retain(|e| e.target_id != id);
        }

        Ok(())
    }

    /// 断开两个节点之间的指定边
    pub fn unlink(&mut self, src: NodeId, dst: NodeId) -> Result<()> {
        if let Some(edge_list) = self.edges.get_mut(&src) {
            edge_list.retain(|e| e.target_id != dst);
            Ok(())
        } else {
            Err(TriviumError::NodeNotFound(src))
        }
    }

    /// 更新节点的元数据（Payload），不影响向量和图谱
    pub fn update_payload(&mut self, id: NodeId, payload: serde_json::Value) -> Result<()> {
        match self.payloads.get_mut(&id) {
            Some(existing) => {
                *existing = payload;
                Ok(())
            }
            None => Err(TriviumError::NodeNotFound(id)),
        }
    }

    /// 就地替换节点的向量（维度必须一致）
    pub fn update_vector(&mut self, id: NodeId, vector: &[T]) -> Result<()> {
        if vector.len() != self.dim {
            return Err(TriviumError::DimensionMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }
        match self.ids_to_indices.get(&id) {
            Some(&idx) => {
                let offset = idx * self.dim;
                self.vectors[offset..offset + self.dim].copy_from_slice(vector);
                Ok(())
            }
            None => Err(TriviumError::NodeNotFound(id)),
        }
    }

    /// 按 ID 获取节点的原生向量（返回切片引用）
    pub fn get_vector(&self, id: NodeId) -> Option<&[T]> {
        self.ids_to_indices.get(&id).map(|&idx| {
            let offset = idx * self.dim;
            &self.vectors[offset..offset + self.dim]
        })
    }

    /// 当前活跃节点数量
    pub fn node_count(&self) -> usize {
        self.payloads.len()
    }

    /// 某节点是否存在
    pub fn contains(&self, id: NodeId) -> bool {
        self.payloads.contains_key(&id)
    }

    /// 返回所有活跃节点 ID
    pub fn all_node_ids(&self) -> Vec<NodeId> {
        self.payloads.keys().cloned().collect()
    }

    /// 遍历所有可用的 (index, NodeId) 对，跳过已删除节点
    pub fn active_entries(&self) -> impl Iterator<Item = (usize, NodeId)> + '_ {
        self.indices_to_ids
            .iter()
            .enumerate()
            .filter(|(_, nid)| self.payloads.contains_key(nid))
            .map(|(idx, nid)| (idx, *nid))
    }

    /// 估算当前 MemTable 占用的内存字节数
    ///
    /// 这是一个保守的下界估算（不含 HashMap 内部开销），
    /// 足以用于内存预算控制。
    pub fn estimated_memory_bytes(&self) -> usize {
        let vec_bytes = self.vectors.len() * std::mem::size_of::<T>();
        let payload_bytes: usize = self.payloads.values()
            .map(|v| v.to_string().len())
            .sum();
        let edge_bytes: usize = self.edges.values()
            .map(|es| es.len() * std::mem::size_of::<Edge>())
            .sum();
        let index_bytes = self.indices_to_ids.len() * std::mem::size_of::<NodeId>()
            + self.ids_to_indices.len() * (std::mem::size_of::<NodeId>() + std::mem::size_of::<usize>());
        vec_bytes + payload_bytes + edge_bytes + index_bytes
    }
}
