'use strict'

// 基础读取的类型边界、完整事实、旧结果所有权与实际安装制品契约。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const graph = require('../contracts/graph_arguments.json')
const fixture = require('../contracts/read_arguments.json')
const { invalid } = require('./graph_arguments')

function edge(value) {
  return { targetId: value.target_id, label: value.label, weight: value.weight, metadata: value.metadata }
}

function orderedEdges(values) {
  return [...values].sort((left, right) => {
    if (left.targetId !== right.targetId) return left.targetId - right.targetId
    if (left.label === right.label) return 0
    return left.label < right.label ? -1 : 1
  })
}

function node(value) {
  assert.notEqual(value, null)
  return { ...value, edges: orderedEdges(value.edges) }
}

function runReadArguments(root, { TriviumDB }) {
  const source = {
    id: 7, vector: fixture.source.vector, payload: fixture.source.payload,
    numEdges: 3, edges: fixture.source.edges.map(edge),
  }
  const incoming = fixture.incoming.map(value => ({ sourceId: value.source_id, ...edge(value) }))
  for (const dtype of ['f32', 'f16', 'u64']) {
    const directory = path.join(root, 'read-arguments-' + dtype)
    fs.mkdirSync(directory)
    const file = path.join(directory, 'database.tdb')
    const options = { dim: 2, dtype, autoBuildQuiver: false }
    const db = new TriviumDB(file, options)
    try {
      for (const value of graph.nodes) db.insertWithId(value.id, value.vector, value.payload)
      for (const value of [...graph.edges, ...fixture.extra_edges]) {
        db.upsertEdge(value.source_id, value.target_id, value.label, value.weight, value.metadata)
      }
      const before = fs.readFileSync(file + '.wal')
      const readers = [
        ['id', value => node(db.get(value)), 7, source],
        ['id', value => db.getPayload(value), 7, source.payload],
        ['id', value => db.contains(value), 7, true],
        ['id', value => orderedEdges(db.getEdges(value)), 7, source.edges],
        ['id', value => db.getIncomingEdges(value), 11, incoming],
        ['src', value => db.getEdge(value, 11, 'route'), 7, source.edges[1]],
        ['dst', value => db.getEdge(7, value, 'route'), 11, source.edges[1]],
      ]
      for (const [field, action, valid, expected] of readers) {
        for (const value of [null, undefined, '7', true, {}, [], 1n, Symbol('id'), -1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
          assert.throws(() => action(value), invalid(field))
        }
        let calls = 0
        const wrapped = { valueOf() { calls++; throw new Error('禁止读取 ID 强制转换') } }
        assert.throws(() => action(wrapped), invalid(field))
        assert.equal(calls, 0)
        assert.deepEqual(action(valid), expected)
      }
      for (const action of [
        label => db.getEdge(7, 11, label),
        label => db.getIncomingEdges(11, label),
      ]) {
        for (const value of [1, true, {}, [], 1n, Symbol('label'), '\ud800']) {
          assert.throws(() => action(value), invalid('label'))
        }
        let calls = 0
        const wrapped = { toString() { calls++; throw new Error('禁止读取标签强制转换') } }
        assert.throws(() => action(wrapped), invalid('label'))
        assert.equal(calls, 0)
      }
      for (const expected of source.edges) {
        assert.deepEqual(db.getEdge(7, 11, expected.label), expected)
        assert.deepEqual(db.getIncomingEdges(11, expected.label), [{ sourceId: 7, ...expected }])
      }
      for (const label of [undefined, null]) {
        assert.deepEqual(db.getIncomingEdges(11, label), incoming)
        assert.throws(() => db.getEdge(7, 11, label), invalid('label'))
      }
      for (const absent of [0, 999, Number.MAX_SAFE_INTEGER]) {
        assert.equal(db.get(absent), null)
        assert.equal(db.getPayload(absent), null)
        assert.equal(db.contains(absent), false)
        assert.deepEqual(db.getEdges(absent), [])
        assert.deepEqual(db.getIncomingEdges(absent), [])
        assert.equal(db.getEdge(absent, 11, 'route'), null)
      }
      let callbacks = 0
      const parameters = { get value() {
        callbacks++
        for (const [, action, valid] of readers) {
          assert.throws(() => action(valid), error => {
            assert.equal(error.schema_version, 1)
            assert.equal(error.code, 'TDB_REENTRANT_ACCESS')
            assert.deepEqual(error.details, { operation: 'node_callback' })
            assert.equal(error.stage, undefined)
            assert.equal(error.target, undefined)
            assert.equal(error.write, undefined)
            return true
          })
        }
        assert.throws(() => db.updatePayload(7, {}), error => {
          assert.equal(error.code, 'TDB_REENTRANT_ACCESS')
          assert.deepEqual(error.write, { phase: 'admission', outcome: 'not_committed' })
          return true
        })
        return 42
      } }
      const prepared = db.prepareTql('MATCH (n) RETURN $value AS result LIMIT 1')
      assert.deepEqual(db.executePreparedTql(prepared, parameters), [{ result: 42 }])
      assert.equal(callbacks, 1)
      const held = db.get(7)
      const heldPayload = db.getPayload(7)
      const heldEdges = db.getEdges(7)
      const heldIncoming = db.getIncomingEdges(11)
      assert.deepEqual(fs.readFileSync(file + '.wal'), before)
      db.updatePayload(7, { name: '已更新' })
      db.updateVector(7, [0, 1])
      db.close()
      const moved = directory + '-moved'
      assert.ok(path.isAbsolute(directory))
      assert.equal(path.dirname(directory), path.dirname(moved))
      fs.renameSync(directory, moved)
      assert.deepEqual(node(held), source)
      assert.deepEqual(heldPayload, source.payload)
      assert.deepEqual(orderedEdges(heldEdges), source.edges)
      assert.deepEqual(heldIncoming, incoming)
      assert.throws(() => db.get(7), error => error.code === 'TDB_DATABASE_CLOSED')
      for (let pass = 0; pass < 2; pass++) {
        const reopened = new TriviumDB(path.join(moved, 'database.tdb'), options)
        try {
          assert.deepEqual(reopened.allNodeIds(), [7, 11, 13])
          assert.deepEqual(node(reopened.get(7)), { ...source, vector: [0, 1], payload: { name: '已更新' } })
          for (const value of graph.nodes.slice(1)) {
            const edges = graph.edges.filter(item => item.source_id === value.id).map(edge)
            assert.deepEqual(node(reopened.get(value.id)), { ...value, edges, numEdges: edges.length })
          }
          assert.deepEqual(reopened.getIncomingEdges(11), incoming)
          reopened.flush()
        } finally { reopened.close() }
      }
    } finally { db.close() }
  }
}

module.exports = { runReadArguments }
