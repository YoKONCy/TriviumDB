'use strict'

// Node 原生模块加载器：只按当前平台、架构和 libc 选择已发布二进制。
// 未命中时返回包含平台信息的明确错误，不静默切换到不兼容 ABI。

const path = require('node:path')

const platform = process.platform
const arch = process.arch
const libc = platform === 'linux' && process.report?.getReport?.().header?.glibcVersionRuntime
  ? 'gnu'
  : 'musl'
const platformArch = platform === 'win32'
  ? `${platform}-${arch}-msvc`
  : platform === 'linux'
    ? `${platform}-${arch}-${libc}`
    : `${platform}-${arch}`
const candidates = [
  `triviumdb.${platformArch}.node`,
  `triviumdb.${platform}-${arch}.node`,
  'triviumdb.node',
]

let binding
let lastError
for (const candidate of candidates) {
  try {
    binding = require(path.join(__dirname, candidate))
    break
  } catch (error) {
    if (error?.code !== 'MODULE_NOT_FOUND') {
      throw error
    }
    lastError = error
  }
}

if (!binding) {
  throw new Error(
    `无法加载 TriviumDB 原生模块 (Unable to load TriviumDB native addon) for ${platformArch}`,
    { cause: lastError },
  )
}

// 必需位置参数由当前公开绑定契约登记，额外实参继续由原生调用规则处理。
const requiredArguments = {
  setHook: ['hook'], loadFfiHook: ['libPath'],
  searchWithContext: ['queryVector'],
  insert: ['vector', 'payload'], insertWithId: ['id', 'vector', 'payload'],
  upsertWithId: ['id', 'vector', 'payload'],
  batchInsert: ['vectors', 'payloads'], batchInsertWithIds: ['ids', 'vectors', 'payloads'],
  commitTransaction: ['operations'],
  get: ['id'], getPayload: ['id'], contains: ['id'], getEdges: ['id'],
  getIncomingEdges: ['id'], getEdge: ['src', 'dst', 'label'],
  updatePayload: ['id', 'payload'], patchPayload: ['id', 'patch'], updateVector: ['id', 'vector'],
  delete: ['id'], deleteManyAtomic: ['ids'],
  compareAndSetPayloadField: ['id', 'field', 'expected', 'replacement'],
  link: ['src', 'dst'], upsertEdge: ['src', 'dst', 'label', 'weight'],
  updateEdge: ['src', 'dst', 'label'], unlink: ['src', 'dst'],
  neighbors: ['id'], reachable: ['id'], reachableDetailed: ['id'], querySubgraph: ['id'],
  searchGraphFirst: ['queryVector', 'anchorIds', 'topK'], searchExact: ['queryVector', 'topK'],
  search: ['queryVector'], searchGrouped: ['queryVector'], searchBatch: ['queryVectors'],
  searchAdvanced: ['queryVector'], searchHybrid: ['queryVector', 'queryText'],
  indexText: ['id', 'text'], indexKeyword: ['id', 'keyword'], rebuildTextIndex: ['documents'],
  indexedLookup: ['equalities'], substringLookup: ['field', 'needle'],
  createIndex: ['field'], createOrderedIndex: ['field'], createCompositeIndex: ['fields'],
  createBitmapIndex: ['field'], createNgramIndex: ['field'], createUniqueIndex: ['field'],
  createUniqueCompositeIndex: ['fields'], dropIndex: ['field'], dropOrderedIndex: ['field'],
  dropCompositeIndex: ['fields'], dropBitmapIndex: ['field'], dropNgramIndex: ['field'],
  tql: ['query'], prepareTql: ['query'], executePreparedTql: ['prepared', 'parameters'],
  consoleRequest: ['session', 'requestJson'], documentRequest: ['requestJson'], graphRequest: ['requestJson'], quiverRequest: ['requestJson'], tqlMut: ['query'],
  publishGenerationManifest: ['generationId'], setSyncMode: ['mode'],
  setAutoBuildQuiver: ['enabled'], reserveNodes: ['additional'], setMemoryLimit: ['mb'],
  migrate: ['newPath', 'newDim'], closeTimeout: ['timeoutMs'],
  createSnapshot: ['destination', 'generationId'],
}
const callArgumentInfo = binding.__callArgumentInfo
const { AsyncLocalStorage } = require('node:async_hooks')
const requestContexts = new AsyncLocalStorage()
const invokeRequest = binding.__invokeRequest
const requestOperation = binding.__requestOperation
if (typeof binding.RequestContext !== 'function' || typeof invokeRequest !== 'function' || typeof requestOperation !== 'function') {
  throw new Error('原生请求上下文接口不可用，请重新构建绑定 (Native request context interface unavailable. Rebuild the binding)')
}
Object.defineProperty(binding, '__invokeRequest', { enumerable: false })
Object.defineProperty(binding, '__requestOperation', { enumerable: false })

