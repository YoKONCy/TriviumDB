//! TriviumDB 核心的统一结构化错误模型。
//!
//! 错误变体承载版本、预算、访问模式和迁移等可判定字段；Python/Node 绑定据此映射稳定
//! 异常或错误码。控制流不得匹配中英文消息文本，面向用户的消息则保持双语。

use thiserror::Error;

/// 各 SDK 与传输适配器共享的错误字段；消息用于诊断，code 与 details 用于程序判断。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ErrorInfo {
    pub schema_version: u32,
    pub code: &'static str,
    pub message: String,
    pub details: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<QueryErrorStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<ErrorTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write: Option<WriteErrorInfo>,
}

pub const ERROR_SCHEMA_VERSION: u32 = 1;

/// 写请求失败时由实际执行点提供的结果事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct WriteErrorInfo {
    pub phase: WritePhase,
    pub outcome: WriteOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_consumed: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WritePhase {
    Admission,
    Validation,
    Wal,
    Apply,
    Publish,
    GroupSync,
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOutcome {
    NotCommitted,
    Unknown,
    Committed,
}

impl WriteErrorInfo {
    pub const TRANSACTION_ADMISSION: Self = Self {
        phase: WritePhase::Admission,
        outcome: WriteOutcome::NotCommitted,
        transaction_consumed: Some(false),
    };

    pub(crate) const MAINTENANCE_ADMISSION: Self = Self {
        transaction_consumed: None,
        ..Self::TRANSACTION_ADMISSION
    };

    #[cfg(any(feature = "python", feature = "nodejs"))]
    pub(crate) const MAINTENANCE_VALIDATION: Self = Self {
        phase: WritePhase::Validation,
        ..Self::MAINTENANCE_ADMISSION
    };

    pub(crate) fn capture<R>(operation: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        let mut write = Self::MAINTENANCE_ADMISSION;
        operation(&mut write).map_err(|error| error.with_write_info(write))
    }

    /// 独立目标操作保留源库错误链，最外层结果描述目标发布。
    pub(crate) fn capture_destination<R>(
        operation: impl FnOnce(&mut Self) -> Result<R>,
    ) -> Result<R> {
        let mut write = Self::MAINTENANCE_ADMISSION;
        operation(&mut write).map_err(|source| TriviumError::WriteFailure {
            source: Box::new(source),
            write,
        })
    }
}

/// TQL 各处理阶段在实际失败检查点提供的分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryErrorStage {
    Lex,
    Parse,
    Bind,
    Plan,
    Execute,
}

impl QueryErrorStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lex => "lex",
            Self::Parse => "parse",
            Self::Bind => "bind",
            Self::Plan => "plan",
            Self::Execute => "execute",
        }
    }
}

/// 原始查询的 UTF-8 半开字节范围；输入末尾可使用零长度锚点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct QuerySpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: usize,
    pub column: usize,
}

struct TextCoordinates {
    line: usize,
    column: usize,
    previous_cr: bool,
}

impl TextCoordinates {
    fn new() -> Self {
        Self {
            line: 1,
            column: 1,
            previous_cr: false,
        }
    }

    fn advance(&mut self, ch: char) {
        match ch {
            '\r' => {
                self.line = self.line.saturating_add(1);
                self.column = 1;
            }
            '\n' if self.previous_cr => {}
            '\n' => {
                self.line = self.line.saturating_add(1);
                self.column = 1;
            }
            _ => self.column = self.column.saturating_add(1),
        }
        self.previous_cr = ch == '\r';
    }

    fn span(&self, byte: usize) -> QuerySpan {
        QuerySpan {
            start_byte: byte,
            end_byte: byte,
            line: self.line,
            column: self.column,
        }
    }
}

impl QuerySpan {
    /// 范围两端必须落在真实字符边界上，起点的行列沿用统一计数规则。
    pub fn range(input: &str, start: usize, end: usize) -> Option<Self> {
        input.get(start..end)?;
        let mut span = Self::at(input, start)?;
        span.end_byte = end;
        Some(span)
    }

