'use strict'

// 实际绑定验证维护准入、候选拒绝、完整事实与重开持久化。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const contract = require('../contracts/index_maintenance_cases.json')

function execute(db, test) {
  const actions = {
    index_text: () => db.indexText(test.id, test.text),
    index_keyword: () => db.indexKeyword(test.id, test.keyword),
    build_text_index: () => db.buildTextIndex(),
    rebuild_text_index: () => db.rebuildTextIndex(contract.rebuilt_documents),
    create_index: () => db.createIndex(test.field),
    create_composite_index: () => db.createCompositeIndex(test.fields),
    create_bitmap_index: () => db.createBitmapIndex(test.field),
    create_ordered_index: () => db.createOrderedIndex(test.field),
    create_ngram_index: () => db.createNgramIndex(test.field),
    create_unique_index: () => db.createUniqueIndex(test.field),
    create_unique_composite_index: () => db.createUniqueCompositeIndex(test.fields),
    drop_index: () => db.dropIndex(test.field),
    drop_composite_index: () => db.dropCompositeIndex(test.fields),
    drop_bitmap_index: () => db.dropBitmapIndex(test.field),
    drop_ordered_index: () => db.dropOrderedIndex(test.field),
    drop_ngram_index: () => db.dropNgramIndex(test.field),
    flush: () => db.flush(),
    compact: () => db.compact(),
    set_sync_mode: () => db.setSyncMode(test.mode),
    reserve_nodes: () => db.reserveNodes(test.additional),
    repair_graph_indexes: () => db.repairGraphIndexes(),
    set_auto_build_quiver: () => db.setAutoBuildQuiver(test.enabled),
    enable_auto_compaction: () => db.enableAutoCompaction(test.interval_secs),
  }
  return actions[test.api]()
}

function facts(db) {
  const nodes = db.allNodeIds().sort((a, b) => a - b).map(id => {
    const node = db.get(id)
    return { id, vector: node.vector, payload: node.payload,
      edges: node.edges.map(edge => ({ target: edge.targetId, label: edge.label, weight: edge.weight, metadata: edge.metadata })) }
  })
  const queries = contract.queries.map(query => db.tql(
    `TEXT ${query.mode} "${query.term}" TOP 10 AS hits WITH hits RETURN hits`,
  ).map(row => Number(row.hits.id)).sort((a, b) => a - b))
  const indexes = db.indexInfo().map(({ kind, fields, unique }) => ({ kind, fields, unique }))
  return { nodes, queries, indexes }
}

function expected(after, indexes = []) {
  return { nodes: contract.nodes, queries: contract.queries.map(query => query[after ? 'after' : 'before']), indexes }
}

function check(code, details, write, reason = false, target) {
  return error => {
    assert.equal(error.schema_version, 1)
    assert.equal(error.code, code)
    if (reason) {
      assert.deepEqual(Object.keys(error.details), ['reason'])
      assert.equal(typeof error.details.reason, 'string')
      assert.ok(error.details.reason.length > 0)
    } else assert.deepEqual(error.details, details)
    assert.deepEqual(error.write, write)
    assert.deepEqual(error.target, target)
    return true
  }
}

