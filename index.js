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

  // Console 调用借用整个原生实例；两个方向的 Hook 重入都必须在 N-API 前拒绝。
  // 普通调用只记录同步借用深度，保持既有入口之间的行为。
  const activeConsoleCalls = new WeakSet()
  const activeNativeCalls = new WeakMap()
  const prototype = binding.TriviumDB.prototype
  const invoke = (database, original, arguments_, consoleCall = false) => {
    const depth = activeNativeCalls.get(database) || 0
    if (activeConsoleCalls.has(database) || (consoleCall && depth > 0)) {
      const error = new Error('控制台与宿主数据库访问冲突 (Console and host database access conflict)')
      error.code = 'TDB_IO'
      error.details = { io_kind: 'WouldBlock', os_code: null }
      throw error
    }
    activeNativeCalls.set(database, depth + 1)
    if (consoleCall) activeConsoleCalls.add(database)
    try {
      return original.apply(database, arguments_)
    } finally {
      if (depth === 0) activeNativeCalls.delete(database)
      else activeNativeCalls.set(database, depth)
      if (consoleCall) activeConsoleCalls.delete(database)
    }
  }
  for (const name of Reflect.ownKeys(prototype)) {
    if (name === 'constructor') continue
    const descriptor = Object.getOwnPropertyDescriptor(prototype, name)
    if (typeof descriptor.value === 'function') {
      const original = descriptor.value
      descriptor.value = function (...arguments_) {
        return invoke(this, original, arguments_, name === 'consoleRequest')
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
