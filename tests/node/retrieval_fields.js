'use strict'

// 实际包验证检索字段、Promise 完成、文本重建和完整持久化事实。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const fixture = require('../contracts/retrieval_fields.json')
const text = require('../contracts/index_maintenance_cases.json')
const { invalid } = require('./query_arguments')
const validation = { phase: 'validation', outcome: 'not_committed' }

function rejectReentry(db) {
  assert.throws(() => db.get(7), error => error.code === 'TDB_REENTRANT_ACCESS' && error.write === undefined)
  assert.throws(() => db.indexText(7, '回调'), error => {
    assert.equal(error.code, 'TDB_REENTRANT_ACCESS')
    assert.deepEqual(error.write, { phase: 'admission', outcome: 'not_committed' })
    return true
  })
}

function hits(result, score) {
  assert.deepEqual(result, [{ id: 7, score, payload: fixture.query_seed.payload }])
}

function defaultGraphHits(result) {
  // 默认图扩散配置验证完整事实与评分有效性，精确分数由零扩散样例和内核算法契约验证。
  assert.deepEqual(result.map(({ id, payload }) => ({ id, payload })), [{ id: 7, payload: fixture.query_seed.payload }])
  assert.ok(Number.isFinite(result[0].score) && result[0].score > 0)
}

function queryActions(db) {
  return [
    [value => db.search(value, 1, 0, 0), result => result, 'queryVector'],
    [value => db.searchGrouped(value, 1, 0, 0), result => result.semanticHits, 'queryVector'],
    [value => db.searchBatch(value, 1, 1, 0), result => result[0], 'queryVectors'],
    [value => db.searchExact(value, 1), result => result, 'queryVector'],
    [value => db.searchGraphFirst(value, [7], 1), result => result, 'queryVector'],
    [value => db.searchWithContext(value, { topK: 1, expandDepth: 0 }), result => result.hits, 'queryVector'],
    [value => db.searchAdvanced(value, { topK: 1, expandDepth: 0, enableAdvancedPipeline: false }), result => result, 'queryVector'],
    [value => db.searchHybrid(value, '', 1, 0, 0), result => result, 'queryVector'],
  ]
}

