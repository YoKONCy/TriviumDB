'use strict'

// 安装制品验证完整 JSON 等值预过滤和同步 Hook 的返回契约。
const assert = require('node:assert/strict')
const path = require('node:path')
const fixture = require('../contracts/search_filter_cases.json')

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
