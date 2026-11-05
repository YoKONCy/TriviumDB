/* Console RPC v1 数据适配器；独立于 DOM，宿主负责数据库与会话生命周期。 */
(function (root) {
  'use strict';
  function failure(code, message, details = {}, status = 400, schema_version = 1, diagnostic = {}) {
    const error = Object.assign(new Error(message), { schema_version, code, details, status });
    if (diagnostic.stage != null) error.stage = diagnostic.stage;
    if (diagnostic.target != null) error.target = diagnostic.target;
    if (diagnostic.write != null) error.write = diagnostic.write;
    return error;
  }
  function nodeId(value) {
    if (typeof value === 'number' && !Number.isSafeInteger(value)) {
      throw failure('TDB_INVALID_INPUT', '节点 ID 精度丢失 (Node ID precision lost)');
    }
    const id = String(value);
    if (!/^(0|[1-9][0-9]{0,19})$/.test(id) || BigInt(id) > 18446744073709551615n) {
      throw failure('TDB_INVALID_INPUT', '节点 ID 无效 (Invalid node ID)');
    }
    return id;
  }
  function operationsCopy(operations) {
    return JSON.parse(JSON.stringify(operations)).map(op => {
      for (const key of ['id', 'source', 'target']) if (op[key] !== undefined) op[key] = nodeId(op[key]);
      return op;
    });
  }
  function freezeTransaction(operations, revision, key) {
    if (typeof revision !== 'string' || !revision || !Array.isArray(operations) || !operations.length) {
      throw failure('TDB_WRITE_CONFLICT', '请重新读取并审阅修改 (Read and review changes again)', {}, 409);
    }
    if (typeof key !== 'string' || !key || new TextEncoder().encode(key).length > 128) {
      throw failure('TDB_INVALID_INPUT', '幂等键长度无效 (Invalid idempotency key length)');
    }
    return JSON.stringify({ operation: 'guardedTransaction', expectedRevision: revision,
      idempotencyKey: key, operations: operationsCopy(operations) });
  }
  function errorStatus(code) {
    if (['TDB_WRITE_CONFLICT', 'TDB_IDEMPOTENCY_CONFLICT', 'TDB_WRITE_OUTCOME_UNKNOWN', 'TDB_REENTRANT_ACCESS', 'TDB_TRANSACTION_FINISHED'].includes(code)) return 409;
    if (code === 'TDB_BUSY') return 503;
    if (code === 'TDB_READ_ONLY') return 403;
    if (['TDB_DATABASE_CLOSED', 'TDB_CONSOLE_SESSION_CLOSED'].includes(code)) return 410;
    if (['TDB_IO', 'TDB_WAL_CLOSED', 'TDB_RECOVERY_REQUIRED', 'TDB_CONSOLE_TASK_FAILED', 'TDB_HOST_LOCK_POISONED'].includes(code)) return 500;
    return 400;
  }
  function httpTransport({ endpoint, headers = {}, credentials = 'same-origin', detach }) {
    const send = async (url, init) => {
      const response = await fetch(url, { credentials, ...init,
        headers: { ...headers, 'Content-Type': 'application/json' } });
      const body = await response.json();
      if (!response.ok && !body.error) throw failure('TDB_CONSOLE_TRANSPORT',
        '控制台 HTTP 请求失败 (Console HTTP request failed)', {}, response.status);
      return body;
    };
    return {
      request: raw => send(endpoint, { method: 'POST', body: raw }),
      cancel: id => send(endpoint.replace(/\/$/, '') + '/cancel/' + encodeURIComponent(id), { method: 'POST' }),
      detach,
    };
  }
  function create(options) {
    if (!options || typeof options.request !== 'function') throw new TypeError('需要请求桥接 (Request bridge required)');
    let closed = false, sequence = 0, tail = Promise.resolve(), disposing = null;
    const prefix = Array.from(globalThis.crypto.getRandomValues(new Uint32Array(4)), value => value.toString(16).padStart(8, '0')).join('');
    const pending = new Map();
    const client = { info: null };
    client.call = (command, { signal } = {}) => {
      if (closed) return Promise.reject(failure('TDB_CONSOLE_SESSION_CLOSED', '控制台已卸载 (Console detached)', {}, 410));
      const requestId = prefix + '-' + (++sequence);
      // 排队前复制请求，调用方继续编辑也不能改写已准入内容。
      const raw = JSON.stringify({ version: 1, requestId, command });
      let started = false;
      const cancel = () => {
        if (started && typeof options.cancel === 'function') Promise.resolve().then(() => options.cancel(requestId)).catch(() => {});
      };
      signal?.addEventListener('abort', cancel, { once: true });
      const task = tail.then(async () => {
        if (closed || signal?.aborted) throw new DOMException('请求已取消 (Request cancelled)', 'AbortError');
        started = true;
        let response;
        try { response = await options.request(raw); } catch (error) {
          // 原生 SDK 直接抛结构化错误，HTTP 桥接则返回 error 信封。
          if (error.code && !error.status) throw failure(error.code, error.message, error.details || {}, errorStatus(error.code), error.schema_version ?? 1, error);
          throw error;
        }
        if (typeof response === 'string') response = JSON.parse(response);
        if (response?.error) {
          const e = response.error;
          throw failure(e.code, e.message, e.details, errorStatus(e.code), e.schema_version ?? 1, e);
        }
        if (response?.version !== 1 || response.requestId !== requestId || !Object.hasOwn(response, 'data')) {
          throw failure('TDB_CONSOLE_TRANSPORT', '控制台响应身份无效 (Invalid console response identity)', {}, 502);
        }
        if (command.operation === 'info') client.info = response.data;
        return response.data;
      }).finally(() => {
        signal?.removeEventListener('abort', cancel);
        pending.delete(requestId);
      });
      pending.set(requestId, cancel);
      tail = task.catch(() => {});
      return task;
    };
    client.submit = async (body, confirmOnly = false) => {
      const command = JSON.parse(body);
      if (confirmOnly) command.operation = 'transactionStatus';
      const result = await client.call(command);
      if (confirmOnly && result.outcome === 'unknown') throw failure('TDB_WRITE_OUTCOME_UNKNOWN',
        '原事务结果待核对，已保留完整请求 (Original outcome needs reconciliation; complete request retained)', {}, 409);
      if ((confirmOnly && result.outcome !== 'committed') || !Array.isArray(result.createdIds)
          || typeof result.revision !== 'string') {
        throw failure('TDB_CONSOLE_TRANSPORT', '事务确认响应无效 (Invalid transaction acknowledgement)', {}, 502);
      }
      return result;
    };
    client.dispose = () => {
      if (disposing) return disposing;
      closed = true;
      for (const cancel of pending.values()) cancel();
      // detach 先阻止宿主接收新请求，再等待已准入请求完成；数据库关闭由宿主执行。
      disposing = Promise.allSettled([Promise.resolve().then(() => options.detach?.()), tail]).then(results => {
        if (results[0].status === 'rejected') {
          disposing = null;
          throw results[0].reason;
        }
      });
      return disposing;
    };
    const getNodes = (ids, signal) => client.call({ operation: 'getNodes', ids: ids.map(nodeId), withRevision: true }, { signal });
    client.fetch = async (input, init = {}) => {
      const url = new URL(input, 'http://console.invalid');
      const path = url.pathname, params = url.searchParams;
      const call = command => client.call(command, { signal: init.signal });
      const node = value => ({ ...value, type: 'node', edges: (value.edges || []).map(({ target, ...rest }) => ({ ...rest, targetId: target })) });
      const edge = value => ({ ...value, type: 'edge' });
      const json = (data, status = 200) => new Response(JSON.stringify(data), { status, headers: { 'Content-Type': 'application/json' } });
      try {
        let data, match;
        // batch-get 的 ID 在调用点以十进制字符串发送，禁止经 Number 中转。
        const body = typeof init.body === 'string' ? JSON.parse(init.body) : {};
        if (path === '/health/ready' || path === '/health/details') {
          const info = await call({ operation: 'info' });
          data = { status: 'ready', version: info.packageVersion, console: true };
        } else if (path === '/v1/tql') {
          data = await call(body.mutation ? { operation: 'mutate', query: body.query }
            : { operation: 'query', query: body.query, parameters: body.parameters || {} });
          if (!body.mutation) {
            // 仅依据协议列类型转换，Payload 内长得像节点的对象保持原值。
            data.rows = data.rows.map(row => {
              const output = { ...row };
              for (const column of data.columns) {
                const value = row[column.name];
                if (value == null) continue;
                if (column.kind === 'node') output[column.name] = node(value);
                if (column.kind === 'path') output[column.name] = { type: 'path', value };
              }
              return output;
            });
          }
        } else if (path === '/v1/nodes/batch-get') {
          data = await getNodes(body.ids, init.signal);
          data.nodes = data.nodes.map(node);
        } else if ((match = path.match(/^\/v1\/nodes\/(\d+)$/))) {
          const snapshot = await getNodes([match[1]], init.signal);
          if (!snapshot.nodes.length) throw failure('TDB_NODE_NOT_FOUND', '节点不存在 (Node not found)', { id: match[1] }, 404);
          data = { node: node(snapshot.nodes[0]), version: snapshot.revision };
        } else if (path === '/v1/indexes') {
          data = await call({ operation: 'indexes' });
        } else if (path === '/v1/indexes/delete') {
          data = await call({ operation: 'manageIndex', kind: body.kind, fields: body.fields, create: false });
        } else if (path === '/v1/indexes/quiver') {
          data = init.method === 'POST' ? await call({ operation: 'buildQuiver' }) : { status: 'unknown' };
        } else if (path === '/v1/search/vector') {
          if (client.info?.dtype === 'u64') throw failure('TDB_INVALID_INPUT',
            '二进制向量请使用 TQL 查询 (Use TQL for binary vectors)');
          const bytes = init.body;
          const view = ArrayBuffer.isView(bytes) ? new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength) : new DataView(bytes);
          if (!view.byteLength || view.byteLength % 4) throw failure('TDB_INVALID_INPUT', '向量字节长度无效 (Invalid vector byte length)');
          const vector = Array.from({ length: view.byteLength / 4 }, (_, i) => view.getFloat32(i * 4, true));
          if (vector.some(value => !Number.isFinite(value))) throw failure('TDB_INVALID_INPUT', '向量必须为有限值 (Vector values must be finite)');
          data = await call({ operation: 'search', vector, topK: Number(params.get('top_k') || 10),
            minScore: Number(params.get('min_score') ?? -1), expandDepth: Number(params.get('expand_depth') || 0) });
        } else if ((match = path.match(/^\/v1\/nodes\/(\d+)\/neighbors$/))) {
          if (params.has('direction') && params.get('direction') !== 'out') throw failure('TDB_INVALID_INPUT',
            '嵌入式邻域按出边扩展 (Embedded neighborhoods follow outgoing edges)');
          data = await call({ operation: 'subgraph', source: match[1], depth: Number(params.get('depth') || 1),
            maxNodes: Math.min(Number(params.get('limit') || 1000), client.info?.limits.maxOperations || 1000) });
          data.nodes = data.nodes.map(node); data.edges = data.edges.map(edge);
        } else if ((match = path.match(/^\/v1\/nodes\/(\d+)\/edges$/))) {
          const outgoing = await call({ operation: 'edges', node: match[1] });
          const incoming = await call({ operation: 'edges', node: match[1], incoming: true });
          const all = [...outgoing.edges, ...incoming.edges.filter(e => e.source !== match[1])];
          const offset = Number(params.get('offset') || 0), limit = Number(params.get('limit') || 100);
          data = { edges: all.slice(offset, offset + limit).map(edge), total: all.length, hasMore: offset + limit < all.length };
        } else if ((match = path.match(/^\/v1\/nodes\/(\d+)\/paths\/to\/(\d+)$/))) {
          const result = await call({ operation: 'path', source: match[1], target: match[2], maxDepth: Number(params.get('maxHops') || 3) });
          const edges = [];
          for (let i = 0; result.path && i + 1 < result.path.length; i++) {
            const part = await call({ operation: 'edges', node: result.path[i] });
            const hit = part.edges.find(e => e.target === result.path[i + 1]);
            if (!hit) throw failure('TDB_WRITE_CONFLICT', '路径已变化，请重新查询 (Path changed; query again)', {}, 409);
            edges.push(edge(hit));
          }
          data = { paths: result.path ? [{ nodes: result.path, edges }] : [], truncated: false };
        } else {
          throw failure('TDB_INVALID_INPUT', '此操作需要控制台事务入口 (Use the console transaction entry for this operation)');
        }
        return json(data);
      } catch (error) {
        if (error.name === 'AbortError' || !error.code) throw error;
        return json({ schema_version: error.schema_version ?? 1, code: error.code, title: error.code, message: error.message, detail: error.message, details: error.details, stage: error.stage, target: error.target, write: error.write }, error.status || 500);
      }
    };
    return client;
  }
  const api = { create, httpTransport, nodeId, freezeTransaction };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else root.TriviumConsoleClient = api;
})(globalThis);