    /// 只接受真实字符边界；列按 Unicode 标量计数，CRLF 合并，Tab 计一列。
    pub fn at(input: &str, byte: usize) -> Option<Self> {
        let prefix = input.get(..byte)?;
        let mut coordinates = TextCoordinates::new();
        for ch in prefix.chars() {
            coordinates.advance(ch);
        }
        Some(coordinates.span(byte))
    }

    /// 对排序后的原文边界一次扫描，批量证据沿用相同行列与CRLF规则。
    pub(crate) fn at_many(input: &str, positions: &[usize]) -> Result<Vec<Self>> {
        let mut result = Vec::new();
        result.try_reserve_exact(positions.len()).map_err(|error| {
            TriviumError::CapacityAllocationFailed {
                reason: format!(
                    "原文位置工作区分配失败 (Source position workspace allocation failed): {error}"
                ),
            }
        })?;
        let mut coordinates = TextCoordinates::new();
        let mut characters = input.char_indices().peekable();
        let mut previous = 0usize;
        let mut visited = 0usize;
        for &byte in positions {
            crate::request::check_current()?;
            if byte < previous || byte > input.len() || !input.is_char_boundary(byte) {
                return Err(TriviumError::InvalidInput("原文边界须排序且位于UTF-8字符边界 (Source boundaries must be sorted UTF-8 character boundaries)".into()).at_field("reference"));
            }
            while characters.peek().is_some_and(|(offset, _)| *offset < byte) {
                if let Some((_, ch)) = characters.next() {
                    coordinates.advance(ch);
                }
                visited = visited.saturating_add(1);
                if visited.is_multiple_of(1024) {
                    crate::request::check_current()?;
                }
            }
            result.push(coordinates.span(byte));
            previous = byte;
        }
        Ok(result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub enum ErrorTarget {
    Query {
        query_span: QuerySpan,
    },
    Parameter {
        parameter: String,
    },
    ParameterReference {
        parameter: String,
        query_span: QuerySpan,
    },
    Field {
        field: String,
    },
    FieldReference {
        field: String,
        query_span: QuerySpan,
    },
}

impl ErrorTarget {
    fn source_span(&self) -> Option<QuerySpan> {
        match self {
            Self::Query { query_span }
            | Self::ParameterReference { query_span, .. }
            | Self::FieldReference { query_span, .. } => Some(*query_span),
            _ => None,
        }
    }
}

#[derive(Error, Debug)]
pub enum TriviumError {
    /// 查询的实际失败节点提供阶段与来源，保留内部更精确的诊断。
    #[error("{source}")]
    QuerySource {
        stage: QueryErrorStage,
        span: Option<QuerySpan>,
        #[source]
        source: Box<TriviumError>,
    },
    /// Prepared 参数引用的源码范围保留原始分类、详情与错误链。
    #[error("{source}")]
    QueryParameterSource {
        parameter: String,
        query_span: QuerySpan,
        #[source]
        source: Box<TriviumError>,
    },
    /// 普通 API 字段定位保留原始错误类别、详情与 cause。
    #[error("{source}")]
    ApiField {
        field: String,
        #[source]
        source: Box<TriviumError>,
    },
    /// 写入阶段上下文保留原始错误类别及 cause。
    #[error("{source}")]
    WriteFailure {
        #[source]
        source: Box<TriviumError>,
        write: WriteErrorInfo,
    },
    /// 为参数转换或绑定错误保留参数身份及原始错误。
    #[error("{source}")]
    QueryParameter {
        parameter: String,
        #[source]
        source: Box<TriviumError>,
    },
    #[error("I/O 错误 (I/O error): {0}")]
    Io(#[from] std::io::Error),

    #[error("序列化错误 (Serialization error): {0}")]
    Serialization(#[from] bincode::Error),

    #[error("向量维度不匹配 (Vector dimension mismatch): 期望 {expected}，实际 {got}")]
    DimensionMismatch { expected: usize, got: usize },

    #[error("向量类型不匹配 (Vector dtype mismatch): stored={stored}, requested={requested}")]
    DtypeMismatch {
        stored: &'static str,
        requested: &'static str,
    },

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

    /// 请求尚未进入实例，调用方可在持有者释放后重新提交。
    #[error("数据库请求繁忙 (Database request is busy): {resource}")]
    DatabaseBusy { resource: &'static str },

    /// 在当前线程持锁期间重复进入同一实例，拒绝请求以避免自锁。
    #[error("数据库重入被拒绝 (Database reentrant access rejected): {operation}")]
    ReentrantAccess { operation: &'static str },

    /// 宿主锁内发生异常展开，交由宿主结束该实例并检查恢复结果。
    #[error("数据库宿主锁已中毒 (Database host lock is poisoned): {resource}")]
    HostLockPoisoned { resource: &'static str },

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

    /// 词法与语法入口保留结构化阶段；只有可靠定位时携带源码锚点。
    #[error("查询解析错误 (Query parse error): {reason}")]
    QuerySyntax {
        reason: String,
        stage: QueryErrorStage,
        span: Option<QuerySpan>,
    },

    /// 查询执行错误
    #[error("查询执行错误 (Query execution error): {0}")]
    QueryExecution(String),

    #[error("Prepared TQL 缺少参数 ${parameter} (Prepared TQL is missing parameter ${parameter})")]
    MissingQueryParameter { parameter: String },

    #[error(
        "Prepared TQL 收到额外参数 ${parameter} (Prepared TQL received unexpected parameter ${parameter})"
    )]
    UnexpectedQueryParameter { parameter: String },

    /// 查询在协作检查点观察到主动取消。
    #[error("查询已取消 (Query cancelled)")]
    QueryCancelled,

    /// 查询在协作检查点观察到请求期限已到。
    #[error("查询执行期限已到 (Query deadline exceeded)")]
    QueryDeadlineExceeded,

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

    /// 共享运行时拒绝内存、任务或工作线程准入；调用方可根据维度决定重试策略。
    #[error(
        "共享资源预算耗尽 (Resource budget exceeded): {resource}，请求 {requested}，已使用 {used}，上限 {limit}"
    )]
    ResourceBudgetExceeded {
        resource: &'static str,
        requested: usize,
        used: usize,
        limit: usize,
    },

    #[error("快照目标已经存在 (Snapshot destination exists): {path}")]
    SnapshotDestinationExists { path: String },

    #[error("快照已重命名但目录同步未能确认 (Snapshot publication uncertain): {path}，{reason}")]
    SnapshotPublicationUncertain { path: String, reason: String },

    /// 关闭工作任务未能正常完成，数据和锁仍由关闭状态持有。
    #[error("关闭任务失败 (Shutdown failed): {reason}")]
    ShutdownFailed { reason: String },

    /// 数据库已经关闭或正在关闭。
    #[error("数据库已关闭 (Database closed)")]
    DatabaseClosed,

    /// SDK 事务构造器已消费或回滚，结束状态不允许再次提交或添加操作。
    #[error("事务已结束，请创建新事务 (Transaction finished; create a new transaction)")]
    TransactionFinished,

    #[error("控制台会话已卸载 (Console session is detached)")]
    ConsoleSessionClosed,

    #[error(
        "控制台响应超出大小上限 (Console response exceeds size limit): max_bytes={max_bytes}, committed={committed}"
    )]
    ConsoleResponseTooLarge { max_bytes: usize, committed: bool },

    #[error("编辑版本冲突，请重新读取并合并 (Edit revision conflict; reload and merge)")]
    WriteConflict { expected: String, actual: String },

    #[error("图修订冲突，请重新读取并合并 (Graph revision conflict, reload and merge)")]
    GraphRevisionMismatch {
        graph_id: String,
        expected: String,
        actual: String,
    },

    #[error("图身份不存在 (Graph identity does not exist): {graph_id}")]
    GraphNotFound { graph_id: String },

    #[error("文档上传不存在 (Document upload does not exist): {upload_id}")]
    DocumentUploadNotFound { upload_id: String },
    #[error("文档上传身份、顺序或状态冲突 (Document upload identity, sequence or state conflict)")]
    DocumentUploadConflict { field: String },

    #[error("文档修订不匹配，请读取当前来源 (Document revision mismatch; read the current source)")]
    DocumentRevisionMismatch {
        document_id: u64,
        expected: String,
        actual: String,
    },

    #[error("幂等键已用于其他事务内容 (Idempotency key was used for a different transaction)")]
    IdempotencyConflict,

    #[error(
        "事务结果无法确认，请核对事实且勿换键盲目重试 (Transaction outcome unknown; reconcile facts and do not blindly retry with a new key)"
    )]
    WriteOutcomeUnknown,

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

impl TriviumError {
    /// 外层准入只补充尚未由执行点提供的上下文。
    pub(crate) fn with_write_info(self, write: WriteErrorInfo) -> Self {
        if self.write_info().is_some() {
            self
        } else {
            Self::WriteFailure {
                source: Box::new(self),
                write,
            }
        }
    }

