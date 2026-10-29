/**
 * TriviumDB - AI-native Embedded Database
 * Vector + Graph + Relational in one file.
 * 数字 ID 的输入与输出均为 JavaScript 安全整数；遇到完整 u64 大 ID 时返回参数错误。
 * TQL 查询以十进制字符串返回完整 u64 ID。数量参数在整数转换前检查有限性、整数性与范围。
 * TQL 整数标量及列表、对象中的整数超出安全整数范围时返回十进制字符串。
 */

// ==========================================
// 辅助类型定义
// ==========================================

/** u64 分量要求有限非负整数且小于 2**64；number 按 binary64 精度表示。 */
export type Vector = number[];

export type SyncMode = 'full' | 'normal' | 'off';

export type DType = 'f32' | 'f16' | 'u64';

export interface JsSearchHit<T = any> {
  /** JavaScript 安全整数节点 ID；超范围结果使整个调用返回参数错误。 */
  id: number;
  /** 相似度得分 (余弦相似度或图谱扩散热度) */
  score: number;
  /** 节点 JSON 元数据 */
  payload: T;
}

export interface JsGroupedSearchResult<T = any> {
  semanticHits: JsSearchHit<T>[];
  graphHits: JsSearchHit<T>[];
}

export interface JsEdge {
  /** 目标节点 ID */
  targetId: number;
  /** 边的分组类型或者名称 */
  label: string;
  /** 边权重 */
  weight: number;
  /** 边的 JSON 元数据。 */
  metadata: unknown;
}

export interface JsIncomingEdge {
  /** 源节点 ID */
  sourceId: number;
  /** 目标节点 ID */
  targetId: number;
  /** 边标签 */
  label: string;
  /** 边权重 */
  weight: number;
  /** 边的 JSON 元数据。 */
  metadata: unknown;
}

export type ReachabilityDirection = 'outgoing' | 'incoming' | 'both';

export interface JsReachabilityOptions {
  minDepth?: number;
  maxDepth?: number;
  labels?: string[];
  direction?: ReachabilityDirection;
  maxVisitedNodes?: number;
  maxResults?: number;
  maxEdges?: number;
}

export interface JsReachabilityStep {
  from: number;
  to: number;
  label: string;
  weight: number;
  metadata: any;
}

export interface JsReachabilityResult {
  sourceId: number;
  targetId: number;
  depth: number;
  path: number[];
  steps: JsReachabilityStep[];
}

export interface JsReachabilityOutput {
  results: JsReachabilityResult[];
  visitedNodes: number;
  traversedEdges: number;
  truncated: boolean;
}

export interface JsSubgraphNode<T = any> {
  id: number;
  payload: T;
}

export interface JsSubgraphEdge {
  sourceId: number;
  targetId: number;
  label: string;
  weight: number;
  metadata: any;
}

export interface JsSubgraphResult<T = any> {
  nodes: JsSubgraphNode<T>[];
  edges: JsSubgraphEdge[];
  visitedNodes: number;
  traversedEdges: number;
  truncated: boolean;
}

export type TransactionOperation =
  | { type: 'insert'; vector: Vector; payload?: any }
  | { type: 'insertWithId'; id: number; vector: Vector; payload?: any }
  | { type: 'delete'; id: number }
  | { type: 'updatePayload'; id: number; payload: any }
  | { type: 'updateVector'; id: number; vector: Vector }
  | { type: 'link' | 'upsertEdge'; src: number; dst: number; label?: string; weight?: number; metadata?: any }
  | { type: 'unlink'; src: number; dst: number }
  | { type: 'unlinkLabel'; src: number; dst: number; label: string };

export interface GraphStats {
  nodeCount: number;
  edgeCount: number;
  isolatedNodeCount: number;
  labelCount: number;
}

export interface GraphIntegrityReport {
  danglingEdges: number;
  duplicateEdges: number;
  incomingIndexMismatches: number;
  degreeIndexMismatches: number;
  labelIndexMismatches: number;
  valid: boolean;
}

export interface GraphRepairReport {
  removedDanglingEdges: number;
  removedDuplicateEdges: number;
  rebuiltIndexes: boolean;
}

export interface JsNodeView<T = any> {
  /** 节点 ID */
  id: number;
  /** 特征向量 */
  vector: Vector;
  /** 节点 JSON 元数据 */
  payload: T;
  /** 该节点出发的有向边列表 */
  edges: JsEdge[];
  /** 该节点出发的有向边数量 */
  numEdges: number;
}

/**
 * Cypher 查询的一行结果
 * 键名为 MATCH 语句中你定义的绑定变量（如 a, b），值为匹配到的节点视图摘要
 */
export type QueryRow = Record<string, {
  id: number;
  payload: any;
  numEdges: number;
}>;

