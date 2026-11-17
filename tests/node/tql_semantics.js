'use strict'
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const contract = require('../contracts/tql_return_cases.json')

module.exports = function tqlSemantics(root, TriviumDB) {
  require('./tql_boundaries.js')(root, TriviumDB)
  const options = { dim: contract.dimension, autoBuildQuiver: false }
  const open = (name, extra = {}) => new TriviumDB(path.join(root, `semantics-${name}.tdb`), { ...options, ...extra })
  const db = open('returns')
  try {
    for (const row of contract.records) db.insertWithId(row.id, [1], row.payload)
    db.insertWithId(99, [0], { kind: 'root' })
    for (const row of contract.records) db.link(99, row.id, 'item', row.id)
    const plan = db.tql('EXPLAIN SEARCH VECTOR [1] TOP 1 RETURN *')[0].plan.payload
    assert.equal(plan.vector_access_path, 'exact_fallback')
    assert.equal(plan.access_path.kind, 'full_node_scan')
    for (const source of contract.sources) {
      for (const test of contract.cases) {
        const query = source.query + ' ' + test.suffix.replaceAll('{var}', source.variable)
        assert.deepEqual(db.tql(query), test.rows, query)
      }
      const n = source.variable
      assert.deepEqual(db.tql(`${source.query} RETURN ${n}.g AS g, COUNT(*) AS ${n} ORDER BY ${n}.g ASC`),
        [{ g: 'a', [n]: 1 }, { g: 'b', [n]: 2 }])
    }
  } finally { db.close() }

  const indexed = open('indexed-order')
  try {
    for (const row of contract.indexed_order.records) indexed.insertWithId(row.id, [1], row.payload)
    for (const target of [2, 3]) indexed.link(1, target, 'related', 3.4028234663852886e38)
    for (let phase = 0; phase <= contract.indexed_order.indexes.length; phase++) {
      if (phase > 0) indexed.createOrderedIndex(contract.indexed_order.indexes[phase - 1])
      for (const test of contract.indexed_order.cases) assert.deepEqual(indexed.tql(test.query), test.rows, test.query)
    }
  } finally { indexed.close() }

  const updates = open('set')
  try {
    updates.insertWithId(1, [1], { a: 0, b: 0, keep: true })
    assert.equal(updates.tqlMut('MATCH (n) SET n.a == 1, n.b == 2, n.a == 3').affected, 1)
    assert.deepEqual(updates.getPayload(1), { a: 3, b: 2, keep: true })
  } finally { updates.close() }
  const reopened = open('set')
  try { assert.deepEqual(reopened.getPayload(1), { a: 3, b: 2, keep: true }) } finally { reopened.close() }

  const vector = open('vector', { dtype: 'u64' })
  try {
    vector.insertWithId(1, [7], {})
    for (const value of [16777217, 9007199254740991]) {
      vector.tqlMut(`MATCH (n) SET VECTOR(n) == [${value}]`)
      assert.deepEqual(vector.get(1).vector, [value])
    }
    const wal = fs.readFileSync(path.join(root, 'semantics-vector.tdb.wal'))
    for (const value of ['-1', '1.5', '18446744073709551616']) {
      assert.throws(() => vector.tqlMut(`MATCH (n) SET VECTOR(n) == [${value}]`), Error)
      assert.deepEqual(vector.get(1).vector, [9007199254740991])
      assert.deepEqual(fs.readFileSync(path.join(root, 'semantics-vector.tdb.wal')), wal)
    }
  } finally { vector.close() }

  const budget = open('budget', { maxQueryRows: 2, memoryLimitMb: 1 })
  try {
    for (let id = 1; id <= 3; id++) budget.insertWithId(id, [1], { x: id })
    for (const source of ['MATCH (n)', 'MATCH (n) AS n WITH n']) {
      assert.throws(() => budget.tql(`${source} RETURN n`), Error)
      assert.deepEqual(budget.tql(`${source} RETURN n.id AS id LIMIT 3`), [{ id: 1 }, { id: 2 }, { id: 3 }])
      assert.throws(() => budget.tql(`${source} RETURN n LIMIT 2 OFFSET 10000`), Error)
    }
  } finally { budget.close() }

  const create = open('create', { maxQueryRows: 1 })
  try {
    assert.deepEqual(create.tqlMut('MATCH (n {kind:"seed"}) CREATE (m {kind:"copy"})'), { affected: 0, createdIds: [] })
    for (let id = 1; id <= 3; id++) create.insertWithId(id, [1], { kind: 'seed' })
    assert.deepEqual(create.tqlMut('MATCH (n {kind:"seed"}) CREATE (n)-[:copy]->(m {kind:"copy"})'),
      { affected: 6, createdIds: [4, 5, 6] })
  } finally { create.close() }
  const created = open('create')
  try {
    assert.deepEqual(created.tql('MATCH (n)-[e:copy]->(m) RETURN n.id AS source, m.id AS target ORDER BY n.id ASC'),
      [{ source: 1, target: 4 }, { source: 2, target: 5 }, { source: 3, target: 6 }])
  } finally { created.close() }
}
