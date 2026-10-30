'use strict'

const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawnSync } = require('node:child_process')
const { Worker } = require('node:worker_threads')
const { ResourceRuntime, TriviumDB } = require('../../index.js')
const contract = require('../contracts/lifecycle_cases.json')
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tdb-资源 生命周期-'))
const held = []
const heldValues = []
let sequence = 0
const next = () => new Promise(resolve => setImmediate(resolve))

function create(options = {}) {
  const directory = path.join(root, `样本-${sequence++}`)
  fs.mkdirSync(directory)
  const file = path.join(directory, 'sample.tdb')
  const db = new TriviumDB(file, { dim: contract.dimension, autoBuildQuiver: false, ...options })
  db.insertWithId(contract.node_id, contract.vector, contract.original_payload)
  return { db, file, directory }
}

function released(directory) {
  if (process.platform === 'linux') {
    assert.equal(fs.readFileSync('/proc/self/maps', 'utf8').includes(fs.realpathSync(directory) + path.sep), false,
      '关闭后仍保留数据库文件映射')
  }
  fs.renameSync(directory, directory + '.moved')
  fs.renameSync(directory + '.moved', directory)
}

function setupHook(mode) {
  const { db } = create()
  let invoked = 0
  const hook = { onPostSearch(hits) { invoked++; return hits } }
  if (mode === 'cycle') hook.db = db
  const reference = new WeakRef(hook)
  if (mode === 'rejected') {
    db.close()
    assert.throws(() => db.setHook(hook), { code: 'TDB_DATABASE_CLOSED', details: {} })
  } else db.setHook(hook)
  if (mode === 'clear') db.clearHook()
  else if (mode === 'replace') db.setHook({})
  else if (mode === 'timeout') assert(db.closeTimeout(contract.close_timeout_ms))
  else if (mode === 'close') db.close()
  else if (mode === 'frozen') {
    Object.freeze(db)
    assert.throws(() => db.setHook({}))
    assert.equal(db.search(contract.vector, 1, 0, 0).length, 1)
    assert.equal(invoked, 1, '注册失败必须保留原 Hook')
    db.close()
  }
  if (mode !== 'drop' && mode !== 'cycle') held.push(db)
  return reference
}