// ==========================================
// MongoDB 风格的 Filter 定义
// ==========================================

export type FilterOperator<T> =
  | T
  | { $eq?: T }
  | { $ne?: T }
  | { $gt?: number | string }
  | { $gte?: number | string }
  | { $lt?: number | string }
  | { $lte?: number | string }
  | { $before?: string }
  | { $beforeEq?: string }
  | { $after?: string }
  | { $afterEq?: string }
  | { $in?: T[] };

export type FilterCondition = {
  [field: string]: FilterOperator<any>;
} | {
  $and?: FilterCondition[];
  $or?: FilterCondition[];
};

// ==========================================
// 认知管线配置
// ==========================================

export interface JsSearchConfig {
  /** 最终返回结果数量 (默认 5) */
  topK?: number;
  /** 初始稠密/稀疏召回池；0表示自动 */
  recallK?: number;
  /** SA-PPR/FISTA/DPP前候选池；0表示自动 */
  rerankK?: number;
  /** 图谱扩散跳数 (默认 2) */
  expandDepth?: number;
  /** 余弦相似度下限 (默认 0.1) */
  minScore?: number;
  /** PPR 回跳概率 0.0~1.0，越高越抑制深层扩散 (默认 0.0) */
  teleportAlpha?: number;
  /** 认知管线总开关 (默认 true) */
  enableAdvancedPipeline?: boolean;
  /** 启用 FISTA 残差寻隐 + 影子查询 (默认 false) */
  enableSparseResidual?: boolean;
  /** FISTA L1 正则化系数 (默认 0.1) */
  fistaLambda?: number;
  /** 残差范数超过此值时触发影子查询 (默认 0.3) */
  fistaThreshold?: number;
  /** 启用 DPP 多样性采样 (默认 false) */
  enableDpp?: boolean;
  /** DPP 质量权重幂次 (默认 1.0) */
  dppQualityWeight?: number;
  /** 启用物理神经不应期（Fatigue），强制避免对高频节点的死循环访问 (默认 false) */
  enableRefractoryFatigue?: boolean;
  /** 启用文本/关键词混合检索 (默认 false) */
  enableTextHybridSearch?: boolean;
  /** 加权RRF中的稀疏排名权重 (默认 1.5) */
  textBoost?: number;
  /** 自定义检索文本（用于跨模态或覆盖 payload 文本） */
  customQueryText?: string;
  /** 类 MongoDB JSON Payload 过滤器，在向量召回阶段生效 */
  payloadFilter?: Record<string, unknown>;
  /** 强制使用暴力搜索 (默认 false) */
  forceBruteForce?: boolean;
  /**
   * CCSA: 扩散方向偏置向量
   *
   * 当提供时，图扩散优先沿着与此向量语义相近的节点方向传播。
   * gate_j = σ(bias · v_j / √dim)，gate ∈ (0, 1) 调制能量传导强度。
   * 不提供时退化为无上下文门控的 SA-PPR。
   *
   * 典型用途:
   * - 对话系统: 传入 RNN 隐状态的投影向量，让扩散感知对话方向
   * - RAG 应用: 传入查询向量本身，让扩散偏向查询语义方向
   * - 推荐系统: 传入用户偏好向量，让扩散偏向用户兴趣方向
   */
  diffusionBias?: number[];
  /** 图扩散允许的边标签；省略表示全部，空数组表示禁止扩散 */
  expandLabels?: string[];
  /** 每个扩散节点最多选择的边数；0 或省略表示不限制 */
  maxEdgesPerNode?: number;
  /** 仅传播绝对权重大于等于该阈值的边 */
  minEdgeWeight?: number;
  /** 图扩散方向，默认 out */
  edgeDirection?: 'out' | 'in' | 'both';
}

export interface JsClusterResult {
  /** 节点到簇的扁平映射 [nodeId1, clusterId1, nodeId2, clusterId2...] */
  nodeToCluster: number[];
  /** 簇及其标签的扁平映射 [clusterId(string), label, ...] */
  clusterLabels: string[];
  /** 各簇的质心 [clusterId1, v0, v1, ..., clusterId2, ...] */
  centroids: number[];
}

export interface JsLeidenConfig {
  /** 最小社区大小 (节点数 < 此值的碎片簇被丢弃, 默认 3) */
  minCommunitySize?: number;
  /** 最大迭代轮次 (默认 15) */
  maxIterations?: number;
  /** 是否计算质心 (默认 true) */
  withCentroids?: boolean;
}

