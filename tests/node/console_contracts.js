'use strict'

// 真实原生模块与共享独立样例验证控制台协议、持久化和实例绑定。
const assert = require('node:assert/strict')
const path = require('node:path')
const { TriviumDB, ConsoleSession } = require('../../index.js')
const fixture = require('../contracts/console_cases.json')
const editing = require('../contracts/console_editing_cases.json')
const admission = require('../contracts/error_cases.json').admission

function runEditingContract(root, Database, Session) {
  const wire = (command, requestId = '编辑请求') => JSON.stringify({ version: 1, requestId, command })
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `editing-${dtype}.tdb`)
    const db = new Database(file, { dim: editing.dimension, dtype, autoBuildQuiver: false })
    const session = new Session('{"readOnly":false}')
    const call = (database, current, command) => JSON.parse(database.consoleRequest(current, wire(command))).data
    const read = (database, current) => call(database, current, { operation: 'getNodes', ids: ['1'], withRevision: true })
    let command
    try {
      db.insertWithId(1, editing.vector, editing.basePayload)
      const old = read(db, session)
      assert.deepEqual(old.nodes, [{ id: '1', vector: dtype === 'u64' ? ['1', '0'] : editing.vector, payload: editing.basePayload, edges: [] }])
      assert.deepEqual(old.missingIds, [])
      const info = call(db, session, { operation: 'info' })
      assert.equal(info.transport.occ, true)
      assert.equal(info.transport.idempotency.maxReceipts, editing.receiptCapacity)
      db.updatePayload(1, editing.hostPayload)
      command = { operation: 'guardedTransaction', expectedRevision: old.revision, idempotencyKey: 'edit-1', operations: [
        { op: 'insert', vector: editing.vector, payload: editing.createdPayload },
        { op: 'updatePayload', id: '1', payload: editing.draftPayload },
      ] }
      assert.throws(() => db.consoleRequest(session, wire(command)), error => error.code === editing.conflictCode)
      assert.equal(db.nodeCount(), 1)
      assert.deepEqual(db.getPayload(1), editing.hostPayload)
      command.expectedRevision = read(db, session).revision
      const committed = call(db, session, command)
      assert.deepEqual(committed.createdIds, [editing.createdId])
      assert.equal(committed.replayed, false)
      assert.notEqual(committed.revision, command.expectedRevision)
      const other = new Session('{"readOnly":false}')
      const replay = JSON.parse(db.consoleRequest(other, wire(command, '重新确认')))
      assert.equal(replay.requestId, '重新确认')
      assert.deepEqual(replay.data, { ...committed, replayed: true })
      assert.deepEqual(call(db, other, { ...command, operation: 'transactionStatus' }), {
        outcome: 'committed', revision: committed.revision, createdIds: [editing.createdId],
      })
      const changed = JSON.parse(JSON.stringify(command))
      changed.operations[1].payload = { owner: '换内容' }
      assert.throws(() => db.consoleRequest(other, wire(changed)), error => error.code === editing.keyConflictCode)
      assert.throws(() => db.consoleRequest(other, wire({ ...changed, operation: 'transactionStatus' })), error => error.code === editing.keyConflictCode)
      db.flush()
      assert.equal(read(db, other).revision, committed.revision)
    } finally { db.close() }
    const reopened = new Database(file, { dim: editing.dimension, dtype, autoBuildQuiver: false })
    try {
      const fresh = new Session('{"readOnly":false}')
      assert.throws(() => reopened.consoleRequest(fresh, wire(command)), error => error.code === editing.conflictCode)
      assert.deepEqual(call(reopened, fresh, { ...command, operation: 'transactionStatus' }), { outcome: 'unknown' })
      assert.equal(reopened.nodeCount(), 2)
      for (const [id, payload] of [[1, editing.draftPayload], [2, editing.createdPayload]]) {
        const node = reopened.get(id)
        assert.deepEqual(node.payload, payload)
        assert.deepEqual(node.vector, editing.vector)
        assert.deepEqual(node.edges, [])
      }
    } finally { reopened.close() }
  }
}