Object.defineProperty(binding.RequestContext.prototype, 'run', {
  configurable: true,
  writable: true,
  value(operation) {
    if (typeof operation !== 'function') {
      const info = callArgumentInfo('RequestContext', 'run', 'operation', 'invalid_arguments')
      throw Object.assign(new TypeError(info.message), info, info.details)
    }
    const parent = requestContexts.getStore() || []
    if (parent.length >= 32) {
      const info = callArgumentInfo('RequestContext', 'run', 'operation', 'invalid_arguments')
      throw Object.assign(new TypeError(info.message), info, info.details)
    }
    return requestContexts.run([...parent, this], operation)
  },
})

function withRequest(owner, method, operation) {
  const contexts = requestContexts.getStore()
  if (!contexts?.length || !requestOperation(owner, method)) return operation()
  const call = index => index === contexts.length ? operation()
    : invokeRequest(contexts[index], owner, method, () => call(index + 1))
  return call(0)
}
if (typeof callArgumentInfo !== 'function') {
  throw new Error('原生诊断接口不可用，请重新构建绑定 (Native diagnostic interface unavailable. Rebuild the binding)')
}
Object.defineProperty(binding, '__callArgumentInfo', { enumerable: false })

function missingArgument(owner, method, field) {
  const operation = method.replace(/[A-Z]/g, letter => '_' + letter.toLowerCase())
  const info = callArgumentInfo(owner, operation, field, 'missing')
  const error = new TypeError(info.message)
  Object.assign(error, info, info.details)
  return error
}

for (const [name, parameters] of Object.entries({
  verifySnapshot: ['directory'], restoreSnapshot: ['source', 'destination'],
})) {
  const original = binding[name]
  binding[name] = function (...arguments_) {
    if (arguments_.length < parameters.length) {
      throw missingArgument('module', name, parameters[arguments_.length])
    }
    return withRequest('module', name.replace(/[A-Z]/g, letter => '_' + letter.toLowerCase()), () => original.apply(this, arguments_))
  }
  Object.defineProperty(binding[name], 'name', { value: original.name })
}

const consoleSessions = new WeakSet()
const preparedQueries = new WeakSet()
if (binding.ConsoleSession) {
  const NativeSession = binding.ConsoleSession
  binding.ConsoleSession = new Proxy(NativeSession, {
    construct(target, arguments_, newTarget) {
      const session = Reflect.construct(target, arguments_, newTarget)
      consoleSessions.add(session)
      return session
    },
  })
  const constructor = Object.getOwnPropertyDescriptor(NativeSession.prototype, 'constructor')
  Object.defineProperty(NativeSession.prototype, 'constructor', { ...constructor, value: binding.ConsoleSession })
  const original = binding.ConsoleSession.prototype.cancel
  binding.ConsoleSession.prototype.cancel = function (...arguments_) {
    if (arguments_.length === 0) throw missingArgument('ConsoleSession', 'cancel', 'requestId')
    return original.apply(this, arguments_)
  }
}

