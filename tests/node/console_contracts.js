'use strict'

// 真实原生模块与共享独立样例验证控制台协议、持久化和实例绑定。
const assert = require('node:assert/strict')
const path = require('node:path')
const { TriviumDB, ConsoleSession } = require('../../index.js')
const fixture = require('../contracts/console_cases.json')

function runConsoleContract(root, api = { TriviumDB, ConsoleSession }) {
  const { TriviumDB: Database, ConsoleSession: Session } = api
  const wire = command => JSON.stringify({ version: 1, requestId: '验收请求', command })
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
        assert.throws(reenter, error => error.code === 'TDB_IO' && error.details.io_kind === 'WouldBlock')
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
        error => error.code === 'TDB_IO' && error.details.io_kind === 'WouldBlock')
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
      error => error.code === 'TDB_QUERY_CANCELLED')
    assert.equal(cancelCallbacks, 1)
    assert.equal(session.cancel('验收请求'), false)
    assert.deepEqual(db.getPayload(1), { owner: 'host' })
  } finally { db.close() }
}

module.exports = { runConsoleContract }
