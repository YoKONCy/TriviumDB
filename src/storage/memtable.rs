//! 数据库的权威内存工作区与派生索引协调层。
//!
//! MemTable 以稳定 NodeId/slot 映射统一管理 VecPool、JSON Payload、出入边目录、
//! Label 目录和 Fast Tags，并同步维护属性、文本、BQ 与 QuIVer 派生索引。CRUD 必须
//! 先验证后修改，删除/slot 复用后不得留下幽灵命中；统计信息供 Planner/Cascades
//! 使用，但任何索引候选最终仍需权威数据精确校验。

use crate::VectorType;
use crate::error::{Result, TriviumError};
use crate::index::bq::{Bq2Store, BqSignature};
use crate::index::property::PropertyIndexRegistry;
use crate::index::quiver::{QuIVer, QuIVerConfig};
use crate::index::text::TextIndex;
use crate::node::{Edge, NodeId};
use crate::storage::vec_pool::VecPool;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LabelStats {
    pub edge_count: usize,
    pub distinct_source_count: usize,
    pub distinct_target_count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DegreeBucket {
    pub upper_bound: usize,
    pub node_count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GraphStats {
    pub node_count: usize,
    pub edge_count: usize,
    pub isolated_node_count: usize,
    pub label_count: usize,
    pub avg_out_degree: f64,
    pub avg_in_degree: f64,
    pub max_out_degree: usize,
    pub max_in_degree: usize,
    pub label_stats: BTreeMap<String, LabelStats>,
    pub out_degree_histogram: Vec<DegreeBucket>,
    pub in_degree_histogram: Vec<DegreeBucket>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CrossModalStats {
    pub degree_skew: f64,
    pub fanout_skew: f64,
    pub vector_density_skew: Option<f64>,
    pub sampled: usize,
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphIntegrityReport {
    pub dangling_edges: usize,
    pub duplicate_edges: usize,
    pub incoming_index_mismatches: usize,
    pub degree_index_mismatches: usize,
    pub label_index_mismatches: usize,
    pub valid: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphRepairReport {
    pub removed_dangling_edges: usize,
    pub removed_duplicate_edges: usize,
    pub rebuilt_indexes: bool,
}
use std::sync::OnceLock;

fn degree_histogram(degrees: impl Iterator<Item = usize>) -> Vec<DegreeBucket> {
    const UPPER_BOUNDS: [usize; 8] = [0, 1, 2, 4, 8, 16, 64, usize::MAX];
    let mut counts = [0usize; UPPER_BOUNDS.len()];
    for degree in degrees {
        let index = UPPER_BOUNDS
            .iter()
            .position(|upper| degree <= *upper)
            .unwrap_or(UPPER_BOUNDS.len() - 1);
        counts[index] += 1;
    }
    UPPER_BOUNDS
        .into_iter()
        .zip(counts)
        .map(|(upper_bound, node_count)| DegreeBucket {
            upper_bound,
            node_count,
        })
        .collect()
}

struct PayloadEntry {
    raw: Box<[u8]>,
    parsed: OnceLock<serde_json::Value>,
}

impl PayloadEntry {
    fn from_value(value: serde_json::Value) -> Self {
        let raw = serde_json::to_vec(&value).unwrap_or_else(|_| b"null".to_vec());
        let parsed = OnceLock::new();
        let _ = parsed.set(value);
        Self {
            raw: raw.into_boxed_slice(),
            parsed,
        }
    }

    fn from_raw(raw: &[u8]) -> Result<Self> {
        use serde::Deserialize;
        let mut deserializer = serde_json::Deserializer::from_slice(raw);
        serde::de::IgnoredAny::deserialize(&mut deserializer).map_err(|error| {
            TriviumError::CorruptedFile(format!("JSON 解析错误 (JSON parse error): {error}"))
        })?;
        deserializer.end().map_err(|error| {
            TriviumError::CorruptedFile(format!("JSON 尾部数据无效 (Invalid JSON tail): {error}"))
        })?;
        Ok(Self {
            raw: raw.to_vec().into_boxed_slice(),
            parsed: OnceLock::new(),
        })
    }

    fn get(&self) -> &serde_json::Value {
        self.parsed
            .get_or_init(|| serde_json::from_slice(&self.raw).unwrap_or(serde_json::Value::Null))
    }

    fn raw(&self) -> &[u8] {
        &self.raw
    }

    fn memory_bytes(&self) -> usize {
        self.raw.len()
            + self
                .parsed
                .get()
                .map(estimate_json_memory)
                .unwrap_or_default()
    }
}

fn estimate_json_memory(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            std::mem::size_of::<serde_json::Value>()
        }
        serde_json::Value::String(text) => {
            std::mem::size_of::<serde_json::Value>() + text.capacity()
        }
        serde_json::Value::Array(values) => {
            std::mem::size_of::<serde_json::Value>()
                + values.capacity() * std::mem::size_of::<serde_json::Value>()
                + values.iter().map(estimate_json_memory).sum::<usize>()
        }
        serde_json::Value::Object(map) => {
            std::mem::size_of::<serde_json::Value>()
                + map
                    .iter()
                    .map(|(key, value)| key.capacity() + estimate_json_memory(value))
                    .sum::<usize>()
        }
    }
}

/// 计算给定 JSON 对象的行级特征布隆签名（共 64 位）
fn calculate_json_signature(value: &serde_json::Value) -> u64 {
    let mut sig = 0u64;
    flatten_and_hash_json("", value, &mut sig);
    sig
}

fn flatten_and_hash_json(prefix: &str, value: &serde_json::Value, sig: &mut u64) {
    use std::hash::{Hash, Hasher};
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let new_prefix = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", prefix, k)
                };
                flatten_and_hash_json(&new_prefix, v, sig);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                flatten_and_hash_json(prefix, v, sig);
            }
        }
        serde_json::Value::String(s) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            format!("{}:{}", prefix, s).hash(&mut hasher);
            *sig |= 1u64 << (hasher.finish() % 64);
        }
        serde_json::Value::Bool(b) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            format!("{}:{}", prefix, b).hash(&mut hasher);
            *sig |= 1u64 << (hasher.finish() % 64);
        }
        serde_json::Value::Number(n) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            format!("{}:{}", prefix, n).hash(&mut hasher);
            *sig |= 1u64 << (hasher.finish() % 64);
        }
        serde_json::Value::Null => {}
    }
}

pub(crate) struct QuiverBuildSnapshot {
    pub generation: u64,
    pub dim: usize,
    pub signatures: Bq2Store,
    pub ids: Vec<u64>,
    pub slots: Vec<usize>,
}

/// 内存工作区，扮演类似 LSM Tree 中 MemTable 的角色。
///
/// v0.4 改进：向量存储委托给 VecPool（分层 mmap + 内存增量），
/// Payload 和邻接表保持纯内存存储（小而热，随机访问）。
pub struct MemTable<T: VectorType> {
    dim: usize,
    next_id: NodeId,
    generation: u64,
    vector_generation: u64,
    property_generation: u64,

    // --- 三位一体的核心存储 ---

    // 1. 向量池（分层 mmap）：
    // 委托给 VecPool，底层为 mmap 基础层 + Vec 增量层
    // 基础层由 OS PageCache 按需加载，启动零拷贝
    vec_pool: VecPool<T>,

    // 量化签名池 (LSH / Binary Quantization) 初筛选
    bq_signatures: Vec<BqSignature>,
    bq_dirty: bool, // delete / update_vector 后标记需要重建

    // 附设文本倒排引擎 (完全可选，纯碎占用独立内存不干扰底座)
    text_index: TextIndex,

    // 2. 元数据映射（文档型负载）—— 原始 JSON 紧凑存储，按访问惰性解析
    payloads: HashMap<NodeId, PayloadEntry>,

    // 3. 图谱邻接表 —— RW 使用内存 delta，ReadOnly/Immutable 可使用 mmap 基础块
    edges: HashMap<NodeId, Vec<Edge>>,
    mapped_graph: Option<crate::storage::graph_blocks::MappedGraphStore>,

    // 入度统计表：用于快速查询目标节点的被连接数（支持图谱反向抑制算法）
    in_degrees: HashMap<NodeId, usize>,

    // 反向入度哈希网：用于 O(1) 解决删除节点时的全库雪崩扫表
    incoming_edges: HashMap<NodeId, Vec<NodeId>>,

    // 边标签倒排索引：label → [(src, dst)]，加速图谱按标签查询
    label_index: HashMap<String, Vec<(NodeId, NodeId)>>,

    // 图统计缓存：generation 变化时失效，避免 Planner 每次扫描全图。
    graph_stats_cache: std::sync::RwLock<Option<(u64, GraphStats)>>,
    // 跨模统计按“字段 + 类型稳定 JSON 值”缓存；任意写操作后随 generation 失效。
    cross_modal_stats_cache: std::sync::RwLock<BTreeMap<(String, Vec<u8>), CrossModalStats>>,

    // 属性二级索引 Registry：字段名 → 类型稳定 Hash 索引
    property_indexes: PropertyIndexRegistry,

    // 节点不应期（疲劳状态）映射表：
    // 0 = 正常；1 = 疲劳中（被激活后，下一轮扩散大幅衰减，消费一次后清零）
    fatigue_map: std::sync::RwLock<HashMap<NodeId, u8>>,

    // 映射表：内部索引 (0, 1, 2...) 到 NodeId
    // 用于在 vectors 数组里定位数据位置
    indices_to_ids: Vec<NodeId>,
    ids_to_indices: HashMap<NodeId, usize>,

    // 行级哈希阵列：与 indices 同步，提供 O(1) 的布隆屏蔽检查，跳过极其昂贵的 JSON 反序列化
    fast_tags: Vec<u64>,

    // 空闲索引回收槽：O(1) 回收墓碑位置，防止物理大数组无尽膨胀
    free_slots: Vec<usize>,

