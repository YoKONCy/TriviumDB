'use strict'
// 原生 SDK 验证持久化类型、完整语句与打开预算的共享契约。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const contract = require('../contracts/kernel_boundaries.json')

function runKernelBoundaries(root, TriviumDB = require('../../index.js').TriviumDB, ResourceRuntime = require('../../index.js').ResourceRuntime) {
  require('./system_boundaries.js').runSystemBoundaries(root, { TriviumDB, ResourceRuntime })
  const graphContract = require('../contracts/graph_query_boundaries.json')
  for (const storageMode of ['rom', 'mmap']) {
    const file = path.join(root, `graph-query-${storageMode}.tdb`)
    const db = new TriviumDB(file, { dim: 2, storageMode, autoBuildQuiver: false })
    for (const node of graphContract.nodes) db.insertWithId(node.id, [1, 0], { kind: node.kind })
    for (const [source, target, label] of graphContract.edges) db.link(source, target, label, 1)
    for (const indexed of [false, true]) {
      if (indexed) db.createIndex('kind')
      for (const { query, expected } of graphContract.counts) assert.deepEqual(db.tql(query), [{ total: expected }])
    }
    for (const invalid of [NaN, Infinity, -Infinity]) {
      for (const action of [
        () => db.search([invalid, 0], 1, 0, -1),
        () => db.searchExact([invalid, 0], 1),
        () => db.searchGraphFirst([invalid, 0], [], 1),
        () => db.searchGraphFirst([invalid, 0], [1], 1),
      ]) assert.throws(action, error => error.code === 'TDB_INVALID_VECTOR' && typeof error.details.reason === 'string' && error.details.reason.length > 0)
    }
    assert.deepEqual(db.search([1, 0], Number.MAX_SAFE_INTEGER, 0, -1).map(hit => hit.id), [1, 2, 3])
    db.close()
    const reader = new TriviumDB(file, { dim: 2, storageMode, accessMode: 'readOnly' })
    for (const { query, expected } of graphContract.counts) assert.deepEqual(reader.tql(query), [{ total: expected }])
    reader.close()
  }
  const retrieval = require('../contracts/retrieval_boundaries.json')
  for (const [name, test] of Object.entries({ binary: retrieval.binary, metric: retrieval.metric })) {
    const file = path.join(root, `typed-retrieval-${name}.tdb`)
    const db = new TriviumDB(file, { dim: 1, dtype: 'u64', autoBuildQuiver: false })
    test.vectors.forEach((vector, index) => db.insertWithId(index + 1, vector, { index }))
    for (const result of [db.search([test.query], 2, 0, -1), db.searchExact([test.query], 2)]) {
      assert.deepEqual(result.map(hit => hit.id), test.expected_ids)
      assert.deepEqual(result.map(hit => hit.score), test.expected_scores)
    }
    db.setAutoBuildQuiver(true)
    db.close()
    const reader = new TriviumDB(file, { dim: 1, dtype: 'u64' })
    assert.deepEqual(reader.search([test.query], 2, 0, -1).map(hit => hit.id), test.expected_ids)
    reader.close()
  }
  for (const [index, scale] of retrieval.float_scales.entries()) {
    const file = path.join(root, `float-scale-${index}.tdb`)
    const db = new TriviumDB(file, { dim: 2, autoBuildQuiver: false })
    db.insertWithId(1, [scale, 0], { self: true })
    for (const result of [db.searchExact([scale, 0], 1), db.search([scale, 0], 1, 0, -1)]) {
      assert.deepEqual(result.map(hit => [hit.id, hit.score]), [[1, 1]])
    }
    db.close()
  }
  {
    const sourceFile = path.join(root, 'migrate-source.tdb')
    const targetFile = path.join(root, 'migrate-existing.tdb')
    const source = new TriviumDB(sourceFile, { dim: 1, autoBuildQuiver: false })
    source.insertWithId(1, [1], { origin: 'source' })
    const target = new TriviumDB(targetFile, { dim: 2, autoBuildQuiver: false })
    target.insertWithId(2, [0, 1], { origin: 'target' }); target.close()
    const snapshot = () => fs.readdirSync(root).filter(name => name.startsWith('migrate-existing.tdb')).sort().map(name => [name, fs.readFileSync(path.join(root, name))])
    const before = snapshot()
    assert.throws(() => source.migrate(targetFile, 2), error => error.code === 'TDB_IO' && error.details.io_kind === 'AlreadyExists')
    assert.deepEqual(snapshot(), before)
    source.close()
  }
  const schema = require('../contracts/index_schema_boundaries.json')
  for (const storageMode of ['rom', 'mmap']) {
    for (const unique of [false, true]) {
      const file = path.join(root, `schema-fields-${storageMode}-${unique}.tdb`)
      const options = { dim: 2, storageMode, autoBuildQuiver: false }
      const db = new TriviumDB(file, options)
      db.insertWithId(1, [1, 0], schema.payload)
      for (const fields of schema.definitions) {
        if (unique) db.createUniqueCompositeIndex(fields)
        else db.createCompositeIndex(fields)
      }
      db.close()
      for (const accessMode of ['readOnly', 'readWrite']) {
        const snapshot = () => fs.readdirSync(root).filter(name => name.startsWith(path.basename(file))).sort().map(name => [name, fs.readFileSync(path.join(root, name))])
        const before = snapshot()
        const reader = new TriviumDB(file, { ...options, accessMode })
        try {
          assert.deepEqual(reader.get(1).payload, schema.payload)
          const definitions = reader.indexInfo().map(item => item.fields)
          assert.deepEqual(definitions.sort(), schema.definitions.map(fields => [...fields]).sort())
          for (const fields of schema.definitions) {
            const equalities = Object.fromEntries(fields.map(field => [field, schema.payload[field]]))
            assert.deepEqual(reader.indexedLookup(equalities, 10), schema.expected_ids)
          }
        } finally { reader.close() }
        if (accessMode === 'readOnly') assert.deepEqual(snapshot(), before)
      }
    }
  }
  for (const dtype of contract.dtypes) {
    const file = path.join(root, `json-fields-${dtype}.tdb`)
    const db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    const special = schema.payload
    db.insertWithId(1, [1, 0], special)
    db.batchInsertWithIds([2, 3], [[0, 1], [1, 1]], [special, special])
    db.updatePayload(1, { wrapper: special })
    db.patchPayload(2, { $set: { wrapper: special } })
    db.compareAndSetPayloadField(2, 'a\0b', 1, 7)
    db.commitTransaction([{ type: 'updatePayload', id: 3, payload: special }])
    db.upsertEdge(1, 2, 'rel', 1, special)
    assert.deepEqual(db.getPayload(1), { wrapper: special })
    assert.equal(db.get(2).payload['a\0b'], 7)
    const actual = db.get(3).payload
    assert.deepEqual(actual, special)
    assert.equal(Object.getPrototypeOf(actual), Object.prototype)
    assert.ok(Object.hasOwn(actual, '__proto__'))
    assert.deepEqual(db.getEdge(1, 2, 'rel').metadata, special)
    assert.deepEqual(db.getEdges(1)[0].metadata, special)
    assert.deepEqual(db.getIncomingEdges(2)[0].metadata, special)
    assert.deepEqual(db.searchExact([1, 0], 1)[0].payload, { wrapper: special })
    assert.deepEqual(db.tql('MATCH (n {id:3}) RETURN n')[0].n.payload, special)
    const cycle = {}; cycle.self = cycle
    const before = fs.readFileSync(file + '.wal')
    for (const invalid of [cycle, { value: 1n }]) {
      assert.throws(() => db.insert([1, 0], invalid), error => error.code === 'InvalidArg')
      assert.deepEqual(fs.readFileSync(file + '.wal'), before)
    }
    db.close()
    for (const storageMode of ['rom', 'mmap']) {
      const file = path.join(root, `kernel-${dtype}-${storageMode}.tdb`)
      const options = { dim: 2, dtype, storageMode, autoBuildQuiver: false }
      const db = new TriviumDB(file, options)
      try {
        for (const node of contract.nodes) db.insertWithId(node.id, node.vector, node.payload)
        const before = fs.readFileSync(file + '.wal')
        for (const query of contract.invalid_mutations) {
          assert.throws(() => db.tqlMut(query), error => error.code === 'TDB_QUERY_PARSE')
          assert.deepEqual(fs.readFileSync(file + '.wal'), before)
          for (const node of contract.nodes) assert.deepEqual(db.get(node.id).payload, node.payload)
        }
        assert.throws(() => db.tql(contract.invalid_read), error => error.code === 'TDB_QUERY_PARSE')
        assert.throws(() => db.tql('MATCH (n) WHERE ' + 'NOT '.repeat(16384) + 'n.x == 1 RETURN n'), error => error.code === 'TDB_QUERY_PARSE')
      } finally { db.close() }
      for (const requested of contract.dtypes.filter(value => value !== dtype)) {
        assert.throws(() => new TriviumDB(file, { ...options, dtype: requested }), error => {
          assert.equal(error.code, 'TDB_DTYPE_MISMATCH')
          assert.deepEqual(error.details, { stored: dtype, requested })
          return true
        })
      }
      const reopened = new TriviumDB(file, options)
      try {
        for (const node of contract.nodes) {
          assert.deepEqual(reopened.get(node.id).vector, node.vector)
          assert.deepEqual(reopened.get(node.id).payload, node.payload)
        }
      } finally { reopened.close() }
    }
  }
  const file = path.join(root, 'kernel-load-budget.tdb')
  const db = new TriviumDB(file, { dim: 2, autoBuildQuiver: false })
  db.insertWithId(1, [1, 0], { text: 'x'.repeat(2 * 1024 * 1024) })
  db.close()
  const snapshot = () => fs.readdirSync(root).filter(name => name.startsWith('kernel-load-budget.tdb')).sort().map(name => [name, fs.readFileSync(path.join(root, name))])
  const before = snapshot()
  assert.throws(() => new TriviumDB(file, { dim: 2, memoryLimitMb: 1 }), error => {
    assert.equal(error.code, 'TDB_CAPACITY_RESERVATION_REJECTED')
    assert.equal(error.details.memory_limit, 1024 * 1024)
    assert.ok(error.details.current_bytes + error.details.estimated_bytes > error.details.memory_limit)
    return true
  })
  assert.deepEqual(snapshot(), before)
}

module.exports = { runKernelBoundaries }
