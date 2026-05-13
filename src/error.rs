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

    /// 插入时节点 ID 已存在
    #[error("节点已存在 (Node already exists): {0}")]
    NodeAlreadyExists(u64),

    /// 数据库文件被其他进程锁定
    #[error("数据库已锁定 (Database locked): {0}")]
    DatabaseLocked(String),

    /// 数据库文件格式损坏或不兼容
    #[error("文件损坏 (Corrupted file): {0}")]
    CorruptedFile(String),

    /// 查询语法解析错误
    #[error("查询解析错误 (Query parse error): {0}")]
    QueryParse(String),

    /// 查询执行错误
    #[error("查询执行错误 (Query execution error): {0}")]
    QueryExecution(String),

    /// 外置 Hook 动态库加载失败
    #[error("Hook 加载失败 (Hook load error): {0}")]
    HookLoadError(String),

    /// WAL 写入器已关闭
    #[error("WAL 写入器已关闭，无法执行写操作 (WAL writer is closed)")]
    WalClosed,

    /// 输入参数无效（维度越界、非法配置等）
    #[error("无效输入 (Invalid input): {0}")]
    InvalidInput(String),

    #[error("数据库错误 (Database error): {0}")]
    Generic(String),
}

pub type Result<T> = std::result::Result<T, TriviumError>;