function runIndexMaintenance(root, TriviumDB) {
  // 显式构建、运行期模式和任务捕获属于 Rust 入口，绑定验证已公开的维护操作。
  const operations = contract.operations.filter(test => !test.rust_only)
  for (const dtype of ['f32', 'f16', 'u64']) {
    for (const storageMode of ['rom', 'mmap']) {
      const file = path.join(root, `maintenance-${dtype}-${storageMode}.tdb`)
      const options = { dim: 2, dtype, storageMode, loadTextIndex: true, autoBuildQuiver: false, memoryLimitMb: 1 }
      const db = new TriviumDB(file, options)
      try {
        for (const node of contract.nodes) db.insertWithId(node.id, node.vector, node.payload)
        db.link(1, 2, 'related', 1)
        db.indexText(1, 'oldalpha')
        db.indexText(2, 'oldbeta')
        db.indexKeyword(1, 'oldanchor')
        db.publishGenerationManifest('maintenance-seed')
        const before = fs.readFileSync(file + '.wal')
        for (const test of contract.validation) {
          assert.throws(() => execute(db, test), check(test.code, test.details, contract.validation_write, test.reason, test.target))
          assert.deepEqual(fs.readFileSync(file + '.wal'), before)
          assert.deepEqual(facts(db), expected(false))
        }
        for (const api of ['index_text', 'index_keyword']) {
          assert.throws(() => execute(db, { api, id: 1, text: 'x'.repeat(100000), keyword: 'x'.repeat(100000) }), error => {
            assert.equal(error.code, 'TDB_CAPACITY_RESERVATION_REJECTED')
            assert.deepEqual(error.write, contract.validation_write)
            return true
          })
        }
        assert.throws(() => db.setSyncMode('unsupported'), check('TDB_INVALID_INPUT', undefined,
          contract.validation_write, true, { field: 'mode' }))
        for (const value of [-1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
          for (const [field, action] of [
            ['additional', () => db.reserveNodes(value)],
            ['mb', () => db.setMemoryLimit(value)],
            ['intervalSecs', () => db.enableAutoCompaction(value)],
          ]) {
            assert.throws(action, check('TDB_INVALID_INPUT', undefined, contract.validation_write, true, { field }))
          }
        }
        assert.throws(() => db.setMemoryLimit(Number.MAX_SAFE_INTEGER),
          check('TDB_INVALID_INPUT', undefined, contract.validation_write, true, { field: 'mb' }))
        for (const [field, action, values] of [
          ['additional', value => db.reserveNodes(value), ['1', true, null, {}, [], 1n, Symbol('invalid')]],
          ['mb', value => db.setMemoryLimit(value), ['1', true, null, {}, [], 1n, Symbol('invalid')]],
          ['intervalSecs', value => db.enableAutoCompaction(value), ['1', true, {}, [], 1n, Symbol('invalid')]],
          ['enabled', value => db.setAutoBuildQuiver(value), [0, 1, 'false', null, {}, []]],
          ['mode', value => db.setSyncMode(value), [0, true, null, {}, [], 1n, Symbol('invalid')]],
          ['generationId', value => db.publishGenerationManifest(value), [0, true, null, {}, [], 1n, Symbol('invalid')]],
        ]) {
          for (const value of values) {
            assert.throws(() => action(value), check('TDB_INVALID_INPUT', undefined,
              contract.validation_write, true, { field }))
          }
          let coercions = 0
          const value = { valueOf() { coercions++; throw new Error('禁止数值强制转换') },
            toString() { coercions++; throw new Error('禁止字符串强制转换') } }
          assert.throws(() => action(value), check('TDB_INVALID_INPUT', undefined,
            contract.validation_write, true, { field }))
          assert.equal(coercions, 0)
        }
        db.setAutoBuildQuiver(true)
        db.setAutoBuildQuiver(false)
        db.enableAutoCompaction()
        db.disableAutoCompaction()
        db.enableAutoCompaction(null)
        db.disableAutoCompaction()
        assert.deepEqual(fs.readFileSync(file + '.wal'), before)
        let managementCallbacks = 0
        const payload = { tenant: 't', category: 'same', get tag() {
          managementCallbacks++
          for (const action of [() => db.setSyncMode('full'), () => db.reserveNodes(1),
            () => db.setMemoryLimit(1), () => db.repairGraphIndexes(),
            () => db.setAutoBuildQuiver(false), () => db.enableAutoCompaction(7200),
            () => db.publishGenerationManifest('reentrant')]) {
            assert.throws(action, check('TDB_REENTRANT_ACCESS', { operation: 'node_callback' }, contract.admission))
          }
          return 'callback-confirmed'
        } }
        db.updatePayload(1, payload)
        assert.ok(managementCallbacks > 0, '必须执行实际转换回调')
        assert.deepEqual(db.getPayload(1), { tenant: 't', category: 'same', tag: 'callback-confirmed' })
        db.updatePayload(1, contract.nodes[0].payload)
        db.publishGenerationManifest('maintenance-after-callback')
        assert.deepEqual(facts(db), expected(false))
        db.close()
        for (const test of operations) {
          assert.throws(() => execute(db, test), check('TDB_DATABASE_CLOSED', {}, contract.admission))
        }
        assert.throws(() => db.setMemoryLimit(0), check('TDB_DATABASE_CLOSED', {}, contract.admission))
        assert.throws(() => db.publishGenerationManifest('closed'), check('TDB_DATABASE_CLOSED', {}, contract.admission))
      } finally { db.close() }
      for (const accessMode of ['readOnly', 'immutable']) {
        const reader = new TriviumDB(file, { ...options, accessMode })
        try {
          for (const test of operations) {
            assert.throws(() => execute(reader, test), check('TDB_READ_ONLY', { operation: test.api }, contract.admission))
          }
          reader.setMemoryLimit(1)
          assert.throws(() => reader.publishGenerationManifest('read-only'),
            check('TDB_READ_ONLY', { operation: 'publish_generation_manifest' }, contract.admission))
          assert.deepEqual(facts(reader), expected(false))
        } finally { reader.close() }
      }
      const writer = new TriviumDB(file, options)
      try {
        writer.rebuildTextIndex(contract.rebuilt_documents)
        writer.createUniqueIndex('tag')
        assert.deepEqual(facts(writer), expected(true, [{ kind: 'hash', fields: ['tag'], unique: true }]))
      } finally { writer.close() }
      const reopened = new TriviumDB(file, options)
      try {
        assert.deepEqual(facts(reopened), expected(true, [{ kind: 'hash', fields: ['tag'], unique: true }]))
      } finally { reopened.close() }
    }
  }
}

module.exports = { runIndexMaintenance }