async function registrationBudget() {
  const entered = path.join(root, '已进入回调')
  const release = path.join(root, '允许回调结束')
  const source = path.join(root, 'gated_hook.rs')
  const library = path.join(root, `gated_hook.${process.platform === 'win32' ? 'dll' : process.platform === 'darwin' ? 'dylib' : 'so'}`)
  fs.writeFileSync(source, `
    #[repr(C)]
    pub struct Hit { id: u64, score: f32 }
    #[no_mangle]
    pub extern "C" fn trivium_hook_abi_version() -> u32 { 2 }
    #[no_mangle]
    pub extern "C" fn trivium_hook_invoke_v2(stage: u32, _: *const f32, _: usize, hits: *mut Hit, capacity: usize, count: *mut usize) -> i32 {
      if stage == 1 {
        if std::fs::write(${JSON.stringify(entered)}, b"1").is_err() { return 91; }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !std::path::Path::new(${JSON.stringify(release)}).exists() {
          if std::time::Instant::now() >= deadline { return 92; }
          std::thread::sleep(std::time::Duration::from_millis(1));
        }
      }
      if stage == 2 {
        if capacity == 0 || hits.is_null() || count.is_null() { return 93; }
        // 宿主提供的可写缓冲区已通过容量与空指针检查，此处写入一个确定的候选。
        unsafe { *hits = Hit { id: ${contract.node_id}, score: 1.0 }; *count = 1; }
      }
      0
    }
  `)
  const compiled = spawnSync('rustc', ['--crate-type=cdylib', source, '-o', library], { encoding: 'utf8' })
  assert.equal(compiled.status, 0, compiled.stderr)
  const runtime = new ResourceRuntime({ maxThreads: 1, maxActiveTasks: 1 })
  const firstSample = create({ runtime })
  const first = firstSample.db
  const second = create({ runtime }).db
  let invoked = 0
  second.setHook({ onPostSearch(hits) { invoked++; return hits } })
  first.loadFfiHook(library)
  const pending = first.searchBatch([contract.vector], 1, 1, 0)
  pending.catch(() => {})
  try {
    const deadline = Date.now() + contract.close_timeout_ms
    while (!fs.existsSync(entered)) { assert(Date.now() < deadline, '原生回调未到达屏障'); await next() }
    const denied = error => {
      assert.equal(error.code, 'QueueFull')
      assert.equal(error.resource, 'active_tasks')
      assert.equal(error.requested, 1)
      assert.equal(error.used, 1)
      assert.equal(error.limit, 1)
      return true
    }
    // 用回调屏障保持准入名额占用，验证真实绑定错误及注册失败的原子性。
    for (const read of [() => second.get(contract.node_id), () => second.getPayload(contract.node_id),
      () => second.nodeCount(), () => second.contains(contract.node_id), () => second.allNodeIds(),
      () => second.getEdges(contract.node_id), () => second.getIncomingEdges(contract.node_id),
      () => second.getEdge(contract.node_id, 2, '关联'), () => second.neighbors(contract.node_id),
      () => second.graphStats(), () => second.validateGraph(), () => second.indexInfo(),
      () => second.listUniqueIndexes(), () => second.clearSearchState(), () => second.setHook({})]) {
      assert.throws(read, denied)
    }
    first.requestShutdown()
    assert.equal(first.shutdownStatus().phase, 'draining')
    assert.equal(first.closeTimeout(0), false)
    assert.throws(() => first.nodeCount(), { code: 'TDB_DATABASE_CLOSED', details: {} })
    assert.throws(() => new TriviumDB(firstSample.file, { dim: contract.dimension }), { code: 'TDB_DATABASE_LOCKED' })
  } finally {
    fs.writeFileSync(release, '1')
    const results = await pending
    heldValues.push(results, pending)
    assert.equal(results[0][0].id, contract.node_id)
    assert.equal(first.closeTimeout(contract.close_timeout_ms), true)
    held.push(first, second)
  }
  released(firstSample.directory)
  assert.equal(runtime.stats().attached_databases, 1)
  assert.equal(second.search(contract.vector, 1, 0, 0).length, 1)
  assert.equal(invoked, 1, '准入拒绝后原 Hook 必须仍可执行')
  second.close()
  fs.unlinkSync(library)
}

