'use strict';

// 非 UI 公共契约：通过真实原生 Core 验证传输编码、冲突、整批回执与卸载排空。
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { TriviumDB, ConsoleSession, RequestContext } = require('../../index.js');
const { create, httpTransport, nodeId, freezeTransaction } = require('../../crates/triviumdb-server/web/console-client.js');

async function httpCompletion() {
  const originalFetch = globalThis.fetch;
  const disconnected = Object.assign(new Error('连接中断 (Connection interrupted)'), { code: 'ECONNRESET' });
  let respond;
  globalThis.fetch = (url, init) => respond(url, init);
  const client = create(httpTransport({ endpoint: 'http://console/v1/console' }));
  respond = async (_url, init) => {
    const wire = JSON.parse(init.body);
    const encoded = new Headers(init.headers).get('X-TDB-Request-ID');
    assert.match(encoded, /^[\x20-\x7e]+$/);
    assert.equal(JSON.parse(encoded), wire.requestId);
    return new Response(JSON.stringify({ version: 1, requestId: wire.requestId, data: { ok: true } }));
  };
  assert.deepEqual(await client.call({ operation: 'info' }), { ok: true });
  const unicodeWire = JSON.stringify({ version: 1, requestId: '请求🐱', command: { operation: 'info' } });
  assert.deepEqual(await httpTransport({ endpoint: 'http://console/v1/console', headers: { 'x-tdb-request-id': 'stale' } })
    .request(unicodeWire, { requestId: '请求🐱' }), { version: 1, requestId: '请求🐱', data: { ok: true } });
  try {
    const writes = require('../../src/database/console_contract.json').writeOperations;
    for (const operation of ['info', ...writes]) {
      for (const mode of ['connection', 'json', 'identity', 'error-identity', 'error-shape']) {
        respond = async (_url, init) => {
          if (mode === 'connection') throw disconnected;
          if (mode === 'json') return new Response('{', { status: 200 });
          const body = { version: 1, requestId: '其他请求', data: {} };
          if (mode === 'error-identity') body.error = { schema_version: 1, code: 'TDB_BUSY', message: '繁忙 (Busy)', details: {} };
          if (mode === 'error-shape') { body.requestId = JSON.parse(init.body).requestId; body.error = {}; }
          return new Response(JSON.stringify(body), { status: mode.startsWith('error-') ? 503 : 200 });
        };
        await assert.rejects(client.call({ operation, query: 'CREATE ({})' }), error => {
          assert.equal(error.code, 'TDB_CONSOLE_TRANSPORT', mode);
          assert.equal(error.status, 502, mode);
          assert.deepEqual(error.write, operation !== 'info' ? { phase: 'response', outcome: 'unknown' } : undefined, mode);
          if (mode === 'connection') assert.equal(error.cause, disconnected);
          if (mode === 'json') assert.ok(error.cause instanceof SyntaxError);
          return true;
        });
      }
    }
    const coreError = { schema_version: 1, code: 'TDB_DIMENSION_MISMATCH', message: '维度不匹配 (Dimension mismatch)',
      details: { expected: 2, got: 1 }, stage: 'plan', target: { query_span: { start_byte: 10, end_byte: 30, line: 1, column: 11 } },
      write: { phase: 'validation', outcome: 'not_committed' } };
    respond = async () => new Response(JSON.stringify({ version: 1, error: coreError }), { status: 422 });
    await assert.rejects(client.call({ operation: 'mutate', query: 'MATCH (n) SET VECTOR(n) == [1]' }), error => {
      for (const field of ['schema_version', 'code', 'message', 'details', 'stage', 'target', 'write']) assert.deepEqual(error[field], coreError[field]);
      assert.equal(error.status, 422);
      return true;
    });
    let calls = 0;
    respond = async () => { calls++; throw disconnected; };
    const controller = new AbortController();
    controller.abort();
    await assert.rejects(client.call({ operation: 'mutate' }, { signal: controller.signal }), error => {
      assert.equal(error.name, 'AbortError');
      assert.equal(error.code, 'TDB_QUERY_CANCELLED');
      assert.deepEqual(error.write, { phase: 'admission', outcome: 'not_committed' });
      return true;
    });
    assert.equal(calls, 0);
    const command = { operation: 'mutate', query: 'CREATE ({})' };
    const inFlight = client.call(command);
    command.operation = 'info';
    await assert.rejects(inFlight, error => {
      assert.deepEqual(error.write, { phase: 'response', outcome: 'unknown' });
      return true;
    });
    assert.equal(calls, 1);
    const activeController = new AbortController();
    respond = async (url, init) => {
      if (url.includes('/cancel/')) return new Response(JSON.stringify({ cancelled: false }));
      assert.equal(init.signal, activeController.signal);
      activeController.abort();
      init.signal.throwIfAborted();
      throw new Error('活动请求必须观察取消 (Active request must observe cancellation)');
    };
    await assert.rejects(client.call({ operation: 'mutate' }, { signal: activeController.signal }), error => {
      assert.equal(error.name, 'AbortError');
      assert.equal(error.code, 'TDB_QUERY_CANCELLED');
      assert.deepEqual(error.write, { phase: 'response', outcome: 'unknown' });
      assert.equal(error.cause.name, 'AbortError');
      return true;
    });
  } finally {
    await client.dispose();
    globalThis.fetch = originalFetch;
  }
}

