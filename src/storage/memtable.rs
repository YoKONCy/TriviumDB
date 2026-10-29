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
use crate::observability::PayloadMemoryStats;
use crate::storage::payload_store::PayloadStore;
use crate::storage::vec_pool::VecPool;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const AUTO_QUIVER_TARGET_COMPONENTS: usize = 666_667;
const AUTO_QUIVER_MIN_NODES: usize = 209;
const AUTO_QUIVER_MAX_NODES: usize = 834;

fn increment_json_number(
    current: &serde_json::Value,
    delta: &serde_json::Value,
) -> Result<serde_json::Value> {
    let invalid = || {
        TriviumError::InvalidInput(
            "$inc 要求数值且结果可精确表示 (Increment requires numbers and a representable result)"
                .into(),
        )
    };
    let integer = |value: &serde_json::Value| {
        value
            .as_i64()
            .map(i128::from)
            .or_else(|| value.as_u64().map(i128::from))
    };
    if let (Some(a), Some(b)) = (integer(current), integer(delta)) {
        let sum = a + b;
        return if sum < 0 {
            i64::try_from(sum)
                .map(serde_json::Value::from)
                .map_err(|_| invalid())
        } else {
            u64::try_from(sum)
                .map(serde_json::Value::from)
                .map_err(|_| invalid())
        };
    }
    let exact_float = |value: &serde_json::Value| -> Result<f64> {
        let f = value.as_f64().ok_or_else(invalid)?;
        if let Some(i) = integer(value)
            && f as i128 != i
        {
            return Err(invalid());
        }
        Ok(f)
    };
    let a = exact_float(current)?;
    let b = exact_float(delta)?;
    if a.fract() == 0.0
        && b.fract() == 0.0
        && a >= i64::MIN as f64
        && a < u64::MAX as f64
        && b >= i64::MIN as f64
        && b < u64::MAX as f64
    {
        let sum = a as i128 + b as i128;
        return if sum < 0 {
            i64::try_from(sum)
                .map(serde_json::Value::from)
                .map_err(|_| invalid())
        } else {
            u64::try_from(sum)
                .map(serde_json::Value::from)
                .map_err(|_| invalid())
        };
    }
    let sum = a + b;
    if (b != 0.0 && sum == a) || (a != 0.0 && sum == b) {
        return Err(invalid());
    }
    serde_json::Number::from_f64(sum)
        .map(serde_json::Value::Number)
        .ok_or_else(invalid)
}

/// 根据暴力检索的近似向量分量工作量计算自动 QuIVer 节点阈值。
pub fn auto_quiver_node_threshold(dim: usize) -> usize {
    AUTO_QUIVER_TARGET_COMPONENTS
        .div_ceil(dim.max(1))
        .clamp(AUTO_QUIVER_MIN_NODES, AUTO_QUIVER_MAX_NODES)
}

fn edge_dynamic_bytes(edge: &Edge) -> usize {
    edge.label.capacity()
        + crate::storage::payload_store::estimate_json_memory(&edge.metadata)
            .saturating_sub(std::mem::size_of::<serde_json::Value>())
}

fn hash_growth_bytes<K, V>(len: usize, capacity: usize, additional: usize) -> usize {
    if additional <= capacity.saturating_sub(len) {
        0
    } else {
        len.saturating_add(additional)
            .saturating_mul(4)
            .saturating_mul(std::mem::size_of::<(K, V)>().saturating_add(1))
            .saturating_add(64)
    }
}

#[cfg(test)]
mod graph_heap_tests {
    use super::*;
    use serde_json::Value;

    fn assert_allocations(mt: &MemTable<f32>) {
        // 此样本的 Metadata 仅含字符串或 null，直接读取容器容量作为独立分配 oracle。
        let outgoing = mt
            .edges
            .values()
            .map(|edges| {
                edges.capacity() * std::mem::size_of::<Edge>()
                    + edges
                        .iter()
                        .map(|edge| {
                            edge.label.capacity()
                                + match &edge.metadata {
                                    Value::String(text) => text.capacity(),
                                    Value::Null => 0,
                                    _ => panic!("样本包含未声明的 Metadata 类型"),
                                }
                        })
                        .sum::<usize>()
            })
            .sum::<usize>();
        assert_eq!(mt.edge_heap_bytes, outgoing);
        assert_eq!(
            mt.label_heap_bytes,
            mt.label_index
                .iter()
                .map(|(key, pairs)| key.capacity() + pairs.capacity() * 16)
                .sum::<usize>()
        );
        assert_eq!(
            mt.incoming_heap_bytes,
            mt.incoming_edges
                .values()
                .map(|sources| sources.capacity() * 8)
                .sum::<usize>()
        );
        let stats = mt.graph_stats_state.read().unwrap();
        assert_eq!(
            stats.label_heap_bytes,
            stats
                .labels
                .iter()
                .map(|(label, stats)| label.capacity()
                    + (stats.sources.capacity() + stats.targets.capacity())
                        * std::mem::size_of::<(NodeId, usize)>())
                .sum::<usize>()
        );
        assert!(mt.validate_graph().valid);
    }

    #[test]
    fn 图堆账本覆盖容量增长替换收缩与删除修复() {
        let mut mt = MemTable::<f32>::new(2);
        for id in 1..=8 {
            mt.insert_with_id(id, &[1., 0.], Value::Null).unwrap();
        }
        let mut expected = BTreeMap::new();
        for i in 0..160 {
            let src = i % 8 + 1;
            let dst = (i / 8) % 8 + 1;
            let label = format!("label_{}", "中".repeat((i % 3 + 1) as usize));
            let text = "m".repeat((i * 17) as usize);
            mt.upsert_edge(
                src,
                dst,
                label.clone(),
                i as f32,
                Value::String(text.clone()),
            )
            .unwrap();
            expected.insert((src, dst, label), (i as f32, text));
            assert_allocations(&mt);
        }
        for (&(src, dst, ref label), &(weight, ref text)) in &expected {
            let edge = mt.get_edge(src, dst, label).unwrap();
            assert_eq!(edge.weight, weight);
            assert_eq!(edge.metadata, Value::String(text.clone()));
        }
        mt.upsert_edge(1, 1, "label_中".into(), 0., Value::Null)
            .unwrap();
        assert_allocations(&mt);
        mt.unlink_label(1, 1, "label_中").unwrap();
        assert_allocations(&mt);
        mt.unlink(2, 3).unwrap();
        assert_allocations(&mt);
        mt.delete(4).unwrap();
        assert_allocations(&mt);
        mt.repair_graph_indexes();
        assert_allocations(&mt);
        for src in [1, 2, 3, 5, 6, 7, 8] {
            for dst in [1, 2, 3, 5, 6, 7, 8] {
                mt.unlink(src, dst).unwrap();
                assert_allocations(&mt);
            }
        }
        assert_eq!(mt.graph_stats().edge_count, 0);
        assert_eq!(
            mt.edge_heap_bytes,
            mt.edges
                .values()
                .map(|edges| edges.capacity() * std::mem::size_of::<Edge>())
                .sum::<usize>()
        );
        assert_eq!(mt.label_heap_bytes, 0);
    }
}

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

#[derive(Debug, Clone, Default)]
struct MutableLabelStats {
    edge_count: usize,
    sources: HashMap<NodeId, usize>,
    targets: HashMap<NodeId, usize>,
}

#[derive(Debug, Clone, Default)]
struct GraphStatsState {
    node_count: usize,
    edge_count: usize,
    isolated_node_count: usize,
    out_degrees: HashMap<NodeId, usize>,
    in_degrees: HashMap<NodeId, usize>,
    out_degree_frequency: BTreeMap<usize, usize>,
    in_degree_frequency: BTreeMap<usize, usize>,
    labels: BTreeMap<String, MutableLabelStats>,
    sum_out_degree_squared: u128,
    label_heap_bytes: usize,
}

impl GraphStatsState {
    fn label_bytes(&self, label: &str) -> usize {
        self.labels.get_key_value(label).map_or(0, |(key, stats)| {
            key.capacity()
                + (stats.sources.capacity() + stats.targets.capacity())
                    * std::mem::size_of::<(NodeId, usize)>()
        })
    }

