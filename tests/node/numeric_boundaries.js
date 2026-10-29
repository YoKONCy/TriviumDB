'use strict'

const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

module.exports = async function numericBoundaries(root, TriviumDB) {
  require('./query_numeric_indexes.js')(root, TriviumDB)
  require('./tql_semantics.js')(root, TriviumDB)
  const options = { dim: 2, autoBuildQuiver: false }
  const file = path.join(root, 'numeric-inputs.tdb')
  const db = new TriviumDB(file, options)
  try {
    for (const id of [1, 2, 3]) db.insertWithId(id, [1, 0], { value: id })
    db.link(1, 2, 'edge', 1)
    // 小数、非有限值、负数与 u32 回绕边界必须在整数转换前拒绝。
    for (const value of [-1, 1.9, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
      for (const query of [
        () => db.searchExact([1, 0], value),
        () => db.searchGraphFirst([1, 0], [1], value),
        () => db.search([1, 0], value),
        () => db.searchAdvanced([1, 0], { topK: value }),
        () => db.searchWithContext([1, 0], { recallK: value }),
        () => db.searchGrouped([1, 0], 1, value),
        () => db.searchBatch([[1, 0]], value),
        () => db.searchBatch([[1, 0]], 1, value),
        () => db.neighbors(1, value),
        () => db.reachable(1, { maxDepth: value }),
        () => db.indexedLookup({ value: 1 }, value),
        () => db.substringLookup('value', '1', value),
        () => db.leidenCluster({ maxIterations: value }),
        () => db.enableAutoCompaction(value),
      ]) assert.throws(query, Error)
    }
    // 大于 u32 的合法数量保持原值，不能被截断为 1。
    assert.deepEqual(db.searchExact([1, 0], 2 ** 32 + 1).map(hit => hit.id), [1, 2, 3])
    assert.deepEqual(db.search([1, 0], 2 ** 32 + 1, 0, 0).map(hit => hit.id), [1, 2, 3])
    assert.deepEqual(db.neighbors(1, 1), [2])
    assert.deepEqual(db.searchExact([1, 0], 1).map(hit => hit.id), [1])
  } finally { db.close() }

  for (const storageMode of ['rom', 'mmap']) {
    const file = path.join(root, `numeric-outputs-${storageMode}.tdb`)
    new TriviumDB(file, { ...options, storageMode }).close()
    fs.copyFileSync(path.join(__dirname, '../fixtures/numeric_ids/wal_v3.bin'), file + '.wal')
    const db = new TriviumDB(file, { ...options, storageMode })
    try {
      const expected = ['1', '9007199254740991', '9007199254740992', '9007199254740993']
      assert.deepEqual(db.tql('FIND {group: "fixture"} RETURN *').map(row => row._.id).sort(), expected)
      assert.equal(db.getPayload(1).exact_id, '1')
      assert.equal(db.getPayload(Number.MAX_SAFE_INTEGER).exact_id, '9007199254740991')
      assert.deepEqual(db.searchGraphFirst([1, 0], [1], 4).map(hit => hit.id), [1])
      db.createIndex('group')
      db.createNgramIndex('group')
      db.createUniqueIndex('exact_id')
      assert.throws(() => db.updatePayload(1, { exact_id: '9007199254740993' }), error => {
        assert.equal(error.code, 'TDB_UNIQUE_CONSTRAINT')
        assert.deepEqual(error.details, { fields: ['exact_id'], existing_id: '9007199254740993', conflicting_id: 1 })
        assert.equal(error.existing_id, '9007199254740993')
        return true
      })
      assert.equal(db.getPayload(1).exact_id, '1')
      const unsafeId = error => error instanceof Error && error.code === 'TDB_INVALID_INPUT'
      for (const query of [
        () => db.searchExact([1, 0], 4),
        () => db.search([1, 0], 4, 0, 0),
        () => db.searchAdvanced([1, 0], { topK: 4, minScore: 0, expandDepth: 0, forceBruteForce: true }),
        () => db.searchWithContext([1, 0], { topK: 4, minScore: 0, expandDepth: 0 }),
        () => db.searchGrouped([1, 0], 4, 0, 0),
        () => db.allNodeIds(),
        () => db.get(1),
        () => db.getEdges(1),
        () => db.getIncomingEdges(1),
        () => db.neighbors(1, 1),
        () => db.reachable(1),
        () => db.reachableDetailed(1),
        () => db.querySubgraph(1),
        () => db.indexedLookup({ group: 'fixture' }),
        () => db.substringLookup('group', 'fixture'),
        () => db.leidenCluster(),
      ]) assert.throws(query, unsafeId, query.toString())
      await assert.rejects(() => db.searchBatch([[1, 0]], 4, 1, 0), unsafeId)
      const before = fs.readFileSync(file + '.wal')
      const destination = path.join(root, `migration-${storageMode}.tdb`)
      for (const write of [
        () => db.insert([1, 0], {}),
        () => db.batchInsert([[1, 0]], [{}]),
        () => db.commitTransaction([{ type: 'insert', vector: [1, 0], payload: {} }]),
        () => db.tqlMut('CREATE (n)'),
        () => db.migrate(destination, 4),
      ]) {
        assert.throws(write, unsafeId, write.toString())
        assert.deepEqual(fs.readFileSync(file + '.wal'), before)
        assert.equal(db.nodeCount(), 4)
      }
      assert.equal(fs.existsSync(destination), false)
      db.updatePayload(1, { exact_id: '1', updated: true })
      assert.equal(db.getPayload(1).updated, true)
    } finally { db.close() }
    const reader = new TriviumDB(file, { ...options, storageMode, accessMode: 'readOnly' })
    try {
      assert.throws(() => reader.allNodeIds(), error => error.code === 'TDB_INVALID_INPUT')
      assert.throws(() => reader.searchExact([1, 0], 4), error => error.code === 'TDB_INVALID_INPUT')
      assert.deepEqual(reader.tql('FIND {group: "fixture"} RETURN *').map(row => row._.id).sort(),
        ['9007199254740991', '9007199254740992', '9007199254740993'])
    } finally { reader.close() }
  }

  // 最后一个安全 ID 可以分配，跨越边界的批量/事务操作完整拒绝。
  for (const operation of ['batch', 'transaction', 'tql']) {
    const file = path.join(root, `numeric-allocator-${operation}.tdb`)
    const db = new TriviumDB(file, options)
    try {
      db.insertWithId(Number.MAX_SAFE_INTEGER - 1, [1, 0], {})
      const before = fs.readFileSync(file + '.wal')
      const write = operation === 'batch'
        ? () => db.batchInsert([[1, 0], [1, 0]], [{}, {}])
        : operation === 'tql' ? () => db.tqlMut('CREATE (a), (b)')
          : () => db.commitTransaction([{ type: 'insert', vector: [1, 0] }, { type: 'insert', vector: [1, 0] }])
      assert.throws(write, Error)
      assert.deepEqual(fs.readFileSync(file + '.wal'), before)
      assert.equal(db.nodeCount(), 1)
      assert.equal(db.insert([1, 0], {}), Number.MAX_SAFE_INTEGER)
      assert.throws(() => db.insert([1, 0], {}), Error)
      assert.deepEqual(db.allNodeIds().sort((a, b) => a - b), [Number.MAX_SAFE_INTEGER - 1, Number.MAX_SAFE_INTEGER])
    } finally { db.close() }
  }
}
