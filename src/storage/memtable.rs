use crate::error::{Result, TriviumError};
use crate::node::{Edge, NodeId};
use crate::storage::vec_pool::VecPool;
use crate::VectorType;
use crate::index::bq::BqSignature;
use crate::index::text::TextIndex;
use std::collections::HashMap;

/// 内存工作区，扮演类似 LSM Tree 中 MemTable 的角色。
///
/// v0.4 改进：向量存储委托给 VecPool（分层 mmap + 内存增量），
/// Payload 和邻接表保持纯内存存储（小而热，随机访问）。
pub struct MemTable<T: VectorType> {
    dim: usize,
    next_id: NodeId,

    // --- 三位一体的核心存储 ---

    // 1. 向量池（分层 mmap）：
    // 委托给 VecPool，底层为 mmap 基础层 + Vec 增量层
    // 基础层由 OS PageCache 按需加载，启动零拷贝
    vec_pool: VecPool<T>,
    
    // 量化签名池 (LSH / Binary Quantization) 初筛选
    bq_signatures: Vec<BqSignature>,

    // 附设文本倒排引擎 (完全可选，纯碎占用独立内存不干扰底座)
    text_index: TextIndex,

    // 2. 元数据映射（关系型负载）—— 保持纯内存
    payloads: HashMap<NodeId, serde_json::Value>,

    // 3. 图谱邻接表 —— 保持纯内存
    edges: HashMap<NodeId, Vec<Edge>>,
    
    // 入度统计表：用于快速查询目标节点的被连接数（支持图谱反向抑制算法）
    in_degrees: HashMap<NodeId, usize>,

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
            vec_pool: VecPool::new(dim),
            bq_signatures: Vec::new(),
            text_index: TextIndex::new(),
            payloads: HashMap::new(),
            edges: HashMap::new(),
            in_degrees: HashMap::new(),
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

    /// 从持久化文件恢复时使用：指定起始 ID 并提供已加载的 VecPool
    pub fn new_with_vec_pool(dim: usize, next_id: NodeId, vec_pool: VecPool<T>) -> Self {
        Self {
            dim,
            next_id,
            vec_pool,
            bq_signatures: Vec::new(),
            text_index: TextIndex::new(),
            payloads: HashMap::new(),
            edges: HashMap::new(),
            in_degrees: HashMap::new(),
            indices_to_ids: Vec::new(),
            ids_to_indices: HashMap::new(),
        }
    }

    /// 暴露当前 ID 计数器值（供 save 时写入文件头）
    pub fn next_id_value(&self) -> NodeId {
        self.next_id
    }

    /// 暴露 VecPool 的可变引用（供 flush 时持久化向量池）
    pub fn vec_pool_mut(&mut self) -> &mut VecPool<T> {
        &mut self.vec_pool
    }

    /// 暴露 VecPool 的只读引用
    pub fn vec_pool(&self) -> &VecPool<T> {
        &self.vec_pool
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
        self.vec_pool.push(vector);
        self.payloads.insert(id, payload);
        self.indices_to_ids.push(id);
        self.ids_to_indices.insert(id, idx);
        Ok(())
    }

    /// 从 mmap 加载时使用：仅注册映射关系，不推入向量（向量已在 VecPool 中）
    pub fn register_node(&mut self, id: NodeId, payload: serde_json::Value) -> Result<()> {
        let idx = self.indices_to_ids.len();
        self.payloads.insert(id, payload);
        self.indices_to_ids.push(id);
        self.ids_to_indices.insert(id, idx);
        Ok(())
    }

    /// 从持久化文件加载时遇到逻辑删除节点（Tombstone），仅推进内部索引映射空洞
    pub fn register_tombstone(&mut self) -> Result<()> {
        // NodeId=0 仅作为位置占位符，不在 payloads/ids_to_indices 中建立映射
        self.indices_to_ids.push(0);
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

        // 1. 记录向量（推入 VecPool 增量层）
        let idx = self.indices_to_ids.len();
        self.vec_pool.push(vector);

        // 2. 更新关系型负载
        self.payloads.insert(id, payload);

        // 3. 构建映射
        self.indices_to_ids.push(id);
        self.ids_to_indices.insert(id, idx);

        Ok(id)
    }

    /// 使用外部指定的 ID 插入节点（例如从外部知识库导入数据）。
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

        // 推入 VecPool 并映射
        let idx = self.indices_to_ids.len();
        self.vec_pool.push(vector);
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
        
        // 增加目标节点的入度计数
        *self.in_degrees.entry(dst).or_insert(0) += 1;
        
