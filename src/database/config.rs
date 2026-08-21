//! 数据库配置与查询参数矩阵
//!
//! 从 database.rs 中独立拆分，包含：
//! - `StorageMode`: 存储引擎选择（Mmap / Rom）
//! - `Config`: 数据库打开时的配置
//! - `SearchConfig`: 混合检索管线的参数矩阵

use crate::filter::Filter;
use crate::storage::wal::SyncMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageMode {
    /// Mmap 分离模式（默认）：高性能、海量数据，产生 `.tdb` 和 `.vec` 两个文件
    #[default]
    Mmap,
    /// Rom 单文件模式：高便携性，所有数据保存在一个 `.tdb` 文件中（纯内存加载以获得极高并发）
    Rom,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub dim: usize,
    pub sync_mode: SyncMode,
    pub storage_mode: StorageMode,
    /// 是否在首次查询/flush 时自动构建 QuIVer。批量导入可关闭以避免中途构建峰值。
    pub auto_build_quiver: bool,
    /// 是否在打开时加载持久化全文索引。纯向量数据库默认关闭以降低启动内存。
    pub load_text_index: bool,
    /// 预计总节点数，仅用于本次进程的核心容器容量预留，不持久化且不是硬上限。
    pub expected_nodes: Option<usize>,
    /// TriviumDB 内核内存预算（字节），0 表示不限制。
    pub memory_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dim: 1536,
            sync_mode: SyncMode::default(),
            storage_mode: StorageMode::default(),
            auto_build_quiver: true,
            load_text_index: false,
            expected_nodes: None,
            memory_limit: 0,
        }
    }
}

/// 基于外部参数化配置的查询配置参数矩阵
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// 最终返回数量。
    pub top_k: usize,
    /// 初始召回池大小；0 表示自动取 `max(top_k * 8, 64)`。
    pub recall_k: usize,
    /// SA-PPR/FISTA/DPP 前的重排池大小；0 表示自动取 `max(top_k * 4, 32)`。
    pub rerank_k: usize,
    pub expand_depth: usize,
    /// 图扩散允许的边标签；None 表示全部，Some(empty) 表示禁止扩散。
    pub expand_labels: Option<Vec<String>>,
    pub min_score: f32,
    pub teleport_alpha: f32, // L6 SA-PPR 个性化重启比例

    // 认知层统开开关 (当为 false 时，管线完全退化为最极简的传统检索引擎)
    pub enable_advanced_pipeline: bool,

    // L4 / L5: 残差与二次搜索
    pub enable_sparse_residual: bool,
    pub fista_lambda: f32,
    pub fista_threshold: f32,

    // L9: DPP
    pub enable_dpp: bool,
    pub dpp_quality_weight: f32,

    // --- 高级认知选项 (完全 Opt-in) ---
    /// 启用物理神经不应期（Fatigue），强制避免对高频节点的死循环访问，提供极强的长期多样性
    pub enable_refractory_fatigue: bool,
    /// 启用时，将使用 `1.0 / (1.0 + log10(in_degree))` 对泛化扩散节点施加反向惩罚
    pub enable_inverse_inhibition: bool,
    /// 当 > 0 时，作为侧向抑制起保护作用，自动截断扩散网络 (如传入 5000)
    pub lateral_inhibition_threshold: usize,
    /// 强制使用暴力搜索，禁用 QuIVer 图索引（用于基准测试和需要精确结果的场景）
    /// 当为 true 时，即使已构建 QuIVer 索引也不会使用
    pub force_brute_force: bool,

    // --- 混合倒排与文本检索 (Hybrid Search) ---
    /// 启用文本混合查询时，决定文本匹配的分数提权倍率 (Boost)
    pub text_boost: f32,
    /// 开启基于 AC 自动机的强制文本召回锚点机制 (等价于 PEDSA第一阶段)
    pub enable_text_hybrid_search: bool,
    pub bm25_k1: f32,
    pub bm25_b: f32,

    // --- Payload 预过滤 (向量召回阶段生效) ---
    /// 可选的 Payload 过滤条件，在向量搜索阶段即可跳过不符合条件的节点。
    /// 典型用途：多 Agent 隔离（按 agent_id 过滤）。
    pub payload_filter: Option<Filter>,

    // --- 上下文条件扩散 (Context-Conditioned Spreading Activation) ---
    /// 扩散方向偏置向量：当提供时，图扩散优先沿着与此向量语义相近的节点方向传播。
    ///
    /// 在 SA-PPR 扩散的每条边上施加 attention gate: `gate_j = σ(bias · v_j / √dim)`，
    /// 其中 `v_j` 为目标节点的 embedding。gate ∈ (0, 1) 调制能量传导强度。
    ///
    /// 不提供时退化为无上下文门控的 SA-PPR（向后兼容）。
    ///
    /// 典型用途：
    /// - 对话系统：传入 RNN 隐状态的投影向量，让扩散感知对话方向
    /// - RAG 应用：传入查询向量本身，让扩散偏向查询语义方向
    /// - 推荐系统：传入用户偏好向量，让扩散偏向用户兴趣方向
    pub diffusion_bias: Option<Vec<f32>>,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            top_k: 5,
            recall_k: 0,
            rerank_k: 0,
            expand_depth: 2,
            expand_labels: None,
            min_score: 0.1,
            teleport_alpha: 0.0,
            enable_advanced_pipeline: false,
            enable_sparse_residual: false,
            fista_lambda: 0.1,
            fista_threshold: 0.30,
            enable_dpp: false,
            dpp_quality_weight: 1.0,
            enable_refractory_fatigue: false,
            enable_inverse_inhibition: false,
            lateral_inhibition_threshold: 0,
            force_brute_force: false,
            text_boost: 1.5,
            enable_text_hybrid_search: false,
            bm25_k1: 1.2,
            bm25_b: 0.75,
            payload_filter: None,
            diffusion_bias: None,
        }
    }
}
