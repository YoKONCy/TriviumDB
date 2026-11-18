'use strict'

// 安装制品验证完整 JSON 等值预过滤和同步 Hook 的返回契约。
const assert = require('node:assert/strict')
const path = require('node:path')
const fixture = require('../contracts/search_filter_cases.json')
const entries = require('../contracts/search_entry_cases.json')

function verifyHybridWeights(root, TriviumDB) {
  const db = new TriviumDB(path.join(root, 'hybrid-weights.tdb'), { dim: 2, autoBuildQuiver: false })
  try {
    db.insertWithId(1, [1, 0], { name: 'z' })
    db.insertWithId(2, [0.8, 0.6], { name: 'a' })
    db.indexText(2, 'rareword')
    db.buildTextIndex()
    const hybrid = alpha => db.searchHybrid([1, 0], 'rareword', 2, 0, 0, alpha, undefined, true)
    assert.deepEqual(hybrid(1), db.searchExact([1, 0], 2))
    assert.deepEqual(hybrid(0).map(hit => hit.id), [2])
    assert.deepEqual(hybrid(0.2).map(hit => hit.id), [2, 1])
    for (const alpha of [-1, 2, 1.00000001, NaN, Infinity]) {
      assert.throws(() => hybrid(alpha), error => error.code === 'TDB_INVALID_INPUT')
    }
    assert.equal(db.tql('FIND {name: {$exists: true}} RETURN name ORDER BY name LIMIT 1')[0].name, 'a')
  } finally {
    db.close()
  }
}

async function verifySearchEntries(root, TriviumDB) {
  verifyHybridWeights(root, TriviumDB)
  const query = entries.query
  const records = new Map(entries.records.map(record => [record.id, record]))
  function verify(hits, item) {
    assert.deepEqual(hits.map(hit => hit.id), item.ids, item.name)
    for (const hit of hits) {
      assert.deepEqual(hit.payload, records.get(hit.id).payload)
      assert.ok(Math.abs(hit.score - records.get(hit.id).score) < 1e-6, `${item.name}/${hit.id}/${hit.score}`)
    }
  }
  for (const dtype of ['f32', 'f16']) {
    const file = path.join(root, `search-entry-${dtype}.tdb`)
    let db = new TriviumDB(file, { dim: 2, dtype })
    try {
      for (const record of records.values()) db.insertWithId(record.id, record.vector, record.payload)
      for (const reopened of [false, true]) {
        if (reopened) {
          db.close()
          db = new TriviumDB(file, { dim: 2, dtype })
        }
        for (const item of entries.cases) {
          const topK = item.top_k ?? records.size
          const config = { topK, expandDepth: 0, exact: item.exact, minScore: item.min_score, payloadFilter: item.filter }
          verify(db.search(query, topK, 0, item.min_score, item.filter, item.exact), item)
          verify(db.searchGrouped(query, topK, 0, item.min_score, item.filter, item.exact).semanticHits, item)
          const result = db.searchWithContext(query, config)
          verify(result.hits, item)
          assert.equal(result.context.observations.vector_route_quiver, 0)
          verify(db.searchAdvanced(query, { ...config, enableAdvancedPipeline: false }), item)
          verify(db.searchHybrid(query, '', topK, 0, item.min_score, undefined, item.filter, item.exact), item)
          if (!item.filter) {
            const batch = await db.searchBatch([query, query], topK, 2, item.min_score, item.exact)
            assert.equal(batch.length, 2)
            for (const hits of batch) verify(hits, item)
          }
        }
      }
      const invalid = error => error.code === 'TDB_INVALID_INPUT' && error.target?.field === 'exact'
      assert.throws(() => db.search(query, 3, 0, undefined, undefined, 'true'), invalid)
      assert.throws(() => db.searchBatch([query], 3, 1, undefined, 1), invalid)
      assert.throws(() => db.searchAdvanced(query, { exact: 'true' }), invalid)
    } finally {
      db.close()
    }
  }
  const db = new TriviumDB(path.join(root, 'search-exact-route.tdb'), { dim: 2 })
  try {
    for (let id = 900; id > 0; id--) db.insertWithId(id, [1, 0], { id })
    for (const indexed of [false, true]) {
      if (indexed) {
        db.search(query, 1)
        assert.equal(db.searchWithContext(query, { topK: 1 }).context.observations.vector_route_quiver, 1)
      }
      const result = db.searchWithContext(query, { topK: 3, expandDepth: 0, exact: true })
      assert.equal(result.context.observations.vector_route_quiver, 0)
      assert.deepEqual(result.hits.map(hit => [hit.id, hit.score, hit.payload]), [1, 2, 3].map(id => [id, 1, { id }]))
      assert.deepEqual(db.search(query, 3, 0, undefined, undefined, true).map(hit => hit.id), [1, 2, 3])
    }
    for (const config of [{ exact: true, forceBruteForce: false }, { exact: false, forceBruteForce: true }]) {
      const hits = db.searchAdvanced(query, { ...config, topK: 3, expandDepth: 0, enableAdvancedPipeline: false })
      assert.deepEqual(hits.map(hit => [hit.id, hit.score]), [[1, 1], [2, 1], [3, 1]])
    }
  } finally {
    db.close()
  }
}

