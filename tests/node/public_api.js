'use strict'

// Node 公共 API 跨层契约测试：覆盖 Prepared TQL、索引/存储诊断、
// u64 字符串映射、Payload Filter 和已移除入口的稳定迁移错误。

const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const contract = require('../contracts/public_cases.json')
const { runErrorContract } = require('./error_contracts')
const { runConsoleContract } = require('./console_contracts')
const { runKernelBoundaries } = require('./kernel_boundaries')
const { TriviumDB, ResourceRuntime, verifySnapshot, restoreSnapshot } = require('../../index.js')

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'triviumdb-node-public-api-'))
const file = path.join(root, 'api.tdb')

function runTransactionContract(root) {
  const contract = require('../contracts/transaction_cases.json')
  for (const dtype of contract.dtypes) {
    for (const storageMode of contract.storage_modes) {
      for (const test of contract.cases) {
        const file = path.join(root, `transaction-${dtype}-${storageMode}-${test.name}.tdb`)
        const options = { dim: contract.dimension, dtype, storageMode, autoBuildQuiver: false }
        const db = new TriviumDB(file, options)
        try {
          for (const node of contract.setup) db.insertWithId(node.id, node.vector, node.payload)
          const beforeWal = fs.readFileSync(file + '.wal')
          if (test.expected.error) {
            assert.throws(() => db.commitTransaction(test.operations), Error)
            assert.deepEqual(fs.readFileSync(file + '.wal'), beforeWal)
          } else {
            assert.deepEqual(db.commitTransaction(test.operations), test.expected.ids)
          }
          const check = database => {
            assert.deepEqual(database.allNodeIds().sort((a, b) => a - b), test.expected.nodes.map(node => node.id))
            for (const expected of test.expected.nodes) {
              const actual = database.get(expected.id)
              assert.ok(actual)
              assert.equal(actual.id, expected.id)
              assert.deepEqual(actual.payload, expected.payload)
              assert.deepEqual(actual.vector, expected.vector)
            }
          }
          check(db)
          const replayFile = file + '.replay'
          fs.copyFileSync(file, replayFile)
          fs.copyFileSync(file + '.wal', replayFile + '.wal')
          const replay = new TriviumDB(replayFile, options)
          try { check(replay) } finally { replay.close() }
          db.close()
          const reopened = new TriviumDB(file, options)
          try { check(reopened) } finally { reopened.close() }
        } finally {
          db.close()
        }
      }
    }
  }
}

function runTextRecoveryContract(root) {
  const source = path.join(__dirname, '..', 'fixtures', 'v0810_text_mmap')
  for (const missing of [false, true]) {
    const target = path.join(root, `text-upgrade-${missing}.tdb`)
    for (const suffix of ['', '.vec', '.flush_ok', '.pld.2', '.text', '.text.meta', '.wal']) {
      if (missing && suffix === '.text.meta') continue
      fs.copyFileSync(path.join(source, `sample.tdb${suffix}`), target + suffix)
    }
    const db = new TriviumDB(target, { dim: 4, autoBuildQuiver: false })
    db.insertWithId(5, [0, 0, 1, 0], {})
    db.flush()
    db.buildTextIndex()
    assert.deepEqual(db.tql('TEXT BM25 "alpha" TOP 10 AS hits WITH hits RETURN hits')
      .map(row => row.hits.id).sort(), ['1', '2', '3', '4'])
    db.close()
    fs.writeFileSync(target + '.text', 'corrupt')
    const repair = new TriviumDB(target, { dim: 4, loadTextIndex: true, autoBuildQuiver: false })
    assert.equal(fs.readFileSync(target + '.text', 'utf8'), 'corrupt')
    assert.throws(() => repair.buildTextIndex())
    repair.rebuildTextIndex([{ id: 1, text: 'restoredtoken', keywords: ['restoredanchor'] }])
    repair.close()
    const reader = new TriviumDB(target, { dim: 4, loadTextIndex: true, accessMode: 'readOnly' })
    assert.equal(reader.nodeCount(), 5)
    assert.deepEqual(reader.tql('TEXT BM25 "restoredtoken" TOP 10 AS hits WITH hits RETURN hits')
      .map(row => row.hits.id), ['1'])
    assert.deepEqual(reader.tql('TEXT AC "restoredanchor" TOP 10 AS hits WITH hits RETURN hits')
      .map(row => row.hits.id), ['1'])
    reader.close()
  }
}