async function queryDiagnostics(root) {
  const file = path.join(root, 'diagnostics.tdb');
  const db = new TriviumDB(file, { dim: 2, autoBuildQuiver: false });
  const { cases } = require('../contracts/query_diagnostics.json');
  try {
    db.insertWithId(7, [1, 0], { saved: '完整事实' });
    db.createUniqueIndex('saved');
    // 同一真实内核错误分别通过原生异常与 JSON 信封穿过传输边界。
    for (const envelope of [false, true]) {
      const session = new ConsoleSession('{"readOnly":false}');
      const client = create({
        request(raw) {
          try { return db.consoleRequest(session, raw); } catch (error) {
            if (!envelope) throw error;
            return JSON.stringify({ version: 1, error: {
              schema_version: error.schema_version, code: error.code, message: error.message,
              details: error.details, stage: error.stage, target: error.target, write: error.write,
            } });
          }
        },
        detach() { session.detach(); },
      });
      try {
        for (const test of cases) {
          const check = error => {
            assert.equal(error.schema_version, 1);
            assert.equal(error.code, 'TDB_QUERY_PARSE');
            assert.equal(error.stage, test.stage, test.name);
            assert.deepEqual(error.write, test.mutation ? { phase: 'validation', outcome: 'not_committed' } : undefined);
            assert.deepEqual(error.target, test.span ? { query_span: test.span } : undefined, test.name);
            assert.deepEqual(Object.keys(error.details), ['reason']);
            assert.equal(typeof error.details.reason, 'string');
            return true;
          };
          await assert.rejects(client.call({ operation: test.mutation ? 'mutate' : 'query', query: test.query }), check);
          const response = await client.fetch('http://console/v1/tql', {
            method: 'POST', body: JSON.stringify({ query: test.query, mutation: !!test.mutation }),
          });
          assert.equal(response.status, 400);
          check(await response.json());
        }
        await assert.rejects(client.call({ operation: 'transaction', operations: [
          { op: 'insert', vector: [1], payload: { state: 'rejected' } },
        ] }), error => {
          assert.equal(error.code, 'TDB_DIMENSION_MISMATCH');
          assert.deepEqual(error.details, { expected: 2, got: 1 });
          assert.deepEqual(error.write, require('../contracts/write_outcomes.json').validation);
          return true;
        });
        const writeResponse = await client.fetch('http://console/v1/tql', {
          method: 'POST', body: JSON.stringify({ mutation: true, query: 'MATCH (n) CREATE (m {saved: "完整事实"})' }),
        });
        assert.equal(writeResponse.status, 400);
        const writeError = await writeResponse.json();
        assert.equal(writeError.code, 'TDB_UNIQUE_CONSTRAINT');
        assert.deepEqual(writeError.details, { fields: ['saved'], existing_id: 7, conflicting_id: 8 });
        assert.deepEqual(writeError.write, require('../contracts/write_outcomes.json').validation);
        const planResponse = await client.fetch('http://console/v1/tql', {
          method: 'POST', body: JSON.stringify({ mutation: true, query: 'MATCH (n) SET VECTOR(n) == [1]' }),
        });
        assert.equal(planResponse.status, 400);
        const planError = await planResponse.json();
        assert.equal(planError.code, 'TDB_DIMENSION_MISMATCH');
        assert.deepEqual(planError.details, { expected: 2, got: 1 });
        assert.deepEqual(planError.write, require('../contracts/write_outcomes.json').direct_validation);
        for (const test of require('../contracts/prepared_diagnostics.json').cases) {
          const check = error => {
            assert.equal(error.schema_version, 1);
            assert.equal(error.code, test.code);
            assert.equal(error.stage, 'bind');
            const target = { parameter: test.parameter };
            if (test.span) target.query_span = test.span;
            assert.deepEqual(error.target, target, test.name);
            if (test.details) assert.deepEqual(error.details, test.details);
            else {
              assert.deepEqual(Object.keys(error.details), ['reason']);
              assert.ok(typeof error.details.reason === 'string' && error.details.reason.length > 0);
            }
            return true;
          };
          const command = { operation: 'query', query: test.query, parameters: test.parameters };
          await assert.rejects(client.call(command), check);
          const response = await client.fetch('http://console/v1/tql', { method: 'POST', body: JSON.stringify(command) });
          assert.equal(response.status, 400);
          check(await response.json());
        }
      } finally {
        await client.dispose();
      }
    }
  } finally {
    db.close();
  }
  const reopened = new TriviumDB(file, { dim: 2, autoBuildQuiver: false });
  try {
    assert.deepEqual(reopened.allNodeIds(), [7]);
    const node = reopened.get(7);
    assert.deepEqual(node.vector, [1, 0]);
    assert.deepEqual(node.payload, { saved: '完整事实' });
    assert.deepEqual(node.edges, []);
  } finally {
    reopened.close();
  }
}