async function runRetrievalFields(root, { TriviumDB, ConsoleSession, verifySnapshot, restoreSnapshot }) {
  for (const dtype of ['f32', 'f16', 'u64']) for (const storageMode of ['rom', 'mmap']) {
    const directory = path.join(root, `retrieval-fields-${dtype}-${storageMode}`)
    const file = path.join(directory, 'database.tdb')
    const options = { dim: 2, dtype, storageMode, autoBuildQuiver: false }
    const db = new TriviumDB(file, options)
    const score = dtype === 'u64' ? 128 : 1
    let held
    try {
      db.insertWithId(7, [1, 0], fixture.query_seed.payload)
      const wal = fs.readFileSync(file + '.wal')
      const session = new ConsoleSession()
      for (const [action, field, write] of [
        [() => db.migrate(1, 2), 'newPath', validation],
        [() => db.migrate(path.join(directory, 'unused.tdb'), '2'), 'newDim', validation],
        [() => db.createSnapshot(1, 'generation'), 'destination', validation],
        [() => db.createSnapshot(path.join(directory, 'unused'), 1), 'generationId', validation],
        [() => db.closeTimeout('0'), 'timeoutMs'], [() => db.loadFfiHook(1), 'libPath'],
        [() => db.consoleRequest(session, 1), 'requestJson'], [() => db.consoleRequest(null, '{}'), 'session'],
        [() => db.executePreparedTql({}, {}), 'prepared'],
        [() => session.cancel(1), 'requestId'], [() => verifySnapshot(1), 'directory'],
        [() => restoreSnapshot(1, 'unused'), 'source', validation],
        [() => restoreSnapshot('unused', 1), 'destination', validation],
      ]) assert.throws(action, invalid(field, write))
      for (const [operations, field] of [
        [null, 'operations'], [[{}], '/operations/0/type'],
        [[{ type: 'insert', vector: [1, 'x'] }], '/operations/0/vector/1'],
        [[{ type: 'insertWithId', id: '7', vector: [1, 0] }], '/operations/0/id'],
        [[{ type: 'link', src: 7, dst: 7, label: true }], '/operations/0/label'],
        [[{ type: 'link', src: 7, dst: 7, weight: '1' }], '/operations/0/weight'],
      ]) assert.throws(() => db.commitTransaction(operations), invalid(field, { ...validation, transaction_consumed: false }))
      for (const item of fixture.index_fields) for (const value of [null, 1, '\ud800']) {
        assert.throws(() => db[item.node](value), invalid('field', validation))
      }
      for (const item of fixture.composite_fields) {
        assert.throws(() => db[item.node](null), invalid('fields', validation))
        const fields = ['saved', 1]
        Object.defineProperty(fields, 1, { get() { rejectReentry(db); return 1 } })
        assert.throws(() => db[item.node](fields), invalid('/fields/1', validation))
      }
      for (const [field, value] of [['minCommunitySize', '3'], ['maxIterations', -1], ['withCentroids', 'true']]) {
        assert.throws(() => db.leidenCluster({ [field]: value }), invalid(field))
      }
      const clustered = db.leidenCluster({ get minCommunitySize() { rejectReentry(db); return 1 }, withCentroids: false })
      assert.deepEqual(clustered.nodeToCluster, [7, 1])
      assert.deepEqual(clustered.clusterLabels, ['1', 'Cluster 1 (1)'])
      assert.deepEqual(clustered.centroids, [])
      for (const [action, unpack, field] of queryActions(db)) {
        const values = field === 'queryVectors'
          ? [[null, field], [[null], `/${field}/0`], [[[1, 0], [1, 'x']], `/${field}/1/1`]]
          : [[null, field], ['10', field], [[1, 'x'], `/${field}/1`]]
        for (const [value, location] of values) assert.throws(() => action(value), invalid(location))
        const vector = [1, 0]
        let calls = 0
        Object.defineProperty(vector, 0, { get() { calls++; rejectReentry(db); return 1 } })
        hits(unpack(await action(field === 'queryVectors' ? [vector] : vector)), score)
        assert.equal(calls, 1)
      }
      for (const [value, field] of [[null, 'anchorIds'], [[7, 'x'], '/anchorIds/1']]) {
        assert.throws(() => db.searchGraphFirst([1, 0], value, 1), invalid(field))
      }
      if (dtype === 'u64') {
        const pending = db.searchBatch([[1, -1]], 1)
        assert.ok(pending instanceof Promise)
        await assert.rejects(pending, invalid('/queryVectors/0/1'))
      }
      for (const method of ['searchAdvanced', 'searchWithContext']) {
        for (const entry of fixture.floats) {
          for (const value of [null, '1', true, [], {}, NaN, Infinity, -Infinity, 1e100, -1e100]) {
            assert.throws(() => db[method]([1, 0], { [entry.node]: value }), invalid(entry.node))
          }
        }
        for (const entry of fixture.booleans) {
          for (const value of [null, 0, 1, 'false', [], {}]) {
            assert.throws(() => db[method]([1, 0], { [entry.node]: value }), invalid(entry.node))
          }
          const result = db[method]([1, 0], { topK: 1, expandDepth: 0, enableAdvancedPipeline: false, [entry.node]: false })
          hits(method === 'searchWithContext' ? result.hits : result, score)
        }
        for (const value of [[], 'config', true, 1, () => {}]) {
          assert.throws(() => db[method]([1, 0], value), invalid('config'))
        }
        for (const [bias, code] of [[[1], 'TDB_DIMENSION_MISMATCH'], [[NaN, 0], 'TDB_INVALID_VECTOR']]) {
          assert.throws(() => db[method]([1, 0], { diffusionBias: bias }), error => {
            assert.equal(error.code, code)
            assert.deepEqual(error.target, { field: 'diffusionBias' })
            assert.equal(error.write, undefined)
            return true
          })
        }
        for (const [field, value, location] of [
          ['expandLabels', 'route', 'expandLabels'], ['expandLabels', ['route', 1], '/expandLabels/1'],
          ['diffusionBias', 'vector', 'diffusionBias'], ['diffusionBias', [1, 'x'], '/diffusionBias/1'],
          ['customQueryText', 1, 'customQueryText'], ['customQueryText', '\ud800', 'customQueryText'],
          ['edgeDirection', 'sideways', 'edgeDirection'],
          ['payloadFilter', { 'a/b~c': [undefined] }, '/payloadFilter/a~1b~0c/0'],
        ]) assert.throws(() => db[method]([1, 0], { [field]: value }), invalid(location))
        let reads = 0
        const config = { expandDepth: 0, enableAdvancedPipeline: false, get minScore() { reads++; rejectReentry(db); return 0 } }
        const result = db[method]([1, 0], config)
        hits(method === 'searchWithContext' ? result.hits : result, score)
        assert.equal(reads, 1)
      }
      for (const action of [
        value => db.search([1, 0], 1, 0, value), value => db.searchGrouped([1, 0], 1, 0, value),
        value => db.searchBatch([[1, 0]], 1, 1, value), value => db.searchHybrid([1, 0], '', 1, 0, value),
      ]) assert.throws(() => action('1'), invalid('minScore'))
      for (const value of [NaN, Infinity, -Infinity]) {
        assert.throws(() => db.searchHybrid([1, 0], '', 1, 0, 0, value), invalid('hybridAlpha'))
        const pending = db.searchBatch([], 1, 1, value)
        assert.ok(pending instanceof Promise)
        await assert.rejects(pending, invalid('minScore'))
      }
      assert.deepEqual(await db.searchBatch([]), [])
      hits(db.search([1, 0], null, null, null, null), score)
      hits(db.searchGrouped([1, 0], null, null, null, null).semanticHits, score)
      hits((await db.searchBatch([[1, 0]], null, null, null))[0], score)
      defaultGraphHits(db.searchHybrid([1, 0], '', null, null, null, null, null))
      defaultGraphHits(db.searchAdvanced([1, 0], null))
      defaultGraphHits(db.searchWithContext([1, 0], null).hits)
      const cause = new Error('宿主 cause')
      const sentinel = Object.assign(new TypeError('检索 getter 原异常', { cause }), { code: 'HOST_GETTER', target: { host: true } })
      for (const action of [
        () => db.searchAdvanced([1, 0], { get minScore() { throw sentinel } }),
        () => db.searchWithContext([1, 0], { payloadFilter: { get key() { throw sentinel } } }),
      ]) assert.throws(action, error => {
        assert.equal(error, sentinel)
        assert.equal(error.cause, cause)
        assert.equal(error.code, 'HOST_GETTER')
        assert.deepEqual(error.target, { host: true })
        return true
      })
      held = db.get(7)
      assert.deepEqual(held, { ...fixture.query_seed, numEdges: 0, edges: [] })
      assert.deepEqual(fs.readFileSync(file + '.wal'), wal)
    } finally { db.close() }
    const moved = directory + '-moved'
    fs.renameSync(directory, moved)
    assert.deepEqual(held, { ...fixture.query_seed, numEdges: 0, edges: [] })
    for (let pass = 0; pass < 2; pass++) {
      const reopened = new TriviumDB(path.join(moved, 'database.tdb'), options)
      try {
        assert.deepEqual(reopened.allNodeIds(), [7])
        assert.deepEqual(reopened.get(7), { ...fixture.query_seed, numEdges: 0, edges: [] })
        reopened.flush()
      } finally { reopened.close() }
    }
  }
  runTextFields(root, TriviumDB)
}