    fn write_info(&self) -> Option<WriteErrorInfo> {
        match self {
            Self::WriteFailure { write, .. } => Some(*write),
            Self::QueryParameter { source, .. }
            | Self::QuerySource { source, .. }
            | Self::QueryParameterSource { source, .. }
            | Self::ApiField { source, .. } => source.write_info(),
            _ => None,
        }
    }

    pub fn at_field(self, field: &str) -> Self {
        Self::ApiField {
            field: field.into(),
            source: Box::new(self),
        }
    }

    /// 为已知参数附加定位，原错误码、详情和错误链继续保留。
    pub fn at_query_parameter(self, parameter: &str) -> Self {
        Self::QueryParameter {
            parameter: parameter.into(),
            source: Box::new(self),
        }
    }

    pub(crate) fn at_query_source(self, stage: QueryErrorStage, span: Option<QuerySpan>) -> Self {
        let info = self.info();
        let located = info
            .target
            .as_ref()
            .and_then(ErrorTarget::source_span)
            .is_some();
        if info.stage.is_some() && (span.is_none() || located) {
            return self;
        }
        Self::QuerySource {
            stage: info.stage.unwrap_or(stage),
            span,
            source: Box::new(self),
        }
    }

    /// 语言异常分类与 Rust 模式匹配使用原始错误变体。
    pub fn underlying(&self) -> &Self {
        match self {
            Self::QueryParameter { source, .. }
            | Self::QuerySource { source, .. }
            | Self::QueryParameterSource { source, .. }
            | Self::WriteFailure { source, .. }
            | Self::ApiField { source, .. } => source.underlying(),
            _ => self,
        }
    }