/** Hook 管线执行上下文（包含各阶段计时统计和自定义数据） */
export interface JsHookContext {
  /** 各管线阶段的耗时统计（JSON 对象, 单位: 毫秒） */
  timings: any;
  /** 每阶段候选数量 */
  counts: any;
  /** Hook 注入的自定义数据 */
  customData: any;
  /** 进程内存、缺页、检索路由与候选规模观测值 */
  observations: Record<string, number>;
  /** 管线是否被 Hook 提前终止 */
  aborted: boolean;
}
/** 管线执行错误信息 */
export interface JsHookError {
  /** 错误类型 */
  type: string;
  /** 错误消息 */
  message: string;
}


/** 带上下文的检索结果 */
export interface JsSearchWithContextResult {
  /** 检索结果列表 */
  hits: JsSearchHit[];
  /** Hook 管线上下文 */
  context: JsHookContext;
}

// ==========================================
// 核心类定义
// ==========================================

/**
 * TriviumDB 实例。
 * 读写实例使用排他锁；多个只读实例可使用共享锁并发打开。
 */
export interface ResourceLimits {
  /** 总预算内保护的维护额度（MiB），0 自动保护总预算的八分之一 */
  maintenanceMemoryLimitMb?: number;
  /** 数据库配额与临时工作区总额（MiB），0 表示不限 */
  memoryLimitMb?: number;
  /** 工作线程总上限，0 自动，最大 64 */
  maxThreads?: number;
  /** 同时准入的根任务数，0 表示不限 */
  maxActiveTasks?: number;
}

export interface ResourceStats {
  maintenance_memory_limit: number;
  reserved_maintenance_bytes: number;
  active_maintenance_tasks: number;
  memory_limit: number;
  reserved_database_bytes: number;
  reserved_workspace_bytes: number;
  peak_reserved_bytes: number;
  attached_databases: number;
  max_threads: number;
  allocated_worker_threads: number;
  live_worker_threads: number;
  peak_worker_threads: number;
  max_active_tasks: number;
  active_tasks: number;
  peak_active_tasks: number;
  rejected_memory_requests: number;
  rejected_task_requests: number;
  rejected_thread_requests: number;
  maintenance_jobs: number;
  maintenance_completed: number;
  maintenance_failed: number;
  /** 账本余额；null 表示不限。两个内存余额共享总额，不能相加。 */
  available_foreground_bytes: number | null;
  available_maintenance_bytes: number | null;
  available_task_slots: number | null;
}

/** 错误详情中的大整数使用十进制字符串，其余整数使用安全 Number。 */
export type ErrorInteger = number | string;

/** 内核错误字段；错误码与字段名在 Rust、Python、Node 三端保持一致。 */
export interface ErrorDetailsByCode {
  TDB_IO: { io_kind: string; os_code: number | null };
  TDB_SERIALIZATION: { reason: string };
  TDB_DTYPE_MISMATCH: { stored: DType; requested: DType };
  TDB_DIMENSION_MISMATCH: { expected: ErrorInteger; got: ErrorInteger };
  TDB_NODE_NOT_FOUND: { id: ErrorInteger };
  TDB_INVALID_VECTOR: { reason: string };
  TDB_PAYLOAD_TOO_LARGE: { size_bytes: ErrorInteger; max_bytes: ErrorInteger };
  TDB_UNIQUE_CONSTRAINT: { fields: string[]; existing_id: ErrorInteger; conflicting_id: ErrorInteger };
  TDB_CONDITIONAL_UPDATE_NOT_MATCHED: { id: ErrorInteger };
  TDB_NODE_ALREADY_EXISTS: { id: ErrorInteger };
  TDB_DATABASE_LOCKED: { reason: string };
  TDB_READ_ONLY: { operation: string };
  TDB_RECOVERY_REQUIRED: { wal_path: string };
  TDB_IMMUTABLE_ARTIFACT: { reason: string };
  TDB_GENERATION_BUSY: { generation_id: string };
  TDB_CORRUPTED_FILE: { reason: string };
  TDB_QUERY_PARSE: { reason: string };
  TDB_QUERY_EXECUTION: { reason: string };
  TDB_MISSING_QUERY_PARAMETER: { parameter: string };
  TDB_UNEXPECTED_QUERY_PARAMETER: { parameter: string };
  TDB_QUERY_CANCELLED: Record<string, never>;
  TDB_QUERY_BUDGET: { budget: ErrorInteger };
  TDB_PAYLOAD_BUDGET_EXCEEDED: { dimension: string; used: ErrorInteger; budget: ErrorInteger };
  TDB_TRAVERSAL_BUDGET_EXCEEDED: { dimension: 'visited_nodes' | 'examined_edges' | 'frontier_size' | 'depth'; visited_nodes: ErrorInteger; examined_edges: ErrorInteger; peak_frontier_size: ErrorInteger; depth_reached: ErrorInteger };
  TDB_HOOK_EXECUTION: { reason: string };
  TDB_HOOK_LOAD: { reason: string };
  TDB_WAL_CLOSED: Record<string, never>;
  TDB_CAPACITY_RESERVATION_REJECTED: { requested_nodes: ErrorInteger; estimated_bytes: ErrorInteger; current_bytes: ErrorInteger; memory_limit: ErrorInteger };
  TDB_CAPACITY_ALLOCATION_FAILED: { reason: string };
  QueueFull: ResourceBudgetDetails;
  TDB_SNAPSHOT_EXISTS: { path: string };
  TDB_SNAPSHOT_PUBLICATION_UNCERTAIN: { path: string; reason: string };
  TDB_SHUTDOWN_FAILED: { reason: string };
  TDB_DATABASE_CLOSED: Record<string, never>;
  TDB_UNSUPPORTED_WAL_VERSION: { found: number; supported: number };
  TDB_UNSUPPORTED_DATABASE_VERSION: { found: number; minimum_supported: number; current: number };
  TDB_API_MIGRATION_REQUIRED: { removed_api: string; replacement: string };
  TDB_UNSUPPORTED_SIDECAR_VERSION: { component: string; found: number; minimum_supported: number; current: number; action: string };
  TDB_INVALID_INPUT: { reason: string };
  TDB_ERROR: { reason: string };
}

