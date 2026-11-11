'use strict'

const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

function runQuerySources(root, { TriviumDB, ConsoleSession }) {
  const { cases } = require('../contracts/query_execution_diagnostics.json')
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `query-sources-${dtype}.tdb`)
    const options = { dim: 2, dtype, autoBuildQuiver: false, maxQueryRows: 1, rowOverflow: 'throw' }
    const db = new TriviumDB(file, options)
    try {
      db.insertWithId(7, [1, 0], { kind: 'alpha' })
      db.insertWithId(8, [0, 1], { kind: 'beta' })
      db.createUniqueIndex('kind')
      const session = new ConsoleSession('{"readOnly":false}')
      const before = fs.readFileSync(`${file}.wal`)
      for (const test of cases) {
        const actions = []
        if (test.mutation) actions.push(() => db.tqlMut(test.query))
        else {
          actions.push(() => db.tql(test.query))
          if (test.prepare_error) actions.push(() => db.prepareTql(test.query))
          else {
            const prepared = db.prepareTql(test.query)
            actions.push(() => db.executePreparedTql(prepared, {}))
          }
        }
        const request = JSON.stringify({ version: 1, requestId: test.name, command: {
          operation: test.mutation ? 'mutate' : 'query', query: test.query,
        } })
        actions.push(() => db.consoleRequest(session, request))
        for (const action of actions) assert.throws(action, error => {
          assert.equal(error.schema_version, 1)
          assert.equal(error.code, test.code, test.name)
          assert.equal(error.stage, test.stage, test.name)
          assert.deepEqual(error.target, { query_span: test.span }, test.name)
          assert.deepEqual(error.write, test.write, test.name)
          assert.ok(error.details && Object.keys(error.details).length > 0)
          return true
        })
        assert.deepEqual(fs.readFileSync(`${file}.wal`), before, test.name)
      }
      const prepared = db.prepareTql("FIND {kind: 'alpha'} RETURN $x AS value")
      assert.deepEqual(db.executePreparedTql(prepared, { x: 42 }), [{ value: 42 }])
    } finally { db.close() }
    const reopened = new TriviumDB(file, options)
    try {
      assert.deepEqual(reopened.allNodeIds(), [7, 8])
      for (const [id, vector, kind] of [[7, [1, 0], 'alpha'], [8, [0, 1], 'beta']]) {
        const node = reopened.get(id)
        assert.deepEqual(node.vector, vector)
        assert.deepEqual(node.payload, { kind })
        assert.deepEqual(node.edges, [])
      }
    } finally { reopened.close() }
  }
}

module.exports = { runQuerySources }
