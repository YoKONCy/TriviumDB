'use strict'

// 实际原生制品的图参数、宿主回调、完整路径和关闭资源契约。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const fixture = require('../contracts/graph_arguments.json')

function invalid(field) {
  return error => {
    assert.equal(error.schema_version, 1)
    assert.equal(error.code, 'TDB_INVALID_INPUT')
    assert.deepEqual(error.target, { field })
    assert.equal(error.stage, undefined)
    assert.equal(error.write, undefined)
    assert.deepEqual(Object.keys(error.details), ['reason'])
    assert.equal(typeof error.details.reason, 'string')
    assert.ok(error.details.reason.length > 0)
    return true
  }
}

function runGraphArguments(root, { TriviumDB }) {
  const paths = fixture.paths.map(row => ({
    sourceId: row.source_id, targetId: row.target_id, depth: row.depth, path: row.path,
    steps: row.steps.map(step => ({
      from: step.from, to: step.to, label: step.label, weight: step.weight, metadata: step.metadata,
    })),
  }))
  const subgraph = {
    nodes: fixture.nodes.map(({ id, payload }) => ({ id, payload })),
    edges: fixture.edges.map(edge => ({
      sourceId: edge.source_id, targetId: edge.target_id,
      label: edge.label, weight: edge.weight, metadata: edge.metadata,
    })),
    visitedNodes: 3, traversedEdges: 2, truncated: false,
  }
  const expected = {
    neighbors: fixture.neighbors,
    reachable: paths,
    reachableDetailed: { results: paths, visitedNodes: 3, traversedEdges: 2, truncated: false },
    querySubgraph: subgraph,
  }
  for (const dtype of ['f32', 'f16', 'u64']) {
    const directory = path.join(root, 'graph-arguments-' + dtype)
    fs.mkdirSync(directory)
    const file = path.join(directory, 'database.tdb')
    const options = { dim: 2, dtype, autoBuildQuiver: false }
    const db = new TriviumDB(file, options)
    try {
      for (const node of fixture.nodes) db.insertWithId(node.id, node.vector, node.payload)
      for (const edge of fixture.edges) db.upsertEdge(edge.source_id, edge.target_id, edge.label, edge.weight, edge.metadata)
      const before = fs.readFileSync(file + '.wal')
      // 数量回调采用公开默认预算，深度二覆盖完整三节点链。
      const validFields = {
        direction: 'outgoing', minDepth: 1, maxDepth: 2,
        maxVisitedNodes: 10000, maxResults: 10000, maxEdges: 50000,
      }
      const rejectReentry = () => {
        for (const action of [
          () => db.insert([1, 0], {}),
          () => db.insertWithId(17, [1, 0], {}),
          () => db.upsertWithId(7, [1, 0], {}),
          () => db.batchInsert([[1, 0]], [{}]),
          () => db.batchInsertWithIds([17], [[1, 0]], [{}]),
          () => db.delete(7),
          () => db.deleteManyAtomic([7]),
          () => db.updatePayload(7, {}),
          () => db.patchPayload(7, { $set: { name: '变化' } }),
          () => db.updateVector(7, [0, 1]),
          () => db.compareAndSetPayloadField(7, 'name', '源', '变化'),
          () => db.link(7, 13),
          () => db.upsertEdge(7, 13, 'route', 1, {}),
          () => db.updateEdge(7, 11, 'route', 1),
          () => db.unlink(7, 11, 'route'),
        ]) {
          assert.throws(action, error => {
            assert.equal(error.schema_version, 1)
            assert.equal(error.code, 'TDB_REENTRANT_ACCESS')
            assert.deepEqual(error.details, { operation: 'node_callback' })
            assert.equal(error.target, undefined)
            assert.equal(error.stage, undefined)
            assert.deepEqual(error.write, { phase: 'admission', outcome: 'not_committed' })
            return true
          })
        }
      }
      for (const method of ['neighbors', 'reachable', 'reachableDetailed', 'querySubgraph']) {
        const labeled = labels => method === 'neighbors' ? db.neighbors(7, 2, labels)
          : db[method](7, { maxDepth: 2, labels })
        for (const [labels, field] of [['route', 'labels'], [['route', 1], '/labels/1'], [['\ud800'], '/labels/0']]) {
          assert.throws(() => labeled(labels), invalid(field))
        }
        if (method !== 'neighbors') {
          for (const value of [[], 'options', true, 1, () => {}]) {
            assert.throws(() => db[method](7, value), invalid('options'))
          }
        }
        const config = { maxDepth: 2, labels: ['route'] }
        const invoke = (id = 7, argument = config) => method === 'neighbors'
          ? db.neighbors(id, argument, ['route']) : db[method](id, argument)
        const valid = method === 'neighbors' ? 2 : config
        for (const value of [null, undefined, '7', true, {}, [], 1n, Symbol('id'), -1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
          // 显式调用确保 undefined 按必需实参校验。
          assert.throws(() => method === 'neighbors' ? db.neighbors(value, 2)
            : db[method](value, config), invalid('id'))
        }
        if (method === 'neighbors') {
          for (const value of ['1', true, {}, [], 1n, Symbol('depth'), -1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
            assert.throws(() => db.neighbors(7, value), invalid('depth'))
          }
          assert.deepEqual(db.neighbors(7, null), [11])
          assert.deepEqual(db.neighbors(7), [11])
        } else {
          for (const field of ['minDepth', 'maxDepth', 'maxVisitedNodes', 'maxResults', 'maxEdges', 'direction']) {
            const values = field === 'direction' ? [null, 1, true, {}, [], 1n, Symbol('direction'), 'sideways']
              : [null, '1', true, {}, [], 1n, Symbol('budget'), -1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]
            for (const value of values) {
              assert.throws(() => invoke(7, { ...config, [field]: value }), invalid(field))
            }
            let coercions = 0
            const wrapped = { valueOf() { coercions++; return 1 }, toString() { coercions++; return 'outgoing' } }
            assert.throws(() => invoke(7, { ...config, [field]: wrapped }), invalid(field))
            assert.equal(coercions, 0)
            let calls = 0
            const callback = { ...config, get [field]() {
              calls++
              rejectReentry()
              return validFields[field]
            } }
            assert.deepEqual(invoke(7, callback), expected[method])
            assert.ok(calls > 0, '必须执行图配置 getter')
            const cause = new Error('图配置宿主原因')
            const sentinel = new TypeError('图配置宿主异常', { cause })
            assert.throws(() => invoke(7, { ...config, get [field]() { throw sentinel } }), error => {
              assert.equal(error, sentinel)
              assert.equal(error.cause, cause)
              assert.equal(error.code, undefined)
              return true
            })
          }
          assert.deepEqual(invoke(7, { ...config, minDepth: undefined, maxResults: undefined, direction: undefined }), expected[method])
          // 双向展开在中间节点还扫描回源边，扫描边数为三。
          const both = method === 'reachable' ? paths : { ...expected[method], traversedEdges: 3 }
          assert.deepEqual(invoke(7, { ...config, direction: 'both' }), both)
        }
        let calls = 0
        const labels = [null]
        Object.defineProperty(labels, '0', { get() { calls++; rejectReentry(); return 'route' } })
        const result = method === 'neighbors' ? db.neighbors(7, 2, labels) : invoke(7, { ...config, labels })
        assert.deepEqual(result, expected[method])
        assert.equal(calls, 1, '标签元素 getter 执行一次')
        assert.deepEqual(invoke(7, valid), expected[method])
      }
      for (const options of [undefined, null]) {
        assert.deepEqual(db.reachable(7, options), paths.slice(0, 1))
        assert.deepEqual(db.reachableDetailed(7, options), {
          results: paths.slice(0, 1), visitedNodes: 2, traversedEdges: 1, truncated: false,
        })
        assert.deepEqual(db.querySubgraph(7, options), {
          nodes: subgraph.nodes.slice(0, 2), edges: subgraph.edges.slice(0, 1),
          visitedNodes: 2, traversedEdges: 1, truncated: false,
        })
      }
      assert.deepEqual(fs.readFileSync(file + '.wal'), before)
      db.updateVector(7, [0, 1])
    } finally { db.close() }
    const moved = directory + '-moved'
    assert.ok(path.isAbsolute(directory))
    assert.equal(path.dirname(directory), path.dirname(moved))
    fs.renameSync(directory, moved)
    assert.throws(() => db.neighbors(7), error => error.code === 'TDB_DATABASE_CLOSED')
    const reopened = new TriviumDB(path.join(moved, 'database.tdb'), options)
    try {
      assert.deepEqual(reopened.allNodeIds(), [7, 11, 13])
      for (const expectedNode of fixture.nodes) {
        const node = reopened.get(expectedNode.id)
        assert.deepEqual(node.vector, node.id === 7 ? [0, 1] : expectedNode.vector)
        assert.deepEqual(node.payload, expectedNode.payload)
        assert.deepEqual(node.edges, fixture.edges.filter(edge => edge.source_id === node.id).map(edge => ({
          targetId: edge.target_id, label: edge.label, weight: edge.weight, metadata: edge.metadata,
        })))
      }
      assert.deepEqual(reopened.querySubgraph(7, { maxDepth: 2, labels: ['route'] }), subgraph)
      assert.equal(reopened.compareAndSetPayloadField(7, 'name', '源', '已更新'), undefined)
      assert.deepEqual(reopened.getPayload(7), { name: '已更新' })
      assert.equal(reopened.deleteManyAtomic([11, 13]), 2)
      assert.deepEqual(reopened.allNodeIds(), [7])
      assert.deepEqual(reopened.getEdges(7), [])
    } finally { reopened.close() }
    const final = new TriviumDB(path.join(moved, 'database.tdb'), options)
    try {
      assert.deepEqual(final.allNodeIds(), [7])
      const node = final.get(7)
      assert.deepEqual(node.vector, [0, 1])
      assert.deepEqual(node.payload, { name: '已更新' })
      assert.deepEqual(node.edges, [])
    } finally { final.close() }
  }
}

module.exports = { runGraphArguments, invalid }
