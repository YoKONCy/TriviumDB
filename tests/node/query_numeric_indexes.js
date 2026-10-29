'use strict'

const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

module.exports = function queryNumericIndexes(root, TriviumDB) {
  const options = { dim: 2, autoBuildQuiver: false }
  const file = path.join(root, 'tql-numeric.tdb')
  const db = new TriviumDB(file, options)
  try {
    db.insertWithId(1, [1, 0], { tenant: 't', key: 'a', tiny: 1e-20 })
    db.insertWithId(2, [1, 0], { tenant: 't', key: 'b', tiny: 0 })
    db.createUniqueCompositeIndex(['tenant', 'key'])
    db.commitTransaction([
      { type: 'updatePayload', id: 1, payload: { tenant: 't', key: 'b' } },
      { type: 'updatePayload', id: 1, payload: { tenant: 't', key: 'c', tiny: 1e-20 } },
    ])
    const before = fs.readFileSync(file + '.wal')
    assert.throws(() => db.insertWithId(3, [1, 0], { tenant: 't', key: 'b' }), Error)
    assert.deepEqual(fs.readFileSync(file + '.wal'), before)
    assert.deepEqual(db.getPayload(2), { tenant: 't', key: 'b', tiny: 0 })
    assert.deepEqual(db.tql('MATCH (n) WHERE n.tiny > 0 AND n.tiny == 0.0 RETURN n'), [])
    assert.equal(db.tql('MATCH (n) WHERE n.id == 1 RETURN 1.0 / n.tiny AS result')[0].result, 1e20)
    const numeric = db.tql('MATCH (n) RETURN 18446744073709551613 AS id, 9223372036854775809 AS value, -9223372036854775808 AS negative LIMIT 1')[0]
    assert.deepEqual(numeric, { id: '18446744073709551613', value: '9223372036854775809', negative: '-9223372036854775808' })
    const collected = db.tql('MATCH (n) RETURN collect(18446744073709551613) AS ids')[0].ids
    assert.deepEqual(collected, ['18446744073709551613', '18446744073709551613'])
    db.close()
    const reopened = new TriviumDB(file, options)
    try {
      assert.equal(reopened.getPayload(1).key, 'c')
      assert.equal(reopened.getPayload(2).key, 'b')
      assert.throws(() => reopened.insertWithId(3, [1, 0], { tenant: 't', key: 'b' }), Error)
    } finally { reopened.close() }
  } finally { db.close() }

  const mixed = new TriviumDB(path.join(root, 'typed-ranges.tdb'), options)
  try {
    for (let i = 0; i < 10; i++) mixed.insertWithId(i + 1, [1, 0], { v: i })
    mixed.insertWithId(11, [1, 0], { v: 'a' })
    mixed.createOrderedIndex('v')
    assert.deepEqual(mixed.tql('FIND {v: {$lt: "z"}} RETURN * LIMIT 1').map(row => row._.id), ['11'])
  } finally { mixed.close() }

  const nestedFile = path.join(root, 'tql-nested-integers.tdb')
  new TriviumDB(nestedFile, options).close()
  fs.copyFileSync(path.join(__dirname, '../fixtures/numeric_ids/full_u64_v3.bin'), nestedFile + '.wal')
  const nested = new TriviumDB(nestedFile, options)
  try {
    assert.deepEqual(nested.tql('MATCH (n) WHERE n.id > 0 RETURN n.id AS id, n.unsigned AS value'),
      [{ id: '18446744073709551613', value: '18446744073709551615' }])
    assert.deepEqual(nested.tql('MATCH (n) RETURN collect(n.nested) AS blobs')[0].blobs,
      [{ large: '18446744073709551613' }])
  } finally { nested.close() }

  const budgetFile = path.join(root, 'index-build-budget.tdb')
  const budget = new TriviumDB(budgetFile, options)
  try {
    // 512 个独立标量键至少占 1.5 MiB，向上取整后的配额余量小于 1 MiB。
    for (let i = 1; i <= 512; i++) budget.insertWithId(i, [1, 0], { key: `${i}-${'x'.repeat(3072)}` })
    const before = budget.estimatedMemory()
    budget.setMemoryLimit(Math.ceil(before / (1024 * 1024)))
    const wal = fs.readFileSync(budgetFile + '.wal')
    assert.throws(() => budget.createIndex('key'), Error)
    assert.deepEqual(budget.indexInfo(), [])
    assert.equal(budget.estimatedMemory(), before)
    assert.deepEqual(fs.readFileSync(budgetFile + '.wal'), wal)
  } finally { budget.close() }
}
