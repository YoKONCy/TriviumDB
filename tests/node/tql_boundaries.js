'use strict'
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const contract = require('../contracts/tql_boundary_cases.json')
const aliases = require('../contracts/tql_alias_cases.json')

module.exports = function tqlBoundaries(root, TriviumDB) {
  const open = (name, extra = {}) => new TriviumDB(path.join(root, `boundaries-${name}.tdb`), { dim: 1, autoBuildQuiver: false, ...extra })
  const aliasDb = open('aliases')
  try {
    for (const record of aliases.records) aliasDb.insertWithId(record.id, [1], record.payload)
    for (const [a, b] of aliases.edges) aliasDb.link(a, b, 'rel', 1)
    for (const indexed of [false, true]) {
      if (indexed) aliasDb.createIndex('x')
      for (const test of aliases.cases) {
        const result = aliasDb.tql(test.query)
        if (test.ids) assert.deepEqual(result.map(row => row.id), test.ids, test.query)
        else assert.deepEqual(result, test.rows, test.query)
      }
    }
    for (const query of aliases.invalid) assert.throws(() => aliasDb.tql(query), Error)
  } finally { aliasDb.close() }
  const db = open('values')
  try {
    for (const row of contract.records) db.insertWithId(row.id, [1], row.payload)
    db.upsertEdge(1, 2, 'rel', 2, contract.edge_metadata)
    for (const source of contract.sources) for (const test of contract.cases) {
      const query = source.query + ' ' + test.suffix.replaceAll('{var}', source.variable)
      assert.deepEqual(db.tql(query), test.rows, query)
    }
    for (const indexed of [false, true]) {
      if (indexed) db.createIndex('x')
      for (const test of contract.edge_cases) assert.deepEqual(db.tql(test.query), test.rows, test.query)
    }
    const edge = db.tql('MATCH (a)-[e:rel]->(b) RETURN COALESCE(e,NULL) AS edge')[0].edge
    assert.deepEqual(edge, { type: 'edge', source: '1', target: '2', label: 'rel', weight: 2, metadata: contract.edge_metadata })
    for (const query of ['MATCH (n) RETURN 1 AS x, 2 AS x', 'MATCH (n) RETURN 1, 1', 'MATCH (n) RETURN $unbound', 'MATCH (n)-[n]->(b) RETURN n']) assert.throws(() => db.tql(query), Error)
    const prepared = db.prepareTql('MATCH (n {id:1}) RETURN $a, $b')
    assert.deepEqual(db.executePreparedTql(prepared, { a: 1, b: 1 }), [{ '$a': 1, '$b': 1 }])
  } finally { db.close() }

  const ids = open('id-index')
  try {
    for (const id of [1, 2, 3]) ids.insertWithId(id, [1], { kind: 'sample', id: 4 - id })
    ids.createIndex('id'); ids.createOrderedIndex('id')
    assert.deepEqual(ids.tql('FIND {kind:"sample"} RETURN _.id AS id ORDER BY _.id ASC LIMIT 1'), [{ id: 1 }])
    assert.deepEqual(ids.tql('SEARCH VECTOR [1] TOP 3 AS n WITH n WHERE n.id == 1 RETURN n.id AS id'), [{ id: 1 }])
  } finally { ids.close() }

  const numeric = open('numeric-index')
  try {
    // 80 个候选覆盖索引选择路径；TQL 写入使 2 号节点保留浮点表示。
    for (let id = 1; id <= 80; id++) numeric.insertWithId(id, [1], { x: id === 2 ? 1 : id })
    numeric.tqlMut('MATCH (n {id:2}) SET n.x == 1.0')
    for (const indexed of [false, true]) {
      if (indexed) numeric.createIndex('x')
      for (const literal of ['1', '1.0']) assert.deepEqual(numeric.tql(`MATCH (n) AS n WITH n WHERE n.x == ${literal} RETURN n.id AS id ORDER BY id ASC`), [{ id: 1 }, { id: 2 }])
    }
  } finally { numeric.close() }

  const file = path.join(root, 'boundaries-full-id.tdb')
  new TriviumDB(file, { dim: 2, autoBuildQuiver: false }).close()
  fs.copyFileSync(path.join(__dirname, '../fixtures/numeric_ids/full_u64_v3.bin'), file + '.wal')
  const large = new TriviumDB(file, { dim: 2, autoBuildQuiver: false })
  try {
    const before = fs.readFileSync(file + '.wal')
    assert.deepEqual(large.tql('MATCH (n {id:-3}) RETURN n'), [])
    assert.equal(large.tqlMut('MATCH (n {id:-3}) SET n.mark == false').affected, 0)
    assert.deepEqual(fs.readFileSync(file + '.wal'), before)
    large.tqlMut('MATCH (n {id:18446744073709551613}) SET n.mark == true')
    assert.deepEqual(large.tql('MATCH (n {id:18446744073709551613}) RETURN n.id AS id, n.nested AS nested, n.mark AS mark'),
      [{ id: '18446744073709551613', nested: { large: '18446744073709551613' }, mark: true }])
  } finally { large.close() }

  const vector = open('u64', { dtype: 'u64' })
  try {
    vector.insertWithId(1, [16777216], {}); vector.insertWithId(2, [16777217], {})
    for (const query of ['SEARCH VECTOR [16777217] TOP 1 AS n WITH n RETURN n.id AS id', 'MATCH (n) RANK n BY VECTOR [16777217] TOP 1 RETURN n.id AS id']) assert.deepEqual(vector.tql(query), [{ id: 2 }])
    const prepared = vector.prepareTql('SEARCH VECTOR [$q] TOP 1 AS n WITH n RETURN n.id AS id')
    assert.deepEqual(vector.executePreparedTql(prepared, { q: 16777217 }), [{ id: 2 }])
    vector.tqlMut('MATCH (n {id:1}) SET VECTOR(n) == [18446744073709551614]')
    vector.tqlMut('MATCH (n {id:2}) SET VECTOR(n) == [18446744073709551615]')
    assert.deepEqual(vector.tql('SEARCH VECTOR [18446744073709551614] TOP 1 AS n WITH n RETURN n.id AS id'), [{ id: 1 }])
    for (const literal of ['-1', '1.5', '18446744073709551616']) assert.throws(() => vector.tql(`SEARCH VECTOR [${literal}] TOP 1 RETURN *`), Error)
  } finally { vector.close() }
}
