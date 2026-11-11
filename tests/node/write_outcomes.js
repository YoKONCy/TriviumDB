'use strict'

// 验证真实原生入口、构造前拒绝及已提交回执的响应边界。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const contract = require('../contracts/write_outcomes.json')

function check(code, details, write) {
  return error => {
    assert.equal(error.schema_version, 1)
    assert.equal(error.code, code)
    assert.deepEqual(error.details, details)
    assert.deepEqual(error.write, write)
    return true
  }
}

function runWriteOutcomes(root, { TriviumDB, ConsoleSession }) {
  require('./index_maintenance').runIndexMaintenance(root, TriviumDB)
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `write-outcomes-${dtype}.tdb`)
    const db = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    try {
      db.insertWithId(1, [1, 0], { state: 'saved' })
      const before = fs.readFileSync(`${file}.wal`)
      assert.throws(() => db.insert([1], {}),
        check('TDB_DIMENSION_MISMATCH', { expected: 2, got: 1 }, contract.direct_validation))
      assert.throws(() => db.updatePayload(999, {}),
        check('TDB_NODE_NOT_FOUND', { id: 999 }, contract.direct_validation))
      assert.throws(() => db.tqlMut('MATCH (n) SET VECTOR(n) == [1]'),
        check('TDB_DIMENSION_MISMATCH', { expected: 2, got: 1 }, contract.direct_validation))
      assert.throws(() => db.commitTransaction([{ type: 'insertWithId', id: 2, vector: [1], payload: {} }]),
        check('TDB_DIMENSION_MISMATCH', { expected: 2, got: 1 }, contract.validation))
      assert.deepEqual(fs.readFileSync(`${file}.wal`), before)
      if (dtype === 'u64') {
        assert.throws(() => db.commitTransaction([{ type: 'insertWithId', id: 2, vector: [-1, 0], payload: {} }]), error => {
          assert.equal(error.code, 'TDB_INVALID_INPUT')
          assert.equal(error.schema_version, 1)
          assert.equal(typeof error.details.reason, 'string')
          assert.deepEqual(error.write, contract.binding_validation)
          return true
        })
      }
      let converted = false
      const payload = { get state() {
        assert.throws(() => db.commitTransaction([]), check('TDB_REENTRANT_ACCESS', { operation: 'node_callback' }, contract.admission))
        converted = true
        return 'valid'
      } }
      assert.deepEqual(db.commitTransaction([{ type: 'insertWithId', id: 2, vector: [0, 1], payload }]), [2])
      assert.equal(converted, true)
      const session = new ConsoleSession('{"readOnly":false}')
      const call = (session, command) => db.consoleRequest(session, JSON.stringify({ version: 1, requestId: '写入结果', command }))
      const revision = JSON.parse(call(session, { operation: 'getNodes', ids: ['1'], withRevision: true })).data.revision
      const command = { operation: 'guardedTransaction', idempotencyKey: 'response-outcome', expectedRevision: revision,
        operations: [{ op: 'updatePayload', id: '1', payload: { state: 'committed' } }] }
      const ack = JSON.parse(call(session, command)).data
      const small = new ConsoleSession('{"readOnly":false,"maxResponseBytes":1}')
      assert.throws(() => call(small, command), check('TDB_CONSOLE_RESPONSE_TOO_LARGE', { max_bytes: 1, committed: true }, contract.response))
      assert.deepEqual(JSON.parse(call(session, { ...command, operation: 'transactionStatus' })).data,
        { outcome: 'committed', createdIds: [], revision: ack.revision })
      db.close()
      assert.throws(() => db.insert([1, 0], {}), check('TDB_DATABASE_CLOSED', {}, contract.direct_admission))
      assert.throws(() => db.commitTransaction([]), check('TDB_DATABASE_CLOSED', {}, contract.admission))
    } finally {
      db.close()
    }
    const reopened = new TriviumDB(file, { dim: 2, dtype, autoBuildQuiver: false })
    try {
      assert.deepEqual(reopened.allNodeIds(), [1, 2])
      for (const [id, vector, state] of [[1, [1, 0], 'committed'], [2, [0, 1], 'valid']]) {
        const node = reopened.get(id)
        assert.deepEqual(node.vector, vector)
        assert.deepEqual(node.payload, { state })
        assert.deepEqual(node.edges, [])
      }
    } finally {
      reopened.close()
    }
    const readonly = new TriviumDB(file, { dim: 2, dtype, accessMode: 'readOnly', autoBuildQuiver: false })
    try {
      assert.throws(() => readonly.insertWithId(3, [1, 0], {}),
        check('TDB_READ_ONLY', { operation: 'insert_with_id' }, contract.direct_admission))
      assert.throws(() => readonly.commitTransaction([{ type: 'insertWithId', id: 3, vector: [1, 0], payload: {} }]),
        check('TDB_READ_ONLY', { operation: 'transaction commit' }, contract.admission))
      assert.deepEqual(readonly.allNodeIds(), [1, 2])
    } finally {
      readonly.close()
    }
  }
}

module.exports = { runWriteOutcomes }