async function bridgeCompletion() {
  let drainCalls = 0, releaseDrain, enteredDrain;
  const draining = new Promise(resolve => { releaseDrain = resolve; });
  const drainEntered = new Promise(resolve => { enteredDrain = resolve; });
  const metadata = [];
  const bridged = create({
    request(raw, context) {
      assert.equal(Object.isFrozen(context), true);
      assert.equal(context.requestId, JSON.parse(raw).requestId);
      metadata.push([context.operation, context.write]);
      return { version: 1, requestId: context.requestId, data: {} };
    },
    drain() { drainCalls++; enteredDrain(); return draining; },
  });
  await bridged.call({ operation: 'info' });
  await bridged.call({ operation: 'transaction', operations: [] });
  assert.deepEqual(metadata, [['info', false], ['transaction', true]]);
  let disposed = false;
  const disposingBridge = bridged.dispose().then(() => { disposed = true; });
  await drainEntered;
  assert.equal(disposed, false);
  releaseDrain();
  await disposingBridge;
  assert.equal(drainCalls, 1);
  let drainAttempts = 0;
  const retryDrain = create({ request() {}, drain() {
    if (++drainAttempts === 1) throw new Error('宿主排空失败');
  } });
  await assert.rejects(retryDrain.dispose(), /宿主排空失败/);
  await retryDrain.dispose();
  assert.equal(drainAttempts, 2);
  let hostDrained = false;
  const negativeDrain = create({ request() {}, drain: () => hostDrained });
  await assert.rejects(negativeDrain.dispose(), error => error.code === 'TDB_BUSY' && error.details.resource === 'console_bridge_drain');
  hostDrained = true;
  await negativeDrain.dispose();
  const invalidDrain = create({ request() {}, drain: () => ({ drained: true }) });
  await assert.rejects(invalidDrain.dispose(), error => error.code === 'TDB_CONSOLE_TRANSPORT');
  const rejectedPrimitive = create({ request() { return Promise.reject(null); } });
  await assert.rejects(rejectedPrimitive.call({ operation: 'transaction', operations: [] }), error => {
    assert.equal(error.code, 'TDB_CONSOLE_TRANSPORT');
    assert.equal(error.cause, null);
    assert.deepEqual(error.write, { phase: 'response', outcome: 'unknown' });
    return true;
  });
  await rejectedPrimitive.dispose();
}

