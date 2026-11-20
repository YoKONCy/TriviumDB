//! 运行策略、研究选项与构图常量。

use super::*;
/// 统一度量化的球面角度损失定点数尺度：0 表示 cos(θ)=1，16384 表示 cos(θ)=0，32768 表示 cos(θ)=-1。
pub(crate) const ANGULAR_SCALE: f64 = 16384.0;
/// 极坐标路由表的无量纲测地深度（sqrt(c) * R）分层数量。
pub(crate) const NUM_RADIAL_BANDS: usize = 8;
/// 每个（方向锥地标，测地深度层）网格单元保留的代表节点上限。
pub(crate) const POLAR_CELL_CAP: usize = 8;
/// 传统固定无量纲测地深度切分边界（保留供 A5/A6/A7 历史代际消融严格复现）。
pub(crate) const LEGACY_RADIAL_BAND_EDGES: [f32; NUM_RADIAL_BANDS - 1] =
    [0.25, 0.48, 0.75, 1.15, 1.75, 2.60, 3.80];
/// 由边界因子 f = cosh^2(sqrt(c)*R/2) 恢复无量纲测地半径 sqrt(c)*R，全程无除零风险。
#[inline]
pub(crate) fn dimensionless_depth(boundary_factor: f64) -> f64 {
    2.0 * (boundary_factor - 1.0).max(0.0).sqrt().asinh()
}
/// 将无量纲测地深度按给定切分边界映射至 [0, NUM_RADIAL_BANDS) 壳层索引。
#[inline]
pub(crate) fn radial_band_with_edges(depth: f32, edges: &[f32; NUM_RADIAL_BANDS - 1]) -> usize {
    let mut band = 0usize;
    while band < edges.len() && depth >= edges[band] {
        band += 1;
    }
    band
}
/// 计算传统 2-bit 离散向量 q ∈ {-2, -1, +1, +2}^D 的欧氏范数倒数 1 / sqrt(D + 3 * strong_ones)。
#[inline]
pub(crate) fn bq2_inv_qnorm(dim: usize, strong_ones: u32) -> f32 {
    1.0 / ((dim as f64 + 3.0 * f64::from(strong_ones)).sqrt() as f32)
}
/// 由传统 2-bit 距离与两端量化范数倒数计算固定系数 Hermite 校准球面余弦（保留供 A6 消融复现）。
#[inline]
pub(crate) fn calibrated_cosine_from_bq(
    bq_distance: u32,
    dim: usize,
    inv_qnorm_a: f32,
    inv_qnorm_b: f32,
) -> f64 {
    let dot = 4.0 * dim as f64 - f64::from(bq_distance);
    let cq = dot * f64::from(inv_qnorm_a) * f64::from(inv_qnorm_b);
    calibrate_bq2_cosine(cq)
}
/// 将球面余弦映射为满足 d(u, u) = 0 的定点角度损失 ∈ [0, 32768]。
#[inline]
pub(crate) fn cosine_to_angular_loss(cos_theta: f64) -> u32 {
    ((1.0 - cos_theta.clamp(-1.0, 1.0)) * ANGULAR_SCALE).round() as u32
}
/// 黎曼 QuIVer 正交消融与极坐标方向表示对照模式（仅在 research 特性下启用，默认 Full = A8）。
#[cfg(feature = "research")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HyperbolicStrategy {
    /// A8（默认）：第四代黎曼 QuIVer（自适应分位数壳层 + 凸支撑顶点选种 + Stage-0.5 非对称校准过滤器精筛 + 深度自适应罗盘）
    Full = 0,
    /// A0：原版双曲 Vamana 基线（单通道双曲剪枝、无保底补边、单阶段双曲搜索）
    GeodesicPruning = 1,
    /// A1：原版双曲剪枝 + 仅保底补边（按双曲距离补满 m0，单阶段双曲搜索）
    GeodesicPruningWithFill = 2,
    /// A2：第一代双纤维平行移动构图 + 纯单阶段精确双曲搜索（搜索禁用 Stage-0 门户与路由表）
    DualFiber = 3,
    /// A3：第一代双纤维构图 + Stage-0 切锥门户（禁用门户一跳测地展开与路由表）
    DualFiberPortals = 4,
    /// A1+Portal：原版双曲剪枝保底补边图 + 两阶段切锥门户一跳展开搜索
    GeodesicPortals = 5,
    /// 同算法（A8 拓扑）下采用 Polar-SQ8（8-bit 归一化方向量化）作为球面方向算子
    ScalarQuantizedDirection = 6,
    /// 同算法（A8 拓扑）下采用 Polar-FP32（32-bit 精确单位方向）作为球面方向算子
    FullPrecisionDirection = 7,
    /// A4：第一代完整黎曼 QuIVer（双纤维构图 + 未校准 BQ2 门户 + 一跳测地展开，无极坐标路由与顶点制导）
    DualFiberExpansion = 8,
    /// A5：第一代双纤维 + 极坐标锥-壳路由图谱与内向外测地主干构图（隔离 PolarRouter 的独立增益）
    PolarRouting = 9,
    /// A6：第二代黎曼 QuIVer（固定权重 Hermite 校准余弦 + 双遍 Stage-0 门户 + 四向罗盘剪枝）
    CalibratedCompass = 10,
    /// A7：第三代黎曼 QuIVer（对称 SM-Poincaré + 固定壳层顶点选种 + 单遍双保持束搜索 + 固定四向罗盘，供 A7 vs A8 对照）
    FixedShellCompass = 11,
}
/// 复用实际搜索入口的 V、C、F、R 候选损失诊断。
///
/// V 为已评分节点，C 为完整候选池，F 为精排预算选择集，R 为最终结果。
/// `R ⊆ F ⊆ C ⊆ V`，总损失分解为未触达、束保留、预算截断和精排四项。
#[cfg(feature = "research")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchLossStats {
    /// 最终返回结果命中真值数 |G_k ∩ R_k|
    pub hits_returned: usize,
    /// 全过程（Stage-0 + Stage-1）触达真值数 |G_k ∩ V|
    pub hits_visited: usize,
    /// 导航束保留候选池命中真值数 |G_k ∩ C|
    pub hits_in_beam: usize,
    /// 精排预算选择集 F 的真值命中数。
    pub hits_selected_for_rerank: usize,
    pub beam_candidates: usize,
    pub selected_for_rerank: usize,
    pub expanded_nodes: usize,
    pub expansion_calls: usize,
    pub approximate_evals: usize,
    pub bound_rejections: usize,
    /// 去重后总访问节点数 |V|
    pub visited_nodes: usize,
    /// 实际触发冷存原始向量精确距离计算的次数
    pub exact_vector_evals: usize,
    /// 束内头行（Cache Line 0）两级早筛检查次数
    pub header_checks: usize,
    /// 束内头行早筛直接拦截次数（免除后三条签名缓存行读取与 768D 位核计算）
    pub header_rejections: usize,
}
/// 无HashSet集合记录的单次查询阶段耗时与工作量，仅在显式阶段剖析时启用。
#[cfg(feature = "research")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStageProfile {
    /// 高层贪心入口下降耗时（纳秒）。
    pub upper_greedy_ns: u64,
    /// 查询编码 + 极坐标锥-壳选种 + 初始种子评估耗时（纳秒）
    pub router_seed_ns: u64,
    /// 主导航束（apex + horiz）图扩展与近似评分/头行早筛耗时（纳秒）
    pub beam_nav_ns: u64,
    /// Stage-0.5 非对称校准过滤器候选池重排耗时（纳秒）
    pub asym_gate_ns: u64,
    /// 重排读取、解码、径向过滤、堆操作及结果整理的整体耗时（纳秒）。
    pub exact_rerank_ns: u64,
    /// 候选池返回后的预算选择与自适应扩容耗时（纳秒）。
    pub candidate_select_ns: u64,
    /// 重排记录读取、寄存器解码与距离计算的累计耗时（纳秒）。
    pub rerank_score_ns: u64,
    pub rerank_selected_count: usize,
    pub reranked_count: usize,
    pub rerank_radial_rejections: usize,
    pub navigation_passes: usize,
    pub main_expansion_calls: usize,
    pub horizon_expansion_calls: usize,
    pub stage05_candidates: usize,
    pub ef_nav: usize,
    pub router_seeds: usize,
}
#[cfg(feature = "research")]
impl SearchLossStats {
    /// C 到 F 的预算截断损失。
    pub fn budget_truncation_loss(&self, k: usize) -> f64 {
        if k == 0 {
            return 0.0;
        }
        self.hits_in_beam
            .saturating_sub(self.hits_selected_for_rerank) as f64
            / k as f64
    }
    /// F 到 R 的精排损失，应在精确精排与一致真值下为零。
    pub fn exact_rerank_loss(&self, k: usize) -> f64 {
        if k == 0 {
            return 0.0;
        }
        self.hits_selected_for_rerank
            .saturating_sub(self.hits_returned) as f64
            / k as f64
    }
    /// 图路由与束扩展未触达损失：`|G_k \ V| / k`
    #[inline]
    pub fn reachability_loss(&self, k: usize) -> f64 {
        if k == 0 {
            return 0.0;
        }
        k.saturating_sub(self.hits_visited) as f64 / k as f64
    }

