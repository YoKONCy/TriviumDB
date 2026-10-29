'use strict'

// 真实原生 API 与共享独立样例核对错误码、字段及同步/异步一致性。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const { TriviumDB } = require('../../index.js')
const contract = require('../contracts/error_cases.json')

function assertError(error, test) {
  assert.equal(error.code, test.code, test.operation)
  const details = { ...error.details }
  for (const field of test.string_fields || []) {
    assert.equal(typeof details[field], 'string', test.operation)
    assert.ok(details[field].length > 0, test.operation)
    delete details[field]
  }
  for (const field of test.integer_fields || []) {
    assert.ok(Number.isInteger(details[field]), test.operation)
    delete details[field]
  }
  assert.deepEqual(details, test.details, test.operation)
  for (const [field, value] of Object.entries(error.details)) assert.deepEqual(error[field], value)
  return true
}

async function runErrorContract(root) {
  for (const test of contract.cases) {
    const directory = path.join(root, `errors-${test.operation}`)
    fs.mkdirSync(directory)
    const file = path.join(directory, 'database.tdb')
    let db = new TriviumDB(file, { dim: 2, autoBuildQuiver: false })
    try {
      db.insertWithId(1, [1, 0], { kind: 'alpha' })
      db.insertWithId(2, [0, 1], { kind: 'beta' })
      if (['readonly', 'rows'].includes(test.operation)) {
        db.close()
        db = new TriviumDB(file, { dim: 2, accessMode: 'readOnly', maxQueryRows: 1 })
      }
      if (test.operation === 'unique') db.createUniqueIndex('kind')
      if (test.operation === 'closed') db.close()
      if (test.operation === 'io') fs.writeFileSync(path.join(directory, 'blocked'), '文件占据父路径')
      if (test.operation === 'version') {
        const header = Buffer.alloc(58)
        header.write('TVDB')
        header.writeUInt16LE(65535, 4)
        fs.writeFileSync(path.join(directory, 'future.tdb'), header)
      }
      const actions = {
        missing_parameter: () => db.executePreparedTql(db.prepareTql('MATCH (n) RETURN $required AS value'), {}),
        unexpected_parameter: () => db.executePreparedTql(db.prepareTql('MATCH (n) RETURN $required AS value'), { required: 1, extra: 2 }),
        dimension: () => db.insert([1], {}),
        missing_node: () => db.updatePayload(999, {}),
        duplicate_node: () => db.insertWithId(1, [1, 0], {}),
        unique: () => db.insertWithId(3, [1, 0], { kind: 'alpha' }),
        readonly: () => db.insert([1, 0], {}),
        rows: () => db.tql('MATCH (n) RETURN n'),
        migration: () => db.tqlMut('MATCH (n) RETURN n'),
        closed: () => db.getPayload(1),
        locked: () => new TriviumDB(file, { dim: 2 }),
        io: () => new TriviumDB(path.join(directory, 'blocked/database.tdb'), { dim: 2 }),
        version: () => new TriviumDB(path.join(directory, 'future.tdb'), { dim: 2 }),
      }
      assert.throws(actions[test.operation], error => assertError(error, test))
      if (test.operation === 'dimension') {
        await assert.rejects(db.searchBatch([[1]], 1, 1, 0), error => assertError(error, test))
      }
    } finally {
      db.close()
    }
  }
  const integers = new TriviumDB(path.join(root, 'error-u64-vectors.tdb'), { dim: 1, dtype: 'u64', autoBuildQuiver: false })
  try {
    integers.insertWithId(1, [1], {})
    const invalid = error => {
      assert.equal(error.code, 'TDB_INVALID_INPUT')
      assert.deepEqual(Object.keys(error.details), ['reason'])
      assert.equal(typeof error.details.reason, 'string')
      assert.ok(error.details.reason.length > 0)
      return true
    }
    for (const component of [-1, 0.5, NaN, Infinity]) {
      assert.throws(() => integers.insertWithId(2, [component], {}), invalid)
      await assert.rejects(integers.searchBatch([[component]], 1, 1, 0), invalid)
      assert.equal(integers.getPayload(2), null)
    }
    for (const component of [-1, 0.5]) {
      assert.throws(() => integers.commitTransaction([{ type: 'insertWithId', id: 2, vector: [component], payload: {} }]), invalid)
      assert.equal(integers.getPayload(2), null)
    }
    // JSON 参数转换在进入事务构造前拒绝非有限值。
    for (const component of [NaN, Infinity]) {
      assert.throws(() => integers.commitTransaction([{ type: 'insertWithId', id: 2, vector: [component], payload: {} }]),
        error => error instanceof Error && error.code === 'InvalidArg')
      assert.equal(integers.getPayload(2), null)
    }
  } finally {
    integers.close()
  }
}

module.exports = { runErrorContract }