    pub fn info(&self) -> ErrorInfo {
        if let Self::QuerySource {
            stage,
            span,
            source,
        } = self
        {
            let mut info = source.info();
            info.stage = Some(*stage);
            if let Some(query_span) = span {
                info.target = Some(match info.target {
                    Some(ErrorTarget::Parameter { parameter }) => ErrorTarget::ParameterReference {
                        parameter,
                        query_span: *query_span,
                    },
                    Some(ErrorTarget::Field { field }) => ErrorTarget::FieldReference {
                        field,
                        query_span: *query_span,
                    },
                    Some(target) => target,
                    None => ErrorTarget::Query {
                        query_span: *query_span,
                    },
                });
            }
            return info;
        }
        if let Self::QueryParameterSource {
            parameter,
            query_span,
            source,
        } = self
        {
            let mut info = source.info();
            info.target = Some(ErrorTarget::ParameterReference {
                parameter: parameter.clone(),
                query_span: *query_span,
            });
            return info;
        }
        if let Self::ApiField { field, source } = self {
            let mut info = source.info();
            info.target = Some(
                match info.target.as_ref().and_then(ErrorTarget::source_span) {
                    Some(query_span) => ErrorTarget::FieldReference {
                        field: field.clone(),
                        query_span,
                    },
                    None => ErrorTarget::Field {
                        field: field.clone(),
                    },
                },
            );
            return info;
        }
        if let Self::QueryParameter { parameter, source } = self {
            let mut info = source.info();
            info.stage = Some(QueryErrorStage::Bind);
            info.target = Some(
                match info.target.as_ref().and_then(ErrorTarget::source_span) {
                    Some(query_span) => ErrorTarget::ParameterReference {
                        parameter: parameter.clone(),
                        query_span,
                    },
                    None => ErrorTarget::Parameter {
                        parameter: parameter.clone(),
                    },
                },
            );
            return info;
        }
        if let Self::WriteFailure { source, write } = self {
            let mut info = source.info();
            info.write = Some(*write);
            return info;
        }
        let (stage, target) = match self {
            Self::QuerySyntax { stage, span, .. } => (
                Some(*stage),
                span.map(|query_span| ErrorTarget::Query { query_span }),
            ),
            Self::MissingQueryParameter { parameter }
            | Self::UnexpectedQueryParameter { parameter } => (
                Some(QueryErrorStage::Bind),
                Some(ErrorTarget::Parameter {
                    parameter: parameter.clone(),
                }),
            ),
            _ => (None, None),
        };
        ErrorInfo {
            schema_version: ERROR_SCHEMA_VERSION,
            code: self.code(),
            message: self.to_string(),
            details: self.details(),
            stage,
            target,
            write: self.write_info(),
        }
    }