export type TriviumDBError = {
  [Code in keyof ErrorDetailsByCode]: Error & { code: Code; details: ErrorDetailsByCode[Code] }
}[keyof ErrorDetailsByCode];

/** QueueFull 错误上的附加字段；内存单位为字节，任务和线程单位为个数。 */
export interface ResourceBudgetDetails {
  resource: string;
  requested: ErrorInteger;
  used: ErrorInteger;
  limit: ErrorInteger;
}

export interface ResourceBudgetError extends Error, ResourceBudgetDetails {
  code: 'QueueFull';
  details: ResourceBudgetDetails;
}

export interface ResourceFailure {
  kind: 'resource_budget' | 'io' | 'capacity' | 'database_closed' | 'shutdown' | 'other';
  message: string;
  budget: ResourceBudgetDetails | null;
}

export interface MaintenanceStatus {
  phase: 'scheduled' | 'running' | 'stopped';
  deferred_runs: number;
  completed_runs: number;
  failed_runs: number;
  last_defer_reason: 'not_ready' | 'resource_budget' | 'database_busy' | null;
  last_failure: ResourceFailure | null;
}

export interface InstanceStats {
  database_path: string;
  memory_limit_bytes: number;
  /** 写锁繁忙时为 null；已关闭实例的预留配额和节点数为 0。 */
  memory: {
    reserved_database_bytes: number;
    estimated_memory_bytes: number;
    node_count: number;
  } | null;
  shutdown: ShutdownStatus;
  compaction: MaintenanceStatus | null;
}

/** 多实例共享的配额与计算运行时；额度不等于进程 RSS。 */
export class ResourceRuntime {
  constructor(options?: ResourceLimits);
  stats(): ResourceStats;
}

export interface ShutdownStatus {
  phase: 'open' | 'draining' | 'flushing' | 'failed' | 'closed';
  active_operations: number;
  last_error: string | null;
  last_failure: ResourceFailure | null;
  maintenance: MaintenanceStatus | null;
}

export interface SnapshotManifest {
  format_version: number;
  generation_id: string;
  dtype: string;
  dim: number;
  node_count: number;
  complete: boolean;
  files: Array<{ suffix: string; size: number; crc32: number }>;
}

export function verifySnapshot(directory: string, runtime?: ResourceRuntime): SnapshotManifest;
export function restoreSnapshot(source: string, destination: string, runtime?: ResourceRuntime): SnapshotManifest;

export interface TriviumDBOptions {
  /** 共享运行时，省略时复用进程默认运行时 */
  runtime?: ResourceRuntime;
  /** 向量维度，默认 1536 */
  dim?: number;
  /** 向量数据类型，默认 f32 */
  dtype?: DType;
  /** WAL 同步模式，默认 normal */
  syncMode?: SyncMode;
  /** 存储模式，默认 mmap */
  storageMode?: 'mmap' | 'rom';
  /** 是否自动构建 QuIVer，默认 true */
  autoBuildQuiver?: boolean;
  /** 是否在打开时加载全文索引，默认 false */
  loadTextIndex?: boolean;
  /** 预计总节点数，仅用于本次进程预留核心容器，不是硬上限且不会持久化 */
  expectedNodes?: number;
  /** TriviumDB 内核内存预算（MiB），0 或省略表示不限制 */
  memoryLimitMb?: number;
  /** Payload 解析缓存总上限（MiB），默认 64，0 表示禁用 */
  payloadCacheMb?: number;
  /** 单条 Payload 可进入解析缓存的最大估算大小（MiB），默认 8 */
  payloadCacheEntryMb?: number;
  /** 访问模式；readOnly 不创建 WAL、不会修改任何数据库文件 */
  accessMode?: 'readWrite' | 'readOnly' | 'immutable';
  /** Reader 遇到缺失或损坏 sidecar 时的行为 */
  missingIndexPolicy?: 'fallback' | 'buildInMemory' | 'error';
  /** TQL 默认结果行上限；0 表示禁用默认上限，显式 LIMIT 可覆盖该值 */
  maxQueryRows?: number;
  /** 普通 TQL 查询超过默认行上限时抛错，或告警后返回部分结果 */
  rowOverflow?: 'throw' | 'break';
}

