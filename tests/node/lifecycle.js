'use strict'

const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawnSync } = require('node:child_process')
const { TriviumDB } = require('../../index.js')

if (process.env.TRIVIUM_NODE_CHILD_PATH) {
  try {
    const db = new TriviumDB(process.env.TRIVIUM_NODE_CHILD_PATH, { dim: 2 })
    db.close()
    process.exit(2)
  } catch (error) {
    assert.match(String(error), /Database locked|数据库已锁定/)
    process.exit(0)
  }
}

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'triviumdb-node-lifecycle-'))
const dbPath = name => path.join(root, `${name}.tdb`)

try {
  for (const [dtype, vector] of [
    ['f32', [1, 0]],
    ['f16', [1, 0]],
    ['u64', [1, 0]],
  ]) {
    const file = dbPath(dtype)
    const db = new TriviumDB(file, { dim: 2, dtype })
    db.insert(vector, { dtype })
    db.close()
    db.close()
    const reopened = new TriviumDB(file, { dim: 2, dtype })
    assert.equal(reopened.nodeCount(), 1)
    reopened.dispose()
  }

  const disposedPath = dbPath('symbol-dispose')
  const disposable = new TriviumDB(disposedPath, { dim: 2 })
  disposable.insert([1, 0], {})
  assert.equal(typeof disposable.dispose, 'function')
  if (typeof Symbol.dispose === 'symbol') {
    assert.equal(typeof disposable[Symbol.dispose], 'function')
    disposable[Symbol.dispose]()
  } else {
    disposable.dispose()
  }
  const afterDispose = new TriviumDB(disposedPath, { dim: 2 })
  assert.equal(afterDispose.nodeCount(), 1)
  afterDispose.close()

  const exceptionPath = dbPath('exception')
  let exception
  const exceptional = new TriviumDB(exceptionPath, { dim: 2 })
  try {
    exceptional.insert([1, 0], {})
    throw new Error('sentinel')
  } catch (error) {
    exception = error
  } finally {
    exceptional.close()
  }
  assert.equal(exception.message, 'sentinel')
  const afterException = new TriviumDB(exceptionPath, { dim: 2 })
  afterException.close()

  const lockedPath = dbPath('cross-process')
  const owner = new TriviumDB(lockedPath, { dim: 2 })
  owner.insert([1, 0], { owner: true })
  owner.flush()
  const tracked = ['', '.vec', '.wal', '.flush_ok']
    .map(suffix => `${lockedPath}${suffix}`)
    .filter(fs.existsSync)
  const before = new Map(tracked.map(file => [file, fs.readFileSync(file)]))
  const child = spawnSync(process.execPath, [__filename], {
    env: { ...process.env, TRIVIUM_NODE_CHILD_PATH: lockedPath },
    encoding: 'utf8',
  })
  assert.equal(child.status, 0, child.stderr)
  for (const [file, bytes] of before) {
    assert.deepEqual(fs.readFileSync(file), bytes)
  }
  owner.close()

  const graphPath = dbPath('graph-controls')
  const graph = new TriviumDB(graphPath, { dim: 2 })
  const seed = graph.insert([1, 0], { name: 'seed' })
  const strong = graph.insert([0, 1], { name: 'strong' })
  const weak = graph.insert([-1, 0], { name: 'weak' })
  const incoming = graph.insert([0, -1], { name: 'incoming' })
  graph.link(seed, strong, 'allowed', 0.9)
  graph.link(seed, weak, 'allowed', 0.1)
  graph.link(incoming, seed, 'allowed', 0.8)
  const outgoing = graph.searchAdvanced([1, 0], {
    topK: 2,
    recallK: 1,
    rerankK: 1,
    expandDepth: 1,
    minScore: 0.9,
    maxEdgesPerNode: 1,
    minEdgeWeight: 0.5,
    edgeDirection: 'out',
    expandLabels: ['allowed'],
  })
  assert(outgoing.some(hit => hit.id === strong))
  assert(!outgoing.some(hit => hit.id === weak))
  const incomingHits = graph.searchAdvanced([1, 0], {
    topK: 2,
    recallK: 1,
    rerankK: 1,
    expandDepth: 1,
    minScore: 0.9,
    edgeDirection: 'in',
    expandLabels: ['allowed'],
  })
  assert(incomingHits.some(hit => hit.id === incoming))
  assert.throws(
    () => graph.searchAdvanced([1, 0], { minEdgeWeight: -0.1 }),
    /min_edge_weight/,
  )
  assert.throws(
    () => graph.searchAdvanced([1, 0], { edgeDirection: 'sideways' }),
    /edgeDirection/,
  )
  graph.close()
} finally {
  fs.rmSync(root, { recursive: true, force: true })
}
