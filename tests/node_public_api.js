'use strict'

// Node 公共 API 跨层契约测试：覆盖 Prepared TQL、索引/存储诊断、
// u64 字符串映射、Payload Filter 和已移除入口的稳定迁移错误。

const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { TriviumDB } = require('../index.js')

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'triviumdb-node-public-api-'))
const file = path.join(root, 'api.tdb')

try {
  assert.throws(
    () => new TriviumDB(file, 2),
    /object|对象|options/i,
    '旧位置参数构造器必须明确拒绝',
  )

  const db = new TriviumDB(file, { dim: 2 })
  db.insertWithId(1, [1, 0], { kind: 'a', score: 1, tenant: 't', region: 'r' })
  db.insertWithId(2, [0.9, 0.1], { kind: 'b', score: 2, tenant: 't', region: 'r' })
  db.link(1, 2, 'next', 1)

  db.createIndex('kind')
  db.createOrderedIndex('score')
  db.createCompositeIndex(['tenant', 'region'])
  db.createBitmapIndex('region')
  const indexes = db.indexInfo()
  assert.equal(indexes.length, 4)
  assert.deepEqual(indexes.find(item => item.kind === 'composite').fields, ['tenant', 'region'])

  const storage = db.storageInfo()
  assert.equal(storage.database_format_current, 7)
  assert.equal(storage.property_index_format, 4)
  assert.equal(storage.node_count, 2)

  const prepared = db.prepareTql('FIND {kind: "a"} RETURN $bonus + 1 AS score')
  assert.deepEqual(prepared.parameterNames(), ['bonus'])
  const preparedRows = db.executePreparedTql(prepared, { bonus: 4 })
  assert.equal(preparedRows.length, 1)
  assert.throws(() => db.executePreparedTql(prepared, {}), /missing parameter|缺少参数/)

  const pathRows = db.tql('SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed shortest_paths seed TO [2] LABEL next AS route WITH route RETURN path(route) AS path')
  assert.deepEqual(pathRows[0].path, ['1', '2'])

  const advancedFiltered = db.searchAdvanced([1, 0], {
    topK: 2,
    minScore: 0,
    payloadFilter: { kind: 'b' },
  })
  assert.equal(advancedFiltered.length, 1)
  assert.equal(advancedFiltered[0].payload.kind, 'b')

  const contextFiltered = db.searchWithContext([1, 0], {
    topK: 2,
    minScore: 0,
    payloadFilter: { kind: 'b' },
  })
  assert.equal(contextFiltered.hits.length, 1)
  assert.equal(contextFiltered.hits[0].payload.kind, 'b')

  const basicFiltered = db.search([1, 0], 2, 0, 0, { kind: 'b' })
  assert.equal(basicFiltered.length, 1)
  assert.equal(basicFiltered[0].payload.kind, 'b')

  assert.throws(
    () => db.searchAdvanced([1, 0], { payloadFilter: { score: { $gt: true } } }),
    /payloadFilter|过滤器|Filter/i,
  )

  assert.throws(
    () => db.tqlMut('FIND {kind: "a"} RETURN *'),
    /TDB_API_MIGRATION_REQUIRED/,
  )
  db.close()
} finally {
  fs.rmSync(root, { recursive: true, force: true })
}