    /// 代理度量排序偏移导致的束保留淘汰损失：`|(G_k ∩ V) \ C| / k`
    #[inline]
    pub fn beam_retention_loss(&self, k: usize) -> f64 {
        if k == 0 {
            return 0.0;
        }
        self.hits_visited.saturating_sub(self.hits_in_beam) as f64 / k as f64
    }

    /// 候选池有界精排截断损失：`|(G_k ∩ C) \ R_k| / k`
    #[inline]
    pub fn rerank_truncation_loss(&self, k: usize) -> f64 {
        if k == 0 {
            return 0.0;
        }
        self.hits_in_beam.saturating_sub(self.hits_returned) as f64 / k as f64
    }
}
#[cfg(feature = "research")]
pub(crate) static HYPERBOLIC_STRATEGY: AtomicU8 = AtomicU8::new(0);
/// 正交模块开关。全局实验配置在构图及并发查询期间必须保持不变。
#[cfg(feature = "research")]
#[derive(Clone, Copy, Debug)]
pub struct GeometryComponents {
    pub asymmetric_gate: bool,
    pub adaptive_shells: bool,
    pub radial_bound: bool,
    pub connectivity_repair: bool,
    pub compass: bool,
}
#[cfg(feature = "research")]
impl Default for GeometryComponents {
    fn default() -> Self {
        Self {
            asymmetric_gate: true,
            adaptive_shells: true,
            radial_bound: true,
            connectivity_repair: true,
            compass: true,
        }
    }
}
#[cfg(feature = "research")]
pub(crate) static COMPONENTS: AtomicU8 = AtomicU8::new(31);
#[cfg(feature = "research")]
pub fn set_geometry_components(c: GeometryComponents) {
    COMPONENTS.store(
        c.asymmetric_gate as u8
            | (c.adaptive_shells as u8) << 1
            | (c.radial_bound as u8) << 2
            | (c.connectivity_repair as u8) << 3
            | (c.compass as u8) << 4,
        AtomicOrdering::Relaxed,
    );
}
#[cfg(feature = "research")]
pub(crate) fn geometry_components() -> GeometryComponents {
    let f = COMPONENTS.load(AtomicOrdering::Relaxed);
    GeometryComponents {
        asymmetric_gate: f & 1 != 0,
        adaptive_shells: f & 2 != 0,
        radial_bound: f & 4 != 0,
        connectivity_repair: f & 8 != 0,
        compass: f & 16 != 0,
    }
}
#[inline]
pub(crate) fn connectivity_repair_enabled() -> bool {
    #[cfg(feature = "research")]
    {
        geometry_components().connectivity_repair
    }
    #[cfg(not(feature = "research"))]
    {
        true
    }
}
/// 查询期遍历预算档位：用于分离无损系统/微架构加速（Standard）与搜索束过饱和去冗余（Balanced / Lean）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TraversalProfile {
    /// 与 v3 主表严格一致的默认遍历预算（seed_budget=4*ef, ef_nav=ef+ef/4, horiz_steps=24）
    Standard = 0,
    /// 温和去冗余档位（seed_budget=3*ef, ef_nav=ef+ef/6, horiz_steps=10）
    Balanced = 1,
    /// 紧凑高吞吐档位（seed_budget=2*ef+16, ef_nav=ef+ef/8, horiz_steps=4）
    Lean = 2,
}
impl TraversalProfile {
    #[inline]
    pub(crate) fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Balanced,
            2 => Self::Lean,
            _ => Self::Standard,
        }
    }
}
/// 查询热路径系统与微架构消融配置（在 research 下可逐项独立开关，生产构建默认全开 Standard）。
#[cfg(feature = "research")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionConfig {
    /// O0：消除非诊断查询期间的 TLS/RefCell 空借用开销并将原子配置提升至查询外层寄存器
    pub zero_trace_overhead: bool,
    /// O1：启用两级 Tammes 粗到细球面方向锥路由（64 粗地标 → Top-K 细锥短表，避免全扫 2048 锥）
    pub two_level_cones: bool,
    /// O2b：启用 LFB 纪律滚动窗口预取、同步 PolarNodeMeta 预取与单遍 visited 去重（消除种子 L1 自驱逐）
    pub rolling_prefetch: bool,
    /// O3：启用 768D 双端口/CSA 硬件位核、2KB 查表非对称核与查询半径/候选缓冲零冗余复用
    pub fast_kernels: bool,
    /// 对称位核编号，与缓冲及半径复用开关独立（0=AVX2，1=POPCNT-60，2=CSA-40，3=混合核）。
    pub kernel_variant: u8,
    /// O4：查询遍历预算档位
    pub traversal: TraversalProfile,
}
#[cfg(feature = "research")]
impl ExecutionConfig {
    /// 复现 v3 基线查询执行路径（关闭 O0、O1、O2b、O3，保留 Standard 遍历预算）。
    pub const fn reference() -> Self {
        Self {
            zero_trace_overhead: false,
            two_level_cones: false,
            rolling_prefetch: false,
            fast_kernels: false,
            kernel_variant: 0,
            traversal: TraversalProfile::Standard,
        }
    }