export class PreparedTql {
  parameterNames(): string[];
}

export interface PropertyIndexInfo {
  field: string;
  fields: string[];
  kind: 'hash' | 'ordered' | 'composite' | 'bitmap' | 'ngram';
  unique: boolean;
  entry_count: number;
  distinct_count: number;
  null_count: number;
}

export interface StorageInfo {
  package_version: string;
  database_format_current: number;
  database_format_minimum: number;
  wal_format: number;
  property_index_format: number;
  graph_index_format: number;
  quiver_format: number;
  text_index_format: number;
  manifest_format: number;
  dim: number;
  node_count: number;
  storage_mode: 'mmap' | 'rom';
  access_mode: 'read_write' | 'read_only' | 'immutable';
  estimated_memory_bytes: number;
  sidecars: Record<string, boolean>;
}

export class TriviumDB {
  /**
   * 打开或创建数据库
   * @param path         数据库文件路径 (如 "data.tdb")
   * @param dim          向量维度，默认为 1536
   * @param dtype        数据类型设定: "f32" | "f16" | "u64", 默认为 "f32"
   * @param syncMode     WAL 同步模式设定: "full" | "normal" | "off", 默认为 "normal"
   */
  constructor(path: string, options?: TriviumDBOptions);

  publishGenerationManifest(generationId: string): {
    formatVersion: number;
    generationId: string;
    dtype: string;
    dim: number;
    nodeCount: number;
    files: Array<{ suffix: string; size: number; crc32: number }>;
    complete: boolean;
  };

  // ── Hook 管理 ──

  /** 注册同步 JavaScript 六阶段 Hook；回调不得返回 Promise */
  setHook(hook: {
    onPreSearch?: (query: number[]) => number[] | null;
    onCustomRecall?: (query: number[]) => Array<{ id: string; score: number; payload?: unknown }> | null;
    onPostRecall?: (hits: Array<{ id: string; score: number; payload?: unknown }>) => Array<{ id: string; score: number; payload?: unknown }> | null;
    onPreGraphExpand?: (hits: Array<{ id: string; score: number; payload?: unknown }>) => Array<{ id: string; score: number; payload?: unknown }> | null;
    onRerank?: (hits: Array<{ id: string; score: number; payload?: unknown }>) => Array<{ id: string; score: number; payload?: unknown }> | null;
    onPostSearch?: (hits: Array<{ id: string; score: number; payload?: unknown }>) => Array<{ id: string; score: number; payload?: unknown }> | null;
  }): void;

  /**
   * 加载 C/C++ 动态库作为检索管线 Hook
   *
   * 动态库需导出 C ABI 符号（均可选）：
   * - `trivium_recall`: 自定义召回
   * - `trivium_rerank`: 自定义重排序
   *
   * ```js
   * db.loadFfiHook('./libmy_plugin.so')
   * const results = db.search(queryVec)  // 自动经过 C++ Hook
   * ```
   */
  loadFfiHook(libPath: string): void;

  /** 清除当前已注册的 Hook，恢复为默认的零开销 NoopHook */
  clearHook(): void;

  /**
   * 带 Hook 上下文的检索：返回 { hits, context }
   *
   * 除了检索结果外，同时返回管线各阶段的计时统计和 Hook 注入的自定义数据。
   *
   * ```js
   * const { hits, context } = db.searchWithContext(queryVec, { topK: 10 })
   * console.log(context.timings)     // { hook_pre_search: 0.1, graph_expand: 2.3 }
   * console.log(context.customData)  // Hook 注入的自定义数据
   * ```
   */
  searchWithContext(queryVector: Vector, config?: JsSearchConfig): JsSearchWithContextResult;

  // ── CRUD ──

  /**
   * 插入新节点自动生成 ID
   * @param vector  向量数组，长度必须与 dim 保持一致
   * @param payload 挂在节点上的 payload 数据（可以是任何 JSON 支持类型）
   * @returns 分配的新节点 ID
   */
  insert(vector: Vector, payload: any): number;

