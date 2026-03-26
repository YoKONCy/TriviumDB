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

export interface JsNodeView<T = any> {
  /** 节点 ID */
  id: number;
  /** 特征向量 */
  vector: Vector;
  /** 节点 JSON 元数据 */
  payload: T;
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
   * @returns 深度之内的所有不重复的周边点 ID
   */
  neighbors(id: number, depth?: number): number[];

  // ── 检索与查询 ──

  /**
   * 混合检索：向量锚定 + 图谱连带扩散！
   * @param queryVector 查询向量
   * @param topK        向外找多少个最相似锚点向量 (默认 5)
   * @param expandDepth 获取到上述锚点后，在图谱里扩散的跳跃深度 (默认 0，纯粹退化为向量相似度检索)
   * @param minScore    只接受相似度大于这个阈值的搜索命中 (默认 0.5)
   */
  search(queryVector: Vector, topK?: number, expandDepth?: number, minScore?: number): JsSearchHit[];

  /**
   * 像类 MongoDB 一样去条件匹配！
   * @param condition 类似 { age: { $gt: 18 } } 或 { $and: [...] }
   */
  filterWhere(condition: FilterCondition): JsNodeView[];

  /**
   * 类似 neo4j 这样的图谱专用语法检索引擎！
   * @param cypherQuery eg: `MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b`
   */
  query(cypherQuery: string): QueryRow[];

  // ── 辅助与生命周期 ──

  /** 手动把记录在内存中的所有东西强制安全落盘 */
  flush(): void;

  /** 动态在运行时调整同步安全性 */
  setSyncMode(mode: SyncMode): void;

  /** 无人值守: 后台每隔 x 秒去自动压缩落排 (如果数据正在高频大吞吐) */
  enableAutoCompaction(intervalSecs?: number): void;

  /** 关闭后台的定期压缩 */
  disableAutoCompaction(): void;

  /** 当估计的内存占用超过了这个 MB 阈值时会强制落排。填 0 = 不限制 */
  setMemoryLimit(mb: number): void;

  /** 查询估算的内存占用总量 Bytes */
  estimatedMemory(): number;

  /** 所有被存入库里的所有 ID 的乱序数组 */
  allNodeIds(): number[];

  /** 在启用 HNSW 模式时使用，在批量灌量数据完成时调用 */
  rebuildIndex(): void;

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

  /** 获取设置的浮点格式 (f32, f16, u64) */
  get dtype(): string;
}