async function main() {
  await bridgeCompletion();
  await httpCompletion();
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tdb-console-transport-'));
  await queryDiagnostics(root);
  const file = path.join(root, 'data.tdb');
  let db = new TriviumDB(file, { dim: 2, autoBuildQuiver: false });
  const session = new ConsoleSession('{"readOnly":false}');
  const commands = [];
  let responseFailure = null, detached = 0;
  const client = create({
    request(raw, context) {
      const command = JSON.parse(raw).command;
      commands.push(command);
      const incoming = new RequestContext(context.requestId);
      const abort = () => incoming.cancel();
      context.signal?.addEventListener('abort', abort, { once: true });
      let result;
      try { result = incoming.run(() => db.consoleRequest(session, raw)); }
      finally { context.signal?.removeEventListener('abort', abort); }
      if (responseFailure && ['guardedTransaction', 'transactionStatus'].includes(command.operation)) {
        const mode = responseFailure;
        responseFailure = null;
        if (mode === 'json') return '{';
        if (mode === 'identity') return { ...JSON.parse(result), requestId: '错误身份' };
        if (mode === 'receipt') return { ...JSON.parse(result), data: {} };
        throw new Error('模拟响应交付失败 (Simulated response delivery failure)');
      }
      return result;
    },
    detach() { detached++; session.detach(); },
      drain() { assert.equal(session.isDrained(), true); },
  });
  try {
    const info = await client.call({ operation: 'info' });
    assert.equal(info.readOnly, false);
    await client.call({ operation: 'transaction', operations: [
      { op: 'insert', id: '1', vector: [1, 0], payload: { content: '原始正文' } },
      { op: 'insert', id: '9007199254740993', vector: [0, 1], payload: { content: '大 ID' } },
      { op: 'link', source: '1', target: '9007199254740993', label: 'next', weight: 1 },
    ] });
    const read = async () => (await client.fetch('http://console/v1/nodes/1')).json();
    const old = await read();
    assert.equal(typeof old.version, 'string');
    assert.deepEqual(old.node.payload, { content: '原始正文' });
    assert.deepEqual(old.node.edges, [{ targetId: '9007199254740993', label: 'next', weight: 1, metadata: null }]);
    db.updatePayload(1, { content: '宿主更新' });
    const conflict = freezeTransaction([{ op: 'updatePayload', id: '1', payload: { content: '旧草稿' } }], old.version, 'old-draft');
    await assert.rejects(client.submit(conflict), error => error.code === 'TDB_WRITE_CONFLICT' && error.status === 409 && error.schema_version === 1);
    assert.deepEqual(db.getPayload(1), { content: '宿主更新' });

    const fresh = await read();
    const operations = [
      { op: 'updatePayload', id: '1', payload: { content: '提交内容' } },
      { op: 'insert', vector: [1, 1], payload: { content: '同批新增' } },
    ];
    const frozen = freezeTransaction(operations, fresh.version, 'whole-batch');
    operations[0].payload.content = '继续编辑的新草稿';
    responseFailure = 'lost';
    await assert.rejects(client.submit(frozen), error => {
      assert.equal(error.code, 'TDB_CONSOLE_TRANSPORT');
      assert.equal(error.status, 502);
      assert.deepEqual(error.write, { phase: 'response', outcome: 'unknown' });
      assert.match(error.cause.message, /Simulated response delivery failure/);
      return true;
    });
    assert.equal(db.nodeCount(), 3);
    for (const mode of ['json', 'identity', 'receipt']) {
      responseFailure = mode;
      await assert.rejects(client.submit(frozen), error => {
        assert.equal(error.code, 'TDB_CONSOLE_TRANSPORT', mode);
        assert.deepEqual(error.write, { phase: 'response', outcome: 'unknown' }, mode);
        return true;
      });
      const confirmed = await client.submit(frozen, true);
      assert.deepEqual(confirmed.createdIds, ['9007199254740994']);
      assert.equal(confirmed.outcome, 'committed');
      assert.equal(db.nodeCount(), 3);
      assert.deepEqual(db.getPayload(1), { content: '提交内容' });
    }
    responseFailure = 'lost';
    await assert.rejects(client.submit(frozen, true), error => {
      assert.equal(error.code, 'TDB_CONSOLE_TRANSPORT');
      assert.deepEqual(error.write, { phase: 'response', outcome: 'unknown' });
      return true;
    });
    const status = await client.submit(frozen, true);
    assert.deepEqual(status.createdIds, ['9007199254740994']);
    assert.equal(status.outcome, 'committed');
    assert.deepEqual(commands.at(-1), { ...JSON.parse(frozen), operation: 'transactionStatus' });
    assert.deepEqual(db.getPayload(1), { content: '提交内容' });
    assert.deepEqual((await client.submit(frozen)).createdIds, status.createdIds);
    assert.equal(db.nodeCount(), 3);
    const absent = freezeTransaction([{ op: 'delete', id: '1' }], status.revision, 'never-submitted');
    await assert.rejects(client.submit(absent, true), error => error.code === 'TDB_WRITE_OUTCOME_UNKNOWN');
    assert.equal(db.nodeCount(), 3);

    const batch = await (await client.fetch('http://console/v1/nodes/batch-get', {
      body: JSON.stringify({ ids: ['9007199254740993', '1', '7'] }), method: 'POST',
    })).json();
    assert.deepEqual(batch.nodes.map(n => n.id), ['9007199254740993', '1']);
    assert.deepEqual(batch.missingIds, ['7']);
    assert.deepEqual(batch.nodes[0].vector, [0, 1]);
    assert.equal(nodeId('18446744073709551615'), '18446744073709551615');
    assert.throws(() => nodeId(9007199254740992), error => error.code === 'TDB_INVALID_INPUT');
    const query = await (await client.fetch('http://console/v1/tql', {
      method: 'POST', body: JSON.stringify({ query: 'MATCH (n) RETURN n LIMIT 10' }),
    })).json();
    assert.equal(query.rowCount, 3);
    assert.deepEqual(query.rows.map(row => row.n.id).sort(), ['1', '9007199254740993', '9007199254740994']);
    assert.ok(query.rows.every(row => row.n.type === 'node'));
    assert.deepEqual(query.rows.find(row => row.n.id === '1').n.edges,
      [{ targetId: '9007199254740993', label: 'next', weight: 1, metadata: null }]);
    const graph = await (await client.fetch('http://console/v1/nodes/1/neighbors?depth=1&limit=10')).json();
    assert.deepEqual(graph.edges.map(e => [e.source, e.target, e.label]), [['1', '9007199254740993', 'next']]);
    const route = await (await client.fetch('http://console/v1/nodes/1/paths/to/9007199254740993?maxHops=2')).json();
    assert.deepEqual(route.paths[0].nodes, ['1', '9007199254740993']);
    assert.deepEqual(route.paths[0].edges.map(e => [e.source, e.target]), [['1', '9007199254740993']]);
    const missing = await client.fetch('http://console/v1/nodes/7');
    assert.equal(missing.status, 404);
    const missingError = await missing.json();
    assert.equal(missingError.schema_version, 1);
    assert.equal(missingError.code, 'TDB_NODE_NOT_FOUND');
    assert.deepEqual(missingError.details, { id: '7' });
    const hits = await (await client.fetch('http://console/v1/search/vector?top_k=3&min_score=0.9', {
      method: 'POST', body: new Float32Array([1, 0]),
    })).json();
    assert.deepEqual(hits.hits, [{ id: '1', score: 1, payload: { content: '提交内容' } }]);

    const readonlySession = new ConsoleSession();
    const readonly = create({ request: raw => db.consoleRequest(readonlySession, raw), detach: () => readonlySession.detach() });
    await assert.rejects(readonly.submit(frozen), error => {
      assert.equal(error.code, 'TDB_READ_ONLY');
      assert.equal(error.status, 403);
      assert.deepEqual(error.write, { phase: 'admission', outcome: 'not_committed' });
      return true;
    });
    await readonly.dispose();
    await Promise.all([client.dispose(), client.dispose()]);
    assert.equal(detached, 1);
    await assert.rejects(client.call({ operation: 'info' }), error => error.code === 'TDB_CONSOLE_SESSION_CLOSED');
    db.close();
    db = new TriviumDB(file, { dim: 2, autoBuildQuiver: false });
    const reopenedSession = new ConsoleSession('{"readOnly":false}');
    const reopened = create({ request: raw => db.consoleRequest(reopenedSession, raw), detach: () => reopenedSession.detach() });
    assert.deepEqual(await reopened.submit(frozen, true), { outcome: 'committed', revision: status.revision, createdIds: status.createdIds });
    assert.deepEqual(await reopened.submit(frozen), { revision: status.revision, createdIds: status.createdIds, replayed: true });
    assert.deepEqual(db.getPayload(1), { content: '提交内容' });
    assert.equal(db.nodeCount(), 3);
    await reopened.dispose();
  } finally {
    await client.dispose();
    db.close();
    const relative = path.relative(os.tmpdir(), root);
    assert.ok(relative && !relative.startsWith('..') && !path.isAbsolute(relative));
    fs.rmSync(root, { recursive: true, force: true });
  }

  // 使用受控屏障验证取消和排空；固定 sleep 不作为完成证据。
  let release, entered;
  const started = new Promise(resolve => { entered = resolve; });
  const gate = new Promise(resolve => { release = resolve; });
  const cancelled = [];
  let cancelEntered, releaseCancel;
  const cancelStarted = new Promise(resolve => { cancelEntered = resolve; });
  const cancelGate = new Promise(resolve => { releaseCancel = resolve; });
  let count = 0, drainedDetach = 0;
  const blocked = create({
    async request(raw) { count++; const request = JSON.parse(raw); entered(); await gate;
      return { version: 1, requestId: request.requestId, data: { value: '完成' } }; },
    async cancel(id) { cancelled.push(id); cancelEntered(); await cancelGate; },
    detach() { drainedDetach++; },
  });
  const activeController = new AbortController();
  const active = blocked.call({ operation: 'info' }, { signal: activeController.signal });
  await started;
  activeController.abort();
  const queued = blocked.call({ operation: 'transaction', operations: [{ op: 'delete', id: '1' }] });
  const queuedRejected = assert.rejects(queued, error => {
    assert.equal(error.name, 'AbortError');
    assert.equal(error.code, 'TDB_QUERY_CANCELLED');
    assert.deepEqual(error.write, { phase: 'admission', outcome: 'not_committed' });
    return true;
  });
  let disposed = false;
  const disposing = blocked.dispose().then(() => { disposed = true; });
  await cancelStarted;
  assert.equal(disposed, false);
  release();
  assert.deepEqual(await active, { value: '完成' });
  await queuedRejected;
  assert.equal(disposed, false);
  releaseCancel();
  await disposing;
  assert.equal(count, 1);
  assert.equal(cancelled.length, 1);
  assert.equal(drainedDetach, 1);

  let teardownAttempts = 0;
  const retryable = create({ request() { throw new Error('卸载后不应准入'); }, detach() {
    if (++teardownAttempts === 1) throw new Error('模拟卸载失败');
  } });
  await assert.rejects(retryable.dispose(), /模拟卸载失败/);
  await retryable.dispose();
  assert.equal(teardownAttempts, 2);
  await assert.rejects(retryable.call({ operation: 'info' }), error => error.code === 'TDB_CONSOLE_SESSION_CLOSED');

  const malformed = create({ request() { return { version: 1, requestId: '其他请求', data: {} }; } });
  await assert.rejects(malformed.call({ operation: 'info' }), error => error.code === 'TDB_CONSOLE_TRANSPORT' && error.status === 502);
  await malformed.dispose();
  // IPC 边界只提供已结束事务的错误信封，适配器保留公共错误并映射 HTTP 状态。
  const finishedCode = require('../contracts/error_cases.json').admission.transaction_finished_code;
  const finishedTransaction = create({ request() { return { version: 1, error: {
    schema_version: 1, code: finishedCode, message: '事务已结束 (Transaction finished)', details: {},
  } }; } });
  await assert.rejects(finishedTransaction.call({ operation: 'info' }), error => {
    assert.equal(error.code, finishedCode);
    assert.equal(error.schema_version, 1);
    assert.equal(error.status, 409);
    assert.deepEqual(error.details, {});
    return true;
  });
  await finishedTransaction.dispose();
  console.log('Console 传输、版本冲突、整批回执与排空契约通过 (Console transport, conflict, receipt and drain contracts passed)');
}

main().catch(error => { console.error(error); process.exitCode = 1; });