    fn heap_memory_bytes(&self) -> usize {
        self.label_heap_bytes
            + (self.out_degrees.capacity() + self.in_degrees.capacity())
                * std::mem::size_of::<(NodeId, usize)>()
            + self.labels.len() * (std::mem::size_of::<(String, MutableLabelStats)>() + 32)
            + (self.out_degree_frequency.len() + self.in_degree_frequency.len())
                * (std::mem::size_of::<(usize, usize)>() + 32)
    }
    fn adjust_frequency(frequency: &mut BTreeMap<usize, usize>, old: usize, new: usize) {
        if let Some(count) = frequency.get_mut(&old) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                frequency.remove(&old);
            }
        }
        *frequency.entry(new).or_default() += 1;
    }

    fn node_added(&mut self, id: NodeId) {
        if self.out_degrees.contains_key(&id) {
            return;
        }
        self.node_count += 1;
        self.isolated_node_count += 1;
        self.out_degrees.insert(id, 0);
        self.in_degrees.insert(id, 0);
        *self.out_degree_frequency.entry(0).or_default() += 1;
        *self.in_degree_frequency.entry(0).or_default() += 1;
    }

    fn edge_added(&mut self, src: NodeId, dst: NodeId, label: &str) {
        let before = self.label_bytes(label);
        let old_out = self.out_degrees.get(&src).copied().unwrap_or(0);
        let old_in = self.in_degrees.get(&dst).copied().unwrap_or(0);
        let src_was_isolated = old_out == 0 && self.in_degrees.get(&src).copied().unwrap_or(0) == 0;
        let dst_was_isolated =
            src != dst && old_in == 0 && self.out_degrees.get(&dst).copied().unwrap_or(0) == 0;
        Self::adjust_frequency(&mut self.out_degree_frequency, old_out, old_out + 1);
        Self::adjust_frequency(&mut self.in_degree_frequency, old_in, old_in + 1);
        self.out_degrees.insert(src, old_out + 1);
        self.in_degrees.insert(dst, old_in + 1);
        self.edge_count += 1;
        self.sum_out_degree_squared = self
            .sum_out_degree_squared
            .saturating_add((2usize.saturating_mul(old_out).saturating_add(1)) as u128);
        self.isolated_node_count = self
            .isolated_node_count
            .saturating_sub(usize::from(src_was_isolated) + usize::from(dst_was_isolated));
        let stats = self.labels.entry(label.to_owned()).or_default();
        stats.edge_count += 1;
        *stats.sources.entry(src).or_default() += 1;
        *stats.targets.entry(dst).or_default() += 1;
        self.label_heap_bytes = self
            .label_heap_bytes
            .saturating_sub(before)
            .saturating_add(self.label_bytes(label));
    }

    fn edge_removed(&mut self, src: NodeId, dst: NodeId, label: &str) {
        let before = self.label_bytes(label);
        let old_out = self.out_degrees.get(&src).copied().unwrap_or(0);
        let old_in = self.in_degrees.get(&dst).copied().unwrap_or(0);
        if old_out == 0 || old_in == 0 {
            return;
        }
        Self::adjust_frequency(&mut self.out_degree_frequency, old_out, old_out - 1);
        Self::adjust_frequency(&mut self.in_degree_frequency, old_in, old_in - 1);
        self.out_degrees.insert(src, old_out - 1);
        self.in_degrees.insert(dst, old_in - 1);
        self.edge_count = self.edge_count.saturating_sub(1);
        self.sum_out_degree_squared = self
            .sum_out_degree_squared
            .saturating_sub((2usize.saturating_mul(old_out).saturating_sub(1)) as u128);
        let src_is_isolated = old_out == 1 && self.in_degrees.get(&src).copied().unwrap_or(0) == 0;
        let dst_is_isolated =
            src != dst && old_in == 1 && self.out_degrees.get(&dst).copied().unwrap_or(0) == 0;
        self.isolated_node_count += usize::from(src_is_isolated) + usize::from(dst_is_isolated);
        if let Some(stats) = self.labels.get_mut(label) {
            stats.edge_count = stats.edge_count.saturating_sub(1);
            for (map, id) in [(&mut stats.sources, src), (&mut stats.targets, dst)] {
                if let Some(count) = map.get_mut(&id) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        map.remove(&id);
                    }
                }
            }
            if stats.edge_count == 0 {
                self.labels.remove(label);
            }
        }
        self.label_heap_bytes = self
            .label_heap_bytes
            .saturating_sub(before)
            .saturating_add(self.label_bytes(label));
    }

    fn node_removed(&mut self, id: NodeId) {
        let out = self.out_degrees.remove(&id).unwrap_or(0);
        let incoming = self.in_degrees.remove(&id).unwrap_or(0);
        if out == 0 && incoming == 0 {
            self.isolated_node_count = self.isolated_node_count.saturating_sub(1);
        }
        for (frequency, degree) in [
            (&mut self.out_degree_frequency, out),
            (&mut self.in_degree_frequency, incoming),
        ] {
            if let Some(count) = frequency.get_mut(&degree) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    frequency.remove(&degree);
                }
            }
        }
        self.node_count = self.node_count.saturating_sub(1);
    }

    fn histogram(frequency: &BTreeMap<usize, usize>) -> Vec<DegreeBucket> {
        const UPPER_BOUNDS: [usize; 8] = [0, 1, 2, 4, 8, 16, 64, usize::MAX];
        let mut counts = [0usize; UPPER_BOUNDS.len()];
        for (degree, count) in frequency {
            let index = UPPER_BOUNDS
                .iter()
                .position(|upper| degree <= upper)
                .unwrap_or(UPPER_BOUNDS.len() - 1);
            counts[index] = counts[index].saturating_add(*count);
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

    fn snapshot(&self) -> GraphStats {
        GraphStats {
            node_count: self.node_count,
            edge_count: self.edge_count,
            isolated_node_count: self.isolated_node_count,
            label_count: self.labels.len(),
            avg_out_degree: self.edge_count as f64 / self.node_count.max(1) as f64,
            avg_in_degree: self.edge_count as f64 / self.node_count.max(1) as f64,
            max_out_degree: self
                .out_degree_frequency
                .keys()
                .next_back()
                .copied()
                .unwrap_or(0),
            max_in_degree: self
                .in_degree_frequency
                .keys()
                .next_back()
                .copied()
                .unwrap_or(0),
            label_stats: self
                .labels
                .iter()
                .map(|(label, stats)| {
                    (
                        label.clone(),
                        LabelStats {
                            edge_count: stats.edge_count,
                            distinct_source_count: stats.sources.len(),
                            distinct_target_count: stats.targets.len(),
                        },
                    )
                })
                .collect(),
            out_degree_histogram: Self::histogram(&self.out_degree_frequency),
            in_degree_histogram: Self::histogram(&self.in_degree_frequency),
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
    pub sm: crate::index::quiver::corrected_sm::CorrectedSmIndex,
}

type CrossModalCacheKey = (String, Vec<u8>);
type CrossModalCacheEntry = (u64, u64, u64, CrossModalStats);

enum PendingTextMutation {
    Keyword(NodeId, String),
    Text(NodeId, String),
    Delete(NodeId),
}

/// 数据库的权威内存工作区，向量池与派生索引共享节点生命周期。
pub struct MemTable<T: VectorType> {
    dim: usize,
    next_id: NodeId,
    generation: u64,
    /// 最近一次完整检查点之后是否发生了需要持久化的变更。
    checkpoint_dirty: bool,
    recovery_required: Arc<AtomicBool>,
    load_memory_limit: usize,
    vector_generation: u64,
    property_generation: u64,
    graph_generation: u64,

    // --- 三位一体的核心存储 ---

    // 1. 向量池（分层 mmap）：
    // 委托给 VecPool，底层为 mmap 基础层 + Vec 增量层
    // 基础层由 OS PageCache 按需加载，启动零拷贝
    vec_pool: VecPool<T>,

    // 旧 Rust 整库签名视图仅在调用方显式请求时生成；正式查询和持久化不分配它。
    bq_signatures: std::sync::OnceLock<Vec<BqSignature>>,

    // 附设文本倒排引擎 (完全可选，纯碎占用独立内存不干扰底座)
    text_index: TextIndex,
    resource_runtime: crate::resources::ResourceRuntime,
    resource_reservation: Option<crate::resources::DatabaseReservation>,
    // 未加载的 sidecar 与待合并操作分开保存，避免把“未加载”解释为“空索引”。
    deferred_text_path: Option<String>,
    deferred_text_meta: Option<[u8; 28]>,
    pending_text_mutations: Vec<PendingTextMutation>,
    pending_text_bytes: usize,
    text_memory_limit: usize,

    // 2. 元数据映射（文档型负载）—— 原始 JSON 紧凑存储，按访问惰性解析
    payloads: PayloadStore,

    // 3. 图谱邻接表 —— RW 使用内存 delta，ReadOnly/Immutable 可使用 mmap 基础块
    edges: HashMap<NodeId, Vec<Edge>>,
    edge_heap_bytes: usize,
    label_heap_bytes: usize,
    incoming_heap_bytes: usize,
    mapped_graph: Option<crate::storage::graph_blocks::MappedGraphStore>,

    // 入度统计表：用于快速查询目标节点的被连接数（支持图谱反向抑制算法）
    in_degrees: HashMap<NodeId, usize>,

    // 反向入度哈希网：用于 O(1) 解决删除节点时的全库雪崩扫表
    incoming_edges: HashMap<NodeId, Vec<NodeId>>,

    // 边标签倒排索引：label → [(src, dst)]，加速图谱按标签查询
    label_index: HashMap<String, Vec<(NodeId, NodeId)>>,

    // 图统计随节点和边写入增量维护，查询只生成轻量快照。
    graph_stats_state: std::sync::RwLock<GraphStatsState>,
    // 复合字段统计仅随 Payload 或索引定义变化失效。
    column_pair_stats_cache:
        std::sync::RwLock<Option<(u64, Vec<crate::index::property::ColumnPairStats>)>>,
    // 跨模统计按“字段 + 类型稳定 JSON 值”缓存；任意写操作后随 generation 失效。
    cross_modal_stats_cache: std::sync::RwLock<BTreeMap<CrossModalCacheKey, CrossModalCacheEntry>>,

    // 属性二级索引 Registry：字段名 → 类型稳定 Hash 索引
    property_indexes: PropertyIndexRegistry,

    // 节点不应期（疲劳状态）映射表：
    // 0 = 正常；1 = 疲劳中（被激活后，下一轮扩散大幅衰减，消费一次后清零）
    fatigue_map: std::sync::RwLock<HashMap<NodeId, u8>>,

    // 活跃 NodeId 的压缩有序集合：FullNodeScan 可惰性遍历并按 LIMIT 早停。
    active_ids: roaring::RoaringTreemap,
    // 仅测试构建记录全库物化，生产构建没有原子计数开销。
    #[cfg(any(test, feature = "test-hooks"))]
    id_materializations: std::sync::atomic::AtomicU64,

    // 映射表：内部索引 (0, 1, 2...) 到 NodeId
    // 用于在 vectors 数组里定位数据位置
    indices_to_ids: Vec<NodeId>,
    ids_to_indices: HashMap<NodeId, usize>,

    // 行级哈希阵列：与 indices 同步，提供 O(1) 的布隆屏蔽检查，跳过极其昂贵的 JSON 反序列化
    fast_tags: Vec<u64>,

    // 空闲索引回收槽：O(1) 回收墓碑位置，防止物理大数组无尽膨胀
    free_slots: Vec<usize>,

    // 4. QuIVer BQ-native Vamana 图索引，达到维度相关阈值后按需构建。
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
            checkpoint_dirty: true,
            recovery_required: Arc::new(AtomicBool::new(false)),
            load_memory_limit: 0,
            vector_generation: 0,
            property_generation: 0,
            graph_generation: 0,
            vec_pool: VecPool::new(dim),
            bq_signatures: std::sync::OnceLock::new(),
            text_index: TextIndex::new(),
            resource_runtime: crate::resources::ResourceRuntime::shared_default(),
            resource_reservation: None,
            deferred_text_path: None,
            deferred_text_meta: None,
            pending_text_mutations: Vec::new(),
            pending_text_bytes: 0,
            text_memory_limit: 0,
            payloads: PayloadStore::default(),
            edges: HashMap::new(),
            edge_heap_bytes: 0,
            label_heap_bytes: 0,
            incoming_heap_bytes: 0,
            mapped_graph: None,
            in_degrees: HashMap::new(),
            incoming_edges: HashMap::new(),
            label_index: HashMap::new(),
            graph_stats_state: std::sync::RwLock::new(GraphStatsState::default()),
            column_pair_stats_cache: std::sync::RwLock::new(None),
            cross_modal_stats_cache: std::sync::RwLock::new(BTreeMap::new()),
            property_indexes: PropertyIndexRegistry::default(),
            fatigue_map: std::sync::RwLock::new(HashMap::new()),
            active_ids: roaring::RoaringTreemap::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            id_materializations: std::sync::atomic::AtomicU64::new(0),
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
            checkpoint_dirty: true,
            recovery_required: Arc::new(AtomicBool::new(false)),
            load_memory_limit: 0,
            vector_generation: 0,
            property_generation: 0,
            graph_generation: 0,
            vec_pool,
            bq_signatures: std::sync::OnceLock::new(),
            text_index: TextIndex::new(),
            resource_runtime: crate::resources::ResourceRuntime::shared_default(),
            resource_reservation: None,
            deferred_text_path: None,
            deferred_text_meta: None,
            pending_text_mutations: Vec::new(),
            pending_text_bytes: 0,
            text_memory_limit: 0,
            payloads: PayloadStore::default(),
            edges: HashMap::new(),
            edge_heap_bytes: 0,
            label_heap_bytes: 0,
            incoming_heap_bytes: 0,
            mapped_graph: None,
            in_degrees: HashMap::new(),
            incoming_edges: HashMap::new(),
            label_index: HashMap::new(),
            graph_stats_state: std::sync::RwLock::new(GraphStatsState::default()),
            column_pair_stats_cache: std::sync::RwLock::new(None),
            cross_modal_stats_cache: std::sync::RwLock::new(BTreeMap::new()),
            property_indexes: PropertyIndexRegistry::default(),
            fatigue_map: std::sync::RwLock::new(HashMap::new()),
            active_ids: roaring::RoaringTreemap::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            id_materializations: std::sync::atomic::AtomicU64::new(0),
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
    pub fn graph_generation(&self) -> u64 {
        self.graph_generation
    }

    #[inline]
    fn mark_property_changed(&mut self) {
        self.checkpoint_dirty = true;
        self.property_generation = self.property_generation.wrapping_add(1);
        *self
            .column_pair_stats_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.cross_modal_stats_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    #[inline]
    fn mark_graph_changed(&mut self) {
        self.checkpoint_dirty = true;
        self.graph_generation = self.graph_generation.wrapping_add(1);
        self.cross_modal_stats_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    #[inline]
    fn mark_changed(&mut self, vectors_changed: bool) {
        self.checkpoint_dirty = true;
        self.generation = self.generation.wrapping_add(1);
        if vectors_changed {
            self.bq_signatures.take();
            self.vector_generation = self.vector_generation.wrapping_add(1);
            self.cross_modal_stats_cache
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
        }
    }

    fn rebuild_graph_stats_state(&mut self) {
        let mut state = GraphStatsState::default();
        for id in self.payloads.keys().copied() {
            state.node_added(id);
        }
        for src in self.payloads.keys().copied() {
            for edge in self.get_edges(src).unwrap_or_default() {
                state.edge_added(src, edge.target_id, &edge.label);
            }
        }
        *self
            .graph_stats_state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = state;
        self.mark_graph_changed();
    }

    /// 将 next_id 推进到至少 candidate 值（WAL 回放时防止 ID 复用）
    #[inline]
    pub fn advance_next_id(&mut self, candidate: NodeId) {
        if candidate > self.next_id {
            self.checkpoint_dirty = true;
            self.next_id = candidate;
        }
    }

    /// 估算为额外节点预留核心容器容量所需的新增堆内存。
    pub(crate) fn estimate_reserve_bytes(&self, additional: usize) -> Result<usize> {
        if additional == 0 {
            return Ok(0);
        }
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
                .checked_mul(std::mem::size_of::<(NodeId, usize)>() + 32)
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
        let stats = self
            .graph_stats_state
            .read()
            .unwrap_or_else(|p| p.into_inner());
        let graph_bytes = hash_growth_bytes::<NodeId, usize>(
            stats.out_degrees.len(),
            stats.out_degrees.capacity(),
            additional,
        )
        .saturating_add(hash_growth_bytes::<NodeId, usize>(
            stats.in_degrees.len(),
            stats.in_degrees.capacity(),
            additional,
        ))
        .saturating_add(128);
        checked(
            vector_bytes
                .checked_add(graph_bytes)
                .and_then(|n| n.checked_add(payload_bytes))
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
        self.payloads.reserve(additional)?;
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
        self.checkpoint_dirty = true;
        self.bq_signatures.take();
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
        if id == 0 || id == NodeId::MAX {
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
        self.payloads.insert_value(id, payload)?;
        self.active_ids.insert(id);
        self.ids_to_indices.insert(id, idx);
        self.mark_property_changed();
        self.graph_stats_state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .node_added(id);
        self.mark_graph_changed();
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
        if id == 0 || id == NodeId::MAX || self.ids_to_indices.contains_key(&id) {
            return Err(TriviumError::CorruptedFile(
                "节点映射包含非法或重复 ID (Node mapping contains an invalid or duplicate ID)"
                    .into(),
            ));
        }
        // 包含 JSON 校验临时 DOM、容器倍增和新记录原始字节。
        self.check_load_allocation(payload_raw.len().saturating_mul(64).saturating_add(1024))?;
        let idx = self.indices_to_ids.len();
        self.payloads.insert_raw(id, payload_raw)?;
        self.active_ids.insert(id);
        self.indices_to_ids.push(id);
        // 冷 Payload 尚未解析时使用全 1，布隆预过滤选择保守放行，避免假阴性。
        self.fast_tags.push(u64::MAX);
        self.ids_to_indices.insert(id, idx);
        self.graph_stats_state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .node_added(id);
        Ok(())
    }

    /// 从分离 Payload 格式加载时，仅恢复活动 slot；raw JSON 随后由已校验的 sidecar 安装。
    pub(crate) fn register_payload_slot(&mut self, id: NodeId) -> Result<()> {
        self.check_load_allocation(1024)?;
        if id == 0 || id == NodeId::MAX || self.ids_to_indices.contains_key(&id) {
            return Err(TriviumError::CorruptedFile(
                "Payload slot 包含非法或重复 NodeId".into(),
            ));
        }
        let idx = self.indices_to_ids.len();
        // 边和索引恢复发生在 sidecar 安装前；使用最小合法 JSON 占位，随后整体替换为 mmap raw。
        self.payloads.insert_raw(id, b"null")?;
        self.active_ids.insert(id);
        self.indices_to_ids.push(id);
        self.fast_tags.push(u64::MAX);
        self.ids_to_indices.insert(id, idx);
        self.graph_stats_state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .node_added(id);
        Ok(())
    }

    /// 从持久化文件加载时遇到逻辑删除节点（Tombstone），仅推进内部索引映射空洞
    pub fn register_tombstone(&mut self) -> Result<()> {
        self.check_load_allocation(64)?;
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
        crate::storage::payload_store::json_encoded_len(&payload)?;

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
        self.payloads.insert_value(id, payload)?;
        self.active_ids.insert(id);

        // 3. 构建反向映射
        self.ids_to_indices.insert(id, idx);
        self.mark_property_changed();
        self.graph_stats_state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .node_added(id);
        self.mark_graph_changed();

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
        crate::storage::payload_store::json_encoded_len(&payload)?;
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
        self.payloads.insert_value(id, payload)?;
        self.active_ids.insert(id);
        self.ids_to_indices.insert(id, idx);
        self.mark_property_changed();
        self.graph_stats_state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .node_added(id);
        self.mark_graph_changed();

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
        if id == NodeId::MAX {
            return Err(TriviumError::InvalidInput(
                "节点 ID 空间已耗尽 (Node ID space exhausted)".into(),
            ));
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

    pub(crate) fn validate_edge_value(label: &str, metadata: &serde_json::Value) -> Result<()> {
        if label.len() > u16::MAX as usize {
            return Err(TriviumError::InvalidInput(
                "边标签 UTF-8 长度不能超过 65535 字节 (Edge label must fit in 65535 UTF-8 bytes)"
                    .into(),
            ));
        }
        if crate::storage::payload_store::json_encoded_len(metadata)? > u32::MAX as usize {
            return Err(TriviumError::InvalidInput(
                "单条边元数据不能超过 4 GiB (Edge metadata must fit in 4 GiB)".into(),
            ));
        }
        Ok(())
    }

    /// 覆盖持久标签副本、WAL 暂存、容器增长和派生图统计的保守峰值。
    pub(crate) fn estimate_edge_write_bytes(
        &self,
        src: NodeId,
        dst: NodeId,
        label: &str,
        metadata: &serde_json::Value,
    ) -> Result<usize> {
        Self::validate_edge_value(label, metadata)?;
        let outgoing = self.edges.get(&src);
        let existing = outgoing
            .is_some_and(|edges| edges.iter().any(|e| e.target_id == dst && e.label == label));
        let mut bytes = label
            .len()
            .saturating_mul(if existing { 2 } else { 5 })
            .saturating_add(256);
        if existing {
            return Ok(bytes);
        }
        let vec_growth = |len: usize, cap: usize, size: usize| {
            if len < cap {
                0
            } else {
                cap.saturating_mul(2).max(4).saturating_mul(size)
            }
        };
        bytes = bytes.saturating_add(vec_growth(
            outgoing.map_or(0, Vec::len),
            outgoing.map_or(0, Vec::capacity),
            std::mem::size_of::<Edge>(),
        ));
        let pairs = self.label_index.get(label);
        bytes = bytes.saturating_add(vec_growth(
            pairs.map_or(0, Vec::len),
            pairs.map_or(0, Vec::capacity),
            std::mem::size_of::<(NodeId, NodeId)>(),
        ));
        let incoming = self.incoming_edges.get(&dst);
        if incoming.is_none_or(|sources| !sources.contains(&src)) {
            bytes = bytes.saturating_add(vec_growth(
                incoming.map_or(0, Vec::len),
                incoming.map_or(0, Vec::capacity),
                std::mem::size_of::<NodeId>(),
            ));
        }
        bytes = bytes
            .saturating_add(hash_growth_bytes::<NodeId, Vec<Edge>>(
                self.edges.len(),
                self.edges.capacity(),
                usize::from(outgoing.is_none()),
            ))
            .saturating_add(hash_growth_bytes::<String, Vec<(NodeId, NodeId)>>(
                self.label_index.len(),
                self.label_index.capacity(),
                usize::from(pairs.is_none()),
            ))
            .saturating_add(hash_growth_bytes::<NodeId, Vec<NodeId>>(
                self.incoming_edges.len(),
                self.incoming_edges.capacity(),
                usize::from(incoming.is_none()),
            ))
            .saturating_add(hash_growth_bytes::<NodeId, usize>(
                self.in_degrees.len(),
                self.in_degrees.capacity(),
                usize::from(!self.in_degrees.contains_key(&dst)),
            ));
        let stats = self
            .graph_stats_state
            .read()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(label_stats) = stats.labels.get(label) {
            bytes = bytes
                .saturating_add(hash_growth_bytes::<NodeId, usize>(
                    label_stats.sources.len(),
                    label_stats.sources.capacity(),
                    usize::from(!label_stats.sources.contains_key(&src)),
                ))
                .saturating_add(hash_growth_bytes::<NodeId, usize>(
                    label_stats.targets.len(),
                    label_stats.targets.capacity(),
                    usize::from(!label_stats.targets.contains_key(&dst)),
                ));
        } else {
            bytes = bytes.saturating_add(1024);
        }
        Ok(bytes)
    }

    fn label_entry_bytes(&self, label: &str) -> usize {
        self.label_index
            .get_key_value(label)
            .map_or(0, |(key, pairs)| {
                key.capacity() + pairs.capacity() * std::mem::size_of::<(NodeId, NodeId)>()
            })
    }
    fn incoming_entry_bytes(&self, id: NodeId) -> usize {
        self.incoming_edges.get(&id).map_or(0, |sources| {
            sources.capacity() * std::mem::size_of::<NodeId>()
        })
    }
    fn rebuild_graph_heap(&mut self) {
        self.edge_heap_bytes = self
            .edges
            .values()
            .map(|edges| {
                edges.capacity() * std::mem::size_of::<Edge>()
                    + edges.iter().map(edge_dynamic_bytes).sum::<usize>()
            })
            .sum();
        self.label_heap_bytes = self
            .label_index
            .iter()
            .map(|(key, pairs)| {
                key.capacity() + pairs.capacity() * std::mem::size_of::<(NodeId, NodeId)>()
            })
            .sum();
        self.incoming_heap_bytes = self
            .incoming_edges
            .values()
            .map(|sources| sources.capacity() * std::mem::size_of::<NodeId>())
            .sum();
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
        Self::validate_edge_value(&label, &metadata)?;
        if !weight.is_finite() {
            return Err(TriviumError::InvalidInput(
                "边权重必须是有限浮点数 (Edge weight must be finite)".into(),
            ));
        }

        let incoming_before = self.incoming_entry_bytes(dst);
        let label_before = self.label_entry_bytes(&label);
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
                self.edge_heap_bytes = self
                    .edge_heap_bytes
                    .saturating_sub(edge_dynamic_bytes(edge));
                edge.metadata = metadata;
                self.edge_heap_bytes = self
                    .edge_heap_bytes
                    .saturating_add(edge_dynamic_bytes(edge));
                changed = true;
            }
            if changed {
                self.checkpoint_dirty = true;
                self.generation = self.generation.wrapping_add(1);
            }
            return Ok(());
        }

        let slots_before = outgoing.capacity() * std::mem::size_of::<Edge>();
        let edge = Edge {
            target_id: dst,
            label: label.clone(),
            weight,
            metadata,
        };
        self.edge_heap_bytes += edge_dynamic_bytes(&edge);
        outgoing.push(edge);
        self.edge_heap_bytes = self
            .edge_heap_bytes
            .saturating_sub(slots_before)
            .saturating_add(outgoing.capacity() * std::mem::size_of::<Edge>());
        *self.in_degrees.entry(dst).or_insert(0) += 1;
        if !self.incoming_edges.entry(dst).or_default().contains(&src) {
            self.incoming_edges.entry(dst).or_default().push(src);
        }
        self.label_index
            .entry(label.clone())
            .or_default()
            .push((src, dst));
        self.incoming_heap_bytes = self
            .incoming_heap_bytes
            .saturating_sub(incoming_before)
            .saturating_add(self.incoming_entry_bytes(dst));
        self.label_heap_bytes = self
            .label_heap_bytes
            .saturating_sub(label_before)
            .saturating_add(self.label_entry_bytes(&label));
        self.graph_stats_state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .edge_added(src, dst, &label);
        self.mark_graph_changed();
        self.mark_changed(false);
        Ok(())
    }

    pub fn get_edge(&self, src: NodeId, dst: NodeId, label: &str) -> Option<&Edge> {
        self.get_edges(src)?
            .iter()
            .find(|edge| edge.target_id == dst && edge.label == label)
    }

    pub fn graph_stats(&self) -> GraphStats {
        self.graph_stats_state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot()
    }

    pub fn validate_graph(&self) -> GraphIntegrityReport {
        let mut expected_incoming: HashMap<NodeId, HashSet<NodeId>> = HashMap::new();
        let mut expected_degrees: HashMap<NodeId, usize> = HashMap::new();
        let mut expected_labels: HashMap<String, HashSet<(NodeId, NodeId)>> = HashMap::new();
        let mut dangling_edges = 0;
        let mut duplicate_edges = 0;

        for src in self
            .edges
            .keys()
            .copied()
            .chain(self.mapped_graph.iter().flat_map(|graph| graph.sources()))
        {
            let edges = self.get_edges(src).unwrap_or(&[]);
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
            .mapped_graph
            .as_ref()
            .map_or(&self.incoming_edges, |graph| graph.incoming_index())
            .iter()
            .map(|(&id, sources)| (id, sources.iter().copied().collect()))
            .filter(|(_, sources): &(NodeId, HashSet<NodeId>)| !sources.is_empty())
            .collect();
        let actual_labels: HashMap<String, HashSet<(NodeId, NodeId)>> = self
            .mapped_graph
            .as_ref()
            .map_or(&self.label_index, |graph| graph.label_index())
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
        self.rebuild_graph_stats_state();
        self.rebuild_graph_heap();
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
        !crate::vector::is_binary_vector::<T>()
            && self.dim <= crate::index::bq::MAX_BQ_DIM
            && self.auto_build_quiver
            && self.payloads.len() >= auto_quiver_node_threshold(self.dim)
            && self.quiver_index.is_none()
    }

    pub fn search_cache_needs_prepare(&self, materialize_flat: bool) -> bool {
        materialize_flat && self.vec_pool.cache_needs_rebuild()
    }

    pub fn prepare_search_cache(&mut self, materialize_flat: bool) {
        if materialize_flat {
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
        // 导航树父节点、串行遍历栈和访问位图也必须在分配前纳入峰值预算。
        let connectivity = nodes
            .saturating_mul(std::mem::size_of::<u32>() * 2)
            .saturating_add(nodes.div_ceil(8));
        // SM位码按64字节行对齐，包含外部ID与精排归一化比例；不保存FP32副本。
        let sm_stride = (chunks.saturating_mul(2).saturating_add(3))
            .div_ceil(8)
            .saturating_mul(64);
        let sm_rows = nodes.saturating_mul(sm_stride.saturating_add(12));
        signatures
            .saturating_add(adjacency)
            .saturating_add(mappings)
            .saturating_add(worker_bitsets)
            .saturating_add(connectivity)
            .saturating_add(sm_rows)
    }

    /// 显式准备连续视图；当前持久化直接使用分段向量，不需要物化。
    pub fn prepare_persistence_cache(&mut self, materialize_flat: bool) {
        if materialize_flat {
            self.vec_pool.ensure_cache();
        }
    }

    /// 管理 QuIVer 自动构建；只有显式请求连续视图时才准备合并缓存。
    /// 当前查询按 slot 或分段读取向量，不依赖连续副本和旧 1-bit 签名。
    pub fn ensure_vectors_cache(&mut self, materialize_flat: bool) {
        // 连续视图入口与普通查询共享自动构建资格和阈值。
        if self.auto_quiver_build_needed() {
            self.build_quiver_impl(&QuIVerConfig::default());
        }

        // 仅为显式连续视图请求物化混合两层的缓存。
        if materialize_flat {
            self.vec_pool.ensure_cache();
        }
    }

    /// 按需计算单条旧式签名，不创建整库数组。
    pub fn get_bq_signature(&self, index: usize) -> Option<BqSignature> {
        if let Some(signatures) = self.bq_signatures.get() {
            return signatures.get(index).copied();
        }
        self.vec_pool.get(index).map(BqSignature::from_vector)
    }

    /// 显式请求旧式连续签名视图时才分配；该分配计入内存统计。
    pub fn bq_signatures_slice(&self) -> &[BqSignature] {
        self.bq_signatures.get_or_init(|| {
            (0..self.vec_pool.total_count())
                .map(|index| self.get_bq_signature(index).unwrap_or_default())
                .collect()
        })
    }

    /// 已物化旧签名视图所占的堆容量；未请求时为零。
    pub fn bq_cache_bytes(&self) -> usize {
        self.bq_signatures.get().map_or(0, |signatures| {
            signatures
                .capacity()
                .saturating_mul(std::mem::size_of::<BqSignature>())
        })
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
        self.checkpoint_dirty = true;
        self.bq_signatures = std::sync::OnceLock::from(sigs);
    }

    /// 从持久化文件恢复 QuIVer 索引（跳过重建）
    pub fn set_quiver_index(&mut self, mut quiver: QuIVer) {
        if crate::vector::is_binary_vector::<T>() {
            return;
        }
        self.checkpoint_dirty = true;
        if self.dim <= crate::index::bq::MAX_BQ_DIM {
            if let Err(error) = quiver.prepare_default_sm(|slot, buffer| {
                if let Some(vector) = self.vec_pool.get(slot) {
                    buffer.extend(vector.iter().map(|v| v.to_f32()));
                    true
                } else {
                    false
                }
            }) {
                tracing::warn!(%error, "旧QuIVer索引SM恢复失败，拒绝发布失配索引");
                return;
            }
            self.quiver_index = Some(quiver);
        }
    }

    /// 设置 QuIVer 同步暂停标记（事务 commit 期间暂停，commit 后恢复）
    #[inline]
    pub(crate) fn quiver_sync_paused(&self) -> bool {
        self.quiver_sync_paused
    }

    pub fn set_quiver_sync_paused(&mut self, paused: bool) {
        self.quiver_sync_paused = paused;
    }

    /// 获取 QuIVer 图索引引用（如果已构建）
    #[inline]
    pub fn quiver(&self) -> Option<&QuIVer> {
        self.quiver_index.as_ref()
    }

    /// 给已构建的快速QuIVer图显式安装查询校正，系数按外部节点ID绑定。
    /// 安装过程中只临时收集FP32向量，查询继续通过原VecPool按slot读取。
    pub fn enable_quiver_corrected_sm(
        &mut self,
        coefficients: &[(NodeId, f32)],
    ) -> std::io::Result<()> {
        let index = self.quiver_index.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "请先构建QuIVer快速图")
        })?;
        let pool = &self.vec_pool;
        index.enable_corrected_sm_with_vectors(coefficients, |slot, buffer| {
            if let Some(vector) = pool.get(slot) {
                buffer.clear();
                buffer.extend(vector.iter().map(|v| v.to_f32()));
                true
            } else {
                false
            }
        })
    }

    /// 查询校正状态可被CRUD失效，调用方可据此决定是否重新校准。
    pub fn quiver_corrected_sm_enabled(&self) -> bool {
        self.quiver_index
            .as_ref()
            .is_some_and(QuIVer::corrected_sm_enabled)
    }

    /// 恢复当前图的默认无训练重建L2；查询只在末尾精排读取原始向量。
    pub fn enable_quiver_sm_l2(&mut self) -> std::io::Result<()> {
        let index = self.quiver_index.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "请先构建QuIVer快速图")
        })?;
        let pool = &self.vec_pool;
        index.enable_sm_l2_with_vectors(|slot, buffer| {
            if let Some(vector) = pool.get(slot) {
                buffer.clear();
                buffer.extend(vector.iter().map(|v| v.to_f32()));
                true
            } else {
                false
            }
        })
    }

    pub fn quiver_sm_l2_enabled(&self) -> bool {
        self.quiver_index
            .as_ref()
            .is_some_and(QuIVer::sm_l2_enabled)
    }

    /// 显式原生欧氏安装；保留实际VecPool向量幅值及零向量。
    pub fn enable_quiver_sm_native_l2(&mut self) -> std::io::Result<()> {
        let index = self.quiver_index.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "请先构建QuIVer快速图")
        })?;
        let pool = &self.vec_pool;
        index.enable_sm_native_l2_with_vectors(|slot, buffer| {
            if let Some(vector) = pool.get(slot) {
                buffer.clear();
                buffer.extend(vector.iter().map(|v| v.to_f32()));
                true
            } else {
                false
            }
        })
    }

    pub fn quiver_sm_native_l2_enabled(&self) -> bool {
        self.quiver_index
            .as_ref()
            .is_some_and(QuIVer::sm_native_l2_enabled)
    }

    /// 手动构建 QuIVer BQ-native Vamana 索引
    ///
    /// 达到维度相关阈值后，查询准备或 `ensure_vectors_cache()` 会按需自动构建。
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
        if crate::vector::is_binary_vector::<T>() || self.dim > crate::index::bq::MAX_BQ_DIM {
            self.quiver_index = None;
            return;
        }
        self.build_quiver_impl(config);
    }

    pub(crate) fn quiver_build_snapshot(&self) -> Option<QuiverBuildSnapshot> {
        if crate::vector::is_binary_vector::<T>()
            || self.dim > crate::index::bq::MAX_BQ_DIM
            || self.vec_pool.total_count() == 0
            || self.payloads.is_empty()
        {
            return None;
        }
        let mut signatures = Bq2Store::new(self.dim);
        signatures.reserve(self.payloads.len());
        let mut ids = Vec::with_capacity(self.payloads.len());
        let mut slots = Vec::with_capacity(self.payloads.len());
        let mut sm =
            crate::index::quiver::corrected_sm::CorrectedSmIndex::empty_routing(self.dim).ok()?;
        sm.reserve_routing(self.payloads.len());
        let mut buffer = Vec::with_capacity(self.dim);
        for (slot, &node_id) in self.indices_to_ids.iter().enumerate() {
            if node_id == 0 || !self.payloads.contains_key(&node_id) {
                continue;
            }
            if let Some(vector) = self.vec_pool.get(slot) {
                buffer.clear();
                buffer.extend(vector.iter().map(|v| v.to_f32()));
                sm.push_routing_vector(&buffer, node_id).ok()?;
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
            sm,
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
        index.set_default_sm(snapshot.sm);
        index
    }

    pub(crate) fn publish_quiver_if_current(
        &mut self,
        source_generation: u64,
        index: QuIVer,
    ) -> bool {
        if crate::vector::is_binary_vector::<T>() || self.vector_generation != source_generation {
            return false;
        }
        self.quiver_index = Some(index);
        self.checkpoint_dirty = true;
        self.vec_pool.advise_random();
        true
    }

    /// QuIVer 构建的内部实现（不调用 ensure_vectors_cache，避免递归）
    ///
    /// 按 slot 流式从 mmap 零拷贝读取向量并转 f32，不物化全量 merged 缓存，
    /// 避免构建期把整个冷数据集复制入堆。
    fn build_quiver_impl(&mut self, config: &QuIVerConfig) {
        self.checkpoint_dirty = true;
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

    pub fn get_payload(&self, id: NodeId) -> Option<std::sync::Arc<serde_json::Value>> {
        self.payloads.get_value(id)
    }

    pub(crate) fn try_get_payload(
        &self,
        id: NodeId,
    ) -> Result<Option<std::sync::Arc<serde_json::Value>>> {
        self.payloads.try_get_value(id)
    }

    pub(crate) fn payload_fingerprint(&self) -> (u64, u32) {
        self.payloads.fingerprint()
    }

    pub(crate) fn get_payload_raw(&self, id: NodeId) -> Option<&[u8]> {
        self.payloads.raw(id)
    }

    pub(crate) fn payload_query_peak(&self, id: NodeId) -> Result<usize> {
        let raw = self
            .get_payload_raw(id)
            .ok_or(TriviumError::NodeNotFound(id))?;
        crate::storage::payload_store::query_decode_peak(raw)
    }

    pub(crate) fn payload_memory_stats(&self) -> PayloadMemoryStats {
        self.payloads.memory_stats()
    }

    pub(crate) fn payload_access_counters(&self) -> (u64, u64) {
        self.payloads.access_counters()
    }

    pub fn configure_payload_cache(&mut self, max_bytes: usize, max_entry_bytes: usize) {
        self.payloads.configure_cache(max_bytes, max_entry_bytes);
    }

    pub(crate) fn save_payload_sidecar(
        &self,
        path: &std::path::Path,
        generation: u64,
    ) -> Result<()> {
        self.payloads.save_sidecar(path, generation)
    }

    pub(crate) fn install_payload_sidecar(
        &mut self,
        mmap: memmap2::Mmap,
        records: Vec<(NodeId, std::ops::Range<usize>)>,
    ) -> Result<()> {
        let mut expected = self.active_node_ids().collect::<Vec<_>>();
        let mut actual = records.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        expected.sort_unstable();
        actual.sort_unstable();
        if expected != actual {
            return Err(TriviumError::CorruptedFile(
                "Payload sidecar 节点集合与主文件不一致".into(),
            ));
        }
        self.payloads.install_mapped(mmap, records)
    }

    pub fn get_edges(&self, id: NodeId) -> Option<&[Edge]> {
        self.edges
            .get(&id)
            .map(Vec::as_slice)
            .or_else(|| self.mapped_graph.as_ref()?.edges(id))
    }

    pub fn incident_edge_count(&self, id: NodeId) -> usize {
        self.get_edges(id)
            .map(<[Edge]>::len)
            .unwrap_or_default()
            .saturating_add(self.in_degrees.get(&id).copied().unwrap_or_default())
    }

    pub fn set_mapped_graph(&mut self, graph: crate::storage::graph_blocks::MappedGraphStore) {
        self.edges.clear();
        self.incoming_edges.clear();
        self.label_index.clear();
        self.edge_heap_bytes = 0;
        self.label_heap_bytes = 0;
        self.incoming_heap_bytes = 0;
        self.in_degrees.clear();
        for pairs in graph.label_index().values() {
            for &(_, target) in pairs {
                *self.in_degrees.entry(target).or_default() += 1;
            }
        }
        self.mapped_graph = Some(graph);
        self.rebuild_graph_stats_state();
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
            .filter_map(|(id, payload)| {
                if payload.get(field) == Some(value) {
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
            .ids()
            .collect::<Vec<_>>()
            .into_par_iter()
            .filter(|&id| {
                self.payloads
                    .get_value(id)
                    .is_some_and(|payload| payload.get(field) == Some(value))
            })
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }

    // ════════════════════════════════════════════════════════
    //  属性二级索引 API
    // ════════════════════════════════════════════════════════

    /// 候选索引在隔离目录中构建；解析与构建峰值均通过准入后才安装到实例。
    pub(crate) fn build_property_index(
        &mut self,
        kind: crate::index::property::PropertyIndexKind,
        fields: &[String],
        unique: bool,
        limit: usize,
    ) -> Result<()> {
        use crate::index::property::{PropertyIndexKind, PropertyIndexRegistry};
        crate::index::property::validate_index_fields(
            fields,
            kind == PropertyIndexKind::Composite,
        )?;
        if self.property_indexes.has_definition(kind, fields, unique) {
            return Ok(());
        }
        // 一次仅解析一条冷记录，读原始字节不会填充实例的 Payload 缓存。
        let parse_peak = self
            .payloads
            .ids()
            .filter_map(|id| self.payloads.raw(id))
            .map(|raw| raw.len().saturating_mul(64))
            .max()
            .unwrap_or(0)
            .saturating_add(1024);
        self.ensure_additional_memory(limit, parse_peak)?;
        let mut build_peak = fields
            .iter()
            .map(|field| field.len().saturating_mul(4))
            .sum::<usize>()
            .saturating_add(1024);
        for id in self.payloads.ids() {
            let raw = self.payloads.raw(id).ok_or_else(|| {
                TriviumError::CorruptedFile(
                    "索引构建缺少 Payload (Index build is missing a Payload)".into(),
                )
            })?;
            let payload: serde_json::Value = serde_json::from_slice(raw).map_err(|e| {
                TriviumError::CorruptedFile(format!(
                    "索引构建 Payload 无效 (Invalid Payload during index build): {e}"
                ))
            })?;
            build_peak = build_peak.saturating_add(PropertyIndexRegistry::estimate_build_row(
                kind, fields, unique, &payload,
            )?);
            self.ensure_additional_memory(limit, build_peak.saturating_add(parse_peak))?;
        }
        let mut candidate = PropertyIndexRegistry::empty_build(kind, fields, unique)?;
        for id in self.payloads.ids() {
            let raw = self.payloads.raw(id).ok_or_else(|| {
                TriviumError::CorruptedFile(
                    "索引构建缺少 Payload (Index build is missing a Payload)".into(),
                )
            })?;
            let payload: serde_json::Value = serde_json::from_slice(raw).map_err(|e| {
                TriviumError::CorruptedFile(format!(
                    "索引构建 Payload 无效 (Invalid Payload during index build): {e}"
                ))
            })?;
            candidate.validate_unique_changes([(id, Some(&payload))])?;
            candidate.insert(id, &payload);
        }
        self.ensure_additional_memory(limit, candidate.estimated_memory_bytes())?;
        self.property_indexes.install_build(candidate);
        self.mark_property_changed();
        Ok(())
    }

    pub fn register_unique_property_index(&mut self, field: &str) -> Result<()> {
        let payloads = &self.payloads;
        self.property_indexes
            .register_unique(field, payloads.iter())?;
        self.mark_property_changed();
        Ok(())
    }

    pub fn register_unique_composite_property_index(&mut self, fields: &[String]) -> Result<()> {
        let payloads = &self.payloads;
        self.property_indexes
            .register_unique_composite(fields, payloads.iter())?;
        self.mark_property_changed();
        Ok(())
    }

    pub fn validate_unique_changes<'a, I>(&self, changes: I) -> Result<()>
    where
        I: IntoIterator<Item = (NodeId, Option<&'a serde_json::Value>)>,
    {
        self.property_indexes.validate_unique_changes(changes)
    }

    pub(crate) fn prepare_unique_delta(
        &self,
        changes: &[(NodeId, Option<&serde_json::Value>)],
    ) -> Result<crate::index::property::UniqueIndexDelta> {
        self.property_indexes
            .prepare_unique_delta(changes, |id| {
                self.payloads
                    .raw(id)
                    .map(|raw| {
                        serde_json::from_slice(raw)
                            .map(std::sync::Arc::new)
                            .map_err(|error| {
                                TriviumError::CorruptedFile(format!(
                                    "唯一键原值无法解析 (Cannot decode previous unique key payload): {error}"
                                ))
                            })
                    })
                    .transpose()
            })
    }
    pub(crate) fn reserve_unique_delta(
        &mut self,
        delta: &crate::index::property::UniqueIndexDelta,
    ) -> Result<()> {
        self.property_indexes.reserve_unique_delta(delta)
    }
    pub(crate) fn apply_unique_delta(
        &mut self,
        delta: crate::index::property::UniqueIndexDelta,
    ) -> Result<()> {
        self.property_indexes.apply_unique_delta(delta)
    }
    pub(crate) fn unique_updates_paused(&self) -> bool {
        self.property_indexes.unique_updates_paused()
    }
    pub(crate) fn set_unique_updates_paused(&mut self, paused: bool) {
        self.property_indexes.set_unique_updates_paused(paused);
    }

    pub fn validate_unique_payloads<'a, I>(&self, payloads: I) -> Result<()>
    where
        I: IntoIterator<Item = (NodeId, &'a serde_json::Value)>,
    {
        self.property_indexes.validate_unique_payloads(payloads)
    }

    pub fn unique_index_definitions(&self) -> Vec<Vec<String>> {
        self.property_indexes.unique_definitions()
    }

    pub(crate) fn admit_index_rebuild(
        &self,
        kind: crate::index::property::PropertyIndexKind,
        fields: &[String],
        unique: bool,
    ) -> Result<()> {
        if self.load_memory_limit == 0 {
            return Ok(());
        }
        let mut peak = 1024usize;
        for id in self.payloads.ids() {
            let raw = self.payloads.raw(id).ok_or_else(|| {
                TriviumError::CorruptedFile("索引缺少 Payload (Index payload is missing)".into())
            })?;
            let parse_peak = raw.len().saturating_mul(64);
            self.check_load_allocation(peak.saturating_add(parse_peak))?;
            let payload = serde_json::from_slice(raw).map_err(|e| {
                TriviumError::CorruptedFile(format!(
                    "索引 Payload 无效 (Invalid index payload): {e}"
                ))
            })?;
            peak = peak.saturating_add(PropertyIndexRegistry::estimate_build_row(
                kind, fields, unique, &payload,
            )?);
            self.check_load_allocation(peak.saturating_add(parse_peak))?;
        }
        Ok(())
    }

    pub fn restore_unique_index_definitions(&mut self, definitions: &[Vec<String>]) -> Result<()> {
        self.property_indexes.clear_unique_constraints();
        for fields in definitions {
            self.admit_index_rebuild(
                if fields.len() == 1 {
                    crate::index::property::PropertyIndexKind::Hash
                } else {
                    crate::index::property::PropertyIndexKind::Composite
                },
                fields,
                true,
            )?;
            if fields.len() == 1 {
                self.register_unique_property_index(&fields[0])?;
            } else {
                self.register_unique_composite_property_index(fields)?;
            }
        }
        Ok(())
    }

    /// 注册属性索引：对指定字段建立倒排索引，并回填所有已有节点
    pub fn register_property_index(&mut self, field: &str) {
        let payloads = &self.payloads;
        self.property_indexes.register(field, payloads.iter());
        self.mark_property_changed();
    }

    pub fn register_composite_property_index(&mut self, fields: &[String]) {
        let payloads = &self.payloads;
        self.property_indexes
            .register_composite(fields, payloads.iter());
        self.mark_property_changed();
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

    pub fn register_ngram_property_index(&mut self, field: &str) -> Result<()> {
        let payloads = &self.payloads;
        self.property_indexes
            .register_ngram(field, payloads.iter())?;
        self.mark_property_changed();
        Ok(())
    }

    pub fn find_by_ngram_property_index(
        &self,
        field: &str,
        needle: &str,
    ) -> Result<Option<Vec<NodeId>>> {
        self.property_indexes.ngram_lookup(field, needle)
    }

    pub fn drop_ngram_property_index(&mut self, field: &str) {
        self.property_indexes.drop_ngram_index(field);
        self.mark_property_changed();
    }

    pub fn register_bitmap_property_index(&mut self, field: &str) {
        let payloads = &self.payloads;
        self.property_indexes
            .register_bitmap(field, payloads.iter());
        self.mark_property_changed();
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
        self.mark_property_changed();
    }

    pub fn drop_bitmap_property_index(&mut self, field: &str) {
        self.property_indexes.drop_bitmap_index(field);
        self.mark_property_changed();
    }

    pub fn register_ordered_property_index(&mut self, field: &str) {
        let payloads = &self.payloads;
        self.property_indexes
            .register_ordered(field, payloads.iter());
        self.mark_property_changed();
    }

    pub fn drop_ordered_property_index(&mut self, field: &str) {
        self.property_indexes.drop_ordered_index(field);
        self.mark_property_changed();
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
        self.mark_property_changed();
    }

    /// 查询属性索引。字段已建索引时即使值不存在也返回空切片。
    pub fn find_by_property_index_limit(
        &self,
        field: &str,
        value: &serde_json::Value,
        limit: Option<usize>,
    ) -> Option<Vec<NodeId>> {
        self.property_indexes.lookup_limit(field, value, limit)
    }

    pub fn find_by_property_index(
        &self,
        field: &str,
        value: &serde_json::Value,
    ) -> Option<Vec<NodeId>> {
        self.find_by_property_index_limit(field, value, None)
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
        if let Some((generation, stats)) = self
            .column_pair_stats_cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            && *generation == self.property_generation
        {
            return stats.clone();
        }
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
                    for payload in self.payloads.values() {
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
        *self
            .column_pair_stats_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some((self.property_generation, output.clone()));
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
        if let Some((_, _, _, stats)) = self
            .cross_modal_stats_cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&cache_key)
            .filter(|(property, graph, vector, _)| {
                *property == self.property_generation
                    && *graph == self.graph_generation
                    && *vector == self.vector_generation
            })
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
            let graph_state = self
                .graph_stats_state
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let global_first = graph_state.edge_count as f64;
            let global_second = graph_state.sum_out_degree_squared as f64;
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
            .insert(
                cache_key,
                (
                    self.property_generation,
                    self.graph_generation,
                    self.vector_generation,
                    stats.clone(),
                ),
            );
        Some(stats)
    }

    pub fn index_memory_stats(&self) -> crate::observability::IndexMemoryStats {
        let property_bytes = self.property_indexes.estimated_memory_bytes();
        let property_mapped_bytes = self.property_indexes.mapped_bytes();
        let graph_mapped_bytes = self.mapped_graph_bytes();
        let quiver_bytes = self
            .quiver_index
            .as_ref()
            .map(|index| index.hot_memory_bytes())
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

    pub fn rebuild_property_indexes_from_definitions(
        &mut self,
        definitions: &[(crate::index::property::PropertyIndexKind, Vec<String>)],
    ) {
        let mut indexes = PropertyIndexRegistry::default();
        for (kind, fields) in definitions {
            match kind {
                crate::index::property::PropertyIndexKind::Hash => {
                    if let Some(field) = fields.first() {
                        indexes.register(field, self.payloads.iter());
                    }
                }
                crate::index::property::PropertyIndexKind::Ordered => {
                    if let Some(field) = fields.first() {
                        indexes.register_ordered(field, self.payloads.iter());
                    }
                }
                crate::index::property::PropertyIndexKind::Composite => {
                    indexes.register_composite(fields, self.payloads.iter());
                }
                crate::index::property::PropertyIndexKind::Bitmap => {
                    if let Some(field) = fields.first() {
                        indexes.register_bitmap(field, self.payloads.iter());
                    }
                }
                crate::index::property::PropertyIndexKind::Ngram => {
                    if let Some(field) = fields.first() {
                        let _ = indexes.register_ngram(field, self.payloads.iter());
                    }
                }
            }
        }
        self.property_indexes = indexes;
        self.mark_property_changed();
    }

    pub fn set_property_indexes(&mut self, indexes: PropertyIndexRegistry) {
        self.checkpoint_dirty = true;
        self.property_indexes = indexes;
        self.mark_property_changed();
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
        self.reserve_text_deletes(1)?;

        // 1. 向量层：通过 VecPool 逻辑删除（置零），并回收物理卡槽
        if let Some(idx) = self.ids_to_indices.remove(&id) {
            self.vec_pool.zero_out(idx);
            self.indices_to_ids[idx] = 0; // 盖上墓碑标识，防止后续被误认
            self.free_slots.push(idx); // 抛入环保回收池，供下一个 insert 使用！
        }

        // 2. 属性索引清理（必须在 payload 移除之前）
        if let Some(payload) = self.get_payload(id).map(|payload| (*payload).clone()) {
            self.remove_from_property_index(id, &payload);
        }

        // 3. 元数据层
        if self.deferred_text_path.is_some() {
            self.pending_text_mutations
                .push(PendingTextMutation::Delete(id));
        } else {
            self.text_index.remove_node(id);
        }
        self.payloads.remove(id);
        self.active_ids.remove(id);

        // 3. 图谱层：删除出边 + 清理其他节点指向该节点的入边
        //    同时收集需要从 label_index 中清理的标签集合，最后批量清理
        let mut dirty_labels: HashMap<String, Vec<(NodeId, NodeId)>> = HashMap::new();

        if let Some(outgoing_edges) = self.edges.remove(&id) {
            self.edge_heap_bytes = self.edge_heap_bytes.saturating_sub(
                outgoing_edges.capacity() * std::mem::size_of::<Edge>()
                    + outgoing_edges.iter().map(edge_dynamic_bytes).sum::<usize>(),
            );
            // 清理这些出边目标节点的入度计数与反向哈希网记录
            for edge in &outgoing_edges {
                let target = edge.target_id;
                if let Some(in_deg) = self.in_degrees.get_mut(&target) {
                    *in_deg = in_deg.saturating_sub(1);
                    if *in_deg == 0 {
                        self.in_degrees.remove(&target);
                    }
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
            self.incoming_heap_bytes = self
                .incoming_heap_bytes
                .saturating_sub(incoming.capacity() * std::mem::size_of::<NodeId>());
            for src_id in incoming {
                if let Some(edge_list) = self.edges.get_mut(&src_id) {
                    for edge in edge_list.iter() {
                        if edge.target_id == id {
                            self.edge_heap_bytes = self
                                .edge_heap_bytes
                                .saturating_sub(edge_dynamic_bytes(edge));
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

        // 批量清理 label_index：每个标签只做一次 retain，避免 O(N²) 雪崩。
        // 删除自环时，同一条边可能同时出现在出边与入边收集路径，必须先去重。
        let dirty_labels = dirty_labels
            .into_iter()
            .map(|(label, pairs)| (label, pairs.into_iter().collect::<HashSet<_>>()))
            .collect::<HashMap<_, _>>();
        for (label, to_remove) in &dirty_labels {
            let before = self.label_entry_bytes(label);
            if let Some(pairs) = self.label_index.get_mut(label) {
                pairs.retain(|pair| !to_remove.contains(pair));
                if pairs.is_empty() {
                    self.label_index.remove(label);
                }
            }
            self.label_heap_bytes = self
                .label_heap_bytes
                .saturating_sub(before)
                .saturating_add(self.label_entry_bytes(label));
        }
        {
            let mut state = self
                .graph_stats_state
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for (label, pairs) in &dirty_labels {
                for &(src, dst) in pairs {
                    state.edge_removed(src, dst, label);
                }
            }
            state.node_removed(id);
        }
        self.mark_property_changed();
        self.mark_graph_changed();

        if !self.quiver_sync_paused {
            self.invalidate_quiver_for_delete(id);
        }
        self.mark_changed(true);

        Ok(())
    }

    /// 断开两个节点之间的指定标签边（幂等）。
    pub fn unlink_label(&mut self, src: NodeId, dst: NodeId, label: &str) -> Result<()> {
        let label_before = self.label_entry_bytes(label);
        let Some(edge_list) = self.edges.get_mut(&src) else {
            return self.validate_unlink(src);
        };
        let before = edge_list.len();
        edge_list.retain(|edge| {
            let remove = edge.target_id == dst && edge.label == label;
            if remove {
                self.edge_heap_bytes = self
                    .edge_heap_bytes
                    .saturating_sub(edge_dynamic_bytes(edge));
            }
            !remove
        });
        let removed = before - edge_list.len();
        if removed == 0 {
            return Ok(());
        }
        if let Some(in_deg) = self.in_degrees.get_mut(&dst) {
            *in_deg = in_deg.saturating_sub(removed);
            if *in_deg == 0 {
                self.in_degrees.remove(&dst);
            }
        }
        if let Some(pairs) = self.label_index.get_mut(label) {
            pairs.retain(|&(source, target)| !(source == src && target == dst));
            if pairs.is_empty() {
                self.label_index.remove(label);
            }
        }
        self.label_heap_bytes = self
            .label_heap_bytes
            .saturating_sub(label_before)
            .saturating_add(
                self.label_index
                    .get_key_value(label)
                    .map_or(0, |(key, pairs)| {
                        key.capacity() + pairs.capacity() * std::mem::size_of::<(NodeId, NodeId)>()
                    }),
            );
        let still_connected = edge_list.iter().any(|edge| edge.target_id == dst);
        if !still_connected && let Some(incoming) = self.incoming_edges.get_mut(&dst) {
            incoming.retain(|&source| source != src);
        }
        for _ in 0..removed {
            self.graph_stats_state
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .edge_removed(src, dst, label);
        }
        self.mark_graph_changed();
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
            let removed_labels = edge_list
                .iter()
                .filter(|edge| edge.target_id == dst)
                .map(|edge| edge.label.clone())
                .collect::<Vec<_>>();
            let mut labels_before = 0;
            let mut labels_after = 0;
            // 先清理 label_index 中对应的条目
            for edge in edge_list.iter() {
                if edge.target_id == dst
                    && let Some(pairs) = self.label_index.get_mut(&edge.label)
                {
                    labels_before += pairs.capacity() * std::mem::size_of::<(NodeId, NodeId)>()
                        + edge.label.len();
                    pairs.retain(|&(s, d)| !(s == src && d == dst));
                    if !pairs.is_empty() {
                        labels_after += pairs.capacity() * std::mem::size_of::<(NodeId, NodeId)>()
                            + edge.label.len();
                    }
                }
            }
            edge_list.retain(|e| {
                if e.target_id == dst {
                    self.edge_heap_bytes =
                        self.edge_heap_bytes.saturating_sub(edge_dynamic_bytes(e));
                    false
                } else {
                    true
                }
            });
            if edge_list.len() < initial_len {
                let removed_count = initial_len - edge_list.len();
                if let Some(in_deg) = self.in_degrees.get_mut(&dst) {
                    *in_deg = in_deg.saturating_sub(removed_count);
                    if *in_deg == 0 {
                        self.in_degrees.remove(&dst);
                    }
                }
                if let Some(incoming) = self.incoming_edges.get_mut(&dst) {
                    incoming.retain(|&id| id != src);
                }
                for label in removed_labels {
                    if self.label_index.get(&label).is_some_and(Vec::is_empty) {
                        self.label_index.remove(&label);
                    }
                    self.graph_stats_state
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .edge_removed(src, dst, &label);
                }
                self.label_heap_bytes = self
                    .label_heap_bytes
                    .saturating_sub(labels_before)
                    .saturating_add(labels_after);
                self.mark_graph_changed();
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
        crate::storage::payload_store::json_encoded_len(&payload)?;
        match self.get_payload(id).map(|payload| (*payload).clone()) {
            Some(old_payload) => {
                let sig = calculate_json_signature(&payload);
                if let Some(&idx) = self.ids_to_indices.get(&id) {
                    self.fast_tags[idx] = sig;
                }
                // 属性索引同步更新
                self.property_indexes.update(id, &old_payload, &payload);
                self.mark_property_changed();
                self.payloads.insert_value(id, payload)?;
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
            .map(|payload| (*payload).clone())
            .ok_or(TriviumError::NodeNotFound(id))?;

        let mut new_payload = old_payload;

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
                    let current = obj
                        .get(k)
                        .unwrap_or(&serde_json::Value::Number(0.into()))
                        .clone();
                    obj.insert(k.clone(), increment_json_number(&current, v)?);
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

    pub(crate) fn active_vector_at_slot(&self, slot: usize) -> Result<Option<(NodeId, &[T])>> {
        let Some(&id) = self.indices_to_ids.get(slot) else {
            return Ok(None);
        };
        if id == 0 || !self.payloads.contains_key(&id) {
            return Ok(None);
        }
        let vector = self.vec_pool.get(slot).ok_or_else(|| {
            TriviumError::CorruptedFile("活跃节点缺少向量 (Active node vector is missing)".into())
        })?;
        Ok(Some((id, vector)))
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

    /// 惰性遍历活跃 NodeId，顺序稳定且不产生查询期全量分配。
    pub fn active_node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.active_ids.iter()
    }

    /// 返回所有活跃节点 ID，按 ID 升序。
    pub fn all_node_ids(&self) -> Vec<NodeId> {
        #[cfg(any(test, feature = "test-hooks"))]
        self.id_materializations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.active_node_ids().collect()
    }

    /// 测试专用：读取全库 NodeId 物化次数。
    #[cfg(feature = "test-hooks")]
    pub fn id_materializations(&self) -> u64 {
        self.id_materializations
            .load(std::sync::atomic::Ordering::Relaxed)
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
        let payload_bytes = self.payloads.heap_memory_bytes();
        let edge_bytes = self.edge_heap_bytes
            + self.label_heap_bytes
            + self.incoming_heap_bytes
            + self.edges.capacity() * std::mem::size_of::<(NodeId, Vec<Edge>)>()
            + self.label_index.capacity() * std::mem::size_of::<(String, Vec<(NodeId, NodeId)>)>()
            + self.incoming_edges.capacity() * std::mem::size_of::<(NodeId, Vec<NodeId>)>()
            + self.in_degrees.capacity() * std::mem::size_of::<(NodeId, usize)>()
            + self
                .graph_stats_state
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .heap_memory_bytes()
            + self
                .mapped_graph
                .as_ref()
                .map_or(0, |graph| graph.heap_memory_bytes());
        let index_bytes = self.indices_to_ids.len() * std::mem::size_of::<NodeId>()
            + self.ids_to_indices.len()
                * (std::mem::size_of::<NodeId>() + std::mem::size_of::<usize>());
        let quiver_bytes = self
            .quiver_index
            .as_ref()
            .map(|index| index.hot_memory_bytes())
            .unwrap_or_default();
        let text_bytes = self.text_index.estimated_memory_bytes()
            + self.pending_text_mutations.capacity() * std::mem::size_of::<PendingTextMutation>()
            + self.pending_text_bytes;
        let property_index_bytes = self.property_indexes.estimated_memory_bytes();
        vec_bytes
            + self.bq_cache_bytes()
            + payload_bytes
            + edge_bytes
            + index_bytes
            + quiver_bytes
            + text_bytes
            + property_index_bytes
    }

    pub(crate) fn begin_loading(&mut self, limit: usize) {
        self.load_memory_limit = limit;
        self.text_memory_limit = limit;
        self.auto_build_quiver = false;
        self.configure_payload_cache(0, 0);
    }

    pub(crate) fn check_load_allocation(&self, additional: usize) -> Result<()> {
        self.ensure_additional_memory(self.load_memory_limit, additional)
    }

    pub(crate) fn finish_loading(
        &mut self,
        cache_bytes: usize,
        cache_entry_bytes: usize,
    ) -> Result<()> {
        self.check_load_allocation(0)?;
        let cache_bytes = if self.load_memory_limit == 0 {
            cache_bytes
        } else {
            cache_bytes.min(
                self.load_memory_limit
                    .saturating_sub(self.estimated_memory_bytes()),
            )
        };
        self.configure_payload_cache(cache_bytes, cache_entry_bytes.min(cache_bytes));
        self.load_memory_limit = 0;
        Ok(())
    }

    pub(crate) fn recovery_latch(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.recovery_required)
    }

    pub(crate) fn require_recovery(&mut self) {
        self.recovery_required.store(true, Ordering::Release);
    }

    pub(crate) fn ensure_readable(&self) -> Result<()> {
        if self.recovery_required.load(Ordering::Acquire) {
            Err(TriviumError::WalClosed)
        } else {
            Ok(())
        }
    }

    pub(crate) fn ensure_additional_memory(&self, limit: usize, additional: usize) -> Result<()> {
        let current = self.estimated_memory_bytes();
        if limit > 0 && current.saturating_add(additional) > limit {
            return Err(TriviumError::CapacityReservationRejected {
                requested_nodes: 0,
                estimated_bytes: additional,
                current_bytes: current,
                memory_limit: limit,
            });
        }
        Ok(())
    }

    pub(crate) fn resource_runtime(&self) -> &crate::resources::ResourceRuntime {
        &self.resource_runtime
    }

    pub(crate) fn attach_resources(
        &mut self,
        runtime: crate::resources::ResourceRuntime,
        reservation: crate::resources::DatabaseReservation,
    ) {
        self.resource_runtime = runtime;
        self.resource_reservation = Some(reservation);
    }

    pub(crate) fn resize_resource_reservation(&self, bytes: usize) -> Result<()> {
        if self.resource_runtime.stats().memory_limit > 0 && bytes < self.estimated_memory_bytes() {
            return Err(TriviumError::ResourceBudgetExceeded {
                resource: "database_heap_bytes",
                requested: self.estimated_memory_bytes(),
                used: 0,
                limit: bytes,
            });
        }
        if let Some(reservation) = &self.resource_reservation {
            reservation.resize(bytes)?;
        }
        Ok(())
    }

    pub(crate) fn reserved_database_bytes(&self) -> usize {
        self.resource_reservation
            .as_ref()
            .map_or(0, |reservation| reservation.bytes())
    }

    pub(crate) fn release_resource_storage(&mut self) {
        let runtime = self.resource_runtime.clone();
        *self = Self::new(self.dim);
        self.resource_runtime = runtime;
    }

    pub(crate) fn needs_checkpoint(&self) -> bool {
        self.checkpoint_dirty
    }

    pub(crate) fn mark_checkpoint_clean(&mut self) {
        self.checkpoint_dirty = false;
    }

    pub(crate) fn advise_cold_vectors_dontneed(&self) {
        self.vec_pool.advise_dontneed();
    }

    pub(crate) fn finish_vector_loading(&mut self) -> Result<()> {
        self.vec_pool.finish_loading()
    }

    pub(crate) fn mmap_vector_bytes(&self) -> usize {
        self.vec_pool.mmap_bytes()
    }

    // --- 文本引擎接口 ---

    pub fn index_keyword(&mut self, id: NodeId, keyword: &str) {
        if self.contains(id) {
            self.checkpoint_dirty = true;
            if self.deferred_text_path.is_some() {
                self.pending_text_bytes = self.pending_text_bytes.saturating_add(keyword.len());
                self.pending_text_mutations
                    .push(PendingTextMutation::Keyword(id, keyword.to_owned()));
            } else {
                self.text_index.add_keyword(id, keyword);
            }
        }
    }

    pub fn index_text(&mut self, id: NodeId, text: &str) {
        if self.contains(id) {
            self.checkpoint_dirty = true;
            if self.deferred_text_path.is_some() {
                self.pending_text_bytes = self.pending_text_bytes.saturating_add(text.len());
                self.pending_text_mutations
                    .push(PendingTextMutation::Text(id, text.to_owned()));
            } else {
                self.text_index.add_text(id, text);
            }
        }
    }

    pub fn build_text_index(&mut self) {
        self.checkpoint_dirty = true;
        self.text_index.build();
    }

    pub(crate) fn try_build_text_index(&mut self) -> Result<()> {
        self.checkpoint_dirty = true;
        self.text_index.try_build()
    }

    pub(crate) fn rebuild_text_index(
        &mut self,
        documents: &[crate::index::text::TextIndexDocument],
    ) -> Result<()> {
        let estimate = documents
            .iter()
            .try_fold(64 * 1024usize, |total, document| {
                std::iter::once(document.text.len())
                    .chain(document.keywords.iter().map(String::len))
                    .try_fold(total, |total, length| {
                        length
                            .checked_mul(64)
                            .and_then(|bytes| bytes.checked_add(512))
                            .and_then(|bytes| total.checked_add(bytes))
                    })
            })
            .ok_or_else(|| TriviumError::CapacityAllocationFailed {
                reason: "文本重建工作区估算溢出".into(),
            })?;
        self.check_text_memory_budget(estimate)?;
        let runtime = self.resource_runtime.clone();
        let _workspace = if runtime.is_maintenance_task() {
            runtime.reserve_maintenance_workspace(estimate)?
        } else {
            runtime.reserve_workspace(estimate)?
        };
        let mut seen = HashSet::new();
        seen.try_reserve(documents.len()).map_err(|error| {
            TriviumError::CapacityAllocationFailed {
                reason: format!("文本重建节点目录预留失败：{error}"),
            }
        })?;
        for document in documents {
            self.validate_update_payload(document.id)?;
            if !seen.insert(document.id) {
                return Err(TriviumError::InvalidInput(format!(
                    "文本重建节点重复：{}",
                    document.id
                )));
            }
            if document.keywords.iter().any(String::is_empty) {
                return Err(TriviumError::InvalidInput("关键词不能为空".into()));
            }
        }
        let mut index = TextIndex::new();
        for document in documents {
            index.add_text(document.id, &document.text);
            for keyword in &document.keywords {
                index.add_keyword(document.id, keyword);
            }
        }
        index.try_build()?;
        self.set_text_index(index);
        Ok(())
    }

    pub(crate) fn set_text_index(&mut self, text_index: TextIndex) {
        self.checkpoint_dirty = true;
        self.text_index = text_index;
        self.text_index
            .retain_nodes(|id| self.active_ids.contains(id));
        self.deferred_text_path = None;
        self.deferred_text_meta = None;
        self.pending_text_mutations = Vec::new();
        self.pending_text_bytes = 0;
    }

    pub(crate) fn defer_text_index(&mut self, path: &str) {
        self.text_index = TextIndex::new();
        self.deferred_text_path = Some(path.to_owned());
        self.deferred_text_meta = crate::storage::file_format::deferred_text_metadata(path);
        self.pending_text_mutations = Vec::new();
        self.pending_text_bytes = 0;
    }

    pub(crate) fn text_index_is_deferred(&self) -> bool {
        self.deferred_text_path.is_some()
    }

    pub(crate) fn text_index_has_pending_changes(&self) -> bool {
        !self.pending_text_mutations.is_empty()
    }

    pub(crate) fn deferred_text_metadata(&self) -> Option<[u8; 28]> {
        self.deferred_text_meta
    }

    pub(crate) fn update_deferred_text_metadata(&mut self, metadata: [u8; 28]) {
        self.deferred_text_meta = Some(metadata);
    }

    pub(crate) fn set_text_memory_limit(&mut self, limit: usize) {
        self.text_memory_limit = limit;
    }

    fn check_text_memory_budget(&self, estimated_bytes: usize) -> Result<()> {
        let current_bytes = self.estimated_memory_bytes();
        if self.text_memory_limit > 0
            && current_bytes.saturating_add(estimated_bytes) > self.text_memory_limit
        {
            return Err(TriviumError::CapacityReservationRejected {
                requested_nodes: 0,
                estimated_bytes,
                current_bytes,
                memory_limit: self.text_memory_limit,
            });
        }
        Ok(())
    }

    /// 提交 WAL 前预留延期删除记录，避免事务应用期间再做可失败分配。
    pub(crate) fn reserve_text_deletes(&mut self, count: usize) -> Result<()> {
        if self.deferred_text_path.is_none() || count == 0 {
            return Ok(());
        }
        let len = self.pending_text_mutations.len();
        let capacity = self.pending_text_mutations.capacity();
        let required =
            len.checked_add(count)
                .ok_or_else(|| TriviumError::CapacityAllocationFailed {
                    reason: "延期文本操作数量溢出".into(),
                })?;
        if required <= capacity {
            return Ok(());
        }
        let target = required.max(capacity.saturating_mul(2)).max(4);
        let bytes = (target - capacity).saturating_mul(std::mem::size_of::<PendingTextMutation>());
        self.check_text_memory_budget(bytes)?;
        self.pending_text_mutations
            .try_reserve_exact(target - len)
            .map_err(|error| TriviumError::CapacityAllocationFailed {
                reason: format!("预留延期文本操作失败: {error}"),
            })
    }

    /// 在显式文本操作或发布待清理记录前加载，I/O 错误不丢弃待合并操作。
    pub(crate) fn prepare_text_index(&mut self) -> Result<()> {
        let Some(path) = self.deferred_text_path.as_deref() else {
            return Ok(());
        };
        let bytes = std::fs::metadata(format!("{path}.text"))?.len();
        // 解码前同时执行实例预算和共享运行时工作区准入。
        let estimate = usize::try_from(bytes)
            .ok()
            .and_then(|bytes| bytes.checked_mul(32))
            .and_then(|bytes| bytes.checked_add(64 * 1024))
            .ok_or_else(|| TriviumError::CapacityAllocationFailed {
                reason: "文本索引加载峰值估算溢出".into(),
            })?;
        self.check_text_memory_budget(estimate)?;
        let runtime = self.resource_runtime.clone();
        let _workspace = if runtime.is_maintenance_task() {
            runtime.reserve_maintenance_workspace(estimate)?
        } else {
            runtime.reserve_workspace(estimate)?
        };
        let mut index =
            crate::storage::file_format::read_deferred_text_index(path, self.deferred_text_meta)?;
        for mutation in &self.pending_text_mutations {
            match mutation {
                PendingTextMutation::Keyword(id, keyword) => index.add_keyword(*id, keyword),
                PendingTextMutation::Text(id, text) => index.add_text(*id, text),
                PendingTextMutation::Delete(id) => index.remove_node(*id),
            }
        }
        self.set_text_index(index);
        Ok(())
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
        self.checkpoint_dirty = true;
        self.deferred_text_path = None;
        self.deferred_text_meta = None;
        self.pending_text_mutations = Vec::new();
        self.pending_text_bytes = 0;
        self.text_index.clear();
        for (id, payload) in self.payloads.iter() {
            if let serde_json::Value::Object(map) = payload.as_ref() {
                let text = map
                    .values()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ");
                self.text_index.add_text(id, &text);
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
