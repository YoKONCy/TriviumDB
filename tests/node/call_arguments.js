'use strict'

// 缺参在 N-API 转换之前拒绝，并验证真实 npm 包中的事实与资源生命周期。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const cases = require('../contracts/call_arguments.json')
const graph = require('../contracts/graph_arguments.json')

function invalid(field, write) {
  return error => {
    assert.ok(error instanceof TypeError)
    assert.equal(error.schema_version, 1)
    assert.equal(error.code, 'TDB_INVALID_INPUT')
    assert.deepEqual(error.target, { field })
    assert.equal(error.stage, undefined)
    assert.deepEqual(Object.keys(error.details).sort(), ['argument_kind', 'reason'])
    assert.equal(error.details.argument_kind, 'missing')
    assert.ok(error.details.reason)
    assert.deepEqual(error.write, write)
    return true
  }
}

function runCallArguments(root, api) {
  const { TriviumDB, ConsoleSession, verifySnapshot, restoreSnapshot } = api
  const validation = { phase: 'validation', outcome: 'not_committed' }
  assert.ok(!Object.keys(api).includes('__callArgumentInfo'))
  for (const dtype of ['f32', 'f16', 'u64']) {
    const directory = path.join(root, 'call-arguments-' + dtype)
    fs.mkdirSync(directory)
    const file = path.join(directory, 'database.tdb')
    const options = { dim: 2, dtype, autoBuildQuiver: false }
    const db = new TriviumDB(file, options)
    try {
      for (const value of graph.nodes) db.insertWithId(value.id, value.vector, value.payload)
      const before = fs.readFileSync(file + '.wal')
      for (const entry of cases.missing) {
        assert.throws(() => db[entry.node](...entry.args),
          invalid(entry.node_field || entry.field, entry.write ? validation : undefined))
      }
      assert.throws(() => db.commitTransaction(), invalid('operations', { ...validation, transaction_consumed: false }))
      assert.throws(() => db.setMemoryLimit(), invalid('mb', validation))
      assert.throws(() => verifySnapshot(), invalid('directory'))
      assert.throws(() => restoreSnapshot('unused'), invalid('destination', validation))
      assert.throws(() => new ConsoleSession().cancel(), invalid('requestId'))
      let calls = 0
      const vector = [1, 0]
      Object.defineProperty(vector, 0, { get() { calls++; throw new Error('禁止错误形状读取向量') } })
      assert.throws(() => db.insert(vector), invalid('payload', validation))
      assert.equal(calls, 0)
      // undefined 已占据位置，继续使用既有类型校验；额外位置沿用 JavaScript 调用规则。
      assert.throws(() => db.get(undefined), error => {
        assert.equal(error.code, 'TDB_INVALID_INPUT')
        assert.deepEqual(error.target, { field: 'id' })
        assert.equal(error.details.argument_kind, undefined)
        return true
      })
      assert.deepEqual(db.get(7, 'extra'), db.get(7))
      const prepared = db.prepareTql('MATCH (n) RETURN $value AS result LIMIT 1')
      const parameters = { get value() {
        calls++
        assert.throws(() => db.get(), error => {
          assert.equal(error.code, 'TDB_REENTRANT_ACCESS')
          assert.deepEqual(error.details, { operation: 'node_callback' })
          assert.equal(error.target, undefined)
          return true
        })
        return 42
      } }
      assert.deepEqual(db.executePreparedTql(prepared, parameters), [{ result: 42 }])
      assert.equal(calls, 1)
      const held = graph.nodes.map(value => db.get(value.id))
      const expected = graph.nodes.map(value => ({ ...value, numEdges: 0, edges: [] }))
      assert.deepEqual(held, expected)
      assert.deepEqual(fs.readFileSync(file + '.wal'), before)
      const operations = [{ type: 'insertWithId', id: 19, vector: [1, 1], payload: { name: '待提交' } }]
      assert.deepEqual(db.allNodeIds(), [7, 11, 13])
      assert.deepEqual(db.commitTransaction(operations), [19])
      assert.deepEqual(operations, [{ type: 'insertWithId', id: 19, vector: [1, 1], payload: { name: '待提交' } }])
      db.close()
      const moved = directory + '-moved'
      assert.ok(path.isAbsolute(directory))
      assert.equal(path.dirname(directory), path.dirname(moved))
      fs.renameSync(directory, moved)
      assert.deepEqual(held, expected)
      for (let pass = 0; pass < 2; pass++) {
        const reopened = new TriviumDB(path.join(moved, 'database.tdb'), options)
        try {
          assert.deepEqual(reopened.allNodeIds(), [7, 11, 13, 19])
          assert.deepEqual([7, 11, 13, 19].map(id => reopened.get(id)), [
            ...expected, { id: 19, vector: [1, 1], payload: { name: '待提交' }, numEdges: 0, edges: [] },
          ])
          reopened.flush()
        } finally { reopened.close() }
      }
    } finally { db.close() }
  }
}

module.exports = { runCallArguments }