    // 4. QuIVer BQ-native Vamana 图索引（惰性构建，N >= 10,000 时自动触发）
    //    冷热分离：QuIVer 内部 BQ sigs + 图拓扑 = hot，f32 向量 = cold
    //    事务安全：事务 commit 期间暂停 QuIVer 同步，commit 后由事务层统一同步
    quiver_index: Option<QuIVer>,
    auto_build_quiver: bool,
    /// 事务同步暂停标记：为 true 时 insert/delete/update_vector 不触发 QuIVer 增量操作
    quiver_sync_paused: bool,
}

impl<T: VectorType> MemTable<T> {
    /// 内部辅助：校验向量中是否包含 NaN 或 Infinity
    ///
    /// **为什么在写入时检查而不是查询时？**
    /// NaN 进入 mmap 基础层后会永久残留。在 BruteForce 并行昦描时，
    /// `score >= min_score`（NaN 比较永远为 false）会静默将该节点永久消失于检索结果，
    /// 且权会无任何错误提示。一旦进入就难以排查。
    ///
    /// `raw_insert` 是内部恢复路径（WAL 回放 / 文件重建），剛意不加此检查。
    #[inline]
    fn validate_vector(vector: &[T]) -> Result<()> {
        for elem in vector {
            let f = elem.to_f32();
            if f.is_nan() || f.is_infinite() {
                return Err(TriviumError::InvalidVector {
                    reason: "向量包含 NaN 或 Infinity，已拒绝插入以防止搜索污染 (Vector contains NaN or Infinity; insert rejected)".into(),
                });
            }
        }
        Ok(())
    }

    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            next_id: 1, // 从 1 开始，保留 0 作为特殊标记
            generation: 0,
            vector_generation: 0,
            property_generation: 0,
            vec_pool: VecPool::new(dim),
            bq_signatures: Vec::new(),
            bq_dirty: false,
            text_index: TextIndex::new(),
            payloads: HashMap::new(),
            edges: HashMap::new(),
            mapped_graph: None,
            in_degrees: HashMap::new(),
            incoming_edges: HashMap::new(),
            label_index: HashMap::new(),
            graph_stats_cache: std::sync::RwLock::new(None),
            cross_modal_stats_cache: std::sync::RwLock::new(BTreeMap::new()),
            property_indexes: PropertyIndexRegistry::default(),
            fatigue_map: std::sync::RwLock::new(HashMap::new()),
            indices_to_ids: Vec::new(),
            ids_to_indices: HashMap::new(),
            fast_tags: Vec::new(),
            free_slots: Vec::new(),
            quiver_index: None,
            auto_build_quiver: true,
            quiver_sync_paused: false,
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
            generation: 0,
            vector_generation: 0,
            property_generation: 0,
            vec_pool,
            bq_signatures: Vec::new(),
            bq_dirty: false,
            text_index: TextIndex::new(),
            payloads: HashMap::new(),
            edges: HashMap::new(),
            mapped_graph: None,
            in_degrees: HashMap::new(),
            incoming_edges: HashMap::new(),
            label_index: HashMap::new(),
            graph_stats_cache: std::sync::RwLock::new(None),
            cross_modal_stats_cache: std::sync::RwLock::new(BTreeMap::new()),
            property_indexes: PropertyIndexRegistry::default(),
            fatigue_map: std::sync::RwLock::new(HashMap::new()),
            indices_to_ids: Vec::new(),
            ids_to_indices: HashMap::new(),
            fast_tags: Vec::new(),
            free_slots: Vec::new(),
            quiver_index: None,
            auto_build_quiver: true,
            quiver_sync_paused: false,
        }
    }

    /// 暴露当前 ID 计数器值（供 save 时写入文件头）
    pub fn next_id_value(&self) -> NodeId {
        self.next_id
    }

    #[inline]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[inline]
    pub fn vector_generation(&self) -> u64 {
        self.vector_generation
    }

    #[inline]
    pub fn property_generation(&self) -> u64 {
        self.property_generation
    }

    #[inline]
    fn mark_property_changed(&mut self) {
        self.property_generation = self.property_generation.wrapping_add(1);
    }

    #[inline]
    fn mark_changed(&mut self, vectors_changed: bool) {
        self.generation = self.generation.wrapping_add(1);
        *self
            .graph_stats_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.cross_modal_stats_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        if vectors_changed {
            self.vector_generation = self.vector_generation.wrapping_add(1);
        }
    }

    /// 将 next_id 推进到至少 candidate 值（WAL 回放时防止 ID 复用）
    #[inline]
    pub fn advance_next_id(&mut self, candidate: NodeId) {
        if candidate > self.next_id {
            self.next_id = candidate;
        }
    }

    /// 估算为额外节点预留核心容器容量所需的新增堆内存。
    pub(crate) fn estimate_reserve_bytes(&self, additional: usize) -> Result<usize> {
        let checked = |value: Option<usize>, label: &str| {
            value.ok_or_else(|| TriviumError::CapacityAllocationFailed {
                reason: format!("{label}容量计算溢出"),
            })
        };
        let missing_vec = additional.saturating_sub(self.vec_pool.delta_spare_nodes());
        let vector_bytes = checked(
            missing_vec
                .checked_mul(self.dim)
                .and_then(|n| n.checked_mul(std::mem::size_of::<T>())),
            "向量增量层",
        )?;
        let missing_payload =
            additional.saturating_sub(self.payloads.capacity().saturating_sub(self.payloads.len()));
        let payload_bytes = checked(
            missing_payload
                .checked_mul(std::mem::size_of::<(NodeId, PayloadEntry)>() + 32)
                .and_then(|n| n.checked_mul(2)),
            "Payload 映射",
        )?;
        let missing_id_map = additional.saturating_sub(
            self.ids_to_indices
                .capacity()
                .saturating_sub(self.ids_to_indices.len()),
        );
        let id_map_bytes = checked(
            missing_id_map
                .checked_mul(std::mem::size_of::<(NodeId, usize)>() + 16)
                .and_then(|n| n.checked_mul(2)),
            "ID 映射",
        )?;
        let missing_slots = additional.saturating_sub(
            self.indices_to_ids
                .capacity()
                .saturating_sub(self.indices_to_ids.len()),
        );
        let slot_bytes = checked(
            missing_slots.checked_mul(std::mem::size_of::<NodeId>() + std::mem::size_of::<u64>()),
            "槽位数组",
        )?;
        checked(
            vector_bytes
                .checked_add(payload_bytes)
                .and_then(|n| n.checked_add(id_map_bytes))
                .and_then(|n| n.checked_add(slot_bytes)),
            "核心容器",
        )
    }

    /// 为后续插入预留核心容器容量。失败时不修改任何逻辑数据。
    pub(crate) fn try_reserve_for_insert(&mut self, additional: usize) -> Result<()> {
        if additional == 0 {
            return Ok(());
        }
        self.vec_pool.try_reserve_nodes(additional)?;
        self.payloads.try_reserve(additional).map_err(|error| {
            TriviumError::CapacityAllocationFailed {
                reason: format!("Payload 映射预留失败: {error}"),
            }
        })?;
        self.ids_to_indices
            .try_reserve(additional)
            .map_err(|error| TriviumError::CapacityAllocationFailed {
                reason: format!("ID 映射预留失败: {error}"),
            })?;
        self.indices_to_ids
            .try_reserve(additional)
            .map_err(|error| TriviumError::CapacityAllocationFailed {
                reason: format!("槽位数组预留失败: {error}"),
            })?;
        self.fast_tags.try_reserve(additional).map_err(|error| {
            TriviumError::CapacityAllocationFailed {
                reason: format!("快速标签数组预留失败: {error}"),
            }
        })?;
        Ok(())
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
    pub fn raw_insert(
        &mut self,
        id: NodeId,
        vector: &[T],
        payload: serde_json::Value,
    ) -> Result<()> {
        if id == 0 {
            return Err(TriviumError::InvalidInput("节点 ID 0 为内部保留值".into()));
        }
        if vector.len() != self.dim {
            return Err(TriviumError::DimensionMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }

        // 优先从空闲槽复活
        let sig = calculate_json_signature(&payload);
        let idx = if let Some(free_idx) = self.free_slots.pop() {
            self.vec_pool.update(free_idx, vector);
            self.indices_to_ids[free_idx] = id;
            self.fast_tags[free_idx] = sig;
            free_idx
        } else {
            let i = self.indices_to_ids.len();
            self.vec_pool.push(vector);
            self.indices_to_ids.push(id);
            self.fast_tags.push(sig);
            i
        };
        self.add_to_property_index(id, &payload);
        self.payloads.insert(id, PayloadEntry::from_value(payload));
        self.ids_to_indices.insert(id, idx);
        self.mark_changed(true);
        Ok(())
    }

    pub fn register_node(&mut self, id: NodeId, payload: serde_json::Value) -> Result<()> {
        let raw = serde_json::to_vec(&payload)
            .map_err(|error| TriviumError::InvalidInput(format!("Payload 序列化失败: {error}")))?;
        self.register_node_raw(id, &raw)
    }

    /// 从 mmap 加载时使用：仅注册紧凑 JSON 与映射关系，不解析 Payload DOM。
    pub fn register_node_raw(&mut self, id: NodeId, payload_raw: &[u8]) -> Result<()> {
        let idx = self.indices_to_ids.len();
        self.payloads
            .insert(id, PayloadEntry::from_raw(payload_raw)?);
        self.indices_to_ids.push(id);
        // 冷 Payload 尚未解析时使用全 1，布隆预过滤选择保守放行，避免假阴性。
        self.fast_tags.push(u64::MAX);
        self.ids_to_indices.insert(id, idx);
        Ok(())
    }

    /// 从持久化文件加载时遇到逻辑删除节点（Tombstone），仅推进内部索引映射空洞
    pub fn register_tombstone(&mut self) -> Result<()> {
        let idx = self.indices_to_ids.len();
        // NodeId=0 仅作为位置占位符，不在 payloads/ids_to_indices 中建立映射
        self.indices_to_ids.push(0);
        self.fast_tags.push(0);
        self.free_slots.push(idx); // 加入环保回收池
        Ok(())
    }

    /// 插入具有原生三维度属性的节点，保证原子性。
    pub fn insert(&mut self, vector: &[T], payload: serde_json::Value) -> Result<NodeId> {
        self.validate_insert(vector)?;

        let id = self.next_id;
        let next_id = id
            .checked_add(1)
            .ok_or_else(|| TriviumError::InvalidInput("节点 ID 空间已耗尽".into()))?;

        // 1. 记录向量（优先尝试从空闲槽复活，否则推入尾部增量层）
        let sig = calculate_json_signature(&payload);
        let idx = if let Some(free_idx) = self.free_slots.pop() {
            self.vec_pool.update(free_idx, vector); // 原地重生
            self.indices_to_ids[free_idx] = id;
            self.fast_tags[free_idx] = sig;
            free_idx
        } else {
            let i = self.indices_to_ids.len();
            self.vec_pool.push(vector); // 追尾拓展
            self.indices_to_ids.push(id);
            self.fast_tags.push(sig);
            i
        };

        // 2. 更新文档型负载
        self.add_to_property_index(id, &payload);
        self.payloads.insert(id, PayloadEntry::from_value(payload));

        // 3. 构建反向映射
        self.ids_to_indices.insert(id, idx);

        // 4. 属性索引已在 Payload 转入冷存储前维护

        // 5. 增量更新 QuIVer 索引（如果已构建且未暂停同步）
        if !self.quiver_sync_paused
            && let Some(ref mut quiver) = self.quiver_index
        {
            let vec_f32: Vec<f32> = vector.iter().map(|v| v.to_f32()).collect();
            let mut lcg = id.wrapping_mul(0x9E3779B97F4A7C15);
            quiver.insert(&vec_f32, id, idx, &mut lcg);
            quiver.dirty_count_inc(); // 追加也算增量变更
            if quiver.needs_rebuild() {
                self.quiver_index = None;
                tracing::debug!(
                    "QuIVer 索引增量变更超过 25%，已丢弃，下次搜索前将自动重建 (QuIVer index exceeded 25% dirty changes and was discarded; it will rebuild before the next search)"
                );
            }
        }

        self.mark_changed(true);
        self.next_id = next_id;
        Ok(id)
    }

    /// 使用外部指定的 ID 插入节点（例如从外部知识库导入数据）。
    /// 如果 ID 已存在会返回错误，并且会自动更新内部的 next_id 以免未来冲突。
    pub fn insert_with_id(
        &mut self,
        id: NodeId,
        vector: &[T],
        payload: serde_json::Value,
    ) -> Result<()> {
        self.validate_insert_with_id(id, vector)?;
        let advanced_next_id = if id >= self.next_id {
            Some(
                id.checked_add(1)
                    .ok_or_else(|| TriviumError::InvalidInput("节点 ID 空间已耗尽".into()))?,
            )
        } else {
            None
        };

        // 优先从空闲槽复活
        let sig = calculate_json_signature(&payload);
        let idx = if let Some(free_idx) = self.free_slots.pop() {
            self.vec_pool.update(free_idx, vector);
            self.indices_to_ids[free_idx] = id;
            self.fast_tags[free_idx] = sig;
            free_idx
        } else {
            let i = self.indices_to_ids.len();
            self.vec_pool.push(vector);
            self.indices_to_ids.push(id);
            self.fast_tags.push(sig);
            i
        };
        self.add_to_property_index(id, &payload);
        self.payloads.insert(id, PayloadEntry::from_value(payload));
        self.ids_to_indices.insert(id, idx);

        // 属性索引已在 Payload 转入冷存储前维护

        // 防御性推进分配器指针，避免后续普通 insert 撞车
        if let Some(next_id) = advanced_next_id {
            self.next_id = next_id;
        }

        // 增量更新 QuIVer 索引（如果已构建且未暂停同步）
        if !self.quiver_sync_paused
            && let Some(ref mut quiver) = self.quiver_index
        {
            let vec_f32: Vec<f32> = vector.iter().map(|v| v.to_f32()).collect();
            let mut lcg = id.wrapping_mul(0x9E3779B97F4A7C15);
            quiver.insert(&vec_f32, id, idx, &mut lcg);
            quiver.dirty_count_inc();
            if quiver.needs_rebuild() {
                self.quiver_index = None;
                tracing::debug!(
                    "QuIVer 索引增量变更超过 25%，已丢弃，下次搜索前将自动重建 (QuIVer index exceeded 25% dirty changes and was discarded; it will rebuild before the next search)"
                );
            }
        }

        self.mark_changed(true);
        Ok(())
    }

    pub(crate) fn validate_insert(&self, vector: &[T]) -> Result<()> {
        if vector.len() != self.dim {
            return Err(TriviumError::DimensionMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }
        Self::validate_vector(vector)
    }

    pub(crate) fn validate_insert_with_id(&self, id: NodeId, vector: &[T]) -> Result<()> {
        if id == 0 {
            return Err(TriviumError::InvalidInput("节点 ID 0 为内部保留值".into()));
        }
        if self.payloads.contains_key(&id) {
            return Err(TriviumError::NodeAlreadyExists(id));
        }
        self.validate_insert(vector)
    }

    pub(crate) fn validate_link(&self, src: NodeId, dst: NodeId) -> Result<()> {
        if !self.payloads.contains_key(&src) {
            return Err(TriviumError::NodeNotFound(src));
        }
        if !self.payloads.contains_key(&dst) {
            return Err(TriviumError::NodeNotFound(dst));
        }
        Ok(())
    }

    pub(crate) fn validate_delete(&self, id: NodeId) -> Result<()> {
        if self.payloads.contains_key(&id) {
            Ok(())
        } else {
            Err(TriviumError::NodeNotFound(id))
        }
    }

    pub(crate) fn validate_unlink(&self, src: NodeId) -> Result<()> {
        // 幂等语义：只要节点存在即可（edges 表只登记有出边的节点，
        // src 无出边时断开不存在的边应视为无操作，而非误报 NodeNotFound）。
        if self.payloads.contains_key(&src) {
            Ok(())
        } else {
            Err(TriviumError::NodeNotFound(src))
        }
    }

    pub(crate) fn validate_update_payload(&self, id: NodeId) -> Result<()> {
        self.validate_delete(id)
    }

    pub(crate) fn validate_update_vector(&self, id: NodeId, vector: &[T]) -> Result<()> {
        if vector.len() != self.dim {
            return Err(TriviumError::DimensionMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }
        Self::validate_vector(vector)?;
        self.validate_delete(id)
    }

    /// 在两节点间建立图谱边；(src, dst, label) 三元组唯一，重复 link 更新权重。
    pub fn link(&mut self, src: NodeId, dst: NodeId, label: String, weight: f32) -> Result<()> {
        self.upsert_edge(src, dst, label, weight, serde_json::Value::Null)
    }

    pub fn upsert_edge(
        &mut self,
        src: NodeId,
        dst: NodeId,
        label: String,
        weight: f32,
        metadata: serde_json::Value,
    ) -> Result<()> {
        self.validate_link(src, dst)?;
        if !weight.is_finite() {
            return Err(TriviumError::InvalidInput(
                "边权重必须是有限浮点数 (Edge weight must be finite)".into(),
            ));
        }

        let outgoing = self.edges.entry(src).or_default();
        if let Some(edge) = outgoing
            .iter_mut()
            .find(|edge| edge.target_id == dst && edge.label == label)
        {
            let mut changed = false;
            if edge.weight != weight {
                edge.weight = weight;
                changed = true;
            }
            if edge.metadata != metadata {
                edge.metadata = metadata;
                changed = true;
            }
            if changed {
                self.generation = self.generation.wrapping_add(1);
            }
            return Ok(());
        }

        outgoing.push(Edge {
            target_id: dst,
            label: label.clone(),
            weight,
            metadata,
        });
        *self.in_degrees.entry(dst).or_insert(0) += 1;
        if !self.incoming_edges.entry(dst).or_default().contains(&src) {
            self.incoming_edges.entry(dst).or_default().push(src);
        }
        self.label_index.entry(label).or_default().push((src, dst));
        self.mark_changed(false);
        Ok(())
    }

    pub fn get_edge(&self, src: NodeId, dst: NodeId, label: &str) -> Option<&Edge> {
        self.get_edges(src)?
            .iter()
            .find(|edge| edge.target_id == dst && edge.label == label)
    }

    pub fn graph_stats(&self) -> GraphStats {
        if let Some((generation, stats)) = self
            .graph_stats_cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            && *generation == self.generation
        {
            return stats.clone();
        }
        let node_count = self.payloads.len();
        let mut edge_count = 0usize;
        let mut max_out_degree = 0usize;
        let mut label_pairs = BTreeMap::<String, Vec<(NodeId, NodeId)>>::new();
        let mut out_degree_values = Vec::with_capacity(node_count);
        for id in self.payloads.keys().copied() {
            let edges = self.get_edges(id).unwrap_or_default();
            edge_count = edge_count.saturating_add(edges.len());
            max_out_degree = max_out_degree.max(edges.len());
            out_degree_values.push(edges.len());
            for edge in edges {
                label_pairs
                    .entry(edge.label.clone())
                    .or_default()
                    .push((id, edge.target_id));
            }
        }
        let isolated_node_count = self
            .payloads
            .keys()
            .filter(|id| {
                self.get_edges(**id).is_none_or(<[Edge]>::is_empty)
                    && self.in_degrees.get(id).copied().unwrap_or_default() == 0
            })
            .count();
        let max_in_degree = self.in_degrees.values().copied().max().unwrap_or(0);
        let label_stats = label_pairs
            .into_iter()
            .map(|(label, pairs)| {
                let stats = LabelStats {
                    edge_count: pairs.len(),
                    distinct_source_count: pairs
                        .iter()
                        .map(|pair| pair.0)
                        .collect::<HashSet<_>>()
                        .len(),
                    distinct_target_count: pairs
                        .iter()
                        .map(|pair| pair.1)
                        .collect::<HashSet<_>>()
                        .len(),
                };
                (label, stats)
            })
            .collect::<BTreeMap<_, _>>();
        let in_degrees = self
            .payloads
            .keys()
            .map(|id| self.in_degrees.get(id).copied().unwrap_or(0));
        let stats = GraphStats {
            node_count,
            edge_count,
            isolated_node_count,
            label_count: label_stats.len(),
            avg_out_degree: if node_count == 0 {
                0.0
            } else {
                edge_count as f64 / node_count as f64
            },
            avg_in_degree: if node_count == 0 {
                0.0
            } else {
                edge_count as f64 / node_count as f64
            },
            max_out_degree,
            max_in_degree,
            label_stats,
            out_degree_histogram: degree_histogram(out_degree_values.into_iter()),
            in_degree_histogram: degree_histogram(in_degrees),
        };
        *self
            .graph_stats_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some((self.generation, stats.clone()));
        stats
    }

    pub fn validate_graph(&self) -> GraphIntegrityReport {
        let mut expected_incoming: HashMap<NodeId, HashSet<NodeId>> = HashMap::new();
        let mut expected_degrees: HashMap<NodeId, usize> = HashMap::new();
        let mut expected_labels: HashMap<String, HashSet<(NodeId, NodeId)>> = HashMap::new();
        let mut dangling_edges = 0;
        let mut duplicate_edges = 0;

        for (&src, edges) in &self.edges {
            let mut seen = HashSet::new();
            for edge in edges {
                if !self.payloads.contains_key(&src) || !self.payloads.contains_key(&edge.target_id)
                {
                    dangling_edges += 1;
                    continue;
                }
                if !seen.insert((edge.target_id, edge.label.as_str())) {
                    duplicate_edges += 1;
                    continue;
                }
                expected_incoming
                    .entry(edge.target_id)
                    .or_default()
                    .insert(src);
                *expected_degrees.entry(edge.target_id).or_default() += 1;
                expected_labels
                    .entry(edge.label.clone())
                    .or_default()
                    .insert((src, edge.target_id));
            }
        }

        let actual_incoming: HashMap<NodeId, HashSet<NodeId>> = self
            .incoming_edges
            .iter()
            .map(|(&id, sources)| (id, sources.iter().copied().collect()))
            .filter(|(_, sources): &(NodeId, HashSet<NodeId>)| !sources.is_empty())
            .collect();
        let actual_labels: HashMap<String, HashSet<(NodeId, NodeId)>> = self
            .label_index
            .iter()
            .map(|(label, pairs)| {
                (
                    label.clone(),
                    pairs.iter().copied().collect::<HashSet<(NodeId, NodeId)>>(),
                )
            })
            .filter(|(_, pairs)| !pairs.is_empty())
            .collect();
        let incoming_index_mismatches = usize::from(actual_incoming != expected_incoming);
        let degree_index_mismatches = usize::from(self.in_degrees != expected_degrees);
        let label_index_mismatches = usize::from(actual_labels != expected_labels);
        GraphIntegrityReport {
            dangling_edges,
            duplicate_edges,
            incoming_index_mismatches,
            degree_index_mismatches,
            label_index_mismatches,
            valid: dangling_edges == 0
                && duplicate_edges == 0
                && incoming_index_mismatches == 0
                && degree_index_mismatches == 0
                && label_index_mismatches == 0,
        }
    }

    pub fn repair_graph_indexes(&mut self) -> GraphRepairReport {
        let payloads = &self.payloads;
        let mut removed_dangling_edges = 0;
        let mut removed_duplicate_edges = 0;
        self.edges.retain(|src, edges| {
            if !payloads.contains_key(src) {
                removed_dangling_edges += edges.len();
                return false;
            }
            let mut seen = HashSet::new();
            edges.retain(|edge| {
                if !payloads.contains_key(&edge.target_id) {
                    removed_dangling_edges += 1;
                    return false;
                }
                if !seen.insert((edge.target_id, edge.label.clone())) {
                    removed_duplicate_edges += 1;
                    return false;
                }
                true
            });
            !edges.is_empty()
        });

        self.incoming_edges.clear();
        self.in_degrees.clear();
        self.label_index.clear();
        for (&src, edges) in &self.edges {
            for edge in edges {
                *self.in_degrees.entry(edge.target_id).or_default() += 1;
                let sources = self.incoming_edges.entry(edge.target_id).or_default();
                if !sources.contains(&src) {
                    sources.push(src);
                }
                self.label_index
                    .entry(edge.label.clone())
                    .or_default()
                    .push((src, edge.target_id));
            }
        }
        for sources in self.incoming_edges.values_mut() {
            sources.sort_unstable();
        }
        for pairs in self.label_index.values_mut() {
            pairs.sort_unstable();
            pairs.dedup();
        }
        self.mark_changed(false);
        GraphRepairReport {
            removed_dangling_edges,
            removed_duplicate_edges,
            rebuilt_indexes: true,
        }
    }

    // ── 节点不应期（疲劳）接口 ────────────────────────────────────────────────

    /// 将一批节点标记为「疲劳」（被本轮扩散激活的节点）
    pub fn mark_fatigued(&self, ids: &[NodeId]) {
        if let Ok(mut map) = self.fatigue_map.write() {
            for &id in ids {
                map.insert(id, 1);
            }
        }
    }

    /// 查询指定节点的疲劳状态
    /// 0 = 正常，1 = 疲劳中
    pub fn get_fatigue(&self, id: NodeId) -> u8 {
        if let Ok(map) = self.fatigue_map.read() {
            *map.get(&id).unwrap_or(&0)
        } else {
            0
        }
    }

    /// 消耗一次疲劳（在扩散使用后调用，清零不应期）
    pub fn clear_fatigue(&self) {
        if let Ok(mut map) = self.fatigue_map.write() {
            map.clear();
        }
    }

    pub fn consume_fatigue(&self, id: NodeId) {
        if let Ok(mut map) = self.fatigue_map.write()
            && let Some(f) = map.get_mut(&id)
        {
            *f = 0;
        }
    }

    /// 批量消耗疲劳（由扩散引擎在每轮迭代末调用）
    pub fn consume_fatigue_batch(&self, ids: &[NodeId]) {
        if let Ok(mut map) = self.fatigue_map.write() {
            for &id in ids {
                if let Some(f) = map.get_mut(&id) {
                    *f = 0;
                }
            }
        }
    }

    pub fn auto_quiver_build_needed(&self) -> bool {
        self.dim <= crate::index::bq::MAX_BQ_DIM
            && self.auto_build_quiver
            && self.payloads.len() >= 10_000
            && self.quiver_index.is_none()
    }

    pub fn search_cache_needs_prepare(&self, materialize_flat: bool) -> bool {
        let total = self.vec_pool.total_count();
        let bq_needed = self.bq_signatures.len() != total || self.bq_dirty;
        let flat_needed = (materialize_flat || self.quiver_index.is_none())
            && self.vec_pool.cache_needs_rebuild();
        bq_needed || flat_needed
    }

    pub fn prepare_search_cache(&mut self, materialize_flat: bool) {
        let total = self.vec_pool.total_count();
        if self.bq_signatures.len() != total || self.bq_dirty {
            self.rebuild_bq_signatures(total);
            self.bq_dirty = false;
        }
        if materialize_flat || self.quiver_index.is_none() {
            self.vec_pool.ensure_cache();
        }
    }

    pub fn search_needs_prepare(&self, materialize_flat: bool) -> bool {
        self.auto_quiver_build_needed() || self.search_cache_needs_prepare(materialize_flat)
    }

    /// 估算一次 QuIVer 全量构建相对当前常驻结构的额外峰值。
    pub(crate) fn quiver_build_peak_bytes(&self, config: &QuIVerConfig) -> usize {
        let nodes = self.payloads.len();
        let chunks = self.dim.div_ceil(64).max(1);
        let stride = config.m.saturating_mul(4).saturating_add(1);
        let signatures = nodes
            .saturating_mul(chunks)
            .saturating_mul(std::mem::size_of::<u64>())
            .saturating_mul(2);
        let adjacency = nodes
            .saturating_mul(stride)
            .saturating_mul(std::mem::size_of::<u32>())
            .saturating_mul(2);
        let mappings = nodes.saturating_mul(
            std::mem::size_of::<u64>()
                + std::mem::size_of::<usize>()
                + std::mem::size_of::<u32>()
                + std::mem::size_of::<u8>()
                + 24,
        );
        let worker_bitsets = nodes.div_ceil(8).saturating_mul(64);
        signatures
            .saturating_add(adjacency)
            .saturating_add(mappings)
            .saturating_add(worker_bitsets)
    }

    /// 仅准备持久化需要的 BQ 与连续向量缓存，不触发 ANN 构建。
    pub fn prepare_persistence_cache(&mut self, materialize_flat: bool) {
        let total = self.vec_pool.total_count();
        if self.bq_signatures.len() != total || self.bq_dirty {
            self.rebuild_bq_signatures(total);
            self.bq_dirty = false;
        }
        if materialize_flat {
            self.vec_pool.ensure_cache();
        }
    }

    /// 确保 BQ 签名/向量缓存已就绪，并自动管理 QuIVer 索引生命周期
    ///
    /// # 冷热分离与内存控制
    /// `materialize_flat` 控制是否物化全量 merged 缓存（把整个 mmap 复制入堆）：
    /// - `true`：调用方随后需要连续的 `flat_vectors()`（暴力全扫 / 残差影子查询 /
    ///   Rom 持久化），必须物化。
    /// - `false`：纯 QuIVer 检索路径——冷向量按需从 mmap 读取，无需 merged。
    ///
    /// 注意：即便传入 `false`，若没有可用的 QuIVer 索引（查询将回退到暴力全扫），
    /// 仍会物化 merged 以保证暴力路径可用。
    ///
    /// BQ 签名与 QuIVer 构建均采用按 slot 流式读取 mmap 的方式，不依赖 merged 缓存。
    pub fn ensure_vectors_cache(&mut self, materialize_flat: bool) {
        // 1. BQ 签名（流式重建，不触发 merged 缓存）
        let total = self.vec_pool.total_count();
        if self.bq_signatures.len() != total || self.bq_dirty {
            self.rebuild_bq_signatures(total);
            self.bq_dirty = false;
        }

        // 2. QuIVer 自动构建：当数据量 >= 10,000 且索引不存在时自动触发
        let active = self.payloads.len();
        if self.dim <= crate::index::bq::MAX_BQ_DIM
            && self.auto_build_quiver
            && active >= 10_000
            && self.quiver_index.is_none()
        {
            self.build_quiver_impl(&QuIVerConfig::default());
        }

        // 3. 仅在确实需要连续 flat 数组时才物化 merged 缓存：
        //    - 调用方显式要求（Rom 持久化 / 残差 / 强制暴力）
        //    - 或没有 QuIVer 索引（查询将回退暴力全扫，需要 flat）
        if materialize_flat || self.quiver_index.is_none() {
            self.vec_pool.ensure_cache();
        }
    }

    /// 原位重建 BQ 签名，复用旧容量，避免新旧完整数组同时驻留。
    fn rebuild_bq_signatures(&mut self, total: usize) {
        self.bq_signatures.clear();
        self.bq_signatures.reserve(total);
        for i in 0..total {
            match self.vec_pool.get(i) {
                Some(v) => self.bq_signatures.push(BqSignature::from_vector(v)),
                // 兜底以防向量池维度异常或越界
                None => self.bq_signatures.push(BqSignature::empty()),
            }
        }
    }

    /// 获取 BQ 量化初筛签名
    pub fn get_bq_signature(&self, index: usize) -> Option<BqSignature> {
        self.bq_signatures.get(index).copied()
    }

    /// 直接暴露 BQ 签名数组的连续内存切片，用于热循环零开销扫描
    #[inline]
    pub fn bq_signatures_slice(&self) -> &[BqSignature] {
        &self.bq_signatures
    }

    /// 直接暴露 Fast Tags (Bloom 签名) 数组切片，O(1) 极大加速属性过滤
    #[inline]
    pub fn fast_tags_slice(&self) -> &[u64] {
        &self.fast_tags
    }

    /// 按 NodeId 获取 Fast Tag，供查询时导航做近似属性信号判断。
    #[inline]
    pub(crate) fn fast_tag_for_id(&self, id: NodeId) -> Option<u64> {
        self.ids_to_indices
            .get(&id)
            .and_then(|&slot| self.fast_tags.get(slot))
            .copied()
    }

    /// 从持久化文件恢复 BQ 签名数组（跳过重建）
    pub fn set_bq_signatures(&mut self, sigs: Vec<BqSignature>) {
        self.bq_signatures = sigs;
        self.bq_dirty = false; // 刚恢复的签名是干净的
    }

    /// 从持久化文件恢复 QuIVer 索引（跳过重建）
    pub fn set_quiver_index(&mut self, quiver: QuIVer) {
        if self.dim <= crate::index::bq::MAX_BQ_DIM {
            self.quiver_index = Some(quiver);
        }
    }

    /// 设置 QuIVer 同步暂停标记（事务 commit 期间暂停，commit 后恢复）
    #[inline]
    pub fn set_quiver_sync_paused(&mut self, paused: bool) {
        self.quiver_sync_paused = paused;
    }

    /// 获取 QuIVer 图索引引用（如果已构建）
    #[inline]
    pub fn quiver(&self) -> Option<&QuIVer> {
        self.quiver_index.as_ref()
    }

    /// 手动构建 QuIVer BQ-native Vamana 索引
    ///
    /// 通常不需要手动调用——`ensure_vectors_cache()` 会在 N >= 10,000 时自动构建。
    /// 此方法用于提前构建或使用自定义配置。
    ///
    /// **冷热分离**：
    /// - Hot: 2-bit BQ 签名 + Vamana 图拓扑（常驻内存，~2 bits/dim/node）
    /// - Cold: f32 原始向量（仅精排时按需访问，由 QuIVer 内部管理）
    ///
    /// **事务安全**：`delete()` / `update_vector()` 会使索引自动失效，
    /// 下次搜索前由 `ensure_vectors_cache()` 自动重建。
    pub fn set_auto_build_quiver(&mut self, enabled: bool) {
        self.auto_build_quiver = enabled;
    }

    pub fn build_quiver(&mut self, config: &QuIVerConfig) {
        if self.dim > crate::index::bq::MAX_BQ_DIM {
            self.quiver_index = None;
            return;
        }
        // 确保 BQ 签名就绪（流式，不物化 merged），然后构建索引
        let total = self.vec_pool.total_count();
        if self.bq_signatures.len() != total || self.bq_dirty {
            self.rebuild_bq_signatures(total);
            self.bq_dirty = false;
        }
        self.build_quiver_impl(config);
    }

    pub(crate) fn quiver_build_snapshot(&self) -> Option<QuiverBuildSnapshot> {
        if self.dim > crate::index::bq::MAX_BQ_DIM
            || self.vec_pool.total_count() == 0
            || self.payloads.is_empty()
        {
            return None;
        }
        let mut signatures = Bq2Store::new(self.dim);
        signatures.reserve(self.payloads.len());
        let mut ids = Vec::with_capacity(self.payloads.len());
        let mut slots = Vec::with_capacity(self.payloads.len());
        for (slot, &node_id) in self.indices_to_ids.iter().enumerate() {
            if node_id == 0 || !self.payloads.contains_key(&node_id) {
                continue;
            }
            if let Some(vector) = self.vec_pool.get(slot) {
                signatures.push_from_vector(vector);
                ids.push(node_id);
                slots.push(slot);
            }
        }
        (!ids.is_empty()).then_some(QuiverBuildSnapshot {
            generation: self.vector_generation,
            dim: self.dim,
            signatures,
            ids,
            slots,
        })
    }

    pub(crate) fn build_quiver_snapshot(
        snapshot: QuiverBuildSnapshot,
        config: &QuIVerConfig,
    ) -> QuIVer {
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::QuiverBuildStarted);
        let mut index = QuIVer::new(snapshot.dim, config);
        index.batch_build_from_store(&snapshot.ids, &snapshot.slots, snapshot.signatures);
        index
    }

    pub(crate) fn publish_quiver_if_current(
        &mut self,
        source_generation: u64,
        index: QuIVer,
    ) -> bool {
        if self.vector_generation != source_generation {
            return false;
        }
        self.quiver_index = Some(index);
        self.vec_pool.advise_random();
        true
    }

    /// QuIVer 构建的内部实现（不调用 ensure_vectors_cache，避免递归）
    ///
    /// 按 slot 流式从 mmap 零拷贝读取向量并转 f32，不物化全量 merged 缓存，
    /// 避免构建期把整个冷数据集复制入堆。
    fn build_quiver_impl(&mut self, config: &QuIVerConfig) {
        let dim = self.dim;
        if self.vec_pool.total_count() == 0 || self.payloads.is_empty() {
            self.quiver_index = None;
            return;
        }

        // 收集活跃节点的向量、ID 和 slot 索引（跳过 tombstone），按需从 mmap 读取
        let mut vecs_f32: Vec<f32> = Vec::with_capacity(self.payloads.len() * dim);
        let mut ids: Vec<u64> = Vec::with_capacity(self.payloads.len());
        let mut slot_idxs: Vec<usize> = Vec::with_capacity(self.payloads.len());
        let slot_count = self.indices_to_ids.len();
        for i in 0..slot_count {
            let node_id = self.indices_to_ids[i];
            if node_id == 0 {
                continue; // tombstone，跳过
            }
            if !self.payloads.contains_key(&node_id) {
                continue;
            }
            if let Some(v) = self.vec_pool.get(i) {
                vecs_f32.extend(v.iter().map(|x| x.to_f32()));
                ids.push(node_id);
                slot_idxs.push(i);
            }
        }

        if ids.is_empty() {
            self.quiver_index = None;
            return;
        }

        let mut index = QuIVer::new(dim, config);
        index.batch_build_experimental_v2(&vecs_f32, &ids, &slot_idxs);
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::BeforeQuiverPublish);
        self.quiver_index = Some(index);

        // QuIVer 精排按 slot 随机读取冷向量，提示 OS 关闭顺序预读以降低 PageCache 占用
        self.vec_pool.advise_random();

        tracing::info!(
            "QuIVer 索引自动构建完成 (QuIVer index auto-built): {} 个节点，dim={}",
            ids.len(),
            dim
        );
    }

    /// 使 QuIVer 索引失效（delete / update_vector 后调用）
    ///
    /// 如果 QuIVer 索引存在且支持增量删除，优先用 soft_delete。
    /// 当退化超过 25% 时才丢弃索引，下次搜索前自动重建。
    fn invalidate_quiver_for_delete(&mut self, node_id: u64) {
        if let Some(ref mut quiver) = self.quiver_index {
            quiver.soft_delete(node_id);
            if quiver.needs_rebuild() {
                self.quiver_index = None;
                tracing::debug!(
                    "QuIVer 索引退化超过 25%，已丢弃，下次搜索前将自动重建 (QuIVer index exceeded 25% degradation and was discarded; it will rebuild before the next search)"
                );
            }
        }
    }

    /// 事务 commit 后的 QuIVer 增量同步（Phase 5: 分离时间线）
    ///
    /// 遍历已提交的 WAL 条目，将 insert/delete/update_vector 同步到 QuIVer。
    /// 在 Phase 4（Infallible Apply）成功后调用，不需要回滚能力。
    pub fn quiver_sync_tx_entries(&mut self, entries: &[crate::storage::wal::WalEntry<T>]) {
        use crate::storage::wal::WalEntry;

        let had_quiver = self.quiver_index.is_some();

        for entry in entries {
            match entry {
                WalEntry::Insert { id, vector, .. } => {
                    if let Some(ref mut quiver) = self.quiver_index {
                        let vec_f32: Vec<f32> = vector.iter().map(|v| v.to_f32()).collect();
                        let slot_idx = self.ids_to_indices.get(id).copied().unwrap_or(0);
                        let mut lcg = id.wrapping_mul(0x9E3779B97F4A7C15);
                        quiver.insert(&vec_f32, *id, slot_idx, &mut lcg);
                        quiver.dirty_count_inc();
                    }
                }
                WalEntry::Delete { id } => {
                    self.invalidate_quiver_for_delete(*id);
                }
                WalEntry::UpdateVector { id, vector } => {
                    // soft_delete 旧向量 + incremental_insert 新向量
                    if let Some(ref mut quiver) = self.quiver_index {
                        quiver.soft_delete(*id);
                        let vec_f32: Vec<f32> = vector.iter().map(|v| v.to_f32()).collect();
                        let slot_idx = self.ids_to_indices.get(id).copied().unwrap_or(0);
                        let mut lcg = id.wrapping_mul(0x9E3779B97F4A7C15);
                        quiver.insert(&vec_f32, *id, slot_idx, &mut lcg);
                    }
                    // 退化检查
                    if self
                        .quiver_index
                        .as_ref()
                        .is_some_and(|q| q.needs_rebuild())
                    {
                        self.quiver_index = None;
                        tracing::debug!(
                            "QuIVer 索引退化超过 25%，已丢弃，下次搜索前将自动重建 (QuIVer index exceeded 25% degradation and was discarded; it will rebuild before the next search)"
                        );
                    }
                }
                _ => {} // Link/Unlink/UpdatePayload 不影响 QuIVer
            }
        }

        if had_quiver
            && self
                .quiver_index
                .as_ref()
                .is_none_or(|quiver| quiver.needs_rebuild())
        {
            self.build_quiver(&QuIVerConfig::default());
        }
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
        self.payloads.get(&id).map(PayloadEntry::get)
    }

    pub(crate) fn get_payload_raw(&self, id: NodeId) -> Option<&[u8]> {
        self.payloads.get(&id).map(PayloadEntry::raw)
    }

    pub fn get_edges(&self, id: NodeId) -> Option<&[Edge]> {
        self.edges
            .get(&id)
            .map(Vec::as_slice)
            .or_else(|| self.mapped_graph.as_ref()?.edges(id))
    }

    pub fn set_mapped_graph(&mut self, graph: crate::storage::graph_blocks::MappedGraphStore) {
        self.edges.clear();
        self.incoming_edges.clear();
        self.label_index.clear();
        self.in_degrees.clear();
        for target in self.all_node_ids() {
            let degree = graph.incoming(target).len();
            if degree > 0 {
                self.in_degrees.insert(target, degree);
            }
        }
        self.mapped_graph = Some(graph);
    }

    pub fn mapped_graph_bytes(&self) -> usize {
        self.mapped_graph
            .as_ref()
            .map_or(0, |graph| graph.mapped_bytes())
    }

    /// 获取指向 id 的所有源节点（反向边）
    pub fn get_incoming_sources(&self, id: NodeId) -> &[NodeId] {
        self.incoming_edges
            .get(&id)
            .map(Vec::as_slice)
            .or_else(|| self.mapped_graph.as_ref().map(|graph| graph.incoming(id)))
            .unwrap_or(&[])
    }

    /// 获取指向 id 的完整入边，可选按标签过滤。
    pub fn get_incoming_edges(
        &self,
        id: NodeId,
        label: Option<&str>,
    ) -> Vec<crate::node::IncomingEdge> {
        let mut result = Vec::new();
        for &source_id in self.get_incoming_sources(id) {
            if let Some(edges) = self.get_edges(source_id) {
                result.extend(
                    edges
                        .iter()
                        .filter(|edge| {
                            edge.target_id == id
                                && label.is_none_or(|expected| edge.label == expected)
                        })
                        .map(|edge| crate::node::IncomingEdge {
                            source_id,
                            target_id: id,
                            label: edge.label.clone(),
                            weight: edge.weight,
                            metadata: edge.metadata.clone(),
                        }),
                );
            }
        }
        result.sort_by(|a, b| {
            a.source_id
                .cmp(&b.source_id)
                .then_with(|| a.label.cmp(&b.label))
        });
        result
    }

    /// 按标签查询所有边 (src, dst) 对，O(1) 查找
    pub fn get_edges_by_label(&self, label: &str) -> &[(NodeId, NodeId)] {
        self.label_index
            .get(label)
            .map(Vec::as_slice)
            .or_else(|| {
                self.mapped_graph
                    .as_ref()
                    .map(|graph| graph.by_label(label))
            })
            .unwrap_or(&[])
    }

    /// 按 Payload JSON 字段值查找节点（遍历匹配，适用于小规模场景）
    ///
    /// 如需高性能可后续引入二级属性索引，当前先拿 field_index 的语义位置占好
    pub fn find_nodes_by_field(&self, field: &str, value: &serde_json::Value) -> Vec<NodeId> {
        self.payloads
            .iter()
            .filter_map(|(&id, payload)| {
                if payload.get().get(field) == Some(value) {
                    Some(id)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn find_nodes_by_field_parallel(
        &self,
        field: &str,
        value: &serde_json::Value,
        threads: usize,
    ) -> Vec<NodeId> {
        if threads <= 1 {
            return self.find_nodes_by_field(field, value);
        }
        use rayon::prelude::*;
        let mut ids = self
            .payloads
            .par_iter()
            .filter_map(|(&id, payload)| (payload.get().get(field) == Some(value)).then_some(id))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }

    // ════════════════════════════════════════════════════════
    //  属性二级索引 API
    // ════════════════════════════════════════════════════════

    /// 注册属性索引：对指定字段建立倒排索引，并回填所有已有节点
    pub fn register_property_index(&mut self, field: &str) {
        let payloads = &self.payloads;
        self.property_indexes.register(
            field,
            payloads.iter().map(|(&id, payload)| (id, payload.get())),
        );
    }

    pub fn register_composite_property_index(&mut self, fields: &[String]) {
        let payloads = &self.payloads;
        self.property_indexes.register_composite(
            fields,
            payloads.iter().map(|(&id, payload)| (id, payload.get())),
        );
    }

    pub fn find_by_composite_property_index(
        &self,
        equalities: &[(String, serde_json::Value)],
    ) -> Option<(Vec<String>, Vec<NodeId>)> {
        self.property_indexes.composite_lookup(equalities)
    }

    pub fn find_by_composite_property_range(
        &self,
        equalities: &[(String, serde_json::Value)],
        range_field: &str,
        op: std::cmp::Ordering,
        inclusive: bool,
        value: &serde_json::Value,
        descending: bool,
        limit: Option<usize>,
    ) -> Option<(Vec<String>, Vec<NodeId>)> {
        self.property_indexes.composite_range_lookup(
            equalities,
            range_field,
            op,
            inclusive,
            value,
            descending,
            limit,
        )
    }

    pub fn register_bitmap_property_index(&mut self, field: &str) {
        let payloads = &self.payloads;
        self.property_indexes.register_bitmap(
            field,
            payloads.iter().map(|(&id, payload)| (id, payload.get())),
        );
    }

    pub fn find_by_bitmap_property_index(
        &self,
        field: &str,
        value: &serde_json::Value,
    ) -> Option<Vec<NodeId>> {
        self.property_indexes.bitmap_lookup(field, value)
    }

    pub fn find_by_bitmap_intersection(
        &self,
        equalities: &[(String, serde_json::Value)],
    ) -> Option<Vec<NodeId>> {
        self.property_indexes.bitmap_intersection(equalities)
    }

    pub fn drop_composite_property_index(&mut self, fields: &[String]) {
        self.property_indexes.drop_composite_index(fields);
    }

    pub fn drop_bitmap_property_index(&mut self, field: &str) {
        self.property_indexes.drop_bitmap_index(field);
    }

    pub fn register_ordered_property_index(&mut self, field: &str) {
        let payloads = &self.payloads;
        self.property_indexes.register_ordered(
            field,
            payloads.iter().map(|(&id, payload)| (id, payload.get())),
        );
    }

    pub fn drop_ordered_property_index(&mut self, field: &str) {
        self.property_indexes.drop_ordered_index(field);
    }

    pub fn find_by_property_range(
        &self,
        field: &str,
        op: std::cmp::Ordering,
        inclusive: bool,
        value: &serde_json::Value,
        descending: bool,
        limit: Option<usize>,
    ) -> Option<Vec<NodeId>> {
        self.property_indexes
            .range_lookup(field, op, inclusive, value, descending, limit)
    }

    pub fn ordered_property_ids(
        &self,
        field: &str,
        descending: bool,
        limit: Option<usize>,
    ) -> Option<Vec<NodeId>> {
        self.property_indexes.ordered_ids(field, descending, limit)
    }

    pub fn has_ordered_property_index(&self, field: &str) -> bool {
        self.property_indexes.contains_ordered(field)
    }

    /// 删除属性索引
    pub fn drop_property_index(&mut self, field: &str) {
        self.property_indexes.drop_index(field);
    }

    /// 查询属性索引。字段已建索引时即使值不存在也返回空切片。
    pub fn find_by_property_index(
        &self,
        field: &str,
        value: &serde_json::Value,
    ) -> Option<Vec<NodeId>> {
        self.property_indexes.lookup(field, value)
    }

    /// 检查字段是否有属性索引
    pub fn has_property_index(&self, field: &str) -> bool {
        self.property_indexes.contains(field)
    }

    /// 获取所有已注册索引的字段名
    pub fn indexed_field_names(&self) -> HashSet<&str> {
        self.property_indexes.field_names()
    }

    pub fn property_indexes(&self) -> &PropertyIndexRegistry {
        &self.property_indexes
    }

    pub fn property_index_stats(&self) -> Vec<crate::index::property::PropertyIndexStats> {
        self.property_indexes.stats()
    }

    pub fn column_pair_stats(&self) -> Vec<crate::index::property::ColumnPairStats> {
        let mut output = Vec::new();
        for fields in self.property_indexes.composite_definitions() {
            for left_index in 0..fields.len() {
                for right_index in left_index + 1..fields.len() {
                    let left_field = &fields[left_index];
                    let right_field = &fields[right_index];
                    let mut left_values = HashSet::new();
                    let mut right_values = HashSet::new();
                    let mut joint_values = HashSet::new();
                    let mut sampled_rows = 0usize;
                    for payload in self.payloads.values().map(PayloadEntry::get) {
                        let (Some(left), Some(right)) =
                            (payload.get(left_field), payload.get(right_field))
                        else {
                            continue;
                        };
                        let (Ok(left), Ok(right)) =
                            (serde_json::to_vec(left), serde_json::to_vec(right))
                        else {
                            continue;
                        };
                        left_values.insert(left.clone());
                        right_values.insert(right.clone());
                        joint_values.insert((left, right));
                        sampled_rows += 1;
                    }
                    output.push(crate::index::property::ColumnPairStats {
                        left_field: left_field.clone(),
                        right_field: right_field.clone(),
                        left_distinct: left_values.len(),
                        right_distinct: right_values.len(),
                        joint_distinct: joint_values.len(),
                        sampled_rows,
                    });
                }
            }
        }
        output
    }

    pub fn cross_modal_stats(
        &self,
        field: &str,
        value: &serde_json::Value,
    ) -> Option<CrossModalStats> {
        const MAX_SAMPLES: usize = 1024;
        let encoded_value = serde_json::to_vec(value).ok()?;
        let cache_key = (field.to_owned(), encoded_value);
        if let Some(stats) = self
            .cross_modal_stats_cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&cache_key)
            .filter(|stats| stats.generation == self.generation)
        {
            return Some(stats.clone());
        }
        let ids = self.find_by_property_index(field, value)?;
        let sampled = ids.len().min(MAX_SAMPLES);
        if sampled == 0 {
            return None;
        }
        let graph = self.graph_stats();
        let vector_density = self
            .quiver_index
            .as_ref()
            .and_then(|quiver| quiver.vector_density_skew(&ids, MAX_SAMPLES));
        let stats = if graph.edge_count == 0 {
            CrossModalStats {
                degree_skew: 1.0,
                fanout_skew: 1.0,
                vector_density_skew: vector_density.map(|(skew, _)| skew),
                sampled: vector_density.map_or(sampled, |(_, count)| count.min(sampled)),
                generation: self.generation,
            }
        } else {
            let degrees = ids
                .iter()
                .take(sampled)
                .map(|id| self.get_edges(*id).map_or(0, <[Edge]>::len))
                .collect::<Vec<_>>();
            let sample_mean = degrees.iter().sum::<usize>() as f64 / sampled as f64;
            let sample_first = degrees.iter().sum::<usize>() as f64;
            let sample_second = degrees
                .iter()
                .map(|degree| degree.saturating_mul(*degree))
                .sum::<usize>() as f64;
            let sample_fanout = if sample_first > 0.0 {
                sample_second / sample_first
            } else {
                0.0
            };
            let global_first = self
                .payloads
                .keys()
                .map(|id| self.get_edges(*id).map_or(0, <[Edge]>::len))
                .sum::<usize>() as f64;
            let global_second = self
                .payloads
                .keys()
                .map(|id| {
                    let degree = self.get_edges(*id).map_or(0, <[Edge]>::len);
                    degree.saturating_mul(degree)
                })
                .sum::<usize>() as f64;
            let global_fanout = if global_first > 0.0 {
                global_second / global_first
            } else {
                1.0
            };
            CrossModalStats {
                degree_skew: (sample_mean / graph.avg_out_degree.max(f64::EPSILON)).max(0.0),
                fanout_skew: (sample_fanout / global_fanout.max(f64::EPSILON)).max(0.0),
                vector_density_skew: vector_density.map(|(skew, _)| skew),
                sampled: vector_density.map_or(sampled, |(_, count)| count.min(sampled)),
                generation: self.generation,
            }
        };
        self.cross_modal_stats_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(cache_key, stats.clone());
        Some(stats)
    }

    pub fn index_memory_stats(&self) -> crate::observability::IndexMemoryStats {
        let property_bytes = self.property_indexes.estimated_memory_bytes();
        let property_mapped_bytes = self.property_indexes.mapped_bytes();
        let graph_mapped_bytes = self.mapped_graph_bytes();
        let quiver_bytes = self
            .quiver_index
            .as_ref()
            .map(|index| index.stats().hot_bytes)
            .unwrap_or_default();
        let hot_bytes = property_bytes.saturating_add(quiver_bytes);
        crate::observability::IndexMemoryStats {
            resident_heap_bytes: hot_bytes as u64,
            mapped_bytes: self
                .mmap_vector_bytes()
                .saturating_add(property_mapped_bytes)
                .saturating_add(graph_mapped_bytes) as u64,
            hot_bytes: hot_bytes as u64,
            persisted_bytes: self
                .mmap_vector_bytes()
                .saturating_add(property_mapped_bytes)
                .saturating_add(graph_mapped_bytes) as u64,
            posting_entries: self
                .property_index_stats()
                .iter()
                .map(|stats| stats.entry_count as u64)
                .sum::<u64>()
                .saturating_add(self.property_indexes.mapped_posting_entries() as u64),
        }
    }

    pub fn set_property_indexes(&mut self, indexes: PropertyIndexRegistry) {
        self.property_indexes = indexes;
    }

    fn add_to_property_index(&mut self, id: NodeId, payload: &serde_json::Value) {
        self.property_indexes.insert(id, payload);
    }

    fn remove_from_property_index(&mut self, id: NodeId, payload: &serde_json::Value) {
        self.property_indexes.remove(id, payload);
    }

    /// 删除节点：三层原子联删（向量标记为死区 + Payload移除 + 所有关联边清理）
    pub fn delete(&mut self, id: NodeId) -> Result<()> {
        if !self.payloads.contains_key(&id) {
            return Err(TriviumError::NodeNotFound(id));
        }

        // 1. 向量层：通过 VecPool 逻辑删除（置零），并回收物理卡槽
        if let Some(idx) = self.ids_to_indices.remove(&id) {
            self.vec_pool.zero_out(idx);
            self.indices_to_ids[idx] = 0; // 盖上墓碑标识，防止后续被误认
            self.free_slots.push(idx); // 抛入环保回收池，供下一个 insert 使用！
        }

        // 2. 属性索引清理（必须在 payload 移除之前）
        if let Some(payload) = self.get_payload(id).cloned() {
            self.remove_from_property_index(id, &payload);
        }

        // 3. 元数据层
        self.payloads.remove(&id);

        // 3. 图谱层：删除出边 + 清理其他节点指向该节点的入边
        //    同时收集需要从 label_index 中清理的标签集合，最后批量清理
        let mut dirty_labels: HashMap<String, Vec<(NodeId, NodeId)>> = HashMap::new();

        if let Some(outgoing_edges) = self.edges.remove(&id) {
            // 清理这些出边目标节点的入度计数与反向哈希网记录
            for edge in &outgoing_edges {
                let target = edge.target_id;
                if let Some(in_deg) = self.in_degrees.get_mut(&target) {
                    *in_deg = in_deg.saturating_sub(1);
                }
                if let Some(incoming) = self.incoming_edges.get_mut(&target) {
                    incoming.retain(|&src| src != id);
                }
                dirty_labels
                    .entry(edge.label.clone())
                    .or_default()
                    .push((id, target));
            }
        }

        // 神级优化：利用反向哈希网，只遍历指向本节点的死循环入口，彻底消除 O(E) 雪崩扫表！
        if let Some(incoming) = self.incoming_edges.remove(&id) {
            for src_id in incoming {
                if let Some(edge_list) = self.edges.get_mut(&src_id) {
                    for edge in edge_list.iter() {
                        if edge.target_id == id {
                            dirty_labels
                                .entry(edge.label.clone())
                                .or_default()
                                .push((src_id, id));
                        }
                    }
                    edge_list.retain(|e| e.target_id != id);
                }
            }
        }
        self.in_degrees.remove(&id);

        // 批量清理 label_index：每个标签只做一次 retain，避免 O(N²) 雪崩
        for (label, to_remove) in &dirty_labels {
            if let Some(pairs) = self.label_index.get_mut(label) {
                let remove_set: HashSet<&(NodeId, NodeId)> = to_remove.iter().collect();
                pairs.retain(|pair| !remove_set.contains(pair));
            }
        }

        self.bq_dirty = true;
        if !self.quiver_sync_paused {
            self.invalidate_quiver_for_delete(id);
        }
        self.mark_changed(true);

        Ok(())
    }

    /// 断开两个节点之间的指定标签边（幂等）。
    pub fn unlink_label(&mut self, src: NodeId, dst: NodeId, label: &str) -> Result<()> {
        let Some(edge_list) = self.edges.get_mut(&src) else {
            return self.validate_unlink(src);
        };
        let before = edge_list.len();
        edge_list.retain(|edge| !(edge.target_id == dst && edge.label == label));
        let removed = before - edge_list.len();
        if removed == 0 {
            return Ok(());
        }
        if let Some(in_deg) = self.in_degrees.get_mut(&dst) {
            *in_deg = in_deg.saturating_sub(removed);
        }
        if let Some(pairs) = self.label_index.get_mut(label) {
            pairs.retain(|&(source, target)| !(source == src && target == dst));
        }
        let still_connected = edge_list.iter().any(|edge| edge.target_id == dst);
        if !still_connected && let Some(incoming) = self.incoming_edges.get_mut(&dst) {
            incoming.retain(|&source| source != src);
        }
        self.mark_changed(false);
        Ok(())
    }

    /// 断开两个节点之间的所有标签边（幂等）。
    pub fn unlink(&mut self, src: NodeId, dst: NodeId) -> Result<()> {
        let Some(edge_list) = self.edges.get_mut(&src) else {
            // src 没有出边：只要节点本身存在，断开不存在的边应视为幂等无操作，
            // 而不是误报 NodeNotFound（edges 表只登记有出边的节点）。
            if self.payloads.contains_key(&src) {
                return Ok(());
            }
            return Err(TriviumError::NodeNotFound(src));
        };
        {
            let initial_len = edge_list.len();
            // 先清理 label_index 中对应的条目
            for edge in edge_list.iter() {
                if edge.target_id == dst
                    && let Some(pairs) = self.label_index.get_mut(&edge.label)
                {
                    pairs.retain(|&(s, d)| !(s == src && d == dst));
                }
            }
            edge_list.retain(|e| e.target_id != dst);
            if edge_list.len() < initial_len {
                let removed_count = initial_len - edge_list.len();
                if let Some(in_deg) = self.in_degrees.get_mut(&dst) {
                    *in_deg = in_deg.saturating_sub(removed_count);
                }
                if let Some(incoming) = self.incoming_edges.get_mut(&dst) {
                    incoming.retain(|&id| id != src);
                }
                self.mark_changed(false);
            }
            Ok(())
        }
    }

    pub fn get_all_ids(&self) -> Vec<NodeId> {
        self.payloads.keys().copied().collect()
    }

    /// 更新节点的元数据（Payload），不影响向量和图谱
    pub fn update_payload(&mut self, id: NodeId, payload: serde_json::Value) -> Result<()> {
        match self.get_payload(id).cloned() {
            Some(old_payload) => {
                let sig = calculate_json_signature(&payload);
                if let Some(&idx) = self.ids_to_indices.get(&id) {
                    self.fast_tags[idx] = sig;
                }
                // 属性索引同步更新
                self.property_indexes.update(id, &old_payload, &payload);
                self.mark_property_changed();
                self.payloads.insert(id, PayloadEntry::from_value(payload));
                self.mark_changed(false);
                Ok(())
            }
            None => Err(TriviumError::NodeNotFound(id)),
        }
    }

    /// 部分更新节点的元数据（Payload），支持 $set / $inc / $unset 操作
    ///
    /// 不同于 `update_payload` 的全量替换，`patch_payload` 只修改指定字段，
    /// 其他字段保持不变。
    ///
    /// # 操作类型
    /// - `$set`: 设置字段值（不存在则创建）
    /// - `$inc`: 数值字段递增（字段不存在视为 0）
    /// - `$unset`: 删除字段
    ///
    /// # 示例 patch JSON
    /// ```json
    /// {
    ///   "$set": {"name": "Alice", "status": "active"},
    ///   "$inc": {"visit_count": 1, "score": -0.5},
    ///   "$unset": {"deprecated_field": true}
    /// }
    /// ```
    pub fn patch_payload(&mut self, id: NodeId, patch: &serde_json::Value) -> Result<()> {
        let new_payload = self.preview_patch_payload(id, patch)?;
        self.update_payload(id, new_payload)
    }

    pub(crate) fn preview_patch_payload(
        &self,
        id: NodeId,
        patch: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let old_payload = self
            .get_payload(id)
            .cloned()
            .ok_or(TriviumError::NodeNotFound(id))?;

        let mut new_payload = old_payload.clone();

        // 确保 payload 是 Object 类型
        let obj = new_payload
            .as_object_mut()
            .ok_or_else(|| TriviumError::InvalidInput("Payload 不是 JSON 对象".into()))?;

        if let Some(patch_obj) = patch.as_object() {
            // $set: 设置字段值
            if let Some(set_val) = patch_obj.get("$set")
                && let Some(set_map) = set_val.as_object()
            {
                for (k, v) in set_map {
                    obj.insert(k.clone(), v.clone());
                }
            }

            // $inc: 数值递增
            if let Some(inc_val) = patch_obj.get("$inc")
                && let Some(inc_map) = inc_val.as_object()
            {
                for (k, v) in inc_map {
                    let delta = v.as_f64().unwrap_or(0.0);
                    let current = obj.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let new_val = current + delta;
                    // 如果结果是整数，保持整数类型
                    if new_val.fract() == 0.0
                        && new_val >= i64::MIN as f64
                        && new_val <= i64::MAX as f64
                    {
                        obj.insert(
                            k.clone(),
                            serde_json::Value::Number(serde_json::Number::from(new_val as i64)),
                        );
                    } else {
                        obj.insert(k.clone(), serde_json::json!(new_val));
                    }
                }
            }

            // $unset: 删除字段
            if let Some(unset_val) = patch_obj.get("$unset")
                && let Some(unset_map) = unset_val.as_object()
            {
                for k in unset_map.keys() {
                    obj.remove(k);
                }
            }

            if !patch_obj.contains_key("$set")
                && !patch_obj.contains_key("$inc")
                && !patch_obj.contains_key("$unset")
            {
                return Err(TriviumError::ApiMigrationRequired {
                    removed_api: "patch_payload(普通对象简写)",
                    replacement: "patch_payload({\"$set\": {...}}) 或 update_payload()",
                });
            }
        } else {
            return Err(TriviumError::InvalidInput(
                "patch_payload 的 patch 参数必须是 JSON 对象".into(),
            ));
        }

        Ok(new_payload)
    }

    /// 就地替换节点的向量（维度必须一致）
    pub fn update_vector(&mut self, id: NodeId, vector: &[T]) -> Result<()> {
        self.validate_update_vector(id, vector)?;
        match self.ids_to_indices.get(&id) {
            Some(&idx) => {
                self.vec_pool.update(idx, vector);
                self.bq_dirty = true; // 向量变了，BQ 签名需要重建
                // QuIVer 增量更新：soft_delete 旧向量 + incremental_insert 新向量
                if !self.quiver_sync_paused
                    && let Some(ref mut quiver) = self.quiver_index
                {
                    quiver.soft_delete(id);
                    let vec_f32: Vec<f32> = vector.iter().map(|v| v.to_f32()).collect();
                    let mut lcg = id.wrapping_mul(0x9E3779B97F4A7C15);
                    quiver.insert(&vec_f32, id, idx, &mut lcg);
                    if quiver.needs_rebuild() {
                        self.quiver_index = None;
                        tracing::debug!(
                            "QuIVer 索引增量变更超过 25%，已丢弃，下次搜索前将自动重建 (QuIVer index exceeded 25% dirty changes and was discarded; it will rebuild before the next search)"
                        );
                    }
                }
                self.mark_changed(true);
                Ok(())
            }
            None => Err(TriviumError::NodeNotFound(id)),
        }
    }

    /// 按 ID 获取节点的原生向量（返回切片引用）
    pub fn get_vector(&self, id: NodeId) -> Option<&[T]> {
        self.ids_to_indices
            .get(&id)
            .and_then(|&idx| self.vec_pool.get(idx))
    }

    /// 当前活跃节点数量
    pub fn node_count(&self) -> usize {
        self.payloads.len()
    }

    /// 内部槽位总数（含 tombstone 空洞），用于 BQ 签名遍历
    #[inline]
    pub fn internal_slot_count(&self) -> usize {
        self.indices_to_ids.len()
    }

    pub(crate) fn active_vector_at_slot(&self, slot: usize) -> Option<(NodeId, &[T])> {
        let id = *self.indices_to_ids.get(slot)?;
        if id == 0 || !self.payloads.contains_key(&id) {
            return None;
        }
        self.vec_pool.get(slot).map(|vector| (id, vector))
    }

    /// 返回活跃节点对应的内部槽位。只读借用期间槽位映射保持稳定。
    #[inline]
    pub(crate) fn internal_slot_of(&self, id: NodeId) -> Option<usize> {
        self.ids_to_indices.get(&id).copied()
    }

    /// 返回内部槽位上的活跃节点 ID，tombstone 返回 None。
    #[inline]
    pub(crate) fn active_id_at_slot(&self, slot: usize) -> Option<NodeId> {
        let id = *self.indices_to_ids.get(slot)?;
        (id != 0 && self.payloads.contains_key(&id)).then_some(id)
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
        let payload_bytes: usize = self.payloads.values().map(PayloadEntry::memory_bytes).sum();
        let edge_bytes: usize = self
            .edges
            .values()
            .map(|es| es.len() * std::mem::size_of::<Edge>())
            .sum();
        let index_bytes = self.indices_to_ids.len() * std::mem::size_of::<NodeId>()
            + self.ids_to_indices.len()
                * (std::mem::size_of::<NodeId>() + std::mem::size_of::<usize>());
        let label_index_bytes: usize = self
            .label_index
            .values()
            .map(|pairs| pairs.len() * std::mem::size_of::<(NodeId, NodeId)>())
            .sum();
        let quiver_bytes = self
            .quiver_index
            .as_ref()
            .map(|index| index.stats().hot_bytes)
            .unwrap_or_default();
        let text_bytes = self.text_index.estimated_memory_bytes();
        let property_index_bytes = self.property_indexes.estimated_memory_bytes();
        vec_bytes
            + payload_bytes
            + edge_bytes
            + index_bytes
            + label_index_bytes
            + quiver_bytes
            + text_bytes
            + property_index_bytes
    }

    pub(crate) fn advise_cold_vectors_dontneed(&self) {
        self.vec_pool.advise_dontneed();
    }

    pub(crate) fn mmap_vector_bytes(&self) -> usize {
        self.vec_pool.mmap_bytes()
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

    pub(crate) fn set_text_index(&mut self, text_index: TextIndex) {
        self.text_index = text_index;
    }

    pub(crate) fn text_index(&self) -> &TextIndex {
        &self.text_index
    }

    pub fn text_engine(&self) -> &TextIndex {
        &self.text_index
    }

    /// 从已有的 payload 自动重建 TextIndex（供重启加载后调用）
    ///
    /// 遍历所有活跃节点的 payload JSON，将字符串字段值注入 BM25 倒排索引。
    /// 这使得文本混合检索在重启后自动恢复，无需额外持久化文件。
    pub fn rebuild_text_index_from_payloads(&mut self) {
        self.text_index.clear();
        for (&id, payload) in &self.payloads {
            if let serde_json::Value::Object(map) = payload.get() {
                for (_key, value) in map {
                    if let serde_json::Value::String(text) = value
                        && !text.is_empty()
                    {
                        self.text_index.add_text(id, text);
                    }
                }
            }
        }
        self.text_index.build();
        if !self.payloads.is_empty() {
            tracing::info!(
                "TextIndex 从 {} 个节点的 payload 自动重建完成 (TextIndex auto-rebuilt from {} node payloads)",
                self.payloads.len(),
                self.payloads.len()
            );
        }
    }
}