  /**
   * 携带指定 ID 插入新节点
   * @param id      自定义节点 ID
   * @param vector  向量数组
   * @param payload 挂载 payload
   */
  insertWithId(id: number, vector: Vector, payload: any): void;

  /** 使用自定义 ID 插入或原子覆盖节点 */
  upsertWithId(id: number, vector: Vector, payload: any): void;

  /** 原子提交一组异构写操作，返回事务内插入节点的 ID */
  commitTransaction(operations: TransactionOperation[]): number[];

  /** 返回图节点、边、孤立节点及标签统计 */
  graphStats(): GraphStats;

  /** 校验悬空边、重复边和派生图索引 */
  validateGraph(): GraphIntegrityReport;

  /** 清理无效边并从权威出边表重建派生索引 */
  repairGraphIndexes(): GraphRepairReport;

  /**
   * 为后续插入主动预留额外节点容量。
   * 受 memoryLimitMb 约束；失败不会写 WAL 或修改节点数据。
   */
  reserveNodes(additional: number): void;

  /**
   * 批量插入节点（自动生成 ID）
   * @param vectors  向量数组列表
   * @param payloads payload 列表
   * @returns 分配的新 ID 列表
   */
  batchInsert(vectors: Vector[], payloads: any[]): number[];

  /**
   * 批量插入指定 ID 的节点
   * @param ids      ID 列表
   * @param vectors  向量数组列表
   * @param payloads payload 列表
   */
  batchInsertWithIds(ids: number[], vectors: Vector[], payloads: any[]): void;

  /**
   * 获取任意节点信息
   * @param id 节点 ID
   * @returns 如果不存在返回 null
   */
  get<T = any>(id: number): JsNodeView<T> | null;

  /**
   * 整体替换节点的 payload（不影响向量与图谱关系）
   * @param id 节点 ID
   * @param payload 新 payload
   */
  updatePayload(id: number, payload: any): void;

  /**
   * 部分更新节点 Payload（$set / $inc / $unset）
   *
   * 只修改指定字段，其他字段保持不变。
   *
   * ```js
   * db.patchPayload(id, { $set: { name: "Alice" } })
   * db.patchPayload(id, { $inc: { visits: 1 } })
   * db.patchPayload(id, { $unset: { oldField: true } })
   * db.patchPayload(id, { name: "Bob" })  // 简写，等价于 $set
   * ```
   */
  patchPayload(id: number, patch: any): void;

  /**
   * 更换节点的特征向量（必须保持与 dim 维度一致）
   * @param id 节点 ID
   * @param vector 新向量
   */
  updateVector(id: number, vector: Vector): void;

  /**
   * 删除一个节点。
   * **警告**: TriviumDB 实装的是三层原子联删，同时会抹除向量、清空 payload、并断开关联图谱的所有边
   * @param id 要删除的节点 ID
   */
  delete(id: number): void;

  /**
   * 检查节点是否存在
   * @param id 节点 ID
   * @returns 节点是否存在
   */
  contains(id: number): boolean;

  // ── 社区聚类 ──

  /**
   * 基于内存图谱进行 Leiden 社区发现
   *
   * **无锁设计**: 短暂持锁快照邻接表后立即释放，聚类在锁外计算。
   */
  leidenCluster(config?: JsLeidenConfig): JsClusterResult;

  // ── 图谱操作 ──

  /**
   * 在两节点之间建立有向带权边
   * @param src    源节点 ID
   * @param dst    目标节点 ID
   * @param label  边的分组类型或者名称，默认 "related"
   * @param weight 边权重，支持负数（抑制），默认 1.0
   */
  link(src: number, dst: number, label?: string, weight?: number): void;

  /** 精确读取 (src, dst, label) 边 */
  getEdge(src: number, dst: number, label: string): JsEdge | null;

  /** 插入或覆盖唯一边三元组的权重与元数据 */
  upsertEdge(src: number, dst: number, label: string, weight: number, metadata?: any): void;

  /** 更新已存在边的部分字段 */
  updateEdge(src: number, dst: number, label: string, weight?: number, metadata?: any): void;

  /**
   * 移除这亮点之间的所有边
   * @param src 源节点 ID
   * @param dst 目标节点 ID
   */
  unlink(src: number, dst: number, label?: string): void;

  /**
   * 图谱上的 N 跳搜索 (广度优先遍历)
   * @param id    起始点
   * @param depth 跳数 (默认 1)
   * @returns 深度之内的所有不重复的周边点 ID
   */
  neighbors(id: number, depth?: number, labels?: string[]): number[];

