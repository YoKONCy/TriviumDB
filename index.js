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
  const invoke = (database, original, arguments_, method) => {
    const depth = activeNativeCalls.get(database) || 0
    if (depth > 0) {
      const error = new Error('数据库回调重入被拒绝 (Database callback reentry rejected)')
      error.schema_version = 1
      error.code = 'TDB_REENTRANT_ACCESS'
      error.details = { operation: 'node_callback' }
      error.operation = error.details.operation
      if (method === 'commitTransaction') {
        error.write = { phase: 'admission', outcome: 'not_committed', transaction_consumed: false }
      }
      throw error
    }
    activeNativeCalls.set(database, depth + 1)
    try {
      return original.apply(database, arguments_)
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
      descriptor.value = function (...arguments_) {
        return invoke(this, original, arguments_, name)
      }
    }
    if (descriptor.get) {
      const original = descriptor.get
      descriptor.get = function () { return invoke(this, original, []) }
    }
    if (descriptor.set) {
      const original = descriptor.set
      descriptor.set = function (value) { return invoke(this, original, [value]) }
    }
    Object.defineProperty(prototype, name, descriptor)
  }
}

module.exports = binding
