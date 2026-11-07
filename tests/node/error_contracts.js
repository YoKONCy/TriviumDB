'use strict'

// 真实原生 API 与共享独立样例核对错误码、字段及同步/异步一致性。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const { TriviumDB } = require('../../index.js')
const contract = require('../contracts/error_cases.json')

function assertError(error, test) {
  assert.equal(error.schema_version, 1, test.operation)
  assert.equal(typeof error.message, 'string', test.operation)
  assert.equal(error.code, test.code, test.operation)
  const details = { ...error.details }
  for (const field of test.string_fields || []) {
    assert.equal(typeof details[field], 'string', test.operation)
    assert.ok(details[field].length > 0, test.operation)
    delete details[field]
  }
  for (const field of test.integer_fields || []) {
    assert.ok(Number.isInteger(details[field]), test.operation)
    delete details[field]
  }
  assert.deepEqual(details, test.details, test.operation)
  assert.deepEqual(error.write, test.write, test.operation)
  for (const [field, value] of Object.entries(error.details)) assert.deepEqual(error[field], value)
  return true
}

async function runErrorContract(root) {
  for (const test of contract.cases) {
    const directory = path.join(root, `errors-${test.operation}`)
    fs.mkdirSync(directory)
    const file = path.join(directory, 'database.tdb')
    let db = new TriviumDB(file, { dim: 2, autoBuildQuiver: false })
    try {
      db.insertWithId(1, [1, 0], { kind: 'alpha' })
      db.insertWithId(2, [0, 1], { kind: 'beta' })
      if (['readonly', 'rows'].includes(test.operation)) {
        db.close()
        db = new TriviumDB(file, { dim: 2, accessMode: 'readOnly', maxQueryRows: 1 })
      }
      if (test.operation === 'unique') db.createUniqueIndex('kind')
      if (test.operation === 'closed') db.close()
      if (test.operation === 'io') fs.writeFileSync(path.join(directory, 'blocked'), '文件占据父路径')
      if (test.operation === 'version') {
        const header = Buffer.alloc(58)
        header.write('TVDB')
        header.writeUInt16LE(65535, 4)
        fs.writeFileSync(path.join(directory, 'future.tdb'), header)
      }
      const actions = {
        missing_parameter: () => db.executePreparedTql(db.prepareTql('MATCH (n) RETURN $required AS value'), {}),
        unexpected_parameter: () => db.executePreparedTql(db.prepareTql('MATCH (n) RETURN $required AS value'), { required: 1, extra: 2 }),
        dimension: () => db.insert([1], {}),
        missing_node: () => db.updatePayload(999, {}),
        duplicate_node: () => db.insertWithId(1, [1, 0], {}),
        unique: () => db.insertWithId(3, [1, 0], { kind: 'alpha' }),
        readonly: () => db.insert([1, 0], {}),
        rows: () => db.tql('MATCH (n) RETURN n'),
        migration: () => db.tqlMut('MATCH (n) RETURN n'),
        closed: () => db.getPayload(1),
        locked: () => new TriviumDB(file, { dim: 2 }),
        io: () => new TriviumDB(path.join(directory, 'blocked/database.tdb'), { dim: 2 }),
        version: () => new TriviumDB(path.join(directory, 'future.tdb'), { dim: 2 }),
      }
      assert.throws(actions[test.operation], error => assertError(error, test))
      if (test.operation === 'dimension') {
        await assert.rejects(db.searchBatch([[1]], 1, 1, 0), error => assertError(error, { ...test, write: undefined }))
      }
    } finally {
      db.close()
    }
  }
  const integers = new TriviumDB(path.join(root, 'error-u64-vectors.tdb'), { dim: 1, dtype: 'u64', autoBuildQuiver: false })
  try {
    integers.insertWithId(1, [1], {})
    const beforeWal = fs.readFileSync(path.join(root, 'error-u64-vectors.tdb.wal'))
    const invalid = error => {
      assert.equal(error.code, 'TDB_INVALID_INPUT')
      assert.deepEqual(Object.keys(error.details), ['reason'])
      assert.equal(typeof error.details.reason, 'string')
      assert.ok(error.details.reason.length > 0)
      return true
    }
    for (const component of [-1, 0.5, NaN, Infinity]) {
      assert.throws(() => integers.insertWithId(2, [component], {}), invalid)
      await assert.rejects(integers.searchBatch([[component]], 1, 1, 0), invalid)
      assert.equal(integers.getPayload(2), null)
    }
    for (const component of [-1, 0.5]) {
      assert.throws(() => integers.commitTransaction([{ type: 'insertWithId', id: 2, vector: [component], payload: {} }]), invalid)
      assert.equal(integers.getPayload(2), null)
    }
    // JSON 参数转换在进入事务构造前拒绝非有限值。
    for (const component of [NaN, Infinity]) {
      assert.throws(() => integers.commitTransaction([{ type: 'insertWithId', id: 2, vector: [component], payload: {} }]),
        error => {
          invalid(error)
          assert.equal(error.schema_version, 1)
          assert.deepEqual(error.target, { field: '/operations/0/vector/0' })
          assert.deepEqual(error.write, { phase: 'validation', outcome: 'not_committed', transaction_consumed: false })
          return true
        })
      assert.equal(integers.getPayload(2), null)
    }
    assert.deepEqual(integers.allNodeIds(), [1])
    assert.deepEqual(integers.get(1), { id: 1, vector: [1], payload: {}, edges: [], numEdges: 0 })
    assert.deepEqual(fs.readFileSync(path.join(root, 'error-u64-vectors.tdb.wal')), beforeWal)
  } finally {
    integers.close()
  }
  await runQueryDiagnostics(root)
}