function retainedValuesAndPathReplacement() {
  for (const dtype of contract.dtypes) {
    for (const storageMode of ['mmap', 'rom']) {
      const runtime = new ResourceRuntime({ memoryLimitMb: 64, maintenanceMemoryLimitMb: 8, maxThreads: 1 })
      const { db, file, directory } = create({ dtype, storageMode, runtime, memoryLimitMb: 8 })
      db.insertWithId(contract.later_node_id, contract.vector, { tenant: 'agent-a', state: 'neighbor' })
      db.link(contract.node_id, contract.later_node_id, '原关系', 1)
      db.createIndex('tenant')
      db.indexText(contract.node_id, 'originaltoken')
      const query = 'FIND {tenant: "agent-a"} AS n WITH n RETURN n'
      const prepared = db.prepareTql(query)
      const node = db.get(contract.node_id)
      const rows = db.tql(query)
      const expectedRows = [
        { n: { id: String(contract.node_id), payload: contract.original_payload, numEdges: 1 } },
        { n: { id: String(contract.later_node_id), payload: { tenant: 'agent-a', state: 'neighbor' }, numEdges: 0 } },
      ]
      const sortedRows = value => [...value].sort((a, b) => Number(a.n.id) - Number(b.n.id))
      assert.deepEqual(sortedRows(rows), expectedRows)
      const edges = db.getEdges(contract.node_id)
      const operations = [{ type: 'insertWithId', id: 1001, vector: contract.vector, payload: { state: 'pending' } }]
      if (storageMode === 'rom') assert.equal(db.closeTimeout(contract.close_timeout_ms), true)
      else db.close()
      held.push(db)
      heldValues.push(node, rows, edges, prepared, operations)
      assert.equal(runtime.stats().attached_databases, 0)
      released(directory)
      const archived = directory + '.old'
      fs.renameSync(directory, archived)
      fs.mkdirSync(directory)
      const writer = new TriviumDB(file, { dim: contract.dimension, dtype, runtime, memoryLimitMb: 8,
        storageMode: storageMode === 'rom' ? 'mmap' : 'rom', autoBuildQuiver: false })
      held.push(writer)
      writer.insertWithId(contract.node_id, contract.vector, { tenant: 'agent-a', state: 'replacement' })
      writer.flush()
      db.close()
      db.dispose()
      assert.equal(db.closeTimeout(0), true)
      assert.throws(() => db.commitTransaction(operations), { code: 'TDB_DATABASE_CLOSED', details: {} })
      assert.throws(() => new TriviumDB(file, { dim: contract.dimension, dtype }), { code: 'TDB_DATABASE_LOCKED' })
      assert.equal(runtime.stats().attached_databases, 1)
      assert.equal(runtime.stats().reserved_database_bytes, 8 * 1024 * 1024)
      assert.equal(writer.nodeCount(), 1)
      assert.equal(writer.contains(1001), false)
      assert.deepEqual(node.payload, contract.original_payload)
      assert.deepEqual(sortedRows(rows), expectedRows)
      assert.equal(edges[0].targetId, contract.later_node_id)
      assert.deepEqual(writer.executePreparedTql(prepared, {}), [
        { n: { id: String(contract.node_id), payload: { tenant: 'agent-a', state: 'replacement' }, numEdges: 0 } },
      ])
      fs.rmSync(archived, { recursive: true })
      writer.close()
      assert.equal(runtime.stats().attached_databases, 0)
      released(directory)
    }
  }
}

function multipleReadersAndFailedClose() {
  for (const dtype of contract.dtypes) {
    for (const accessMode of ['readOnly', 'immutable']) {
      const { db, file, directory } = create({ dtype })
      db.insertWithId(contract.later_node_id, contract.vector, { tenant: 'agent-a', state: 'neighbor' })
      db.link(contract.node_id, contract.later_node_id, '共享图边', 1)
      db.createIndex('tenant')
      db.indexText(contract.node_id, 'originaltoken')
      db.publishGenerationManifest('持有读者制品')
      db.close()
      const options = { dim: contract.dimension, dtype, accessMode, loadTextIndex: true, autoBuildQuiver: false }
      const first = new TriviumDB(file, options)
      const second = new TriviumDB(file, options)
      held.push(db, first, second)
      const saved = second.get(contract.node_id)
      heldValues.push(saved)
      first.close()
      assert.deepEqual(second.getPayload(contract.node_id), contract.original_payload)
      assert.equal(second.getEdges(contract.node_id)[0].targetId, contract.later_node_id)
      assert.deepEqual(second.tql('FIND {tenant: "agent-a"} AS n WITH n RETURN n')
        .sort((a, b) => Number(a.n.id) - Number(b.n.id)), [
        { n: { id: String(contract.node_id), payload: contract.original_payload, numEdges: 1 } },
        { n: { id: String(contract.later_node_id), payload: { tenant: 'agent-a', state: 'neighbor' }, numEdges: 0 } },
      ])
      if (accessMode === 'readOnly') assert.throws(() => new TriviumDB(file, { dim: contract.dimension, dtype }), { code: 'TDB_DATABASE_LOCKED' })
      assert.equal(second.closeTimeout(contract.close_timeout_ms), true)
      released(directory)
      const writer = new TriviumDB(file, { dim: contract.dimension, dtype, autoBuildQuiver: false })
      held.push(writer)
      first.close(); second.close()
      assert.throws(() => new TriviumDB(file, { dim: contract.dimension, dtype }), { code: 'TDB_DATABASE_LOCKED' })
      writer.updatePayload(contract.node_id, { state: 'updated' })
      writer.close()
      assert.deepEqual(saved.payload, contract.original_payload)
      released(directory)
    }
    const { db: original, file, directory } = create({ dtype })
    original.createIndex('tenant')
    original.indexText(contract.node_id, 'originaltoken')
    original.indexKeyword(contract.node_id, 'originalalias')
    original.close()
    const db = new TriviumDB(file, { dim: contract.dimension, dtype, autoBuildQuiver: false })
    held.push(original, db)
    db.insertWithId(contract.later_node_id, contract.vector, contract.later_payload)
    const obstacle = file + '.text.meta.tmp'
    fs.mkdirSync(obstacle)
    assert.throws(() => db.flush())
    assert.throws(() => db.close())
    assert.notEqual(db.shutdownStatus().phase, 'closed')
    assert.throws(() => new TriviumDB(file, { dim: contract.dimension, dtype }), { code: 'TDB_DATABASE_LOCKED' })
    fs.rmdirSync(obstacle)
    assert.equal(db.closeTimeout(contract.close_timeout_ms), true)
    released(directory)
    const reader = new TriviumDB(file, { dim: contract.dimension, dtype, accessMode: 'readOnly', loadTextIndex: true })
    held.push(reader)
    assert.equal(reader.nodeCount(), 2)
    assert.deepEqual(reader.getPayload(contract.later_node_id), contract.later_payload)
    assert.deepEqual(reader.tql('TEXT AC "originalalias" TOP 10 AS hits WITH hits RETURN hits')
      .map(row => row.hits.id), [String(contract.node_id)])
    assert.deepEqual(reader.tql('TEXT BM25 "originaltoken" TOP 10 AS hits WITH hits RETURN hits')
      .map(row => row.hits.id), [String(contract.node_id)])
    reader.close()
    released(directory)
  }
}

