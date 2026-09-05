'use strict'

// Node 公共 API 跨层契约测试：覆盖 Prepared TQL、索引/存储诊断、
// u64 字符串映射、Payload Filter 和已移除入口的稳定迁移错误。

const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const contract = require('../contracts/public_cases.json')
const { TriviumDB } = require('../../index.js')

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'triviumdb-node-public-api-'))
const file = path.join(root, 'api.tdb')

function runSharedContract(root) {
  assert.equal(contract.schema_version, 1)
  const db = new TriviumDB(path.join(root, 'shared-contract.tdb'), {
    dim: contract.setup.dimension,
  })
  for (const node of contract.setup.nodes) {
    db.insertWithId(node.id, node.vector, node.payload)
  }
  for (const edge of contract.setup.edges) {
    db.link(edge.source, edge.target, edge.label, edge.weight)
  }
  for (const testCase of contract.cases) {
    const expected = testCase.expected
    if (testCase.operation === 'get_payload') {
      const payload = db.getPayload(testCase.node_id)
      assert.equal(payload[expected.field], expected.value, testCase.name)
    } else if (testCase.operation === 'prepared_tql') {
      const prepared = db.prepareTql(testCase.tql)
      const rows = db.executePreparedTql(prepared, testCase.parameters || {})
      assert.equal(rows.length, expected.row_count, testCase.name)
      assert.equal(rows[0][expected.column], expected.value, testCase.name)
    } else if (testCase.operation === 'tql_path') {
      const rows = db.tql(testCase.tql)
      assert.deepEqual(rows[0].path, expected.path.map(String), testCase.name)
    } else if (testCase.operation === 'prepared_missing_parameter') {
      const prepared = db.prepareTql(testCase.tql)
      assert.throws(
        () => db.executePreparedTql(prepared, testCase.parameters || {}),
        error => expected.error_contains_any.some(needle => String(error).includes(needle)),
        testCase.name,
      )
    } else {
      throw new Error(`未知共享契约操作: ${testCase.operation}`)
    }
  }
  db.close()
}

try {
  runSharedContract(root)
  assert.throws(
    () => new TriviumDB(file, 2),
    /object|对象|options/i,
    '旧位置参数构造器必须明确拒绝',
  )

  const db = new TriviumDB(file, { dim: 2 })
  db.insertWithId(1, [1, 0], { kind: 'a', score: 1, tenant: 't', region: 'r' })
  db.insertWithId(2, [0.9, 0.1], { kind: 'b', score: 2, tenant: 't', region: 'r' })
  db.link(1, 2, 'next', 1)

  const stages = []
  db.setHook({
    onPreSearch(query) {
      stages.push('pre')
      return query
    },
    onCustomRecall() {
      stages.push('custom')
      return null
    },
    onPostRecall(hits) {
      stages.push('postRecall')
      return hits
    },
    onPreGraphExpand(hits) {
      stages.push('preGraph')
      return hits
    },
    onRerank(hits) {
      stages.push('rerank')
      return hits
    },
    onPostSearch(hits) {
      stages.push('post')
      return hits.slice(0, 1)
    },
  })
  const hooked = db.searchWithContext([1, 0], {
    topK: 2,
    expandDepth: 1,
    minScore: 0,
  })
  assert.equal(hooked.hits.length, 1)
  assert.deepEqual(new Set(stages), new Set([
    'pre', 'custom', 'postRecall', 'preGraph', 'rerank', 'post',
  ]))
  db.clearHook()

  db.setHook({
    onPreSearch() {
      throw new Error('node-hook-failure')
    },
  })
  assert.throws(
    () => db.searchWithContext([1, 0], { topK: 2 }),
    /TDB_HOOK_EXECUTION|node-hook-failure/,
  )
  db.clearHook()

  db.createIndex('kind')
  db.createOrderedIndex('score')
  db.createCompositeIndex(['tenant', 'region'])
  db.createBitmapIndex('region')
  const indexes = db.indexInfo()
  assert.equal(indexes.length, 4)
  assert.deepEqual(indexes.find(item => item.kind === 'composite').fields, ['tenant', 'region'])

  const storage = db.storageInfo()
  assert.equal(storage.database_format_current, 9)
  assert.equal(storage.property_index_format, 6)
  assert.equal(storage.node_count, 2)

  const vectorPrepared = db.prepareTql('SEARCH VECTOR [$x, $y] TOP 1 RETURN *')
  assert.deepEqual(vectorPrepared.parameterNames(), ['x', 'y'])
  const vectorRows = db.executePreparedTql(vectorPrepared, { x: 1, y: 0 })
  assert.equal(vectorRows[0]._.id, '1')

  const prepared = db.prepareTql('FIND {kind: "a"} AS seed WITH seed RETURN seed, $bonus + 1 AS score')
  assert.deepEqual(prepared.parameterNames(), ['bonus'])
  const preparedRows = db.executePreparedTql(prepared, { bonus: 4 })
  assert.equal(preparedRows.length, 1)
  assert.equal(preparedRows[0].score, 5)
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