function runLifecycleContract(root) {
  const test = require('../contracts/lifecycle_cases.json')
  for (const dtype of test.dtypes) {
    const runtime = new ResourceRuntime({ memoryLimitMb: test.runtime.memory_limit_mb,
      maintenanceMemoryLimitMb: test.runtime.maintenance_memory_limit_mb,
      maxThreads: test.runtime.max_threads, maxActiveTasks: test.runtime.max_active_tasks })
    const db = new TriviumDB(path.join(root, `lifecycle-${dtype}.tdb`), { dim: test.dimension,
      dtype, memoryLimitMb: test.database_memory_limit_mb, autoBuildQuiver: false, runtime })
    db.insertWithId(test.node_id, test.vector, test.original_payload)
    const snapshot = path.join(root, `snapshot-${dtype}`)
    const restored = path.join(root, `restored-${dtype}`)
    const manifest = db.createSnapshot(snapshot, test.generation_id)
    assert.equal(manifest.node_count, 1)
    assert.equal(manifest.complete, true)
    db.updatePayload(test.node_id, test.later_payload)
    db.insertWithId(test.later_node_id, test.vector, {})
    assert.throws(() => db.createSnapshot(snapshot, test.generation_id), error => error.code === 'TDB_SNAPSHOT_EXISTS')
    assert.equal(verifySnapshot(snapshot, runtime).node_count, 1)
    assert.equal(restoreSnapshot(snapshot, restored, runtime).generation_id, test.generation_id)
    assert.throws(() => restoreSnapshot(snapshot, restored, runtime), error => error.code === 'TDB_SNAPSHOT_EXISTS')
    const reader = new TriviumDB(path.join(restored, test.database_file), { dim: test.dimension,
      dtype, accessMode: 'immutable', memoryLimitMb: test.database_memory_limit_mb, runtime })
    assert.deepEqual(reader.getPayload(test.node_id), test.original_payload)
    assert.equal(reader.contains(test.later_node_id), false)
    assert.equal(reader.closeTimeout(test.close_timeout_ms), true)
    assert.equal(verifySnapshot(restored, runtime).node_count, 1)
    for (const milliseconds of [-1, 0.5, NaN, Infinity]) {
      assert.throws(() => db.closeTimeout(milliseconds))
    }
    db.requestShutdown()
    assert.equal(db.closeTimeout(test.close_timeout_ms), true)
    assert.equal(db.shutdownStatus().phase, 'closed')
    assert.equal(db.closeTimeout(0), true)
    const stats = runtime.stats()
    assert.equal(stats.attached_databases, 0)
    assert.equal(stats.active_maintenance_tasks, 0)
    assert.equal(stats.reserved_maintenance_bytes, 0)
    assert.ok(stats.peak_reserved_bytes <= stats.memory_limit)
  }
}