async function main() {
  assert.equal(typeof global.gc, 'function', '回归必须通过 --expose-gc 执行')
  retainedValuesAndPathReplacement()
  multipleReadersAndFailedClose()
  for (const dtype of contract.dtypes) {
    for (const storageMode of ['mmap', 'rom']) {
      for (const close of ['close', 'dispose', 'timeout', 'shutdown', ...(typeof Symbol.dispose === 'symbol' ? ['symbol'] : [])]) {
        const { db, file, directory } = create({ dtype, storageMode })
        if (close === 'timeout') assert(db.closeTimeout(contract.close_timeout_ms))
        else if (close === 'shutdown') {
          db.requestShutdown()
          const deadline = Date.now() + contract.close_timeout_ms
          while (db.shutdownStatus().phase !== 'closed') { assert(Date.now() < deadline, '关闭未在期限内完成'); await next() }
        } else if (close === 'symbol') db[Symbol.dispose]()
        else db[close]()
        held.push(db)
        for (const read of [() => db.get(contract.node_id), () => db.getPayload(contract.node_id), () => db.contains(contract.node_id),
          () => db.nodeCount(), () => db.allNodeIds(), () => db.getEdges(contract.node_id), () => db.getIncomingEdges(contract.node_id),
          () => db.neighbors(contract.node_id), () => db.getEdge(contract.node_id, 2, "关联"),
          () => db.graphStats(), () => db.validateGraph(), () => db.listUniqueIndexes(), () => db.indexInfo(), () => db.clearSearchState()]) {
          assert.throws(read, { code: 'TDB_DATABASE_CLOSED', details: {} })
        }
        assert.equal(db.shutdownStatus().phase, 'closed')
        released(directory)
        const reader = new TriviumDB(file, { dim: contract.dimension, dtype, accessMode: 'readOnly', autoBuildQuiver: false })
        assert.deepEqual(reader.getPayload(contract.node_id), contract.original_payload)
        reader.close()
        held.push(reader)
        released(directory)
      }
    }
  }

  for (const asynchronous of [false, true]) {
    const { db, file, directory } = create()
    db.publishGenerationManifest('关闭制品')
    const before = new Map(fs.readdirSync(directory).filter(name => !name.endsWith('.lock')).map(name => [name, fs.readFileSync(path.join(directory, name))]))
    if (asynchronous) assert(db.closeTimeout(contract.close_timeout_ms))
    else db.close()
    for (const [name, bytes] of before) assert.deepEqual(fs.readFileSync(path.join(directory, name)), bytes)
    const reader = new TriviumDB(file, { dim: contract.dimension, accessMode: 'immutable', autoBuildQuiver: false })
    assert.equal(reader.nodeCount(), 1)
    reader.close()
    held.push(db, reader)
    released(directory)
  }

  const refs = ['clear', 'replace', 'close', 'timeout', 'drop', 'cycle', 'rejected', 'frozen'].map(mode => setupHook(mode))
  // 跨事件循环再触发 GC，避免 WeakRef 在创建它的当前任务中保活目标。
  for (let count = 0; count < 8; count++) { await next(); global.gc() }
  await next()
  for (const reference of refs) assert.equal(reference.deref(), undefined, '清除、替换、关闭与析构必须解除 Hook 持有关系')

  const library = path.join(root, `hook.${process.platform === 'win32' ? 'dll' : process.platform === 'darwin' ? 'dylib' : 'so'}`)
  const source = path.join(root, 'hook.rs')
  fs.writeFileSync(source, 'pub fn marker() -> u32 { 1 }')
  const compiled = spawnSync('rustc', ['--crate-type=cdylib', source, '-o', library], { encoding: 'utf8' })
  assert.equal(compiled.status, 0, compiled.stderr)
  for (const asynchronous of [false, true]) {
    const copy = library + (asynchronous ? '.async' : '.sync')
    fs.copyFileSync(library, copy)
    const { db } = create()
    db.loadFfiHook(copy)
    if (asynchronous) assert(db.closeTimeout(contract.close_timeout_ms))
    else db.close()
    held.push(db)
    fs.unlinkSync(copy)
  }

  await registrationBudget()

  for (let count = 0; count < 3; count++) {
    const directory = path.join(root, `工作线程-${count}`)
    fs.mkdirSync(directory)
    const file = path.join(directory, 'sample.tdb')
    const worker = new Worker(`
      const { parentPort, workerData } = require('node:worker_threads')
      const { TriviumDB } = require(workerData.module)
      const db = new TriviumDB(workerData.file, { dim: 3, autoBuildQuiver: false })
      db.insertWithId(1, [1, 2, 3], {})
      db.setHook({ onPostSearch(hits) { return hits } })
      db.requestShutdown()
      parentPort.postMessage('已登记关闭')
    `, { eval: true, workerData: { module: path.resolve(__dirname, '../../index.js'), file } })
    await new Promise((resolve, reject) => { worker.once('message', resolve); worker.once('error', reject) })
    await worker.terminate()
    let reopened
    const deadline = Date.now() + contract.close_timeout_ms
    while (!reopened) {
      try { reopened = new TriviumDB(file, { dim: 3, autoBuildQuiver: false }) }
      catch (error) {
        assert.equal(error.code, 'TDB_DATABASE_LOCKED',
          `工作线程重开返回非锁错误 (Worker reopen returned a non-lock error): ${error.message}; details=${JSON.stringify(error.details)}`)
        assert(Date.now() < deadline, '工作线程退出后关闭任务未排空')
        await next()
      }
    }
    assert.equal(reopened.nodeCount(), 1)
    assert.deepEqual(reopened.get(1), { id: 1, vector: [1, 2, 3], payload: {}, edges: [], numEdges: 0 })
    reopened.close()
    held.push(reopened)
    released(directory)
  }
}

main().then(() => {
  for (const db of held) db.close()
  fs.rmSync(root, { recursive: true, force: true })
  console.log('Node 资源释放、循环引用与工作线程退出回归通过')
}).catch(error => { console.error('Node 资源生命周期回归失败', error); process.exitCode = 1 })
