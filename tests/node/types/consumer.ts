import {
  TriviumDB, ResourceRuntime, ConsoleSession, type TriviumDBOptions, type TransactionOperation,
  type JsSearchHit, type PreparedTql, type ShutdownStatus, type StorageInfo,
  type TriviumDBError, type ErrorInteger, type DType,
} from 'triviumdb';

function check(condition: boolean, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

export function inspectCoreError(error: TriviumDBError): void {
  if (error.code === 'TDB_DTYPE_MISMATCH') {
    const stored: DType = error.details.stored;
    const requested: DType = error.details.requested;
    void [stored, requested];
    // @ts-expect-error 类型名采用字符串枚举。
    const invalid: number = error.details.stored;
    void invalid;
  }
  if (error.code === 'TDB_DIMENSION_MISMATCH') {
    const expected: ErrorInteger = error.details.expected;
    const got: ErrorInteger = error.details.got;
    void [expected, got];
    // @ts-expect-error 维度错误详情没有参数名字段。
    error.details.parameter;
  }
  if (error.code === 'TDB_MISSING_QUERY_PARAMETER') {
    const parameter: string = error.details.parameter;
    void parameter;
    // @ts-expect-error 参数名是字符串。
    const invalid: number = error.details.parameter;
    void invalid;
  }
}

export async function exercise(file: string): Promise<void> {
  const runtime = new ResourceRuntime({ memoryLimitMb: 64, maxThreads: 1 });
  const options: TriviumDBOptions = {
    dim: 2, dtype: 'f32', storageMode: 'mmap', syncMode: 'full',
    autoBuildQuiver: false, loadTextIndex: true, memoryLimitMb: 8, runtime,
  };
  const db = new TriviumDB(file, options);
  const console = new ConsoleSession();
  const response: string = db.consoleRequest(console, '{"version":1,"requestId":"q1","command":{"operation":"info"}}');
  const detached: boolean = console.isDetached();
  const cancelled: boolean = console.cancel('q1');
  void [response, detached, cancelled];
  console.detach();
  try {
    const operations: TransactionOperation[] = [
      { type: 'insertWithId', id: 11, vector: [1, 0], payload: { kind: 'primary' } },
      { type: 'insertWithId', id: 22, vector: [0, 1], payload: { kind: 'neighbor' } },
    ];
    const inserted: number[] = db.commitTransaction(operations);
    check(JSON.stringify(inserted) === '[11,22]', '事务返回的节点 ID 与写入事实不符');
    db.upsertEdge(11, 22, 'next', 0.5, { note: 'typed-metadata' });
    const edgeMetadata: unknown = db.getEdge(11, 22, 'next')?.metadata;
    const incomingMetadata: unknown = db.getIncomingEdges(22)[0].metadata;
    check(JSON.stringify(edgeMetadata) === '{"note":"typed-metadata"}', '出边元数据与样例不符');
    check(JSON.stringify(incomingMetadata) === '{"note":"typed-metadata"}', '入边元数据与样例不符');
    db.createIndex('kind');
    db.rebuildTextIndex([{ id: 11, text: 'primarytoken', keywords: ['primaryalias'] }]);
    const prepared: PreparedTql = db.prepareTql('FIND {kind: "primary"} AS n WITH n RETURN n, $label AS label');
    const rows = db.executePreparedTql(prepared, { label: 'verified' });
    check(rows.length === 1 && rows[0].n.id === '11' && rows[0].n.payload.kind === 'primary' && rows[0].label === 'verified',
      'Prepared TQL 的节点 ID 或 Payload 与独立样例不符');
    const hits: JsSearchHit[] = db.searchExact([1, 0], 2);
    check(hits.length === 2, '精确向量查询必须返回两个样例节点');
    check(hits[0].id === 11 && hits[0].score === 1 && hits[0].payload.kind === 'primary',
      '主向量的 ID、分数或 Payload 与正交样例不符');
    check(hits[1].id === 22 && hits[1].score === 0 && hits[1].payload.kind === 'neighbor',
      '正交向量的 ID、分数或 Payload 与样例不符');
    const batches: JsSearchHit[][] = await db.searchBatch([[1, 0]], 1, 1, 0);
    check(batches.length === 1 && batches[0].length === 1 && batches[0][0].id === 11,
      '异步批查询的嵌套结果与样例不符');
    const info: StorageInfo = db.storageInfo();
    check(info.dim === 2 && info.node_count === 2, '存储诊断与样例不符');
    db.flush();
    const status: ShutdownStatus = db.requestShutdown();
    check(['draining', 'flushing', 'closed'].includes(status.phase), '关机状态未进入排空阶段');
    const closed: boolean = db.closeTimeout(5000);
    check(closed, '样例数据库未在五秒期限内关闭');
    const reader = new TriviumDB(file, { ...options, accessMode: 'readOnly' });
    try {
      const node = reader.get<{ kind: string }>(11);
      check(node !== null && node.id === 11 && node.payload.kind === 'primary', '持久化节点内容不符');
      const text = reader.tql('TEXT AC "primaryalias" TOP 10 AS hits WITH hits RETURN hits');
      check(text.length === 1 && text[0].hits.id === '11', '持久化关键词关联到错误节点');
    } finally {
      reader.close();
    }
  } finally {
    db.close();
  }
}

// 此函数只参与编译；每个错误样例都必须被声明拒绝，缺少诊断会触发 TS2578。
export function rejectedInputs(db: TriviumDB): void {
  // @ts-expect-error 配置通过 JSON 字符串传递。
  new ConsoleSession({ readOnly: true });
  // @ts-expect-error 请求必须是 JSON 字符串。
  db.consoleRequest(new ConsoleSession(), {});
  // @ts-expect-error 数据类型只接受公开的三种枚举值。
  new TriviumDB('sample.tdb', { dtype: 'float32' });
  // @ts-expect-error 访问模式必须使用公开枚举。
  new TriviumDB('sample.tdb', { accessMode: 'writeOnly' });
  // @ts-expect-error 配额单位为数值。
  new ResourceRuntime({ memoryLimitMb: '64' });
  // @ts-expect-error 节点 ID 必须为数值。
  db.insertWithId('11', [1, 0], {});
  // @ts-expect-error 向量元素必须为数值。
  db.updateVector(11, ['1', '0']);
  // @ts-expect-error 关键词为字符串数组。
  db.rebuildTextIndex([{ id: 11, text: 'token', keywords: [42] }]);
  // @ts-expect-error Prepared 参数值为公开的标量集合。
  db.executePreparedTql(db.prepareTql('FIND {kind: "primary"} RETURN $label AS label'), { label: {} });
  // @ts-expect-error 事务操作必须提供对应的向量。
  db.commitTransaction([{ type: 'insertWithId', id: 11, payload: {} }]);
  // @ts-expect-error 批查询返回 Promise。
  const synchronous: JsSearchHit[][] = db.searchBatch([[1, 0]]);
  // @ts-expect-error 关闭超时返回布尔值。
  const closed: string = db.closeTimeout(0);
  void synchronous;
  void closed;
}