function runResourceContract(root) {
  const test = require('../contracts/resource_cases.json')
  for (const dtype of test.dtypes) {
    const runtime = new ResourceRuntime({
      memoryLimitMb: test.runtime.memory_limit_mb,
      maintenanceMemoryLimitMb: test.runtime.maintenance_memory_limit_mb,
      maxThreads: test.runtime.max_threads,
      maxActiveTasks: test.runtime.max_active_tasks,
    })
    const check = phase => {
      const stats = runtime.stats()
      for (const [key, value] of Object.entries(test.expected[phase])) {
        assert.equal(stats[key], value, `${dtype}/${phase}/${key}`)
      }
      assert.equal(stats.active_tasks, 0)
      assert.equal(stats.reserved_workspace_bytes, 0)
      assert.ok(stats.peak_worker_threads <= test.runtime.max_threads)
    }
    const open = (name, memoryLimitMb) => new TriviumDB(
      path.join(root, `resource-${dtype}-${name}.tdb`),
      { dim: test.database.dim, dtype, memoryLimitMb, autoBuildQuiver: false, runtime },
    )
    const first = open('first', test.database.memory_limit_mb)
    const second = open('second', test.database.memory_limit_mb)
    const checkInstance = (db, phase) => {
      const stats = db.instanceStats()
      const expected = test.instances[phase]
      assert.equal(stats.memory_limit_bytes, expected.memory_limit_bytes)
      assert.equal(stats.memory.reserved_database_bytes, expected.reserved_database_bytes)
      assert.equal(stats.memory.node_count, expected.node_count)
      assert.equal(stats.shutdown.phase, expected.phase)
      assert.equal(stats.shutdown.active_operations, 0)
      assert.equal(stats.shutdown.last_failure, null)
      assert.ok(path.isAbsolute(stats.database_path))
    }
    const checkBudget = phase => error => {
      assert.equal(error.code, 'QueueFull')
      for (const [key, value] of Object.entries(test.budget_errors[phase])) {
        assert.equal(error[key], value, `${phase}/${key}`)
      }
      return true
    }
    check('opened')
    checkInstance(first, 'opened')
    checkInstance(second, 'opened')
    assert.throws(() => open('rejected', test.overbook_mb), checkBudget('open'))
    assert.ok(!fs.existsSync(path.join(root, `resource-${dtype}-rejected.tdb`)))
    first.setMemoryLimit(test.resize_mb)
    check('resized')
    checkInstance(first, 'resized')
    assert.throws(() => second.setMemoryLimit(test.failed_resize_mb), checkBudget('resize'))
    check('resized')
    checkInstance(second, 'opened')
    first.setMemoryLimit(test.database.memory_limit_mb)
    first.insertWithId(101, test.vector, { owner: 'first' })
    second.insertWithId(202, test.vector, { owner: 'second' })
    assert.equal(Number(first.searchExact(test.vector, 1)[0].id), 101)
    assert.equal(Number(second.searchExact(test.vector, 1)[0].id), 202)
    first.enableAutoCompaction(3600)
    assert.equal(first.instanceStats().compaction.phase, 'scheduled')
    first.disableAutoCompaction()
    assert.equal(first.instanceStats().compaction, null)
    first.close()
    checkInstance(first, 'closed')
    check('closed_one')
    const replacement = open('replacement', test.replacement_mb)
    check('replaced')
    second.close()
    replacement.close()
    check('closed_all')
  }
  const unlimited = new ResourceRuntime().stats()
  for (const key of ['available_foreground_bytes', 'available_maintenance_bytes', 'available_task_slots']) {
    assert.equal(unlimited[key], null)
  }
  for (const maxThreads of [-1, 0.5, 65, NaN, Infinity]) {
    assert.throws(() => new ResourceRuntime({ maxThreads }))
  }
  assert.throws(() => new TriviumDB(path.join(root, 'bad-runtime.tdb'), { runtime: {} }))
  const wrong = new TriviumDB(path.join(root, 'wrong-class.tdb'), { dim: 3 })
  assert.throws(() => new TriviumDB(path.join(root, 'bad-class.tdb'), { runtime: wrong }))
  const forged = Object.create(ResourceRuntime.prototype)
  assert.throws(() => new TriviumDB(path.join(root, 'bad-prototype.tdb'), { runtime: forged }))
  assert.throws(() => new TriviumDB(path.join(root, 'bad-handle.tdb'), {
    runtime: { __tdbResourceRuntime: {} },
  }))
  wrong.close()
}

