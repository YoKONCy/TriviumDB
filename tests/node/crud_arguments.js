'use strict'

// 普通写入口使用独立字段矩阵，验证实际包的转换、原异常和完整持久化事实。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const fixture = require('../contracts/crud_arguments.json')

function invalid(field) {
  return error => {
    assert.equal(error.schema_version, 1)
    assert.equal(error.code, 'TDB_INVALID_INPUT')
    assert.deepEqual(error.target, { field })
    assert.equal(error.stage, undefined)
    assert.deepEqual(error.write, { phase: 'validation', outcome: 'not_committed' })
    assert.deepEqual(Object.keys(error.details), ['reason'])
    assert.ok(error.details.reason)
    return true
  }
}

function rejectedValues(kind, field) {
  switch (kind) {
    case 'id': return [null, undefined, '7', true, {}, [], -1, 0.5, NaN, Infinity,
      Number.MAX_SAFE_INTEGER + 1, 1n, Symbol('id')].map(value => [value, field])
    case 'string': return [1, [], '\ud800'].map(value => [value, field])
    case 'number': return ['1', [], {}].map(value => [value, field])
    case 'vector': return [[null, field], ['10', field], [new Float32Array([1, 0]), field], [[1, 'x'], `/${field}/1`]]
    case 'vectors': return [[null, field], [[[1, 0], null], `/${field}/1`], [[[1, 0], ['x', 1]], `/${field}/1/0`]]
    case 'ids': return [[null, field], [[7, 'x'], `/${field}/1`], [[7, -1], `/${field}/1`]]
    case 'payloads': return [[null, field], [[{}, { 'a/b~c': [1, undefined] }], `/${field}/1/a~1b~0c/1`]]
    case 'json': return [
      ...(field === 'metadata' ? [] : [[undefined, field]]), [{ '\ud800': 1 }, field],
      ...[undefined, () => {}, Symbol('json'), 1n, NaN, Infinity, '\ud800']
        .map(value => [{ 'a/b~c': [1, value] }, `/${field}/a~1b~0c/1`]),
      [{ '词.行[]\0😀': undefined }, `/${field}/词.行[]\0😀`],
    ]
    default: throw new Error('未知契约字段类型 ' + kind)
  }
}

function expected(values) {
  return values.map(value => ({
    id: value.id, vector: value.vector, payload: value.payload, numEdges: value.num_edges,
    edges: value.edges.map(edge => ({ targetId: edge.target_id, label: edge.label,
      weight: edge.weight, metadata: edge.metadata })),
  }))
}

function facts(db) {
  return db.allNodeIds().map(id => db.get(id))
}

