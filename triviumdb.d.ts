/**
 * TriviumDB - AI-native Embedded Database
 * Vector + Graph + Relational in one file.
 */

// ==========================================
// 辅助类型定义
// ==========================================

export type Vector = number[];

export type SyncMode = 'full' | 'normal' | 'off';

export type DType = 'f32' | 'f16' | 'u64';

export interface JsSearchHit<T = any> {
  /** 节点 ID */
  id: number;
  /** 相似度得分 (余弦相似度或图谱扩散热度) */
  score: number;
  /** 节点 JSON 元数据 */
  payload: T;
}

export interface JsEdge {
  /** 目标节点 ID */
  targetId: number;
  /** 边的分组类型或者名称 */
  label: string;
  /** 边权重 */
  weight: number;
}

export interface JsIncomingEdge {
  /** 源节点 ID */
  sourceId: number;
  /** 边的分组类型或者名称 */
  label: string;
  /** 边权重 */
  weight: number;
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
  | { $gt?: number }
  | { $gte?: number }
  | { $lt?: number }
  | { $lte?: number }
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
  /** 文本分数的加权系数 (默认 1.5) */
  textBoost?: number;
  /** 自定义检索文本（用于跨模态或覆盖 payload 文本） */
  customQueryText?: string;
  /** 强制使用暴力搜索 (默认 false) */
  forceBruteForce?: boolean;
  /**
   * CCSA: 扩散方向偏置向量
   *
   * 当提供时，图扩散优先沿着与此向量语义相近的节点方向传播。
   * gate_j = σ(bias · v_j / √dim)，gate ∈ (0, 1) 调制能量传导强度。
   * 不提供时退化为标准 PPR（向后兼容）。
   *
   * 典型用途:
   * - 对话系统: 传入 RNN 隐状态的投影向量，让扩散感知对话方向
   * - RAG 应用: 传入查询向量本身，让扩散偏向查询语义方向
   * - 推荐系统: 传入用户偏好向量，让扩散偏向用户兴趣方向
   */
  diffusionBias?: number[];
  /**
   * 图扩散边标签白名单。省略时沿全部出边（与历史行为一致）。
   * 传入如 `['about','decided','broke','fixed']` 时不会顺着 `in_workspace` 扩散。
   */
  expandLabels?: string[];
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
  /** 管线是否被 Hook 提前终止 */
  aborted: boolean;
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
 * 每个 `.tdb` 文件同一时刻仅能被一个进程的 TriviumDB 实例打开锁定。
 */
export class TriviumDB {
  /**
   * 打开或创建数据库
   * @param path         数据库文件路径 (如 "data.tdb")
   * @param dim          向量维度，默认为 1536
   * @param dtype        数据类型设定: "f32" | "f16" | "u64", 默认为 "f32"
   * @param syncMode     WAL 同步模式设定: "full" | "normal" | "off", 默认为 "normal"
   */
  constructor(path: string, dim?: number, dtype?: DType, syncMode?: SyncMode);

  // ── Hook 管理 ──

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

  /**
   * 移除这亮点之间的所有边
   * @param src 源节点 ID
   * @param dst 目标节点 ID
   */
  unlink(src: number, dst: number): void;

  /**
   * 图谱上的 N 跳搜索 (广度优先遍历)
   * @param id    起始点
   * @param depth 跳数 (默认 1)
   * @param labels 边标签白名单；省略则沿全部出边
   * @returns 深度之内的所有不重复的周边点 ID
   */
  neighbors(id: number, depth?: number, labels?: string[]): number[];

  /**
   * 获取节点的出边列表
   * @param id 节点 ID
   * @returns 该节点出发的所有有向边
   */
  getEdges(id: number): JsEdge[];

  /**
   * 获取指向该节点的入边（Reverse Hash Net，O(入度)）
   * @param id 节点 ID
   * @param label 可选标签过滤
   */
  getIncomingEdges(id: number, label?: string): JsIncomingEdge[];

  // ── 检索与查询 ──

  /**
   * 混合检索：向量锚定 + 图谱连带扩散！
   * @param queryVector 查询向量
   * @param topK        向外找多少个最相似锚点向量 (默认 5)
   * @param expandDepth 获取到上述锚点后，在图谱里扩散的跳跃深度 (默认 0，纯粹退化为向量相似度检索)
   * @param minScore    只接受相似度大于这个阈值的搜索命中 (默认 0.5)
   * @param expandLabels 图扩散边标签白名单；省略则沿全部出边
   */
  search(queryVector: Vector, topK?: number, expandDepth?: number, minScore?: number, expandLabels?: string[]): JsSearchHit[];

  /**
   * 认知管线检索：向量锚定 + FISTA 残差寻隐 + PPR 图扩散 + DPP 多样性采样
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
   * @param expandLabels 图扩散边标签白名单；省略则沿全部出边
   */
  searchHybrid(queryVector: Vector, queryText: string, topK?: number, expandDepth?: number, minScore?: number, hybridAlpha?: number, expandLabels?: string[]): JsSearchHit[];

  /**
   * 建立用于双路召回的长文本 BM25 索引
   * @param id   节点 ID
   * @param text 文本内容
   */
  indexText(id: number, text: string): void;

  /**
   * 建立用于精确命中的 AC 自动机高级关键词索引
   * @param id      节点 ID
   * @param keyword 关键词内容
   */
  indexKeyword(id: number, keyword: string): void;

  /**
   * 重编译文本索引与词频，在批量插入或重启后必须调用以生效
   */
  buildTextIndex(): void;

  // ── 属性二级索引 ──

  /**
   * 创建属性索引：对指定 payload 字段建立倒排索引
   *
   * ```js
   * db.createIndex('name')   // 之后 tql('FIND {name: "Alice"} RETURN *') 使用 O(1) 索引
   * ```
   */
  createIndex(field: string): void;

  /** 删除属性索引（查询仍可用，退化为全扫描） */
  dropIndex(field: string): void;

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

  /** 无人值守: 后台每隔 x 秒去自动压缩落排 (如果数据正在高频大吞吐) */
  enableAutoCompaction(intervalSecs?: number): void;

  /** 关闭后台的定期压缩 */
  disableAutoCompaction(): void;

  /** 手动触发全量压实（阻塞当前线程） */
  compact(): void;

  /** 当估计的内存占用超过了这个 MB 阈值时会强制落排。填 0 = 不限制 */
  setMemoryLimit(mb: number): void;

  /** 查询估算的内存占用总量 Bytes */
  estimatedMemory(): number;

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

  /** 显式关闭数据库（落盘后释放资源） */
  close(): void;

  /** 获取设置的浮点格式 (f32, f16, u64) */
  get dtype(): string;
}