function runSharedContract(root) {
  assert.equal(contract.schema_version, 1)
  const db = new TriviumDB(path.join(root, 'shared-contract.tdb'), {
    dim: contract.setup.dimension,
  })
  for (const node of contract.setup.nodes) {
    db.insertWithId(node.id, node.vector, node.payload)
  }
  for (const edge of contract.setup.edges) {
    db.link(edge.source, edge.target, edge.label, edge.weight)
  }
  for (const testCase of contract.cases) {
    const expected = testCase.expected
    if (testCase.operation === 'get_payload') {
      const payload = db.getPayload(testCase.node_id)
      assert.equal(payload[expected.field], expected.value, testCase.name)
    } else if (testCase.operation === 'prepared_tql') {
      const prepared = db.prepareTql(testCase.tql)
      const rows = db.executePreparedTql(prepared, testCase.parameters || {})
      assert.equal(rows.length, expected.row_count, testCase.name)
      assert.equal(rows[0][expected.column], expected.value, testCase.name)
    } else if (testCase.operation === 'tql_path') {
      const rows = db.tql(testCase.tql)
      assert.deepEqual(rows[0].path, expected.path.map(String), testCase.name)
    } else if (testCase.operation === 'prepared_missing_parameter') {
      const prepared = db.prepareTql(testCase.tql)
      assert.throws(
        () => db.executePreparedTql(prepared, testCase.parameters || {}),
        error => {
          assert.equal(error.code, expected.error_code)
          assert.deepEqual(error.details, expected.error_details)
          return true
        },
        testCase.name,
      )
    } else if (testCase.operation === 'text_lifecycle') {
      const parameters = testCase.parameters
      db.indexText(1, parameters.old_text)
      db.indexKeyword(1, parameters.keyword)
      db.indexKeyword(1, parameters.keyword)
      db.indexKeyword(2, parameters.keyword)
      db.buildTextIndex()
      db.indexText(1, parameters.new_text)
      for (const phase of ['before_delete', 'after_delete']) {
        if (phase === 'after_delete') db.delete(1)
        for (const check of expected[phase]) {
          assert.equal(db.tql(check.query).length, check.count, testCase.name)
        }
      }
    } else if (testCase.operation === 'text_rebuild') {
      for (const id of [-1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
        assert.throws(() => db.rebuildTextIndex([{ id, text: 'invalid' }]))
      }
      db.rebuildTextIndex(testCase.documents)
      for (const check of expected.checks) {
        assert.equal(db.tql(check.query).length, check.count, testCase.name)
      }
    } else {
      throw new Error(`未知共享契约操作: ${testCase.operation}`)
    }
  }
  db.close()
  assert.throws(() => db.buildTextIndex(), /closed|关闭/i)
  assert.throws(() => db.rebuildTextIndex([]), /closed|关闭/i)
  const reader = new TriviumDB(path.join(root, 'shared-contract.tdb'), {
    dim: contract.setup.dimension, loadTextIndex: true, accessMode: 'readOnly',
  })
  for (const check of contract.cases.find(item => item.operation === 'text_rebuild').expected.checks) {
    assert.equal(reader.tql(check.query).length, check.count)
  }
  assert.throws(() => reader.rebuildTextIndex([]), /read.only|只读/i)
  reader.close()
}

async function runAsyncResourceContract(root) {
  const test = require('../contracts/resource_cases.json')
  const mb = 1024 * 1024
  for (const dtype of test.dtypes) {
    const runtime = new ResourceRuntime({ memoryLimitMb: 40, maintenanceMemoryLimitMb: 8,
      maxThreads: 2, maxActiveTasks: 1 })
    const db = new TriviumDB(path.join(root, `async-budget-${dtype}.tdb`), {
      dim: 3, dtype, memoryLimitMb: 8, autoBuildQuiver: false, runtime,
    })
    try {
      db.insertWithId(1, test.vector, {})
      db.setMemoryLimit(32)
      const checkError = error => {
        assert.equal(error.code, 'QueueFull')
        assert.equal(error.resource, 'memory_bytes')
        assert.equal(error.used, 32 * mb)
        assert.equal(error.limit, 32 * mb)
        assert.ok(error.requested > 0)
        return true
      }
      assert.throws(() => db.searchExact(test.vector, 1), checkError)
      await assert.rejects(() => db.searchBatch([test.vector], 1, 1, 0), checkError)
      assert.equal(runtime.stats().active_tasks, 0)
      db.setMemoryLimit(8)
      assert.equal((await db.searchBatch([test.vector], 1, 1, 0))[0][0].id, 1)
    } finally {
      db.close()
    }
  }
}

function runSearchBoundaries(root) {
  for (const storageMode of ['rom', 'mmap']) {
    const file = path.join(root, `search-order-${storageMode}.tdb`)
    const options = { dim: 2, storageMode, autoBuildQuiver: false }
    const db = new TriviumDB(file, options)
    for (const id of [1, 2, 3]) db.insertWithId(id, [1, 0], { value: id })
    db.delete(1)
    db.insertWithId(10, [1, 0], { value: 10 })
    const check = database => assert.deepEqual(database.searchAdvanced([1, 0], {
      topK: 1, recallK: 1, rerankK: 1, expandDepth: 0, minScore: 0, forceBruteForce: true,
    }).map(hit => hit.id), [2])
    check(db)
    db.updatePayload(2, { value: 2 ** 53 + 4 })
    db.updatePayload(3, { value: 2 ** 53 })
    db.updatePayload(10, { value: 2 ** 53 + 2 })
    db.createOrderedIndex('value')
    const query = 'FIND {value: {$exists: true}} RETURN * ORDER BY _.value ASC'
    assert.deepEqual(db.tql(query).map(row => row._.id), ['3', '10', '2'])
    db.close()
    const reopened = new TriviumDB(file, options)
    try {
      check(reopened)
      assert.deepEqual(reopened.tql(query).map(row => row._.id), ['3', '10', '2'])
    } finally { reopened.close() }
  }
}

async function main() {
  try {
    runSearchBoundaries(root)
    runLifecycleContract(root)
    runResourceContract(root)
    runSharedContract(root)
    await runErrorContract(root)
    runConsoleContract(root)
    runKernelBoundaries(root)
    runTransactionContract(root)
    await require('./write_boundaries.js')(root, TriviumDB)
    await require('./numeric_boundaries.js')(root, TriviumDB)
    runTextRecoveryContract(root)
    await runAsyncResourceContract(root)
    assert.throws(
      () => new TriviumDB(file, 2),
      /object|对象|options/i,
      '旧位置参数构造器必须明确拒绝',
    )

    const db = new TriviumDB(file, { dim: 2 })
    db.insertWithId(1, [1, 0], { kind: 'a', score: 1, tenant: 't', region: 'r' })
    db.insertWithId(2, [0.9, 0.1], { kind: 'b', score: 2, tenant: 't', region: 'r' })
    db.link(1, 2, 'next', 1)

    const stages = []
    db.setHook({
      onPreSearch(query) {
        stages.push('pre')
        return query
      },
      onCustomRecall() {
        stages.push('custom')
        return null
      },
      onPostRecall(hits) {
        stages.push('postRecall')
        return hits
      },
      onPreGraphExpand(hits) {
        stages.push('preGraph')
        return hits
      },
      onRerank(hits) {
        stages.push('rerank')
        return hits
      },
      onPostSearch(hits) {
        stages.push('post')
        return hits.slice(0, 1)
      },
    })
    const hooked = db.searchWithContext([1, 0], {
      topK: 2,
      expandDepth: 1,
      minScore: 0,
    })
    assert.equal(hooked.hits.length, 1)
    assert.deepEqual(new Set(stages), new Set([
      'pre', 'custom', 'postRecall', 'preGraph', 'rerank', 'post',
    ]))
    db.clearHook()

    db.setHook({
      onPreSearch() {
        throw new Error('node-hook-failure')
      },
    })
    assert.throws(
      () => db.searchWithContext([1, 0], { topK: 2 }),
      /TDB_HOOK_EXECUTION|node-hook-failure/,
    )
    db.clearHook()

    db.createIndex('kind')
    db.createOrderedIndex('score')
    db.createCompositeIndex(['tenant', 'region'])
    db.createBitmapIndex('region')
    const indexes = db.indexInfo()
    assert.equal(indexes.length, 4)
    assert.deepEqual(indexes.find(item => item.kind === 'composite').fields, ['tenant', 'region'])

    const storage = db.storageInfo()
    assert.equal(storage.database_format_current, 10)
    assert.equal(storage.property_index_format, 8)
    assert.equal(storage.node_count, 2)

    const vectorPrepared = db.prepareTql('SEARCH VECTOR [$x, $y] TOP 1 RETURN *')
    assert.deepEqual(vectorPrepared.parameterNames(), ['x', 'y'])
    const vectorRows = db.executePreparedTql(vectorPrepared, { x: 1, y: 0 })
    assert.equal(vectorRows[0]._.id, '1')

    const prepared = db.prepareTql('FIND {kind: "a"} AS seed WITH seed RETURN seed, $bonus + 1 AS score')
    assert.deepEqual(prepared.parameterNames(), ['bonus'])
    const preparedRows = db.executePreparedTql(prepared, { bonus: 4 })
    assert.equal(preparedRows.length, 1)
    assert.equal(preparedRows[0].score, 5)
    assert.throws(() => db.executePreparedTql(prepared, {}), error => {
      assert.equal(error.code, 'TDB_MISSING_QUERY_PARAMETER')
      assert.deepEqual(error.details, { parameter: 'bonus' })
      return true
    })

    const pathRows = db.tql('SEARCH VECTOR [1, 0] TOP 1 AS seed WITH seed shortest_paths seed TO [2] LABEL next AS route WITH route RETURN path(route) AS path')
    assert.deepEqual(pathRows[0].path, ['1', '2'])

    const advancedFiltered = db.searchAdvanced([1, 0], {
      topK: 2,
      minScore: 0,
      payloadFilter: { kind: 'b' },
    })
    assert.equal(advancedFiltered.length, 1)
    assert.equal(advancedFiltered[0].payload.kind, 'b')

    const contextFiltered = db.searchWithContext([1, 0], {
      topK: 2,
      minScore: 0,
      payloadFilter: { kind: 'b' },
    })
    assert.equal(contextFiltered.hits.length, 1)
    assert.equal(contextFiltered.hits[0].payload.kind, 'b')

    const basicFiltered = db.search([1, 0], 2, 0, 0, { kind: 'b' })
    assert.equal(basicFiltered.length, 1)
    assert.equal(basicFiltered[0].payload.kind, 'b')

    assert.throws(
      () => db.searchAdvanced([1, 0], { payloadFilter: { score: { $gt: true } } }),
      /payloadFilter|过滤器|Filter/i,
    )

    assert.throws(
      () => db.tqlMut('FIND {kind: "a"} RETURN *'),
      error => error.code === 'TDB_API_MIGRATION_REQUIRED',
    )
    db.close()
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
}

main().catch(error => {
  console.error('公共接口契约验证失败：', error)
  process.exitCode = 1
})
