'use strict'

// 原生制品的查询实参、回调准入、异步完成与重开完整事实。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const payload = { saved: '完整事实' }
const writeQuery = 'MATCH (n) SET VECTOR(n) == [0, 1]'

function invalid(field, write) {
  return error => {
    assert.equal(error.schema_version, 1)
    assert.equal(error.code, 'TDB_INVALID_INPUT')
    assert.deepEqual(error.target, { field })
    assert.equal(error.stage, undefined)
    assert.deepEqual(error.write, write)
    assert.deepEqual(Object.keys(error.details), ['reason'])
    assert.equal(typeof error.details.reason, 'string')
    assert.ok(error.details.reason.length > 0)
    return true
  }
}

function checkHits(hits, score) {
  assert.deepEqual(hits, [{ id: 7, score, payload }])
}

async function runQueryArguments(root, { TriviumDB }) {
  for (const dtype of ['f32', 'f16', 'u64']) {
    // 相同浮点向量的余弦为 1；两段 u64 的匹配位数为 128。
    const score = dtype === 'u64' ? 128 : 1
    const directory = path.join(root, `query-arguments-${dtype}`)
    fs.mkdirSync(directory)
    const file = path.join(directory, 'database.tdb')
    const options = { dim: 2, dtype, autoBuildQuiver: false }
    const db = new TriviumDB(file, options)
    try {
      db.insertWithId(7, [1, 0], payload)
      const before = fs.readFileSync(file + '.wal')
      for (const [field, action] of [
        ['topK', value => db.search([1, 0], value)],
        ['expandDepth', value => db.search([1, 0], 1, value)],
        ['topK', value => db.searchGrouped([1, 0], value)],
        ['expandDepth', value => db.searchGrouped([1, 0], 1, value)],
        ['topK', value => db.searchBatch([[1, 0]], value)],
        ['parallelism', value => db.searchBatch([[1, 0]], 1, value)],
        ['topK', value => db.searchExact([1, 0], value)],
        ['topK', value => db.searchGraphFirst([1, 0], [7], value)],
        ['maxAnchorNodes', value => db.searchGraphFirst([1, 0], [7], 1, value)],
        ['topK', value => db.searchHybrid([1, 0], '', value)],
        ['expandDepth', value => db.searchHybrid([1, 0], '', 1, value)],
      ]) {
        for (const value of ['1', true, {}, [], 1n, Symbol('invalid'), -1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
          assert.throws(() => action(value), invalid(field))
        }
        let coercions = 0
        const value = { valueOf() { coercions++; throw new Error('禁止数值强制转换') } }
        assert.throws(() => action(value), invalid(field))
        assert.equal(coercions, 0)
      }
      for (const action of [() => db.search([1, 0], 0), () => db.searchGrouped([1, 0], 0),
        () => db.searchBatch([[1, 0]], 0), () => db.searchExact([1, 0], 0),
        () => db.searchGraphFirst([1, 0], [7], 0), () => db.searchHybrid([1, 0], '', 0)]) {
        assert.throws(action, invalid('topK'))
      }
      for (const method of ['searchAdvanced', 'searchWithContext']) {
        for (const field of ['topK', 'recallK', 'rerankK', 'expandDepth', 'maxEdgesPerNode']) {
          for (const value of [null, '1', true, {}, [], 1n, Symbol('count'), -1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
            assert.throws(() => db[method]([1, 0], { [field]: value }), invalid(field))
          }
          let coercions = 0
          assert.throws(() => db[method]([1, 0], { [field]: { valueOf() { coercions++; throw new Error('禁止数值强制转换') } } }), invalid(field))
          assert.equal(coercions, 0)
          let calls = 0
          const config = { expandDepth: 0, enableAdvancedPipeline: false, get [field]() {
            calls++
            assert.throws(() => db.tqlMut(writeQuery), error => {
              assert.equal(error.code, 'TDB_REENTRANT_ACCESS')
              assert.deepEqual(error.write, { phase: 'admission', outcome: 'not_committed' })
              return true
            })
            return field === 'expandDepth' ? 0 : 1
          } }
          const result = db[method]([1, 0], config)
          checkHits(method === 'searchWithContext' ? result.hits : result, score)
          assert.ok(calls > 0, '必须执行配置 getter')
          const cause = new Error('配置 getter 原因')
          const sentinel = new TypeError('配置 getter 失败', { cause })
          assert.throws(() => db[method]([1, 0], { get [field]() { throw sentinel } }), error => {
            assert.equal(error, sentinel)
            assert.equal(error.cause, cause)
            assert.equal(error.code, undefined)
            return true
          })
        }
        assert.throws(() => db[method]([1, 0], { topK: 0 }), invalid('topK'))
        const result = db[method]([1, 0], { topK: undefined, recallK: undefined, rerankK: undefined,
          expandDepth: 0, maxEdgesPerNode: undefined, enableAdvancedPipeline: false })
        checkHits(method === 'searchWithContext' ? result.hits : result, score)
      }
      for (const value of [null, undefined, 1, true, {}, [], 1n, Symbol('text')]) {
        assert.throws(() => db.searchHybrid([1, 0], value), invalid('queryText'))
      }
      checkHits(db.searchExact([1, 0], 1), score)
      checkHits(db.searchGraphFirst([1, 0], [7], 1), score)
      checkHits(db.searchGraphFirst([1, 0], [7], 1, null), score)
      assert.throws(() => db.searchExact([1, 0], null), invalid('topK'))
      assert.throws(() => db.searchGraphFirst([1, 0], [7], null), invalid('topK'))
      for (const [action, write] of [[value => db.tql(value)], [value => db.prepareTql(value)],
        [value => db.tqlMut(value), { phase: 'validation', outcome: 'not_committed' }]]) {
        for (const value of [null, undefined, 1, true, {}, [], 1n, Symbol('query')]) {
          assert.throws(() => action(value), invalid('query', write))
        }
        let coercions = 0
        assert.throws(() => action({ toString() { coercions++; throw new Error('禁止字符串强制转换') } }), invalid('query', write))
        assert.equal(coercions, 0)
      }
      checkHits(db.search([1, 0]), score)
      checkHits(db.search([1, 0], null, null), score)
      assert.deepEqual(db.searchGrouped([1, 0]), { semanticHits: [{ id: 7, score, payload }], graphHits: [] })
      assert.deepEqual(db.searchGrouped([1, 0], null, null), { semanticHits: [{ id: 7, score, payload }], graphHits: [] })
      const batches = await db.searchBatch([[1, 0], [1, 0]], null, null)
      assert.equal(batches.length, 2)
      for (const hits of batches) checkHits(hits, score)
      let callbacks = 0
      const parameters = { get x() {
        for (const [action, write] of [[() => db.search([1, 0])], [() => db.searchGrouped([1, 0])],
          [() => db.searchBatch([[1, 0]])], [() => db.searchExact([1, 0], 1)],
          [() => db.searchGraphFirst([1, 0], [7], 1)], [() => db.searchAdvanced([1, 0])],
          [() => db.searchWithContext([1, 0])], [() => db.searchHybrid([1, 0], '')],
          [() => db.prepareTql('MATCH (n) RETURN n')],
          [() => db.tqlMut(writeQuery), { phase: 'admission', outcome: 'not_committed' }]]) {
          assert.throws(action, error => {
            assert.equal(error.code, 'TDB_REENTRANT_ACCESS')
            assert.deepEqual(error.write, write)
            return true
          })
        }
        callbacks++
        return 42
      } }
      assert.deepEqual(db.executePreparedTql(db.prepareTql('MATCH (n) RETURN $x AS value'), parameters), [{ value: 42 }])
      assert.equal(callbacks, 1)
      for (const action of [
        value => db.search([1, 0], 1, 0, 0.5, value),
        value => db.searchGrouped([1, 0], 1, 0, 0.5, value).semanticHits,
        value => db.searchWithContext([1, 0], { expandDepth: 0, payloadFilter: value }).hits,
        value => db.searchAdvanced([1, 0], { expandDepth: 0, enableAdvancedPipeline: false, payloadFilter: value }),
        value => db.searchHybrid([1, 0], '', 1, 0, 0.1, 0.7, value),
      ]) {
        assert.deepEqual(action({ saved: '完整事实' }).map(({ id, payload }) => ({ id, payload })), [{ id: 7, payload }])
        assert.deepEqual(action({ saved: '其他内容' }), [])
        assert.throws(() => action({ saved: { $gt: true } }), invalid('payloadFilter'))
      }
      assert.deepEqual(fs.readFileSync(file + '.wal'), before)
      assert.deepEqual(db.tqlMut(writeQuery), { affected: 1, createdIds: [] })
    } finally { db.close() }
    const moved = directory + '-moved'
    assert.ok(path.isAbsolute(directory))
    assert.equal(path.dirname(directory), path.dirname(moved))
    fs.renameSync(directory, moved)
    assert.throws(() => db.nodeCount(), error => error.code === 'TDB_DATABASE_CLOSED')
    const reopened = new TriviumDB(path.join(moved, 'database.tdb'), options)
    try {
      assert.deepEqual(reopened.allNodeIds(), [7])
      const node = reopened.get(7)
      assert.deepEqual(node.vector, [0, 1])
      assert.deepEqual(node.payload, payload)
      assert.deepEqual(node.edges, [])
    } finally { reopened.close() }
  }
}

module.exports = { runQueryArguments, invalid }