        Ok(())
    }

    /// 确保向量合并缓存已构建（需要 &mut self）
    ///
    /// 在调用 flat_vectors() 之前调用此方法，确保缓存已准备好。
    /// 这样设计是为了解决 Rust 借用检查器的限制：
    /// 允许在获取向量切片后同时调用其他 &self 方法。
    #[inline]
    pub fn ensure_vectors_cache(&mut self) {
        self.vec_pool.ensure_cache();
        
        let total = self.vec_pool.total_count();
        if self.bq_signatures.len() != total {
            self.rebuild_bq_signatures(total);
        }
    }
    
    fn rebuild_bq_signatures(&mut self, total: usize) {
        let dim = self.dim();
        let flat = self.vec_pool.flat_vectors();
        
        // 我们利用 flat_vectors 来并行 / 串行提取 1-bit BQ 特征
        let mut new_bq = Vec::with_capacity(total);
        for chunk in flat.chunks(dim) {
            new_bq.push(BqSignature::from_vector(chunk));
        }
        
        // 兜底以防向量池维度异常不对齐
        while new_bq.len() < total {
            new_bq.push(BqSignature::empty());
        }
        self.bq_signatures = new_bq;
    }
    
    /// 获取 BQ 量化初筛签名
    pub fn get_bq_signature(&self, index: usize) -> Option<BqSignature> {
        self.bq_signatures.get(index).copied()
    }

    /// 暴露底层向量数组供检索层消费（只需 &self）
    ///
    /// 调用方应先调用 ensure_vectors_cache() 确保缓存有效。
    #[inline]
    pub fn flat_vectors(&self) -> &[T] {
        self.vec_pool.flat_vectors()
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

        // 1. 向量层：通过 VecPool 逻辑删除（置零）
        if let Some(&idx) = self.ids_to_indices.get(&id) {
            self.vec_pool.zero_out(idx);
        }

        // 2. 元数据层
        self.payloads.remove(&id);

        // 3. 图谱层：删除出边 + 清理其他节点指向该节点的入边
        if let Some(outgoing_edges) = self.edges.remove(&id) {
            // 清理这些出边目标节点的入度计数
            for edge in outgoing_edges {
                if let Some(in_deg) = self.in_degrees.get_mut(&edge.target_id) {
                    *in_deg = in_deg.saturating_sub(1);
                }
            }
        }
        
        for edge_list in self.edges.values_mut() {
            edge_list.retain(|e| e.target_id != id);
            // 这里就不需要在外层大循环里再去减 self.in_degrees[&id] 了，直接在下面把这个 id 从 in_degrees 中移除即可
        }
        self.in_degrees.remove(&id);

        Ok(())
    }

    /// 断开两个节点之间的指定边
    pub fn unlink(&mut self, src: NodeId, dst: NodeId) -> Result<()> {
        if let Some(edge_list) = self.edges.get_mut(&src) {
            let initial_len = edge_list.len();
            edge_list.retain(|e| e.target_id != dst);
            if edge_list.len() < initial_len {
                let removed_count = initial_len - edge_list.len();
                if let Some(in_deg) = self.in_degrees.get_mut(&dst) {
                    *in_deg = in_deg.saturating_sub(removed_count);
                }
            }
            Ok(())
        } else {
            Err(TriviumError::NodeNotFound(src))
        }
    }

    pub fn get_all_ids(&self) -> Vec<NodeId> {
        self.payloads.keys().copied().collect()
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
                self.vec_pool.update(idx, vector);
                Ok(())
            }
            None => Err(TriviumError::NodeNotFound(id)),
        }
    }

    /// 按 ID 获取节点的原生向量（返回切片引用）
    pub fn get_vector(&self, id: NodeId) -> Option<&[T]> {
        self.ids_to_indices.get(&id).and_then(|&idx| {
            self.vec_pool.get(idx)
        })
    }

    /// 当前活跃节点数量
    pub fn node_count(&self) -> usize {
        self.payloads.len()
    }
    
    /// 获取节点的入度数（若不存在则返回0）
    pub fn get_in_degree(&self, id: NodeId) -> usize {
        self.in_degrees.get(&id).copied().unwrap_or(0)
    }

    /// 某节点是否存在
    pub fn contains(&self, id: NodeId) -> bool {
        self.payloads.contains_key(&id)
    }

    /// 返回所有活跃节点 ID
    pub fn all_node_ids(&self) -> Vec<NodeId> {
        self.payloads.keys().cloned().collect()
    }

    /// 返回包含逻辑删除（tombstones）在内的完整内部 ID 阵列，
    /// 用于安全持久化，保持与向量池严格逐一对应。
    pub fn internal_indices(&self) -> &[NodeId] {
        &self.indices_to_ids
    }

    /// 遍历所有可用的 (index, NodeId) 对，跳过已删除节点
    pub fn active_entries(&self) -> impl Iterator<Item = (usize, NodeId)> + '_ {
        self.indices_to_ids
            .iter()
            .enumerate()
            .filter(|(_, nid)| self.payloads.contains_key(nid))
            .map(|(idx, nid)| (idx, *nid))
    }

    /// 估算当前 MemTable 占用的堆内存字节数
    ///
    /// v0.4 改进：VecPool 的 mmap 部分不计入堆内存（由 OS PageCache 管理），
    /// 只计算增量层和合并缓存的实际堆分配。
    pub fn estimated_memory_bytes(&self) -> usize {
        let vec_bytes = self.vec_pool.heap_memory_bytes();
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

    // --- 文本引擎接口 ---
    
    pub fn index_keyword(&mut self, id: NodeId, keyword: &str) {
        if self.contains(id) {
            self.text_index.add_keyword(id, keyword);
        }
    }
    
    pub fn index_text(&mut self, id: NodeId, text: &str) {
        if self.contains(id) {
            self.text_index.add_text(id, text);
        }
    }
    
    pub fn build_text_index(&mut self) {
        self.text_index.build();
    }
    
    pub fn text_engine(&self) -> &TextIndex {
        &self.text_index
    }
}