function runCrudArguments(root, { TriviumDB }) {
  assert.equal(fixture.operations.length, 15)
  const before = expected(fixture.before)
  const after = expected(fixture.after)
  for (const dtype of ['f32', 'f16', 'u64']) for (const storageMode of ['rom', 'mmap']) {
    const directory = path.join(root, `crud-arguments-${dtype}-${storageMode}`)
    const file = path.join(directory, 'database.tdb')
    const options = { dim: 2, dtype, storageMode, autoBuildQuiver: false }
    const db = new TriviumDB(file, options)
    let held, heldError
    try {
      for (const value of fixture.before) db.insertWithId(value.id, value.vector, value.payload)
      db.upsertEdge(7, 11, 'route', 0.5, { part: '前段' })
      assert.deepEqual(facts(db), before)
      const wal = fs.readFileSync(file + '.wal')
      for (const operation of fixture.operations) {
        for (const entry of operation.fields) {
          for (const [value, field] of rejectedValues(entry.kind, entry.field)) {
            const args = structuredClone(operation.args)
            args[entry.index] = value
            assert.throws(() => db[operation.node](...args), invalid(field), `${operation.node} ${field}`)
          }
        }
        assert.deepEqual(facts(db), before, operation.node)
        assert.deepEqual(fs.readFileSync(file + '.wal'), wal, operation.node)
      }
      for (const [action, field] of [
        [() => db.batchInsert([[1, 0]], []), 'payloads'],
        [() => db.batchInsertWithIds([], [[1, 0]], [{}]), 'ids'],
        [() => db.batchInsertWithIds([19], [[1, 0]], []), 'payloads'],
      ]) assert.throws(action, invalid(field))
      for (const action of [
        () => db.batchInsert([[1, 0], [1]], [{}, {}]),
        () => db.batchInsertWithIds([19, 23], [[1, 0], [1]], [{}, {}]),
      ]) {
        assert.throws(action, error => {
          assert.equal(error.code, 'TDB_DIMENSION_MISMATCH')
          assert.deepEqual(error.details, { expected: 2, got: 1 })
          assert.deepEqual(error.write, { phase: 'validation', outcome: 'not_committed' })
          return true
        })
        assert.deepEqual(facts(db), before)
        assert.deepEqual(fs.readFileSync(file + '.wal'), wal)
      }
      if (dtype === 'u64') for (const value of [-1, 0.5, NaN, Infinity, 2 ** 64]) {
        assert.throws(() => db.updateVector(7, [1, value]), invalid('/vector/1'))
      }
      const cause = new Error('宿主 cause')
      const sentinel = Object.assign(new TypeError('宿主 getter 原异常', { cause }), {
        code: 'HOST_GETTER', target: { host: true }, write: { host: '原字段' },
      })
      const hostError = error => {
        assert.equal(error, sentinel)
        assert.equal(error.cause, cause)
        assert.equal(error.code, 'HOST_GETTER')
        assert.deepEqual(error.target, { host: true })
        assert.deepEqual(error.write, { host: '原字段' })
        assert.equal(error.details, undefined)
        return true
      }
      const vector = [1, 0]
      Object.defineProperty(vector, 0, { get() { throw sentinel } })
      for (const action of [
        () => db.insert(vector, {}),
        () => db.updatePayload(7, { get content() { throw sentinel } }),
        () => db.updatePayload(7, new Proxy({}, { ownKeys() { throw sentinel } })),
      ]) assert.throws(action, hostError)
      let calls = 0
      const wrapped = { valueOf() { calls++; throw sentinel }, toString() { calls++; throw sentinel } }
      assert.throws(() => db.delete(wrapped), invalid('id'))
      assert.throws(() => db.link(7, 11, wrapped), invalid('label'))
      assert.equal(calls, 0)
      const reentrant = [1, 0]
      Object.defineProperty(reentrant, 0, { get() {
        calls++
        assert.throws(() => db.get(7), error => {
          assert.equal(error.code, 'TDB_REENTRANT_ACCESS')
          assert.deepEqual(error.details, { operation: 'node_callback' })
          assert.equal(error.write, undefined)
          return true
        })
        assert.throws(() => db.delete(7), error => {
          assert.equal(error.code, 'TDB_REENTRANT_ACCESS')
          assert.deepEqual(error.write, { phase: 'admission', outcome: 'not_committed' })
          return true
        })
        return 1
      } })
      db.updateVector(7, reentrant)
      assert.equal(calls, 1)
      assert.deepEqual(facts(db), before)
      const afterProtocol = fs.readFileSync(file + '.wal')
      assert.throws(() => db.deleteManyAtomic([7, 'x']), error => {
        heldError = error
        return invalid('/ids/1')(error)
      })
      assert.deepEqual(fs.readFileSync(file + '.wal'), afterProtocol)
      const automatic = [db.insert([1, 1], { 临时: 1 })]
      automatic.push(...db.batchInsert([[1, 0], [0, 1]], [{ 临时: 2 }, { 临时: 3 }]))
      db.insertWithId(29, [0, 1], { 临时: 4 })
      db.batchInsertWithIds([19, 23], [[1, 0], [0, 1]], [fixture.after[2].payload, {}])
      db.upsertWithId(23, [1, 1], { name: '替换' })
      db.updatePayload(7, { name: '待更新', count: 1 })
      db.patchPayload(7, { $inc: { count: 1 } })
      db.compareAndSetPayloadField(7, 'name', '待更新', '更新')
      db.updateVector(7, [1, 1])
      db.link(7, 11)
      db.unlink(7, 11, 'related')
      db.upsertEdge(7, 11, 'route', 1, { part: '新增' })
      db.updateEdge(7, 11, 'route', 0.25, { part: '更新' })
      db.delete(29)
      assert.equal(db.deleteManyAtomic(automatic), 3)
      assert.deepEqual(facts(db), after)
      held = facts(db)
      db.flush()
    } finally { db.close() }
    const moved = directory + '-moved'
    fs.renameSync(directory, moved)
    assert.deepEqual(heldError.target, { field: '/ids/1' })
    assert.deepEqual(held, after)
    for (let pass = 0; pass < 2; pass++) {
      const reopened = new TriviumDB(path.join(moved, 'database.tdb'), options)
      try {
        assert.deepEqual(facts(reopened), after)
        reopened.flush()
      } finally { reopened.close() }
    }
  }
}

module.exports = { runCrudArguments }