async function runQueryDiagnostics(root, api = require('../../index.js')) {
  await require('./query_arguments').runQueryArguments(root, api)
  require('./graph_arguments').runGraphArguments(root, api)
  require('./read_arguments').runReadArguments(root, api)
  require('./call_arguments').runCallArguments(root, api)
  require('./constructor_arguments').runConstructorArguments(root, api)
  require('./crud_arguments').runCrudArguments(root, api)
  await require('./retrieval_fields').runRetrievalFields(root, api)
  require('./write_outcomes').runWriteOutcomes(root, api)
  runPreparedDiagnostics(root, api)
  require('./query_sources').runQuerySources(root, api)
  const { TriviumDB, ConsoleSession } = api
  const { cases } = require('../contracts/query_diagnostics.json')
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `diagnostics-${dtype}.tdb`)
    const db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    try {
      db.insertWithId(7, [1, 0], { saved: '完整事实' })
      const session = new ConsoleSession('{"readOnly":false}')
      for (const test of cases) {
        const actions = test.mutation ? [() => db.tqlMut(test.query)] : [
          () => db.tql(test.query), () => db.prepareTql(test.query),
        ]
        actions.push(() => db.consoleRequest(session, JSON.stringify({
          version: 1, requestId: test.name,
          command: { operation: test.mutation ? 'mutate' : 'query', query: test.query },
        })))
        for (const action of actions) assert.throws(action, error => {
          assert.equal(error.schema_version, 1, test.name)
          assert.equal(error.code, 'TDB_QUERY_PARSE', test.name)
          assert.equal(error.stage, test.stage, test.name)
          assert.deepEqual(error.write, test.mutation ? { phase: 'validation', outcome: 'not_committed' } : undefined)
          assert.deepEqual(error.target, test.span ? { query_span: test.span } : undefined, test.name)
          assert.deepEqual(Object.keys(error.details), ['reason'])
          assert.equal(typeof error.details.reason, 'string')
          assert.ok(error.details.reason.length > 0)
          return true
        })
      }
    } finally {
      db.close()
    }
    const reopened = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    try {
      assert.deepEqual(reopened.allNodeIds(), [7])
      const node = reopened.get(7)
      assert.deepEqual(node.vector, [1, 0])
      assert.deepEqual(node.payload, { saved: '完整事实' })
      assert.deepEqual(node.edges, [])
    } finally {
      reopened.close()
    }
  }
}