    /// 启用全部无损系统与微架构优化（O0 + O1 + O2b + O3），保持 Standard 遍历预算。
    pub const fn optimized(kernel_variant: u8) -> Self {
        Self {
            zero_trace_overhead: true,
            two_level_cones: true,
            rolling_prefetch: true,
            fast_kernels: true,
            kernel_variant,
            traversal: TraversalProfile::Standard,
        }
    }
}
#[cfg(feature = "research")]
impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            zero_trace_overhead: true,
            two_level_cones: true,
            rolling_prefetch: true,
            fast_kernels: true,
            kernel_variant: 0,
            traversal: TraversalProfile::Standard,
        }
    }
}
// 位编码：bit0=O0, bit1=O1, bit2=O2b, bit3=O3, bits[4..6]=kernel_variant(0..3), bits[6..8]=traversal(0..2)
// 默认值：0b00_00_1111 = 0x0F (O0=1, O1=1, O2b=1, O3=1, variant=0, traversal=0)
#[cfg(feature = "research")]
pub(crate) static EXECUTION_BITS: AtomicU16 = AtomicU16::new(0x000F);
#[cfg(feature = "research")]
pub fn set_execution_config(cfg: ExecutionConfig) {
    let variant = cfg.kernel_variant.min(3);
    crate::index::pelorus::engine::quantization::set_sm_fast_kernel_variant(variant);
    let bits = (cfg.zero_trace_overhead as u16)
        | ((cfg.two_level_cones as u16) << 1)
        | ((cfg.rolling_prefetch as u16) << 2)
        | ((cfg.fast_kernels as u16) << 3)
        | ((u16::from(variant) & 0b11) << 4)
        | ((cfg.traversal as u16 & 0b11) << 6);
    EXECUTION_BITS.store(bits, AtomicOrdering::Relaxed);
}
#[cfg(feature = "research")]
#[inline]
pub fn execution_config() -> ExecutionConfig {
    let bits = EXECUTION_BITS.load(AtomicOrdering::Relaxed);
    let fast_kernels = (bits & (1 << 3)) != 0;
    let raw_var = ((bits >> 4) & 0b11) as u8;
    ExecutionConfig {
        zero_trace_overhead: (bits & 1) != 0,
        two_level_cones: (bits & (1 << 1)) != 0,
        rolling_prefetch: (bits & (1 << 2)) != 0,
        fast_kernels,
        kernel_variant: raw_var,
        traversal: TraversalProfile::from_u8(((bits >> 6) & 0b11) as u8),
    }
}
/// 束内头行（Cache Line 0）两级早筛策略：仅依赖单页合表记录第 0 条 64B 缓存行即可拦截无望进入导航束的候选。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SketchFilter {
    /// 关闭头行早筛（用于单独隔离 Opt 1 单页合表消除二次 DTLB 的纯无损加速比）。
    #[default]
    Off,
    /// 纯无损径向与视界壳层下界早筛：开启头行高并发滚动预取且结果 CRC32 与基线 100% 等价。
    LosslessRadial,
    /// 对称 256-bit 跨步符号草图早筛（4×XOR+POPCNT + 1KB 静态余弦表，零线程私有表开销）。
    SymSketch256 { margin: f32 },
    /// 非对称 u_q × 256-bit 跨步符号草图早筛（2KB L1d i16 半字节查表，残差标准差更低）。
    AsymSketch256 { margin: f32 },
}
/// 热路径访存合表（Opt 1）与束内头行两级早筛（Opt 2）运行期消融配置。
#[cfg(feature = "research")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StorageConfig {
    /// Opt 1：启用 4KiB 单页对齐的 256B 合表热节点记录（`HotSmStore`，消除跨数组二次 DTLB 与跨页断裂）。
    pub colocated_store: bool,
    /// Opt 2：束内头行两级早筛模式（仅当 `colocated_store = true` 时生效）。
    pub early_gate: SketchFilter,
    /// 头行（单缓存行 = 1 LFB）滚动预取前瞻深度（默认 8，利用单行低 LFB 占用提升随机访存并发度 MLP）。
    pub header_prefetch_window: u8,
    /// 在 visited 去重循环内提前发射首批热节点预取并交叠预取下一跳 L0 邻接表（无损访存重叠）。
    pub eager_prefetch_overlap: bool,
}
#[cfg(feature = "research")]
impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            colocated_store: false,
            early_gate: SketchFilter::Off,
            header_prefetch_window: 8,
            eager_prefetch_overlap: false,
        }
    }
}
#[cfg(feature = "research")]
pub(crate) static STORAGE_BITS: AtomicU64 = AtomicU64::new(8u64 << 8);
#[cfg(feature = "research")]
pub fn set_storage_config(cfg: StorageConfig) {
    let (kind, margin_bits): (u64, u32) = match cfg.early_gate {
        SketchFilter::Off => (0, 0),
        SketchFilter::LosslessRadial => (1, 0),
        SketchFilter::SymSketch256 { margin } => (2, margin.to_bits()),
        SketchFilter::AsymSketch256 { margin } => (3, margin.to_bits()),
    };
    let pf = u64::from(cfg.header_prefetch_window.clamp(1, 32));
    let packed = u64::from(cfg.colocated_store)
        | (kind << 1)
        | (u64::from(cfg.eager_prefetch_overlap) << 4)
        | (pf << 8)
        | (u64::from(margin_bits) << 32);
    STORAGE_BITS.store(packed, AtomicOrdering::Relaxed);
}
#[cfg(feature = "research")]
#[inline]
pub fn storage_config() -> StorageConfig {
    let packed = STORAGE_BITS.load(AtomicOrdering::Relaxed);
    let colocated_store = (packed & 1) != 0;
    let kind = (packed >> 1) & 0x7;
    let eager_prefetch_overlap = ((packed >> 4) & 1) != 0;
    let pf = ((packed >> 8) & 0xFF) as u8;
    let margin = f32::from_bits((packed >> 32) as u32);
    let early_gate = match kind {
        1 => SketchFilter::LosslessRadial,
        2 => SketchFilter::SymSketch256 { margin },
        3 => SketchFilter::AsymSketch256 { margin },
        _ => SketchFilter::Off,
    };
    StorageConfig {
        colocated_store,
        early_gate,
        header_prefetch_window: if pf == 0 { 8 } else { pf },
        eager_prefetch_overlap,
    }
}
/// 无有效切锥残差轴分配时的哨兵编号（占 12-bit 全 1）。
pub(crate) const NO_CONE_ID: u16 = 0x0FFF;
/// `PolarNodeMeta::packed_strong_cone` 低 20 位掩码（支持最高 1,048,575 维）。
pub(crate) const STRONG_ONES_MASK: u32 = 0x000F_FFFF;
/// 球面切锥正交残差字典的最大主轴数量。
pub(crate) const MAX_RESIDUAL_CONES: usize = 512;
/// 极端分布（高深聚簇流形与高维窄壳流形）召回救援与信噪比增强配置。
#[cfg(feature = "research")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecallConfig {
    /// 方案 B：启用球面切锥正交残差 2-bit SM 编码（消除高深聚簇流形共形放大下的同簇量化信噪比坍塌）。
    pub tangent_residual_sm: bool,
    /// 方案 A：启用代理距离平台感知自适应精排预算（消除平坦代理分布下的预算截断与大 ef 拥挤退化）。
    pub adaptive_rerank: bool,
    /// 触发平台自适应扩容的相对代理跨度阈值 `(p_{B-1} - p_{k_ref}) / max(p_{k_ref}, 1e-9)`（默认 0.30）。
    pub plateau_rel_gap: f32,
    /// 自适应精排扩容硬上限（默认 512）。
    pub adaptive_rerank_cap: u16,
    /// 方案 C：启用 256-bit 符号草图二项方差稳定化裕量（消除近正交高维均匀流形上的头行早筛过剪枝）。
    pub variance_stabilized_sketch: bool,
}
#[cfg(feature = "research")]
impl RecallConfig {
    /// 同时启用全套召回救援原语（方案 B 切锥正交残差 + 方案 A 平台自适应精排 + 方案 C 方差稳定草图与各向同性保护）。
    pub const fn adaptive() -> Self {
        Self {
            tangent_residual_sm: true,
            adaptive_rerank: true,
            plateau_rel_gap: 0.22,
            adaptive_rerank_cap: 768,
            variance_stabilized_sketch: true,
        }
    }
}
#[cfg(feature = "research")]
impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            tangent_residual_sm: false,
            adaptive_rerank: false,
            plateau_rel_gap: 0.22,
            adaptive_rerank_cap: 768,
            variance_stabilized_sketch: false,
        }
    }
}
// 默认值：开关全关，cap=768，plateau_rel_gap=0.22f32 (0x3E6147AE)
#[cfg(feature = "research")]
pub(crate) static RECALL_BITS: AtomicU64 = AtomicU64::new((768u64 << 16) | (0x3E61_47AEu64 << 32));
#[cfg(feature = "research")]
pub fn set_recall_config(cfg: RecallConfig) {
    let gap_bits = cfg.plateau_rel_gap.clamp(0.0, 4.0).to_bits();
    let cap = u64::from(cfg.adaptive_rerank_cap.max(16));
    let packed = u64::from(cfg.tangent_residual_sm)
        | (u64::from(cfg.adaptive_rerank) << 1)
        | (u64::from(cfg.variance_stabilized_sketch) << 2)
        | (cap << 16)
        | (u64::from(gap_bits) << 32);
    RECALL_BITS.store(packed, AtomicOrdering::Relaxed);
}
#[cfg(feature = "research")]
#[inline]
pub fn recall_config() -> RecallConfig {
    let packed = RECALL_BITS.load(AtomicOrdering::Relaxed);
    let cap = ((packed >> 16) & 0xFFFF) as u16;
    let gap = f32::from_bits((packed >> 32) as u32);
    RecallConfig {
        tangent_residual_sm: (packed & 1) != 0,
        adaptive_rerank: ((packed >> 1) & 1) != 0,
        variance_stabilized_sketch: ((packed >> 2) & 1) != 0,
        adaptive_rerank_cap: if cap == 0 { 512 } else { cap },
        plateau_rel_gap: if gap.is_finite() && gap >= 0.0 {
            gap
        } else {
            0.30
        },
    }
}
#[inline]
pub(crate) fn tangent_residual_enabled() -> bool {
    #[cfg(feature = "research")]
    {
        recall_config().tangent_residual_sm
            && matches!(hyperbolic_strategy(), HyperbolicStrategy::Full)
    }
    #[cfg(not(feature = "research"))]
    {
        false
    }
}
/// 在当前硬件上对 768D 对称 SM 5-popcount 四种位核变体执行微基准测量，返回 `(variant_id, 名称, 单次纳秒)`。
#[cfg(feature = "research")]
pub fn benchmark_sign_magnitude_kernels(iters: usize) -> [(u8, &'static str, f64); 4] {
    benchmark_sign_magnitude_workload(64, iters, false, 12345)
}
/// 使用明确的工作集和访问顺序测量位核，同时在计时前核对各位核的整数计数。
#[cfg(feature = "research")]
pub fn benchmark_sign_magnitude_workload(
    n: usize,
    iters: usize,
    random: bool,
    seed: u64,
) -> [(u8, &'static str, f64); 4] {
    assert!(n > 0, "工作集节点数必须大于零");
    let prev = crate::index::pelorus::engine::quantization::sm_fast_kernel_variant();
    let dim = 768usize;
    let mut store = Bq2Store::new(dim);
    store.reserve(n);
    let mut vector = vec![0.0; dim];
    let mut state = seed;
    for _ in 0..n {
        for value in &mut vector {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            *value = ((state >> 40) as i32 - (1 << 23)) as f32 / (1 << 23) as f32;
        }
        store.push_from_vector(&vector);
    }
    let q_sig = Bq2Signature::from_vector(&vector);
    let rounds = iters.max(10_000);
    let order: Vec<usize> = (0..rounds)
        .map(|i| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            if random {
                (state >> 16) as usize % n
            } else {
                i % n
            }
        })
        .collect();
    crate::index::pelorus::engine::quantization::set_sm_fast_kernel_variant(0);
    let reference: Vec<_> = (0..n.min(1024))
        .map(|i| store.sm_counts_to_sig(i, &q_sig, dim))
        .collect();
    let variants: [(u8, &'static str); 4] = [
        (0, "AVX2_VPSHUFB_CSA(v3)"),
        (1, "POPCNT_60_Unrolled"),
        (2, "POPCNT_40_CSA"),
        (3, "Hybrid_Port1_POPCNT_Port5_AVX2"),
    ];
    let mut out = [(0u8, "", 0.0f64); 4];
    for (slot, &(vid, name)) in variants.iter().enumerate() {
        crate::index::pelorus::engine::quantization::set_sm_fast_kernel_variant(vid);
        for (i, expected) in reference.iter().enumerate() {
            assert_eq!(
                &store.sm_counts_to_sig(i, &q_sig, dim),
                expected,
                "位核整数计数不一致"
            );
        }
        let mut sink = 0u32;
        for r in 0..2048 {
            let c = store.sm_counts_to_sig(order[r % rounds], &q_sig, dim);
            sink = sink.wrapping_add(c[0] ^ c[4]);
        }
        std::hint::black_box(sink);
        let t0 = std::time::Instant::now();
        for &node in &order {
            let c = store.sm_counts_to_sig(node, &q_sig, dim);
            sink = sink
                .wrapping_add(c[0])
                .wrapping_add(c[1])
                .wrapping_add(c[4]);
        }
        let elapsed_ns = t0.elapsed().as_secs_f64() * 1e9 / (rounds as f64);
        std::hint::black_box(sink);
        out[slot] = (vid, name, elapsed_ns);
    }
    crate::index::pelorus::engine::quantization::set_sm_fast_kernel_variant(prev);
    out
}
/// 设置当前消融实验模式（仅用于 research 基准实验）。
#[cfg(feature = "research")]
pub fn set_hyperbolic_strategy(mode: HyperbolicStrategy) {
    HYPERBOLIC_STRATEGY.store(mode as u8, AtomicOrdering::Relaxed);
}
/// 读取当前消融实验模式。
#[cfg(feature = "research")]
#[inline]
pub fn hyperbolic_strategy() -> HyperbolicStrategy {
    match HYPERBOLIC_STRATEGY.load(AtomicOrdering::Relaxed) {
        1 => HyperbolicStrategy::GeodesicPruning,
        2 => HyperbolicStrategy::GeodesicPruningWithFill,
        3 => HyperbolicStrategy::DualFiber,
        4 => HyperbolicStrategy::DualFiberPortals,
        5 => HyperbolicStrategy::GeodesicPortals,
        6 => HyperbolicStrategy::ScalarQuantizedDirection,
        7 => HyperbolicStrategy::FullPrecisionDirection,
        8 => HyperbolicStrategy::DualFiberExpansion,
        9 => HyperbolicStrategy::PolarRouting,
        10 => HyperbolicStrategy::CalibratedCompass,
        11 => HyperbolicStrategy::FixedShellCompass,
        _ => HyperbolicStrategy::Full,
    }
}
#[cfg(feature = "research")]
pub(crate) fn directional_cosine_slices(a: &[f32], b: &[f32], mode: HyperbolicStrategy) -> f64 {
    match mode {
        HyperbolicStrategy::FullPrecisionDirection => {
            let mut dot = 0.0f64;
            let mut na = 0.0f64;
            let mut nb = 0.0f64;
            for (&x, &y) in a.iter().zip(b.iter()) {
                let xf = f64::from(x);
                let yf = f64::from(y);
                dot += xf * yf;
                na += xf * xf;
                nb += yf * yf;
            }
            if na <= 0.0 || nb <= 0.0 {
                return 0.0;
            }
            (dot / (na.sqrt() * nb.sqrt())).clamp(-1.0, 1.0)
        }
        HyperbolicStrategy::ScalarQuantizedDirection => {
            let max_a = a.iter().fold(0.0f64, |m, &v| m.max(f64::from(v).abs()));
            let max_b = b.iter().fold(0.0f64, |m, &v| m.max(f64::from(v).abs()));
            if max_a <= 0.0 || max_b <= 0.0 {
                return 0.0;
            }
            let sa = 127.0 / max_a;
            let sb = 127.0 / max_b;
            let mut dot = 0i64;
            let mut na = 0i64;
            let mut nb = 0i64;
            for (&x, &y) in a.iter().zip(b.iter()) {
                let qa = (f64::from(x) * sa).round().clamp(-127.0, 127.0) as i64;
                let qb = (f64::from(y) * sb).round().clamp(-127.0, 127.0) as i64;
                dot += qa * qb;
                na += qa * qa;
                nb += qb * qb;
            }
            if na <= 0 || nb <= 0 {
                return 0.0;
            }
            (dot as f64 / ((na as f64).sqrt() * (nb as f64).sqrt())).clamp(-1.0, 1.0)
        }
        _ => unreachable!(),
    }
}
#[cfg(feature = "research")]
#[inline]
pub(crate) fn directional_distance_slices(a: &[f32], b: &[f32], mode: HyperbolicStrategy) -> u32 {
    cosine_to_angular_loss(directional_cosine_slices(a, b, mode))
}
