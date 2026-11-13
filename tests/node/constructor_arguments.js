'use strict'

// 构造参数使用独立预期，验证原始 getter、配额归还及打开失败后的实际恢复。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const fixture = require('../contracts/constructor_arguments.json')

function invalid(field, write) {
  return error => {
    assert.equal(error.schema_version, 1)
    assert.equal(error.code, 'TDB_INVALID_INPUT')
    assert.deepEqual(error.target, { field })
    assert.equal(error.stage, undefined)
    assert.deepEqual(error.write, write)
    assert.deepEqual(Object.keys(error.details), ['reason'])
    assert.ok(error.details.reason)
    return true
  }
}

function runConstructorArguments(root, { TriviumDB, ResourceRuntime, ConsoleSession, verifySnapshot, restoreSnapshot }) {
  const parent = path.join(root, 'constructor-rejected')
  const file = path.join(parent, 'database.tdb')
  assert.throws(() => new TriviumDB(), error => {
    assert.ok(error instanceof TypeError)
    assert.equal(error.code, 'TDB_INVALID_INPUT')
    assert.deepEqual(error.target, { field: 'path' })
    assert.equal(error.details.argument_kind, 'missing')
    assert.equal(error.write, undefined)
    return true
  })
  for (const field of ['dtype', 'syncMode', 'storageMode', 'accessMode', 'missingIndexPolicy', 'rowOverflow']) {
    assert.throws(() => new TriviumDB(file, { [field]: '\ud800' }), invalid(field))
  }
  assert.throws(() => new TriviumDB('\ud800'), invalid('path'))
  assert.throws(() => new ConsoleSession('\ud800'), invalid('optionsJson'))
  class DerivedDatabase extends TriviumDB {}
  const derived = new DerivedDatabase(path.join(root, 'constructor-derived.tdb'), { dim: 2 }, 'extra')
  try {
    assert.ok(derived instanceof DerivedDatabase && derived instanceof TriviumDB)
    assert.equal(derived.constructor, DerivedDatabase)
    assert.equal(Object.getPrototypeOf(DerivedDatabase.prototype), TriviumDB.prototype)
    derived.insertWithId(fixture.saved.id, fixture.saved.vector, fixture.saved.payload)
    assert.deepEqual(derived.get(7), { ...fixture.saved, numEdges: 0, edges: [] })
  } finally { derived.close() }
  assert.equal(TriviumDB.prototype.constructor, TriviumDB)
  for (const entry of fixture.integers) {
    for (const value of [null, '1', true, {}, [], -1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
      assert.throws(() => new TriviumDB(file, { [entry.node]: value }), invalid(entry.node))
    }
  }
  for (const entry of [...fixture.strings, { node: 'storageMode' }]) {
    for (const value of [null, 1, true, {}, [], 'invalid']) {
      assert.throws(() => new TriviumDB(file, { [entry.node]: value }), invalid(entry.node))
    }
  }
  for (const { node: field } of fixture.booleans) {
    for (const value of [null, 0, 'false', {}, []]) {
      assert.throws(() => new TriviumDB(file, { [field]: value }), invalid(field))
    }
  }
  for (const entry of fixture.runtime_integers) {
    for (const value of [null, '1', true, {}, [], -1, 0.5, NaN, Infinity]) {
      assert.throws(() => new ResourceRuntime({ [entry.node]: value }), invalid(entry.node))
    }
  }
  for (const field of ['memoryLimitMb', 'payloadCacheMb', 'payloadCacheEntryMb']) {
    assert.throws(() => new TriviumDB(file, { [field]: Number.MAX_SAFE_INTEGER }), invalid(field))
  }
  for (const field of ['memoryLimitMb', 'maintenanceMemoryLimitMb']) {
    assert.throws(() => new ResourceRuntime({ [field]: Number.MAX_SAFE_INTEGER }), invalid(field))
  }
  for (const value of [null, undefined, 1, {}, []]) assert.throws(() => new TriviumDB(value), invalid('path'))
  assert.throws(() => new ResourceRuntime({ maxThreads: 65 }), invalid('maxThreads'))
  assert.throws(() => new ResourceRuntime({ memoryLimitMb: 1, maintenanceMemoryLimitMb: 2 }), invalid('maintenanceMemoryLimitMb'))
  assert.throws(() => new ConsoleSession(1), invalid('optionsJson'))
  assert.throws(() => new ConsoleSession('{'), invalid('optionsJson'))
  assert.equal(new ConsoleSession(null).isDetached(), false)
  for (const value of [true, false, 'options', [], [1], () => {}, 1n, Symbol('options')]) {
    assert.throws(() => new ResourceRuntime(value), invalid('options'))
    assert.throws(() => new TriviumDB(file, value), invalid('options'))
  }
  for (const value of [0, 2, NaN, Infinity]) {
    assert.throws(() => new ResourceRuntime(value), invalid('options'))
    assert.throws(() => new TriviumDB(file, value), error => {
      assert.equal(error.code, 'TDB_API_MIGRATION_REQUIRED')
      assert.deepEqual(error.target, { field: 'options' })
      assert.deepEqual(error.details, { removed_api: 'new TriviumDB(path, dim)',
        replacement: 'new TriviumDB(path, { dim })' })
      assert.equal(error.stage, undefined)
      assert.equal(error.write, undefined)
      return true
    })
  }
  for (const value of [null, 1, 'runtime', true, [], {}, new ConsoleSession(),
    { __tdbResourceRuntime: 1 }, { __tdbResourceRuntime: {} }]) {
    assert.throws(() => new TriviumDB(file, { runtime: value }), invalid('runtime'))
    // 模块函数的显式 null 沿用可选实参的默认值规则。
    if (value !== null) {
      assert.throws(() => verifySnapshot('unused', value), invalid('runtime'))
      assert.throws(() => restoreSnapshot('unused', 'unused-destination', value),
        invalid('runtime', { phase: 'validation', outcome: 'not_committed' }))
    }
  }
  assert.equal(fs.existsSync(parent), false)
  let calls = 0
  const wrapped = { valueOf() { calls++; throw new Error('禁止构造数值强制转换') },
    toString() { calls++; throw new Error('禁止构造字符串强制转换') } }
  assert.throws(() => new TriviumDB(file, { dim: wrapped }), invalid('dim'))
  assert.throws(() => new TriviumDB(file, { dtype: wrapped }), invalid('dtype'))
  assert.equal(calls, 0)
  const cause = new Error('宿主 cause')
  const sentinel = Object.assign(new TypeError('构造 getter 原异常', { cause }), { code: 'HOST_GETTER' })
  assert.throws(() => new TriviumDB(file, { get dim() { calls++; throw sentinel } }), error => {
    assert.equal(error, sentinel)
    assert.equal(error.cause, cause)
    assert.equal(error.code, 'HOST_GETTER')
    assert.equal(error.target, undefined)
    return true
  })
  assert.equal(calls, 1)
  for (const action of [
    runtime => new TriviumDB(file, { runtime }),
    runtime => verifySnapshot('unused', runtime),
    runtime => restoreSnapshot('unused', 'unused-destination', runtime),
  ]) {
    let reads = 0
    const throwing = { get __tdbResourceRuntime() { reads++; throw sentinel } }
    assert.throws(() => action(throwing), error => error === sentinel && error.cause === cause && error.target === undefined)
    assert.equal(reads, 1)
    const proxy = new Proxy({}, { getOwnPropertyDescriptor() { throw sentinel } })
    assert.throws(() => action(proxy), error => error === sentinel && error.cause === cause)
  }
  for (const value of [undefined, null, Object.create(null), Object.create({ maxThreads: 2 })]) {
    const stats = new ResourceRuntime(value).stats()
    assert.equal(stats.attached_databases, 0)
    assert.equal(stats.reserved_database_bytes, 0)
  }
  const rejectedArray = []
  Object.defineProperty(rejectedArray, 'dim', { get() { throw sentinel } })
  assert.throws(() => new TriviumDB(file, rejectedArray), invalid('options'))
  assert.throws(() => new ResourceRuntime({ get maxThreads() { throw sentinel } }),
    error => error === sentinel && error.cause === cause)
  for (const [index, options] of [undefined, null].entries()) {
    const database = new TriviumDB(path.join(root, `constructor-default-${index}.tdb`), options)
    try {
      const vector = Array(1536).fill(0)
      vector[0] = 1
      database.insertWithId(7, vector, fixture.saved.payload)
      assert.deepEqual(database.allNodeIds(), [7])
      assert.deepEqual(database.get(7), { ...fixture.saved, vector, numEdges: 0, edges: [] })
    } finally { database.close() }
  }
  let inheritedReads = 0
  const configurations = [Object.assign(Object.create(null), { dim: 2 }),
    Object.create({ get dim() { inheritedReads++; return 2 } }),
    { dim: 2, get unknownOption() { throw sentinel } }]
  for (const [index, options] of configurations.entries()) {
    const database = new TriviumDB(path.join(root, `constructor-object-${index}.tdb`), options)
    try {
      database.insertWithId(7, fixture.saved.vector, fixture.saved.payload)
      assert.deepEqual(database.allNodeIds(), [7])
      assert.deepEqual(database.get(7), { ...fixture.saved, numEdges: 0, edges: [] })
    } finally { database.close() }
  }
  assert.equal(inheritedReads, 1)
  for (const dtype of ['f32', 'f16', 'u64']) for (const storageMode of ['rom', 'mmap']) {
    const runtime = new ResourceRuntime({ memoryLimitMb: 8, maintenanceMemoryLimitMb: 1, maxThreads: 2 })
    const options = { dim: 2, dtype, storageMode, autoBuildQuiver: false, loadTextIndex: true, memoryLimitMb: 1,
      payloadCacheMb: 0, payloadCacheEntryMb: 0, runtime }
    const directory = path.join(root, `constructor-${dtype}-${storageMode}`)
    let file = path.join(directory, 'database.tdb')
    function released(attached = 0) {
      const stats = runtime.stats()
      assert.equal(stats.attached_databases, attached)
      assert.equal(stats.reserved_database_bytes, attached * 1024 * 1024)
      assert.equal(stats.reserved_workspace_bytes, 0)
      assert.equal(stats.active_tasks, 0)
    }
    assert.throws(() => new TriviumDB(file, { ...options, memoryLimitMb: 9 }), error => {
      assert.equal(error.code, 'QueueFull')
      assert.deepEqual(error.details, { resource: 'memory_bytes', requested: 9 * 1024 * 1024,
        used: 0, limit: 7 * 1024 * 1024 })
      assert.equal(error.write, undefined)
      return true
    })
    released()
    assert.equal(fs.existsSync(directory), false)
    for (const dim of [0, 65537, Number.MAX_SAFE_INTEGER]) {
      assert.throws(() => new TriviumDB(file, { ...options, dim }), invalid('dim'))
      released()
      assert.equal(fs.existsSync(directory), false)
    }
    const db = new TriviumDB(file, options)
    let held, heldError
    try {
      db.insertWithId(fixture.saved.id, fixture.saved.vector, fixture.saved.payload)
      const wal = fs.readFileSync(file + '.wal')
      assert.throws(() => new TriviumDB(file, []), invalid('options'))
      assert.throws(() => new TriviumDB(file, { ...options, runtime: {} }), invalid('runtime'))
      assert.deepEqual(fs.readFileSync(file + '.wal'), wal)
      released(1)
      assert.throws(() => new TriviumDB(file, options), error => {
        assert.equal(error.code, 'TDB_DATABASE_LOCKED')
        heldError = error
        return true
      })
      released(1)
      held = db.get(7)
      const snapshot = directory + '-snapshot'
      const restored = directory + '-restored'
      db.createSnapshot(snapshot, 'runtime-containers')
      assert.equal(verifySnapshot(snapshot, null).node_count, 1)
      assert.equal(restoreSnapshot(snapshot, restored, null).generation_id, 'runtime-containers')
      const recovered = new TriviumDB(path.join(restored, 'database.tdb'), options)
      try {
        assert.deepEqual(recovered.allNodeIds(), [7])
        assert.deepEqual(recovered.get(7), { ...fixture.saved, numEdges: 0, edges: [] })
      } finally { recovered.close() }
      released(1)
    } finally { db.close() }
    released()
    const moved = directory + '-moved'
    fs.renameSync(directory, moved)
    assert.equal(heldError.code, 'TDB_DATABASE_LOCKED')
    const expected = { ...fixture.saved, numEdges: 0, edges: [] }
    assert.deepEqual(held, expected)
    file = path.join(moved, 'database.tdb')
    assert.throws(() => new TriviumDB(file, { ...options, dim: 3 }), error => error.code === 'TDB_DIMENSION_MISMATCH')
    released()
    for (let pass = 0; pass < 2; pass++) {
      const reader = new TriviumDB(file, options)
      try {
        assert.deepEqual(reader.allNodeIds(), [7])
        assert.deepEqual(reader.get(7), expected)
        reader.flush()
      } finally { reader.close() }
      released()
    }
  }
}

module.exports = { runConstructorArguments }