  /**
   * 获取节点的出边列表
   * @param id 节点 ID
   * @returns 该节点出发的所有有向边
   */
  getEdges(id: number): JsEdge[];

  /** 获取节点的完整入边，可按标签过滤 */
  getIncomingEdges(id: number, label?: string): JsIncomingEdge[];

  /** 按方向、标签和深度执行确定性可达性查询 */
  reachable(id: number, options?: JsReachabilityOptions): JsReachabilityResult[];

  /** 返回部分结果、预算统计及 truncated 标记 */
  reachableDetailed(id: number, options?: JsReachabilityOptions): JsReachabilityOutput;

  /** 返回确定性最短路径并集子图 */
  querySubgraph<T = any>(id: number, options?: JsReachabilityOptions): JsSubgraphResult<T>;

  /** 只在给定 anchor 集合内执行精确向量 Top-K */
  searchGraphFirst(queryVector: Vector, anchorIds: number[], topK: number, maxAnchorNodes?: number): JsSearchHit[];

  /** 全库精确向量 Top-K，用于 ANN 质量基准和小规模精确检索 */
  searchExact(queryVector: Vector, topK: number): JsSearchHit[];

  // ── 检索与查询 ──

  /**
   * 混合检索：向量锚定 + 图谱连带扩散！
   * @param queryVector 查询向量
   * @param topK        向外找多少个最相似锚点向量 (默认 5)
   * @param expandDepth 获取到上述锚点后，在图谱里扩散的跳跃深度 (默认 0，纯粹退化为向量相似度检索)
   * @param minScore    只接受相似度大于这个阈值的搜索命中 (默认 0.5)
   */
  search(queryVector: Vector, topK?: number, expandDepth?: number, minScore?: number, payloadFilter?: Record<string, unknown>): JsSearchHit[];

  searchGrouped(queryVector: Vector, topK?: number, expandDepth?: number, minScore?: number, payloadFilter?: Record<string, unknown>): JsGroupedSearchResult;
  searchBatch(queryVectors: Vector[], topK?: number, parallelism?: number, minScore?: number): Promise<JsSearchHit[][]>;

  /**
   * 认知管线检索：向量锚定 + FISTA残差 + SA-PPR有限深度扩散 + DPP多样性采样
   * @param queryVector 查询向量
   * @param config      管线配置（所有字段均可选，有安全默认值）
   */
  searchAdvanced(queryVector: Vector, config?: JsSearchConfig): JsSearchHit[];

  /**
   * 向量 + 文本双路混合检索：带图扩散的双路检索入口
   * @param queryVector 查询向量
   * @param queryText   文本关键词
   * @param topK        结果数
   * @param expandDepth 扩散深度
   * @param minScore    最小分数
   * @param hybridAlpha 混合权重 (0.0~1.0)，越大向量占比越高。默认 0.7
   */
  searchHybrid(queryVector: Vector, queryText: string, topK?: number, expandDepth?: number, minScore?: number, hybridAlpha?: number, payloadFilter?: Record<string, unknown>): JsSearchHit[];

  /**
   * 替换节点的 BM25 正文；空文本清除正文，关键词保持不变
   * @param id   节点 ID
   * @param text 文本内容
   */
  indexText(id: number, text: string): void;

  /**
   * 注册 AC 关键词；同节点重复注册幂等，同词可关联多个节点
   * @param id      节点 ID
   * @param keyword 关键词内容
   */
  indexKeyword(id: number, keyword: string): void;

  /**
   * 预热关键词自动机；延期索引加载或构建失败会抛出异常
   */
  buildTextIndex(): void;

  /** 从完整正文与关键词集合重建文本索引，通过 flush 或 close 持久化。 */
  rebuildTextIndex(documents: Array<{ id: number; text: string; keywords?: string[] }>): void;

  // ── 属性二级索引 ──

  /**
   * 创建属性索引：对指定 payload 字段建立倒排索引
   *
   * ```js
   * db.createIndex('name')   // 之后 tql('FIND {name: "Alice"} RETURN *') 使用 O(1) 索引
   * ```
   */
  createIndex(field: string): void;
  indexedLookup(equalities: Record<string, any>, maxResults?: number): number[];
  createNgramIndex(field: string): void;
  substringLookup(field: string, needle: string, maxResults?: number): number[];
  createUniqueIndex(field: string): void;
  createUniqueCompositeIndex(fields: string[]): void;
  listUniqueIndexes(): string[][];
  createOrderedIndex(field: string): void;
  createCompositeIndex(fields: string[]): void;
  createBitmapIndex(field: string): void;