function textFacts(db, after) {
  assert.deepEqual(db.allNodeIds(), [1, 2])
  for (const value of text.nodes) {
    assert.deepEqual(db.get(value.id), {
      id: value.id, vector: value.vector, payload: value.payload, numEdges: value.edges.length,
      edges: value.edges.map(edge => ({ targetId: edge.target, label: edge.label, weight: edge.weight, metadata: edge.metadata })),
    })
  }
  for (const query of text.queries) {
    const rows = db.tql(`TEXT ${query.mode} "${query.term}" TOP 10 AS hits WITH hits RETURN hits`)
    assert.deepEqual(rows.map(row => Number(row.hits.id)).sort((left, right) => left - right), query[after ? 'after' : 'before'])
  }
  assert.deepEqual(db.indexedLookup({ tag: 'alpha' }), [1])
  assert.deepEqual(db.substringLookup('tag', 'bet'), [2])
}

function runTextFields(root, TriviumDB) {
  for (const dtype of ['f32', 'f16', 'u64']) for (const storageMode of ['rom', 'mmap']) {
    const directory = path.join(root, `text-fields-${dtype}-${storageMode}`)
    const file = path.join(directory, 'database.tdb')
    const options = { dim: 2, dtype, storageMode, autoBuildQuiver: false, loadTextIndex: true }
    const db = new TriviumDB(file, options)
    let held, heldError
    try {
      for (const value of text.nodes) db.insertWithId(value.id, value.vector, value.payload)
      db.link(1, 2)
      db.rebuildTextIndex([{ id: 1, text: 'oldalpha', keywords: ['oldanchor'] }, { id: 2, text: 'oldbeta' }])
      db.createIndex('tag')
      db.createNgramIndex('tag')
      db.buildTextIndex()
      const wal = fs.readFileSync(file + '.wal')
      assert.deepEqual(db.indexedLookup({ tag: 'alpha' }, null), [1])
      assert.deepEqual(db.substringLookup('tag', 'bet', null), [2])
      for (const [method, field] of [['indexText', 'text'], ['indexKeyword', 'keyword']]) {
        for (const value of ['1', -1, 1n, Number.MAX_SAFE_INTEGER + 1]) assert.throws(() => db[method](value, 'x'), invalid('id', validation))
        assert.throws(() => db[method](1, 1), invalid(field, validation))
        assert.throws(() => db[method](1, '\ud800'), invalid(field, validation))
      }
      for (const entry of fixture.bad_documents) {
        assert.throws(() => db.rebuildTextIndex(entry.value), invalid(entry.field, validation))
        textFacts(db, false)
      }
      let unknownReads = 0
      assert.throws(() => db.rebuildTextIndex([{ id: 1, text: 'x', get unknown() { unknownReads++; throw new Error('未知字段值应在读取前拒绝') } }]),
        invalid('/documents/0/unknown', validation))
      assert.equal(unknownReads, 0)
      for (const value of [null, 1, 'documents']) assert.throws(() => db.rebuildTextIndex(value), invalid('documents', validation))
      assert.throws(() => db.rebuildTextIndex([{ id: 1, text: '\ud800' }]), invalid('/documents/0/text', validation))
      for (const action of [value => db.indexedLookup({ tag: 'alpha' }, value), value => db.substringLookup('tag', 'bet', value)]) {
        for (const value of ['1', -1, NaN, 0, 1000001]) assert.throws(() => action(value), invalid('maxResults'))
      }
      assert.throws(() => db.indexedLookup([]), invalid('equalities'))
      assert.throws(() => db.indexedLookup({ 'a/b~c': undefined }), invalid('/equalities/a~1b~0c'))
      assert.throws(() => db.substringLookup(1, 'x'), invalid('field'))
      assert.throws(() => db.substringLookup('tag', 1), invalid('needle'))
      assert.deepEqual(fs.readFileSync(file + '.wal'), wal)
      const cause = new Error('宿主 cause')
      const sentinel = new TypeError('文本 getter 原异常', { cause })
      assert.throws(() => db.rebuildTextIndex([{ id: 1, get text() { throw sentinel } }]), error => error === sentinel && error.cause === cause)
      let calls = 0
      const records = [{ id: 1, get text() {
        calls++
        assert.throws(() => db.indexText(1, '回调'), error => error.code === 'TDB_REENTRANT_ACCESS' && error.write.outcome === 'not_committed')
        return 'nexttext'
      }, keywords: ['nextanchor'] }, { id: 2, text: 'oldbeta' }]
      db.rebuildTextIndex(records)
      assert.equal(calls, 1)
      db.indexText(1, 'nexttext')
      db.indexKeyword(1, 'nextanchor')
      db.buildTextIndex()
      textFacts(db, true)
      held = db.get(1)
      assert.throws(() => db.indexText(1, 1), error => { heldError = error; return invalid('text', validation)(error) })
      db.flush()
    } finally { db.close() }
    const moved = directory + '-moved'
    fs.renameSync(directory, moved)
    assert.deepEqual(held.payload, text.nodes[0].payload)
    assert.deepEqual(heldError.target, { field: 'text' })
    for (let pass = 0; pass < 2; pass++) {
      const reopened = new TriviumDB(path.join(moved, 'database.tdb'), options)
      try { textFacts(reopened, true); reopened.flush() } finally { reopened.close() }
    }
  }
}

module.exports = { runRetrievalFields }