const searchOptions = {
  topK: fixture.records.length, recallK: fixture.records.length,
  rerankK: fixture.records.length, expandDepth: 0, minScore: -1,
  enableAdvancedPipeline: false, forceBruteForce: true,
}

function verifyFilters(db) {
  for (const item of fixture.cases) {
    const hits = db.searchAdvanced([1, 0], { ...searchOptions, payloadFilter: item.filter })
    assert.deepEqual(hits.map(hit => hit.id), item.ids, JSON.stringify(item.filter))
    for (const hit of hits) {
      const record = fixture.records.find(record => record.id === hit.id)
      // JSON 的正负零按数值相等核对，其他内容逐字段保留。
      assert.deepEqual(JSON.parse(JSON.stringify(hit.payload)), JSON.parse(JSON.stringify(record.payload)))
      assert.equal(hit.score, 1)
    }
  }
}

module.exports = async function runSearchBoundaries(root, TriviumDB) {
  await verifySearchEntries(root, TriviumDB)
  for (const storageMode of ['rom', 'mmap']) {
    const file = path.join(root, `search-filter-${storageMode}.tdb`)
    const options = { dim: 2, storageMode, autoBuildQuiver: false }
    let db = new TriviumDB(file, options)
    try {
      for (const record of fixture.records) db.insertWithId(record.id, [1, 0], record.payload)
      verifyFilters(db)
      db.close()
      db = new TriviumDB(file, options)
      verifyFilters(db)
      for (const record of fixture.records) db.updatePayload(record.id, record.payload)
      verifyFilters(db)
    } finally {
      db.close()
    }
  }

  const db = new TriviumDB(path.join(root, 'hook-returns.tdb'), { dim: 2, autoBuildQuiver: false })
  const options = { topK: 2, recallK: 2, rerankK: 2, expandDepth: 0, minScore: -1 }
  const search = () => db.searchAdvanced([1, 0], options)
  const hookError = error => error.code === 'TDB_HOOK_EXECUTION'
  const hitStages = ['onCustomRecall', 'onPostRecall', 'onPreGraphExpand', 'onRerank', 'onPostSearch']
  try {
    db.insertWithId(1, [1, 0], { value: 'kept' })
    db.insertWithId(2, [0, 1], { value: 'second' })
    assert.deepEqual(search().map(hit => hit.id), [1, 2])
    for (const stage of hitStages) {
      let calls = 0
      db.setHook({ [stage]() { calls++; return null } })
      const hits = search()
      assert.equal(calls, 1, stage)
      assert.deepEqual(hits.map(hit => hit.id), [1, 2], stage)
      assert.deepEqual(hits[0].payload, { value: 'kept' })
      db.setHook({ [stage]() { return [] } })
      assert.deepEqual(search(), [], stage)
      for (const malformed of [
        {}, [null], [{}], [{ id: 1, score: 1 }], [{ id: 'bad', score: 1 }],
        [{ id: '0', score: 1 }], [{ id: '18446744073709551616', score: 1 }],
        [{ id: '1' }], [{ id: '1', score: '1' }], [{ id: '1', score: 1e308 }],
        [{ id: '1', score: NaN }], [{ id: '1', score: Infinity }],
      ]) {
        // 合法前缀用于确认错误成员触发整批拒绝。
        const result = Array.isArray(malformed) ? [{ id: '1', score: 1 }, ...malformed] : malformed
        db.setHook({ [stage]() { return result } })
        assert.throws(search, hookError, stage)
      }
    }
    for (const value of [17, {}, ['bad', 0]]) {
      db.setHook({ onPreSearch() { return value } })
      assert.throws(search, hookError)
    }
    for (const stage of ['onPreSearch', ...hitStages]) {
      let calls = 0
      db.setHook({ [stage]() { calls++; return Promise.resolve(null) } })
      assert.throws(search, hookError, stage)
      assert.equal(calls, 1, stage)
      db.setHook({ async [stage]() { throw new Error('受控异步失败') } })
      assert.throws(search, hookError, stage)
      // 事件循环完成点确认 Promise 拒绝已被接收。
      await new Promise(resolve => setImmediate(resolve))
    }
    db.setHook({ onRerank(hits) { return hits.slice(0, 1) } })
    assert.deepEqual(search(), [{ id: 1, score: 1, payload: { value: 'kept' } }])
    db.clearHook()
    assert.deepEqual(search().map(hit => hit.id), [1, 2])
  } finally {
    db.close()
  }

  const half = new TriviumDB(path.join(root, 'hook-f16.tdb'), { dim: 2, dtype: 'f16', autoBuildQuiver: false })
  try {
    half.insertWithId(1, [1, 0], { value: 'kept' })
    assert.throws(() => half.search([70000, 0], 1, 0, -1), error => error.code === 'TDB_INVALID_VECTOR')
    half.setHook({ onPreSearch() { return [70000, 0] } })
    assert.throws(() => half.search([1, 0], 1, 0, -1), error => error.code === 'TDB_INVALID_VECTOR')
    half.setHook({ onPreSearch() { return [60000, 0] } })
    assert.deepEqual(half.search([1, 0], 1, 0, -1), [{ id: 1, score: 1, payload: { value: 'kept' } }])
  } finally {
    half.close()
  }
}