  /** 删除属性索引（查询仍可用，退化为全扫描） */
  dropIndex(field: string): void;
  dropNgramIndex(field: string): void;
  dropOrderedIndex(field: string): void;
  dropCompositeIndex(fields: string[]): void;
  dropBitmapIndex(field: string): void;
  indexInfo(): PropertyIndexInfo[];
  storageInfo(): StorageInfo;

  // ── 轻量级单字段查询 ──

  /**
   * 获取节点的 payload（不含向量，比 get() 更轻量）
   * @param id 节点 ID
   * @returns payload 数据，节点不存在时返回 null
   */
  getPayload(id: number): any | null;

  // ── TQL 统一查询 ──

  /**
   * 执行 TQL (Trivium Query Language) 统一查询
   *
   * 支持三种入口：MATCH (图遍历) / FIND (文档过滤) / SEARCH (向量检索)
   *
   * ```js
   * // 图遍历
   * const rows = db.tql('MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b')
   * // 文档过滤
   * const rows = db.tql('FIND {type: "event", heat: {$gte: 0.7}} RETURN *')
   * ```
   */
  tql(query: string): any[];
  prepareTql(query: string): PreparedTql;
  executePreparedTql(prepared: PreparedTql, parameters: Record<string, string | number | boolean | null>): any[];

  /**
   * 执行 TQL 写操作（CREATE / SET / DELETE / DETACH DELETE）
   *
   * 返回 { affected: number, createdIds: number[] }
   *
   * ```js
   * const result = db.tqlMut('CREATE (a {name: "Alice", age: 30})')
   * console.log(result.affected)     // 1
   * console.log(result.createdIds)   // [1]
   *
   * db.tqlMut('MATCH (a {name: "Alice"}) SET a.age == 31')
   * db.tqlMut('MATCH (a {name: "Alice"}) DELETE a')
   * ```
   */
  tqlMut(query: string): { affected: number; createdIds: number[] };

  // ── 辅助与生命周期 ──

  /** 手动把记录在内存中的所有东西强制安全落盘 */
  flush(): void;

  /** 动态在运行时调整同步安全性 */
  setSyncMode(mode: SyncMode): void;

  /** 无人值守后台压缩；间隔必须大于0秒 */
  enableAutoCompaction(intervalSecs?: number): void;

  /** 控制查询或flush是否允许自动构建QuIVer */
  setAutoBuildQuiver(enabled: boolean): void;

  /** 清空疲劳等跨查询检索状态 */
  clearSearchState(): void;

  /** 关闭后台的定期压缩 */
  disableAutoCompaction(): void;

  /** 手动触发全量压实（阻塞当前线程） */
  compact(): void;

  /** 当估计的内存占用超过了这个 MB 阈值时会强制落排。填 0 = 不限制 */
  setMemoryLimit(mb: number): void;

  /** 查询估算的内存占用总量 Bytes */
  estimatedMemory(): number;
  /** 所属共享运行时的资源统计；资源拒绝的 error.code 为 QueueFull */
  resourceStats(): ResourceStats;
  /** 本实例诊断；不获取任务名额，也不等待 MemTable 写锁。 */
  instanceStats(): InstanceStats;
  requestShutdown(): ShutdownStatus;
  shutdownStatus(): ShutdownStatus;
  /** 超时返回 false；后台继续持有数据和文件锁，true 表示持久化及数据库文件、Hook 资源释放完成 */
  closeTimeout(timeoutMs: number): boolean;
  /** 创建新的独立快照目录；已有目标返回 TDB_SNAPSHOT_EXISTS */
  createSnapshot(destination: string, generationId: string): SnapshotManifest;

  /** 所有被存入库里的所有 ID 的乱序数组 */
  allNodeIds(): number[];

  /**
   * 维度结构化迁移。将所有关系和 payload 数据迁移到具有新权重要求尺寸的另一个 tdb 数据库！
   * 迁移后，这批 ID 在新库内的向量将是空的（0填充），供后续重新 updateVector 更新！
   * @param newPath 新数据库名称
   * @param newDim 新维度
   * @returns 迁移落库的所有新源节点 ID 列表
   */
  migrate(newPath: string, newDim: number): number[];

  /** 获取设置里的当前数据库维度 */
  dim(): number;

  /** 返回存储内的所有活跃点数量 */
  nodeCount(): number;

  /** 显式关闭并释放 WAL、mmap、Payload 与数据库持有的 Hook；关闭后数据访问抛错，状态与配置诊断仍可读取 */
  close(): void;

  /** `close()` 的幂等资源释放别名，供不支持 Explicit Resource Management 的环境调用 */
  dispose(): void;

  /** 支持 `using db = new TriviumDB(...)`，作用域退出时自动安全关闭 */
  [Symbol.dispose](): void;

  /** 获取设置的浮点格式 (f32, f16, u64) */
  get dtype(): string;
}