    /// 三端共享的稳定错误码；消息可翻译，控制流依赖错误码与字段。
    pub fn code(&self) -> &'static str {
        match self {
            Self::QueryParameter { source, .. }
            | Self::QuerySource { source, .. }
            | Self::QueryParameterSource { source, .. }
            | Self::WriteFailure { source, .. }
            | Self::ApiField { source, .. } => source.code(),
            Self::Io(_) => "TDB_IO",
            Self::Serialization(_) => "TDB_SERIALIZATION",
            Self::DimensionMismatch { .. } => "TDB_DIMENSION_MISMATCH",
            Self::DtypeMismatch { .. } => "TDB_DTYPE_MISMATCH",
            Self::NodeNotFound(_) => "TDB_NODE_NOT_FOUND",
            Self::InvalidVector { .. } => "TDB_INVALID_VECTOR",
            Self::PayloadTooLarge { .. } => "TDB_PAYLOAD_TOO_LARGE",
            Self::UniqueConstraintViolation { .. } => "TDB_UNIQUE_CONSTRAINT",
            Self::ConditionalUpdateNotMatched { .. } => "TDB_CONDITIONAL_UPDATE_NOT_MATCHED",
            Self::NodeAlreadyExists(_) => "TDB_NODE_ALREADY_EXISTS",
            Self::DatabaseLocked(_) => "TDB_DATABASE_LOCKED",
            Self::DatabaseBusy { .. } => "TDB_BUSY",
            Self::ReentrantAccess { .. } => "TDB_REENTRANT_ACCESS",
            Self::HostLockPoisoned { .. } => "TDB_HOST_LOCK_POISONED",
            Self::ReadOnlyViolation { .. } => "TDB_READ_ONLY",
            Self::RecoveryRequired { .. } => "TDB_RECOVERY_REQUIRED",
            Self::ImmutableArtifactInvalid { .. } => "TDB_IMMUTABLE_ARTIFACT",
            Self::GenerationBusy { .. } => "TDB_GENERATION_BUSY",
            Self::CorruptedFile(_) => "TDB_CORRUPTED_FILE",
            Self::QueryParse(_) | Self::QuerySyntax { .. } => "TDB_QUERY_PARSE",
            Self::QueryExecution(_) => "TDB_QUERY_EXECUTION",
            Self::MissingQueryParameter { .. } => "TDB_MISSING_QUERY_PARAMETER",
            Self::UnexpectedQueryParameter { .. } => "TDB_UNEXPECTED_QUERY_PARAMETER",
            Self::QueryCancelled => "TDB_QUERY_CANCELLED",
            Self::QueryDeadlineExceeded => "TDB_QUERY_DEADLINE_EXCEEDED",
            Self::QueryRowBudgetExceeded { .. } => "TDB_QUERY_BUDGET",
            Self::PayloadQueryBudgetExceeded { .. } => "TDB_PAYLOAD_BUDGET_EXCEEDED",
            Self::TraversalBudgetExceeded { .. } => "TDB_TRAVERSAL_BUDGET_EXCEEDED",
            Self::HookExecutionError(_) => "TDB_HOOK_EXECUTION",
            Self::HookLoadError(_) => "TDB_HOOK_LOAD",
            Self::WalClosed => "TDB_WAL_CLOSED",
            Self::CapacityReservationRejected { .. } => "TDB_CAPACITY_RESERVATION_REJECTED",
            Self::CapacityAllocationFailed { .. } => "TDB_CAPACITY_ALLOCATION_FAILED",
            Self::ResourceBudgetExceeded { .. } => "QueueFull",
            Self::SnapshotDestinationExists { .. } => "TDB_SNAPSHOT_EXISTS",
            Self::SnapshotPublicationUncertain { .. } => "TDB_SNAPSHOT_PUBLICATION_UNCERTAIN",
            Self::ShutdownFailed { .. } => "TDB_SHUTDOWN_FAILED",
            Self::DatabaseClosed => "TDB_DATABASE_CLOSED",
            Self::TransactionFinished => "TDB_TRANSACTION_FINISHED",
            Self::ConsoleSessionClosed => "TDB_CONSOLE_SESSION_CLOSED",
            Self::ConsoleResponseTooLarge { .. } => "TDB_CONSOLE_RESPONSE_TOO_LARGE",
            Self::WriteConflict { .. } => "TDB_WRITE_CONFLICT",
            Self::GraphNotFound { .. } => "TDB_GRAPH_NOT_FOUND",
            Self::GraphRevisionMismatch { .. } => "TDB_GRAPH_REVISION_MISMATCH",
            Self::DocumentRevisionMismatch { .. } => "TDB_DOCUMENT_REVISION_MISMATCH",
            Self::DocumentUploadNotFound { .. } => "TDB_DOCUMENT_UPLOAD_NOT_FOUND",
            Self::DocumentUploadConflict { .. } => "TDB_DOCUMENT_UPLOAD_CONFLICT",
            Self::IdempotencyConflict => "TDB_IDEMPOTENCY_CONFLICT",
            Self::WriteOutcomeUnknown => "TDB_WRITE_OUTCOME_UNKNOWN",
            Self::UnsupportedWalVersion { .. } => "TDB_UNSUPPORTED_WAL_VERSION",
            Self::UnsupportedDatabaseVersion { .. } => "TDB_UNSUPPORTED_DATABASE_VERSION",
            Self::ApiMigrationRequired { .. } => "TDB_API_MIGRATION_REQUIRED",
            Self::UnsupportedSidecarVersion { .. } => "TDB_UNSUPPORTED_SIDECAR_VERSION",
            Self::InvalidInput(_) => "TDB_INVALID_INPUT",
            Self::Generic(_) => "TDB_ERROR",
        }
    }