function runConsoleContract(root, api = { TriviumDB, ConsoleSession }) {
  const { TriviumDB: Database, ConsoleSession: Session } = api
  const wire = command => JSON.stringify({ version: 1, requestId: '验收请求', command })
  runEditingContract(root, Database, Session)
  for (const [index, test] of fixture.cases.entries()) {
    const file = path.join(root, `console-${index}.tdb`)
    const db = new Database(file, { dim: 2, autoBuildQuiver: false })
    const session = new Session(JSON.stringify(test.options))
    if (test.detached) session.detach()
    const verify = database => {
      const rows = database.tql('MATCH (n) RETURN n')
      const fresh = new Session()
      for (const expected of test.verify || []) {
        const native = rows.find(row => row.n.id === expected.id)
        assert.ok(native)
        assert.deepEqual(native.n.payload, expected.payload)
        const node = JSON.parse(database.consoleRequest(fresh, wire({ operation: 'getNodes', ids: [expected.id] }))).data.nodes[0]
        assert.equal(node.id, expected.id)
        assert.deepEqual(node.vector, expected.vector)
        assert.deepEqual(node.payload, expected.payload)
        if (expected.edges) assert.deepEqual(node.edges, expected.edges)
      }
      for (const id of test.absent || []) assert.ok(rows.every(row => row.n.id !== id))
    }
    try {
      for (const node of fixture.setup) db.insertWithId(node.id, node.vector, node.payload)
      if (test.error) {
        assert.throws(() => db.consoleRequest(session, wire(test.command)), error => {
          assert.equal(error.code, test.error.code, test.name)
          assert.deepEqual(error.details, test.error.details, test.name)
          return true
        })
      } else {
        assert.deepEqual(JSON.parse(db.consoleRequest(session, wire(test.command))), { version: 1, requestId: '验收请求', data: test.expected }, test.name)
      }
      verify(db)
      db.close()
      const reopened = new Database(file, { dim: 2, autoBuildQuiver: false })
      try {
        verify(reopened)
        assert.throws(() => reopened.consoleRequest(session, wire({ operation: 'info' })), error => error.code === (test.detached ? 'TDB_CONSOLE_SESSION_CLOSED' : 'TDB_INVALID_INPUT'))
      } finally { reopened.close() }
      session.detach()
      assert.equal(session.isDetached(), true)
      assert.equal(session.cancel('验收请求'), false)
    } finally { db.close() }
  }
  for (const dtype of ['f16', 'u64']) {
    const db = new Database(path.join(root, `console-${dtype}.tdb`), { dim: 2, dtype })
    try {
      db.insertWithId(1, [1, 0], { dtype })
      const session = new Session()
      const node = JSON.parse(db.consoleRequest(session, wire({ operation: 'getNodes', ids: ['1'] }))).data.nodes[0]
      assert.deepEqual(node, { id: '1', vector: dtype === 'u64' ? ['1', '0'] : [1, 0], payload: { dtype }, edges: [] })
      if (dtype === 'f16') {
        const writable = new Session('{"readOnly":false}')
        db.consoleRequest(writable, wire({ operation: 'importNodes', records: [{ id: '2', vector: [1.0007, 1.0004882812500002], payload: {} }] }))
        assert.deepEqual(db.get(2).vector, [1.0009765625, 1])
      }
    } finally { db.close() }
  }

  const db = new Database(path.join(root, 'console-reentry.tdb'), { dim: 2 })
  try {
    db.insertWithId(1, [1, 0], { owner: 'host' })
    const session = new Session()
    let callbacks = 0
    db.setHook({ onPreSearch(query) {
      callbacks++
      for (const reenter of [() => db.nodeCount(), () => db.dtype, () => db.close(), () => db.consoleRequest(session, wire({ operation: 'info' }))]) {
        assert.throws(reenter, { code: admission.reentrant_code, schema_version: 1, details: { operation: admission.node_operation } })
      }
      return query
    } })
    const result = JSON.parse(db.consoleRequest(session, wire({ operation: 'search', vector: [1, 0], topK: 1 })))
    assert.equal(callbacks, 1)
    assert.deepEqual(result.data.hits, [{ id: '1', score: 1, payload: { owner: 'host' } }])
    let reverseCallbacks = 0
    db.setHook({ onPreSearch(query) {
      reverseCallbacks++
      assert.throws(() => db.consoleRequest(session, wire({ operation: 'info' })),
        { code: admission.reentrant_code, schema_version: 1, details: { operation: admission.node_operation } })
      return query
    } })
    assert.deepEqual(db.search([1, 0], 1), [{ id: 1, score: 1, payload: { owner: 'host' } }])
    assert.equal(reverseCallbacks, 1)
    assert.equal(JSON.parse(db.consoleRequest(session, wire({ operation: 'info' }))).data.readOnly, true)
    let cancelCallbacks = 0
    db.setHook({ onPreSearch(query) {
      cancelCallbacks++
      assert.equal(session.cancel('验收请求'), true)
      return query
    } })
    assert.throws(() => db.consoleRequest(session, wire({ operation: 'search', vector: [1, 0], topK: 1 })),
      error => {
        assert.equal(error.schema_version, 1)
        assert.equal(error.code, 'TDB_QUERY_CANCELLED')
        assert.deepEqual(error.details, {})
        assert.equal(error.write, undefined)
        return true
      })
    assert.equal(cancelCallbacks, 1)
    assert.equal(session.cancel('验收请求'), false)
    db.clearHook()
    const resumed = JSON.parse(db.consoleRequest(session, wire({ operation: 'getNodes', ids: ['1'] })))
    assert.deepEqual(resumed.data, { nodes: [{ id: '1', vector: [1, 0], payload: { owner: 'host' }, edges: [] }], missingIds: [] })
    assert.deepEqual(db.getPayload(1), { owner: 'host' })
  } finally { db.close() }
}

module.exports = { runConsoleContract }