if (binding.TriviumDB) {
  Object.defineProperty(binding.TriviumDB.prototype, 'dispose', {
    configurable: true,
    value() {
      this.close()
    },
  })
  if (typeof Symbol.dispose === 'symbol') {
    Object.defineProperty(binding.TriviumDB.prototype, Symbol.dispose, {
      configurable: true,
      value() {
        this.close()
      },
    })
  }

  // 同步回调期间重复进入同一实例会形成原生借用或锁等待环，在 N-API 前拒绝。
  const activeNativeCalls = new WeakMap()
  const prototype = binding.TriviumDB.prototype
  const invoke = (database, original, arguments_, method, required) => {
    const depth = activeNativeCalls.get(database) || 0
    if (depth > 0) {
      const error = new Error('数据库回调重入被拒绝 (Database callback reentry rejected)')
      error.schema_version = 1
      error.code = 'TDB_REENTRANT_ACCESS'
      error.details = { operation: 'node_callback' }
      error.operation = error.details.operation
      const operation = typeof method === 'string' ? method.replace(/[A-Z]/g, letter => '_' + letter.toLowerCase()) : ''
      const write = callArgumentInfo('TriviumDB', operation, 'arguments', 'invalid_arguments').write
      if (write) {
        error.write = { ...write, phase: 'admission' }
      }
      throw error
    }
    if (required && arguments_.length < required.length) {
      throw missingArgument('TriviumDB', method, required[arguments_.length])
    }
    activeNativeCalls.set(database, depth + 1)
    try {
      if (method === 'consoleRequest' && !consoleSessions.has(arguments_[0])) {
        const reason = 'session 必须是有效的 ConsoleSession 句柄 (session must be a valid ConsoleSession handle)'
        throw Object.assign(new TypeError(reason), { schema_version: 1, code: 'TDB_INVALID_INPUT',
          details: { reason }, target: { field: 'session' } })
      }
      if (method === 'executePreparedTql' && !preparedQueries.has(arguments_[0])) {
        const reason = 'prepared 必须是有效的 PreparedTql 句柄 (prepared must be a valid PreparedTql handle)'
        throw Object.assign(new TypeError(reason), { schema_version: 1, code: 'TDB_INVALID_INPUT',
          details: { reason }, target: { field: 'prepared' } })
      }
      const result = withRequest('TriviumDB', typeof method === 'string' ? method.replace(/[A-Z]/g, letter => '_' + letter.toLowerCase()) : '', () => original.apply(database, arguments_))
      if (method === 'prepareTql') preparedQueries.add(result)
      return result
    } catch (error) {
      if (method === 'searchBatch' && ['TDB_QUERY_CANCELLED', 'TDB_QUERY_DEADLINE_EXCEEDED'].includes(error?.code)) return Promise.reject(error)
      throw error
    } finally {
      if (depth === 0) activeNativeCalls.delete(database)
      else activeNativeCalls.set(database, depth)
    }
  }
  for (const name of Reflect.ownKeys(prototype)) {
    // 资源释放别名转调已受保护的 close，避免把 SDK 自身转发识别为回调重入。
    if (name === 'constructor' || name === 'dispose' || name === Symbol.dispose) continue
    const descriptor = Object.getOwnPropertyDescriptor(prototype, name)
    if (typeof descriptor.value === 'function') {
      const original = descriptor.value
      const required = requiredArguments[name]
      descriptor.value = function (...arguments_) {
        return invoke(this, original, arguments_, name, required)
      }
    }
    if (descriptor.get) {
      const original = descriptor.get
      descriptor.get = function () { return invoke(this, original, [], name) }
    }
    if (descriptor.set) {
      const original = descriptor.set
      descriptor.set = function (value) { return invoke(this, original, [value]) }
    }
    Object.defineProperty(prototype, name, descriptor)
  }
}

// 构造准入保留原生原型和 newTarget，缺参在原生转换之前定位。
const NativeDatabase = binding.TriviumDB
const DatabaseConstructor = new Proxy(NativeDatabase, {
  construct(target, arguments_, newTarget) {
    if (arguments_.length === 0) throw missingArgument('TriviumDB', '__new__', 'path')
    return Reflect.construct(target, arguments_, newTarget)
  },
})
const constructorDescriptor = Object.getOwnPropertyDescriptor(NativeDatabase.prototype, 'constructor')
Object.defineProperty(NativeDatabase.prototype, 'constructor', { ...constructorDescriptor, value: DatabaseConstructor })
binding.TriviumDB = DatabaseConstructor

module.exports = binding