    /// 错误特有的字段；整数保持原始位宽，由绑定按语言数值域转换。
    pub fn details(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Self::QueryParameter { source, .. }
            | Self::QuerySource { source, .. }
            | Self::QueryParameterSource { source, .. }
            | Self::WriteFailure { source, .. }
            | Self::ApiField { source, .. } => source.details(),
            Self::DatabaseBusy { resource } | Self::HostLockPoisoned { resource } => {
                json!({"resource": resource})
            }
            Self::ReentrantAccess { operation } => json!({"operation": operation}),
            Self::Io(error) => {
                json!({"io_kind": format!("{:?}", error.kind()), "os_code": error.raw_os_error()})
            }
            Self::Serialization(error) => json!({"reason": error.to_string()}),
            Self::WriteConflict { expected, actual } => {
                json!({"expected":expected,"actual":actual})
            }
            Self::GraphNotFound { graph_id } => json!({"graph_id":graph_id}),
            Self::DocumentUploadNotFound { upload_id } => json!({"upload_id":upload_id}),
            Self::DocumentUploadConflict { field } => json!({"field":field}),
            Self::GraphRevisionMismatch {
                graph_id,
                expected,
                actual,
            } => json!({"graph_id":graph_id,"expected":expected,"actual":actual}),
            Self::DocumentRevisionMismatch {
                document_id,
                expected,
                actual,
            } => {
                json!({"document_id":document_id.to_string(),"expected":expected,"actual":actual})
            }
            Self::IdempotencyConflict | Self::WriteOutcomeUnknown => json!({}),
            Self::DimensionMismatch { expected, got } => json!({"expected": expected, "got": got}),
            Self::DtypeMismatch { stored, requested } => {
                json!({"stored": stored, "requested": requested})
            }
            Self::NodeNotFound(id)
            | Self::NodeAlreadyExists(id)
            | Self::ConditionalUpdateNotMatched { id } => json!({"id": id}),
            Self::UniqueConstraintViolation {
                fields,
                existing_id,
                conflicting_id,
            } => {
                json!({"fields": fields, "existing_id": existing_id, "conflicting_id": conflicting_id})
            }
            Self::PayloadTooLarge {
                size_bytes,
                max_bytes,
            } => json!({"size_bytes": size_bytes, "max_bytes": max_bytes}),
            Self::ReadOnlyViolation { operation } => json!({"operation": operation}),
            Self::ConsoleResponseTooLarge {
                max_bytes,
                committed,
            } => {
                json!({"max_bytes": max_bytes, "committed": committed})
            }
            Self::RecoveryRequired { wal_path } => json!({"wal_path": wal_path}),
            Self::GenerationBusy { generation_id } => json!({"generation_id": generation_id}),
            Self::MissingQueryParameter { parameter }
            | Self::UnexpectedQueryParameter { parameter } => json!({"parameter": parameter}),
            Self::QueryRowBudgetExceeded { budget } => json!({"budget": budget}),
            Self::PayloadQueryBudgetExceeded {
                dimension,
                used,
                budget,
            } => json!({"dimension": dimension, "used": used, "budget": budget}),
            Self::TraversalBudgetExceeded {
                dimension,
                visited_nodes,
                examined_edges,
                peak_frontier_size,
                depth_reached,
            } => {
                json!({"dimension": dimension, "visited_nodes": visited_nodes, "examined_edges": examined_edges, "peak_frontier_size": peak_frontier_size, "depth_reached": depth_reached})
            }
            Self::CapacityReservationRejected {
                requested_nodes,
                estimated_bytes,
                current_bytes,
                memory_limit,
            } => {
                json!({"requested_nodes": requested_nodes, "estimated_bytes": estimated_bytes, "current_bytes": current_bytes, "memory_limit": memory_limit})
            }
            Self::ResourceBudgetExceeded {
                resource,
                requested,
                used,
                limit,
            } => {
                json!({"resource": resource, "requested": requested, "used": used, "limit": limit})
            }
            Self::SnapshotDestinationExists { path } => json!({"path": path}),
            Self::SnapshotPublicationUncertain { path, reason } => {
                json!({"path": path, "reason": reason})
            }
            Self::UnsupportedWalVersion { found, supported } => {
                json!({"found": found, "supported": supported})
            }
            Self::UnsupportedDatabaseVersion {
                found,
                minimum_supported,
                current,
            } => {
                json!({"found": found, "minimum_supported": minimum_supported, "current": current})
            }
            Self::UnsupportedSidecarVersion {
                component,
                found,
                minimum_supported,
                current,
                action,
            } => {
                json!({"component": component, "found": found, "minimum_supported": minimum_supported, "current": current, "action": action})
            }
            Self::ApiMigrationRequired {
                removed_api,
                replacement,
            } => json!({"removed_api": removed_api, "replacement": replacement}),
            Self::QuerySyntax { reason, .. }
            | Self::InvalidVector { reason }
            | Self::ImmutableArtifactInvalid { reason }
            | Self::CapacityAllocationFailed { reason }
            | Self::ShutdownFailed { reason } => json!({"reason": reason}),
            Self::DatabaseLocked(reason)
            | Self::CorruptedFile(reason)
            | Self::QueryParse(reason)
            | Self::QueryExecution(reason)
            | Self::HookExecutionError(reason)
            | Self::HookLoadError(reason)
            | Self::InvalidInput(reason)
            | Self::Generic(reason) => json!({"reason": reason}),
            Self::QueryCancelled
            | Self::QueryDeadlineExceeded
            | Self::WalClosed
            | Self::DatabaseClosed
            | Self::TransactionFinished
            | Self::ConsoleSessionClosed => json!({}),
        }
    }
}

pub type Result<T> = std::result::Result<T, TriviumError>;

#[cfg(test)]
pub(crate) mod tests;
