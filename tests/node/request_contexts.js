'use strict'

// 实际原生模块验证任务局部上下文、转换回调取消和异步完成点。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const fixture = require('../contracts/request_contexts.json')

module.exports = async function requestContexts(root, api = require('../../index.js')) {
  const { TriviumDB, RequestContext } = api
  assert.throws(() => new RequestContext(''), error => error.code === 'TDB_INVALID_INPUT' && error.target.field === 'requestId')
  for (const dtype of ['f32', 'f16', 'u64']) {
    const file = path.join(root, `typed-request-${dtype}.tdb`)
    const db = new TriviumDB(file, { dim: fixture.dimension, dtype, autoBuildQuiver: false })
    db.insertWithId(fixture.id, fixture.vector, fixture.payload)
    const before = fs.readFileSync(file + '.wal')
    const context = new RequestContext(fixture.requestId)
    assert.equal(context.requestId, fixture.requestId)
    let conversions = 0
    const payload = { get body() { conversions++; context.cancel(); return '取消正文' } }
    assert.throws(() => context.run(() => db.updatePayload(fixture.id, payload)), error => {
      assert.equal(error.code, fixture.cancelCode)
      assert.deepEqual(error.write, fixture.admission)
      return true
    })
    assert.equal(conversions, 1)
    assert.deepEqual(fs.readFileSync(file + '.wal'), before)
    assert.deepEqual(db.getPayload(fixture.id), fixture.payload)
    assert.throws(() => context.run(() => db.commitTransaction([{ type: 'delete', id: fixture.id }])), error => {
      assert.equal(error.code, fixture.cancelCode)
      assert.deepEqual(error.write, fixture.transactionAdmission)
      return true
    })
    const child = new RequestContext('child')
    assert.throws(() => context.run(() => child.run(() => db.get(fixture.id))), error => error.code === fixture.cancelCode)
    await assert.rejects(context.run(() => db.searchBatch([fixture.vector])), error => error.code === fixture.cancelCode)
    const cause = new Error('宿主原异常')
    assert.throws(() => child.run(() => { throw cause }), error => error === cause)
    const nested = (depth, operation) => depth === 0 ? operation() : child.run(() => nested(depth - 1, operation))
    assert.equal(nested(32, () => db.get(fixture.id)).id, fixture.id)
    assert.throws(() => nested(33, () => db.get(fixture.id)), error => error.code === 'TDB_INVALID_INPUT' && error.target.field === 'operation')

    let release
    const gate = new Promise(resolve => { release = resolve })
    const waiting = context.run(async () => { await gate; return db.get(fixture.id) })
    const independent = await child.run(async () => { await Promise.resolve(); return db.get(fixture.id) })
    assert.deepEqual(independent.payload, fixture.payload)
    release()
    await assert.rejects(waiting, error => error.code === fixture.cancelCode)

    const asynchronous = new RequestContext('async-batch')
    let convertedQueries = 0
    const queries = []
    Object.defineProperty(queries, 0, { enumerable: true, get() {
      convertedQueries++
      asynchronous.cancel()
      return fixture.vector
    } })
    const running = asynchronous.run(() => db.searchBatch(queries))
    await assert.rejects(running, error => error.code === fixture.cancelCode)
    assert.equal(convertedQueries, 1)
    const synchronous = new RequestContext('sync-hook')
    let hookCalls = 0
    db.setHook({ onPreSearch(query) {
      hookCalls++
      synchronous.cancel()
      return query
    } })
    assert.throws(() => synchronous.run(() => db.search(fixture.vector)), error => error.code === fixture.cancelCode)
    assert.equal(hookCalls, 1)
    db.clearHook()
    const hits = await child.run(() => db.searchBatch([fixture.vector, fixture.vector], 1, 1, -1))
    // 两个 u64 分量共有 128 个相同位，浮点向量的自相似度为 1。
    const score = dtype === 'u64' ? fixture.binaryScore : fixture.floatScore
    assert.deepEqual(hits.map(rows => rows.map(({ id, score, payload }) => ({ id, score, payload }))), [
      [{ id: fixture.id, score, payload: fixture.payload }], [{ id: fixture.id, score, payload: fixture.payload }],
    ])
    context.run(() => db.close())
    const renamed = file + '.saved'
    for (const name of fs.readdirSync(root).filter(name => name === path.basename(file) || name.startsWith(path.basename(file) + '.'))) {
      fs.renameSync(path.join(root, name), path.join(root, name.replace(path.basename(file), path.basename(renamed))))
    }
    const reopened = new TriviumDB(renamed, { dim: fixture.dimension, dtype, accessMode: 'readOnly' })
    assert.deepEqual(reopened.allNodeIds(), [fixture.id])
    assert.deepEqual(reopened.get(fixture.id), { id: fixture.id, vector: fixture.vector, payload: fixture.payload, edges: [], numEdges: 0 })
    reopened.close()
  }
}
