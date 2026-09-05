//! TriviumDB 核心的统一结构化错误模型。
//!
//! 错误变体承载版本、预算、访问模式和迁移等可判定字段；Python/Node 绑定据此映射稳定
//! 异常或错误码。控制流不得匹配中英文消息文本，面向用户的消息则保持双语。

use thiserror::Error;

#[derive(Error, Debug)]
pub enum TriviumError {
    #[error("I/O 错误 (I/O error): {0}")]
    Io(#[from] std::io::Error),

    #[error("序列化错误 (Serialization error): {0}")]
    Serialization(#[from] bincode::Error),

    #[error("向量维度不匹配 (Vector dimension mismatch): 期望 {expected}，实际 {got}")]
    DimensionMismatch { expected: usize, got: usize },

    #[error("节点不存在 (Node not found): {0}")]
    NodeNotFound(u64),

    /// 向量数据包含非法浮点值（NaN 或 Infinity）
    #[error("非法向量 (Invalid vector): {reason}")]
    InvalidVector { reason: String },

    /// Payload 大小超过允许上限
    #[error("Payload 过大 (Payload too large): {size_bytes} 字节，上限 {max_bytes} 字节")]
    PayloadTooLarge { size_bytes: usize, max_bytes: usize },

    /// 唯一属性约束被另一个节点占用。
    #[error(
        "唯一约束冲突 (Unique constraint violation): 字段 {fields:?} 的键已被节点 {existing_id} 占用，冲突节点 {conflicting_id}"
    )]
    UniqueConstraintViolation {
        fields: Vec<String>,
        existing_id: u64,
        conflicting_id: u64,
    },

    /// 条件更新的前置谓词不成立。
    #[error("条件更新未匹配 (Conditional update did not match): 节点 {id}")]
    ConditionalUpdateNotMatched { id: u64 },

    /// 插入时节点 ID 已存在
    #[error("节点已存在 (Node already exists): {0}")]
    NodeAlreadyExists(u64),

    /// 数据库文件被其他进程锁定
    #[error("数据库已锁定 (Database locked): {0}")]
    DatabaseLocked(String),

    #[error("只读数据库不允许执行操作 (Read-only database operation denied): {operation}")]
    ReadOnlyViolation { operation: &'static str },

    #[error("数据库需要由可写句柄先完成 WAL 恢复 (Database recovery required): {wal_path}")]
    RecoveryRequired { wal_path: String },

    #[error("不可变 generation 不完整或校验失败 (Immutable generation invalid): {reason}")]
    ImmutableArtifactInvalid { reason: String },

    #[error("generation 仍被 Reader 使用 (Generation is busy): {generation_id}")]
    GenerationBusy { generation_id: String },

    /// 数据库文件格式损坏或不兼容
    #[error("文件损坏 (Corrupted file): {0}")]
    CorruptedFile(String),

    /// 查询语法解析错误
    #[error("查询解析错误 (Query parse error): {0}")]
    QueryParse(String),

    /// 查询执行错误
    #[error("查询执行错误 (Query execution error): {0}")]
    QueryExecution(String),

    /// 查询因客户端取消或请求 deadline 到期而协作式终止。
    #[error("查询已取消 (Query cancelled)")]
    QueryCancelled,

    /// 查询需要物化的行数超过安全预算；禁止以静默截断伪装成完整结果。
    #[error(
        "查询行预算耗尽 (Query row budget exceeded): 最多允许处理 {budget} 行；请使用 LIMIT/OFFSET 分页并确保 OFFSET + LIMIT 不超过预算"
    )]
    QueryRowBudgetExceeded { budget: usize },

    /// 查询访问或解析 Payload 的工作量超过显式预算。
    #[error(
        "Payload 查询预算耗尽 (Payload query budget exceeded): {dimension}，已使用 {used}，上限 {budget}"
    )]
    PayloadQueryBudgetExceeded {
        dimension: &'static str,
        used: u64,
        budget: u64,
    },

    #[error(
        "图遍历预算耗尽 (Graph traversal budget exceeded): {dimension:?}，visited={visited_nodes}，edges={examined_edges}，frontier={peak_frontier_size}，depth={depth_reached}"
    )]
    TraversalBudgetExceeded {
        dimension: crate::graph::budget::BudgetDimension,
        visited_nodes: usize,
        examined_edges: usize,
        peak_frontier_size: usize,
        depth_reached: usize,
    },

    /// Hook 回调执行失败；动态语言异常和 FFI 非零错误码统一映射到此变体。
    #[error("Hook 执行失败 (Hook execution error): {0}")]
    HookExecutionError(String),

    /// 外置 Hook 动态库加载失败
    #[error("Hook 加载失败 (Hook load error): {0}")]
    HookLoadError(String),

    /// WAL 写入器已关闭
    #[error("WAL 写入器已关闭，无法执行写操作 (WAL writer is closed)")]
    WalClosed,

    /// 容量预留会突破显式内存预算，未执行任何逻辑数据修改
    #[error(
        "容量预留被拒绝 (Capacity reservation rejected): 请求新增 {requested_nodes} 个节点，预计新增 {estimated_bytes} 字节，当前估算 {current_bytes} 字节，内存上限 {memory_limit} 字节"
    )]
    CapacityReservationRejected {
        requested_nodes: usize,
        estimated_bytes: usize,
        current_bytes: usize,
        memory_limit: usize,
    },

    /// 容量计算溢出或底层分配器拒绝预留
    #[error("容量预留失败 (Capacity allocation failed): {reason}")]
    CapacityAllocationFailed { reason: String },

    /// 数据库已经关闭或正在关闭。
    #[error("数据库已关闭 (Database closed)")]
    DatabaseClosed,

    /// WAL 格式版本高于当前内核支持范围。
    #[error("不支持的 WAL 版本 (Unsupported WAL version): 发现 {found}，当前支持 {supported}")]
    UnsupportedWalVersion { found: u16, supported: u16 },

    #[error(
        "不支持的数据库文件版本 (Unsupported database file version): 发现 v{found}，当前可读取 v{minimum_supported}..=v{current}；v5 对应 TriviumDB 0.7.x/0.8.x，早于 0.7.0 的文件请手动导出迁移"
    )]
    UnsupportedDatabaseVersion {
        found: u16,
        minimum_supported: u16,
        current: u16,
    },

    #[error(
        "API 已移除 (API removed): {removed_api}；请改用 {replacement} (use {replacement} instead)"
    )]
    ApiMigrationRequired {
        removed_api: &'static str,
        replacement: &'static str,
    },

    #[error(
        "不支持的 sidecar 版本 (Unsupported sidecar version): {component} 发现 v{found}，当前支持 v{minimum_supported}..=v{current}；{action}"
    )]
    UnsupportedSidecarVersion {
        component: &'static str,
        found: u32,
        minimum_supported: u32,
        current: u32,
        action: &'static str,
    },

    /// 输入参数无效（维度越界、非法配置等）
    #[error("无效输入 (Invalid input): {0}")]
    InvalidInput(String),

    #[error("数据库错误 (Database error): {0}")]
    Generic(String),
}

pub type Result<T> = std::result::Result<T, TriviumError>;