function runPreparedDiagnostics(root, { TriviumDB, ConsoleSession }) {
  const { cases } = require('../contracts/prepared_diagnostics.json')
  const invalid = (target, stage) => error => {
    assert.equal(error.schema_version, 1)
    assert.equal(error.code, 'TDB_INVALID_INPUT')
    assert.deepEqual(error.target, target)
    assert.equal(error.stage, stage)
    assert.equal(error.write, undefined)
    assert.deepEqual(Object.keys(error.details), ['reason'])
    assert.equal(typeof error.details.reason, 'string')
    assert.ok(error.details.reason.length > 0)
    return true
  }
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `prepared-diagnostics-${dtype}.tdb`)
    const db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    try {
      db.insertWithId(7, [1, 0], { saved: '完整事实' })
      const session = new ConsoleSession()
      const before = fs.readFileSync(`${file}.wal`)
      for (const test of cases) {
        const prepared = db.prepareTql(test.query)
        const request = JSON.stringify({ version: 1, requestId: test.name, command: {
          operation: 'query', query: test.query, parameters: test.parameters,
        } })
        for (const action of [() => db.executePreparedTql(prepared, test.parameters), () => db.consoleRequest(session, request)]) {
          assert.throws(action, error => {
            assert.equal(error.schema_version, 1)
            assert.equal(error.code, test.code, test.name)
            assert.equal(error.stage, 'bind')
            const target = { parameter: test.parameter }
            if (test.span) target.query_span = test.span
            assert.deepEqual(error.target, target, test.name)
            if (test.details) assert.deepEqual(error.details, test.details)
            else {
              assert.deepEqual(Object.keys(error.details), ['reason'])
              assert.equal(typeof error.details.reason, 'string')
              assert.ok(error.details.reason.length > 0)
            }
            return true
          })
        }
      }
      const prepared = db.prepareTql('MATCH (n) RETURN $x AS value')
      const xTarget = { parameter: 'x', query_span: { start_byte: 17, end_byte: 19, line: 1, column: 18 } }
      assert.deepEqual(db.executePreparedTql(prepared, { x: 42 }), [{ value: 42 }])
      for (const parameters of [null, undefined, [], 42, 'x', true, 1n, Symbol('container'), () => 42]) {
        assert.throws(() => db.executePreparedTql(prepared, parameters), invalid({ field: 'parameters' }))
      }
      for (const value of [NaN, Infinity, -Infinity, undefined, 1n, Symbol('value'), () => 42]) {
        assert.throws(() => db.executePreparedTql(prepared, { x: value }), invalid(xTarget, 'bind'))
      }
      for (const value of [null, true, false, '中文\u0000参数', 0, -2147483648, 4294967295, 4294967296, 1.25, Number.MAX_SAFE_INTEGER]) {
        assert.deepEqual(db.executePreparedTql(prepared, { x: value }), [{ value }])
      }
      let nestedReads = 0
      const nested = { get rejected() { nestedReads++; throw new Error('复合参数应在标量边界拒绝') } }
      nested.self = nested
      assert.throws(() => db.executePreparedTql(prepared, { x: nested }), invalid(xTarget, 'bind'))
      assert.equal(nestedReads, 0)
      const literalName = 'extra\u0000中文.key'
      assert.throws(() => db.executePreparedTql(prepared, { [literalName]: undefined }),
        invalid({ parameter: literalName }, 'bind'))
      let callbacks = 0
      const callbackParameters = { get x() {
        assert.throws(() => db.reserveNodes(1), error => {
          assert.equal(error.code, 'TDB_REENTRANT_ACCESS')
          assert.deepEqual(error.write, { phase: 'admission', outcome: 'not_committed' })
          return true
        })
        callbacks++
        return 42
      } }
      assert.deepEqual(db.executePreparedTql(prepared, callbackParameters), [{ value: 42 }])
      assert.equal(callbacks, 1)
      assert.deepEqual(db.executePreparedTql(prepared, Object.create({ x: 42 })), [{ value: 42 }])
      assert.deepEqual(db.executePreparedTql(prepared, Object.assign(Object.create(null), { x: 42 })), [{ value: 42 }])
      const cause = new Error('宿主映射原因')
      const sentinel = new TypeError('宿主参数 getter 失败', { cause })
      assert.throws(() => db.executePreparedTql(prepared, { get x() { throw sentinel } }), error => {
        assert.equal(error, sentinel)
        assert.equal(error.cause, cause)
        assert.equal(error.code, undefined)
        return true
      })
      const proxy = new Proxy({}, { ownKeys() { throw sentinel } })
      assert.throws(() => db.executePreparedTql(prepared, proxy), error => {
        assert.equal(error, sentinel)
        assert.equal(error.cause, cause)
        assert.equal(error.code, undefined)
        return true
      })
      assert.deepEqual(db.executePreparedTql(prepared, { x: 42 }), [{ value: 42 }])
      assert.deepEqual(fs.readFileSync(`${file}.wal`), before)
    } finally {
      db.close()
    }
    const reopened = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    try {
      assert.deepEqual(reopened.allNodeIds(), [7])
      const node = reopened.get(7)
      assert.deepEqual(node.vector, [1, 0])
      assert.deepEqual(node.payload, { saved: '完整事实' })
      assert.deepEqual(node.edges, [])
    } finally {
      reopened.close()
    }
  }
}

module.exports = { runErrorContract, runQueryDiagnostics }
